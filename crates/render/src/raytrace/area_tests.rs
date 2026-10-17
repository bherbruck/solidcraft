use super::*;
use std::f64::consts::PI;

fn rectangle(size: f64, z: f64) -> RectangleLight {
    RectangleLight::new(Vec3::new(0.0, 0.0, z), Vec3::X * size, -Vec3::Y * size, Vec3::new(1.0, 0.5, 0.25), 2.0).unwrap()
}

#[test]
fn centre_sample_matches_area_inverse_square_and_both_cosines_analytically() {
    let light = rectangle(2.0, 10.0);
    assert_eq!(light.area_mm2(), 4.0);
    assert_eq!(light.normal(), -Vec3::Z);
    let sample = light.sample(Vec3::ZERO, 0, 0, 0, 1, 0).unwrap().unwrap();
    assert_eq!(sample.position(), Vec3::new(0.0, 0.0, 10.0));
    assert_eq!(sample.direction(), Vec3::Z);
    assert_eq!(sample.distance_mm(), 10.0);
    assert!((sample.incident() - Vec3::new(0.08, 0.04, 0.02)).len() < 1e-14);
    let surface = OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 0.0, 1.0, 1.0).unwrap();
    assert!((sample.shade(surface, Vec3::Z, Vec3::Z).unwrap() - Vec3::new(0.08, 0.04, 0.02) / PI).len() < 1e-14);
    // Tilting the emitter 60 degrees halves projected area at the centre sample.
    let tilted = RectangleLight::new(
        Vec3::new(0.0, 0.0, 10.0),
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(0.0, -1.0, 3.0_f64.sqrt()),
        Vec3::new(1.0, 0.5, 0.25),
        2.0,
    )
    .unwrap();
    assert!((tilted.sample(Vec3::ZERO, 0, 0, 0, 1, 0).unwrap().unwrap().incident() - sample.incident() * 0.5).len() < 1e-14);
    let far = rectangle(2.0, 20.0).sample(Vec3::ZERO, 0, 0, 0, 1, 0).unwrap().unwrap();
    assert!((far.incident() - sample.incident() * 0.25).len() < 1e-14);
}

#[test]
fn rectangle_emits_only_from_its_oriented_side() {
    let light = rectangle(2.0, 10.0);
    assert!(light.sample(Vec3::new(0.0, 0.0, 20.0), 0, 0, 0, 1, 0).unwrap().is_none());
    assert!(light.sample(Vec3::new(20.0, 0.0, 10.0), 0, 0, 0, 1, 0).unwrap().is_none());
    let dark = RectangleLight::new(Vec3::Z * 10.0, Vec3::X, -Vec3::Y, Vec3::ZERO, 1.0).unwrap();
    assert!(dark.sample(Vec3::ZERO, 0, 0, 0, 1, 0).unwrap().is_none());
}

#[test]
fn uniform_scene_scale_preserves_the_area_over_distance_squared_response() {
    let reference = rectangle(2.0, 10.0).sample(Vec3::ZERO, 19, 0, 3, 16, 7).unwrap().unwrap();
    for scale in [0.01, 1000.0] {
        let scaled = rectangle(2.0 * scale, 10.0 * scale).sample(Vec3::ZERO, 19, 0, 3, 16, 7).unwrap().unwrap();
        assert!((scaled.incident() - reference.incident()).len() < 1e-14);
        assert!((scaled.direction() - reference.direction()).len() < 1e-14);
        assert!((scaled.distance_mm() / scale - reference.distance_mm()).abs() < 1e-12);
    }
}

#[test]
fn sample_pattern_covers_each_stratum_is_repeatable_and_depends_on_pixel_and_seed() {
    let light = rectangle(8.0, 10.0);
    for count in [4, 16, 64] {
        let side = (count as f64).sqrt() as u32;
        for index in 0..count {
            let a = light.sample(Vec3::ZERO, 7, 1, index, count, 11).unwrap().unwrap();
            let b = light.sample(Vec3::ZERO, 7, 1, index, count, 11).unwrap().unwrap();
            assert_eq!(a, b);
            let u = a.position().x / 8.0 + 0.5;
            let v = -a.position().y / 8.0 + 0.5;
            assert_eq!((u * side as f64).floor() as u32, index % side);
            assert_eq!((v * side as f64).floor() as u32, index / side);
        }
    }
    let original = light.sample(Vec3::ZERO, 7, 1, 0, 16, 11).unwrap();
    assert_ne!(original, light.sample(Vec3::ZERO, 8, 1, 0, 16, 11).unwrap());
    assert_ne!(original, light.sample(Vec3::ZERO, 7, 1, 0, 16, 12).unwrap());
}

#[test]
fn finite_shadow_ray_excludes_occluders_beyond_the_emitter() {
    let sample = rectangle(2.0, 10.0).sample(Vec3::ZERO, 0, 0, 0, 1, 0).unwrap().unwrap();
    let ray = sample.shadow_ray(Vec3::ZERO).unwrap();
    assert_eq!(ray.interval(), [f64::from_bits(1), 10.0_f64.next_down()]);
    let near_origin = sample.shadow_ray(Vec3::new(0.0, 0.0, f64::from_bits(1))).unwrap();
    let receiver = super::super::Triangle::new([Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)]).unwrap();
    assert!(receiver.intersect(&near_origin).is_none());
    let triangle = |z| super::super::Triangle::new([Vec3::new(-1.0, -1.0, z), Vec3::new(1.0, -1.0, z), Vec3::new(0.0, 1.0, z)]).unwrap();
    assert!(triangle(5.0).intersect(&ray).is_some());
    assert!(triangle(11.0).intersect(&ray).is_none());
    assert!(sample.shadow_ray(sample.position()).is_err());
    assert!(sample.shadow_ray(Vec3::new(0.0, 0.0, 11.0)).is_err());
    // Representable large-coordinate gaps: offset can overtake a nearby emitter.
    let receiver = Vec3::new(0.0, 0.0, 9e11);
    let gap = receiver.z.next_up() - receiver.z;
    let close = rectangle(1.0, receiver.z + 4.0 * gap).sample(receiver, 0, 0, 0, 1, 0).unwrap().unwrap();
    assert!(close.shadow_ray(Vec3::new(0.0, 0.0, receiver.z + 14.0 * gap)).is_err());
    assert!(close.shadow_ray(receiver).is_ok());
}

#[test]
fn half_rectangle_occlusion_produces_a_partial_direct_response() {
    let light = rectangle(2.0, 10.0);
    let blocker = super::super::Triangle::new([Vec3::new(0.0, -5.0, 5.0), Vec3::new(5.0, -5.0, 5.0), Vec3::new(0.0, 5.0, 5.0)]).unwrap();
    let other = super::super::Triangle::new([Vec3::new(5.0, -5.0, 5.0), Vec3::new(5.0, 5.0, 5.0), Vec3::new(0.0, 5.0, 5.0)]).unwrap();
    let surface = OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 0.0, 1.0, 1.0).unwrap();
    let (mut clear, mut visible, mut blocked) = (Vec3::ZERO, Vec3::ZERO, 0);
    for index in 0..16 {
        let sample = light.sample(Vec3::ZERO, 5, 0, index, 16, 0).unwrap().unwrap();
        let contribution = sample.shade(surface, Vec3::Z, Vec3::Z).unwrap();
        clear += contribution;
        let ray = sample.shadow_ray(Vec3::ZERO).unwrap();
        if blocker.intersect(&ray).is_some() || other.intersect(&ray).is_some() {
            blocked += 1;
        } else {
            visible += contribution;
        }
    }
    // The blocker at half the emitter's height covers exactly its positive-X half.
    assert_eq!(blocked, 8);
    assert!(visible.x > 0.0 && visible.x < clear.x);
    assert!(visible.y > 0.0 && visible.y < clear.y);
}

#[test]
fn hostile_geometry_radiance_and_sample_counts_refuse_without_clipping() {
    let white = Vec3::new(1.0, 1.0, 1.0);
    for u in [Vec3::ZERO, Vec3::Y, Vec3::new(f64::NAN, 0.0, 0.0), Vec3::X * (MAX_EDGE_MM * 2.0)] {
        assert!(RectangleLight::new(Vec3::Z, u, Vec3::Y, white, 1.0).is_err());
    }
    for bad in [f64::NAN, f64::INFINITY, -1.0, 65.0] {
        assert!(RectangleLight::new(Vec3::Z, Vec3::X, Vec3::Y, white, bad).is_err());
    }
    for bad in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        assert!(RectangleLight::new(Vec3::Z, Vec3::X, Vec3::Y, Vec3::new(bad, 1.0, 1.0), 1.0).is_err());
    }
    let light = rectangle(2.0, 10.0);
    for count in [0, 2, 3, 65, u32::MAX] {
        assert!(light.sample(Vec3::ZERO, 0, 0, 0, count, 0).is_err());
    }
    assert!(light.sample(Vec3::ZERO, 0, MAX_AREA_LIGHTS as u32, 0, 16, 0).is_err());
    assert!(light.sample(Vec3::ZERO, 0, 0, 16, 16, 0).is_err());
    assert!(light.sample(Vec3::new(f64::NAN, 0.0, 0.0), 0, 0, 0, 16, 0).is_err());
    assert!(light.sample(Vec3::Z * 10.0, 0, 0, 0, 1, 0).is_err());
}
