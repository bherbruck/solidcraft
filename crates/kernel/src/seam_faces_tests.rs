//! Round faces read from STEP: a whole turn bounded by one wire that runs along its seam twice.
//! A boolean whose intersection loop lies wholly inside such a face takes it in two halves
//! (exactly), where it used to end in a nudged tool or fail.
use crate::*;
use solidcraft_geom::{Loop2, Plane, Region2, Vec2, Vec3};
use std::f64::consts::PI;

const R: f64 = 4.5;

fn volume(b: &Body) -> f64 {
    measure(b).unwrap().volume
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-12)
}

/// `body` written to STEP and read back: its round faces come back whole, as other systems
/// write them.
fn through_step(body: &Body) -> Body {
    step_import(&step_export(&[body], "t").unwrap()).unwrap().bodies.into_iter().next().unwrap().body
}

/// A pin of radius `R` along X, 40 long, centred on the origin.
fn pin() -> Body {
    cylinder(Vec3::new(-20.0, 0.0, 0.0), Vec3::X, R, 40.0).unwrap()
}

/// A block pressed into the top of the pin's side: 10 long, 6 wide, from 2 above the axis up.
fn block() -> Body {
    box_solid(Vec3::new(-5.0, -3.0, 2.0), Vec3::new(5.0, 3.0, 9.0)).unwrap()
}

/// The part of the pin inside [`block`]: the circle's segment above 2 and within 3 of the
/// middle, 10 long.
fn pressed_volume() -> f64 {
    let segment = 3.0 * (R * R - 9.0).sqrt() + R * R * (3.0 / R).asin() - 6.0 * 2.0;
    10.0 * segment
}

/// The boolean, which must be exact: no retry that moves the tool or ignores part of where the
/// bodies meet.
fn exact(a: &Body, b: &Body, op: BoolOp) -> Option<Body> {
    let (r, report) = capture_boolean_diagnostics(|| boolean(a, b, op));
    assert!(report.warning().is_none(), "{op:?}: {:?}", report.warning());
    r.unwrap()
}

#[test]
fn a_pin_read_from_step_has_one_face_along_its_seam() {
    let read = through_step(&pin());
    assert_eq!(read.face_count(), 3);
    assert!(super::needed(&read, &block()));
    assert!(!super::needed(&pin(), &block()));
    // Split in two halves, with the same solid.
    let (halved, history) = super::halved(&read).unwrap();
    assert!(super::next_split(&halved).is_none() && halved.face_count() > 3);
    assert!(history.is_some());
    assert!(rel(volume(&halved), PI * R * R * 40.0) < 1e-3);
}

#[test]
fn a_block_pressed_into_a_read_pins_side_is_intersected_and_cut_exactly() {
    let (read, block) = (through_step(&pin()), block());
    let want = pressed_volume();
    let shared = exact(&read, &block, BoolOp::Intersect).unwrap();
    assert!(rel(volume(&shared), want) < 1e-3, "{} {want}", volume(&shared));
    let dented = exact(&read, &block, BoolOp::Cut).unwrap();
    assert!(rel(volume(&dented), PI * R * R * 40.0 - want) < 1e-3, "{}", volume(&dented));
    // The operands' faces carry over: the dented pin's round face is still the pin's.
    assert!(dented.history().is_some());
}

#[test]
fn a_ball_pressed_into_a_read_pins_side_matches_the_built_pin() {
    let ball = sphere(Vec3::new(3.0, 1.0, 5.5), 3.0).unwrap();
    let built = volume(&boolean(&pin(), &ball, BoolOp::Intersect).unwrap().unwrap());
    let read = volume(&exact(&through_step(&pin()), &ball, BoolOp::Intersect).unwrap());
    assert!(rel(read, built) < 1e-3, "{read} {built}");
}

/// A cone's side read from STEP is one face along its seam as well.
#[test]
fn a_block_pressed_into_a_read_cones_side_matches_the_built_cone() {
    let outline =
        Region2 { outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(8.0, 0.0), Vec2::new(5.0, 20.0), Vec2::new(0.0, 20.0)]), holes: vec![] };
    let cone = revolve(&Plane::YZ, &[outline], Vec2::ZERO, Vec2::Y, 2.0 * PI).unwrap().pop().unwrap();
    let read = through_step(&cone);
    assert!(super::needed(&read, &cone));
    let block = box_solid(Vec3::new(-2.0, 4.0, 6.0), Vec3::new(2.0, 12.0, 12.0)).unwrap();
    let built = volume(&boolean(&cone, &block, BoolOp::Intersect).unwrap().unwrap());
    let read = volume(&exact(&read, &block, BoolOp::Intersect).unwrap());
    assert!(rel(read, built) < 1e-3, "{read} {built}");
}
