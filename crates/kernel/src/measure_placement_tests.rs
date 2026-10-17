//! A body's measured volume does not depend on where it sits: the same revolved ring of cones
//! and cylinders, moved far from the origin, measures what it measures at the origin.
use crate::*;
use solidcraft_geom::{Loop2, Plane, Region2, Vec2, Vec3};
use std::f64::consts::{PI, TAU};

/// Ring cross-section in (radius, height): cones top and bottom, cylinders inside and out.
const PROFILE: [(f64, f64); 6] = [(5.0, 0.0), (9.0, 0.0), (11.0, 2.0), (11.0, 6.0), (8.0, 8.0), (5.0, 8.0)];

/// Pappus: 2π × the section's first moment about the axis.
fn ring_volume() -> f64 {
    let mut moment6 = 0.0;
    for (i, &(x0, y0)) in PROFILE.iter().enumerate() {
        let (x1, y1) = PROFILE[(i + 1) % PROFILE.len()];
        moment6 += (x0 + x1) * (x0 * y1 - x1 * y0);
    }
    TAU * moment6 / 6.0
}

fn revolved(plane: &Plane, pts: &[Vec2]) -> Body {
    let region = Region2 { outer: Loop2::polygon(pts), holes: vec![] };
    revolve(plane, &[region], Vec2::ZERO, Vec2::Y, TAU).unwrap().pop().unwrap()
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs()
}

#[test]
fn ring_volume_does_not_depend_on_placement() {
    let pts: Vec<Vec2> = PROFILE.iter().map(|&(r, z)| Vec2::new(r, z)).collect();
    let base = revolved(&Plane::XZ, &pts);
    let at_origin = measure(&base).unwrap();
    assert!(rel(at_origin.volume, ring_volume()) < 1e-4, "{} vs {}", at_origin.volume, ring_volume());
    for d in [100.0, 1000.0, 10000.0] {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(1.0, 1.0, 1.0)] {
            let m = measure(&transform(&base, axis * d, Vec3::ZERO, Vec3::Z, 0.0).unwrap()).unwrap();
            assert!(rel(m.volume, at_origin.volume) < 1e-6, "moved {d} along {axis:?}: {} vs {}", m.volume, at_origin.volume);
            let off = m.centroid - at_origin.centroid - axis * d;
            assert!(off.len() < 1e-6 * d, "centroid moved {d} along {axis:?}: off by {off:?}");
        }
    }
}

#[test]
fn cylinder_and_cone_volumes_far_from_the_origin() {
    let (r, h) = (7.0, 12.0);
    // A frustum, radii 7 and 3, height 10: πh/3 (R² + Rr + r²).
    let frustum = [Vec2::new(0.0, 0.0), Vec2::new(7.0, 0.0), Vec2::new(3.0, 10.0), Vec2::new(0.0, 10.0)];
    let cone_volume = PI * 10.0 / 3.0 * (49.0 + 21.0 + 9.0);
    for at in [Vec3::ZERO, Vec3::new(5000.0, -3000.0, 8000.0)] {
        let cyl = measure(&cylinder(at, Vec3::Z, r, h).unwrap()).unwrap();
        assert!(rel(cyl.volume, PI * r * r * h) < 5e-4, "cylinder at {at:?}: {}", cyl.volume);
        let cone = measure(&revolved(&Plane { origin: at, ..Plane::XZ }, &frustum)).unwrap();
        assert!(rel(cone.volume, cone_volume) < 5e-4, "frustum at {at:?}: {} vs {cone_volume}", cone.volume);
    }
}
