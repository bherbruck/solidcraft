//! Delete Face with healing: faces are removed and their planar neighbours extend to close the
//! gap, as when a fillet, chamfer, boss or hole is taken off a part.
//!
//! The body is rebuilt from its own topology. Faces away from the gap are kept as they are.
//! Each neighbouring face (planar) keeps its edges that don't touch the gap; a loop that ran
//! entirely along deleted faces is dropped for a hole's rim or a boss's footprint, or restored
//! at the moved corners for a face enclosed by rounds. A corner on the
//! gap moves to where its planes meet the neighbours across the gap (a fillet's tangent points
//! go to the sharp corner), and the stretch of loop that bordered deleted faces closes with a
//! straight edge between the moved corners. Anything else (curved neighbours, a gap the
//! neighbours cannot close) is refused, naming the face.

use std::collections::{BTreeSet, HashMap};

use solidcraft_geom::Vec3;
use truck_modeling::{self as mt, builder};

use crate::body::{Body, Solid, from_p3, p3};
use crate::{KernelError, Result, guard};

fn fail(m: impl Into<String>) -> KernelError {
    KernelError::Invalid(format!("Delete Face: {}", m.into()))
}

/// Outward plane (unit normal, offset) of a face, if it is planar: sampled along its edges,
/// the surface's outward normal stays the same and the points stay on one plane.
fn face_plane(f: &mt::Face, tol: f64) -> Option<(Vec3, f64)> {
    use mt::{BoundedCurve, ParametricCurve, ParametricSurface3D, SearchNearestParameter};
    let surf = f.oriented_surface();
    let mut pts: Vec<(Vec3, Vec3)> = Vec::new();
    let mut last: Option<(f64, f64)> = None;
    for w in f.boundaries() {
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            for k in 0..6 {
                let p = c.subs(t0 + (t1 - t0) * k as f64 / 6.0);
                let (u, v) = surf.search_nearest_parameter(p, last, 50)?;
                last = Some((u, v));
                let n = surf.normal(u, v);
                pts.push((from_p3(p), Vec3::new(n.x, n.y, n.z).normalized()?));
            }
        }
    }
    let (p0, n0) = *pts.first()?;
    let d = n0.dot(p0);
    pts.iter().all(|(p, n)| n.dot(n0) > 1.0 - 1e-9 && (n0.dot(*p) - d).abs() <= tol).then_some((n0, d))
}

type Key = (i64, i64, i64);

/// Key of a point for sharing vertices and edges between rebuilt faces.
fn key(p: Vec3, q: f64) -> Key {
    let r = |x: f64| (x / q).round() as i64;
    (r(p.x), r(p.y), r(p.z))
}

/// Delete faces (indices in the body's face order, as in its mesh's `tri_face`) and heal the
/// gap by extending the neighbouring planes. The result's history is over `[body]`.
pub fn delete_faces(body: &Body, faces: &[usize]) -> Result<Body> {
    body.require_brep("Delete Face")?;
    let all: Vec<mt::Face> = body.solid.face_iter().cloned().collect();
    let n = all.len();
    if faces.is_empty() {
        return Err(fail("no faces given"));
    }
    let del: BTreeSet<usize> = faces.iter().copied().collect();
    if let Some(bad) = del.iter().find(|f| **f >= n) {
        return Err(fail(format!("there is no face {bad} (the body has {n})")));
    }
    if del.len() >= n {
        return Err(fail("that would delete every face"));
    }
    let size = body.size().max(1e-9);
    // Edge and vertex → faces.
    let mut efaces: HashMap<mt::EdgeID, Vec<usize>> = HashMap::new();
    let mut vfaces: HashMap<mt::VertexID, Vec<usize>> = HashMap::new();
    for (i, f) in all.iter().enumerate() {
        for e in f.edge_iter() {
            let v = efaces.entry(e.id()).or_default();
            if !v.contains(&i) {
                v.push(i);
            }
        }
        for v in f.vertex_iter() {
            let l = vfaces.entry(v.id()).or_default();
            if !l.contains(&i) {
                l.push(i);
            }
        }
    }
    // Neighbours: faces sharing an edge with a deleted face; they must be planar.
    let mut neighbours: BTreeSet<usize> = BTreeSet::new();
    for fs in efaces.values() {
        if fs.iter().any(|f| del.contains(f)) {
            neighbours.extend(fs.iter().filter(|f| !del.contains(f)));
        }
    }
    if neighbours.is_empty() {
        return Err(fail("the faces don't border the rest of the body"));
    }
    let mut planes: HashMap<usize, (Vec3, f64)> = HashMap::new();
    for &f in &neighbours {
        // Planar by its geometry (boolean results may carry a plane as another surface type).
        match all.get(f).and_then(|x| face_plane(x, size * 1e-6)) {
            Some(p) => {
                planes.insert(f, p);
            }
            None => {
                let by = efaces
                    .values()
                    .find(|fs| fs.contains(&f) && fs.iter().any(|x| del.contains(x)))
                    .and_then(|fs| fs.iter().find(|x| del.contains(x)).copied())
                    .unwrap_or(0);
                return Err(fail(format!("face {by} borders face {f}, which is curved; only planar neighbours can extend to close the gap yet")));
            }
        }
    }
    // Corners across the gap: three neighbour planes meeting near the deleted faces.
    let (mut lo, mut hi) =
        (Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY));
    for &f in &del {
        for v in all.get(f).into_iter().flat_map(|x| x.vertex_iter()) {
            let p = from_p3(v.point());
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    let reach = (hi - lo).len().max(size * 1e-3);
    let near = |p: Vec3| {
        p.x >= lo.x - reach && p.y >= lo.y - reach && p.z >= lo.z - reach && p.x <= hi.x + reach && p.y <= hi.y + reach && p.z <= hi.z + reach
    };
    let on = |p: Vec3, f: usize| planes.get(&f).is_some_and(|(n, d)| (n.dot(p) - d).abs() <= size * 1e-6);
    let nb: Vec<usize> = neighbours.iter().copied().collect();
    let mut corners: Vec<Vec3> = Vec::new();
    for i in 0..nb.len() {
        for j in (i + 1)..nb.len() {
            for k in (j + 1)..nb.len() {
                let (Some(a), Some(b), Some(c)) =
                    (nb.get(i).and_then(|x| planes.get(x)), nb.get(j).and_then(|x| planes.get(x)), nb.get(k).and_then(|x| planes.get(x)))
                else {
                    continue;
                };
                let den = a.0.dot(b.0.cross(c.0));
                if den.abs() < 1e-9 {
                    continue;
                }
                let x = (b.0.cross(c.0) * a.1 + c.0.cross(a.0) * b.1 + a.0.cross(b.0) * c.1) / den;
                if x.is_finite() && near(x) {
                    corners.push(x);
                }
            }
        }
    }
    // Where each vertex on the gap goes: the nearest corner on all of its remaining planes.
    let mut moved: HashMap<mt::VertexID, Vec3> = HashMap::new();
    for &f in &del {
        for v in all.get(f).into_iter().flat_map(|x| x.vertex_iter()) {
            let Some(fs) = vfaces.get(&v.id()) else { continue };
            let keep: Vec<usize> = fs.iter().copied().filter(|x| !del.contains(x)).collect();
            if keep.is_empty() || keep.len() >= 3 {
                continue;
            }
            if keep.iter().any(|x| !planes.contains_key(x)) {
                return Err(fail(format!("a corner of face {f} touches a face that cannot move")));
            }
            let p = from_p3(v.point());
            let best = corners.iter().filter(|c| keep.iter().all(|x| on(**c, *x))).min_by(|a, b| a.dist(p).total_cmp(&b.dist(p)));
            if let Some(c) = best {
                moved.insert(v.id(), *c);
            }
        }
    }
    let q = size * 1e-7;
    guard("delete face", || {
        // Shared vertices and new straight edges between the rebuilt faces.
        let mut verts: HashMap<Key, mt::Vertex> = HashMap::new();
        for v in body.solid.vertex_iter() {
            if !moved.contains_key(&v.id()) {
                verts.insert(key(from_p3(v.point()), q), v.clone());
            }
        }
        let mut lines: HashMap<(Key, Key), mt::Edge> = HashMap::new();
        let mut line = |a: Vec3, b: Vec3, verts: &mut HashMap<Key, mt::Vertex>| -> mt::Edge {
            let (ka, kb) = (key(a, q), key(b, q));
            if let Some(e) = lines.get(&(ka, kb)) {
                return e.clone();
            }
            if let Some(e) = lines.get(&(kb, ka)) {
                return e.inverse();
            }
            let va = verts.entry(ka).or_insert_with(|| builder::vertex(p3(a))).clone();
            let vb = verts.entry(kb).or_insert_with(|| builder::vertex(p3(b))).clone();
            let e = builder::line(&va, &vb);
            lines.insert((ka, kb), e.clone());
            e
        };
        // Neighbours rebuilt from themselves, moved edges pieces of themselves; the edges that
        // close the gap are new.
        let mut rec = crate::record::Rebuild::default();
        let mut out: Vec<mt::Face> = Vec::new();
        for (fi, f) in all.iter().enumerate() {
            if del.contains(&fi) {
                continue;
            }
            if !neighbours.contains(&fi) {
                // Untouched, unless one of its corners moves.
                if f.vertex_iter().any(|v| moved.contains_key(&v.id())) {
                    return Err(fail(format!("face {fi} meets the gap only at a corner; it cannot follow")));
                }
                out.push(f.clone());
                continue;
            }
            let pos = |v: &mt::Vertex| moved.get(&v.id()).copied().unwrap_or_else(|| from_p3(v.point()));
            let gap = |e: &mt::Edge| efaces.get(&e.id()).is_some_and(|fs| fs.iter().any(|x| del.contains(x)));
            // A face that only loses whole loops (a hole's rim, a pocket's opening) and has no
            // corner moving keeps its own surface: rebuilding a plane from its outer boundary
            // fails when imported edges are not exactly on one plane.
            let mut stays = f.vertex_iter().all(|v| !moved.contains_key(&v.id()));
            let mut kept: Vec<mt::Wire> = Vec::new();
            for w in f.absolute_boundaries() {
                let gaps = w.edge_iter().filter(|e| gap(e)).count();
                if gaps == 0 {
                    kept.push(w.clone());
                } else if gaps != w.len() {
                    stays = false;
                }
            }
            if stays && !kept.is_empty() {
                let mut face = mt::Face::try_new(kept, f.surface()).map_err(|e| fail(format!("face {fi} cannot close: {e}")))?;
                if !f.orientation() {
                    face.invert();
                }
                rec.face(&face, f);
                out.push(face);
                continue;
            }
            let mut wires: Vec<mt::Wire> = Vec::new();
            for w in f.boundaries() {
                let edges: Vec<mt::Edge> = w.edge_iter().cloned().collect();
                if edges.iter().all(gap) {
                    // A hole or boss footprint disappears when its walls are removed. A
                    // face surrounded by rounds does not: its whole boundary moves back to
                    // the intersections of the remaining planes (#1557). Keep that loop,
                    // sharing its restored edges with the adjacent rebuilt faces.
                    let moving = edges.iter().any(|e| moved.contains_key(&e.front().id()) || moved.contains_key(&e.back().id()));
                    if moving {
                        if !edges.iter().all(|e| moved.contains_key(&e.front().id()) && moved.contains_key(&e.back().id())) {
                            return Err(fail(format!("face {fi} has a closed gap boundary that cannot be restored completely")));
                        }
                        let mut boundary = Vec::new();
                        for e in &edges {
                            let (a, b) = (pos(e.front()), pos(e.back()));
                            if a.dist(b) > q * 10.0 {
                                let restored = line(a, b, &mut verts);
                                rec.piece(&restored, e);
                                boundary.push(restored);
                            }
                        }
                        if !boundary.is_empty() {
                            wires.push(boundary.into());
                        }
                    }
                    continue;
                }
                // Start after a gap stretch so stretches don't wrap around.
                let m = edges.len();
                let start = (0..m).find(|i| edges.get((i + m - 1) % m).is_some_and(gap) && edges.get(*i).is_some_and(|e| !gap(e))).unwrap_or(0);
                let mut new_edges: Vec<mt::Edge> = Vec::new();
                // The moved corners along a gap stretch (two rounds meeting in a mitre leave
                // the sharp corner between them).
                let mut pending: Option<Vec<Vec3>> = None;
                let close = |pts: Vec<Vec3>, a: Vec3| -> Vec<(Vec3, Vec3)> {
                    let mut c: Vec<Vec3> = Vec::new();
                    for p in pts.into_iter().chain(std::iter::once(a)) {
                        if c.last().is_some_and(|l| l.dist(p) <= q * 10.0) {
                            continue;
                        }
                        // Drop a corner in line with its neighbours.
                        if let [.., x, y] = c[..]
                            && (y - x).cross(p - y).len() <= q * 10.0 * (p - x).len()
                            && (y - x).dot(p - y) > 0.0
                        {
                            c.pop();
                        }
                        c.push(p);
                    }
                    c.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).collect()
                };
                for k in 0..m {
                    let Some(e) = edges.get((start + k) % m) else { continue };
                    if gap(e) {
                        pending.get_or_insert_with(Vec::new).extend([pos(e.front()), pos(e.back())]);
                        continue;
                    }
                    let (a, b) = (pos(e.front()), pos(e.back()));
                    if let Some(p) = pending.take() {
                        for (x, y) in close(p, a) {
                            new_edges.push(line(x, y, &mut verts));
                        }
                    }
                    let moves = moved.contains_key(&e.front().id()) || moved.contains_key(&e.back().id());
                    if !moves {
                        new_edges.push(e.clone());
                    } else if matches!(e.oriented_curve(), mt::Curve::Line(_)) {
                        if a.dist(b) > q * 10.0 {
                            let moved = line(a, b, &mut verts);
                            rec.piece(&moved, e);
                            new_edges.push(moved);
                        }
                    } else {
                        return Err(fail(format!("face {fi} has a curved edge that would have to stretch")));
                    }
                }
                // A gap stretch at the end closes back to the first edge.
                if let (Some(p), Some(first)) = (pending, new_edges.first()) {
                    let a = from_p3(first.front().point());
                    for (x, y) in close(p, a) {
                        new_edges.push(line(x, y, &mut verts));
                    }
                }
                if new_edges.is_empty() {
                    return Err(fail(format!("face {fi} would lose its boundary")));
                }
                wires.push(new_edges.into());
            }
            let face = builder::try_attach_plane(&wires).map_err(|e| fail(format!("face {fi} cannot close: {e}")))?;
            // Keep the face's outward side.
            let (nrm, _) = planes.get(&fi).copied().ok_or_else(|| fail("plane"))?;
            let face = match face.surface() {
                mt::Surface::Plane(pl) => {
                    let pn = pl.normal();
                    let along = Vec3::new(pn.x, pn.y, pn.z).dot(nrm);
                    if (along > 0.0) == face.orientation() { face } else { face.inverse() }
                }
                _ => face,
            };
            rec.face(&face, f);
            out.push(face);
        }
        let shell: mt::Shell = out.into();
        let solid = Solid::try_new(vec![shell]).map_err(|e| fail(format!("the neighbouring faces do not close the gap ({e})")))?;
        crate::record::rebuilt(solid, &[&body.solid], &rec)
    })
}

#[cfg(test)]
mod tests {
    use solidcraft_geom::Vec3;

    use super::delete_faces;
    use crate::{BoolOp, boolean, box_solid, chamfer, cylinder, fillet};

    fn volume(b: &crate::Body) -> f64 {
        b.tessellate(1e-3).unwrap().measure().volume
    }

    /// The faces of `b` with every triangle corner passing `keep`.
    fn faces_where(b: &crate::Body, keep: impl Fn(Vec3) -> bool) -> Vec<usize> {
        let m = b.tessellate(1e-3).unwrap();
        let n = m.tri_face.iter().max().map(|x| *x as usize + 1).unwrap_or(0);
        (0..n)
            .filter(|f| {
                m.triangles.iter().zip(&m.tri_face).filter(|(_, x)| **x as usize == *f).all(|(t, _)| m.tri(t).unwrap().iter().all(|p| keep(*p)))
            })
            .collect()
    }

    #[test]
    fn issue1557_delete_closed_fillet_ring_keeps_the_floor() {
        let b = box_solid(Vec3::ZERO, Vec3::new(25.0, 25.0, 10.0)).unwrap();
        let picks = [Vec3::new(12.5, 0.0, 0.0), Vec3::new(25.0, 12.5, 0.0), Vec3::new(12.5, 25.0, 0.0), Vec3::new(0.0, 12.5, 0.0)];
        let rounded = fillet(&b, &picks, 2.0).unwrap();
        let rounds: Vec<usize> =
            rounded.solid.face_iter().enumerate().filter(|(_, f)| crate::blend::plane_normal(f).is_none()).map(|(i, _)| i).collect();
        assert_eq!(rounds.len(), 4);
        let restored = delete_faces(&rounded, &rounds).unwrap();
        assert_eq!(restored.face_count(), 6);
        assert_eq!(restored.unmeshed_faces(), 0);
        assert!((volume(&restored) - 6250.0).abs() < 1e-6);
        assert!(restored.contains(Vec3::new(0.1, 0.1, 0.1)));
    }

    #[test]
    fn delete_pocket_keeps_an_imported_face_around_it() {
        use truck_modeling as mt;
        // A shallow round pocket in the top of a plate whose top edges bow 5e-5 mm off the
        // plane, as the edges of an imported part do: within the planar test, but too far for a
        // plane rebuilt from them. The top only loses its inner loop, so it keeps its surface.
        let plate = box_solid(Vec3::ZERO, Vec3::new(100.0, 100.0, 10.0)).unwrap();
        let tool = cylinder(Vec3::new(50.0, 50.0, 9.5), Vec3::Z, 7.5, 2.0).unwrap();
        let pocketed = boolean(&plate, &tool, BoolOp::Cut).unwrap().unwrap();
        let pocket = faces_where(&pocketed, |p| (p.x - 50.0).hypot(p.y - 50.0) < 7.5 + 1e-3 && p.z > 9.5 - 1e-6);
        assert!(!pocket.is_empty());
        let mut bowed = 0;
        for e in pocketed.solid.edge_iter() {
            let (a, b) = (e.front().point(), e.back().point());
            if let mt::Curve::Line(_) = e.curve()
                && (a.z - 10.0).abs() < 1e-9
                && (b.z - 10.0).abs() < 1e-9
            {
                let lift = mt::Vector3::new(0.0, 0.0, 1e-4);
                let mid = mt::Point3::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0, 10.0) + lift;
                let c = if e.orientation() { vec![a, mid, b] } else { vec![b, mid, a] };
                e.set_curve(mt::Curve::BSplineCurve(mt::BSplineCurve::new(mt::KnotVec::bezier_knot(2), c)));
                bowed += 1;
            }
        }
        assert_eq!(bowed, 4);
        let filled = delete_faces(&pocketed, &pocket).unwrap();
        assert_eq!(filled.face_count(), 6);
        assert_eq!(filled.unmeshed_faces(), 0);
        assert!((volume(&filled) - 100000.0).abs() < 1e-3, "{}", volume(&filled));
    }

    #[test]
    fn delete_fillet_chamfer_hole_and_boss_faces() {
        let sharp = 24000.0;
        let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
        // A fillet on a box edge: deleting it gives the sharp box back, exactly.
        let f = fillet(&b, &[Vec3::new(20.0, 30.0, 20.0)], 5.0).unwrap();
        assert!(volume(&f) < sharp - 1.0);
        let fi = faces_where(&f, |p| p.y > 25.0 - 1e-6 && p.z > 15.0 - 1e-6 && (p.y < 30.0 - 1e-9 || p.z < 20.0 - 1e-9));
        assert_eq!(fi.len(), 1, "{fi:?}");
        let back = delete_faces(&f, &fi).unwrap();
        assert!((volume(&back) - sharp).abs() < 1e-9 * sharp, "{}", volume(&back));
        assert_eq!(back.face_count(), 6);
        // A chamfer likewise.
        let c = chamfer(&b, &[Vec3::new(20.0, 30.0, 20.0)], 4.0).unwrap();
        let ci = faces_where(&c, |p| p.y > 26.0 - 1e-6 && p.z > 16.0 - 1e-6 && (p.y < 30.0 - 1e-9 || p.z < 20.0 - 1e-9));
        let back = delete_faces(&c, &ci).unwrap();
        assert!((volume(&back) - sharp).abs() < 1e-9 * sharp, "{}", volume(&back));
        // A through hole: deleting its wall fills it.
        let plate = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
        let pin = cylinder(Vec3::new(20.0, 15.0, -1.0), Vec3::Z, 4.0, 12.0).unwrap();
        let holed = boolean(&plate, &pin, BoolOp::Cut).unwrap().unwrap();
        let wall = faces_where(&holed, |p| ((p.x - 20.0).hypot(p.y - 15.0) - 4.0).abs() < 1e-2);
        assert!(!wall.is_empty());
        let filled = delete_faces(&holed, &wall).unwrap();
        assert!((volume(&filled) - 12000.0).abs() < 1e-9 * 12000.0, "{}", volume(&filled));
        // A blind hole: its wall and floor.
        let drill = cylinder(Vec3::new(20.0, 15.0, 4.0), Vec3::Z, 3.0, 7.0).unwrap();
        let blind = boolean(&plate, &drill, BoolOp::Cut).unwrap().unwrap();
        let hole = faces_where(&blind, |p| (p.x - 20.0).hypot(p.y - 15.0) < 3.0 + 1e-3 && p.z > 4.0 - 1e-6);
        let filled = delete_faces(&blind, &hole).unwrap();
        assert!((volume(&filled) - 12000.0).abs() < 1e-9 * 12000.0, "{}", volume(&filled));
        // A boss on the plate: deleting its wall and top takes it off.
        let boss = cylinder(Vec3::new(10.0, 10.0, 10.0), Vec3::Z, 3.0, 5.0).unwrap();
        let bossed = boolean(&plate, &boss, BoolOp::Union).unwrap().unwrap();
        let bf = faces_where(&bossed, |p| p.z > 10.0 - 1e-9 && (p.x - 10.0).hypot(p.y - 10.0) < 3.0 + 1e-6);
        let off = delete_faces(&bossed, &bf).unwrap();
        assert!((volume(&off) - 12000.0).abs() < 1e-9 * 12000.0, "{}", volume(&off));
        // Errors name what is wrong.
        assert!(delete_faces(&plate, &[99]).is_err());
        assert!(delete_faces(&plate, &[0, 1, 2, 3, 4, 5]).is_err());
        // One face of a box: the sides cannot close it.
        let top = faces_where(&plate, |p| (p.z - 10.0).abs() < 1e-9);
        assert!(delete_faces(&plate, &top).is_err());
    }
}
