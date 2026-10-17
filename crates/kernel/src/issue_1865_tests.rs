//! Bodies that meet along cylindrical faces (#1865). Two cylinders on one axis, of one radius,
//! overlapping along it: where they meet, each body's round side lies in the other's, which the
//! B-rep boolean cannot take, and the exact polygon fallback for such a meeting takes planar
//! faces only. Nothing retried, so the operation was refused although the bodies plainly share
//! a volume. They are faceted and combined instead, which answers the operation at the cost of
//! a result that is flat where the sides curve.

use std::f64::consts::PI;

use solidcraft_geom::Vec3;

use crate::*;

/// The radius both cylinders are turned to, and the heights they stand between: the lower one
/// from z 0 to 30, the upper from z 22 to 46, so they share the 8 mm from 22 to 30.
const R: f64 = 25.6;
const SHARED: f64 = 8.0;

fn lower() -> Body {
    cylinder(Vec3::ZERO, Vec3::Z, R, 30.0).unwrap()
}

fn upper() -> Body {
    cylinder(Vec3::new(0.0, 0.0, 22.0), Vec3::Z, R, 24.0).unwrap()
}

/// The boolean, and how it was made.
fn made(a: &Body, b: &Body, op: BoolOp) -> (Body, BooleanDiagnostics) {
    let (r, how) = capture_boolean_diagnostics(|| boolean(a, b, op));
    (r.unwrap_or_else(|e| panic!("{op:?}: {e}")).expect("a result"), how)
}

fn used(how: &BooleanDiagnostics, strategy: BooleanStrategy) -> bool {
    how.events.iter().any(|e| e.strategy == strategy)
}

/// The volume of a cylinder of radius [`R`] standing `height` tall.
fn disc(height: f64) -> f64 {
    PI * R * R * height
}

/// The faceted result is a prism on a polygon inscribed in the circle, so it holds a little
/// less than the round body does: a thousandth of it is room enough for that and for the
/// measuring mesh's own chords.
fn within(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() < 2e-3 * want, "{what}: {got} mm³, want {want} mm³");
}

#[test]
fn cylinders_meeting_along_their_sides_can_be_intersected() {
    let (body, how) = made(&lower(), &upper(), BoolOp::Intersect);
    // What they share is the 8 mm of one cylinder that both stand through.
    within(measure(&body).unwrap().volume, disc(SHARED), "the shared part");
    assert_eq!(body.lumps().unwrap().len(), 1, "one piece");
    assert!(body.validity().is_empty(), "{:?}", body.validity());
    // The route that answered it says so, and says the result is flat where the sides curve.
    assert!(used(&how, BooleanStrategy::FacetedFallback), "{how:?}");
    assert!(how.warning().is_some(), "a result that is not the exact one is warned about");
}

#[test]
fn a_bored_cylinder_joins_the_one_standing_on_it() {
    // The issue's own operation. A bore across the lower cylinder, well below where the two
    // meet, leaves it no prism along the axis, so the 2D route that takes the plain pair
    // cannot be asked and the join falls through to the same refusal.
    let drill = cylinder(Vec3::new(-40.0, 0.0, 10.0), Vec3::X, 4.0, 80.0).unwrap();
    let bored = boolean(&lower(), &drill, BoolOp::Cut).unwrap().unwrap();
    let (body, how) = made(&bored, &upper(), BoolOp::Union);
    // The join adds the 16 mm of the upper cylinder standing clear of the lower one. The bore
    // lies 8 mm below where they meet, so the join neither reaches it nor fills it.
    within(measure(&body).unwrap().volume, measure(&bored).unwrap().volume + disc(16.0), "the joined body");
    assert_eq!(body.lumps().unwrap().len(), 1, "one piece");
    assert!(body.validity().is_empty(), "{:?}", body.validity());
    assert!(used(&how, BooleanStrategy::FacetedFallback), "{how:?}");
}

#[test]
fn the_join_of_those_cylinders_is_still_taken_exactly() {
    // The join of the same two bodies has always worked, and it is not to start coming back
    // faceted: the faceted route is a last resort, after every exact one has failed.
    let (body, how) = made(&lower(), &upper(), BoolOp::Union);
    within(measure(&body).unwrap().volume, disc(46.0), "the joined body");
    assert!(!used(&how, BooleanStrategy::FacetedFallback), "{how:?}");
}

#[test]
fn boxes_that_overlap_still_join_without_facets() {
    // The control from the issue: a 20 mm cube and a bar through one of its sides, sharing
    // 10 x 5 x 20. Faces that are all planar have an exact route of their own and keep it.
    let cube = box_solid(Vec3::ZERO, Vec3::new(20.0, 20.0, 20.0)).unwrap();
    let bar = box_solid(Vec3::new(10.0, -5.0, 0.0), Vec3::new(30.0, 5.0, 20.0)).unwrap();
    let (body, how) = made(&cube, &bar, BoolOp::Union);
    let want = 8000.0 + 4000.0 - 1000.0;
    let got = measure(&body).unwrap().volume;
    assert!((got - want).abs() < 1e-6, "the joined boxes: {got} mm³, want {want} mm³");
    assert_eq!(body.lumps().unwrap().len(), 1, "one piece");
    assert!(!used(&how, BooleanStrategy::FacetedFallback), "{how:?}");
}
