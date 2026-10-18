//! #1955: the frame a fillet of all sixteen vertical edges refuses at 5 mm with "the largest
//! radius that fits is 4 mm", which then fails at 4 mm too. The frame: a square 52 × 52 with
//! arcs of radius 24√2 bulging out through the middles of its sides (the circle passes through
//! the corners of the 48 × 48 square hole, meeting the square's sides at x = y = ±√476 ≈
//! 21.8174), 10 thick, with the square hole cut through. Whatever radius a refusal's note names,
//! that radius must build.

use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2, Vec3};

use crate::build::extrude;

/// The frame of the issue, top face at z = 0, 10 thick, as the sketch extrudes it.
fn frame() -> crate::Body {
    let r = 24.0 * std::f64::consts::SQRT_2;
    let j = (r * r - 26.0 * 26.0).sqrt(); // The arcs meet the sides at (±j, ±26) and (±26, ±j).
    let (t, u) = (f64::atan2(j, 26.0), f64::atan2(26.0, j)); // ≈ 40° and ≈ 50°.
    let line = |a: Vec2, b: Vec2| Seg2::Line { a, b };
    let arc = |start: f64, sweep: f64| Seg2::Arc { center: Vec2::ZERO, radius: r, start, sweep };
    let pi = std::f64::consts::PI;
    // Counter-clockwise from the top-right corner: each side's middle bulges out along an arc
    // of the circle that passes through the hole's corners; the square's corners stay sharp.
    let outer = Loop2 {
        segs: vec![
            line(Vec2::new(26.0, 26.0), Vec2::new(j, 26.0)),
            arc(u, pi - 2.0 * u),
            line(Vec2::new(-j, 26.0), Vec2::new(-26.0, 26.0)),
            line(Vec2::new(-26.0, 26.0), Vec2::new(-26.0, j)),
            arc(pi - t, 2.0 * t),
            line(Vec2::new(-26.0, -j), Vec2::new(-26.0, -26.0)),
            line(Vec2::new(-26.0, -26.0), Vec2::new(-j, -26.0)),
            arc(u - pi, pi - 2.0 * u),
            line(Vec2::new(j, -26.0), Vec2::new(26.0, -26.0)),
            line(Vec2::new(26.0, -26.0), Vec2::new(26.0, -j)),
            arc(-t, 2.0 * t),
            line(Vec2::new(26.0, j), Vec2::new(26.0, 26.0)),
        ],
    };
    let hole = Loop2::polygon(&[Vec2::new(24.0, 24.0), Vec2::new(24.0, -24.0), Vec2::new(-24.0, -24.0), Vec2::new(-24.0, 24.0)]);
    extrude(&Plane::XY, &[Region2 { outer, holes: vec![hole] }], -10.0, 0.0).unwrap().remove(0)
}

/// The issue's sixteen points: the frame's every vertical edge, on its top face.
#[rustfmt::skip]
const EDGES: [Vec3; 16] = [
    Vec3::new(21.81742422927142, 25.999999999999993, 0.0),
    Vec3::new(26.0, 21.81742422927142, 0.0),
    Vec3::new(26.0, 26.0, 0.0),
    Vec3::new(-21.81742422927141, 26.0, 0.0),
    Vec3::new(-26.0, 21.817424229271424, 0.0),
    Vec3::new(-26.0, 26.0, 0.0),
    Vec3::new(-21.817424229271424, -25.999999999999993, 0.0),
    Vec3::new(-26.0, -21.817424229271392, 0.0),
    Vec3::new(-26.0, -26.0, 0.0),
    Vec3::new(21.817424229271374, -26.0, 0.0),
    Vec3::new(25.999999999999993, -21.817424229271428, 0.0),
    Vec3::new(26.0, -26.0, 0.0),
    Vec3::new(-24.0, 24.0, 0.0),
    Vec3::new(-24.0, -24.0, 0.0),
    Vec3::new(24.0, -24.0, 0.0),
    Vec3::new(24.0, 24.0, 0.0),
];

/// The radius a refusal's note names, if it names one.
fn named_radius(m: &str) -> Option<f64> {
    m.split("the largest radius that fits is ").nth(1)?.trim_end_matches(" mm").parse().ok()
}

/// Every radius the frame refuses — this build refuses them all, at 1 mm as at 5 mm — may only
/// name a largest radius that itself builds. Before the fix the model named 1.895 mm (the issue
/// was told 4 mm), which failed like the rest.
#[test]
fn the_note_names_only_a_radius_that_builds() {
    let f = frame();
    for r in [1.0, 2.0, 3.0, 4.0, 5.0] {
        let Err(e) = crate::fillet(&f, &EDGES, r) else { continue };
        let m = e.to_string();
        if let Some(v) = named_radius(&m) {
            assert!(crate::fillet(&f, &EDGES, v).is_ok(), "r = {r} names {v} mm, which does not build: {m}");
        }
    }
}

/// The corner between two earlier rounds: the build refuses it at every radius while the model
/// still answers 0.82 mm at a 4 mm ask. The same property holds there.
#[test]
fn a_corner_between_rounds_names_only_a_radius_that_builds() {
    let mut cur = frame();
    for p in [&EDGES[0], &EDGES[1]] {
        cur = crate::fillet(&cur, std::slice::from_ref(p), 1.0).unwrap();
    }
    for r in [1.0, 2.0, 4.0] {
        let Err(e) = crate::fillet(&cur, std::slice::from_ref(&EDGES[2]), r) else { continue };
        let m = e.to_string();
        if let Some(v) = named_radius(&m) {
            assert!(crate::fillet(&cur, std::slice::from_ref(&EDGES[2]), v).is_ok(), "r = {r} names {v} mm, which does not build: {m}");
        }
    }
}
