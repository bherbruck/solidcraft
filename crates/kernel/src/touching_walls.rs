//! Cuts by a prism whose walls only touch the target. A corner of the tool's outline lies exactly
//! on the target's face (on a ring's bore, in its end face) with one wall going into the target
//! and the other staying outside it: both walls meet the target's face along the edge between
//! them, the loops the B-rep boolean finds there have no area, and it gives up on every try. Only
//! the last-resort nudge of the tool got through, after seconds, and left a face of no width at
//! the corner.
//!
//! A wall that lies outside the target takes nothing from it wherever it stands. Such walls are
//! pushed outward, past what the boolean takes for touching, their neighbours running on to meet
//! them, and the cut is taken with that tool: the same result, exactly, with the contact gone. A wall is pushed only where the
//! samples across its pushed place all lie outside the target's mesh, and the result is refused
//! if any of its faces lies on a pushed wall (that wall took material after all).

use solidcraft_geom::{InsideIndex, Vec3};
use truck_modeling as mt;

use crate::body::{Body, from_p3};

/// Most walls a tool may have for this route.
const MAX_WALLS: usize = 256;

/// Samples along and up each wall.
const SAMPLES: usize = 7;

/// Where across the slab a wall sweeps it is sampled, as fractions of the push: the pushed
/// wall itself, and two layers between.
const LAYERS: [f64; 3] = [0.125, 0.5, 1.0];

/// How far a vertex may move, in pushes: a corner sharper than about six degrees moves farther,
/// and is left as it is.
const MAX_MOVE: f64 = 20.0;

/// A tool with some of its walls pushed outward, and the planes they were pushed to (outward
/// unit normal and offset).
pub(crate) struct Pushed {
    pub(crate) tool: Body,
    planes: Vec<(Vec3, f64)>,
}

impl Pushed {
    /// Whether `result` has no face on a pushed wall's plane: the walls took nothing.
    pub(crate) fn untouched(&self, result: &Body) -> bool {
        let tol = (result.size() * 1e-9).max(1e-9);
        result.solid.face_iter().all(|f| {
            let mt::Surface::Plane(p) = f.oriented_surface() else { return true };
            let Some(n) = Vec3::new(p.normal().x, p.normal().y, p.normal().z).normalized() else { return true };
            !self.planes.iter().any(|(m, d)| n.dot(*m).abs() > 1.0 - 1e-9 && f.vertex_iter().all(|v| (m.dot(from_p3(v.point())) - d).abs() <= tol))
        })
    }
}

/// `b` with each wall pushed outward by `by` whose pushed place lies outside the target
/// (`target`, an index of its mesh, meshed finer than `by`): `b` must be a prism with planar caps
/// and planar walls, every edge a line, every vertex on two walls. The faces, edges and vertices
/// keep their order, so a boolean's history over the pushed tool holds for `b`. `None` when `b`
/// is no such prism, no wall can be pushed, or a corner would move too far.
pub(crate) fn pushed_tool(b: &Body, target: &InsideIndex, by: f64) -> Option<Pushed> {
    if !(by.is_finite() && by > 0.0) || b.solid.boundaries().len() != 1 {
        return None;
    }
    let faces: Vec<&mt::Face> = b.solid.face_iter().collect();
    if faces.len() < 5 || faces.len() > MAX_WALLS + 2 {
        return None;
    }
    let planes: Vec<(Vec3, f64)> = faces
        .iter()
        .map(|f| {
            let mt::Surface::Plane(p) = f.oriented_surface() else { return None };
            let n = Vec3::new(p.normal().x, p.normal().y, p.normal().z).normalized()?;
            Some((n, n.dot(from_p3(p.origin()))))
        })
        .collect::<Option<_>>()?;
    // The caps: a face whose normal every other face is square to, but for one facing away.
    let square = |n: Vec3, m: Vec3| n.dot(m).abs() < 1e-9;
    let caps = |i: usize| -> Option<usize> {
        let n = planes.get(i)?.0;
        let mut opposite = None;
        for (j, (m, _)) in planes.iter().enumerate() {
            if j == i || square(n, *m) {
                continue;
            }
            if n.dot(*m) < -1.0 + 1e-9 && opposite.is_none() {
                opposite = Some(j);
            } else {
                return None;
            }
        }
        opposite
    };
    let (top, bottom) = (0..planes.len()).find_map(|i| caps(i).map(|j| (i, j)))?;
    let d = planes.get(top)?.0;
    let size = b.size();
    let on = (size * 1e-9).max(1e-9);
    let walls: Vec<usize> = (0..faces.len()).filter(|i| *i != top && *i != bottom).collect();
    // Which walls go: every sample across the pushed place outside the target.
    let mut shift = vec![0.0; faces.len()];
    for &w in &walls {
        let (face, (n, o)) = (faces.get(w)?, planes.get(w)?);
        let corners: Vec<Vec3> = face.vertex_iter().map(|v| from_p3(v.point())).collect();
        if face.boundaries().len() != 1 || corners.len() != 4 {
            return None;
        }
        // The wall's two ends along its cap edge and its two heights.
        let along = d.cross(*n);
        let (lo_u, hi_u) = corners.iter().map(|p| p.dot(along)).fold((f64::INFINITY, f64::NEG_INFINITY), |m, x| (m.0.min(x), m.1.max(x)));
        let (lo_h, hi_h) = corners.iter().map(|p| p.dot(d)).fold((f64::INFINITY, f64::NEG_INFINITY), |m, x| (m.0.min(x), m.1.max(x)));
        let base = *n * *o;
        let clear = LAYERS.iter().all(|layer| {
            (0..SAMPLES).all(|i| {
                (0..SAMPLES).all(|j| {
                    let s = |k: usize, lo: f64, hi: f64| lo + (hi - lo) * (0.02 + 0.96 * k as f64 / (SAMPLES - 1) as f64);
                    let p = base + along * s(i, lo_u, hi_u) + d * s(j, lo_h, hi_h) + *n * (by * layer);
                    !target.contains(p)
                })
            })
        });
        if clear && let Some(x) = shift.get_mut(w) {
            *x = by;
        }
    }
    if shift.iter().all(|x| *x == 0.0) {
        return None;
    }
    let solid = b.deep_copy();
    let copied: Vec<&mt::Face> = solid.face_iter().collect();
    // Each vertex moves square to the walls so that it stays on both of its walls, pushed.
    let mut moved = std::collections::HashMap::new();
    for face in &copied {
        for v in face.vertex_iter() {
            if moved.contains_key(&v.id()) {
                continue;
            }
            let p = from_p3(v.point());
            let on_walls: Vec<usize> = walls.iter().copied().filter(|w| planes.get(*w).is_some_and(|(n, o)| (n.dot(p) - o).abs() <= on)).collect();
            let [i, j] = on_walls.as_slice() else { return None };
            let ((ni, _), (nj, _)) = (planes.get(*i)?, planes.get(*j)?);
            let (di, dj) = (*shift.get(*i)?, *shift.get(*j)?);
            let c = ni.dot(*nj);
            let det = 1.0 - c * c;
            let s = if di == 0.0 && dj == 0.0 {
                Vec3::ZERO
            } else if det < 1e-6 {
                return None;
            } else {
                *ni * ((di - c * dj) / det) + *nj * ((dj - c * di) / det)
            };
            if s.len() > MAX_MOVE * by {
                return None;
            }
            moved.insert(v.id(), (v.clone(), p + s));
        }
    }
    for (v, p) in moved.values() {
        v.set_point(mt::Point3::new(p.x, p.y, p.z));
    }
    // Edges run between their moved ends; each pushed wall is its plane moved along its normal.
    for face in &copied {
        for wire in face.boundaries() {
            for e in wire.edge_iter() {
                let mt::Curve::Line(_) = e.curve() else { return None };
                e.set_curve(mt::Curve::Line(mt::Line(e.absolute_front().point(), e.absolute_back().point())));
            }
        }
    }
    let mut pushed = Vec::new();
    for (k, face) in copied.iter().enumerate() {
        let (Some(&x), Some((n, o))) = (shift.get(k), planes.get(k)) else { return None };
        if x == 0.0 {
            continue;
        }
        let mt::Surface::Plane(p) = face.surface() else { return None };
        let t = mt::Vector3::new(n.x, n.y, n.z) * x;
        face.set_surface(mt::Surface::Plane(mt::Plane::new(p.origin() + t, p.origin() + p.u_axis() + t, p.origin() + p.v_axis() + t)));
        pushed.push((*n, o + x));
    }
    let tool = Body::new(solid).ok()?;
    if !tool.validity().is_empty() {
        return None;
    }
    Some(Pushed { tool, planes: pushed })
}
