//! Convex polyhedra from planes, and the operations that use them on convex planar bodies:
//! shell (hollow out with open faces) and draft (tilt faces about a neutral plane). The solid
//! is built directly from its planes (vertices are triple-plane intersections), so it is exact
//! and needs no booleans except the final shell subtraction.

use std::collections::HashMap;

use solidcraft_geom::{Plane, Vec3};
use truck_modeling::{self as mt, builder};

use crate::body::{Body, Solid, from_p3, p3, v3};
use crate::history::{EdgeOrigin, FaceOrigin, FaceProv, History};
use crate::record;
use crate::{KernelError, Result, guard};

/// A half-space `n · x <= d` (n is the outward unit normal).
#[derive(Clone, Copy, Debug)]
pub struct HalfSpace {
    pub n: Vec3,
    pub d: f64,
}

fn det3(a: Vec3, b: Vec3, c: Vec3) -> f64 {
    a.dot(b.cross(c))
}

/// Vertices, and per half-space with a face its vertex loop (counter-clockwise about the
/// outward normal), of the convex polyhedron bounded by the half-spaces.
fn combinatorics(hs: &[HalfSpace]) -> Result<(Vec<Vec3>, Vec<(usize, Vec<usize>)>)> {
    if hs.len() < 4 || hs.len() > 200 {
        return Err(KernelError::Invalid("a polyhedron needs 4…200 planes".into()));
    }
    let scale = hs.iter().map(|h| h.d.abs()).fold(1.0, f64::max);
    let eps = scale * 1e-9;
    // Vertices: intersections of three planes inside all others.
    let mut verts: Vec<Vec3> = Vec::new();
    for i in 0..hs.len() {
        for j in (i + 1)..hs.len() {
            for k in (j + 1)..hs.len() {
                let (a, b, c) = (hs[i], hs[j], hs[k]);
                let den = det3(a.n, b.n, c.n);
                if den.abs() < 1e-12 {
                    continue;
                }
                let x = (b.n.cross(c.n) * a.d + c.n.cross(a.n) * b.d + a.n.cross(b.n) * c.d) / den;
                if !x.is_finite() || hs.iter().any(|h| h.n.dot(x) > h.d + eps * 10.0) {
                    continue;
                }
                if !verts.iter().any(|v| v.dist(x) < scale * 1e-8) {
                    verts.push(x);
                }
            }
        }
    }
    if verts.len() < 4 {
        return Err(KernelError::Invalid("the planes don't enclose a solid".into()));
    }
    // Faces: the vertices on each plane, ordered counter-clockwise about its outward normal.
    let mut faces_idx: Vec<(usize, Vec<usize>)> = Vec::new();
    for (hi, h) in hs.iter().enumerate() {
        let on: Vec<usize> = (0..verts.len()).filter(|i| verts.get(*i).is_some_and(|v| (h.n.dot(*v) - h.d).abs() < scale * 1e-7)).collect();
        if on.len() < 3 {
            continue;
        }
        let c = on.iter().filter_map(|i| verts.get(*i)).fold(Vec3::ZERO, |a, v| a + *v) / on.len() as f64;
        let u = h.n.any_perp();
        let w = h.n.cross(u);
        let mut ordered = on.clone();
        ordered.sort_by(|a, b| {
            let ang = |i: usize| verts.get(i).map(|v| (*v - c).dot(w).atan2((*v - c).dot(u))).unwrap_or(0.0);
            ang(*a).total_cmp(&ang(*b))
        });
        faces_idx.push((hi, ordered));
    }
    Ok((verts, faces_idx))
}

/// The convex polyhedron bounded by the half-spaces.
pub fn convex_polyhedron(hs: &[HalfSpace]) -> Result<Body> {
    polyhedron(hs).map(|(b, _)| b)
}

/// [`convex_polyhedron`], with the half-space each of its faces lies on (in face order).
fn polyhedron(hs: &[HalfSpace]) -> Result<(Body, Vec<usize>)> {
    let (verts, faces_idx) = combinatorics(hs)?;
    let on: Vec<usize> = faces_idx.iter().map(|(hi, _)| *hi).collect();
    guard("polyhedron", || {
        let tv: Vec<mt::Vertex> = verts.iter().map(|v| builder::vertex(p3(*v))).collect();
        let mut edges: HashMap<(usize, usize), mt::Edge> = HashMap::new();
        let mut faces: Vec<mt::Face> = Vec::new();
        for (_, f) in &faces_idx {
            let n = f.len();
            let mut wire: Vec<mt::Edge> = Vec::with_capacity(n);
            for k in 0..n {
                let (Some(&a), Some(&b)) = (f.get(k), f.get((k + 1) % n)) else { continue };
                let key = (a.min(b), a.max(b));
                let e = match edges.get(&key) {
                    Some(e) => e.clone(),
                    None => {
                        let (Some(va), Some(vb)) = (tv.get(key.0), tv.get(key.1)) else { continue };
                        let e = builder::line(va, vb);
                        edges.insert(key, e.clone());
                        e
                    }
                };
                wire.push(if a < b { e } else { e.inverse() });
            }
            let face = builder::try_attach_plane(&[wire.into()]).map_err(|e| KernelError::Failed(format!("polyhedron face: {e}")))?;
            faces.push(face);
        }
        let shell: mt::Shell = faces.into();
        let solid = Solid::try_new(vec![shell]).map_err(|e| KernelError::Failed(format!("polyhedron: {e}")))?;
        Ok((Body::new(solid)?, on))
    })
}

/// The half-spaces of a convex body with only planar faces, each with a point on it and the
/// body's faces on its plane (in face order), or an error.
pub fn planar_convex(b: &Body) -> Result<Vec<(HalfSpace, Vec3, Vec<usize>)>> {
    b.require_brep("this operation")?;
    let tol = (b.size() * 1e-3).max(1e-3);
    let faces = b.faces(tol)?;
    let mut out: Vec<(HalfSpace, Vec3, Vec<usize>)> = Vec::new();
    for f in &faces {
        let n = f.plane_normal.ok_or_else(|| KernelError::Failed("not supported yet: bodies with curved faces".into()))?;
        let d = n.dot(f.centroid);
        match out.iter_mut().find(|(h, ..)| h.n.dot(n) > 1.0 - 1e-9 && (h.d - d).abs() < tol) {
            Some((.., on)) => on.push(f.index),
            None => out.push((HalfSpace { n, d }, f.centroid, vec![f.index])),
        }
    }
    let verts: Vec<Vec3> = b.solid.vertex_iter().map(|v| from_p3(v.point())).collect();
    for (h, ..) in &out {
        if verts.iter().any(|v| h.n.dot(*v) > h.d + tol) {
            return Err(KernelError::Failed("not supported yet: shell and draft of non-convex bodies".into()));
        }
    }
    Ok(out)
}

/// The faces of `b` on half-space `k`'s plane (`groups[k]`) as the face over them: moved, or
/// merged when several.
fn on_plane(under: &[usize]) -> FaceProv {
    record::region(under.iter().map(|&face| FaceOrigin::Face { operand: 0, face }).collect(), false)
}

/// The edges of `b` between faces on two of its planes (`groups[k]`: its faces on plane `k`),
/// by pair of planes (the lower first).
fn plane_edges(b: &Body, groups: &[Vec<usize>]) -> HashMap<(usize, usize), Vec<usize>> {
    let plane_of: HashMap<usize, usize> = groups.iter().enumerate().flat_map(|(k, g)| g.iter().map(move |f| (*f, k))).collect();
    let mut out: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (edge, fs) in crate::history::edge_faces(&b.solid).iter().enumerate() {
        if let [p, q] = fs[..]
            && let (Some(&x), Some(&y)) = (plane_of.get(&p), plane_of.get(&q))
            && x != y
        {
            out.entry((x.min(y), x.max(y))).or_default().push(edge);
        }
    }
    out
}

/// The history over `[b]` of a convex polyhedron `poly` whose face `j` lies on the plane of
/// half-space `on[j]`, made from the faces `groups[k]` of `b` on half-space `k`'s plane: each
/// face the faces under it, each edge along the edges of `b` between faces of its two planes,
/// or new where they met nowhere.
fn planes_history(poly: &Body, on: &[usize], groups: &[Vec<usize>], b: &Body) -> Option<History> {
    let faces: Vec<FaceProv> = on.iter().map(|k| groups.get(*k).map(|under| on_plane(under))).collect::<Option<_>>()?;
    let along = plane_edges(b, groups);
    let mut h = record::from_faces(&poly.solid, 1, faces);
    for e in &mut h.edges {
        if let [x, y] = e.faces[..]
            && let (Some(&kx), Some(&ky)) = (on.get(x), on.get(y))
            && let Some(edges) = along.get(&(kx.min(ky), kx.max(ky)))
        {
            e.origins = edges.iter().map(|&edge| EdgeOrigin::Edge { operand: 0, edge }).collect();
        }
    }
    Some(h)
}

/// The history over `[b]` of a cavity `poly` whose face `j` lies on half-space `on[j]`, moved
/// in from the faces `groups[k]` of `b`: each face an offset of the faces it lines, each edge new.
fn cavity_history(poly: &Body, on: &[usize], groups: &[Vec<usize>]) -> Option<History> {
    let faces = on
        .iter()
        .map(|k| groups.get(*k).map(|under| record::generated(under.iter().map(|&face| FaceOrigin::Offset { operand: 0, face }).collect())))
        .collect::<Option<_>>()?;
    Some(record::from_faces(&poly.solid, 1, faces))
}

/// Hollow a convex planar body: walls of `thickness` inside, faces near `open` removed. The
/// result's history is over `[b]`: the inner walls are offsets of the faces they line.
pub fn shell(b: &Body, open: &[Vec3], thickness: f64) -> Result<Body> {
    guard("shell", || shell_impl(b, open, thickness)?.with_shell_planar_sources(b, thickness, open))
}

fn shell_impl(b: &Body, open: &[Vec3], thickness: f64) -> Result<Body> {
    b.require_brep("shell")?;
    if !(thickness.is_finite() && thickness > 1e-6) {
        return Err(KernelError::Invalid("shell thickness must be positive".into()));
    }
    let hs = match planar_convex(b) {
        Ok(h) => h,
        Err(_) => return shell_planar(b, open, thickness),
    };
    let size = b.size();
    let inner: Vec<HalfSpace> = hs
        .iter()
        .map(|(h, c, _)| {
            let removed = open.iter().any(|p| (h.n.dot(*p) - h.d).abs() < size * 1e-4 + 1e-6 && p.dist(*c) < size * 2.0);
            if removed { HalfSpace { n: h.n, d: h.d + thickness + size * 0.05 } } else { HalfSpace { n: h.n, d: h.d - thickness } }
        })
        .collect();
    if !hs.iter().zip(&inner).any(|((h, ..), i)| i.d > h.d) {
        return Err(KernelError::Invalid("select at least one face to remove".into()));
    }
    let (cavity, on) = polyhedron(&inner)?;
    let groups: Vec<Vec<usize>> = hs.into_iter().map(|(.., under)| under).collect();
    let walls = cavity_history(&cavity, &on, &groups);
    let r = crate::ops::boolean(b, &cavity, crate::BoolOp::Cut)?.ok_or_else(|| KernelError::Failed("the shell removed everything".into()))?;
    Ok(hollowed(r, b, walls))
}

/// `r`, `b` less a cavity whose history over `[b]` is `walls`, with its history over `[b]`.
fn hollowed(r: Body, b: &Body, walls: Option<History>) -> Body {
    let h = crate::boolean_record::of_pair(&r, &crate::boolean_record::kept(b, 0, 1), &walls);
    r.with_checked_history(h, &record::counts(&[b]))
}

/// The history over `[b]` of a cavity `offset` made from `b` by moving its faces (whose history
/// over `[b]` says so): each face an offset of the face it was moved from, each edge new.
fn offsets(offset: &Body) -> Option<History> {
    let h = offset.history()?;
    let faces: Vec<FaceProv> = h
        .faces
        .iter()
        .map(|f| match f.origins.as_slice() {
            [FaceOrigin::Face { operand, face }] => Some(record::generated(vec![FaceOrigin::Offset { operand: *operand, face: *face }])),
            _ => None,
        })
        .collect::<Option<_>>()?;
    Some(record::from_faces(&offset.solid, h.operands, faces))
}

/// Tilt the faces near `faces` by `angle` about their line on the neutral plane, so that they
/// lean in (positive angle) going along `pull`.
pub fn draft(b: &Body, faces: &[Vec3], neutral: &Plane, pull: Vec3, angle: f64) -> Result<Body> {
    b.require_brep("draft")?;
    if !angle.is_finite() || angle.abs() >= std::f64::consts::FRAC_PI_2 - 1e-3 {
        return Err(KernelError::Invalid("draft angle must be between −90° and 90°".into()));
    }
    let pull = pull.normalized().ok_or_else(|| KernelError::Invalid("pull direction".into()))?;
    let hs = match planar_convex(b) {
        Ok(h) => h,
        Err(_) => return draft_walls(b, faces, neutral, pull, angle),
    };
    let size = b.size();
    let mut out = Vec::new();
    let mut moved = 0;
    for (h, c, _) in &hs {
        let chosen = faces.iter().any(|p| (h.n.dot(*p) - h.d).abs() < size * 1e-4 + 1e-6 && p.dist(*c) < size * 2.0);
        if !chosen {
            out.push(*h);
            continue;
        }
        if h.n.dot(pull).abs() > 1e-6 {
            return Err(KernelError::Failed("not supported yet: drafting faces that aren't parallel to the pull direction".into()));
        }
        // Hinge: the face's line on the neutral plane; rotate the normal toward the pull.
        let n2 = (h.n * angle.cos() + pull * angle.sin()).normalized().ok_or_else(|| KernelError::Failed("draft".into()))?;
        let hinge_pt =
            neutral.intersect_ray(*c, pull).ok_or_else(|| KernelError::Failed("the neutral plane is parallel to the pull direction".into()))?;
        out.push(HalfSpace { n: n2, d: n2.dot(hinge_pt) });
        moved += 1;
    }
    if moved == 0 {
        return Err(KernelError::Invalid("no faces to draft".into()));
    }
    // Each half-space in place of the one it was made from: its faces moved (or as they were).
    let (r, on) = polyhedron(&out)?;
    let groups: Vec<Vec<usize>> = hs.into_iter().map(|(.., under)| under).collect();
    let h = planes_history(&r, &on, &groups, b);
    Ok(r.with_checked_history(h, &record::counts(&[b])))
}

/// Draft of a non-convex body: walls (planes, cylinders) next to caps square to `pull`.
fn draft_walls(b: &Body, at: &[Vec3], neutral: &Plane, pull: Vec3, angle: f64) -> Result<Body> {
    let healed = crate::boolean_record::healed(b)?;
    let size = healed.size();
    let mesh = healed.tessellate((size * 1e-3).max(1e-3))?;
    let mut chosen: Vec<usize> = Vec::new();
    for p in at {
        let near = mesh
            .triangles
            .iter()
            .zip(&mesh.tri_face)
            .filter_map(|(t, f)| mesh.tri(t).map(|[x, y, z]| (point_tri(*p, x, y, z), *f as usize)))
            .min_by(|a, c| a.0.total_cmp(&c.0));
        match near {
            Some((d, f)) if d < size * 1e-3 + 1e-6 => {
                if !chosen.contains(&f) {
                    chosen.push(f);
                }
            }
            _ => return Err(KernelError::Invalid(format!("no face at {:?}", [p.x, p.y, p.z]))),
        }
    }
    // A round wall is stored in pieces (a cylinder's two halves): picking one drafts the whole wall.
    let chosen = crate::offset::with_same_surface(&healed, &chosen);
    Ok(record::after(crate::offset::draft_walls(&healed, &chosen, neutral, pull, angle)?, &healed, &record::counts(&[b])))
}

/// Round every edge of a convex body with planar faces by `r`: faces shrink, each edge becomes a
/// cylinder and each corner a sphere patch (the body is the inner polyhedron grown by a ball).
/// The result's history is over `[b]`: each cylinder in place of the edges of `b` between its
/// two planes' faces, each corner patch in place of those round its corner.
pub fn round_all_edges(b: &Body, r: f64) -> Result<Body> {
    let planes = planar_convex(b)?;
    let hs: Vec<HalfSpace> = planes.iter().map(|(h, ..)| *h).collect();
    let groups: Vec<Vec<usize>> = planes.iter().map(|(.., under)| under.clone()).collect();
    // The edges of `b` between faces on planes f and g.
    let edges = plane_edges(b, &groups);
    let along = |f: usize, g: usize| -> Vec<FaceOrigin> {
        edges.get(&(f.min(g), f.max(g))).into_iter().flatten().map(|&edge| FaceOrigin::Edge { operand: 0, edge }).collect()
    };
    let inner: Vec<HalfSpace> = hs.iter().map(|h| HalfSpace { n: h.n, d: h.d - r }).collect();
    let (q, faces) = combinatorics(&inner)?;
    if faces.len() != hs.len() {
        return Err(KernelError::Failed("not supported yet: the rounding is larger than a face".into()));
    }
    let normal = |hi: usize| hs.get(hi).map(|h| h.n).unwrap_or(Vec3::Z);
    // Edges of the inner polyhedron: (vertex a, vertex b) → (face left, face right).
    let mut edge_faces: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (hi, lp) in &faces {
        let n = lp.len();
        for k in 0..n {
            let (Some(&a), Some(&bb)) = (lp.get(k), lp.get((k + 1) % n)) else { continue };
            edge_faces.entry((a.min(bb), a.max(bb))).or_default().push(*hi);
        }
    }
    if edge_faces.values().any(|f| f.len() != 2) {
        return Err(KernelError::Failed("not supported yet: rounding this shape".into()));
    }
    guard("round edges", || {
        // Vertex of face `hi` at inner vertex `qi`.
        let mut verts: HashMap<(usize, usize), mt::Vertex> = HashMap::new();
        let mut vert = |hi: usize, qi: usize| -> mt::Vertex {
            verts.entry((hi, qi)).or_insert_with(|| builder::vertex(p3(q.get(qi).copied().unwrap_or_default() + normal(hi) * r))).clone()
        };
        let mut lines: HashMap<(usize, usize, usize), mt::Edge> = HashMap::new();
        let mut arcs: HashMap<(usize, usize, usize), mt::Edge> = HashMap::new();
        // Straight edge of face hi between inner vertices a and b (stored low → high).
        let mut line = |hi: usize, a: usize, bb: usize, vert: &mut dyn FnMut(usize, usize) -> mt::Vertex| -> mt::Edge {
            let (lo, hi_v) = (a.min(bb), a.max(bb));
            let e = lines.entry((hi, lo, hi_v)).or_insert_with(|| builder::line(&vert(hi, lo), &vert(hi, hi_v))).clone();
            if a < bb { e } else { e.inverse() }
        };
        // Arc at inner vertex qi from face f to face g (stored with f < g).
        let mut arc = |qi: usize, f: usize, g: usize, vert: &mut dyn FnMut(usize, usize) -> mt::Vertex| -> mt::Edge {
            let (lo, hi_f) = (f.min(g), f.max(g));
            let c = q.get(qi).copied().unwrap_or_default();
            let e = arcs
                .entry((qi, lo, hi_f))
                .or_insert_with(|| {
                    let mid = c + (normal(lo) + normal(hi_f)).normalized().unwrap_or(Vec3::Z) * r;
                    builder::circle_arc(&vert(lo, qi), &vert(hi_f, qi), p3(mid))
                })
                .clone();
            if f < g { e } else { e.inverse() }
        };
        let mut out: Vec<mt::Face> = Vec::new();
        // What each face is made from (in `out`'s order).
        let mut made: Vec<FaceProv> = Vec::new();
        // Planar faces.
        for (hi, lp) in &faces {
            let n = lp.len();
            let mut w: Vec<mt::Edge> = Vec::new();
            for k in 0..n {
                let (Some(&a), Some(&bb)) = (lp.get(k), lp.get((k + 1) % n)) else { continue };
                w.push(line(*hi, a, bb, &mut vert));
            }
            out.push(builder::try_attach_plane(&[w.into()]).map_err(|e| KernelError::Failed(format!("rounded face: {e}")))?);
            made.push(on_plane(groups.get(*hi).map_or(&[], Vec::as_slice)));
        }
        use mt::{ParametricSurface3D, SearchNearestParameter};
        let orient = |mut surf: mt::Surface, at: Vec3, outward: Vec3| -> mt::Surface {
            if let Some((u, v)) = surf.search_nearest_parameter(p3(at), None, 100) {
                let nn = surf.normal(u, v);
                if Vec3::new(nn.x, nn.y, nn.z).dot(outward) < 0.0 {
                    surf = mt::Invertible::inverse(&surf);
                }
            }
            surf
        };
        // Edge cylinders: face f's loop runs a → b, so the cylinder (to f's right) runs b → a
        // along f, then across to g.
        for ((a, bb), fs) in &edge_faces {
            let (Some(&f0), Some(&f1)) = (fs.first(), fs.get(1)) else { continue };
            // Which face runs a → b?
            let runs = |hi: usize| {
                faces.iter().find(|(h, _)| *h == hi).is_some_and(|(_, lp)| {
                    let n = lp.len();
                    (0..n).any(|k| lp.get(k) == Some(a) && lp.get((k + 1) % n) == Some(bb))
                })
            };
            let (f, g) = if runs(f0) { (f0, f1) } else { (f1, f0) };
            let w: Vec<mt::Edge> =
                vec![line(f, *bb, *a, &mut vert), arc(*a, f, g, &mut vert), line(g, *a, *bb, &mut vert), arc(*bb, g, f, &mut vert)];
            let (qa, qb) = (q.get(*a).copied().unwrap_or_default(), q.get(*bb).copied().unwrap_or_default());
            let profile = arc(*a, f, g, &mut vert);
            let swept = builder::tsweep(&profile, v3(qb - qa));
            let out_dir = (normal(f) + normal(g)).normalized().unwrap_or(Vec3::Z);
            let mid = (qa + qb) * 0.5 + out_dir * r;
            let surf = orient(swept.oriented_surface(), mid, out_dir);
            out.push(mt::Face::try_new(vec![w.into()], surf).map_err(|e| KernelError::Failed(format!("rounded edge: {e}")))?);
            made.push(record::generated(along(f, g)));
        }
        // Corner sphere patches: the faces around each inner vertex, counter-clockwise.
        for (qi, c) in q.iter().enumerate() {
            let mut around: Vec<usize> = faces.iter().filter(|(_, lp)| lp.contains(&qi)).map(|(h, _)| *h).collect();
            let axis = around.iter().fold(Vec3::ZERO, |acc, h| acc + normal(*h)).normalized().unwrap_or(Vec3::Z);
            let u = axis.any_perp();
            let w2 = axis.cross(u);
            around.sort_by(|x, y| {
                let ang = |h: usize| normal(h).dot(w2).atan2(normal(h).dot(u));
                ang(*x).total_cmp(&ang(*y))
            });
            let k = around.len();
            let w: Vec<mt::Edge> = (0..k).filter_map(|i| Some(arc(qi, *around.get(i)?, *around.get((i + 1) % k)?, &mut vert))).collect();
            // A sphere surface whose poles and seam are away from this patch.
            let side = axis.any_perp();
            let (np, sp, back) = (*c + side * r, *c - side * r, *c - axis * r);
            let meridian = builder::circle_arc(&builder::vertex(p3(np)), &builder::vertex(p3(sp)), p3(back));
            let surf =
                mt::Surface::RevolutedCurve(mt::Processor::new(mt::RevolutedCurve::by_revolution(meridian.oriented_curve(), p3(*c), v3(side))));
            let surf = orient(surf, *c + axis * r, axis);
            out.push(mt::Face::try_new(vec![w.into()], surf).map_err(|e| KernelError::Failed(format!("rounded corner: {e}")))?);
            let mut corner: Vec<FaceOrigin> =
                (0..k).filter_map(|i| Some((*around.get(i)?, *around.get((i + 1) % k)?))).flat_map(|(f, g)| along(f, g)).collect();
            corner.sort();
            corner.dedup();
            made.push(record::generated(corner));
        }
        let shell: mt::Shell = out.into();
        let solid = Solid::try_new(vec![shell]).map_err(|e| KernelError::Failed(format!("rounded body: {e}")))?;
        let body = Body::new(solid)?;
        // Every face from something of `b` (a blend between planes that met nowhere has none).
        let history = made.iter().all(|f| !f.origins.is_empty()).then(|| record::from_faces(&body.solid, 1, made));
        Ok(body.with_checked_history(history, &record::counts(&[b])))
    })
}

/// Shell of a body whose faces are all planar (convex or not): every face plane moves inward
/// by the thickness (open faces outward, past the body) keeping the body's topology, which
/// gives the cavity; the shell is the body minus the cavity.
fn shell_planar(b: &Body, open: &[Vec3], thickness: f64) -> Result<Body> {
    // Face paint and authored splits are construction boundaries. Retain their
    // eligibility veto through the temporary body used for the cavity Cut.
    // Paint indices stay in source order by avoiding healing in that case.
    let healed =
        if b.paint().is_some_and(|paint| !paint.faces.is_empty()) { crate::boolean_record::copied(b)? } else { crate::boolean_record::healed(b)? };
    let healed = healed.with_paint(b.paint().cloned()).with_split_lines(b.split_lines().cloned()).inherit_regularization_protection(b);
    let size = healed.size();
    let mesh = healed.tessellate((size * 1e-3).max(1e-3))?;
    // Faces to open: the face of the triangle nearest each point.
    let mut opened: Vec<usize> = Vec::new();
    for p in open {
        let near = mesh
            .triangles
            .iter()
            .zip(&mesh.tri_face)
            .filter_map(|(t, f)| mesh.tri(t).map(|[x, y, z]| (point_tri(*p, x, y, z), *f as usize)))
            .min_by(|a, c| a.0.total_cmp(&c.0));
        if let Some((d, f)) = near
            && d < size * 1e-3 + 1e-6
            && !opened.contains(&f)
        {
            opened.push(f);
        }
    }
    if opened.is_empty() {
        return Err(KernelError::Invalid("select at least one face to remove".into()));
    }
    // A successful Boolean is not evidence that a tangential rim has wall thickness.
    // Refuse only when the entire planar opening loses its rim. A single collapsed
    // round can still end in a valid sharp edge (the tutorial plate, for example).
    if let Some(r) = rim_without_thickness(&healed, &opened, thickness, true) {
        return Err(unsupported_shell(&format!(
            "the rim would have no thickness: an open face meets a round of radius {r:.4} mm tangentially, and the shell is {thickness:.4} mm thick (make the round smaller than the thickness)"
        )));
    }
    let margin = thickness + size * 0.05;
    shell_opened(b, &healed, open, &opened, thickness, margin).map_err(|e| match rim_without_thickness(&healed, &opened, thickness, false) {
        Some(r) => unsupported_shell(&format!(
            "the rim would have no thickness: an open face meets a round of radius {r:.4} mm tangentially, and the shell is {thickness:.4} mm thick (make the round smaller than the thickness)"
        )),
        None => e,
    })
}

/// The radius of a convex round (cylinder or torus) that meets an open face tangentially and
/// is at least `thickness`: the cavity's wall then passes through the rim where the round meets
/// the open face, leaving nothing between them.
fn rim_without_thickness(b: &Body, opened: &[usize], thickness: f64, whole_rim: bool) -> Option<f64> {
    use mt::{BoundedCurve, ParametricCurve};
    let faces: Vec<&mt::Face> = b.solid.face_iter().take(if whole_rim { 4097 } else { usize::MAX }).collect();
    let tol = (b.size() * 1e-7).max(1e-9) * 100.0;
    // A distant shell can make the body's tolerance larger than this wall.
    // Such a diagnostic candidate is not sufficient to refuse a valid opening.
    if whole_rim && tol > thickness * 1e-3 {
        return None;
    }
    opened.iter().filter_map(|fi| faces.get(*fi)).find_map(|open| {
        let plane = crate::offset::surf_of(open, tol)?;
        if whole_rim && (!matches!(plane, crate::offset::Surf::Plane { .. }) || faces.len() > 4096 || open.edge_iter().take(1025).count() > 1024) {
            return None;
        }
        let radius = |edge: &mt::Edge| {
            faces.iter().filter(|g| !std::ptr::eq(**g, *open)).find_map(|g| {
                if !g.edge_iter().any(|e| e.id() == edge.id()) {
                    return None;
                }
                let round = crate::offset::surf_of(g, tol)?;
                let r = match round {
                    crate::offset::Surf::Cylinder { r, convex: true, .. } | crate::offset::Surf::Torus { r, convex: true, .. } => r,
                    _ => return None,
                };
                let c = edge.curve();
                let (t0, t1) = c.range_tuple();
                if !t0.is_finite() || !t1.is_finite() || t0 >= t1 {
                    return None;
                }
                let parameter = t0 * 0.5 + t1 * 0.5;
                if !parameter.is_finite() || parameter < t0 || parameter > t1 {
                    return None;
                }
                let mid = from_p3(c.subs(parameter));
                let tangent = plane.normal(mid)?.dot(round.normal(mid)?) > 1.0 - 1e-6;
                (tangent && r >= thickness - tol).then_some(r)
            })
        };
        if whole_rim {
            let mut found = None;
            for edge in open.edge_iter() {
                found = Some(radius(&edge)?);
            }
            found
        } else {
            open.edge_iter().find_map(|edge| radius(&edge))
        }
    })
}

fn shell_opened(b: &Body, healed: &Body, open: &[Vec3], opened: &[usize], thickness: f64, margin: f64) -> Result<Body> {
    let size = healed.size();
    // Open faces pushed out past the body: the simple way, unless a face tangent to an open
    // one (a fillet along its edge) can't follow; then every face moves in (tangencies keep)
    // and the opening is the open face's copy swept out through the body.
    let cavity = match offset_planar(healed, |fi, _| if opened.contains(&fi) { margin } else { -thickness }) {
        Ok(c) => c,
        Err(e) => return shell_by_opening(healed, opened, thickness, margin).map_err(|_| e),
    };
    let cavity = match opening_overreach(healed, &cavity, opened, thickness, margin) {
        Ok(Some(trimmed)) => trimmed,
        Ok(None) => cavity,
        Err(e) => {
            log::warn!("shell: could not trim the opening to the open faces ({e}); the wall next to it may be cut");
            cavity
        }
    };
    let r = crate::ops::boolean(healed, &cavity, crate::BoolOp::Cut);
    if r.is_err()
        && let Ok(x) = shell_by_opening(healed, opened, thickness, margin)
    {
        return Ok(x);
    }
    match r {
        Ok(Some(r)) => {
            // Over `[healed]`, the cavity's faces offsets of the faces they line; then over `[b]`.
            Ok(record::after(hollowed(r, healed, offsets(&cavity)), healed, &record::counts(&[b])))
        }
        Ok(None) => Err(KernelError::Failed("the shell removed everything".into())),
        // Healing can leave faces the boolean dislikes (revolved patches): the body as it was,
        // its faces to open found again.
        Err(e) => {
            let raw = b.tessellate((size * 1e-3).max(1e-3)).map_err(|_| e.clone())?;
            let mut opened_raw: Vec<usize> = Vec::new();
            for p in open {
                let near = raw
                    .triangles
                    .iter()
                    .zip(&raw.tri_face)
                    .filter_map(|(t, f)| raw.tri(t).map(|[x, y, z]| (point_tri(*p, x, y, z), *f as usize)))
                    .min_by(|a, c| a.0.total_cmp(&c.0));
                if let Some((d, f)) = near
                    && d < size * 1e-3 + 1e-6
                    && !opened_raw.contains(&f)
                {
                    opened_raw.push(f);
                }
            }
            let opened = opened_raw;
            let cavity = offset_planar(b, |fi, _| if opened.contains(&fi) { margin } else { -thickness }).map_err(|_| e.clone())?;
            let r = crate::ops::boolean(b, &cavity, crate::BoolOp::Cut)
                .map_err(|_| e)?
                .ok_or_else(|| KernelError::Failed("the shell removed everything".into()))?;
            Ok(hollowed(r, b, offsets(&cavity)))
        }
    }
}

/// The cavity with each open face pushed out (`cavity`), trimmed so that beyond an open face's
/// plane it only reaches through the open face itself; `None` when it already does.
///
/// The pushed-out face carries the cavity past every edge of the open face. Past a convex edge
/// that is outside the body, but past a concave one (a box on a box, its side face open where it
/// meets the lower box's top) it runs on into the neighbouring wall and cuts a strip through it,
/// as wide as the push. The trimmed cavity is the shell's cavity with every face moved in,
/// together with the pushed-out cavity's parts that are on the body's side of every open face's
/// plane or in front of that open face.
fn opening_overreach(healed: &Body, cavity: &Body, opened: &[usize], thickness: f64, margin: f64) -> Result<Option<Body>> {
    let size = healed.size();
    let tol = size * 1e-6 + 1e-9;
    let faces: Vec<mt::Face> = healed.solid.face_iter().cloned().collect();
    // Each open face's plane (outward normal, offset) and its triangles.
    let mesh = healed.tessellate((size * 1e-3).max(1e-3))?;
    let mut open: Vec<(&mt::Face, Vec3, f64, Vec<[Vec3; 3]>)> = Vec::new();
    for fi in opened {
        let Some(f) = faces.get(*fi) else { return Ok(None) };
        let mt::Surface::Plane(pl) = f.oriented_surface() else { return Ok(None) };
        let n = pl.normal();
        let Some(n) = Vec3::new(n.x, n.y, n.z).normalized() else { return Ok(None) };
        let d = n.dot(from_p3(pl.origin()));
        let tris: Vec<[Vec3; 3]> =
            mesh.triangles.iter().zip(&mesh.tri_face).filter(|(_, f)| **f as usize == *fi).filter_map(|(t, _)| mesh.tri(t)).collect();
        if tris.is_empty() {
            return Ok(None);
        }
        open.push((f, n, d, tris));
    }
    // Only a corner of the cavity past an open face's plane and not in front of that face can be
    // in a wall; with none, the cavity is right as it is.
    let overreach = cavity.solid.vertex_iter().any(|v| {
        let q = from_p3(v.point());
        open.iter().any(|(_, n, d, tris)| {
            let h = n.dot(q) - d;
            h > tol && {
                let on = q - *n * h;
                !tris.iter().any(|[a, b, c]| point_tri(on, *a, *b, *c) < size * 1e-4 + 1e-6)
            }
        })
    });
    if !overreach {
        return Ok(None);
    }
    let closed = offset_planar(healed, |_, _| -thickness)?;
    // Keep: on the body's side of each open plane, or in front of the open face.
    let reach = size * 4.0;
    let mut keep: Option<Body> = None;
    for (f, n, d, _) in &open {
        let c = *n * *d;
        let mut hs = vec![HalfSpace { n: *n, d: *d }];
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            hs.push(HalfSpace { n: axis, d: axis.dot(c) + reach });
            hs.push(HalfSpace { n: axis * -1.0, d: -axis.dot(c) + reach });
        }
        let (side, _) = polyhedron(&hs)?;
        // In front of the face: its prism, from half a wall inside the body (overlapping `side`
        // rather than touching it) to past the pushed-out face.
        let inset = thickness * 0.5;
        let prism = guard("shell opening prism", || {
            let start = builder::translated(*f, v3(*n * -inset));
            let solid: Solid = builder::tsweep(&start, v3(*n * (inset + margin * 2.0)));
            Ok(solid)
        })?;
        let prism = Body::new(prism)?;
        let k = crate::ops::boolean(&side, &prism, crate::BoolOp::Union)?.ok_or_else(|| KernelError::Failed("shell: opening".into()))?;
        keep = Some(match keep {
            None => k,
            Some(acc) => crate::ops::boolean(&acc, &k, crate::BoolOp::Intersect)?.ok_or_else(|| KernelError::Failed("shell: opening".into()))?,
        });
    }
    let Some(keep) = keep else { return Ok(None) };
    let kept = crate::ops::boolean(cavity, &keep, crate::BoolOp::Intersect)?.ok_or_else(|| KernelError::Failed("shell: opening".into()))?;
    let trimmed = crate::ops::boolean(&closed, &kept, crate::BoolOp::Union)?.ok_or_else(|| KernelError::Failed("shell: opening".into()))?;
    Ok(Some(trimmed))
}

/// A shell made from the cavity with every face moved in by the thickness (open faces too),
/// joined with each open face's moved copy swept out through the body. No history: the
/// opening's walls, swept from the cavity's edges, come from nothing of `b`.
fn shell_by_opening(b: &Body, opened: &[usize], thickness: f64, reach: f64) -> Result<Body> {
    let closed = offset_planar(b, |_, _| -thickness)?;
    let faces: Vec<mt::Face> = closed.solid.face_iter().cloned().collect();
    let mut cavity = closed.clone();
    for fi in opened {
        let f = faces.get(*fi).ok_or_else(|| KernelError::Failed("shell: open face".into()))?;
        let mt::Surface::Plane(pl) = f.oriented_surface() else {
            return Err(unsupported_shell("the open face must be planar here"));
        };
        let n = pl.normal();
        let n = Vec3::new(n.x, n.y, n.z).normalized().ok_or_else(|| KernelError::Failed("shell: open face normal".into()))?;
        let prism = guard("shell opening", || {
            let solid: Solid = builder::tsweep(f, v3(n * (thickness + reach)));
            Ok(solid)
        })?;
        let prism = Body::new(prism)?;
        cavity = crate::ops::boolean(&cavity, &prism, crate::BoolOp::Union)?.ok_or_else(|| KernelError::Failed("shell: opening".into()))?;
    }
    let r = crate::ops::boolean(b, &cavity, crate::BoolOp::Cut)?.ok_or_else(|| KernelError::Failed("the shell removed everything".into()))?;
    Ok(r.without_history())
}

fn unsupported_shell(m: &str) -> KernelError {
    KernelError::Failed(format!("not supported yet: {m}"))
}

pub(crate) fn point_tri(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f64 {
    let n = (b - a).cross(c - a);
    let Some(nn) = n.normalized() else { return f64::INFINITY };
    let h = (p - a).dot(nn);
    let q = p - nn * h;
    let inside = [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).cross(q - *u).dot(n) >= -1e-12);
    if inside { h.abs() } else { [(a, b), (b, c), (c, a)].iter().map(|(u, v)| p.dist_to_segment(*u, *v)).fold(f64::INFINITY, f64::min) }
}

/// The body with every planar face moved along its outward normal by `shift(face index,
/// normal)`, keeping the topology: each vertex goes to where its faces' moved planes meet.
pub(crate) fn offset_planar(b: &Body, shift: impl Fn(usize, Vec3) -> f64) -> Result<Body> {
    crate::offset::offset_body(b, shift)
}

pub(crate) fn solve3_pub(m: [[f64; 3]; 3], r: [f64; 3]) -> Option<[f64; 3]> {
    solve3(m, r)
}

fn solve3(m: [[f64; 3]; 3], r: [f64; 3]) -> Option<[f64; 3]> {
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(m);
    if d.abs() < 1e-9 {
        return None;
    }
    let mut out = [0.0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let mut mk = m;
        for (row, rv) in mk.iter_mut().zip(r) {
            row[k] = rv;
        }
        *o = det(mk) / d;
    }
    Some(out)
}

/// Move the planar faces at the given points along their outward normals by `distance`
/// (negative: into the body). The body's other faces follow (its topology is kept).
pub fn offset_faces(b: &Body, at: &[Vec3], distance: f64) -> Result<Body> {
    b.require_brep("offset faces")?;
    if !distance.is_finite() || distance.abs() > 1e6 {
        return Err(KernelError::Invalid("offset distance".into()));
    }
    let healed = crate::boolean_record::healed(b)?.with_split_lines(b.splits.clone());
    let size = healed.size();
    let mesh = healed.tessellate((size * 1e-3).max(1e-3))?;
    let mut chosen: Vec<usize> = Vec::new();
    for p in at {
        let near = mesh
            .triangles
            .iter()
            .zip(&mesh.tri_face)
            .filter_map(|(t, f)| mesh.tri(t).map(|[x, y, z]| (point_tri(*p, x, y, z), *f as usize)))
            .min_by(|a, c| a.0.total_cmp(&c.0));
        match near {
            Some((d, f)) if d < size * 1e-3 + 1e-6 => {
                if !chosen.contains(&f) {
                    chosen.push(f);
                }
            }
            _ => return Err(KernelError::Invalid(format!("no face at {:?}", [p.x, p.y, p.z]))),
        }
    }
    let chosen = crate::offset::with_same_surface(&healed, &chosen);
    match offset_planar(&healed, |fi, _| if chosen.contains(&fi) { distance } else { 0.0 }) {
        Ok(r) => Ok(record::after(r, &healed, &record::counts(&[b]))),
        // A piece of a split face (its neighbour stays): the piece swept along its normal
        // joins on (outward) or cuts in, with walls along the split lines.
        Err(e) if !healed.split_keep().is_empty() => press_pieces(&healed, &chosen, distance).map_err(|_| e),
        Err(e) => Err(e),
    }
}

/// No history: the walls along the split lines are swept from edges, and come from nothing of
/// `b`.
fn press_pieces(b: &Body, chosen: &[usize], distance: f64) -> Result<Body> {
    let faces: Vec<mt::Face> = b.solid.face_iter().cloned().collect();
    let mut cur = b.clone();
    for fi in chosen {
        let f = faces.get(*fi).ok_or_else(|| KernelError::Failed("offset: face".into()))?;
        let n = crate::blend::plane_normal(f).ok_or_else(|| KernelError::Failed("offset: only planar pieces".into()))?;
        let tool = guard("offset face", || Body::new(builder::tsweep(f, crate::body::v3(n * distance))))?;
        let op = if distance > 0.0 { crate::BoolOp::Union } else { crate::BoolOp::Cut };
        cur = crate::ops::boolean(&cur, &tool, op)?.ok_or_else(|| KernelError::Failed("offset: the piece moved off the body".into()))?;
    }
    Ok(cur.with_split_lines(b.splits.clone()).without_history())
}

#[cfg(test)]
mod rim_preflight_tests {
    use super::*;

    #[test]
    fn a_distant_shell_does_not_turn_a_smaller_local_round_into_a_zero_rim() {
        let block = crate::box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
        let edges = [Vec3::new(20.0, 0.0, 0.0), Vec3::new(40.0, 15.0, 0.0), Vec3::new(20.0, 30.0, 0.0), Vec3::new(0.0, 15.0, 0.0)];
        let rounded = crate::fillet(&block, &edges, 1.5).unwrap();
        let distant = crate::box_solid(Vec3::new(1_000_000.0, 0.0, 0.0), Vec3::new(1_000_010.0, 10.0, 10.0)).unwrap();
        let combined = crate::boolean(&rounded, &distant, crate::BoolOp::Union).unwrap().unwrap();
        let faces: Vec<_> = (0..combined.solid.face_iter().count()).collect();
        assert!(rim_without_thickness(&combined, &faces, 2.0, true).is_none());
    }
}
