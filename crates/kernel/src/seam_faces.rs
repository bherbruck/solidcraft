//! Faces that run along a seam edge twice: a whole turn of a cylinder or a cone bounded by one
//! wire, as STEP files write them (the wire goes round one end, along the seam, round the other
//! end and back along the seam). SolidCraft builds its own round faces in pieces with no such
//! seam, and a boolean that has to divide one of these faces can fail where the same bodies with
//! the face in two halves work: an intersection loop lying wholly inside the face is a hole in a
//! piece whose outer wire visits the seam's vertices twice, which the boolean refuses. So when the
//! first tries at the boolean of the bodies as they are fail, each such face is split in two along
//! a plane through its axis, a quarter turn either side of the seam (the solids are unchanged), and
//! the boolean is asked again; the result's history is composed back over the operands as they
//! were given.

use solidcraft_geom::{Plane, Vec3};

use crate::body::{Body, from_p3};
use crate::boolean_record as recorded;
use crate::history::History;
use crate::splitface::{SplitTool, split_any};
use crate::{BoolOp, FaceSurface, Result, face_surface};

/// Most faces split before the boolean.
const MAX_SPLITS: usize = 64;

thread_local! {
    /// Set while the boolean of the split bodies runs (no second round of splitting).
    static SPLITTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The first face of `body` whose wire runs along a seam edge twice, on a cylinder or a cone,
/// with the plane that splits it in two: through its axis, square to the seam.
fn next_split(body: &Body) -> Option<(usize, Plane)> {
    body.solid.face_iter().enumerate().find_map(|(i, face)| {
        let mut seen = std::collections::HashSet::new();
        let seam = face.absolute_boundaries().iter().flat_map(|w| w.edge_iter()).find(|e| !seen.insert(e.id()))?;
        let (origin, axis) = match face_surface(body, i)? {
            FaceSurface::Cylinder { origin, axis, .. } => (origin, axis),
            FaceSurface::Cone { apex, axis, .. } => (apex, axis),
            _ => return None,
        };
        let axis = axis.normalized()?;
        let p = from_p3(seam.front().point());
        let off = p - origin;
        let radial: Vec3 = (off - axis * off.dot(axis)).normalized()?;
        Some((i, Plane::from_normal(origin, radial)?))
    })
}

/// Whether either body has a face for [`boolean`] to split.
pub(crate) fn needed(a: &Body, b: &Body) -> bool {
    !a.is_mesh() && !b.is_mesh() && !SPLITTING.with(|c| c.get()) && (next_split(a).is_some() || next_split(b).is_some())
}

/// `body` with every face that runs along a seam edge twice split in two, and its history over
/// `body`. `None` when a split fails.
fn halved(body: &Body) -> Option<(Body, Option<History>)> {
    let (mut x, mut h) = (body.clone(), Some(History::identity(body)));
    for _ in 0..MAX_SPLITS {
        let Some((face, plane)) = next_split(&x) else { return Some((x, h)) };
        let (mut done, _) = split_any(&x, &[face], &SplitTool::Plane(plane)).ok()?;
        // The split is no authored boundary: a later feature may still merge what it separates.
        done.regularization_protected = x.regularization_protected;
        let through = recorded::through(&done, 0, h.as_ref());
        (x, h) = (done, through);
    }
    next_split(&x).is_none().then_some((x, h))
}

/// The boolean `op` of `a` and `b` with their faces that run along a seam edge twice split in
/// two, `run` doing the boolean of the split bodies. `None` when neither body has such a face, a
/// split fails, or the boolean of the split bodies gave no (checked) result: the caller then
/// keeps what it had.
pub(crate) fn boolean(a: &Body, b: &Body, op: BoolOp, run: &dyn Fn(&Body, &Body, BoolOp) -> Result<Option<Body>>) -> Option<Result<Option<Body>>> {
    if !needed(a, b) {
        return None;
    }
    let ((x, hx), (y, hy)) = (halved(a)?, halved(b)?);
    SPLITTING.with(|c| c.set(true));
    let result = crate::guard("boolean of faces split off their seams", || run(&x, &y, op));
    SPLITTING.with(|c| c.set(false));
    let Ok(result) = result else { return None };
    // Held to what a boolean can do, as a retry's result is (see `boolean_guard`).
    crate::boolean_guard::check_retry(a, b, op, result.as_ref()).ok()?;
    let counts = crate::record::counts(&[a, b]);
    Some(Ok(result.map(|body| {
        // Each step's history over the operands `[a, b]`.
        let history = recorded::of_pair(&body, &recorded::placed(hx.clone(), 0, 2), &recorded::placed(hy.clone(), 1, 2));
        body.with_checked_history(history, &counts)
    })))
}

#[cfg(test)]
#[path = "seam_faces_tests.rs"]
mod tests;
