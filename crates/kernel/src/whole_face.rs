//! Extrudes from a whole planar face of a body (#2040). The profile of a face is its outline
//! projected into a sketch, and an edge that is no line or arc comes in fitted: the elliptic
//! arcs where a fillet meets a slanted face are cubic splines a hair off the body's edges. A
//! join's tool then touches the body along a face it does not match, and a cut's walls lie a
//! hair inside or outside the body all round: the boolean finds pieces of no area and gives up.
//!
//! When the profile is the face's outline (to within the fit), the face itself is used: a join
//! sweeps the body's own face, which shares its edges with the body and is sewn on along it, and
//! a cut whose walls take nothing from the body (all of it within the cut's depth lies over the
//! face) is the body cut by a slab beyond the depth plane, which meets it only across that plane.

use solidcraft_geom::{Mesh, Plane, Region2, Vec2, Vec3};
use truck_modeling as mt;

use crate::body::{Body, v3};
use crate::history::{Change, History};
use crate::{KernelError, Result};

/// Most boundary samples compared per outline.
const MAX_SAMPLES: usize = 20_000;

/// A planar face of `target` and where it lies.
#[derive(Clone, Copy, Debug)]
pub struct WholeFace {
    /// The face's index in `target`.
    pub face: usize,
    /// Its outward normal.
    pub normal: Vec3,
    /// Its height along the sketch normal, from the sketch plane's origin.
    pub height: f64,
}

/// How far a fitted outline may lie from the face's edges: the projection fits a smooth chain to
/// within 1e-4 of its extent (`doc::project`), and the mesh the edges are sampled from sags.
fn outline_tol(target: &Body) -> f64 {
    (target.size() * 5e-4).max(1e-4)
}

/// The planar face of `target` at one of `heights` along `plane`'s normal (from its origin)
/// whose outline is `region`'s outer loop, to within the projection's fit, with no holes on
/// either side.
pub fn matching_face(target: &Body, plane: &Plane, region: &Region2, heights: &[f64]) -> Option<WholeFace> {
    if !region.holes.is_empty() {
        return None;
    }
    let n = plane.normal();
    let tol = outline_tol(target);
    let mesh = target.tessellate(tol * 0.2).ok()?;
    let ring: Vec<Vec2> = region.outer.polyline(tol * 0.1);
    if ring.len() < 3 || ring.len() > MAX_SAMPLES {
        return None;
    }
    let faces: Vec<&mt::Face> = target.solid.face_iter().collect();
    for (i, f) in faces.iter().enumerate() {
        let Some(m) = crate::blend::plane_normal(f) else { continue };
        if m.dot(n).abs() < 1.0 - 1e-9 || f.boundaries().len() != 1 {
            continue;
        }
        let edges = face_outline(&mesh, i);
        let Some(height) = edges.first().and_then(|e| e.first()).map(|p| (*p - plane.origin).dot(n)) else { continue };
        if !heights.iter().any(|h| (h - height).abs() <= tol) {
            continue;
        }
        // The outline in the sketch's plane, at the face's height.
        let at = |p: Vec2| plane.to_world(p) + n * height;
        let ring3: Vec<Vec3> = ring.iter().map(|p| at(*p)).collect();
        let segs: Vec<(Vec3, Vec3)> = edges.iter().flat_map(|e| e.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?)))).collect();
        if segs.is_empty() || segs.len() > MAX_SAMPLES {
            continue;
        }
        let near_edges = |p: Vec3| segs.iter().any(|(a, b)| p.dist_to_segment(*a, *b) <= tol);
        let ring_segs: Vec<(Vec3, Vec3)> = (0..ring3.len()).filter_map(|k| Some((*ring3.get(k)?, *ring3.get((k + 1) % ring3.len())?))).collect();
        let near_ring = |p: Vec3| ring_segs.iter().any(|(a, b)| p.dist_to_segment(*a, *b) <= tol);
        let off_plane = segs.iter().any(|(a, _)| ((*a - plane.origin).dot(n) - height).abs() > tol);
        if !off_plane && ring3.iter().all(|p| near_edges(*p)) && segs.iter().all(|(a, _)| near_ring(*a)) {
            return Some(WholeFace { face: i, normal: m, height });
        }
    }
    None
}

/// The boundary polylines of face `face` of `mesh`, seams left out.
fn face_outline(mesh: &Mesh, face: usize) -> Vec<Vec<Vec3>> {
    let Ok(f) = u32::try_from(face) else { return Vec::new() };
    mesh.face_edges(f).into_iter().filter(|e| !mesh.seams.get(*e).copied().unwrap_or(false)).filter_map(|e| mesh.edges.get(e).cloned()).collect()
}

/// The prism `built` (made from a profile that [`matching_face`] matched to `face`) made again
/// from the face itself: the face swept `length` along its outward normal, `outward` or back
/// into the body. Each of its faces comes from the face of `built` it lies on, so the feature
/// names its faces as it named `built`'s.
pub fn face_prism(target: &Body, face: WholeFace, length: f64, outward: bool, built: &Body) -> Result<Body> {
    if !length.is_finite() || length <= 0.0 {
        return Err(KernelError::Invalid("a prism from a face needs a positive length".into()));
    }
    let f = target.solid.face_iter().nth(face.face).cloned().ok_or_else(|| KernelError::Invalid("no such face".into()))?;
    let along = if outward { face.normal * length } else { face.normal * -length };
    let swept = crate::guard("prism from a face", || Body::new(mt::builder::tsweep(&f, v3(along))))?;
    traced_onto(swept, built, outline_tol(target))
}

/// `swept`, made in place of `built`, with each of its faces coming from the face of `built`
/// nearest it (so a feature names its faces as it named `built`'s).
fn traced_onto(swept: Body, built: &Body, tol: f64) -> Result<Body> {
    let (mine, theirs) = (swept.tessellate(tol)?, built.tessellate(tol)?);
    let mut faces = Vec::with_capacity(swept.face_count());
    for i in 0..swept.face_count() {
        let probe = face_probe(&mine, i).ok_or_else(|| KernelError::Failed("tool from a face: a face with no mesh".into()))?;
        let from = nearest_face(&theirs, probe).ok_or_else(|| KernelError::Failed("tool from a face: no face under it".into()))?;
        faces.push(crate::record::from_face(0, from, Change::Modified));
    }
    let history: History = crate::record::from_faces(&swept.solid, 1, faces);
    swept.with_history_counts(history, &crate::record::counts(&[built]))
}

/// The centroid of face `face`'s largest triangle.
fn face_probe(mesh: &Mesh, face: usize) -> Option<Vec3> {
    let f = u32::try_from(face).ok()?;
    mesh.triangles
        .iter()
        .zip(&mesh.tri_face)
        .filter(|(_, tf)| **tf == f)
        .filter_map(|(t, _)| mesh.tri(t))
        .max_by(|x, y| (x[1] - x[0]).cross(x[2] - x[0]).len().total_cmp(&(y[1] - y[0]).cross(y[2] - y[0]).len()))
        .map(|[a, b, c]| (a + b + c) / 3.0)
}

/// The face of `mesh` nearest `p`.
fn nearest_face(mesh: &Mesh, p: Vec3) -> Option<usize> {
    mesh.triangles
        .iter()
        .zip(&mesh.tri_face)
        .filter_map(|(t, f)| mesh.tri(t).map(|[a, b, c]| (crate::polyhedron::point_tri(p, a, b, c), *f)))
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .and_then(|(_, f)| usize::try_from(f).ok())
}

/// Whether a cut `depth` into `target` from `face` takes only what lies over the face: nothing of
/// the body stands beyond the face's plane, and everything of it less than `depth` below lies
/// over the face (on its outline at most). Then the prism over the face and a slab over the whole
/// body take the same material, and the slab meets the body only across its depth plane.
pub fn cut_takes_only_the_face(target: &Body, face: WholeFace, depth: f64) -> bool {
    if !depth.is_finite() || depth <= 0.0 {
        return false;
    }
    let tol = outline_tol(target);
    let Ok(mesh) = target.tessellate(tol * 0.2) else { return false };
    let Some(origin) = face_outline(&mesh, face.face).first().and_then(|e| e.first().copied()) else { return false };
    let n = face.normal;
    // The face's plane with the outline in it, flattened.
    let Some(x) = (if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y }).cross(n).normalized() else { return false };
    let y = n.cross(x);
    let flat = |p: Vec3| Vec2::new((p - origin).dot(x), (p - origin).dot(y));
    let segs: Vec<(Vec2, Vec2)> = face_outline(&mesh, face.face)
        .iter()
        .flat_map(|e| e.windows(2).filter_map(|w| Some((flat(*w.first()?), flat(*w.get(1)?)))).collect::<Vec<_>>())
        .collect();
    if segs.len() < 3 || segs.len() > MAX_SAMPLES {
        return false;
    }
    let mut points = mesh.positions.clone();
    points.extend(mesh.triangles.iter().filter_map(|t| mesh.tri(t)).map(|[a, b, c]| (a + b + c) / 3.0));
    points.iter().all(|p| {
        let h = (*p - origin).dot(n);
        h <= tol && (h <= -depth + tol || over(&segs, flat(*p), tol))
    })
}

/// How a wall swept into the body from an edge of the face meets the body along that edge, from
/// the face beside it (outward normal `n_N`) and the sweep's direction `−n_P`: `n_N · (−n_P)`
/// above zero leaves the body (the wall only touches it along the edge), zero lies in the face
/// beside it, below zero goes into the material (a real cut).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wall {
    Touches,
    Cuts,
}

/// The walls of each edge of face `face` of `mesh` (by mesh edge), `None` where an edge's wall
/// changes from one to the other along it.
fn walls(mesh: &Mesh, face: usize, n: Vec3, tol: f64) -> Option<Vec<(usize, Wall)>> {
    let f = u32::try_from(face).ok()?;
    let mut out = Vec::new();
    for e in mesh.face_edges(f) {
        if mesh.seams.get(e).copied().unwrap_or(false) {
            continue;
        }
        let other = mesh.edge_faces.get(e)?.iter().copied().find(|g| *g != f)?;
        let pts = mesh.edges.get(e)?;
        let tris: Vec<[Vec3; 3]> = mesh.triangles.iter().zip(&mesh.tri_face).filter(|(_, g)| **g == other).filter_map(|(t, _)| mesh.tri(t)).collect();
        let dist = |p: Vec3, t: &[Vec3; 3]| crate::polyhedron::point_tri(p, t[0], t[1], t[2]);
        // Each point's nearest triangle of the face beside, by its normal.
        let mut signs = pts.iter().filter_map(|p| {
            let [a, b, c] = *tris.iter().min_by(|x, y| dist(*p, x).total_cmp(&dist(*p, y)))?;
            let m = (b - a).cross(c - a).normalized()?;
            Some(if -m.dot(n) >= -tol { Wall::Touches } else { Wall::Cuts })
        });
        let first = signs.next()?;
        if signs.any(|s| s != first) {
            return None;
        }
        out.push((e, first));
    }
    Some(out)
}

/// Whether `q` lies inside the outline made of `segs` (in any order and direction, by even-odd
/// crossings) or within `tol` of it.
fn over(segs: &[(Vec2, Vec2)], q: Vec2, tol: f64) -> bool {
    let on = segs.iter().any(|(a, b)| {
        let ab = *b - *a;
        let t = if ab.len2() > 0.0 { ((q - *a).dot(ab) / ab.len2()).clamp(0.0, 1.0) } else { 0.0 };
        q.dist(*a + ab * t) <= tol
    });
    on || segs.iter().filter(|(a, b)| (a.y > q.y) != (b.y > q.y) && q.x < a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x)).count() % 2 == 1
}

/// Even-odd containment of `q` in the closed polygon `poly`.
fn inside(poly: &[Vec2], q: Vec2) -> bool {
    let m = poly.len();
    (0..m)
        .filter(|i| match (poly.get(*i), poly.get((i + 1) % m)) {
            (Some(a), Some(b)) => (a.y > q.y) != (b.y > q.y) && q.x < a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x),
            _ => false,
        })
        .count()
        % 2
        == 1
}

/// A tool cutting `depth` into `target` from its planar face `face`, as the face swept would, with
/// the walls that only touch the body moved off it: the face's outline keeps its edges whose walls
/// cut into the body, exactly (the body's own edges), and runs round the others well outside it.
/// The tool starts a little out from the face, so its cap does not lie on it. `None` when no wall
/// cuts (the face's prism takes only what is over it, see [`cut_takes_only_the_face`]), when an
/// edge's wall changes from touching to cutting along it, or when anything of the body within the
/// depth lies where the outline runs round. Its faces come from those of `built` (the profile's
/// own tool) they lie nearest, when given. With how far out from the face it starts.
pub fn cut_tool(target: &Body, face: WholeFace, depth: f64, built: Option<&Body>) -> Option<(Body, f64)> {
    use mt::{BoundedCurve, ParametricCurve};
    use std::f64::consts::{PI, TAU};
    if !depth.is_finite() || depth <= 0.0 {
        return None;
    }
    let size = target.size();
    let tol = outline_tol(target);
    let mesh = target.tessellate(tol * 0.2).ok()?;
    let n = face.normal;
    let walls = walls(&mesh, face.face, n, 1e-6)?;
    if !walls.iter().any(|(_, w)| *w == Wall::Cuts) || !walls.iter().any(|(_, w)| *w == Wall::Touches) {
        return None;
    }
    let f = target.solid.face_iter().nth(face.face)?.clone();
    let wire = f.absolute_boundaries().first()?.clone();
    // Each edge of the face's outline (in order) with its wall, matched to the mesh's edges by
    // their middles.
    let mid = |e: &mt::Edge| {
        let c = e.curve();
        let (t0, t1) = c.range_tuple();
        crate::body::from_p3(c.subs((t0 + t1) * 0.5))
    };
    let wall_of = |e: &mt::Edge| {
        let m = mid(e);
        walls
            .iter()
            .filter_map(|(i, w)| {
                let pts = mesh.edges.get(*i)?;
                Some((pts.windows(2).filter_map(|s| Some(m.dist_to_segment(*s.first()?, *s.get(1)?))).fold(f64::INFINITY, f64::min), *w))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .filter(|(d, _)| *d <= tol)
            .map(|(_, w)| w)
    };
    let edges: Vec<(mt::Edge, Wall)> = wire.edge_iter().map(|e| wall_of(e).map(|w| (e.clone(), w))).collect::<Option<_>>()?;
    // Start at a cutting edge that follows a touching one.
    let k = edges.len();
    let start =
        (0..k).find(|i| edges.get(*i).is_some_and(|e| e.1 == Wall::Cuts) && edges.get((i + k - 1) % k).is_some_and(|e| e.1 == Wall::Touches))?;
    let edges: Vec<(mt::Edge, Wall)> = edges.iter().cycle().skip(start).take(k).cloned().collect();
    // The plane's frame, round the face's middle.
    let origin = face_probe(&mesh, face.face)?;
    let x = (if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y }).cross(n).normalized()?;
    let y = n.cross(x);
    let flat = |p: Vec3| Vec2::new((p - origin).dot(x), (p - origin).dot(y));
    let lift = |q: Vec2| origin + x * q.x + y * q.y;
    let outline: Vec<(Vec2, Vec2)> = face_outline(&mesh, face.face)
        .iter()
        .flat_map(|e| e.windows(2).filter_map(|w| Some((flat(*w.first()?), flat(*w.get(1)?)))).collect::<Vec<_>>())
        .collect();
    let centre = outline.iter().fold(Vec2::ZERO, |a, (p, _)| a + *p) / outline.len().max(1) as f64;
    let far = size * 2.0;
    let at = |v: &mt::Vertex| flat(crate::body::from_p3(v.point()));
    // Which way the outline runs round its middle.
    let turn: f64 = edges.iter().map(|(e, _)| (at(e.front()) - centre).cross(at(e.back()) - centre)).sum::<f64>().signum();
    let mut out: Vec<mt::Edge> = Vec::new();
    let mut detours: Vec<Vec<Vec2>> = Vec::new();
    let mut i = 0;
    while let Some((e, w)) = edges.get(i) {
        if *w == Wall::Cuts {
            out.push(e.clone());
            i += 1;
            continue;
        }
        // A run of touching edges from vertex `a` to vertex `b`: round it, far out.
        let a = e.front().clone();
        let mut j = i;
        while edges.get(j).is_some_and(|e| e.1 == Wall::Touches) {
            j += 1;
        }
        let b = edges.get(j - 1)?.0.back().clone();
        let (pa, pb) = (at(&a), at(&b));
        let (da, db) = ((pa - centre).normalized()?, (pb - centre).normalized()?);
        let (t0, mut t1) = (da.y.atan2(da.x), db.y.atan2(db.x));
        // Round the same way as the run, which goes from a to b with the outline.
        while (t1 - t0) * turn <= 0.0 {
            t1 += TAU * turn;
        }
        let steps = (((t1 - t0).abs() / (PI / 8.0)).ceil() as usize).clamp(2, 64);
        // A polygon round a circle `far` out (its corners 1/cos(π/16) further), clear of it.
        let r = far / (PI / 16.0).cos();
        let mut path = vec![pa];
        path.extend((0..=steps).map(|s| {
            let t = t0 + (t1 - t0) * s as f64 / steps as f64;
            centre + Vec2::new(t.cos(), t.sin()) * if s == 0 || s == steps { far } else { r }
        }));
        path.push(pb);
        let mut verts = vec![a];
        verts.extend(path.iter().skip(1).take(path.len().saturating_sub(2)).map(|q| mt::builder::vertex(crate::body::p3(lift(*q)))));
        verts.push(b);
        out.extend(verts.windows(2).filter_map(|v| Some(mt::builder::line(v.first()?, v.get(1)?))));
        detours.push(path);
        i = j;
    }
    // Nothing of the body within the depth may lie where the outline runs round.
    let clear = mesh.positions.iter().all(|p| {
        let h = (*p - origin).dot(n);
        let q = flat(*p);
        h >= tol || h <= -depth + tol || !detours.iter().any(|d| inside(d, q)) || over(&outline, q, tol)
    });
    if !clear {
        return None;
    }
    let mut base = mt::Face::try_new(vec![mt::Wire::from(out)], f.surface()).ok()?;
    if !f.orientation() {
        base.invert();
    }
    // Out from the face, then swept back through it and the depth.
    let off = (size * 1e-2).max(tol * 10.0);
    let base = mt::builder::translated(&base, v3(n * off));
    let tool = crate::guard("cut tool from a face", || Body::new(mt::builder::tsweep(&base, v3(n * -(off + depth))))).ok()?;
    match built {
        Some(built) => traced_onto(tool, built, tol).ok().map(|t| (t, off)),
        None => Some((tool, off)),
    }
}

#[cfg(test)]
#[path = "whole_face_tests.rs"]
mod tests;
