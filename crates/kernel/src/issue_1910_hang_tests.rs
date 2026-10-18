//! What a boolean costs on a body the faceted route made (#1910).
//!
//! The route flattens the faces where two bodies curve into each other, so it returns a body of
//! facets where the operands had a handful of faces. Two things used to follow from that, and
//! together they made a later boolean on such a body cost minutes:
//!
//! - the exact polygon fallback refuses a body whose planar faces do not mesh flat, and it
//!   refused it in the same words it refuses a body whose faces genuinely curve. The faceted
//!   route chose itself by matching those words, so it read its own output as a curved contact
//!   and faceted it again, at hundreds of times the facets;
//! - the retries underneath a boolean each run every route above them again, so one operation
//!   asked the last resort seven times over.
//!
//! The first is now decided by a reason the fallback returns ([`crate::polybool::NotTaken`])
//! rather than by the words of an error, and the last resort is asked by the operation rather
//! than by the retries underneath it.
//!
//! A note on what these tests can and cannot show today. Since #1894 and #2008 made coincident
//! cylinders join exactly, the heaviest body the route still produces is the intersection
//! below, of a few hundred facets, and its facets do mesh flat - so the fallback takes it, and
//! the condition the first guard watches for is not reachable from any pair these tests can
//! build. What is asserted here is the behaviour that condition exists to protect: a body the
//! route made is not sent back to the route, and one operation asks it once.

use solidcraft_geom::Vec3;

use crate::*;

const R: f64 = 25.6;

/// The pair whose intersection no exact route takes: two cylinders on one axis, of one radius,
/// sharing the 8 mm from z 22 to 30 (#1865).
fn coaxial_pair() -> (Body, Body) {
    (cylinder(Vec3::ZERO, Vec3::Z, R, 30.0).unwrap(), cylinder(Vec3::new(0.0, 0.0, 22.0), Vec3::Z, R, 24.0).unwrap())
}

/// The body the faceted route leaves: the 8 mm the two share, its round side a ring of facets.
fn faceted_body() -> Body {
    let (lower, upper) = coaxial_pair();
    let body = boolean(&lower, &upper, BoolOp::Intersect).unwrap().unwrap();
    assert!(body.face_count() > 100, "the faceted route should leave a body of facets, got {} faces", body.face_count());
    body
}

#[test]
fn the_faceted_route_is_asked_by_the_operation_not_by_its_retries() {
    // The operation asks the last resort on its own behalf, once. The retries nested under it
    // do not ask again: their tool differs from the operation's by a screw motion of two parts
    // in a million, which decides nothing the facets decide, and asking cost seven times over.
    let (lower, upper) = coaxial_pair();
    let (r, how) = capture_boolean_diagnostics(|| boolean(&lower, &upper, BoolOp::Intersect));
    r.unwrap().expect("a result");
    let asked = how.events.iter().filter(|e| e.strategy == BooleanStrategy::FacetedFallback).count();
    assert_eq!(asked, 1, "the faceted route answered {asked} times for one operation: {how:?}");
    // And it is still reported as what it is: a result that is flat where the faces curve.
    assert!(how.warning().is_some(), "a result that is not the exact one is warned about");
}

#[test]
fn a_body_the_faceted_route_made_is_not_faceted_again() {
    // A third cylinder overlapping the faceted body. Faceting it again answers nothing - it is
    // already flat where it curved - and comes to hundreds of times the facets it was made
    // from, so each pass costs more than the last. Today the fallback takes this body (its
    // facets are planes and they mesh flat), which is the best outcome and the one asserted;
    // what must never happen either way is that it goes back to the facets.
    let faceted = faceted_body();
    let third = cylinder(Vec3::new(0.0, 0.0, 26.0), Vec3::Z, R, 20.0).unwrap();
    let (r, how) = capture_boolean_diagnostics(|| boolean(&faceted, &third, BoolOp::Union));
    let body = r.unwrap_or_else(|e| panic!("a boolean on a faceted body: {e}")).expect("a result");
    assert!(!how.events.iter().any(|e| e.strategy == BooleanStrategy::FacetedFallback), "a body the faceted route made was faceted again: {how:?}");
    assert_eq!(body.lumps().unwrap().len(), 1, "one piece");
    assert!(body.validity().is_empty(), "{:?}", body.validity());
}

#[test]
fn a_face_flush_on_a_faceted_body_keeps_its_retry() {
    // The nudge is skipped for a body whose planar faces do not mesh flat, and a faceted body
    // is a candidate for that. Its caps are flat whatever the sides do, and a face set flush on
    // one is exactly the coincident-face contact the retries underneath a boolean exist for.
    // Skipping a retry for the body as a whole must not cost that contact its answer.
    let faceted = faceted_body();
    let alone = measure(&faceted).unwrap().volume;
    // A block standing on the top cap (z 30), well inside its rim, so the two meet over the
    // whole of the block's underside and nowhere else.
    let block = box_solid(Vec3::new(-10.0, -10.0, 30.0), Vec3::new(10.0, 10.0, 40.0)).unwrap();
    let joined =
        boolean(&faceted, &block, BoolOp::Union).unwrap_or_else(|e| panic!("a block flush on the cap lost its answer: {e}")).expect("a result");
    // Flush, not overlapping: the join is the two volumes, and one body.
    let want = alone + 20.0 * 20.0 * 10.0;
    let got = measure(&joined).unwrap().volume;
    assert!((got - want).abs() < 1e-6 * want, "the joined body: {got} mm³, want {want} mm³");
    assert_eq!(joined.lumps().unwrap().len(), 1, "the block and the body it stands on are one piece");
}
