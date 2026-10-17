//! Boundary patches: one face through the boundary curves of a sketch, each side meeting the
//! surface it was taken from at a requested continuity (G0 up to G3).
//!
//! Two boundaries are the two sides of a [`crate::bridge`]: the patch is the Hermite surface that
//! leaves each boundary along the direction its frames state and repeats as much of that support
//! surface — its tangent plane, its curvature, the change of that curvature — as the side's order
//! asks for, with `crown` shaping the middle without touching a boundary condition.
//!
//! A patch through *three or four* boundaries is refused here rather than built approximately. At
//! a corner two sides prescribe the same entries of the control net from two different surfaces,
//! and a tensor-product patch can hold both only where those two prescriptions already agree (one
//! wall carrying every side, say). Where they do not — the general case — the requested order would
//! silently drop near the corner, or the boundary curve would visibly leave its stations, and a
//! Gregory-type corner (a rational blend of the sides' own data) is needed to hold them instead
//! (issue #763). A loop of one sketch's curves is the case where they agree: it lies in the
//! sketch's plane, and its flat Coons patch ([`crate::boundary_network`]) holds every order
//! exactly; the network there refuses G1/G2 on two sides that meet at a corner for the same
//! reason. Everything this module refuses is an error, never a quietly worse surface.

use solidcraft_geom::Vec3;

use crate::body::Body;
use crate::bridge::{Continuity, Frame, bridge};
use crate::{KernelError, Result};

/// The most stations a boundary of a patch may carry.
const MAX_SAMPLES: usize = 512;

/// One boundary of a patch: the stations along it, and how much of the surface it was taken from
/// the patch repeats there.
#[derive(Clone, Debug)]
pub struct Boundary {
    /// The stations along the boundary, in the order they run: the end of one boundary meets the
    /// start of the next, and the last meets the first.
    pub frames: Vec<Frame>,
    /// How much of the neighbouring surface the patch repeats at this boundary.
    pub order: Continuity,
}

/// A two-sided patch: one face that meets boundary `i` at the continuity its `order` asks for (G0 up
/// to G3), with `samples` stations every boundary must carry and `crown` shaping the middle of the
/// patch without touching any boundary condition (0 the plain blend, up to 3 fuller, down to -0.9
/// flatter). Three or four boundaries are refused: see the module docs.
pub fn boundary_patch(boundaries: &[Boundary], crown: f64, samples: usize) -> Result<Body> {
    check_input(boundaries, crown, samples)?;
    if boundaries.len() > 2 {
        return Err(KernelError::Invalid(format!(
            "not supported yet: a patch through {} boundaries — a three- or four-sided patch needs a Gregory-type corner (issue #763); \
             give two boundaries, which is the bridge this builds exactly",
            boundaries.len()
        )));
    }
    let (Some(a), Some(b)) = (boundaries.first(), boundaries.get(1)) else {
        return Err(KernelError::Invalid("a boundary patch joins 2 boundaries: one of them is missing".into()));
    };
    bridge(&a.frames, &b.frames, [a.order, b.order], [crown, crown])
}

/// Everything a patch refuses, before any geometry is built.
fn check_input(boundaries: &[Boundary], crown: f64, samples: usize) -> Result<()> {
    let n = boundaries.len();
    if !(2..=4).contains(&n) {
        return Err(KernelError::Invalid(format!("a boundary patch joins 2 to 4 boundaries, {n} given")));
    }
    if !(2..=MAX_SAMPLES).contains(&samples) {
        return Err(KernelError::Invalid(format!("a boundary patch needs 2 to {MAX_SAMPLES} stations per boundary, {samples} given")));
    }
    if !crown.is_finite() || !(-0.9..=3.0).contains(&crown) {
        return Err(KernelError::Invalid("a boundary patch's crown must be between -0.9 and 3".into()));
    }
    for (i, b) in boundaries.iter().enumerate() {
        if b.frames.len() != samples {
            return Err(KernelError::Invalid(format!(
                "boundary {i} carries {} stations, not the {samples} every boundary of a patch needs",
                b.frames.len()
            )));
        }
        for f in &b.frames {
            if ![f.p, f.leave, f.bend, f.bend_change].into_iter().all(Vec3::is_finite) {
                return Err(KernelError::Invalid(format!("boundary {i} has a station that is not finite")));
            }
            if f.leave.normalized().is_none() {
                return Err(KernelError::Invalid(format!("boundary {i} has a station with no leave direction")));
            }
        }
        if boundary_length(&b.frames) <= 1e-9 {
            return Err(KernelError::Invalid(format!("boundary {i} has no length: a patch needs a curve, not a single point")));
        }
    }
    // Two boundaries are the two sides of a bridge, which do not meet; three or four would run round
    // a loop, so each one would have to end where the next one begins.
    if n >= 3 {
        let mut scale: f64 = 1.0;
        for b in boundaries {
            scale = scale.max(boundary_length(&b.frames));
        }
        let tol = scale * 1e-4 + 1e-9;
        for i in 0..n {
            let (Some(a), Some(b)) = (boundaries.get(i), boundaries.get((i + 1) % n)) else { continue };
            let (Some(end), Some(start)) = (a.frames.last(), b.frames.first()) else { continue };
            let gap = end.p.dist(start.p);
            if gap > tol {
                return Err(KernelError::Invalid(format!(
                    "boundary {i} ends and boundary {} starts {gap:.6} mm apart: the boundaries of a loop must run on into one another",
                    (i + 1) % n
                )));
            }
        }
    }
    Ok(())
}

/// The length of a run of stations.
fn boundary_length(frames: &[Frame]) -> f64 {
    let mut length: f64 = 0.0;
    for w in frames.windows(2) {
        if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
            length += a.p.dist(b.p);
        }
    }
    length
}

#[cfg(test)]
#[path = "boundary_tests.rs"]
mod tests;
