use super::*;
use crate::surface::OpaqueSurface;

fn point(position: Vec3) -> PunctualLight {
    PunctualLight::new(position, Vec3::new(1.0, 0.5, 0.25), 2.0, 100.0, 0.001, None).unwrap()
}

fn hit(position: Vec3) -> SharedHit<'static> {
    SharedHit {
        body_index: 0,
        body_name: "receiver",
        primitive_index: 0,
        distance_mm: 1.0,
        position,
        geometric_normal: Vec3::Z,
        front_face: true,
        shading_normal: Vec3::Z,
        surface: OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 0.0, 0.5, 1.5).unwrap(),
        vertices: [position, position + Vec3::X, position + Vec3::Y],
        barycentric: [1.0, 0.0, 0.0],
    }
}

#[test]
fn reference_distance_and_inverse_square_are_explicit() {
    let light = point(Vec3::ZERO);
    let near = light.sample(Vec3::Z * 100.0).unwrap();
    let far = light.sample(Vec3::Z * 200.0).unwrap();
    assert_eq!(near.incident_linear_srgb, Vec3::new(2.0, 1.0, 0.5));
    assert_eq!(near.incident_linear_srgb, far.incident_linear_srgb * 4.0);
    assert_eq!(near.direction, -Vec3::Z);
}

#[test]
fn cone_feather_has_analytic_cosine_midpoint_and_boundaries() {
    let inner = 0.2_f64;
    let outer = 0.6_f64;
    let cone = SpotCone::new(Vec3::Z, inner, outer).unwrap();
    let light = PunctualLight::new(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0), 1.0, 100.0, 0.001, Some(cone)).unwrap();
    for (cosine, expected) in [(1.0, 1.0), (inner.cos(), 1.0), ((inner.cos() + outer.cos()) / 2.0, 0.5), (outer.cos(), 0.0), (0.0, 0.0), (-1.0, 0.0)]
    {
        let p = Vec3::new((1.0 - cosine * cosine).sqrt(), 0.0, cosine) * 100.0;
        let actual = light.sample(p).unwrap().incident_linear_srgb.x;
        assert!((actual - expected).abs() < 1e-12, "cos={cosine}, actual={actual}");
    }
}

#[test]
fn world_translation_and_rotation_preserve_response() {
    let origin = Vec3::new(200.0, -300.0, 500.0);
    let a = point(Vec3::ZERO).sample(Vec3::Z * 100.0).unwrap();
    let b = point(origin).sample(origin + Vec3::X * 100.0).unwrap();
    assert_eq!(a.incident_linear_srgb, b.incident_linear_srgb);
    let cone = SpotCone::new(Vec3::X, 0.0, 0.5).unwrap();
    let light = PunctualLight::new(origin, Vec3::new(1.0, 1.0, 1.0), 1.0, 100.0, 0.001, Some(cone)).unwrap();
    assert_eq!(light.sample(origin + Vec3::X * 100.0).unwrap().incident_linear_srgb.x, 1.0);
    assert_eq!(light.sample(origin - Vec3::X * 100.0).unwrap().incident_linear_srgb, Vec3::ZERO);
}

#[test]
fn hostile_inputs_and_near_source_refuse_without_clamping() {
    for bad in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(PunctualLight::new(Vec3::ZERO, Vec3::X, bad, 100.0, 0.001, None).is_err());
        assert!(PunctualLight::new(Vec3::ZERO, Vec3::X, 1.0, bad, 0.001, None).is_err());
        assert!(SpotCone::new(Vec3::Z, 0.0, bad).is_err());
    }
    assert!(SpotCone::new(Vec3::ZERO, 0.0, 0.5).is_err());
    assert!(SpotCone::new(Vec3::Z, 0.5, 0.5).is_err());
    assert!(SpotCone::new(Vec3::Z, 0.0, f64::MIN_POSITIVE).is_err());
    assert!(point(Vec3::ZERO).sample(Vec3::ZERO).is_err());
    assert!(point(Vec3::ZERO).sample(Vec3::Z * 0.0009).is_err());
    assert!(point(Vec3::ZERO).sample(Vec3::Z * 0.001).is_ok());
    assert!(PunctualLight::new(Vec3::ZERO, Vec3::X, 64.0, 1e6, 0.001, None).unwrap().sample(Vec3::Z * 0.001).is_err());
}

#[test]
fn shadow_interval_excludes_source_and_geometry_beyond_it() {
    let sample = point(Vec3::Z * 10.0).sample(Vec3::ZERO).unwrap();
    let ray = sample.shadow_ray(hit(Vec3::ZERO)).unwrap();
    assert!(ray.interval()[0] > 0.0 && ray.interval()[1] < 10.0);
    let triangle = |z| super::super::Triangle::new([Vec3::new(-1.0, -1.0, z), Vec3::new(1.0, -1.0, z), Vec3::new(0.0, 1.0, z)]).unwrap();
    assert!(triangle(5.0).intersect(&ray).is_some());
    assert!(triangle(10.0).intersect(&ray).is_none());
    assert!(triangle(20.0).intersect(&ray).is_none());
}

#[test]
fn offset_passing_source_refuses_instead_of_reversing_shadow() {
    let p = Vec3::Z * (1e12 - 1.0);
    let sample = point(p + Vec3::Z * 0.0011).sample(p).unwrap();
    assert!(sample.shadow_ray(hit(p)).is_err());
}
