//! Operations on bodies: booleans and rigid transforms.

use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;
use truck_modeling as mt;

use std::collections::HashMap;

use crate::body::{Body, Solid, from_p3};
use crate::boolean_diagnostics::{BooleanFallback, BooleanStrategy, capture_boolean_diagnostics, merge, record};
use crate::boolean_record::{self as recorded, Traced};
use crate::{KernelError, Result, guard, record};

#[cfg(test)]
#[path = "bore_union_tests.rs"]
mod bore_union_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoolOp {
    Union,
    Cut,
    Intersect,
}

/// Whether a union can sew two matching planar faces without intersecting them.
/// Callers can retain the exact touching tool instead of extending it into material.
pub fn can_join_touching_faces(a: &Body, b: &Body) -> bool {
    a.solid.boundaries().len() == 1 && b.solid.boundaries().len() == 1 && crate::coplanar::glue(a, b).is_some_and(|r| r.is_ok())
}

/// Shifts applied to both operands on retries. The boolean classifies some faces by casting a
/// ray whose direction is derived from point coordinates; moving both solids (and the result
/// back) leaves the geometry unchanged but changes those rays.
const JITTER: [[f64; 3]; 4] = [[0.0, 0.0, 0.0], [0.1373, 0.2719, 0.3911], [-0.5127, 0.0833, 0.2291], [0.6911, -0.3337, -0.6127]];

fn volume(b: &Body) -> f64 {
    // Measured once per (solid, tolerance) and shared by clones, so an operand a later
    // boolean takes again is not meshed twice for the same number.
    b.volume_at(b.size() * 2e-3).unwrap_or(f64::NAN)
}

/// Halton low-discrepancy value (base `b`) for index `i`.
fn halton(mut i: usize, b: usize) -> f64 {
    let (mut f, mut r) = (1.0, 0.0);
    while i > 0 {
        f /= b as f64;
        r += f * (i % b) as f64;
        i /= b;
    }
    r
}

/// Fraction of sample points where `result` disagrees with `op` applied to `a` and `b`
/// (membership by ray parity). Samples cover the overlap box densely and the union box lightly.
fn mismatch(a: &solidcraft_geom::Mesh, b: &solidcraft_geom::Mesh, result: &solidcraft_geom::Mesh, op: BoolOp) -> Option<f64> {
    if a.triangles.len() + b.triangles.len() + result.triangles.len() > 60_000 {
        return None;
    }
    let (ba, bb) = (a.bounds(), b.bounds());
    let lo = ba.min.max(bb.min);
    let hi = ba.max.min(bb.max);
    let all = ba.union(&bb);
    let mut boxes = vec![(all, 300usize)];
    // A touching seam can overlap by round-off only. Samples there sit on the
    // operands' boundaries, where ray parity cannot classify membership reliably.
    let gap = all.diagonal().max(1.0) * 1e-9;
    if hi.x - lo.x > gap && hi.y - lo.y > gap && hi.z - lo.z > gap {
        boxes.push((solidcraft_geom::Aabb3 { min: lo, max: hi }, 900));
    }
    let (ia_idx, ib_idx, ir_idx) = (a.inside_index(), b.inside_index(), result.inside_index());
    let (mut n, mut bad) = (0usize, 0usize);
    for (bx, count) in boxes {
        let s = bx.size();
        for i in 1..=count {
            let p = bx.min + Vec3::new(s.x * halton(i, 2), s.y * halton(i, 3), s.z * halton(i, 5));
            let (ia, ib) = (ia_idx.contains(p), ib_idx.contains(p));
            let want = match op {
                BoolOp::Union => ia || ib,
                BoolOp::Cut => ia && !ib,
                BoolOp::Intersect => ia && ib,
            };
            n += 1;
            if ir_idx.contains(p) != want {
                bad += 1;
            }
        }
    }
    Some(bad as f64 / n.max(1) as f64)
}

/// How one body sits relative to another, from sample points: (fraction of `b`'s samples
/// inside `a`, fraction of `a`'s samples inside `b`).
fn overlap(
    a: &solidcraft_geom::Mesh,
    b: &solidcraft_geom::Mesh,
    ia: &solidcraft_geom::InsideIndex,
    ib: &solidcraft_geom::InsideIndex,
) -> Option<(f64, f64)> {
    let frac = |inner: &solidcraft_geom::Mesh, inner_i: &solidcraft_geom::InsideIndex, outer_i: &solidcraft_geom::InsideIndex| -> Option<f64> {
        let bx = inner.bounds();
        let s = bx.size();
        let (mut n, mut hit) = (0usize, 0usize);
        // A slender body lying across its box (a long hole tool drilled at an angle) fills
        // little of it: the samples go on past the first 2000 until enough land inside.
        for i in 1..=64_000 {
            if i > 2000 && n >= 50 {
                break;
            }
            let p = bx.min + Vec3::new(s.x * halton(i, 2), s.y * halton(i, 3), s.z * halton(i, 5));
            if !inner_i.contains(p) {
                continue;
            }
            n += 1;
            if outer_i.contains(p) {
                hit += 1;
            }
        }
        (n >= 50).then(|| hit as f64 / n as f64)
    };
    Some((frac(b, ib, ia)?, frac(a, ia, ib)?))
}

/// Whether some facet of `m` has material just under it (`depth` in from its middle) that is
/// inside the other body (`inside`) or outside it. With `within`, only facets in that box.
fn under_surface(
    m: &solidcraft_geom::Mesh,
    own: &solidcraft_geom::InsideIndex,
    other: &solidcraft_geom::InsideIndex,
    depth: f64,
    inside: bool,
    within: Option<&solidcraft_geom::Aabb3>,
) -> bool {
    m.triangles.iter().filter_map(|t| m.tri(t)).any(|[p, q, r]| {
        let Some(n) = (q - p).cross(r - p).normalized() else { return false };
        let c = (p + q + r) * (1.0 / 3.0) - n * depth;
        let near = within.is_none_or(|b| c.x >= b.min.x && c.x <= b.max.x && c.y >= b.min.y && c.y <= b.max.y && c.z >= b.min.z && c.z <= b.max.z);
        // A wall thinner than `depth` puts the point outside its own body: no evidence.
        near && other.contains(c) == inside && own.contains(c)
    })
}

/// Whether the bodies' facetings show that the samples' answer (see [`trivial`]) is wrong:
/// their surfaces pass through each other, or a facet of one has material under it on the
/// wrong side of the other. Overlaps and gaps shallower than `tol` are not seen.
fn contradicted(
    (ma, ia): (&solidcraft_geom::Mesh, &solidcraft_geom::InsideIndex),
    (mb, ib): (&solidcraft_geom::Mesh, &solidcraft_geom::InsideIndex),
    (disjoint, b_in_a, a_in_b): (bool, bool, bool),
    tol: f64,
) -> bool {
    if !(disjoint || b_in_a || a_in_b) {
        return false;
    }
    if ma.crosses(mb, tol) {
        return true;
    }
    if disjoint {
        let (ba, bb) = (ma.bounds(), mb.bounds());
        let shared = solidcraft_geom::Aabb3 { min: ba.min.max(bb.min), max: ba.max.min(bb.max) };
        return !shared.is_empty() && (under_surface(ma, ia, ib, tol, true, Some(&shared)) || under_surface(mb, ib, ia, tol, true, Some(&shared)));
    }
    (a_in_b && under_surface(ma, ia, ib, tol, false, None)) || (b_in_a && !a_in_b && under_surface(mb, ib, ia, tol, false, None))
}

/// Sampled overlap fraction below which two bodies share no material.
const NONE: f64 = 0.003;

/// Booleans that need no intersection: one body clear of the other, or inside it. These are
/// common with coincident curved faces (a hole drilled where one already is, a body joined
/// with itself), which the intersection can't handle. `counts`: the operands' faces and edges.
/// `tol`: how far the meshes' surfaces may pass through each other and still count as touching
/// (they are facetings of the bodies, so a few times their tolerance).
///
/// The second value is false when the facetings contradict the samples ([`contradicted`]):
/// the answer is then only what to fall back on if the bodies can't be intersected.
fn trivial(
    a: &Body,
    b: &Body,
    op: BoolOp,
    ma: &solidcraft_geom::Mesh,
    mb: &solidcraft_geom::Mesh,
    counts: &[(usize, usize)],
    tol: f64,
) -> Option<(Option<Body>, bool)> {
    if ma.triangles.len() + mb.triangles.len() > 80_000 {
        return None;
    }
    let (ia, ib) = (ma.inside_index(), mb.inside_index());
    let (b_in_a, a_in_b) = overlap(ma, mb, &ia, &ib)?;
    const ALL: f64 = 0.997;
    let disjoint = b_in_a < NONE && a_in_b < NONE;
    // The samples only say that nearly all of one body is clear of, or inside, the other. A
    // small part can still be on the wrong side (a peg overlapping a rib, a chamfer taken off
    // one edge), and then the bodies have to be intersected.
    let sure = !contradicted((ma, &ia), (mb, &ib), (disjoint, b_in_a > ALL, a_in_b > ALL), tol);
    let answer = match op {
        // Sampling only selects this candidate. Validate every shell boundary and the
        // complete material/void nesting before preserving a disconnected union.
        BoolOp::Union if disjoint => {
            let pieces = vec![(a.clone(), recorded::kept(a, 0, 2)), (b.clone(), recorded::kept(b, 1, 2))];
            let joined = recorded::join(pieces, counts).ok().flatten()?;
            joined.separated_lumps().ok()?;
            Some(Some(joined))
        }
        BoolOp::Cut if disjoint => Some(Some(recorded::operand(a, 0, counts))),
        BoolOp::Cut if a_in_b > ALL => Some(None),
        BoolOp::Intersect if disjoint => Some(None),
        BoolOp::Intersect if b_in_a > ALL => Some(Some(recorded::operand(b, 1, counts))),
        BoolOp::Intersect if a_in_b > ALL => Some(Some(recorded::operand(a, 0, counts))),
        BoolOp::Union if b_in_a > ALL => Some(Some(recorded::operand(a, 0, counts))),
        BoolOp::Union if a_in_b > ALL => Some(Some(recorded::operand(b, 1, counts))),
        // The tool wholly inside, clear of the outside: a void.
        BoolOp::Cut if b_in_a > ALL => {
            let candidate = void(a, b, counts).ok().flatten()?;
            candidate.separated_lumps().ok()?;
            Some(Some(candidate))
        }
        _ => None,
    };
    answer.map(|r| (r, sure))
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn seam_roundoff_is_not_a_thin_meeting_but_real_thin_solids_are() {
        use solidcraft_geom::{Loop2, Plane, Region2, Vec2};

        // This circle's seams lie on the X axis: its cylinder's vertex box is flat in Y.
        let region = Region2 { outer: Loop2::circle(Vec2::ZERO, 8.0), holes: vec![] };
        let periodic = crate::extrude(&Plane::XY, &[region], 0.0, 20.0).unwrap().pop().unwrap();
        let block = crate::box_solid(Vec3::new(-40.0, -40.0, -10.0), Vec3::new(40.0, 40.0, 10.0)).unwrap();
        assert!(meeting_size(&periodic, &block).is_infinite());
        let thin = crate::box_solid(Vec3::ZERO, Vec3::new(80.0, 80.0, 1e-5)).unwrap();
        assert!((meeting_size(&thin, &thin) - 1e-5).abs() < 1e-12);
    }

    #[test]
    fn curved_near_contact_never_uses_a_shell_only_boolean_candidate() {
        let direction = Vec3::new(1.0, 2.0, 3.0).normalized().unwrap();
        let outer = crate::sphere(Vec3::ZERO, 10.0).unwrap();
        let mesh = outer.tessellate(0.03).unwrap();
        for distance in [19.999, 20.001] {
            let other = crate::sphere(direction * distance, 10.0).unwrap();
            let other_mesh = other.tessellate(0.03).unwrap();
            let counts = record::counts(&[&outer, &other]);
            assert!(trivial(&outer, &other, BoolOp::Union, &mesh, &other_mesh, &counts, 0.09).is_none());
            assert!(trivial(&other, &outer, BoolOp::Union, &other_mesh, &mesh, &counts, 0.09).is_none());
        }
        let protruding = crate::sphere(direction * 9.001, 1.0).unwrap();
        let protruding_mesh = protruding.tessellate(0.03).unwrap();
        let counts = record::counts(&[&outer, &protruding]);
        assert!(trivial(&outer, &protruding, BoolOp::Cut, &mesh, &protruding_mesh, &counts, 0.09).is_none());
    }

    /// Bodies sharing less than the samples can see still meet: no shortcut, and the boolean
    /// gives the shared part.
    #[test]
    fn a_small_overlap_is_not_taken_for_apart_or_inside() {
        let cube = |x: f64, y: f64| crate::box_solid(Vec3::new(x, y, 0.0), Vec3::new(x + 10.0, y + 10.0, 10.0)).unwrap();
        let a = cube(0.0, 0.0);
        // A 0.01 slab between parallel faces (0.1 % of each cube), and a 0.02 x 0.02 corner.
        for (b, want) in [(cube(9.99, 0.0), 1.0), (cube(9.98, 9.98), 0.004)] {
            let (ma, mb) = (a.tessellate(0.001).unwrap(), b.tessellate(0.001).unwrap());
            let counts = record::counts(&[&a, &b]);
            for op in [BoolOp::Intersect, BoolOp::Cut, BoolOp::Union] {
                assert!(trivial(&a, &b, op, &ma, &mb, &counts, 0.003).is_none_or(|(_, sure)| !sure), "{op:?} {want}");
            }
            let shared = boolean(&a, &b, BoolOp::Intersect).unwrap().unwrap();
            assert!((crate::measure(&shared).unwrap().volume - want).abs() < 1e-9, "{want}");
            let cut = boolean(&a, &b, BoolOp::Cut).unwrap().unwrap();
            assert!((crate::measure(&cut).unwrap().volume - (1000.0 - want)).abs() < 1e-9, "{want}");
        }
        // Touching on a face, and one cube well inside the other, still take the shortcut.
        let (touching, inner) = (cube(10.0, 0.0), crate::box_solid(Vec3::new(2.0, 2.0, 2.0), Vec3::new(8.0, 8.0, 8.0)).unwrap());
        let ma = a.tessellate(0.001).unwrap();
        for (b, op) in [(&touching, BoolOp::Intersect), (&inner, BoolOp::Intersect), (&inner, BoolOp::Union)] {
            let counts = record::counts(&[&a, b]);
            assert!(trivial(&a, b, op, &ma, &b.tessellate(0.001).unwrap(), &counts, 0.003).is_some_and(|(_, sure)| sure), "{op:?}");
        }
    }
}

/// `a` with the inside of `b` (wholly within it) taken out: a second, inward-facing shell.
fn void(a: &Body, b: &Body, counts: &[(usize, usize)]) -> Result<Option<Body>> {
    guard("void", || {
        let mut inner = b.deep_copy();
        let inner_edges = record::edge_origins(&inner, 1);
        inner.not();
        let mut shells = a.solid.boundaries().clone();
        shells.extend(inner.boundaries().iter().cloned());
        let s = crate::body::Solid::try_new(shells).map_err(|e| KernelError::Failed(format!("void: {e}")))?;
        let (faces, edges) = recorded::void(a, inner_edges, b.face_count());
        Ok(Some(record::build(s, counts, Some(faces), Some(&edges))?))
    })
}

thread_local! {
    /// Set while a boolean runs on bodies whose coincident faces were pushed apart (no
    /// second round of pushing).
    static APART: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set while a union runs with a bore narrowed off a common cylinder (no second round).
    static BORES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Boolean of two bodies. The result may be empty (`Ok(None)`) for a cut that removes
/// everything or an intersection of disjoint bodies. Results are checked against volume bounds
/// and retried with shifted copies and other tolerances when they fail or look wrong.
///
/// The result carries its exact history ([`Body::history`]) over the operands `[a, b]`,
/// whichever way it was made, or none where a step it took does not report one.
pub fn boolean(a: &Body, b: &Body, op: BoolOp) -> Result<Option<Body>> {
    crate::profile::timed(crate::profile::Kind::Boolean, || boolean_untimed(a, b, op))
}

fn boolean_untimed(a: &Body, b: &Body, op: BoolOp) -> Result<Option<Body>> {
    // Earlier coarse bores can leave a junction a later pattern copy cannot intersect.
    // Resolve conical-cut junctions from the first copy, while the operands are unchanged.
    if op == BoolOp::Cut && !APART.with(|c| c.get()) && conical_cut_junction(a, b) {
        let refined = |a: &Body, b: &Body, op| boolean_whole_with_refinement(a, b, op, true);
        if let Ok(result) = boolean_via(a, b, op, &refined) {
            return Ok(result);
        }
    }
    let r = boolean_via(a, b, op, &boolean_whole);
    if r.is_ok() || !crate::tangent_cylinders::needed(a, b) {
        return r;
    }
    // Equal cylinders crossing each other touch at two points the boolean cannot take in the
    // middle of a face (#788): once more with the faces split along them.
    let split = |a: &Body, b: &Body, op| {
        crate::tangent_cylinders::boolean(a, b, op, &boolean_whole).unwrap_or_else(|| Err(KernelError::Failed("no split of faces helps".into())))
    };
    boolean_via(a, b, op, &split).or(r)
}

/// A conical cutter meeting a body with an earlier conical cut needs finer tracing at
/// their junctions. Guard the analytic recognition as well as the Boolean itself.
fn conical_cut_junction(a: &Body, b: &Body) -> bool {
    guard("conical cut junction", || {
        let conical = |body: &Body| {
            let tol = (body.size() * 1e-5).max(1e-7);
            body.solid.face_iter().any(|face| matches!(crate::offset::surf_of(face, tol), Some(crate::offset::Surf::Cone { .. })))
        };
        Ok(conical(b) && conical(a))
    })
    .unwrap_or(false)
}

/// [`boolean`], the boolean of the whole bodies taken by `whole`.
fn boolean_via(a: &Body, b: &Body, op: BoolOp, whole: &dyn Fn(&Body, &Body, BoolOp) -> Result<Option<Body>>) -> Result<Option<Body>> {
    let (mut r, mut diagnostics) = capture_boolean_diagnostics(|| whole(a, b, op));
    // Bodies in several pieces: when the whole failed (or came out empty), a piece at a time.
    let retry = match &r {
        Err(_) => true,
        Ok(None) => op != BoolOp::Intersect,
        Ok(Some(_)) => false,
    };
    if retry && (a.solid.boundaries().len() > 1 || b.solid.boundaries().len() > 1) {
        let (la, lb) = (a.lumps()?, b.lumps()?);
        if la.len() > 1 || lb.len() > 1 {
            let (pieces, report) = capture_boolean_diagnostics(|| by_lumps(a, b, la, lb, op));
            if let Ok(Some(x)) = pieces {
                r = Ok(Some(x));
                diagnostics = report;
                diagnostics.push(BooleanFallback::new(BooleanStrategy::Piecewise));
            }
        }
    }
    // A piece with sealed voids, cut clear of them: the outer shell alone, the voids put back.
    if op == BoolOp::Cut && r.is_err() {
        let (cut, report) = capture_boolean_diagnostics(|| cut_around_voids(a, b, whole));
        if let Ok(Some(x)) = cut {
            r = Ok(Some(x));
            diagnostics = report;
            diagnostics.push(BooleanFallback::new(BooleanStrategy::Piecewise));
        }
    }
    if op == BoolOp::Union
        && let Ok(Some(body)) = &r
        && !preserves_planar_surfaces(a, b, body)
    {
        // Numerical retries must not quietly alter a planar design's dimensions.
        // Prefer the exact polygon result; reject an edge-only pinch if it cannot
        // form a closed solid without displacing either operand.
        r = match crate::polybool::planar_boolean(a, b, op) {
            Ok(Some(exact)) if preserves_planar_surfaces(a, b, &exact) => {
                diagnostics = Default::default();
                diagnostics.events.push(BooleanFallback::new(BooleanStrategy::PlanarFallback));
                Ok(Some(exact))
            }
            _ => Err(KernelError::Invalid(
                "join cannot preserve the original faces; check for zero-thickness edge contact or add intentional overlap".into(),
            )),
        };
    }
    r = r.map(|result| result.map(|body| body.inherit_regularization_protection(a).inherit_regularization_protection(b)));
    match op {
        BoolOp::Cut => r = r.and_then(|result| result.map(|body| body.with_preserved_boolean_planar_sources(a, b, true)).transpose()),
        BoolOp::Union => r = r.and_then(|result| result.map(|body| body.with_preserved_union_planar_sources(a, b)).transpose()),
        // Every face of an intersection lies on one operand's face, as a union's does: the same
        // roles carry over (#565).
        BoolOp::Intersect => r = r.and_then(|result| result.map(|body| body.with_preserved_union_planar_sources(a, b)).transpose()),
    }
    // A retry's result is held to what the operation can do at all; one that fails is refused
    // rather than returned wrong (issue #616).
    if (!diagnostics.events.is_empty() || diagnostics.truncated)
        && let Ok(result) = &r
        && let Err(e) = crate::boolean_guard::check_retry(a, b, op, result.as_ref())
    {
        let tried = diagnostics.summary().unwrap_or_default();
        r = Err(match e {
            KernelError::Failed(m) => KernelError::Failed(format!("{m} ({})", tried.trim_start_matches("Boolean retry: "))),
            other => other,
        });
    }
    if r.is_ok() {
        merge(diagnostics);
    }
    r
}

/// The most shells [`cut_around_voids`] takes apart.
const MAX_VOID_SHELLS: usize = 64;

/// `a` cut by `b` around `a`'s voids: when `a` is one piece with sealed voids (inner shells) and
/// `b` stays clear of every void, the outer shell alone is cut and the voids are put back. The cut
/// of the whole failed on such bodies where the same cut of the outer shell works: a sphere cut
/// far from a void (robustness seed 15). `None` when `a` has no voids, `b` comes near one, or
/// the cut of the outer shell fails too. The result carries no history.
fn cut_around_voids(a: &Body, b: &Body, whole: &dyn Fn(&Body, &Body, BoolOp) -> Result<Option<Body>>) -> Result<Option<Body>> {
    let shells = a.solid.boundaries().clone();
    if shells.len() < 2 || shells.len() > MAX_VOID_SHELLS || b.solid.boundaries().len() != 1 || a.lumps()?.len() != 1 {
        return Ok(None);
    }
    let tol = (a.size() * 1e-3).max(1e-3);
    let mut boxes = Vec::with_capacity(shells.len());
    for sh in &shells {
        let piece =
            guard("void shell", || Body::new(Solid::try_new(vec![sh.clone()]).map_err(|e| KernelError::Failed(format!("void shell: {e}")))?))?;
        let m = piece.tessellate(tol)?;
        if m.positions.is_empty() {
            return Ok(None);
        }
        boxes.push(m.bounds());
    }
    let extent = |i: usize| {
        boxes.get(i).map_or(0.0, |x| {
            let d = x.max - x.min;
            d.x * d.y * d.z
        })
    };
    let Some(outer) = (0..shells.len()).max_by(|i, j| extent(*i).total_cmp(&extent(*j))) else { return Ok(None) };
    let Some(ob) = boxes.get(outer) else { return Ok(None) };
    let tm = b.tessellate((b.size() * 1e-3).max(1e-3))?;
    let tb = tm.bounds();
    // Clear of the tool by more than both meshes' deflection.
    let gap = 4.0 * tol + (b.size() * 1e-3).max(1e-3);
    let inside = |x: &solidcraft_geom::Aabb3| {
        x.min.x >= ob.min.x && x.min.y >= ob.min.y && x.min.z >= ob.min.z && x.max.x <= ob.max.x && x.max.y <= ob.max.y && x.max.z <= ob.max.z
    };
    let clear = |x: &solidcraft_geom::Aabb3| {
        x.max.x + gap < tb.min.x
            || tb.max.x + gap < x.min.x
            || x.max.y + gap < tb.min.y
            || tb.max.y + gap < x.min.y
            || x.max.z + gap < tb.min.z
            || tb.max.z + gap < x.min.z
    };
    let mut voids = Vec::new();
    for (i, (sh, x)) in shells.iter().zip(&boxes).enumerate() {
        if i == outer {
            continue;
        }
        if !inside(x) || !clear(x) {
            return Ok(None);
        }
        voids.push(sh.clone());
    }
    let Some(outer_shell) = shells.get(outer) else { return Ok(None) };
    let alone =
        guard("outer shell", || Body::new(Solid::try_new(vec![outer_shell.clone()]).map_err(|e| KernelError::Failed(format!("outer shell: {e}")))?))?;
    let Some(cut) = boolean_via(&alone, b, BoolOp::Cut, whole)? else { return Ok(None) };
    let mut all = cut.solid.boundaries().clone();
    all.extend(voids);
    Ok(Some(guard("voids back", || Body::new(Solid::try_new(all).map_err(|e| KernelError::Failed(format!("voids back: {e}")))?))?))
}

/// Every face of an all-planar union must remain on an operand's original plane.
/// Curved operands keep their existing surface-specific boolean handling.
fn preserves_planar_surfaces(a: &Body, b: &Body, result: &Body) -> bool {
    guard("validate planar union", || Ok(planar_surfaces_match(a, b, result))).unwrap_or(false)
}

fn planar_surfaces_match(a: &Body, b: &Body, result: &Body) -> bool {
    let planes: Option<Vec<(Vec3, f64)>> = a
        .solid
        .face_iter()
        .chain(b.solid.face_iter())
        .map(|f| {
            let mt::Surface::Plane(p) = f.oriented_surface() else { return None };
            let n = p.normal();
            let n = Vec3::new(n.x, n.y, n.z).normalized()?;
            Some((n, n.dot(from_p3(p.origin()))))
        })
        .collect();
    let Some(planes) = planes else { return true };
    let tol = (a.size().max(b.size()) * 1e-9).max(1e-9);
    result.solid.face_iter().all(|f| {
        matches!(f.oriented_surface(), mt::Surface::Plane(_))
            && planes.iter().any(|(n, d)| f.vertex_iter().all(|v| (n.dot(from_p3(v.point())) - d).abs() <= tol))
    })
}

/// Smallest side of the box where the bodies' vertex boxes overlap (infinite when they don't).
fn meeting_size(a: &Body, b: &Body) -> f64 {
    let bbox = |x: &Body| {
        let mut bb = solidcraft_geom::Aabb3::EMPTY;
        for v in x.solid.vertex_iter() {
            bb.add(from_p3(v.point()));
        }
        bb
    };
    let (ba, bb) = (bbox(a), bbox(b));
    let s = solidcraft_geom::Aabb3 { min: ba.min.max(bb.min), max: ba.max.min(bb.max) }.size();
    // A closed sweep's section seams need not span its geometry. Roundoff in a flat
    // vertex box is not a thin meeting region: refining for it
    // makes millions of triangles and skips the bounded containment shortcut (#2005).
    let roundoff = a.size().max(b.size()) * 1e-12;
    if s.x > roundoff && s.y > roundoff && s.z > roundoff { s.x.min(s.y).min(s.z) } else { f64::INFINITY }
}

/// Smallest side of the box where the bounds of two meshes overlap (infinite when they don't).
/// Unlike [`meeting_size`], this follows curved faces: a cylinder's vertices are only the points
/// on its seam, so its vertex box says nothing about where its side reaches.
fn mesh_meeting_size(a: &solidcraft_geom::Mesh, b: &solidcraft_geom::Mesh) -> f64 {
    let (ba, bb) = (a.bounds(), b.bounds());
    let s = solidcraft_geom::Aabb3 { min: ba.min.max(bb.min), max: ba.max.min(bb.max) }.size();
    if s.x > 0.0 && s.y > 0.0 && s.z > 0.0 { s.x.min(s.y).min(s.z) } else { f64::INFINITY }
}

/// Whether a result volume is plausible for `op`: within the bounds the operands' volumes
/// leave, plus the slack the meshed volumes are measured to.
fn plausible(v: f64, op: BoolOp, va: f64, vb: f64, slack: f64) -> bool {
    v > 0.0
        && match op {
            BoolOp::Union => v >= va.max(vb) - slack && v <= va + vb + slack,
            BoolOp::Cut => v >= va - vb - slack && v <= va + slack,
            BoolOp::Intersect => v <= va.min(vb) + slack,
        }
}

/// A boolean in 2D of prisms along one direction (see [`prism_result`]).
type Route = fn(&Body, &Body, BoolOp) -> Option<Result<Option<Body>>>;

/// A prism route's result (`route`: [`crate::prism::prism_boolean`] or
/// [`crate::prism::prism_union_between_caps`]): `Some` when both operands are prisms along one
/// direction, with a union's slab seams healed as the general B-rep boolean does.
fn prism_result(a: &Body, b: &Body, op: BoolOp, size: f64, counts: &[(usize, usize)], route: Route) -> Option<Result<Option<Body>>> {
    // Slab-by-slab prism booleans sew a closed solid, but leave cap and wall pieces along slab
    // boundaries: a cut along Z through a box whose side wall already has a port in it splits
    // that wall into strips at the port's top and bottom, though the cut never touches it. Merge
    // those coplanar faces for every operation, just as the general B-rep boolean does, retaining
    // intentional Split Face lines. Authored boundaries (painted faces, regularization
    // protection) keep their pieces.
    let keep: Vec<Vec<Vec3>> = a.split_keep().iter().chain(b.split_keep()).cloned().collect();
    let authored = |x: &Body| x.regularization_protected || x.paint().is_some_and(|p| !p.faces.is_empty());
    route(a, b, op).map(|result| {
        result.map(|body| {
            body.map(|body| {
                if op == BoolOp::Union {
                    guard("heal prism boolean", || recorded::heal_body(&body, (*body.solid).clone(), size, &keep, counts)).unwrap_or(body)
                } else if authored(a) || authored(b) {
                    body
                } else {
                    guard("heal prism boolean", || recorded::heal_prism_result(&body, size, &keep, counts)).unwrap_or(body)
                }
            })
        })
    })
}
/// Walls swept along splines have flat B-spline tops and bottoms. With those made planes the
/// operands are prisms, and two of them (same height, one direction) are decided in 2D: fast, and
/// with no intersection of spline surfaces, which is what fails on them. The outlines are fitted
/// by arcs within a micron (`prism_boolean_free_form`). `None` when the operands have no such
/// face, are not such prisms, or the answer is implausible: the general boolean then takes over.
fn swept_prism(a: &Body, b: &Body, op: BoolOp) -> Option<Body> {
    let (pa, pb) = (crate::planar_faces::planarized(a), crate::planar_faces::planarized(b));
    if pa.is_none() && pb.is_none() {
        return None;
    }
    let (ra, rb) = (pa.as_ref().unwrap_or(a), pb.as_ref().unwrap_or(b));
    let Some(Ok(Some(body))) = crate::prism::prism_boolean_free_form(ra, rb, op) else { return None };
    let (va, vb) = (volume(a), volume(b));
    plausible(volume(&body), op, va, vb, 2e-3 * (va + vb) + 1e-9).then_some(body)
}

fn boolean_whole(a: &Body, b: &Body, op: BoolOp) -> Result<Option<Body>> {
    boolean_whole_with_refinement(a, b, op, false)
}

fn boolean_whole_with_refinement(a: &Body, b: &Body, op: BoolOp, refine: bool) -> Result<Option<Body>> {
    if let Some(body) = swept_prism(a, b, op) {
        return Ok(Some(body));
    }
    a.require_brep("a boolean")?;
    b.require_brep("a boolean")?;
    let size = a.size().max(b.size());
    let counts = record::counts(&[a, b]);
    // The checks mesh both bodies; where they meet in a region much thinner than they are (a
    // thin part trimmed by a long one), a step from their size would facet the small faces
    // there coarsely enough to fail the membership test on a correct result. The meshes are
    // refined: a round side trimmed to a thin band (a short boss crossing a gently sloping
    // floor) is otherwise spanned by triangles cutting through it, which reads as missing
    // material.
    let mut tol_m = (size * 5e-4).min(meeting_size(a, b) * 2e-3).max(size * 1e-5);
    // A prism pair (a plate and the tool through it) is decided exactly in 2D, with no mesh of
    // either operand. The sampled shortcuts below need a refined mesh of both, and that mesh is
    // most of what a boolean costs: a pattern or a multi-profile sketch rebuilds the whole
    // target's mesh once per tool. Only a cut or an intersection meets its tool often enough to
    // be worth asking first - parts that stay apart are settled far more cheaply by the shell
    // join the sampled shortcut does. A pair the route cannot answer still takes the path below,
    // unchanged.
    let early_prism = matches!(op, BoolOp::Cut | BoolOp::Intersect);
    let prism = early_prism.then(|| prism_result(a, b, op, size, &counts, crate::prism::prism_boolean)).flatten();
    if let Some(Ok(Some(body))) = &prism {
        let (va0, vb0) = (volume(a), volume(b));
        let (v0, slack0) = (volume(body), 2e-3 * (va0 + vb0) + 1e-9);
        // A tool that misses its target, or covers it whole, leaves it as it is, and the
        // sampled shortcut below answers that by reusing the operand itself: the very same body.
        // The 2D route rebuilds it instead - the same shape on new faces - which would turn an
        // untouched body in a timeline into a different solid. The route declines those pairs
        // itself, from its exact sections (a volume measured off meshes could not tell an
        // engraving of thin strokes from no change), and they take the path below, which
        // decides them, and the identity they keep, exactly as it always has.
        if plausible(v0, op, va0, vb0, slack0) {
            return Ok(Some(body.clone()));
        }
    }
    let mut meshes = (a.tessellate_with(tol_m, true).ok(), b.tessellate_with(tol_m, true).ok());
    // The vertex boxes above miss a curved operand's reach (a cylinder has only seam vertices),
    // so a thin meeting region of curved bodies kept the coarse step: a sliver 0.5 mm thick was
    // faceted at 0.017 mm and a correct result failed the membership test. The meshes just made
    // do span the bodies, so the step is set again from where they meet and the meshes are
    // remade once, when that is a much finer step.
    let refined = match &meshes {
        // A meeting box only a few steps thick is the tessellation error of bodies that merely
        // touch or miss each other: a finer mesh there cannot help and only slows the operation.
        (Some(ma), Some(mb)) => {
            let meet = mesh_meeting_size(ma, mb);
            (meet > tol_m * 8.0).then(|| (size * 5e-4).min(meet * 2e-3).max(size * 1e-5))
        }
        _ => None,
    };
    if let Some(refined) = refined
        && refined < tol_m * 0.5
    {
        let again = (a.tessellate_with(refined, true).ok(), b.tessellate_with(refined, true).ok());
        if again.0.is_some() && again.1.is_some() {
            tol_m = refined;
            meshes = again;
        }
    }
    let (va, vb) = match &meshes {
        (Some(ma), Some(mb)) => (ma.measure().volume, mb.measure().volume),
        _ => (volume(a), volume(b)),
    };
    let slack = 2e-3 * (va + vb) + 1e-9;
    // Volume bounds, then membership against the operands (catches misclassified pieces), on
    // one mesh of the result. Returns (plausible, volume).
    // Shells that touch were never fused: coincident back-to-back faces leave the intersection
    // nothing to cut, so it returns both operands side by side (#396). Volume and point
    // sampling cannot see that; the shell grouping can. Only contact over an area counts
    // (bodies tangent along a line stay two lumps), and operands already like that, saved
    // before this was caught, stay joinable.
    let unfused = |r: &Body| {
        op == BoolOp::Union
            && r.shell_count() > 1
            && r.union_lumps().is_err()
            && a.union_lumps().is_ok()
            && b.union_lumps().is_ok()
            && crate::coplanar::faces_touch(a, b)
    };
    let check = |r: &Body| -> (bool, f64) {
        let Ok(mr) = r.tessellate_with(tol_m, true) else { return (false, f64::NAN) };
        let v = mr.measure().volume;
        let bounds = plausible(v, op, va, vb, slack);
        if !bounds {
            return (false, v);
        }
        if unfused(r) {
            return (false, v);
        }
        let consistent = match &meshes {
            (Some(ma), Some(mb)) => mismatch(ma, mb, &mr, op).is_none_or(|f| f < 0.004),
            _ => true,
        };
        (consistent, v)
    };
    if refine {
        let keep: Vec<Vec<Vec3>> = a.split_keep().iter().chain(b.split_keep()).cloned().collect();
        // Bounded to two finer meshes; no tool displacement or relaxed acceptance tests.
        for k in [1e-5, 1e-6] {
            let tol = (size * k).max(1e-6);
            if let Ok((solid, history)) = truck_boolean(a, b, op, None, tol)
                && !solid.boundaries().is_empty()
                && let Ok(body) = guard("refined boolean result", || recorded::truck_result(solid, history, size, &keep, &counts))
                && check(&body).0
                && crate::boolean_guard::check_retry(a, b, op, Some(&body)).is_ok()
            {
                let mut event = BooleanFallback::new(BooleanStrategy::RefinedTolerance);
                event.tolerance_mm = tol;
                record(event);
                return Ok(Some(body));
            }
        }
        return Err(KernelError::Failed("no reliable result at refined tolerances".into()));
    }
    // One body clear of or inside the other: no intersection needed.
    // Kept for the end when the facetings disagree with it: bodies that touch along curved
    // faces can pass through each other slightly, and often can't be intersected.
    let mut doubted = None;
    if let (Some(ma), Some(mb)) = &meshes
        && let Some((r, sure)) = trivial(a, b, op, ma, mb, &counts, 3.0 * tol_m)
    {
        if sure {
            return Ok(r);
        }
        doubted = Some(r);
    }
    let mut last = String::new();
    // Coincident faces make the intersection fail after many retries; push them apart first.
    // Prisms along one direction (plates with holes): exact, in 2D (so only the volume bounds
    // are checked). For a cut or an intersection this already ran above; the other operations
    // ask here, after the sampled shortcut, which is where that shortcut is cheapest.
    let prism = prism.or_else(|| (!early_prism).then(|| prism_result(a, b, op, size, &counts, crate::prism::prism_boolean)).flatten());
    if let Some(result) = prism {
        match result {
            Ok(Some(body)) => {
                // Measured as the operands were: a coarser mesh of a plate with many holes
                // overstates its volume by more than the slack.
                let v = match &meshes {
                    (Some(_), Some(_)) => body.tessellate_with(tol_m, true).map(|m| m.measure().volume).unwrap_or(f64::NAN),
                    _ => volume(&body),
                };
                if plausible(v, op, va, vb, slack) {
                    return Ok(Some(body));
                }
            }
            Ok(None) => return Ok(None),
            // A failed prism attempt keeps trying the other strategies, as it did before.
            Err(_) => {}
        }
    }
    // Bodies touching along one whole face (a part joined with its mirror image).
    if op == BoolOp::Union
        && a.solid.boundaries().len() == 1
        && b.solid.boundaries().len() == 1
        && let Some(Ok(body)) = crate::coplanar::glue(a, b)
    {
        let (ok, _) = check(&body);
        if ok {
            return Ok(Some(body));
        }
    }
    // Bores on a common cylinder (a pad's bore around the hole it is joined over, a disc filling
    // a hole). Asked before faces are pushed apart: that doesn't part walls on one cylinder, and
    // its tries at the general boolean, all failing on those walls, cost seconds.
    if op == BoolOp::Union && !BORES.with(|c| c.get()) && crate::coplanar::has_coincident_cylinders(a, b) {
        BORES.with(|c| c.set(true));
        let (r, diagnostics) = capture_boolean_diagnostics(|| crate::coplanar::bore_union(a, b, &|body| check(body).0));
        BORES.with(|c| c.set(false));
        if let Some(body) = r {
            merge(diagnostics);
            return Ok(Some(body));
        }
    }
    // Coincident planar faces: push them apart (exactly). Tried once, before the general
    // boolean; why it failed is reported if everything else fails too.
    let mut apart_failed = String::new();
    // The exact polygon boolean's answer, when it was asked for early.
    let mut planar = None;
    if !APART.with(|c| c.get()) {
        APART.with(|c| c.set(true));
        let (r, diagnostics) = capture_boolean_diagnostics(|| crate::coplanar::boolean_apart(a, b, op));
        APART.with(|c| c.set(false));
        match r {
            Some(Ok(Some(body))) => {
                let (ok, v) = check(&body);
                if ok {
                    merge(diagnostics);
                    let mut event = BooleanFallback::new(BooleanStrategy::CoplanarSeparation);
                    event.face_shift_mm = size * 0.0229;
                    record(event);
                    return Ok(Some(body));
                }
                apart_failed = format!("; faces pushed apart gave an implausible volume {v:.4}");
            }
            Some(Err(e)) => apart_failed = format!("; {e}"),
            Some(Ok(None)) | None => {}
        }
        // A face with no room to move would take or add what it moves through. The exact
        // polygon boolean takes the bodies as they lie (bodies with only planar faces, or a
        // planar tool clear of the other body's curved faces): it is asked now, before the
        // general boolean has its tries at faces lying in each other.
        if apart_failed.contains(crate::coplanar::NO_ROOM) {
            // What its own booleans report counts only when its result is taken.
            let (exact, diagnostics) = capture_boolean_diagnostics(|| crate::polybool::planar_boolean(a, b, op));
            match exact {
                Ok(Some(body)) if check(&body).0 => {
                    merge(diagnostics);
                    record(BooleanFallback::new(BooleanStrategy::PlanarFallback));
                    return Ok(Some(body));
                }
                other => planar = Some(other),
            }
        }
    }
    // A face standing wholly inside a coincident face of the other body, back to back, and no
    // other contact (#396): exact, by topology alone.
    if op == BoolOp::Union
        && a.solid.boundaries().len() == 1
        && b.solid.boundaries().len() == 1
        && let Some(Ok(body)) = crate::coplanar::inset(a, b)
        && let (Some(ma), Some(mb)) = &meshes
        && overlap(ma, mb, &ma.inside_index(), &mb.inside_index()).is_some_and(|(b_in_a, a_in_b)| b_in_a < NONE && a_in_b < NONE)
        && check(&body).0
    {
        return Ok(Some(body));
    }
    // Faces back to back over part of their area: imprinted on each other and glued (exact; held
    // to the retry guard's checks all the same).
    if op == BoolOp::Union && crate::coplanar::faces_touch(a, b) {
        let accept = |body: &Body| check(body).0 && !unfused(body) && crate::boolean_guard::check_retry(a, b, op, Some(body)).is_ok();
        // Imprinted along the contact's own outline (faces there bounded by lines and arcs), then
        // along the sides' surfaces.
        if let Some(body) = crate::outline::outline_union(a, b, &accept) {
            return Ok(Some(body));
        }
        if let Some(body) = crate::coplanar::imprint_union(a, b, &accept) {
            return Ok(Some(body));
        }
    }
    let mut empty_votes = 0;
    let mut empty_diagnostics = crate::BooleanDiagnostics::default();
    let keep: Vec<Vec<Vec3>> = a.split_keep().iter().chain(b.split_keep()).cloned().collect();
    // The tolerances below follow the larger body. Where the bodies meet in a region much
    // smaller than that (a hole a few millimetres across drilled through a long part, whose tool
    // is longer still), they are as coarse as the tool itself: its intersection curves are
    // traced at a step wider than the hole, and the cut comes out empty or fails. When those
    // tries fail, the same boolean is asked again at tolerances (and shifts) scaled to where the
    // bodies meet. Its result is checked as the others are, and held to the retry guard.
    // Bodies in several shells are retried a piece at a time (see [`boolean`]), where each
    // piece gets this pass of its own.
    let fine_tols: Vec<f64> = match &meshes {
        (Some(ma), Some(mb)) if a.solid.boundaries().len() == 1 && b.solid.boundaries().len() == 1 => {
            let (ba, bb) = (ma.bounds(), mb.bounds());
            let s = solidcraft_geom::Aabb3 { min: ba.min.max(bb.min), max: ba.max.min(bb.max) }.size();
            // The thinner operand's thickness (twice its volume over its area: a plate's
            // thickness, a rod's radius) bounds what the tolerance must resolve as well.
            let thickness = |m: &solidcraft_geom::Mesh| {
                let x = m.measure();
                if x.area > 0.0 { 2.0 * x.volume.abs() / x.area } else { f64::INFINITY }
            };
            let feature = s.x.min(s.y).min(s.z).min(thickness(ma)).min(thickness(mb)).max(size * 1e-4);
            if feature > 0.0 && feature * 2e-3 < size * 2.5e-4 { vec![(feature * 2e-3).max(1e-6), (feature * 5e-4).max(1e-6)] } else { Vec::new() }
        }
        _ => Vec::new(),
    };
    let fine = |last: &mut String| -> Option<Body> {
        let feature = fine_tols.first().map(|t| t / 2e-3)?;
        // Unshifted, then once shifted: each try at these tolerances costs many coarser ones.
        for (attempt, j) in JITTER.iter().enumerate().take(2) {
            let shift = mt::Vector3::new(j[0], j[1], j[2]) * (feature * 0.01);
            for &tol in &fine_tols {
                match truck_boolean(a, b, op, if attempt > 0 { Some(shift) } else { None }, tol) {
                    Ok((s, _)) if s.boundaries().is_empty() || s.face_iter().next().is_none() => *last = "empty result".into(),
                    Ok((s, h)) => match guard("boolean result", || recorded::truck_result(s, h, size, &keep, &counts)) {
                        Ok(body) => {
                            let (ok, v) = check(&body);
                            if ok {
                                let mut event = BooleanFallback::new(if attempt > 0 {
                                    BooleanStrategy::ShiftedOperands
                                } else {
                                    BooleanStrategy::RefinedTolerance
                                });
                                event.translation_mm = Vec3::new(j[0], j[1], j[2]).len() * (feature * 0.01);
                                event.tolerance_mm = tol;
                                record(event);
                                return Some(body);
                            }
                            *last = format!("implausible result volume {v:.4}");
                        }
                        Err(e) => *last = e.to_string(),
                    },
                    Err(e) => *last = e.to_string(),
                }
            }
        }
        None
    };
    for (attempt, j) in JITTER.iter().enumerate() {
        // The operands as they lie failed. A planar tool beside the target's curved faces (a tooth
        // or a pocket cut next to a round edge) is decided exactly in the slab it lies in, where
        // the target is planar too, before the shifted tries: that is where they end anyway, as a
        // retry, after costing several times as much (on every copy of a pattern, #1197).
        if attempt == 1 && matches!(op, BoolOp::Cut | BoolOp::Intersect) {
            let (sliced, diagnostics) = capture_boolean_diagnostics(|| crate::polybool::sliced_planar_boolean(a, b, op));
            if let Some(Ok(Some(body))) = sliced
                && check(&body).0
            {
                merge(diagnostics);
                return Ok(Some(body));
            }
        }
        // A prism tool whose walls only touch the target (a corner of its outline on a ring's bore
        // or in its end face, the wall beside it outside the ring): with those walls pushed
        // outward, clear of the target, it takes the same cut exactly, without the contact the
        // tries below fail on (and the nudge after them got through only seconds later, leaving a
        // face of no width at the corner).
        if attempt == 1
            && op == BoolOp::Cut
            && let (Some(ma), Some(_)) = &meshes
            && let Some(body) = pushed_walls_cut(a, b, ma, 2.0 * tol_m, size, &keep, &counts, &check)
        {
            return Ok(Some(body));
        }
        // A prism tool whose target is a prism along its direction only round it (a pocket made
        // deeper in a plate with a chamfered rim, a ring cut round a counterbored hole in a
        // chamfered disc): the exact 2D route in a column cut round the tool. Its walls lying on
        // the target's are what the tries below fail on.
        if attempt == 1 && matches!(op, BoolOp::Cut | BoolOp::Intersect) {
            let (local, diagnostics) = capture_boolean_diagnostics(|| crate::prism_local::local_prism_boolean(a, b, op));
            if let Some(Ok(Some(body))) = local
                && check(&body).0
            {
                merge(diagnostics);
                return Ok(Some(body));
            }
        }
        // A whole turn of a cylinder or cone bounded by one wire along its seam (as STEP files
        // write them) can't be divided by a loop lying inside it, and the tries below end in a
        // nudged tool, or fail: the same boolean with such faces in two halves, which is exact.
        if attempt == 1 && crate::seam_faces::needed(a, b) {
            let (halves, diagnostics) = capture_boolean_diagnostics(|| crate::seam_faces::boolean(a, b, op, &boolean));
            if let Some(Ok(r)) = halves
                && r.as_ref().is_none_or(|body| check(body).0)
            {
                merge(diagnostics);
                return Ok(r);
            }
        }
        let shift = mt::Vector3::new(j[0], j[1], j[2]) * (size * 0.01);
        for k in [2e-3, 5e-4] {
            let tol = (size * k).max(1e-5);
            let r = truck_boolean(a, b, op, if attempt > 0 { Some(shift) } else { None }, tol);
            match r {
                Ok((s, _)) if s.boundaries().is_empty() || s.face_iter().next().is_none() => {
                    empty_votes += 1;
                    if attempt > 0 || k != 2e-3 {
                        let mut event =
                            BooleanFallback::new(if attempt > 0 { BooleanStrategy::ShiftedOperands } else { BooleanStrategy::RefinedTolerance });
                        event.translation_mm = Vec3::new(j[0], j[1], j[2]).len() * (size * 0.01);
                        event.tolerance_mm = tol;
                        empty_diagnostics.push(event);
                    }
                    last = "empty result".into();
                }
                Ok((s, h)) => match guard("boolean result", || recorded::truck_result(s, h, size, &keep, &counts)) {
                    Ok(body) => {
                        let (ok, v) = check(&body);
                        if ok {
                            if attempt > 0 || k != 2e-3 {
                                let mut event = BooleanFallback::new(if attempt > 0 {
                                    BooleanStrategy::ShiftedOperands
                                } else {
                                    BooleanStrategy::RefinedTolerance
                                });
                                event.translation_mm = Vec3::new(j[0], j[1], j[2]).len() * (size * 0.01);
                                event.tolerance_mm = tol;
                                record(event);
                            }
                            return Ok(Some(body));
                        }
                        last = if unfused(&body) {
                            "the bodies touch along faces that could not be fused".into()
                        } else {
                            format!("implausible result volume {v:.4}")
                        };
                    }
                    Err(e) => last = e.to_string(),
                },
                Err(e) => last = e.to_string(),
            }
        }
        if empty_votes >= 2 && op != BoolOp::Union {
            if let Some(body) = fine(&mut last) {
                return Ok(Some(body));
            }
            merge(empty_diagnostics);
            return Ok(None);
        }
    }
    last.push_str(&apart_failed);
    // Coincident faces defeat the B-rep boolean; bodies with only planar faces have an exact
    // polygon fallback.
    match planar.unwrap_or_else(|| crate::polybool::planar_boolean(a, b, op)) {
        Ok(Some(body)) => {
            let (ok, v) = check(&body);
            if ok {
                record(BooleanFallback::new(BooleanStrategy::PlanarFallback));
                return Ok(Some(body));
            }
            last = format!("planar fallback gave an implausible volume {v:.4}");
        }
        Ok(None) => {
            record(BooleanFallback::new(BooleanStrategy::PlanarFallback));
            return Ok(None);
        }
        Err(e) => last = format!("{last}; {e}"),
    }
    // Prisms along one direction that end at different levels (a barrel joined between two
    // faces of a wider part, its ends flush with them) were left to the B-rep boolean, which
    // the coincident ends defeat, with no room to push them apart (the part goes on past
    // them). In 2D they join exactly: asked before the tool is moved.
    if let Some(Ok(Some(body))) = prism_result(a, b, op, size, &counts, crate::prism::prism_union_between_caps) {
        let (ok, v) = check(&body);
        if ok {
            return Ok(Some(body));
        }
        last = format!("{last}; the prisms joined in 2D gave an implausible volume {v:.4}");
    }
    // Last resort: nudge the tool by a micron-scale screw motion (turned 2e-6 rad about a
    // skew axis and moved 2e-6 of the size), which breaks tangencies and seams lying exactly on
    // the other body. The result moves by no more than that.
    if !APART.with(|c| c.get()) {
        APART.with(|c| c.set(true));
        let c = b.solid.vertex_iter().fold(Vec3::ZERO, |acc, v| acc + from_p3(v.point())) * (1.0 / b.solid.vertex_iter().count().max(1) as f64);
        let translation = |screw: usize| SCREWS.get(screw).map_or(Vec3::ZERO, |s| Vec3::new(s.0[0], s.0[1], s.0[2]) * (size * 2e-6));
        let nudge = |screw: usize, sign: f64| {
            let axis = SCREWS.get(screw).map_or(Vec3::Z, |s| Vec3::new(s.1[0], s.1[1], s.1[2]));
            capture_boolean_diagnostics(|| {
                transform(b, translation(screw) * sign, c, axis, 2e-6 * sign).and_then(|nb| Ok(boolean(a, &nb, op)?.map(|r| nudged(r, &nb, &counts))))
            })
        };
        let mut used = 0;
        let (mut r, mut diagnostics) = nudge(used, 1.0);
        // A nudge that pushes a face into a curved face it only touched (a cut whose end lies
        // tangent to a round side) leaves a sliver of that face behind, a fraction of a
        // millimetre wide, which later features (a shell, a fillet) can't work with. The
        // opposite nudge parts them instead. It is tried when this one failed or left more
        // slivers than the operands brought (a thin wall or a small round is no reason to run
        // another boolean), and taken when it works with fewer; an empty result stays as it is.
        let slivers = match &r {
            Ok(Some(body)) if check(body).0 => Some(sliver_faces(body, size)),
            _ => None,
        };
        let made = slivers.is_none_or(|n| n > 0 && n > sliver_faces(a, size).saturating_add(sliver_faces(b, size)));
        if !matches!(r, Ok(None)) && made {
            let (r2, diagnostics2) = nudge(used, -1.0);
            if let Ok(Some(body)) = &r2
                && slivers.is_none_or(|n| sliver_faces(body, size) < n)
                && check(body).0
            {
                (r, diagnostics) = (r2, diagnostics2);
            }
        }
        // Neither way worked: the screw can turn the tool along a seam it lies in (a loft
        // standing in the rim of a shelled case with its ends in the wall's inner face, #1490).
        // The other screws are tried in turn, and the first result that checks is taken.
        let works = |r: &Result<Option<Body>>| match r {
            Ok(None) => true,
            Ok(Some(body)) => check(body).0,
            Err(_) => false,
        };
        if !works(&r) {
            'screws: for screw in 1..SCREWS.len() {
                for sign in [1.0, -1.0] {
                    let (r2, diagnostics2) = nudge(screw, sign);
                    if matches!(&r2, Ok(Some(body)) if check(body).0) {
                        (r, diagnostics, used) = (r2, diagnostics2, screw);
                        break 'screws;
                    }
                }
            }
        }
        let translation = translation(used);
        APART.with(|c| c.set(false));
        match r {
            Ok(Some(body)) => {
                let (ok, v) = check(&body);
                // Measured finely too: a face the boolean left a hair wide, its loops a hair
                // apart, can mesh as its whole plane at a fine step though it meshed as a sliver
                // at the check's (#1360: an annulus cut flush with a hole's wall came out 21000
                // mm³ larger than the part it was cut from, a square plane across it).
                let fine_ok = ok && crate::measure(&body).is_ok_and(|m| plausible(m.volume, op, va, vb, slack));
                if ok && !fine_ok {
                    last = format!("{last}; the nudged tool gave a solid whose faces mesh inconsistently");
                } else if ok {
                    merge(diagnostics);
                    let mut event = BooleanFallback::new(BooleanStrategy::ToolNudge);
                    event.translation_mm = translation.len();
                    event.rotation_rad = 2e-6;
                    record(event);
                    return Ok(Some(body));
                } else {
                    last = format!("{last}; the nudged tool gave an implausible volume {v:.4}");
                }
            }
            Ok(None) => {
                merge(diagnostics);
                let mut event = BooleanFallback::new(BooleanStrategy::ToolNudge);
                event.translation_mm = translation.len();
                event.rotation_rad = 2e-6;
                record(event);
                return Ok(None);
            }
            Err(e) => last = format!("{last}; nudged: {e}"),
        }
    }
    // A join whose coincident planar faces cannot be pushed apart because the neighbours of one of them curve (a flange
    // flush with the end of a lofted tube): the faces are grown instead, and what the growth leaves beyond the plane is cut
    // off by editing the faces. After every route above, the nudge included, has failed, so that a join any of them takes
    // stays as it was.
    if op == BoolOp::Union && !APART.with(|c| c.get()) {
        APART.with(|c| c.set(true));
        let (r, diagnostics) = capture_boolean_diagnostics(|| crate::coplanar::boolean_grown(a, b, op));
        APART.with(|c| c.set(false));
        match r {
            Some(Ok(Some(body))) => {
                let (ok, v) = check(&body);
                if ok {
                    merge(diagnostics);
                    let mut event = BooleanFallback::new(BooleanStrategy::CoplanarSeparation);
                    event.face_shift_mm = size * 0.0137;
                    record(event);
                    return Ok(Some(body));
                }
                last = format!("{last}; faces grown apart gave an implausible volume {v:.4}");
            }
            Some(Err(e)) => last = format!("{last}; {e}"),
            Some(Ok(None)) | None => {}
        }
    }
    // The tries at the meeting region's scale come last, after every strategy that answered
    // before them, so a result any of those gave is kept as it was.
    if let Some(body) = fine(&mut last) {
        return Ok(Some(body));
    }
    match doubted {
        Some(r) => {
            let mut event = BooleanFallback::new(BooleanStrategy::UnverifiedShortcut);
            event.tolerance_mm = 3.0 * tol_m;
            record(event);
            Ok(r)
        }
        None => Err(KernelError::Failed(format!("boolean {op:?}: {last}"))),
    }
}

/// `a` cut by `b` with the walls of `b` that only touch `a` pushed outward (see
/// [`crate::touching_walls`]), at each of the general boolean's tolerances by a little more than
/// it (at least `by`, past the sag of `target`, the mesh of `a`): the first result that checks
/// and whose faces lie on none of the pushed walls. The pushed tool keeps `b`'s order, so the
/// history holds for `b`.
#[allow(clippy::too_many_arguments)]
fn pushed_walls_cut(
    a: &Body,
    b: &Body,
    target: &solidcraft_geom::Mesh,
    by: f64,
    size: f64,
    keep: &[Vec<Vec3>],
    counts: &[(usize, usize)],
    check: &dyn Fn(&Body) -> (bool, f64),
) -> Option<Body> {
    let inside = target.inside_index();
    for k in [2e-3, 5e-4] {
        // Pushed past what the boolean takes for touching, and past the mesh's sag.
        let tol = (size * k).max(1e-5);
        let Some(pushed) = crate::touching_walls::pushed_tool(b, &inside, (2.5 * tol).max(by)) else { continue };
        let Ok((s, h)) = truck_boolean(a, &pushed.tool, BoolOp::Cut, None, tol) else { continue };
        if s.boundaries().is_empty() || s.face_iter().next().is_none() {
            continue;
        }
        let Ok(body) = guard("boolean result", || recorded::truck_result(s, h, size, keep, counts)) else { continue };
        if pushed.untouched(&body) && check(&body).0 {
            return Some(body);
        }
    }
    None
}

/// The screw motions a tool is nudged by as a last resort: (direction of the move, axis of the
/// turn), each tried both ways. The first is the one every nudge used before #1490; the others
/// are asked only when it fails both ways.
const SCREWS: [([f64; 3], [f64; 3]); 3] =
    [([0.61, -0.37, 0.71], [0.27, 0.83, -0.49]), ([-0.42, 0.68, 0.6], [0.77, -0.18, 0.61]), ([0.31, 0.52, -0.79], [-0.58, 0.41, 0.70])];

/// Faces of `b` narrower than 1/200 of `size` (twice their area over their boundary's
/// length): the slivers a nudged tool leaves where it was pushed into a face it touched.
fn sliver_faces(b: &Body, size: f64) -> usize {
    use std::collections::HashMap;
    let Ok(m) = b.tessellate((size * 1e-3).max(1e-4)) else { return usize::MAX };
    let q = (size * 1e-7).max(1e-12);
    let key = |p: Vec3| ((p.x / q).round() as i64, (p.y / q).round() as i64, (p.z / q).round() as i64);
    // Per face: its area, and how many of its triangles use each mesh edge (its boundary once).
    let mut area: HashMap<u32, f64> = HashMap::new();
    let mut sides: HashMap<(u32, (i64, i64, i64), (i64, i64, i64)), (u32, f64)> = HashMap::new();
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        let Some([a, b, c]) = m.tri(t) else { continue };
        *area.entry(*f).or_default() += (b - a).cross(c - a).len() * 0.5;
        for (p, r) in [(a, b), (b, c), (c, a)] {
            let (kp, kr) = (key(p), key(r));
            let k = if kp <= kr { (*f, kp, kr) } else { (*f, kr, kp) };
            sides.entry(k).or_insert((0, p.dist(r))).0 += 1;
        }
    }
    let mut boundary: HashMap<u32, f64> = HashMap::new();
    for ((f, _, _), (n, len)) in &sides {
        if *n == 1 {
            *boundary.entry(*f).or_default() += len;
        }
    }
    area.iter().filter(|(f, a)| boundary.get(f).is_some_and(|l| *l > 0.0 && 2.0 * **a / l < size * 5e-3)).count()
}

/// `r`, a boolean of the target and `nb` (the tool nudged), with its history over the target and
/// the tool itself (the nudge keeps every face and edge).
pub(crate) fn nudged(r: Body, nb: &Body, counts: &[(usize, usize)]) -> Body {
    let h = recorded::through(&r, 1, nb.history());
    r.with_checked_history(h, counts)
}

/// What truck's boolean makes of `a` and `b`: the solid, and its history in our terms (see
/// [`recorded::shapeops`]). On copies of the operands moved by `shift` (when given; the result
/// is moved back), which changes the rays the boolean classifies faces by. Copies (and moved
/// copies) keep the operands' face and edge order, and so does moving the result back, so the
/// history truck records holds for the operands themselves.
pub(crate) fn truck_boolean(a: &Body, b: &Body, op: BoolOp, shift: Option<mt::Vector3>, tol: f64) -> Result<(Solid, Option<recorded::Shapeops>)> {
    guard("boolean", || {
        let (mut sa, mut sb) = (a.deep_copy(), b.deep_copy());
        if let Some(shift) = shift {
            sa = mt::builder::translated(&sa, shift);
            sb = mt::builder::translated(&sb, shift);
        }
        let tool_edges = (op == BoolOp::Cut).then(|| record::edge_origins(&sb, 1));
        let res = match op {
            BoolOp::Union => truck_shapeops::or_with_history(&sa, &sb, tol),
            BoolOp::Cut => {
                sb.not();
                truck_shapeops::and_with_history(&sa, &sb, tol)
            }
            BoolOp::Intersect => truck_shapeops::and_with_history(&sa, &sb, tol),
        };
        let (s, h) = res.ok_or_else(|| KernelError::Failed("no result".into()))?;
        let h = h.and_then(|h| recorded::shapeops(&h, &sb, tool_edges.as_ref()));
        Ok((if let Some(shift) = shift { mt::builder::translated(&s, -shift) } else { s }, h))
    })
}

/// A boolean of `a` and `b` given as their pieces (`la`, `lb`); the result's pieces in one body.
pub(crate) fn by_lumps(a: &Body, b: &Body, la: Vec<Body>, lb: Vec<Body>, op: BoolOp) -> Result<Option<Body>> {
    let boxes_meet = |x: &Body, y: &Body| -> bool {
        let (Ok(mx), Ok(my)) = (x.tessellate(x.size() * 0.05 + 1e-3), y.tessellate(y.size() * 0.05 + 1e-3)) else { return true };
        let (bx, by) = (mx.bounds(), my.bounds());
        let g = (x.size() + y.size()) * 0.01;
        bx.min.x <= by.max.x + g
            && by.min.x <= bx.max.x + g
            && bx.min.y <= by.max.y + g
            && by.min.y <= bx.max.y + g
            && bx.min.z <= by.max.z + g
            && by.min.z <= bx.max.z + g
    };
    // Each piece with its history over [a, b] (a piece's faces and edges are its body's own).
    let traced = |pieces: Vec<Body>, whole: &Body, operand: usize| -> Vec<Traced> {
        let histories = recorded::parts_of(&pieces, whole);
        pieces.into_iter().zip(histories).map(|(p, h)| (p, recorded::placed(h, operand, 2))).collect()
    };
    let (la, lb) = (traced(la, a, 0), traced(lb, b, 1));
    let pair = |(x, hx): &Traced, (y, hy): &Traced, op: BoolOp| -> Result<Option<Traced>> {
        Ok(boolean(x, y, op)?.map(|r| {
            let h = recorded::of_pair(&r, hx, hy);
            (r, h)
        }))
    };
    let mut out: Vec<Traced> = Vec::new();
    match op {
        BoolOp::Cut => {
            for x in la {
                let mut cur = Some(x);
                for y in &lb {
                    cur = match cur {
                        Some(c) if boxes_meet(&c.0, &y.0) => pair(&c, y, BoolOp::Cut)?,
                        other => other,
                    };
                }
                out.extend(cur);
            }
        }
        BoolOp::Intersect => {
            for x in &la {
                for y in &lb {
                    if boxes_meet(&x.0, &y.0)
                        && let Some(r) = pair(x, y, BoolOp::Intersect)?
                    {
                        out.push(r);
                    }
                }
            }
        }
        BoolOp::Union => {
            out = la;
            for y in lb {
                let mut cur = y;
                let mut rest = Vec::new();
                for x in out {
                    if boxes_meet(&x.0, &cur.0) {
                        match pair(&x, &cur, BoolOp::Union)? {
                            Some(u) if u.0.solid.boundaries().len() == 1 => cur = u,
                            _ => rest.push(x),
                        }
                    } else {
                        rest.push(x);
                    }
                }
                rest.push(cur);
                out = rest;
            }
        }
    }
    recorded::join(out, &record::counts(&[a, b]))
}

/// Bodies whose bounding boxes are clearly apart, as one body of several pieces, without any
/// intersection work (many identical pattern tools, #261). `None` when any two may meet, or a
/// body is a mesh: the caller unions those the usual way.
pub fn union_apart(bodies: &[Body]) -> Result<Option<Body>> {
    crate::profile::timed(crate::profile::Kind::Boolean, || union_apart_untimed(bodies))
}

fn union_apart_untimed(bodies: &[Body]) -> Result<Option<Body>> {
    if bodies.len() < 2 || bodies.len() > 10_000 || bodies.iter().any(Body::is_mesh) {
        return Ok(None);
    }
    let mut boxes = Vec::with_capacity(bodies.len());
    for b in bodies {
        // A coarse mesh is enough: the chord tolerance bounds how far the true surface lies
        // outside it, and the box grows by twice that.
        let tol = (b.size() * 0.02).max(1e-3);
        let bb = b.tessellate(tol)?.bounds();
        if bb.is_empty() {
            return Ok(None);
        }
        let pad = Vec3::new(1.0, 1.0, 1.0) * (2.0 * tol);
        boxes.push(solidcraft_geom::Aabb3 { min: bb.min - pad, max: bb.max + pad });
    }
    // Sweep along x: only boxes whose x ranges overlap are compared.
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|&i, &j| boxes.get(i).map(|b| b.min.x).unwrap_or(0.0).total_cmp(&boxes.get(j).map(|b| b.min.x).unwrap_or(0.0)));
    for (k, &i) in order.iter().enumerate() {
        let Some(a) = boxes.get(i) else { continue };
        for &j in order.iter().skip(k + 1) {
            let Some(b) = boxes.get(j) else { continue };
            if b.min.x > a.max.x {
                break;
            }
            if a.min.y <= b.max.y && b.min.y <= a.max.y && a.min.z <= b.max.z && b.min.z <= a.max.z {
                return Ok(None);
            }
        }
    }
    join_apart(bodies)
}

/// Bodies the caller knows are apart (no two meet or touch), as one body of several pieces,
/// without any intersection work: [`union_apart`] without its check, for tools whose making
/// shows they are apart (profiles apart in their sketch, extruded alike). `None` for fewer than
/// two bodies or a mesh.
pub fn join_apart(bodies: &[Body]) -> Result<Option<Body>> {
    crate::profile::timed(crate::profile::Kind::Boolean, || join_apart_untimed(bodies))
}

fn join_apart_untimed(bodies: &[Body]) -> Result<Option<Body>> {
    if bodies.len() < 2 || bodies.len() > 10_000 || bodies.iter().any(Body::is_mesh) {
        return Ok(None);
    }
    // The pieces stay as they are, side by side: each face and edge of the result is its piece's
    // own (the history over `bodies`).
    let Some(joined) = join_pieces(bodies.to_vec())? else { return Ok(None) };
    let operands: Vec<&Body> = bodies.iter().collect();
    let faces = bodies
        .iter()
        .enumerate()
        .flat_map(|(operand, b)| {
            (0..b.face_count()).map(move |face| (std::collections::BTreeSet::from([crate::history::FaceOrigin::Face { operand, face }]), true))
        })
        .collect();
    let mut edges = HashMap::new();
    for (operand, b) in bodies.iter().enumerate() {
        edges.extend(record::edge_origins(&b.solid, operand));
    }
    Ok(Some(record::traced(joined, &record::counts(&operands), Some(record::regions(faces)), Some(&edges))))
}

/// Bodies (apart from each other) as one body of several pieces.
pub(crate) fn join_pieces(pieces: Vec<Body>) -> Result<Option<Body>> {
    match pieces.len() {
        0 => Ok(None),
        1 => Ok(pieces.into_iter().next()),
        _ => {
            let shells: Vec<mt::Shell> = pieces.iter().flat_map(|p| p.solid.boundaries().clone()).collect();
            let solid = guard("pieces", || Solid::try_new(shells).map_err(|e| KernelError::Failed(e.to_string())))?;
            let protected = pieces.iter().any(|piece| piece.regularization_protected);
            Body::new(solid).map(|mut body| {
                body.regularization_protected = protected;
                Some(body)
            })
        }
    }
}

/// Determinant of the linear part of a column-major 4x4 matrix: negative means the placement
/// mirrors, which turns a solid inside out.
fn det3(m: &[[f64; 4]; 4]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// Whether the linear part is a rigid motion (rotation or reflection: no scale or shear).
fn rigid_placement(m: &[[f64; 4]; 4]) -> bool {
    let r = [Vec3::new(m[0][0], m[0][1], m[0][2]), Vec3::new(m[1][0], m[1][1], m[1][2]), Vec3::new(m[2][0], m[2][1], m[2][2])];
    (0..3).all(|i| (r[i].dot(r[i]) - 1.0).abs() < 1e-9) && r[0].dot(r[1]).abs() < 1e-9 && r[0].dot(r[2]).abs() < 1e-9 && r[1].dot(r[2]).abs() < 1e-9
}

/// Affine transform by a column-major 4x4 matrix (rotations, translations, reflections).
pub fn transform_matrix(body: &Body, m: [[f64; 4]; 4]) -> Result<Body> {
    if m.iter().flatten().any(|x| !x.is_finite()) {
        return Err(KernelError::Invalid("non-finite transform".into()));
    }
    if let Some(placed) = body.mesh_transformed(&m) {
        let history = crate::history::History::identity(&placed);
        return placed.with_history(history, &[body]);
    }
    guard("transform", || {
        let mat = mt::Matrix4::new(
            m[0][0], m[0][1], m[0][2], m[0][3], m[1][0], m[1][1], m[1][2], m[1][3], m[2][0], m[2][1], m[2][2], m[2][3], m[3][0], m[3][1], m[3][2],
            m[3][3],
        );
        // An affine placement retains face order and appearance metadata. Its edges are the
        // body's by identity: a reflection turns the solid inside out, which walks each face's
        // edges the other way and so changes the edge order.
        let sources = body.transformed_sources(m)?;
        let (solid, copies) = placed_topology_mapped(&body.solid, mat)?;
        let mut rec = record::Rebuild::default();
        rec.pieces(&copies);
        for (copy, face) in solid.face_iter().zip(body.solid.face_iter()) {
            rec.whole(copy, face);
        }
        let (faces, edges) = rec.history(&solid, &[&body.solid]).unzip();
        let counts = record::solid_counts(&[&body.solid]);
        // A rigid placement leaves every face's mesh, and so the missing-face count, as it was;
        // a mirrored one turns the solid inside out. Either way the orientation is read from the
        // matrix instead of meshing the whole copy again.
        let placed = record::build_placed(
            solid,
            &counts,
            faces,
            edges.as_ref(),
            det3(&m) < 0.0,
            rigid_placement(&m).then_some(body.coarse_missing).flatten(),
        )?;
        Ok(placed.with_color(body.color()).with_paint(body.paint().cloned()).with_face_sources(sources).inherit_regularization_protection(body))
    })
}

/// `source` moved by `matrix`, keeping its face and edge order (transforms report an identity
/// [`crate::history::History`] on that basis). Built here rather than by generic topology
/// builders: a STEP periodic face may traverse a seam edge twice, and those reject that wire
/// while cloning even though placement preserves its topology.
pub(crate) fn placed_topology(source: &Solid, matrix: mt::Matrix4) -> Result<Solid> {
    placed_topology_mapped(source, matrix).map(|(solid, _)| solid)
}

/// [`placed_topology`], with each edge of `source` (by identity) and its copy.
fn placed_topology_mapped(source: &Solid, matrix: mt::Matrix4) -> Result<(Solid, HashMap<mt::EdgeID, mt::Edge>)> {
    use mt::{Transform, Transformed};
    let mut vertices = HashMap::new();
    let mut edges = HashMap::new();
    let mut shells = Vec::new();
    let mut face_count = 0usize;
    let mut edge_visits = 0usize;
    for shell in source.boundaries() {
        let mut faces = Vec::new();
        for face in shell {
            face_count = face_count.saturating_add(1);
            if face_count > 500_000 {
                return Err(KernelError::Invalid("placement exceeds face work limit".into()));
            }
            let mut wires = Vec::new();
            for wire in face.absolute_boundaries() {
                let mut mapped = Vec::new();
                for edge in wire {
                    edge_visits = edge_visits.saturating_add(1);
                    if edge_visits > 4_000_000 {
                        return Err(KernelError::Invalid("placement exceeds edge work limit".into()));
                    }
                    if let std::collections::hash_map::Entry::Vacant(entry) = edges.entry(edge.id()) {
                        for vertex in [edge.absolute_front(), edge.absolute_back()] {
                            if let std::collections::hash_map::Entry::Vacant(entry) = vertices.entry(vertex.id()) {
                                let point = matrix.transform_point(vertex.point());
                                if !from_p3(point).is_finite() {
                                    return Err(KernelError::Invalid("placement produced nonfinite geometry".into()));
                                }
                                entry.insert(mt::Vertex::new(point));
                            }
                        }
                        let front =
                            vertices.get(&edge.absolute_front().id()).ok_or_else(|| KernelError::Failed("placement vertex missing".into()))?;
                        let back = vertices.get(&edge.absolute_back().id()).ok_or_else(|| KernelError::Failed("placement vertex missing".into()))?;
                        entry.insert(mt::Edge::new_unchecked(front, back, edge.curve().transformed(matrix)));
                    }
                    let copied = edges.get(&edge.id()).ok_or_else(|| KernelError::Failed("placement edge missing".into()))?;
                    mapped.push(if edge.orientation() { copied.clone() } else { copied.inverse() });
                }
                wires.push(mt::Wire::from(mapped));
            }
            // Wire connectivity, seam sharing and orientation are copied exactly;
            // no new topology is introduced by this geometry-only operation.
            let mut copied = mt::Face::new_unchecked(wires, face.surface().transformed(matrix));
            if !face.orientation() {
                copied.invert();
            }
            faces.push(copied);
        }
        shells.push(mt::Shell::from(faces));
    }
    Ok((Solid::new_unchecked(shells), edges))
}

#[cfg(test)]
#[test]
fn affine_placement_keeps_body_and_face_paint_attached() {
    let paint = crate::Paint { opacity: 0.4, faces: vec![crate::FacePaint { face: 0, color: [0.8, 0.2, 0.1], opacity: 0.3 }] };
    let body = crate::box_solid(Vec3::ZERO, Vec3::new(10.0, 20.0, 30.0)).unwrap().with_color(Some([0.2, 0.6, 0.8])).with_paint(Some(paint.clone()));
    let source_faces = body.faces(0.01).unwrap();
    let transform = [[0.0, 2.0, 0.0, 0.0], [-1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.5, 0.0], [30.0, 40.0, 50.0, 1.0]];
    let placed = transform_matrix(&body, transform).unwrap();
    assert_eq!(placed.color(), body.color());
    assert_eq!(placed.paint(), Some(&paint));
    let faces = placed.faces(0.01).unwrap();
    for source in source_faces {
        let target = faces.iter().find(|f| f.index == source.index).unwrap();
        let p = source.centroid;
        assert!(target.centroid.dist(Vec3::new(30.0 - p.y, 40.0 + 2.0 * p.x, 50.0 + 0.5 * p.z)) < 1e-6);
    }
}

/// Rigid transform: rotate about `axis` through `origin` by `angle` radians, then translate.
pub fn transform(body: &Body, translate: Vec3, origin: Vec3, axis: Vec3, angle: f64) -> Result<Body> {
    if !(translate.is_finite() && origin.is_finite() && angle.is_finite()) {
        return Err(KernelError::Invalid("non-finite transform".into()));
    }
    {
        // Same motion as a matrix: rotate about the line through `origin`, then translate.
        let ax = if angle.abs() > 1e-12 { axis.normalized().ok_or_else(|| KernelError::Invalid("rotation axis".into()))? } else { Vec3::Z };
        let (c, s) = (angle.cos(), angle.sin());
        let t = 1.0 - c;
        let r = [
            [t * ax.x * ax.x + c, t * ax.x * ax.y + s * ax.z, t * ax.x * ax.z - s * ax.y],
            [t * ax.x * ax.y - s * ax.z, t * ax.y * ax.y + c, t * ax.y * ax.z + s * ax.x],
            [t * ax.x * ax.z + s * ax.y, t * ax.y * ax.z - s * ax.x, t * ax.z * ax.z + c],
        ];
        let ro = Vec3::new(
            r[0][0] * origin.x + r[1][0] * origin.y + r[2][0] * origin.z,
            r[0][1] * origin.x + r[1][1] * origin.y + r[2][1] * origin.z,
            r[0][2] * origin.x + r[1][2] * origin.y + r[2][2] * origin.z,
        );
        let tr = origin - ro + translate;
        let m = [[r[0][0], r[0][1], r[0][2], 0.0], [r[1][0], r[1][1], r[1][2], 0.0], [r[2][0], r[2][1], r[2][2], 0.0], [tr.x, tr.y, tr.z, 1.0]];
        transform_matrix(body, m)
    }
}

/// Split a body with a plane: the parts on the positive and the negative side of the plane
/// (only the non-empty ones).
pub fn split_by_plane(body: &Body, plane: &solidcraft_geom::Plane) -> Result<Vec<Body>> {
    let mut out = Vec::new();
    for positive in [true, false] {
        out.extend(half_space(body, plane, positive)?.0);
    }
    Ok(out)
}

/// The part of a body on one side of a plane (`None` when nothing is there), and the tool it is
/// cut with: the part is the body's intersection with [`half_box`] (the tool), whose history it
/// carries, over the body and the box. The box's face in the plane, the part's cut face, is its
/// start cap on the positive side and its end cap on the negative side.
pub fn half_space(body: &Body, plane: &solidcraft_geom::Plane, positive: bool) -> Result<(Option<Body>, Body)> {
    // The box's own construction roles describe no face of the design: the cut face has none.
    let tool = half_box(body, plane, positive)?.with_face_sources(None);
    let (part, report) = capture_boolean_diagnostics(|| boolean(body, &tool, BoolOp::Intersect));
    // Faces of the body lying in the plane coincide with the box's cap, which the boolean may
    // only take by nudging the box, by a share of its own size: the cut face then moves off the
    // plane, a different way for each part, and the parts lose the slab between (#1510). Moved
    // off the plane to the side the box leaves out, those faces change nothing of the part, and
    // the cap meets no face. Asked only when the boolean failed or nudged the box: a part made
    // without, its faces' names included, stays as it is.
    let nudged = report.events.iter().any(|e| e.strategy == BooleanStrategy::ToolNudge);
    if (part.is_err() || nudged)
        && let Some(moved) = off_plane(body, plane, positive)
    {
        let (exact, moved_report) = capture_boolean_diagnostics(|| boolean(&moved, &tool, BoolOp::Intersect));
        if let Ok(exact) = exact {
            merge(moved_report);
            // The part's history over [body, box], through the moved body (its faces are the body's).
            let hb = recorded::placed(recorded::through(&moved, 0, recorded::kept(body, 0, 1).as_ref()), 0, 2);
            let ht = recorded::placed(recorded::kept(&tool, 0, 1), 1, 2);
            let counts = record::counts(&[body, &tool]);
            let exact = exact.map(|r| {
                let h = recorded::of_pair(&r, &hb, &ht);
                r.with_checked_history(h, &counts)
            });
            return Ok((exact, tool));
        }
    }
    merge(report);
    Ok((part?, tool))
}

/// `body` with its planar faces lying in `plane` moved off it, a thousandth of the body's size,
/// to the side a split's part on the `positive` side leaves out. `None` when no face lies there
/// or moving them doesn't give a sound body.
pub(crate) fn off_plane(body: &Body, plane: &solidcraft_geom::Plane, positive: bool) -> Option<Body> {
    let m = plane.normal().normalized()?;
    let keep = if positive { m } else { -m };
    let size = body.size();
    let tol = (size * 1e-7).max(1e-9);
    // Lying in the plane means in it, not within the boolean's tolerance of it: a face that close
    // but off it bounds a real sliver, which moving the face would take or add.
    let exact = (size * 1e-9).max(1e-12);
    let delta = size * 1e-3;
    let mut out: Option<Body> = None;
    for n in [keep, -keep] {
        let d = n.dot(plane.origin);
        // The faces `push_faces` takes for this plane (by their surface), which must all lie in
        // it: one only nearly parallel, its origin in the plane, would be moved where it isn't.
        let taken: Vec<&mt::Face> = body
            .solid
            .face_iter()
            .filter(|f| match f.oriented_surface() {
                mt::Surface::Plane(p) => {
                    let q = p.normal();
                    Vec3::new(q.x, q.y, q.z).normalized().is_some_and(|q| q.dot(n) > 1.0 - 1e-9)
                        && (n.dot(from_p3(p.origin())) - d).abs() < tol * 10.0
                }
                _ => false,
            })
            .collect();
        if taken.is_empty() {
            continue;
        }
        // Its vertices and points along its edges (a disc has one vertex).
        let lies = |f: &mt::Face| {
            use mt::{BoundedCurve, ParametricCurve};
            let on = |p: mt::Point3| (n.dot(from_p3(p)) - d).abs() < exact;
            f.vertex_iter().all(|v| on(v.point()))
                && f.edge_iter().all(|e| {
                    let c = e.curve();
                    let (t0, t1) = c.range_tuple();
                    [0.25, 0.5, 0.75].iter().all(|k| on(c.subs(t0 + (t1 - t0) * k)))
                })
        };
        if !taken.iter().all(|f| lies(f)) {
            return None;
        }
        // Along -keep: inwards for a face facing the kept side, outwards for one facing away.
        let shift = if n.dot(keep) > 0.0 { -delta } else { delta };
        let cur = out.as_ref().unwrap_or(body);
        out = Some(crate::coplanar::push_faces(cur, n, d, shift).ok().filter(|b| b.validity().is_empty())?);
    }
    out
}

/// A box covering everything of `body` on one side of a plane, its start cap in the plane.
pub(crate) fn half_box(body: &Body, plane: &solidcraft_geom::Plane, positive: bool) -> Result<Body> {
    body.require_brep("split")?;
    let size = body.size();
    let mut bb = solidcraft_geom::Aabb3::EMPTY;
    for v in body.solid.vertex_iter() {
        bb.add(crate::body::from_p3(v.point()));
    }
    let reach = (bb.diagonal() + plane.origin.dist(bb.center())) * 2.0 + size;
    let c = plane.to_local(bb.center());
    let sq = solidcraft_geom::Region2 {
        outer: solidcraft_geom::Loop2::polygon(&[
            solidcraft_geom::Vec2::new(c.x - reach, c.y - reach),
            solidcraft_geom::Vec2::new(c.x + reach, c.y - reach),
            solidcraft_geom::Vec2::new(c.x + reach, c.y + reach),
            solidcraft_geom::Vec2::new(c.x - reach, c.y + reach),
        ]),
        holes: vec![],
    };
    let (lo, hi) = if positive { (0.0, reach) } else { (-reach, 0.0) };
    crate::build::extrude(plane, std::slice::from_ref(&sq), lo, hi)?.pop().ok_or_else(|| KernelError::Failed("half space".into()))
}

#[cfg(test)]
#[path = "prism_fast_path_tests.rs"]
mod prism_fast_path_tests;

#[cfg(test)]
#[path = "placement_tests.rs"]
mod placement_tests;
