//! Revolves of profiles with an edge on the axis that come back to it along a curve or a line
//! (#1853): a half slot about its centreline, a quarter disc and a bullet whose arc ends on the
//! axis, a drill point whose tip does. Their volumes are analytic, and a half slot revolved about
//! a centreline lying in a face of a plate, as a cut, takes what it should.

use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2, Vec3};
use solidcraft_kernel::{BoolOp, boolean, box_solid, measure, revolve};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

const R: f64 = 3.0;

fn line(a: (f64, f64), b: (f64, f64)) -> Seg2 {
    Seg2::Line { a: Vec2::new(a.0, a.1), b: Vec2::new(b.0, b.1) }
}

/// A quarter of a circle of radius `R` about `c`, counter-clockwise from `start`.
fn quarter(c: (f64, f64), start: f64) -> Seg2 {
    Seg2::Arc { center: Vec2::new(c.0, c.1), radius: R, start, sweep: FRAC_PI_2 }
}

fn region(segs: Vec<Seg2>) -> Region2 {
    Region2 { outer: Loop2 { segs }, holes: vec![] }
}

/// Half a slot (radius `R`, centres 14 apart) cut along its centreline on the x axis.
fn half_slot() -> Region2 {
    region(vec![line((0.0, 0.0), (20.0, 0.0)), quarter((17.0, 0.0), 0.0), line((17.0, R), (3.0, R)), quarter((3.0, 0.0), FRAC_PI_2)])
}

#[test]
fn revolves_that_end_on_the_axis_along_a_curve_have_their_volume() {
    let sphere = 4.0 / 3.0 * PI * R.powi(3);
    let cylinder = PI * R * R * 14.0;
    let shapes = [
        ("half slot", half_slot(), cylinder + sphere),
        ("quarter disc", region(vec![line((-R, 0.0), (0.0, 0.0)), line((0.0, 0.0), (0.0, R)), quarter((0.0, 0.0), FRAC_PI_2)]), sphere / 2.0),
        (
            "bullet",
            region(vec![line((0.0, 0.0), (17.0, 0.0)), line((17.0, 0.0), (17.0, R)), line((17.0, R), (3.0, R)), quarter((3.0, 0.0), FRAC_PI_2)]),
            cylinder + sphere / 2.0,
        ),
        // A drill point: a line coming back to the axis (a full turn keeps truck's cone).
        (
            "drill point",
            region(vec![line((0.0, 0.0), (20.0, 0.0)), line((20.0, 0.0), (20.0, R)), line((20.0, R), (3.0, R)), line((3.0, R), (0.0, 0.0))]),
            PI * R * R * 17.0 + PI * R * R * 3.0 / 3.0,
        ),
    ];
    // The plane through the origin, and one moved and turned away from it.
    let planes = [Plane::XY, Plane { origin: Vec3::new(5.0, 7.0, 10.0), x: Vec3::Y, y: Vec3::Z }];
    for (name, r, full) in &shapes {
        for plane in &planes {
            for turn in [FRAC_PI_2, PI, -PI, TAU] {
                let body = revolve(plane, std::slice::from_ref(r), Vec2::ZERO, Vec2::new(1.0, 0.0), turn).unwrap().pop().unwrap();
                assert!(body.validity().is_empty(), "{name}, {turn}: {:?}", body.validity());
                let want = full * turn.abs() / TAU;
                let got = measure(&body).unwrap().volume;
                assert!((got - want).abs() < 1e-3 * want, "{name} turned {turn} on {:?}: {got} instead of {want}", plane.origin);
            }
        }
    }
}

#[test]
fn a_half_slot_revolved_about_a_centreline_in_a_face_cuts_it() {
    let plate = box_solid(Vec3::ZERO, Vec3::new(40.0, 20.0, 10.0)).unwrap();
    let whole = measure(&plate).unwrap().volume;
    // The half slot from x = 10 to 30 with its centreline on y = 10 of the top face; the plate
    // holds the half of the capsule below it.
    let inside = (PI * R * R * 14.0 + 4.0 / 3.0 * PI * R.powi(3)) / 2.0;
    let r = half_slot();
    // The sketch plane facing out of the plate and into it (its y running the other way).
    for flip in [1.0, -1.0] {
        let plane = Plane { origin: Vec3::new(10.0, 10.0, 10.0), x: Vec3::X, y: Vec3::Y * flip };
        for turn in [PI, -PI, TAU] {
            let tool = revolve(&plane, std::slice::from_ref(&r), Vec2::ZERO, Vec2::new(1.0, 0.0), turn).unwrap().pop().unwrap();
            // A half turn into the plate, or a full one, cuts a groove; a half turn out of it misses.
            let into = turn.abs() > PI || tool.tessellate(0.01).unwrap().bounds().min.z < 9.0;
            let cut = boolean(&plate, &tool, BoolOp::Cut).unwrap().unwrap();
            assert!(cut.validity().is_empty(), "flip {flip}, {turn}: {:?}", cut.validity());
            let removed = whole - measure(&cut).unwrap().volume;
            let want = if into { inside } else { 0.0 };
            assert!((removed - want).abs() < 1e-3 * inside, "flip {flip}, turned {turn}: removed {removed} instead of {want}");
        }
    }
}
