//! A shallow circular cut after a pocket (#1765): repeated empty numerical answers must not
//! stop the later geometric strategies. Inject the numerical failure on a real small part;
//! the exact rounding and ray classification of the original design are not available.

use super::*;
use crate::{box_solid, cylinder, fillet, measure, sphere};
use std::cell::Cell;
use std::f64::consts::PI;

#[test]
fn shallow_cut_after_a_pocket_recovers_from_empty_numerical_attempts() {
    let plate = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
    let plate = fillet(&plate, &[Vec3::new(20.0, 0.0, 10.0)], 2.0).unwrap();
    let pocket = box_solid(Vec3::new(5.0, 15.0, 9.25), Vec3::new(35.0, 25.0, 10.25)).unwrap();
    let target = boolean(&plate, &pocket, BoolOp::Cut).unwrap().unwrap();
    let tool = cylinder(Vec3::new(12.0, 15.0, 8.5), Vec3::Z, 5.0, 1.75).unwrap();
    let attempts = Cell::new(0);
    let empty = |_: &Body, _: &Body, _: BoolOp, _: Option<mt::Vector3>, _: f64| {
        attempts.set(attempts.get() + 1);
        Ok((Solid::new_unchecked(Vec::new()), None))
    };
    let (result, diagnostics) =
        capture_boolean_diagnostics(|| boolean_via(&target, &tool, BoolOp::Cut, &|a, b, op| boolean_whole_with(a, b, op, false, &empty)));
    assert!(attempts.get() >= 2, "the first two numerical tries must really have failed");
    let cut = result.unwrap().expect("a shallow cut leaves the plate");
    assert!(cut.validity().is_empty(), "{:?}", cut.validity());
    assert_eq!(cut.lumps().unwrap().len(), 1);
    // Half of the circle overlaps the existing 0.75 mm pocket.
    let expected = 40.0 * 30.0 * 10.0 - (4.0 - PI) * 40.0 - 30.0 * 10.0 * 0.75 - PI * 25.0 * (1.5 - 0.75 / 2.0);
    let actual = measure(&cut).unwrap().volume;
    assert!((actual - expected).abs() < 0.02, "{actual} vs {expected}");
    assert!(diagnostics.events.iter().all(|e| e.strategy != BooleanStrategy::RefinedTolerance));
}

#[test]
fn unrecovered_empty_attempts_keep_the_outer_guard_refusal() {
    let target = sphere(Vec3::ZERO, 10.0).unwrap();
    let tool = sphere(Vec3::new(8.0, 0.0, 0.0), 5.0).unwrap();
    let empty = |_: &Body, _: &Body, _: BoolOp, _: Option<mt::Vector3>, _: f64| Ok((Solid::new_unchecked(Vec::new()), None));
    let result = boolean_via(&target, &tool, BoolOp::Cut, &|a, b, op| boolean_whole_with(a, b, op, false, &empty));
    let error = result.unwrap_err();
    assert!(error.to_string().contains("it came out empty"), "{error}");
}
