//! Hole walls: the inward-facing cylinders of a body, each with its extent along its axis and
//! how much of the turn it covers, for finding the holes and slots a fastener can pass through.
//!
//! Faces are classified from the tessellation (as [`crate::merged_topology`] does), so a hole
//! whose wall is a B-spline (a hole in a refolded sheet part) is found as well as an exact
//! cylinder. The pieces of one cylinder (the kernel splits a full turn into two halves) are
//! merged into one wall.

use solidcraft_geom::{Mesh, Vec3};

use crate::Body;
use crate::topo::{Surf, classify_faces};

/// Faces looked at in one body: a larger body yields no walls rather than a slow scan.
const MAX_FACES: usize = 20_000;

/// Angular bins over a full turn (5° each).
const BINS: usize = 72;

/// One inward-facing cylinder of a body (the material outside it).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct HoleWall {
    /// A point on the axis.
    pub axis_point: Vec3,
    /// Unit axis.
    pub axis: Vec3,
    pub radius: f64,
    /// The wall's extent along `axis`, measured from `axis_point`.
    pub start: f64,
    pub end: f64,
    /// How much of the turn the wall covers (radians, to 5°): 2π for a round hole, about π for
    /// one end of an obround slot.
    pub sweep: f64,
    /// For a wall that does not close (a slot's end), the unit direction from the axis to the
    /// middle of its opening.
    pub opening: Option<Vec3>,
}

/// The inward-facing cylindrical walls of `b`. `mesh` is a tessellation of `b` (its display
/// mesh will do). Empty for a mesh body or one with more than 20,000 faces.
pub fn hole_walls(b: &Body, mesh: &Mesh) -> Vec<HoleWall> {
    let nf = b.face_count();
    if b.mesh.is_some() || nf == 0 || nf > MAX_FACES {
        return Vec::new();
    }
    let tol = (b.size() * 1e-4).max(1e-6);
    let surfs = classify_faces(b, mesh, nf, tol);
    // Group the faces that lie on one cylinder.
    let mut groups: Vec<(Surf, Vec<u32>)> = Vec::new();
    for (face, s) in surfs.iter().enumerate() {
        if !matches!(s, Surf::Cylinder { .. }) {
            continue;
        }
        let Ok(face) = u32::try_from(face) else { continue };
        match groups.iter_mut().find(|(g, _)| g.same(s, tol * 10.0)) {
            Some((_, faces)) => faces.push(face),
            None => groups.push((*s, vec![face])),
        }
    }
    groups.iter().flat_map(|(s, faces)| along_apart(mesh, *s, faces, tol)).filter_map(|(s, faces)| wall(mesh, s, &faces)).collect()
}

/// The faces of one cylinder in runs along its axis that do not overlap: two holes on one line
/// in one body (a folded part's two flanges drilled through together) are two walls, not one
/// running across the gap between them.
fn along_apart(mesh: &Mesh, s: Surf, faces: &[u32], tol: f64) -> Vec<(Surf, Vec<u32>)> {
    let Surf::Cylinder { axis, p, .. } = s else { return Vec::new() };
    let mut spans: Vec<(f64, f64, u32)> = faces.iter().map(|f| (f64::INFINITY, f64::NEG_INFINITY, *f)).collect();
    for (t, f) in mesh.triangles.iter().zip(&mesh.tri_face) {
        let Some(span) = spans.iter_mut().find(|x| x.2 == *f) else { continue };
        for k in t {
            if let Some(q) = mesh.positions.get(*k as usize) {
                let along = (*q - p).dot(axis);
                span.0 = span.0.min(along);
                span.1 = span.1.max(along);
            }
        }
    }
    spans.retain(|x| x.0 <= x.1);
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut runs: Vec<(f64, Vec<u32>)> = Vec::new();
    for (lo, hi, f) in spans {
        match runs.last_mut() {
            Some((end, run)) if lo <= *end + tol * 10.0 => {
                *end = end.max(hi);
                run.push(f);
            }
            _ => runs.push((hi, vec![f])),
        }
    }
    runs.into_iter().map(|(_, run)| (s, run)).collect()
}

fn wall(mesh: &Mesh, s: Surf, faces: &[u32]) -> Option<HoleWall> {
    let Surf::Cylinder { axis, p, r } = s else { return None };
    let u = axis.any_perp().normalized()?;
    let v = axis.cross(u);
    let angle = |q: Vec3| {
        let d = q - p;
        let t = d.dot(v).atan2(d.dot(u));
        if t < 0.0 { t + std::f64::consts::TAU } else { t }
    };
    let bin = |t: f64| ((t / std::f64::consts::TAU * BINS as f64) as usize).min(BINS - 1);
    let (mut lo, mut hi, mut inward, mut points) = (f64::INFINITY, f64::NEG_INFINITY, 0.0, 0usize);
    let mut filled = [false; BINS];
    for (t, f) in mesh.triangles.iter().zip(&mesh.tri_face) {
        if !faces.contains(f) {
            continue;
        }
        let mut angles = [0.0f64; 3];
        for (k, a) in t.iter().zip(angles.iter_mut()) {
            let (Some(q), Some(n)) = (mesh.positions.get(*k as usize), mesh.normals.get(*k as usize)) else { return None };
            let along = (*q - p).dot(axis);
            lo = lo.min(along);
            hi = hi.max(along);
            inward += n.dot(*q - p - axis * along);
            points += 1;
            *a = angle(*q);
        }
        // Mark the bins the triangle spans, the short way round between its corners.
        let (amin, amax) = (angles.iter().copied().fold(f64::INFINITY, f64::min), angles.iter().copied().fold(0.0, f64::max));
        let (from, to) = if amax - amin > std::f64::consts::PI { (amax, amin + std::f64::consts::TAU) } else { (amin, amax) };
        let (b0, b1) = (bin(from), bin(to.rem_euclid(std::f64::consts::TAU)));
        let mut i = b0;
        for _ in 0..BINS {
            if let Some(x) = filled.get_mut(i) {
                *x = true;
            }
            if i == b1 {
                break;
            }
            i = (i + 1) % BINS;
        }
    }
    // Outward normals point toward the axis on a hole's wall.
    if points == 0 || inward >= 0.0 || !(hi > lo) {
        return None;
    }
    let covered = filled.iter().filter(|x| **x).count();
    let sweep = covered as f64 / BINS as f64 * std::f64::consts::TAU;
    // The middle of the longest empty run of bins (wrapping round).
    let opening = (covered < BINS).then(|| {
        let (mut best, mut best_start, mut run, mut run_start) = (0usize, 0usize, 0usize, 0usize);
        for k in 0..2 * BINS {
            if filled.get(k % BINS).copied().unwrap_or(true) {
                run = 0;
            } else {
                if run == 0 {
                    run_start = k;
                }
                run += 1;
                if run > best && run <= BINS {
                    best = run;
                    best_start = run_start;
                }
            }
        }
        let mid = (best_start as f64 + best as f64 / 2.0) / BINS as f64 * std::f64::consts::TAU;
        u * mid.cos() + v * mid.sin()
    });
    Some(HoleWall { axis_point: p, axis, radius: r, start: lo, end: hi, sweep, opening })
}
