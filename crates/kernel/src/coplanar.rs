//! Booleans of bodies with coincident planar faces, by moving faces apart first.
//!
//! The mesh-based intersection cannot handle faces that lie in the same plane, nor the edges
//! lying in them. A face whose neighbours stand perpendicular to it (any extruded or
//! box-like part) can be pushed along its normal exactly, by rebuilding it and stretching or
//! shrinking its neighbours. Then:
//! - cut and intersect: the tool's coincident face moves out of the way (outward when the
//!   bodies are on the same side of the plane, away from the target when they touch back to
//!   back), which leaves the result unchanged;
//! - union of faces facing the same way: both faces move outward by different amounts, the
//!   union is taken, and the slab beyond the plane is cut off (now a transversal cut);
//! - union of faces back to back: the tool's face moves into the target, which overlaps it.
//!
//! Each of these leaves the result unchanged only where the space a face moves through holds
//! what the boolean leaves there anyway: none of the target for a cut or an intersection, the
//! target's material for a union, nothing of either body beyond a plane whose slab is cut off
//! again. The faces of a coincident plane lie on each other, but nothing says that what lies
//! beside them does: a tool's side in the face of a wall has the neighbouring walls beside it,
//! a channel's top in the face a rib stands on has the rib over it. So a face is asked first
//! what it would move through: a join's face moving into the target at points behind it
//! (`backed`), the others on the other body's faces (see [`crate::clearance`]). A face with
//! no room stays where it is and the boolean fails with [`NO_ROOM`], for the strategies that
//! take the bodies as they lie.

use std::collections::HashMap;

use solidcraft_geom::{Aabb3, Vec3};
use truck_modeling::{self as mt, builder};

use crate::body::shell_groups::boxes_separate;
use crate::body::{Body, Solid, from_p3, p3, v3};
use crate::clearance::{Clearance, Occupancy};
use crate::history::{Change, EdgeOrigin, FaceOrigin, FaceProv, History};
use crate::{BoolOp, KernelError, Result, guard, record};

#[derive(Clone, Copy, Debug)]
struct PlaneFace {
    n: Vec3,
    d: f64,
}

fn planes(b: &Body) -> Vec<(usize, PlaneFace)> {
    b.solid
        .face_iter()
        .enumerate()
        .filter_map(|(i, f)| match f.oriented_surface() {
            mt::Surface::Plane(p) => {
                let n = p.normal();
                let n = Vec3::new(n.x, n.y, n.z).normalized()?;
                Some((i, PlaneFace { n, d: n.dot(from_p3(p.origin())) }))
            }
            _ => None,
        })
        .collect()
}

/// Push the planar faces of `b` lying in the plane (n, d) — n their outward normal — along n
/// by `delta` (negative: inward). Their neighbours must stand perpendicular to the plane. The
/// result's history is over `[b]`.
pub(crate) fn push_faces(b: &Body, n: Vec3, d: f64, delta: f64) -> Result<Body> {
    // The exact rebuild, unless it came out unsound (a curved side it couldn't re-sweep).
    match push_faces_exact(b, n, d, delta)
        .and_then(|r| if r.validity().is_empty() { Ok(r) } else { Err(KernelError::Failed("pushing a face: an unsound result".into())) })
    {
        Ok(r) => Ok(r),
        // Faces of planes and cylinders: every face follows (any neighbours).
        Err(e) => {
            let size = b.size();
            let tol = (size * 1e-7).max(1e-9) * 10.0;
            let planes: Vec<bool> = b
                .solid
                .face_iter()
                .map(|f| match f.oriented_surface() {
                    mt::Surface::Plane(p) => {
                        let m = p.normal();
                        let m = Vec3::new(m.x, m.y, m.z);
                        m.dot(n) > 1.0 - 1e-9 && (n.dot(from_p3(p.origin())) - d).abs() < tol
                    }
                    _ => false,
                })
                .collect();
            crate::offset::offset_body(b, |i, _| if planes.get(i).copied().unwrap_or(false) { delta } else { 0.0 }).map_err(|_| e)
        }
    }
}

pub(crate) fn push_faces_exact(b: &Body, n: Vec3, d: f64, delta: f64) -> Result<Body> {
    let size = b.size();
    let tol = (size * 1e-7).max(1e-9);
    let fail = |m: &str| KernelError::Failed(format!("pushing a face: {m}"));
    guard("push face", || {
        let solid = b.deep_copy();
        // The copy's edges are b's, in order; the faces come out in place, one for one.
        let mut edge_origins = record::edge_origins(&solid, 0);
        let mut faces_from = Vec::new();
        let shift = n * delta;
        let mat = mt::Matrix4::from_translation(v3(shift));
        let mut shells = Vec::new();
        for sh in solid.boundaries() {
            let faces: Vec<mt::Face> = sh.face_iter().cloned().collect();
            let on_plane = |f: &mt::Face| match f.oriented_surface() {
                mt::Surface::Plane(p) => {
                    let m = p.normal();
                    let m = Vec3::new(m.x, m.y, m.z);
                    m.dot(n) > 1.0 - 1e-9 && (n.dot(from_p3(p.origin())) - d).abs() < tol * 10.0
                }
                _ => false,
            };
            let moving: Vec<usize> = (0..faces.len()).filter(|i| faces.get(*i).is_some_and(on_plane)).collect();
            let first = faces_from.len();
            if moving.is_empty() {
                faces_from.extend((first..first + faces.len()).map(|i| record::from_face(0, i, Change::Kept)));
                shells.push(sh.clone());
                continue;
            }
            // Vertices and edges of the moving faces.
            let mut vmap: HashMap<mt::VertexID, mt::Vertex> = HashMap::new();
            let mut emap: HashMap<mt::EdgeID, mt::Edge> = HashMap::new();
            for i in &moving {
                let f = faces.get(*i).ok_or_else(|| fail("face"))?;
                for v in f.vertex_iter() {
                    vmap.entry(v.id()).or_insert_with(|| builder::vertex(p3(from_p3(v.point()) + shift)));
                }
            }
            for i in &moving {
                let f = faces.get(*i).ok_or_else(|| fail("face"))?;
                for e in f.edge_iter() {
                    if emap.contains_key(&e.id()) {
                        continue;
                    }
                    let (a, bb) = (
                        vmap.get(&e.absolute_front().id()).ok_or_else(|| fail("vertex"))?,
                        vmap.get(&e.absolute_back().id()).ok_or_else(|| fail("vertex"))?,
                    );
                    let curve = mt::Transformed::transformed(&e.curve(), mat);
                    emap.insert(e.id(), mt::Edge::new(a, bb, curve));
                }
            }
            // Edges leaving the plane at moved vertices: straight, along n.
            for f in &faces {
                for e in f.edge_iter() {
                    if emap.contains_key(&e.id()) {
                        continue;
                    }
                    let (fa, fb) = (e.absolute_front(), e.absolute_back());
                    let (ma, mb) = (vmap.get(&fa.id()), vmap.get(&fb.id()));
                    if ma.is_none() && mb.is_none() {
                        continue;
                    }
                    if !matches!(e.curve(), mt::Curve::Line(_)) {
                        return Err(fail("a neighbouring edge is not straight"));
                    }
                    let dir = (from_p3(fb.point()) - from_p3(fa.point())).normalized().ok_or_else(|| fail("zero edge"))?;
                    if dir.cross(n).len() > 1e-9 {
                        return Err(fail("a neighbouring face is not perpendicular"));
                    }
                    let (na, nb) = (ma.cloned().unwrap_or_else(|| fa.clone()), mb.cloned().unwrap_or_else(|| fb.clone()));
                    if (from_p3(nb.point()) - from_p3(na.point())).dot(dir) <= tol {
                        return Err(fail("the face would pass its opposite side"));
                    }
                    emap.insert(e.id(), builder::line(&na, &nb));
                }
            }
            let rebuild_wires = |f: &mt::Face| -> Vec<mt::Wire> {
                f.absolute_boundaries()
                    .iter()
                    .map(|w| {
                        w.edge_iter()
                            .map(|e| match emap.get(&e.id()) {
                                Some(ne) => {
                                    if e.front() == e.absolute_front() {
                                        ne.clone()
                                    } else {
                                        ne.inverse()
                                    }
                                }
                                None => e.clone(),
                            })
                            .collect::<Vec<_>>()
                            .into()
                    })
                    .collect()
            };
            // Each moved or stretched edge stands for the one it replaces.
            for (old, new) in &emap {
                if let Some(o) = edge_origins.get(old).cloned() {
                    edge_origins.insert(new.id(), o);
                }
            }
            use mt::{ParametricSurface3D, SearchNearestParameter};
            let mut out: Vec<mt::Face> = Vec::new();
            for (i, f) in faces.iter().enumerate() {
                let touches = f.edge_iter().any(|e| emap.contains_key(&e.id()));
                let change = if touches { Change::Modified } else { Change::Kept };
                faces_from.push(record::from_face(0, first + i, change));
                if !touches {
                    out.push(f.clone());
                    continue;
                }
                let wires = rebuild_wires(f);
                let surface = if moving.contains(&i) {
                    mt::Transformed::transformed(&f.surface(), mat)
                } else if matches!(f.surface(), mt::Surface::Plane(_)) {
                    f.surface()
                } else {
                    // A curved side: sweep its edge in the plane along the new height.
                    let base = f.edge_iter().find_map(|e| {
                        emap.get(&e.id()).filter(|_| {
                            let (a, bb) = (from_p3(e.absolute_front().point()), from_p3(e.absolute_back().point()));
                            (n.dot(a) - d).abs() < tol * 10.0 && (n.dot(bb) - d).abs() < tol * 10.0
                        })
                    });
                    let base = base.ok_or_else(|| fail("a curved side without an edge in the plane"))?;
                    let height = f.vertex_iter().map(|v| n.dot(from_p3(v.point())) - d).fold(0.0f64, |m, x| if x.abs() > m.abs() { x } else { m });
                    let swept: mt::Face = builder::tsweep(base, v3(n * (height - delta)));
                    let mut s = swept.surface();
                    // Same normal as the old side at a point both share.
                    let probe = f.vertex_iter().map(|v| from_p3(v.point())).fold(Vec3::ZERO, |a, x| a + x) / f.vertex_iter().count().max(1) as f64;
                    let old = f.surface();
                    if let (Some((u0, v0)), Some((u1, v1))) =
                        (old.search_nearest_parameter(p3(probe), None, 100), s.search_nearest_parameter(p3(probe), None, 100))
                    {
                        let (a, bb) = (old.normal(u0, v0), s.normal(u1, v1));
                        if mt::InnerSpace::dot(a, bb) < 0.0 {
                            s = mt::Invertible::inverse(&s);
                        }
                    }
                    s
                };
                let mut nf = mt::Face::try_new(wires, surface).map_err(|e| fail(&e.to_string()))?;
                if !f.orientation() {
                    nf.invert();
                }
                out.push(nf);
            }
            shells.push(out.into());
        }
        let s = Solid::try_new(shells).map_err(|e| fail(&e.to_string()))?;
        record::build(s, &record::counts(&[b]), Some(faces_from), Some(&edge_origins))
    })
}

/// Move the planar faces of `b` that lie in the plane (n, d), n their outward normal, out by `delta`, whatever their
/// neighbours are. [`push_faces`] stretches the neighbours, which must stand square to the face along straight edges (a
/// box, an extruded outline); the neighbours of a lofted tube's end cap curve and lean. Here nothing is stretched: the
/// faces move, and each edge one shares with a face that stays gets a new face between them, the edge swept along `n`
/// (a strip). The neighbours keep their surfaces and meet the strips at a crease along the old edge, so the body is
/// the old one with a short prism of the faces' outline on each of them. Exact, with no boolean. The result has no
/// history.
pub(crate) fn grow_faces_exact(b: &Body, n: Vec3, d: f64, delta: f64) -> Result<Body> {
    use mt::{BoundedCurve, ParametricCurve, ParametricSurface3D, SearchNearestParameter};
    let size = b.size();
    let tol = (size * 1e-7).max(1e-9);
    let fail = |m: &str| KernelError::Failed(format!("growing a face: {m}"));
    guard("grow face", || {
        let solid = b.deep_copy();
        let shift = n * delta;
        let mat = mt::Matrix4::from_translation(v3(shift));
        let mut shells = Vec::new();
        for sh in solid.boundaries() {
            let faces: Vec<mt::Face> = sh.face_iter().cloned().collect();
            let on_plane = |f: &mt::Face| match f.oriented_surface() {
                mt::Surface::Plane(p) => {
                    let m = p.normal();
                    let m = Vec3::new(m.x, m.y, m.z);
                    m.dot(n) > 1.0 - 1e-9 && (n.dot(from_p3(p.origin())) - d).abs() < tol * 10.0
                }
                _ => false,
            };
            let moving: Vec<usize> = (0..faces.len()).filter(|i| faces.get(*i).is_some_and(on_plane)).collect();
            if moving.is_empty() {
                shells.push(sh.clone());
                continue;
            }
            let stays = |i: &usize| !moving.contains(i);
            // The faces each edge bounds.
            let mut bounds: HashMap<mt::EdgeID, Vec<usize>> = HashMap::new();
            for (i, f) in faces.iter().enumerate() {
                for e in f.edge_iter() {
                    bounds.entry(e.id()).or_default().push(i);
                }
            }
            // The moved copies of the moving faces' vertices and edges.
            let mut vmap: HashMap<mt::VertexID, mt::Vertex> = HashMap::new();
            let mut emap: HashMap<mt::EdgeID, mt::Edge> = HashMap::new();
            for i in &moving {
                let f = faces.get(*i).ok_or_else(|| fail("face"))?;
                for v in f.vertex_iter() {
                    vmap.entry(v.id()).or_insert_with(|| builder::vertex(p3(from_p3(v.point()) + shift)));
                }
            }
            for i in &moving {
                let f = faces.get(*i).ok_or_else(|| fail("face"))?;
                for e in f.edge_iter() {
                    if emap.contains_key(&e.id()) {
                        continue;
                    }
                    let (a, bb) = (
                        vmap.get(&e.absolute_front().id()).ok_or_else(|| fail("vertex"))?,
                        vmap.get(&e.absolute_back().id()).ok_or_else(|| fail("vertex"))?,
                    );
                    emap.insert(e.id(), mt::Edge::new(a, bb, mt::Transformed::transformed(&e.curve(), mat)));
                }
            }
            // The moved copy of an edge, running the way the edge does.
            let moved = |e: &mt::Edge| -> Result<mt::Edge> {
                let ne = emap.get(&e.id()).ok_or_else(|| fail("edge"))?;
                Ok(if e.front() == e.absolute_front() { ne.clone() } else { ne.inverse() })
            };
            // A strip on every edge between a moving face and one that stays; the edges from a vertex to its moved copy are
            // shared by the two strips that meet there.
            let mut lateral: HashMap<mt::VertexID, mt::Edge> = HashMap::new();
            let mut strips: Vec<mt::Face> = Vec::new();
            for i in &moving {
                let f = faces.get(*i).ok_or_else(|| fail("face"))?;
                for w in f.boundaries() {
                    for e in w.edge_iter() {
                        let Some(&beside) = bounds.get(&e.id()).and_then(|fs| fs.iter().find(|j| stays(j))) else { continue };
                        // The moving face runs along the edge from p to q; the strip runs along it the same way.
                        let (p, q) = (e.front().clone(), e.back().clone());
                        let mut lat = |v: &mt::Vertex| -> Result<mt::Edge> {
                            let to = vmap.get(&v.id()).ok_or_else(|| fail("vertex"))?;
                            Ok(lateral.entry(v.id()).or_insert_with(|| builder::line(v, to)).clone())
                        };
                        let (lp, lq) = (lat(&p)?, lat(&q)?);
                        let wire: mt::Wire = vec![e.clone(), lq, moved(e)?.inverse(), lp.inverse()].into();
                        // Its surface is the edge's curve swept along `n`, facing the way the face beside it does; a straight edge
                        // sweeps to a plane (truck makes that a B-spline patch, which does not mesh).
                        let curve = e.curve();
                        let (t0, t1) = curve.range_tuple();
                        let mid = from_p3(curve.subs(0.5 * (t0 + t1)));
                        let old = faces.get(beside).ok_or_else(|| fail("face"))?.oriented_surface();
                        let Some((u0, v0)) = old.search_nearest_parameter(p3(mid), None, 100) else {
                            return Err(fail("which way a strip faces"));
                        };
                        let outward = old.normal(u0, v0);
                        let outward = Vec3::new(outward.x, outward.y, outward.z);
                        // (a loft's straight section edge is a B-spline of degree one, so ask the points, not the type)
                        let straight = [0.2, 0.4, 0.6, 0.8].iter().all(|t| {
                            let x = from_p3(curve.subs(t0 + t * (t1 - t0)));
                            let chord = from_p3(q.point()) - from_p3(p.point());
                            let off = (x - from_p3(p.point())).cross(chord).len();
                            off <= tol * 10.0 * chord.len()
                        });
                        let s = if straight {
                            let dir = (from_p3(q.point()) - from_p3(p.point())).normalized().ok_or_else(|| fail("a zero edge"))?;
                            let mut m = dir.cross(n).normalized().ok_or_else(|| fail("an edge along the normal"))?;
                            if m.dot(outward) < 0.0 {
                                m = -m;
                            }
                            let o = from_p3(p.point());
                            mt::Surface::Plane(mt::Plane::new(p3(o), p3(o + dir), p3(o + m.cross(dir))))
                        } else {
                            let swept: mt::Face = builder::tsweep(e, v3(shift));
                            let mut s = swept.surface();
                            let Some((u1, v1)) = s.search_nearest_parameter(p3(mid + shift * 0.5), None, 100) else {
                                return Err(fail("which way a strip faces"));
                            };
                            let now = s.normal(u1, v1);
                            if outward.dot(Vec3::new(now.x, now.y, now.z)) < 0.0 {
                                s = mt::Invertible::inverse(&s);
                            }
                            s
                        };
                        strips.push(mt::Face::try_new(vec![wire], s).map_err(|e| fail(&e.to_string()))?);
                    }
                }
            }
            let mut out: Vec<mt::Face> = Vec::new();
            for (i, f) in faces.iter().enumerate() {
                if !moving.contains(&i) {
                    out.push(f.clone());
                    continue;
                }
                let wires = f
                    .absolute_boundaries()
                    .iter()
                    .map(|w| w.edge_iter().map(&moved).collect::<Result<Vec<_>>>().map(mt::Wire::from))
                    .collect::<Result<Vec<_>>>()?;
                let mut nf = mt::Face::try_new(wires, mt::Transformed::transformed(&f.surface(), mat)).map_err(|e| fail(&e.to_string()))?;
                if !f.orientation() {
                    nf.invert();
                }
                out.push(nf);
            }
            out.extend(strips);
            shells.push(out.into());
        }
        let s = Solid::try_new(shells).map_err(|e| fail(&e.to_string()))?;
        let r = Body::new(s)?;
        let problems = r.validity();
        if !problems.is_empty() {
            return Err(fail(&format!("an unsound result ({})", problems.iter().take(2).cloned().collect::<Vec<_>>().join("; "))));
        }
        Ok(r)
    })
}

/// An edge's points from its start to its end along its curve, `k + 1` of them.
fn edge_samples(e: &mt::Edge, k: u32) -> Vec<Vec3> {
    use mt::{BoundedCurve, ParametricCurve};
    let c = e.curve();
    let (t0, t1) = c.range_tuple();
    (0..=k).map(|i| from_p3(c.subs(t0 + (t1 - t0) * f64::from(i) / f64::from(k)))).collect()
}

/// Where the edges of `b` cross the plane (n, d), one point for every time (edges are cut there before a body is trimmed).
fn plane_crossings(b: &Body, n: Vec3, d: f64, tol: f64) -> Vec<Vec3> {
    use mt::{BoundedCurve, ParametricCurve};
    const STEPS: u32 = 64;
    let mut out = Vec::new();
    for e in b.solid.edge_iter() {
        let c = e.curve();
        let (t0, t1) = c.range_tuple();
        let at = |t: f64| from_p3(c.subs(t));
        let h = |t: f64| n.dot(at(t)) - d;
        let ts: Vec<f64> = (0..=STEPS).map(|i| t0 + (t1 - t0) * f64::from(i) / f64::from(STEPS)).collect();
        // The samples clear of the plane, and between each two that lie on different sides the crossing.
        let clear: Vec<(f64, f64)> = ts.iter().map(|t| (*t, h(*t))).filter(|(_, x)| x.abs() > tol).collect();
        for pair in clear.windows(2) {
            let [(ta, ha), (tb, hb)] = pair else { continue };
            if (*ha < 0.0) == (*hb < 0.0) {
                continue;
            }
            let (mut lo, mut hi, mut hlo) = (*ta, *tb, *ha);
            for _ in 0..80 {
                let mid = 0.5 * (lo + hi);
                let hm = h(mid);
                if (hm < 0.0) == (hlo < 0.0) {
                    lo = mid;
                    hlo = hm;
                } else {
                    hi = mid;
                }
            }
            out.push(at(0.5 * (lo + hi)));
        }
    }
    out
}

/// Which side of the plane (n, d) an edge lies on, once edges that cross it are cut: -1 near, +1 far, 0 in it.
fn edge_side(e: &mt::Edge, n: Vec3, d: f64, tol: f64) -> Result<i8> {
    let hs: Vec<f64> = edge_samples(e, 8).iter().map(|p| n.dot(*p) - d).collect();
    let (lo, hi) = hs.iter().fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(*x), h.max(*x)));
    if hi <= tol && lo >= -tol {
        Ok(0)
    } else if hi <= tol {
        Ok(-1)
    } else if lo >= -tol {
        Ok(1)
    } else {
        Err(KernelError::Failed("trimming at a plane: an edge still crosses it".into()))
    }
}

/// Whether the point lies on the surface, to within `tol`.
fn on_surface(s: &mt::Surface, p: Vec3, tol: f64) -> bool {
    use mt::{ParametricSurface, SearchNearestParameter};
    if let mt::Surface::Plane(pl) = s {
        let m = pl.normal();
        return (Vec3::new(m.x, m.y, m.z).dot(p - from_p3(pl.origin()))).abs() <= tol;
    }
    s.search_nearest_parameter(p3(p), None, 100).is_some_and(|(u, v)| from_p3(s.subs(u, v)).dist(p) <= tol)
}

/// The edge that closes the boundary of the flat face `face` from the vertex `from` to the vertex `to` (both in the plane) where a run of
/// its edges beyond the plane went: the segment between them, which lies in the face. (A curved face that crosses the plane has been
/// divided along it already.) Nothing when they are the same vertex.
fn bridge(face: &mt::Face, from: &mt::Vertex, to: &mt::Vertex, tol: f64) -> Result<Vec<mt::Edge>> {
    if from.id() == to.id() {
        return Ok(Vec::new());
    }
    let (p, q) = (from_p3(from.point()), from_p3(to.point()));
    let surface = face.oriented_surface();
    if [0.25, 0.5, 0.75].iter().all(|t| on_surface(&surface, p + (q - p) * *t, tol * 10.0)) {
        Ok(vec![builder::line(from, to)])
    } else {
        Err(KernelError::Failed("trimming at a plane: a curved face crosses the plane where it could not be divided".into()))
    }
}

/// One boundary of the flat face `face`, which crosses the plane (n, d), clipped to the near side: the edges beyond the plane go and the
/// segment in the plane from where the boundary leaves it to where it comes back takes their place. `None` when nothing of the boundary
/// is on the near side.
fn clip_wire(face: &mt::Face, w: &mt::Wire, n: Vec3, d: f64, tol: f64) -> Result<Option<Vec<mt::Edge>>> {
    let fail = |m: &str| KernelError::Failed(format!("trimming at a plane: {m}"));
    let mut edges: Vec<(mt::Edge, i8)> = Vec::new();
    for e in w.edge_iter() {
        let side = edge_side(e, n, d, tol)?;
        edges.push((e.clone(), side));
    }
    if !edges.iter().any(|(_, side)| *side < 0) {
        return Ok(None);
    }
    let m = edges.len();
    let gone = |i: usize| edges.get(i % m).is_some_and(|(_, side)| *side > 0);
    // Begin where a run of edges beyond the plane ends, so that no run is split between the end and the start.
    let Some(start) = (0..m).find(|i| !gone(*i) && gone(*i + m - 1)) else {
        return Ok(Some(edges.into_iter().map(|(e, _)| e).collect()));
    };
    let mut out: Vec<mt::Edge> = Vec::new();
    let mut skipping = false;
    for k in 0..m {
        let Some((e, side)) = edges.get((start + k) % m) else { continue };
        if *side > 0 {
            skipping = true;
            continue;
        }
        if skipping {
            let from = out.last().ok_or_else(|| fail("a run beyond the plane first"))?.back().clone();
            out.extend(bridge(face, &from, e.front(), tol)?);
            skipping = false;
        }
        out.push(e.clone());
    }
    if skipping {
        let from = out.last().ok_or_else(|| fail("nothing kept"))?.back().clone();
        let to = out.first().ok_or_else(|| fail("nothing kept"))?.front().clone();
        out.extend(bridge(face, &from, &to, tol)?);
    }
    Ok(Some(out))
}

/// `b` with everything beyond the plane (n, d) cut off, for a body that was grown or pushed past the plane (the faces that
/// [`grow_faces_exact`] and [`push_faces`] moved) and joined there. A cut with a boolean would run along the edges where the grown
/// faces meet their neighbours, which the mesh intersection cannot take. Here the edges that cross the plane are cut at it, the
/// faces beyond it go, each face that crosses it gets one edge in the plane where its far part was, and the rim they leave (edges
/// lying in the plane) is closed with flat faces. The result has no history.
pub(crate) fn trim_exact(b: &Body, n: Vec3, d: f64) -> Result<Body> {
    let fail = |m: &str| KernelError::Failed(format!("trimming at a plane: {m}"));
    let tol = (b.size() * 1e-6).max(1e-9);
    // A curved face that crosses the plane (a sphere or a cylinder stretched past it) is divided along the plane where they meet, exactly
    // ([`crate::splitface::split_faces`]); the flat ones are clipped below.
    let curved: Vec<usize> = b
        .solid
        .face_iter()
        .enumerate()
        .filter(|(_, f)| !matches!(f.oriented_surface(), mt::Surface::Plane(_)))
        .filter(|(_, f)| {
            let (mut near, mut far) = (false, false);
            for e in f.edge_iter() {
                for p in edge_samples(&e, 8) {
                    near |= n.dot(p) - d < -tol;
                    far |= n.dot(p) - d > tol;
                }
            }
            near && far
        })
        .map(|(i, _)| i)
        .collect();
    let divided = if curved.is_empty() {
        b.clone()
    } else {
        let u = n.any_perp();
        let plane = solidcraft_geom::Plane::new(n * d, u, n.cross(u)).ok_or_else(|| fail("the plane"))?;
        crate::splitface::split_faces(b, &curved, &crate::SplitTool::Plane(plane))?
    };
    let cut = crate::splitface::split_edges_at(&divided, &plane_crossings(&divided, n, d, tol))?;
    guard("trim at a plane", || {
        let mut kept: Vec<mt::Face> = Vec::new();
        for f in cut.solid.face_iter() {
            let mut sides: Vec<i8> = Vec::new();
            for e in f.edge_iter() {
                sides.push(edge_side(&e, n, d, tol)?);
            }
            match (sides.contains(&-1), sides.contains(&1)) {
                (true, false) => kept.push(f.clone()),
                (false, true) => {}
                // A face lying in the plane (a sliver the join left there) goes with the others: the rim it leaves is closed below.
                (false, false) => {}
                (true, true) => {
                    let mut wires: Vec<mt::Wire> = Vec::new();
                    for (i, w) in f.absolute_boundaries().iter().enumerate() {
                        match clip_wire(f, w, n, d, tol)? {
                            Some(edges) => wires.push(edges.into()),
                            None if i == 0 => return Err(fail("a face with no edge left on the near side")),
                            None => {}
                        }
                    }
                    let mut nf = mt::Face::try_new(wires, f.surface()).map_err(|x| fail(&x.to_string()))?;
                    if !f.orientation() {
                        nf.invert();
                    }
                    kept.push(nf);
                }
            }
        }
        close_rim(kept, n, d, tol)
    })
}

/// The body whose faces are `faces`, which leave an opening in the plane (n, d): the edges only one face is on (all lying in the plane)
/// are closed with flat faces. Counterclockwise loops about the normal are outlines, the others holes in the outline around them.
fn close_rim(mut faces: Vec<mt::Face>, n: Vec3, d: f64, tol: f64) -> Result<Body> {
    let fail = |m: &str| KernelError::Failed(format!("closing the cut: {m}"));
    let mut uses: HashMap<mt::EdgeID, usize> = HashMap::new();
    for f in &faces {
        for e in f.edge_iter() {
            *uses.entry(e.id()).or_default() += 1;
        }
    }
    if uses.values().any(|c| *c > 2) {
        return Err(fail("an edge on more than two faces"));
    }
    let mut open: Vec<mt::Edge> = Vec::new();
    for f in &faces {
        for w in f.boundaries() {
            for e in w.edge_iter() {
                if uses.get(&e.id()) == Some(&1) {
                    if edge_samples(e, 8).iter().any(|p| (n.dot(*p) - d).abs() > tol) {
                        return Err(fail("an edge left open off the plane"));
                    }
                    // The cap runs along it the other way.
                    open.push(e.inverse());
                }
            }
        }
    }
    if open.is_empty() {
        return Err(fail("nothing was cut off"));
    }
    let mut loops: Vec<Vec<mt::Edge>> = Vec::new();
    while let Some(first) = open.pop() {
        let start = first.front().id();
        let mut at = first.back().id();
        let mut chain = vec![first];
        while at != start {
            let next: Vec<usize> = (0..open.len()).filter(|i| open.get(*i).is_some_and(|e| e.front().id() == at)).collect();
            let [i] = next[..] else { return Err(fail("the rim does not close up, or pinches")) };
            let e = open.remove(i);
            at = e.back().id();
            chain.push(e);
        }
        loops.push(chain);
    }
    let u = n.any_perp();
    let v = n.cross(u);
    let origin = n * d;
    let polys: Vec<Vec<(f64, f64)>> = loops
        .iter()
        .map(|chain| {
            chain
                .iter()
                .flat_map(|e| {
                    let mut pts = edge_samples(e, 8);
                    if !e.orientation() {
                        pts.reverse();
                    }
                    pts.pop();
                    pts
                })
                .map(|p| ((p - origin).dot(u), (p - origin).dot(v)))
                .collect()
        })
        .collect();
    let areas: Vec<f64> = polys
        .iter()
        .map(|poly| {
            let m = poly.len();
            (0..m)
                .map(|i| match (poly.get(i), poly.get((i + 1) % m)) {
                    (Some(a), Some(b)) => a.0 * b.1 - b.0 * a.1,
                    _ => 0.0,
                })
                .sum::<f64>()
                * 0.5
        })
        .collect();
    let inside = |poly: &[(f64, f64)], q: (f64, f64)| -> bool {
        let m = poly.len();
        (0..m)
            .filter(|i| match (poly.get(*i), poly.get((*i + 1) % m)) {
                (Some(a), Some(b)) => (a.1 > q.1) != (b.1 > q.1) && q.0 < (b.0 - a.0) * (q.1 - a.1) / (b.1 - a.1) + a.0,
                _ => false,
            })
            .count()
            % 2
            == 1
    };
    let outers: Vec<usize> = (0..loops.len()).filter(|i| areas.get(*i).is_some_and(|a| *a > 0.0)).collect();
    let mut holes_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in (0..loops.len()).filter(|i| !outers.contains(i)) {
        let q = polys.get(i).and_then(|p| p.first()).copied().ok_or_else(|| fail("an empty loop"))?;
        let around = outers
            .iter()
            .filter(|o| polys.get(**o).is_some_and(|p| inside(p, q)))
            .min_by(|x, y| areas.get(**x).partial_cmp(&areas.get(**y)).unwrap_or(std::cmp::Ordering::Equal))
            .ok_or_else(|| fail("a hole outside every outline"))?;
        holes_of.entry(*around).or_default().push(i);
    }
    let surface = mt::Surface::Plane(mt::Plane::new(p3(origin), p3(origin + u), p3(origin + v)));
    for o in &outers {
        let mut wires: Vec<mt::Wire> = vec![loops.get(*o).cloned().ok_or_else(|| fail("a loop"))?.into()];
        for h in holes_of.get(o).into_iter().flatten() {
            wires.push(loops.get(*h).cloned().ok_or_else(|| fail("a loop"))?.into());
        }
        faces.push(mt::Face::try_new(wires, surface.clone()).map_err(|x| fail(&x.to_string()))?);
    }
    let shell: mt::Shell = faces.into();
    let solid = Solid::try_new(crate::polybool::components_in_order(&shell)).map_err(|x| fail(&x.to_string()))?;
    let r = Body::new(solid)?;
    let problems = r.validity();
    if !problems.is_empty() {
        return Err(fail(&format!("an unsound result ({})", problems.iter().take(2).cloned().collect::<Vec<_>>().join("; "))));
    }
    Ok(r)
}

/// Move the planar faces of `x` in the plane (n, d) out by `delta`: stretched ([`push_faces`]) where their neighbours
/// allow it, else, when `grow`, grown ([`grow_faces_exact`]). Whether it grew them (the body then has no history).
fn move_out(x: &Body, n: Vec3, d: f64, delta: f64, grow: bool) -> Result<(Body, bool)> {
    match push_faces(x, n, d, delta) {
        Ok(r) => Ok((r, false)),
        Err(e) if grow => grow_faces_exact(x, n, d, delta).map(|r| (r, true)).map_err(|_| e),
        Err(e) => Err(e),
    }
}

/// A box covering everything beyond the plane (n, d) by up to `depth`.
fn slab_beyond(n: Vec3, d: f64, depth: f64, extent: f64) -> Result<Body> {
    let u = n.any_perp();
    let w = n.cross(u);
    let o = n * d;
    let pl = solidcraft_geom::Plane::new(o, u, w).ok_or_else(|| KernelError::Failed("slab plane".into()))?;
    let sq = solidcraft_geom::Loop2::polygon(&[
        solidcraft_geom::Vec2::new(-extent, -extent),
        solidcraft_geom::Vec2::new(extent, -extent),
        solidcraft_geom::Vec2::new(extent, extent),
        solidcraft_geom::Vec2::new(-extent, extent),
    ]);
    let r = solidcraft_geom::Region2 { outer: sq, holes: vec![] };
    crate::extrude(&pl, &[r], 0.0, depth)?.pop().ok_or_else(|| KernelError::Failed("slab".into()))
}

/// The box of a face's boundary sampled to within `tol` of each curve: the face, planar, lies
/// within `tol` of it. `None` when the boundary can't be sampled.
fn face_box(f: &mt::Face, tol: f64) -> Option<Aabb3> {
    use mt::{BoundedCurve, ParameterDivision1D};
    const MAX_POINTS: usize = 100_000;
    guard("face box", || {
        let mut bb = Aabb3::EMPTY;
        let mut count = 0usize;
        for e in f.edge_iter() {
            let points = match e.curve() {
                mt::Curve::Line(_) => vec![e.front().point(), e.back().point()],
                // The division checks its deviation at midpoints only (as in blend.rs).
                c => c.parameter_division(c.range_tuple(), tol * 0.25).1,
            };
            count = count.saturating_add(points.len());
            if count > MAX_POINTS {
                return Err(KernelError::Failed("face box: too many boundary points".into()));
            }
            for p in points {
                bb.add(from_p3(p));
            }
        }
        if bb.is_empty() {
            return Err(KernelError::Failed("face box: no boundary".into()));
        }
        Ok(bb)
    })
    .ok()
}

/// The planes (normal, offset) in which a planar face of `a` and one of `b` coincide, as `a`'s
/// face lies in them, each with whether `b`'s face points the same way. Faces in a common plane but apart from
/// each other (the tops of two posts standing on one floor) don't coincide: they need no
/// pushing apart. A face whose extent can't be found counts as meeting every face in its plane.
pub(crate) fn coincident_planes(a: &Body, b: &Body) -> Vec<(Vec3, f64, bool)> {
    let size = a.size().max(b.size());
    let tol = (size * 1e-7).max(1e-9) * 10.0;
    let sample = (size * 1e-4).max(tol);
    let (fa, fb): (Vec<&mt::Face>, Vec<&mt::Face>) = (a.solid.face_iter().collect(), b.solid.face_iter().collect());
    let (pa, pb) = (planes(a), planes(b));
    let (mut boxes_a, mut boxes_b): (Vec<Option<Option<Aabb3>>>, Vec<Option<Option<Aabb3>>>) = (vec![None; fa.len()], vec![None; fb.len()]);
    let mut found: Vec<(Vec3, f64, bool)> = Vec::new();
    for (ia, x) in &pa {
        for (ib, y) in &pb {
            let cos = x.n.dot(y.n);
            if cos.abs() <= 1.0 - 1e-9 || (x.d - y.d * cos).abs() >= tol {
                continue;
            }
            let same = cos > 0.0;
            if found.iter().any(|(n, d, s)| *s == same && n.dot(x.n) > 1.0 - 1e-9 && (d - x.d).abs() < tol) {
                continue;
            }
            let ba = boxes_a.get_mut(*ia).and_then(|b| *b.get_or_insert_with(|| fa.get(*ia).and_then(|f| face_box(f, sample))));
            let bb = boxes_b.get_mut(*ib).and_then(|b| *b.get_or_insert_with(|| fb.get(*ib).and_then(|f| face_box(f, sample))));
            // Each box is within `sample` of its face.
            if !ba.zip(bb).is_some_and(|(p, q)| boxes_separate(p, q, 2.0 * sample)) {
                found.push((x.n, x.d, same));
            }
        }
    }
    found
}

/// What [`boolean_apart`] fails with when a face has no room to move.
pub(crate) const NO_ROOM: &str = "coincident faces were left in place: pushing them apart would change the result";

/// Retry a failed boolean with coincident planar faces pushed apart. `None` when there are no
/// coincident faces or the faces can't be pushed; the error [`NO_ROOM`] when a face would
/// move through something that changes the result.
pub(crate) fn boolean_apart(a: &Body, b: &Body, op: BoolOp) -> Option<Result<Option<Body>>> {
    apart_with(a, b, op, false)
}

/// [`boolean_apart`] for a join whose coincident faces cannot be pushed, because the neighbours of one of them curve
/// (a flange flush with the end of a lofted tube): such a face is grown instead ([`grow_faces_exact`]) and the join's excess beyond
/// the plane is cut off again by editing faces ([`trim_exact`]), where a boolean would fail on the edges the grown faces meet their
/// neighbours along. The planar faces that sit on B-spline surfaces (a loft's flat sides) are put on planes first so that they
/// are seen. Asked after the other routes have failed, so that what they take stays as it was. `None` when it does not apply.
pub(crate) fn boolean_grown(a: &Body, b: &Body, op: BoolOp) -> Option<Result<Option<Body>>> {
    if op != BoolOp::Union {
        return None;
    }
    let (pa, pb) = (crate::planar_faces::planarized(a), crate::planar_faces::planarized(b));
    apart_with(pa.as_ref().unwrap_or(a), pb.as_ref().unwrap_or(b), op, true)
}

fn apart_with(a: &Body, b: &Body, op: BoolOp, grow: bool) -> Option<Result<Option<Body>>> {
    let size = a.size().max(b.size());
    let tol = (size * 1e-7).max(1e-9);
    let coincident = coincident_planes(a, b);
    if coincident.is_empty() {
        return None;
    }
    let same: Vec<(Vec3, f64)> = coincident.iter().filter(|(_, _, s)| *s).map(|(n, d, _)| (*n, *d)).collect();
    let back: Vec<(Vec3, f64)> = coincident.iter().filter(|(_, _, s)| !*s).map(|(n, d, _)| (*n, *d)).collect();
    let (d1, d2) = (size * 0.0137, size * 0.0229);
    let counts = record::counts(&[a, b]);
    let (skin, chord) = (tol * 10.0, size * 2e-4);
    let no_room = || KernelError::Failed(NO_ROOM.into());
    let run = || -> Result<Option<Body>> {
        // The target's faces, meshed when the first face asks what it would move through.
        let faces_of_a = std::cell::OnceCell::new();
        let target = || faces_of_a.get_or_init(|| Clearance::new(a, chord).map(|c| c.with_exact_bounds(grow))).as_ref().ok_or_else(no_room);
        let (mut a2, mut b2) = (a.clone(), b.clone());
        // What the pushed bodies are made of: their histories over [a] and [b].
        let (mut ha, mut hb) = (crate::boolean_record::kept(a, 0, 1), crate::boolean_record::kept(b, 0, 1));
        for (n, d) in &same {
            // b's face points along n too.
            let free = if op == BoolOp::Union {
                // Both faces move out and the slab beyond the plane is cut off the joined body
                // again, everywhere: neither body may have anything in it.
                let depth = d1.max(d2) * 3.0 * 1.25;
                let tool = Clearance::new(b, chord).ok_or_else(no_room)?.with_exact_bounds(grow);
                target()?.clear_of_slab(*n, *d, skin, depth) && tool.clear_of_slab(*n, *d, skin, depth)
            } else {
                // The tool takes whatever of `a` its face moves through (a rib standing on the
                // face a cut ends in).
                target()?.swept(&in_plane(&b2, *n, *d, skin), *n, skin, d2 * 1.25) == Occupancy::Empty
            };
            if !free {
                return Err(no_room());
            }
            // A body that has to be grown goes deeper than one that is pushed: the pushed body's stretched sides then end inside
            // the prism the grown one leaves beyond the plane, wherever they meet it, and cross the plane once each.
            let swap = grow && push_faces(&a2, *n, *d, d1).is_err() && push_faces(&b2, *n, *d, d2).is_ok();
            let (da, db) = if swap { (d2, d1) } else { (d1, d2) };
            let (moved_b, b_grown) = move_out(&b2, *n, *d, db, grow)?;
            b2 = moved_b;
            hb = if b_grown { None } else { crate::boolean_record::through(&b2, 0, hb.as_ref()) };
            if op == BoolOp::Union {
                let (moved_a, a_grown) = move_out(&a2, *n, *d, da, grow)?;
                a2 = moved_a;
                ha = if a_grown { None } else { crate::boolean_record::through(&a2, 0, ha.as_ref()) };
            }
        }
        for (n, d) in &back {
            // a's face points along n; b's face points along −n at the same plane (−d).
            let shift = if op == BoolOp::Union {
                // Pushed into `a`, b's face must have a's material behind it all over and that
                // deep: over a hole in a's face (a narrowed bore's rim over the other body's
                // bore, a rib standing half over a window) it would fill the hole, through a
                // thin wall it would stick out the far side (#616). The other retries take
                // such a join instead.
                [d2, d2 * 0.25, d2 * 0.0625]
                    .into_iter()
                    .find(|depth| backed(a, b, -*n, -*d, *depth))
                    .ok_or_else(|| KernelError::Failed("pushing a face: part of it lies over a hole or past the edge of the other body".into()))?
            } else {
                // Back into the tool, the face gives up what the tool took of `a` there (a
                // wall standing beside the face the tool's foot lies on).
                if target()?.swept(&in_plane(&b2, -*n, -*d, skin), *n, skin, d2 * 1.25) != Occupancy::Empty {
                    return Err(no_room());
                }
                -d2
            };
            b2 = push_faces(&b2, -*n, -*d, shift)?;
            hb = crate::boolean_record::through(&b2, 0, hb.as_ref());
        }
        let Some(mut r) = crate::ops::boolean(&a2, &b2, op)? else { return Ok(None) };
        // The pushed bodies' histories as over [a, b], a's and b's in their places.
        let (ha, hb) = (crate::boolean_record::placed(ha, 0, 2), crate::boolean_record::placed(hb, 1, 2));
        let mut h = crate::boolean_record::of_pair(&r, &ha, &hb);
        if op == BoolOp::Union && grow {
            // What grown faces left beyond each plane is cut off by editing the faces: a cut along the plane would run along the
            // edges where the grown faces meet their neighbours, which the mesh intersection cannot take.
            for (n, d) in &same {
                r = trim_exact(&r, *n, *d)?;
            }
            h = None;
        } else if op == BoolOp::Union {
            for (n, d) in &same {
                let slab = slab_beyond(*n, *d, d1.max(d2) * 3.0, size * 4.0)?;
                match crate::ops::boolean(&r, &slab, BoolOp::Cut)? {
                    Some(x) => {
                        h = h.and_then(|h| trimmed(&x, h, &slab, [a, b], *n, *d, tol));
                        r = x;
                    }
                    None => return Ok(None),
                }
            }
        }
        Ok(Some(r.with_checked_history(h, &counts)))
    };
    Some(run())
}

/// The history over `[a, b]` of `x`, the union `r` of `a` and `b` (with history `h` over them),
/// pushed out past the plane (n, d), with the slab beyond it (`slab`, whose start cap lies in
/// the plane) cut off. The slab's cap in `x` is where the operands' faces in that plane were
/// (all of them, so only a cap that stays one face says which), and where it meets a side of
/// an operand is that side's edge along one of those faces. `None` where that is not so.
fn trimmed(x: &Body, h: History, slab: &Body, operands: [&Body; 2], n: Vec3, d: f64, tol: f64) -> Option<History> {
    let mut cut = x.history()?.clone();
    cut.substitute(0, &h).ok()?;
    let cap = slab.history()?.faces.iter().position(|f| f.origins == [FaceOrigin::Cap { end: false }])?;
    let cap = FaceOrigin::Face { operand: 2, face: cap };
    // The operands' faces in the plane, facing along n, and their edges with each other face.
    let in_plane: Vec<Vec<usize>> = operands
        .iter()
        .map(|o| planes(o).into_iter().filter(|(_, p)| p.n.dot(n) > 1.0 - 1e-9 && (p.d - d).abs() < tol * 10.0).map(|(i, _)| i).collect())
        .collect();
    let bounds: Vec<Vec<Vec<usize>>> = operands.iter().map(|o| crate::history::edge_faces(&o.solid)).collect();
    if cut.faces.iter().filter(|f| f.origins.contains(&cap)).count() != 1 {
        return None;
    }
    for f in &mut cut.faces {
        if f.origins.contains(&cap) {
            let mut origins: std::collections::BTreeSet<FaceOrigin> = f.origins.iter().copied().filter(|o| *o != cap).collect();
            for (operand, faces) in in_plane.iter().enumerate() {
                origins.extend(faces.iter().map(|face| FaceOrigin::Face { operand, face: *face }));
            }
            f.change = if origins.len() > 1 { Change::Merged } else { Change::Modified };
            f.origins = origins.into_iter().collect();
        }
    }
    let cap = crate::history::Side::from(cap);
    for e in &mut cut.edges {
        for o in &mut e.origins {
            let side = match o {
                EdgeOrigin::Meet { a, b } if *a == cap => b,
                EdgeOrigin::Meet { a, b } if *b == cap => a,
                _ => continue,
            };
            let Some(FaceOrigin::Face { operand, face }) = side.single() else { return None };
            let (plane_faces, bounds) = (in_plane.get(operand)?, bounds.get(operand)?);
            let mut along = (0..bounds.len())
                .filter(|k| bounds.get(*k).is_some_and(|fs| fs.len() == 2 && fs.contains(&face) && fs.iter().any(|f| plane_faces.contains(f))));
            let (Some(edge), None) = (along.next(), along.next()) else { return None };
            *o = EdgeOrigin::Edge { operand, edge };
        }
    }
    cut.renumber(&[Some(0), Some(1), None], 2).ok()?;
    Some(cut)
}

/// Do the bodies touch over an area, back to back on a common plane: has `a` material just
/// behind some part of a face of `b` lying in a plane of a face of `a` that faces the other way?
/// (Contact along a line or at a point is not this.)
pub(crate) fn faces_touch(a: &Body, b: &Body) -> bool {
    let size = a.size().max(b.size());
    let tol = (size * 1e-7).max(1e-9) * 10.0;
    let pa = planes(a);
    let faces: Vec<(usize, Vec3)> = planes(b)
        .into_iter()
        .filter(|(_, y)| pa.iter().any(|(_, x)| x.n.dot(y.n) < -1.0 + 1e-9 && (x.d + y.d).abs() < tol))
        .map(|(i, y)| (i, y.n))
        .collect();
    if faces.is_empty() {
        return false;
    }
    let (Ok(ma), Ok(mb)) = (a.tessellate_with(size * 1e-3, true), b.tessellate_with(size * 1e-3, true)) else { return false };
    let inside = ma.inside_index();
    let depth = (size * 1e-4).max(1e-6);
    mb.triangles.iter().zip(&mb.tri_face).take(100_000).any(|(t, fi)| {
        let Some((_, n)) = faces.iter().find(|(i, _)| *i == *fi as usize) else { return false };
        mb.tri(t).is_some_and(|[p, q, r]| inside.contains((p + q + r) / 3.0 + *n * depth))
    })
}

/// The triangles of `tool`'s faces in the plane (`m`, `d`), `m` their outward normal, each with
/// how far its face reaches past it sideways. A face bounded by straight edges is its
/// triangles; they are drawn in by `skin`, so that faces of the other body lying against the
/// face's sides are not in its way. A face with a curved edge reaches past its triangles by
/// the chord error.
fn in_plane(tool: &Body, m: Vec3, d: f64, skin: f64) -> Vec<([Vec3; 3], f64)> {
    let chord = (tool.size() * 2e-4).max(skin);
    let Ok(mesh) = tool.tessellate_with(chord, true) else { return Vec::new() };
    let faces: Vec<&mt::Face> = tool.solid.face_iter().collect();
    let margins: HashMap<usize, f64> = planes(tool)
        .into_iter()
        .filter(|(_, p)| p.n.dot(m) > 1.0 - 1e-9 && (p.d - d).abs() < skin)
        .filter_map(|(i, _)| {
            let straight = faces.get(i)?.edge_iter().all(|e| matches!(e.curve(), mt::Curve::Line(_)));
            Some((i, if straight { -skin } else { 2.0 * chord }))
        })
        .collect();
    mesh.triangles.iter().zip(&mesh.tri_face).filter_map(|(t, face)| Some((mesh.tri(t)?, *margins.get(&(*face as usize))?))).collect()
}

/// Has `a` material behind every part of b's faces in the plane (m, d) (m their outward
/// normal, pointing into `a`), to `depth`? Checked at points inside the faces' triangles.
fn backed(a: &Body, b: &Body, m: Vec3, d: f64, depth: f64) -> bool {
    let size = a.size().max(b.size());
    let tol = (size * 1e-7).max(1e-9) * 10.0;
    let faces: Vec<usize> = planes(b).into_iter().filter(|(_, p)| p.n.dot(m) > 1.0 - 1e-9 && (p.d - d).abs() < tol).map(|(i, _)| i).collect();
    let (Ok(ma), Ok(mb)) = (a.tessellate_with(size * 1e-3, true), b.tessellate_with(size * 1e-3, true)) else { return false };
    let inside = ma.inside_index();
    let mut points = Vec::new();
    for (t, fi) in mb.triangles.iter().zip(&mb.tri_face) {
        if !faces.contains(&(*fi as usize)) {
            continue;
        }
        let Some([p, q, r]) = mb.tri(t) else { continue };
        let c = (p + q + r) / 3.0;
        points.extend([c, c.lerp(p, 0.8), c.lerp(q, 0.8), c.lerp(r, 0.8)]);
        if points.len() > 4000 {
            break;
        }
    }
    !points.is_empty() && points.iter().all(|p| [0.05, 0.5, 1.0].iter().all(|k| inside.contains(*p + m * (depth * k))))
}

/// The faces of `body` lying in the plane (n, d), facing along n.
pub(crate) fn faces_in(body: &Body, n: Vec3, d: f64, tol: f64) -> Vec<usize> {
    planes(body).into_iter().filter(|(_, p)| p.n.dot(n) > 1.0 - 1e-9 && (p.d - d).abs() < tol).map(|(i, _)| i).collect()
}

/// Do two face surfaces lie on one plane or one cylinder?
fn same_surface(x: &Option<crate::FaceSurface>, y: &Option<crate::FaceSurface>, tol: f64) -> bool {
    use crate::FaceSurface::{Cylinder, Plane};
    match (x, y) {
        (Some(Plane { normal: n1, point: p1 }), Some(Plane { normal: n2, point: p2 })) => n1.dot(*n2) > 1.0 - 1e-9 && n1.dot(*p2 - *p1).abs() < tol,
        (Some(Cylinder { origin: o1, axis: a1, radius: r1 }), Some(Cylinder { origin: o2, axis: a2, radius: r2 })) => {
            let off = *o2 - *o1;
            a1.dot(*a2).abs() > 1.0 - 1e-9 && (r1 - r2).abs() < tol && (off - *a1 * off.dot(*a1)).len() < tol
        }
        _ => false,
    }
}

/// A box around face `f` from its edges' control points (a B-spline or NURBS curve with positive
/// weights lies in their hull; an edge's curve may run past the edge, which only grows the box).
/// No dividing: for choosing faces it need only contain the face, and dividing a split face's
/// curved edges down to a tolerance ran away on the sensor enclosure's lid (gigabytes, minutes).
/// `None` for an edge on an intersection curve or with weights that don't bound it.
fn hull_box(f: &mt::Face) -> Option<Aabb3> {
    let mut bb = Aabb3::EMPTY;
    for e in f.edge_iter() {
        match e.curve() {
            mt::Curve::Line(_) => {
                bb.add(from_p3(e.front().point()));
                bb.add(from_p3(e.back().point()));
            }
            mt::Curve::BSplineCurve(c) => c.control_points().iter().for_each(|p| bb.add(from_p3(*p))),
            mt::Curve::NurbsCurve(c) => {
                for p in c.control_points() {
                    if !(p.w > 0.0 && p.w.is_finite()) {
                        return None;
                    }
                    bb.add(Vec3::new(p.x / p.w, p.y / p.w, p.z / p.w));
                }
            }
            mt::Curve::IntersectionCurve(_) => return None,
        }
    }
    (!bb.is_empty() && bb.min.is_finite() && bb.max.is_finite()).then_some(bb)
}

/// The box around faces `faces` of `body` ([`hull_box`]), grown by `gap`. `None` when a face's
/// extent can't be found.
fn faces_box(body: &Body, faces: &[usize], gap: f64) -> Option<Aabb3> {
    let fs: Vec<&mt::Face> = body.solid.face_iter().collect();
    let mut bb = Aabb3::EMPTY;
    for f in faces {
        let b = hull_box(fs.get(*f)?)?;
        bb.add(b.min);
        bb.add(b.max);
    }
    (!bb.is_empty()).then(|| Aabb3 { min: bb.min - Vec3::new(gap, gap, gap), max: bb.max + Vec3::new(gap, gap, gap) })
}

/// `body` (with its history `h` over its operand) with its faces in the plane (n, d) split
/// where the faces of `tool` next to the tool's faces in the opposite plane cross them: the
/// outline of the tool's contact imprinted on the body's. Only the faces near the contact are
/// split, and each surface once.
fn imprinted(body: Body, mut h: Option<History>, n: Vec3, d: f64, tool: &Body, tol: f64) -> Result<(Body, Option<History>)> {
    let contact = faces_in(tool, -n, -d, tol);
    let gap = tol * 10.0;
    // A face whose extent can't be found counts as near.
    let region = faces_box(tool, &contact, gap);
    let near = |b: &Body, faces: Vec<usize>| -> Vec<usize> {
        let Some(region) = region else { return faces };
        let fs: Vec<&mt::Face> = b.solid.face_iter().collect();
        faces.into_iter().filter(|f| fs.get(*f).and_then(|x| hull_box(x)).is_none_or(|bx| !boxes_separate(bx, region, gap))).collect()
    };
    let bounds = crate::history::edge_faces(&tool.solid);
    let mut beside: Vec<usize> = bounds
        .iter()
        .filter(|fs| fs.len() == 2 && fs.iter().any(|f| contact.contains(f)))
        .flat_map(|fs| fs.iter().copied())
        .filter(|f| !contact.contains(f))
        .collect();
    beside.sort_unstable();
    beside.dedup();
    let mut body = body;
    // One split per surface: the halves of a round side lie on one cylinder, and splitting
    // along it again re-traces the line already there, leaving a sliver (or not) depending
    // on how the second trace falls.
    let mut used: Vec<Option<crate::FaceSurface>> = Vec::new();
    for t in beside {
        let surface = crate::face_surface(tool, t);
        if used.iter().any(|u| same_surface(u, &surface, tol)) {
            continue;
        }
        used.push(surface);
        let faces = near(&body, faces_in(&body, n, d, tol));
        if faces.is_empty() {
            break;
        }
        // Faces the tool's face does not cross stay as they are; none crossed is an error.
        let Ok((split, _)) = crate::splitface::split_any(&body, &faces, &crate::SplitTool::Face { body: tool, face: t }) else { continue };
        h = crate::boolean_record::through(&split, 0, h.as_ref());
        body = split;
    }
    Ok((body, h))
}

/// Union of bodies touching back to back over part of a planar face (a pad half past the edge
/// of the face it stands on, two blocks meeting over some of their faces, #674, #679): each
/// body's contact faces are split along the outline of the other's, so the common area is the
/// same face in both, and those faces are glued away. `None` when that does not give a result
/// `accept`ed.
pub(crate) fn imprint_union(a: &Body, b: &Body, accept: &dyn Fn(&Body) -> bool) -> Option<Body> {
    let size = a.size().max(b.size());
    let tol = (size * 1e-7).max(1e-9) * 10.0;
    let back: Vec<(Vec3, f64)> = coincident_planes(a, b).into_iter().filter(|(_, _, same)| !*same).map(|(n, d, _)| (n, d)).collect();
    if back.is_empty() {
        return None;
    }
    let run = || -> Result<Option<Body>> {
        let (mut a2, mut ha) = (a.clone(), crate::boolean_record::kept(a, 0, 1));
        let (mut b2, mut hb) = (b.clone(), crate::boolean_record::kept(b, 0, 1));
        for (n, d) in &back {
            // a's faces face along n in the plane; b's face the other way.
            (a2, ha) = imprinted(a2, ha, *n, *d, b, tol)?;
            (b2, hb) = imprinted(b2, hb, -*n, -*d, a, tol)?;
        }
        // The common faces' boundaries with the same vertices on both sides.
        for (n, d) in &back {
            let corners = |body: &Body, n: Vec3, d: f64| -> Vec<Vec3> {
                let fs: Vec<&mt::Face> = body.solid.face_iter().collect();
                let mut out: Vec<Vec3> =
                    faces_in(body, n, d, tol).iter().filter_map(|i| fs.get(*i)).flat_map(|f| f.vertex_iter().map(|v| from_p3(v.point()))).collect();
                out.dedup_by(|x, y| x.dist(*y) <= tol);
                out
            };
            let (pa, pb) = (corners(&a2, *n, *d), corners(&b2, -*n, -*d));
            let a3 = crate::splitface::split_edges_at(&a2, &pb)?;
            ha = crate::boolean_record::through(&a3, 0, ha.as_ref());
            let b3 = crate::splitface::split_edges_at(&b2, &pa)?;
            hb = crate::boolean_record::through(&b3, 0, hb.as_ref());
            (a2, b2) = (a3, b3);
        }
        let Some(r) = glue(&a2, &b2) else { return Ok(None) };
        let r = r?;
        let (ha, hb) = (crate::boolean_record::placed(ha, 0, 2), crate::boolean_record::placed(hb, 1, 2));
        let h = crate::boolean_record::of_pair(&r, &ha, &hb);
        Ok(Some(r.with_checked_history(h, &record::counts(&[a, b]))))
    };
    match guard("imprint union", run) {
        Ok(Some(r)) if accept(&r) => Some(r),
        _ => None,
    }
}

/// Union of bodies that touch along identical planar faces, back to back (a body joined with
/// its mirror image, or the pieces of a body cut apart by a plane, which may meet in several
/// faces): every such pair of faces goes and the shells are stitched along their edges. `None`
/// when no such pair of faces exists.
pub(crate) fn glue(a: &Body, b: &Body) -> Option<Result<Body>> {
    glue_impl(a, b, false)
}

/// Glue every certified start cap, retaining each tool's original contact tolerance.
pub(crate) fn glue_all_caps(a: &Body, b: &Body) -> Option<Result<Body>> {
    glue_impl(a, b, true)
}

fn glue_impl(a: &Body, b: &Body, all_caps: bool) -> Option<Result<Body>> {
    use mt::{BoundedCurve, ParametricCurve};
    let size = a.size().max(b.size());
    let tol = (size * 1e-7).max(1e-9) * 10.0;
    let mid = |e: &mt::Edge| {
        let c = e.curve();
        let (t0, t1) = c.range_tuple();
        from_p3(c.subs((t0 + t1) * 0.5))
    };
    // Matching faces: same plane, opposite normals, the same edges. Every pair is glued: a
    // pair left behind would stay inside the result as a zero-thickness slit (issue #616).
    let (pa, pb) = (planes(a), planes(b));
    let mut tool_sizes = HashMap::new();
    if all_caps {
        for shell in b.solid.boundaries() {
            let mut bounds = solidcraft_geom::Aabb3::EMPTY;
            for vertex in shell.vertex_iter() {
                bounds.add(from_p3(vertex.point()));
            }
            let size = bounds.diagonal();
            if !size.is_finite() {
                return None;
            }
            for face in shell.face_iter() {
                tool_sizes.insert(face.id(), size);
            }
        }
    }
    let fa_list: Vec<&mt::Face> = a.solid.face_iter().collect();
    let fb_list: Vec<&mt::Face> = b.solid.face_iter().collect();
    // B edge → the matching A edge; B vertex → the matching A vertex.
    let mut map: HashMap<mt::EdgeID, mt::Edge> = HashMap::new();
    let mut vmap: HashMap<mt::VertexID, mt::Vertex> = HashMap::new();
    let (mut glued_a, mut glued_b): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    if all_caps && pa.len().saturating_mul(pb.len()) > 2_000_000 {
        return None;
    }
    for (ia, x) in &pa {
        for (ib, y) in &pb {
            if all_caps
                && !matches!(b.face_sources().and_then(|sources| sources.get(*ib)), Some(Some(crate::FaceSource::ExtrudeCap { high: false, .. })))
            {
                continue;
            }
            let tol = if all_caps { (a.size().max(*tool_sizes.get(&fb_list.get(*ib)?.id())?) * 1e-7).max(1e-9) * 10.0 } else { tol };
            let key = |p: Vec3| ((p.x / tol).round() as i64, (p.y / tol).round() as i64, (p.z / tol).round() as i64);
            if glued_a.contains(ia) || glued_b.contains(ib) || x.n.dot(y.n) > -1.0 + 1e-9 || (x.d + y.d).abs() > tol {
                continue;
            }
            let (Some(fa), Some(fb)) = (fa_list.get(*ia), fb_list.get(*ib)) else { continue };
            let ea: Vec<mt::Edge> = fa.edge_iter().collect();
            let eb: Vec<mt::Edge> = fb.edge_iter().collect();
            if ea.len() != eb.len() {
                continue;
            }
            let (mut pair_map, mut pair_vmap) = (Vec::new(), Vec::new());
            let mut ok = true;
            for e in &eb {
                let ends = (key(from_p3(e.absolute_front().point())), key(from_p3(e.absolute_back().point())));
                let m = mid(e);
                // The same curve: the same ends and e's middle on f (curves of one shape may be
                // parametrised differently, so their parameter middles needn't coincide).
                let on = |f: &mt::Edge| {
                    use mt::SearchNearestParameter;
                    let c = f.curve();
                    let (t0, t1) = c.range_tuple();
                    key(mid(f)) == key(m)
                        || c.search_nearest_parameter(p3(m), None, 100)
                            .is_some_and(|t| (t0.min(t1) - 1e-9..=t0.max(t1) + 1e-9).contains(&t) && from_p3(c.subs(t)).dist(m) <= tol)
                };
                let hit = ea.iter().find(|f| {
                    let fe = (key(from_p3(f.absolute_front().point())), key(from_p3(f.absolute_back().point())));
                    (fe == ends || fe == (ends.1, ends.0)) && on(f)
                });
                let Some(f) = hit else {
                    ok = false;
                    break;
                };
                let same = key(from_p3(f.absolute_front().point())) == ends.0;
                pair_map.push((e.id(), if same { f.absolute_clone() } else { f.absolute_clone().inverse() }));
                let (fa_v, fb_v) = if same {
                    (f.absolute_front().clone(), f.absolute_back().clone())
                } else {
                    (f.absolute_back().clone(), f.absolute_front().clone())
                };
                pair_vmap.push((e.absolute_front().id(), fa_v));
                pair_vmap.push((e.absolute_back().id(), fb_v));
            }
            if !ok {
                continue;
            }
            map.extend(pair_map);
            vmap.extend(pair_vmap);
            glued_a.push(*ia);
            glued_b.push(*ib);
        }
    }
    if all_caps
        && b.face_sources()?
            .iter()
            .enumerate()
            .any(|(index, source)| matches!(source, Some(crate::FaceSource::ExtrudeCap { high: false, .. })) && !glued_b.contains(&index))
    {
        return None;
    }
    if glued_a.is_empty() {
        return None;
    }
    Some(guard("glue", || {
        let fail = |m: &str| KernelError::Failed(format!("joining touching faces: {m}"));
        // B's other edges, rebuilt on A's vertices where they meet a seam.
        let mut edges: HashMap<mt::EdgeID, mt::Edge> = map.clone();
        for e in b.solid.edge_iter() {
            if edges.contains_key(&e.id()) {
                continue;
            }
            let (f0, f1) = (e.absolute_front(), e.absolute_back());
            if !vmap.contains_key(&f0.id()) && !vmap.contains_key(&f1.id()) {
                edges.insert(e.id(), e.absolute_clone());
                continue;
            }
            let v0 = vmap.get(&f0.id()).cloned().unwrap_or_else(|| f0.clone());
            let v1 = vmap.get(&f1.id()).cloned().unwrap_or_else(|| f1.clone());
            edges.insert(e.id(), mt::Edge::new(&v0, &v1, e.curve()));
        }
        let mut faces: Vec<mt::Face> = fa_list.iter().enumerate().filter(|(i, _)| !glued_a.contains(i)).map(|(_, f)| (*f).clone()).collect();
        // Every face but the glued ones is kept; an edge along a seam is an edge of both.
        let kept = |operand: usize, n: usize, skip: &[usize]| {
            (0..n).filter(|i| !skip.contains(i)).map(move |i| record::from_face(operand, i, Change::Kept)).collect::<Vec<_>>()
        };
        let from: Vec<FaceProv> = kept(0, fa_list.len(), &glued_a).into_iter().chain(kept(1, fb_list.len(), &glued_b)).collect();
        let mut origins = record::edge_origins(&a.solid, 0);
        for (old, o) in record::edge_origins(&b.solid, 1) {
            if let Some(new) = edges.get(&old) {
                origins.entry(new.id()).or_default().extend(o);
            }
        }
        for (i, f) in fb_list.iter().enumerate() {
            if glued_b.contains(&i) {
                continue;
            }
            let wires: Vec<mt::Wire> = f
                .absolute_boundaries()
                .iter()
                .map(|w| {
                    w.edge_iter()
                        .filter_map(|e| edges.get(&e.id()).map(|ne| if e.front() == e.absolute_front() { ne.clone() } else { ne.inverse() }))
                        .collect::<Vec<_>>()
                        .into()
                })
                .collect();
            let mut nf = mt::Face::try_new(wires, f.surface()).map_err(|e| fail(&e.to_string()))?;
            if !f.orientation() {
                nf.invert();
            }
            faces.push(nf);
        }
        // Bodies of several pieces stay in several pieces (each face's provenance follows it).
        let prov: HashMap<mt::FaceID, FaceProv> = faces.iter().map(|f| f.id()).zip(from).collect();
        let shell: mt::Shell = faces.into();
        let solid = Solid::try_new(crate::polybool::components_in_order(&shell)).map_err(|e| fail(&e.to_string()))?;
        let from: Vec<FaceProv> = solid.face_iter().filter_map(|f| prov.get(&f.id()).cloned()).collect();
        // Each edge's origins in the solid's edge order.
        let origins = crate::body::edge_order(solid.face_iter()).0.iter().map(|e| origins.get(&e.id()).cloned()).collect();
        let keep: Vec<_> = a.split_keep().iter().chain(b.split_keep()).cloned().collect();
        crate::boolean_record::heal_build(solid, size, &keep, Some(from), origins, &record::counts(&[a, b]))
            .map(|body| body.with_split_lines(Some(std::sync::Arc::new(keep))))
    }))
}

/// Union of bodies touching back to back where a planar face of one lies wholly inside a
/// coincident face of the other, clear of its edges (a loft or revolve standing on a box top,
/// #396): the inner face goes, and the outer face takes its outline as a hole, sharing its
/// edges. `None` when no such pair of faces exists. The caller checks that the bodies don't
/// also overlap elsewhere.
pub(crate) fn inset(a: &Body, b: &Body) -> Option<Result<Body>> {
    inset_into(a, b, 0).or_else(|| inset_into(b, a, 1))
}

/// [`inset`] with a face of `inner` inside one of `outer`, `outer` being operand `outer_operand`.
fn inset_into(outer: &Body, inner: &Body, outer_operand: usize) -> Option<Result<Body>> {
    use mt::{BoundedCurve, ParametricCurve};
    const SAMPLES: usize = 64;
    const MAX_WORK: usize = 4_000_000;
    let size = outer.size().max(inner.size());
    let tol = (size * 1e-7).max(1e-9) * 10.0;
    // Clear of the outer face's edges by more than the samples' chord error.
    let clear = (size * 2e-3).max(1e-6);
    let sample = |w: &mt::Wire| -> Option<Vec<Vec3>> {
        if w.len() > 1000 {
            return None;
        }
        let mut pts = Vec::with_capacity(w.len() * SAMPLES);
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            pts.extend((0..SAMPLES).map(|k| from_p3(c.subs(t0 + (t1 - t0) * k as f64 / SAMPLES as f64))));
        }
        (!pts.is_empty() && pts.iter().all(|p| p.is_finite())).then_some(pts)
    };
    let fo: Vec<&mt::Face> = outer.solid.face_iter().collect();
    let fi: Vec<&mt::Face> = inner.solid.face_iter().collect();
    for (io, x) in &planes(outer) {
        for (ii, y) in &planes(inner) {
            if x.n.dot(y.n) > -1.0 + 1e-9 || (x.d + y.d).abs() > tol {
                continue;
            }
            let (Some(face_o), Some(face_i)) = (fo.get(*io), fi.get(*ii)) else { continue };
            let outline = match face_i.boundaries().as_slice() {
                [w] => w.clone(),
                _ => continue,
            };
            let (Some(inner_pts), Some(outer_loops)) = (sample(&outline), face_o.boundaries().iter().map(sample).collect::<Option<Vec<_>>>()) else {
                continue;
            };
            let outer_count: usize = outer_loops.iter().map(Vec::len).sum();
            if inner_pts.len().saturating_mul(outer_count) > MAX_WORK {
                continue;
            }
            let u = x.n.any_perp();
            let w = x.n.cross(u);
            let flat = |pts: &[Vec3]| {
                solidcraft_geom::Loop2::polygon(&pts.iter().map(|p| solidcraft_geom::Vec2::new(p.dot(u), p.dot(w))).collect::<Vec<_>>())
            };
            let flat_point = |p: Vec3| solidcraft_geom::Vec2::new(p.dot(u), p.dot(w));
            let outer_polys: Vec<_> = outer_loops.iter().map(|l| flat(l)).collect();
            let inner_poly = flat(&inner_pts);
            let in_outer = |p: Vec3| outer_polys.iter().filter(|l| l.contains(flat_point(p))).count() % 2 == 1;
            let near_outer =
                |p: Vec3| outer_loops.iter().any(|l| l.iter().zip(l.iter().cycle().skip(1)).any(|(s, t)| p.dist_to_segment(*s, *t) < clear));
            let fits = inner_pts.iter().all(|p| in_outer(*p) && !near_outer(*p))
                && outer_loops.iter().flatten().all(|p| !inner_poly.contains(flat_point(*p)));
            if !fits {
                continue;
            }
            return Some(guard("inset", || {
                let fail = |m: &str| KernelError::Failed(format!("joining a face standing inside another: {m}"));
                // The outline runs round the inner face's outward normal, the reverse of the
                // outer face's: the way a hole in the outer face runs.
                let mut wires = face_o.absolute_boundaries().clone();
                wires.push(if face_o.orientation() { outline.clone() } else { outline.inverse() });
                let mut holed = mt::Face::try_new(wires, face_o.surface()).map_err(|e| fail(&e.to_string()))?;
                if !face_o.orientation() {
                    holed.invert();
                }
                let inner_operand = 1 - outer_operand;
                let mut faces = Vec::with_capacity(fo.len() + fi.len());
                let mut from = Vec::with_capacity(fo.len() + fi.len());
                for (i, f) in fo.iter().enumerate() {
                    let (face, change) = if i == *io { (holed.clone(), Change::Modified) } else { ((*f).clone(), Change::Kept) };
                    faces.push(face);
                    from.push(record::from_face(outer_operand, i, change));
                }
                for (i, f) in fi.iter().enumerate().filter(|(i, _)| i != ii) {
                    faces.push((*f).clone());
                    from.push(record::from_face(inner_operand, i, Change::Kept));
                }
                let solid = Solid::try_new(vec![faces.into()]).map_err(|e| fail(&e.to_string()))?;
                let mut origins = record::edge_origins(&outer.solid, outer_operand);
                origins.extend(record::edge_origins(&inner.solid, inner_operand));
                let origins = crate::body::edge_order(solid.face_iter()).0.iter().map(|e| origins.get(&e.id()).cloned()).collect();
                let counts = if outer_operand == 0 { record::counts(&[outer, inner]) } else { record::counts(&[inner, outer]) };
                crate::boolean_record::heal_build(solid, size, &[], Some(from), origins, &counts)
            }));
        }
    }
    None
}

/// A cylindrical face: its index, a point on the axis, the unit axis and the radius.
#[derive(Clone, Copy, Debug)]
struct CylFace {
    index: usize,
    origin: Vec3,
    axis: Vec3,
    radius: f64,
}

fn cylinders(b: &Body) -> Vec<CylFace> {
    (0..b.face_count())
        .filter_map(|index| match crate::face_surface(b, index)? {
            crate::FaceSurface::Cylinder { origin, axis, radius } => Some(CylFace { index, origin, axis, radius }),
            _ => None,
        })
        .collect()
}

/// Do the two cylinders lie on the same surface?
fn same_cylinder(x: &CylFace, y: &CylFace, tol: f64) -> bool {
    let off = y.origin - x.origin;
    x.axis.dot(y.axis).abs() > 1.0 - 1e-9 && (x.radius - y.radius).abs() < tol && (off - x.axis * off.dot(x.axis)).len() < tol
}

/// The history over `[a, b]` of `r`, made in [`bore_union`]: `joined` (the target joined with
/// `narrowed`, the tool with its bore narrowed) less `cut`, the cylinder whose wall restores the
/// tool's bore faces `bore` (the tool is operand `tool_operand`). Nothing of the cylinder but its
/// wall may be left. Only a bore of one face: the cylinder's wall faces are split where its own
/// construction splits them, so which of several bore faces a piece of it lies over is not known.
fn recut(r: &Body, joined: &Body, narrowed: &Body, cut: &Body, bore: &[usize], tool_operand: usize) -> Option<History> {
    if bore.len() != 1 {
        return None;
    }
    // Over [target, tool].
    let joined = crate::boolean_record::through(joined, 1, narrowed.history())?;
    let wall = || record::region(bore.iter().map(|&face| FaceOrigin::Face { operand: 1, face }).collect(), false);
    let faces = cut
        .history()?
        .faces
        .iter()
        .map(|f| match f.origins.first() {
            Some(FaceOrigin::Profile { .. }) => wall(),
            _ => record::generated(Vec::new()),
        })
        .collect();
    let cut = record::from_faces(&cut.solid, 2, faces);
    let h = crate::boolean_record::of_pair(r, &Some(joined), &Some(cut))?;
    let map = if tool_operand == 1 { [Some(0), Some(1)] } else { [Some(1), Some(0)] };
    crate::boolean_record::composed(h.renumbered(&map, 2))
}

/// Do the bodies have cylindrical faces on a common cylinder?
pub(crate) fn has_coincident_cylinders(a: &Body, b: &Body) -> bool {
    let tol = (a.size().max(b.size()) * 1e-7).max(1e-9) * 10.0;
    let cb = cylinders(b);
    cylinders(a).iter().any(|x| cb.iter().any(|y| same_cylinder(x, y, tol)))
}

/// Union of bodies with bores on a common cylinder (a drain pad's bore around the drain hole
/// it is joined over). The mesh intersection can't take the coincident walls, so the tool's
/// bore is narrowed (its walls pushed toward the axis, now clear of the target's), the bodies
/// are joined, and the cylinder of the original bore is cut out again over the tool's length.
/// Recutting is valid only where that cylinder holds no material of either body. The
/// samples are plausibility checks; a whole-boundary guard rejects additional boundaries
/// crossing the recut. Original common-bore trims retain the legacy eligibility policy.
/// This is not a general emptiness or Boolean-equivalence certificate. Each direction
/// returns a result only when `accept` takes it; otherwise this returns `None`. The result's
/// history is over `[a, b]`.
///
/// A boss standing in the other body's bore on that cylinder (a disc joined into the bottom of
/// a hole it fills) is grown instead ([`grown_boss_union`]).
pub(crate) fn bore_union(a: &Body, b: &Body, accept: &dyn Fn(&Body) -> bool) -> Option<Body> {
    let size = a.size().max(b.size());
    let tol = (size * 1e-7).max(1e-9) * 10.0;
    let mtol = size * 2e-3;
    let (ma, mb) = (a.tessellate_with(mtol, true).ok()?, b.tessellate_with(mtol, true).ok()?);
    let (ia, ib) = (ma.inside_index(), mb.inside_index());
    let empty = |p: Vec3| !ia.contains(p) && !ib.contains(p);
    let (ca, cb) = (cylinders(a), cylinders(b));
    // (body to narrow, its operand number, its mesh and inside test, the other body and its
    // inside test, their faces on the cylinder).
    let ways = [(b, 1, &mb, &ib, a, &ia, &cb, &ca), (a, 0, &ma, &ia, b, &ib, &ca, &cb)];
    for (tool, tool_operand, mesh, inside, target, target_inside, ct, co) in ways {
        for c in ct {
            if !co.iter().any(|o| same_cylinder(c, o, tol)) {
                continue;
            }
            let bore: Vec<usize> = ct.iter().filter(|x| same_cylinder(c, x, tol)).map(|x| x.index).collect();
            // The tool's extent along the axis on that cylinder, and whether it is a bore (the
            // material outside it) at a point of the face.
            let (mut t0, mut t1) = (f64::INFINITY, f64::NEG_INFINITY);
            let mut probe = None;
            for (t, fi) in mesh.triangles.iter().zip(&mesh.tri_face) {
                if !bore.contains(&(*fi as usize)) {
                    continue;
                }
                let Some(ps) = mesh.tri(t) else { continue };
                for p in ps {
                    let s = (p - c.origin).dot(c.axis);
                    t0 = t0.min(s);
                    t1 = t1.max(s);
                }
                if probe.is_none() {
                    probe = Some((ps[0] + ps[1] + ps[2]) / 3.0);
                }
            }
            let Some(q) = probe else { continue };
            if !(t1 - t0 > tol && c.radius > tol) {
                continue;
            }
            let along = c.origin + c.axis * (q - c.origin).dot(c.axis);
            let Some(radial) = (q - along).normalized() else { continue };
            let on = along + radial * c.radius;
            let e = (c.radius * 0.05).max(size * 1e-4);
            if inside.contains(on - radial * e) && !inside.contains(on + radial * e) {
                let boss = Boss { tool, operand: tool_operand, cylinder: c, faces: &bore, t0, t1 };
                if let Some(r) = grown_boss_union(a, b, &boss, target_inside, size, accept) {
                    return Some(r);
                }
                continue;
            }
            if !(inside.contains(on + radial * e) && !inside.contains(on - radial * e)) {
                continue;
            }
            // The cylinder over the tool's length holds no material of either body.
            let (u, w) = (c.axis.any_perp(), c.axis.cross(c.axis.any_perp()));
            let at = |s: f64, r: f64, ang: f64| c.origin + c.axis * s + (u * ang.cos() + w * ang.sin()) * r;
            let clear = (1..=9).all(|i| {
                let s = t0 + (t1 - t0) * f64::from(i) / 10.0;
                empty(at(s, 0.0, 0.0))
                    && (0..8).all(|k| {
                        let ang = f64::from(k) * std::f64::consts::FRAC_PI_4 + 0.3;
                        empty(at(s, c.radius * 0.5, ang)) && empty(at(s, c.radius * 0.9, ang))
                    })
            });
            if !clear {
                continue;
            }
            // Past an end where the space is empty well beyond the radius too, the cut runs on a
            // little (so its cap doesn't lie in a face's plane). Not where the other body's bore
            // goes on: the cut's side would lie on its wall.
            let ext = size * 0.01;
            let past = |s: f64| {
                empty(at(s, 0.0, 0.0))
                    && (0..8).all(|k| {
                        let ang = f64::from(k) * std::f64::consts::FRAC_PI_4 + 0.3;
                        empty(at(s, c.radius * 0.9, ang)) && empty(at(s, c.radius * 1.1, ang)) && empty(at(s, c.radius * 1.5, ang))
                    })
            };
            let lo = if past(t0 - ext * 0.5) && past(t0 - ext) { t0 - ext } else { t0 };
            let hi = if past(t1 + ext * 0.5) && past(t1 + ext) { t1 + ext } else { t1 };
            // The samples above are plausibility checks, not proof that an
            // arbitrarily thin web is absent. Every additional current boundary
            // must be wholly clear of the recut; unsupported trims refuse.
            let other_bore: Vec<usize> = co.iter().filter(|x| same_cylinder(c, x, tol)).map(|x| x.index).collect();
            if crate::bore_guard::boundaries_clear(tool, &bore, c.origin, c.axis, c.radius, lo, hi, tol).is_none()
                || crate::bore_guard::boundaries_clear(target, &other_bore, c.origin, c.axis, c.radius, lo, hi, tol).is_none()
            {
                continue;
            }
            let d = (c.radius * 0.2).min(size * 0.01);
            let run = || -> Result<Option<Body>> {
                let narrowed = crate::offset::offset_body(tool, |i, _| if bore.contains(&i) { d } else { 0.0 })?;
                if !narrowed.validity().is_empty() {
                    return Err(KernelError::Failed("narrowing a bore: an unsound result".into()));
                }
                let Some(joined) = crate::ops::boolean(target, &narrowed, BoolOp::Union)? else { return Ok(None) };
                let cut = crate::build::cylinder(at(lo, 0.0, 0.0), c.axis, c.radius, hi - lo)?;
                let Some(r) = crate::ops::boolean(&joined, &cut, BoolOp::Cut)? else { return Ok(None) };
                let h = recut(&r, &joined, &narrowed, &cut, &bore, tool_operand);
                Ok(Some(r.with_checked_history(h, &record::counts(&[a, b]))))
            };
            let (result, diagnostics) = crate::capture_boolean_diagnostics(run);
            match result {
                Ok(Some(r)) if accept(&r) => {
                    crate::boolean_diagnostics::merge(diagnostics);
                    let mut event = crate::BooleanFallback::new(crate::BooleanStrategy::CommonBore);
                    event.face_shift_mm = d;
                    crate::boolean_diagnostics::record(event);
                    return Some(r);
                }
                _ => continue,
            }
        }
    }
    None
}

/// A tool (operand `operand`) with faces on a cylinder it shares with the other body, the
/// material inside them (a boss), and its extent `t0..t1` along the axis there.
struct Boss<'a> {
    tool: &'a Body,
    operand: usize,
    cylinder: &'a CylFace,
    faces: &'a [usize],
    t0: f64,
    t1: f64,
}

/// Union of `a` and `b` where one has a boss on a cylinder that is a bore of the other body (a disc joined into the bottom of a hole it fills): the boss's walls
/// coincide with the bore's, which the mesh intersection can't take. Grown outward by a step,
/// the boss reaches into the material around the bore instead, and joining it adds nothing
/// more, provided that material is there all along the boss. The ring the step adds is sampled
/// against the other body for that, and `accept` checks the result as a whole. The result's
/// history is over `[a, b]` (the grown tool keeps the tool's faces and edges).
fn grown_boss_union(
    a: &Body,
    b: &Body,
    boss: &Boss,
    target_inside: &solidcraft_geom::InsideIndex,
    size: f64,
    accept: &dyn Fn(&Body) -> bool,
) -> Option<Body> {
    let c = boss.cylinder;
    let d = (c.radius * 0.2).min(size * 0.01);
    let (u, w) = (c.axis.any_perp(), c.axis.cross(c.axis.any_perp()));
    let at = |s: f64, r: f64, ang: f64| c.origin + c.axis * s + (u * ang.cos() + w * ang.sin()) * r;
    // The other body's material all around the boss, as far out as it grows, over its length.
    let backed = (1..=9).all(|i| {
        let s = boss.t0 + (boss.t1 - boss.t0) * f64::from(i) / 10.0;
        (0..16).all(|k| {
            let ang = f64::from(k) * std::f64::consts::FRAC_PI_8 + 0.3;
            [0.1, 0.5, 0.95].iter().all(|f| target_inside.contains(at(s, c.radius + d * f, ang)))
        })
    });
    if !backed {
        return None;
    }
    // Ends where the bore comes out of the other body and the boss's cap lies flush with the
    // face it comes out of (a floor at the bottom of a through hole): faces in one plane, which
    // the intersection can't take either. There the cap is moved out by the same step, and the
    // stub that leaves past the face is deleted afterwards, the face healing over it.
    let tol = (size * 1e-7).max(1e-9) * 10.0;
    let target = if boss.operand == 1 { a } else { b };
    let along = |p: Vec3| (p - c.origin).dot(c.axis);
    let radial = |p: Vec3| (p - c.origin - c.axis * along(p)).len();
    let plane_at = |body: &Body, i: usize, s: f64, out: f64| {
        matches!(crate::face_surface(body, i), Some(crate::FaceSurface::Plane { normal, point })
            if normal.dot(c.axis) * out > 1.0 - 1e-9 && (along(point) - s).abs() < tol)
    };
    let mut caps: Vec<(usize, f64, f64)> = Vec::new();
    for (s, out) in [(boss.t0, -1.0), (boss.t1, 1.0)] {
        let open = (0..8).all(|k| {
            let ang = f64::from(k) * std::f64::consts::FRAC_PI_4 + 0.3;
            !target_inside.contains(at(s + out * d * 0.5, c.radius + d * 0.5, ang))
        });
        if !open || !(0..target.face_count()).any(|i| plane_at(target, i, s, out)) {
            continue;
        }
        let cap = boss
            .tool
            .solid
            .face_iter()
            .enumerate()
            .position(|(i, f)| plane_at(boss.tool, i, s, out) && f.vertex_iter().all(|v| radial(from_p3(v.point())) < c.radius + tol));
        // A flush end with no cap of its own to move: the faces would stay in one plane.
        caps.push((cap?, s, out));
    }
    let counts = record::counts(&[a, b]);
    let run = || -> Result<Option<Body>> {
        let grown = crate::offset::offset_body(boss.tool, |i, _| if boss.faces.contains(&i) || caps.iter().any(|x| x.0 == i) { d } else { 0.0 })?;
        if !grown.validity().is_empty() {
            return Err(KernelError::Failed("growing a boss: an unsound result".into()));
        }
        // In the operands' order, so the history is over [a, b] once the grown tool's own is
        // composed in.
        let joined = if boss.operand == 1 { crate::ops::boolean(a, &grown, BoolOp::Union)? } else { crate::ops::boolean(&grown, b, BoolOp::Union)? };
        let Some(joined) = joined else { return Ok(None) };
        let joined = {
            let h = crate::boolean_record::through(&joined, boss.operand, grown.history());
            joined.with_checked_history(h, &counts)
        };
        if caps.is_empty() {
            return Ok(Some(joined));
        }
        // The stubs: faces wholly past a flush end, within the grown boss, reaching out to its cap.
        let stubs: Vec<usize> = joined
            .solid
            .face_iter()
            .enumerate()
            .filter(|(_, f)| {
                let ps: Vec<Vec3> = f.vertex_iter().map(|v| from_p3(v.point())).collect();
                caps.iter().any(|&(_, s, out)| {
                    let past = |p: &Vec3| (along(*p) - s) * out;
                    !ps.is_empty() && ps.iter().all(|p| past(p) > -tol && radial(*p) < c.radius + d * 1.01) && ps.iter().any(|p| past(p) > d * 0.5)
                })
            })
            .map(|(i, _)| i)
            .collect();
        if stubs.is_empty() {
            return Err(KernelError::Failed("growing a boss: no stub past its flush end".into()));
        }
        let healed = crate::delete_faces(&joined, &stubs)?;
        let h = crate::boolean_record::through(&healed, 0, joined.history());
        Ok(Some(healed.with_checked_history(h, &counts)))
    };
    let (result, diagnostics) = crate::capture_boolean_diagnostics(run);
    match result {
        Ok(Some(r)) if accept(&r) => {
            crate::boolean_diagnostics::merge(diagnostics);
            let mut event = crate::BooleanFallback::new(crate::BooleanStrategy::GrownBoss);
            event.face_shift_mm = d;
            crate::boolean_diagnostics::record(event);
            Some(r)
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "coplanar_tests.rs"]
mod tests;
