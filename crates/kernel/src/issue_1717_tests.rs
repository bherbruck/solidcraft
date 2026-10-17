//! Issue #1717: a second sphere cut overlapping the first failed ("no result"): an end of an
//! intersection curve was merged into a vertex in one shell's loops but stayed a separate vertex
//! in the other's, so a loop on the tool's face never closed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use solidcraft_geom::Vec3;

use crate::{BoolOp, boolean, box_solid, measure, sphere};

#[test]
fn overlapping_sphere_cuts_on_a_box_edge() {
    let plate = box_solid(Vec3::ZERO, Vec3::new(30.0, 20.0, 5.0)).unwrap();
    let first = boolean(&plate, &sphere(Vec3::new(12.0, 20.0, 0.0), 10.0).unwrap(), BoolOp::Cut).unwrap().expect("first cut");
    // One sphere centred on the edge removes a quarter of the ball between z = ±5: 1375π/6.
    // `measure` sums a tessellation, whose chords leave a little of each sphere's cavity in the
    // part (about 0.06 mm³ here): hence 0.2 mm³, still far below a face kept whole or lost.
    let one = measure(&first).unwrap().volume;
    let expected_one = 3000.0 - 1375.0 * std::f64::consts::PI / 6.0;
    assert!((one - expected_one).abs() < 0.2, "first cut {one}, expected {expected_one}");
    let second = boolean(&first, &sphere(Vec3::new(17.0, 20.0, 0.0), 10.0).unwrap(), BoolOp::Cut).unwrap().expect("second cut");
    let m = measure(&second).unwrap();
    // 3000 less the union of the two quarter slabs: 2·1375π/6 less their lens, 483.551 mm³
    // (the lens's inner integral in closed form, its outer one by Simpson's rule).
    assert!(m.volume.is_finite() && (m.volume - 2043.655).abs() < 0.2, "{m:?}");
    assert!(second.solid.boundaries().iter().all(|s| s.extract_boundaries().is_empty()), "open shell");
}
