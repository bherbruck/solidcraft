use super::*;

fn strip() -> LedStripOptions {
    LedStripOptions {
        center_mm: [0.0, 0.0, 10.0],
        length_mm: 8.0,
        width_mm: 0.5,
        length_axis: [1.0, 0.0, 0.0],
        width_axis: [0.0, -1.0, 0.0],
        color: [1.0, 0.5, 0.25],
        intensity: 2.0,
    }
}

#[test]
fn dimensions_and_orientation_lower_without_power_multiplier() {
    let rectangle = strip().rectangle().unwrap();
    assert_eq!(rectangle.edge_u_mm, [8.0, 0.0, 0.0]);
    assert_eq!(rectangle.edge_v_mm, [0.0, -0.5, 0.0]);
    assert_eq!(rectangle.intensity, 2.0);
    assert_eq!(rectangle.color, [1.0, 0.5, 0.25]);
}

#[test]
fn rejects_hostile_geometry_radiance_and_unsupported_pitch() {
    for bad in [f64::NAN, f64::INFINITY, -1.0, 0.0, 1e-7, 1e6 + 1.0] {
        assert!(LedStripOptions { length_mm: bad, ..strip() }.rectangle().is_err());
        assert!(LedStripOptions { width_mm: bad, ..strip() }.rectangle().is_err());
    }
    for axis in [[0.0; 3], [2.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0], [0.0, -1.0, 0.0]] {
        assert!(LedStripOptions { length_axis: axis, ..strip() }.rectangle().is_err());
    }
    assert!(LedStripOptions { center_mm: [1e12, 0.0, 0.0], ..strip() }.rectangle().is_err());
    assert!(LedStripOptions { color: [1.01, 0.0, 0.0], ..strip() }.rectangle().is_err());
    assert!(LedStripOptions { intensity: 65.0, ..strip() }.rectangle().is_err());
    let mut wire = serde_json::to_value(strip()).unwrap();
    wire["pitch_mm"] = serde_json::json!(10);
    assert!(serde_json::from_value::<LedStripOptions>(wire).is_err());
}

#[test]
fn one_slot_per_strip_and_original_capacity_accounted() {
    let mut options = RaytraceOptions { lights: Some(Vec::new()), ..Default::default() };
    for count in 1..=4 {
        strip().append_to(&mut options, 64).unwrap();
        assert_eq!(options.area_lights.as_ref().unwrap().len(), count);
        assert_eq!(options.area_samples, Some(64));
        assert_eq!(
            options.retained_bytes().unwrap(),
            (options.area_lights.as_ref().unwrap().capacity() * std::mem::size_of::<RectangleLightOptions>()) as u64
        );
        options.validate().unwrap();
    }
    let original = options.clone();
    assert!(strip().append_to(&mut options, 64).is_err());
    assert_eq!(options, original);
}

#[test]
fn rejection_is_atomic_including_capacity_and_conflicting_sample_count() {
    let mut options = RaytraceOptions::default();
    strip().append_to(&mut options, 16).unwrap();
    for (input, samples) in [(strip(), 4), (strip(), 3), (LedStripOptions { width_mm: 0.0, ..strip() }, 16)] {
        let before = options.clone();
        let capacity = options.area_lights.as_ref().unwrap().capacity();
        assert!(input.append_to(&mut options, samples).is_err());
        assert_eq!(options, before);
        assert_eq!(options.area_lights.as_ref().unwrap().capacity(), capacity);
    }
    let mut over = RaytraceOptions { area_lights: Some(Vec::with_capacity(5)), ..Default::default() };
    assert!(strip().append_to(&mut over, 1).is_err());
    assert_eq!(over.area_lights.as_ref().unwrap().capacity(), 5);
}

#[test]
fn final_edge_cosine_rejection_preserves_request_and_capacity() {
    let a = 1.0 - 5e-13;
    let input = LedStripOptions { length_axis: [a, 0.0, 0.0], width_axis: [1e-10, a, 0.0], ..strip() };
    assert!(input.rectangle().is_err());
    let mut options = RaytraceOptions::default();
    strip().append_to(&mut options, 16).unwrap();
    let before = options.clone();
    let capacity = options.area_lights.as_ref().unwrap().capacity();
    assert!(input.append_to(&mut options, 16).is_err());
    assert_eq!(options, before);
    assert_eq!(options.area_lights.as_ref().unwrap().capacity(), capacity);
    options.validate().unwrap();
}

#[test]
fn rounded_dimension_endpoints_never_return_rt3_invalid_edges() {
    let k = 0.5_f64.sqrt();
    for dimension in [1e-6_f64.next_down(), 1e-6, 1e-6_f64.next_up(), 1e6_f64.next_down(), 1e6, 1e6_f64.next_up()] {
        for (u, v) in [([1.0, 0.0, 0.0], [0.0, -1.0, 0.0]), ([k, k, 0.0], [k, -k, 0.0])] {
            let input = LedStripOptions { length_mm: dimension, width_mm: dimension, length_axis: u, width_axis: v, ..strip() };
            let mut options = RaytraceOptions { lights: Some(Vec::new()), ..Default::default() };
            if let Ok(rectangle) = input.rectangle() {
                options.area_lights = Some(vec![rectangle]);
                options.validate().unwrap();
                let mut lowered = RaytraceOptions { lights: Some(Vec::new()), ..Default::default() };
                input.append_to(&mut lowered, 1).unwrap();
                lowered.validate().unwrap();
            } else {
                let before = options.clone();
                assert!(input.append_to(&mut options, 1).is_err());
                assert_eq!(options, before);
            }
        }
    }
    for dimension in [1e-6, 1e6] {
        LedStripOptions { length_mm: dimension, width_mm: dimension, ..strip() }.rectangle().unwrap();
    }
}

#[test]
fn recipe_roundtrip_and_tints_are_explicit_linear_values() {
    assert_eq!(serde_json::from_value::<LedStripOptions>(serde_json::to_value(strip()).unwrap()).unwrap(), strip());
    assert_eq!(StripTint::Red.linear_rgb(), [1.0, 0.0, 0.0]);
    assert_eq!(StripTint::Green.linear_rgb(), [0.0, 1.0, 0.0]);
    assert_eq!(StripTint::Blue.linear_rgb(), [0.0, 0.0, 1.0]);
    assert!(StripTint::Warm.linear_rgb()[0] > StripTint::Warm.linear_rgb()[2]);
    assert!(StripTint::Cool.linear_rgb()[2] > StripTint::Cool.linear_rgb()[0]);
}

#[cfg(feature = "raytrace-cpu")]
mod consumer {
    use super::*;
    use crate::job::{CancelToken, ImageRequest, Integrator, TaskPoll};
    use crate::raytrace::{area::RectangleLight, image::ImageTask};
    use crate::{Camera, ProductMesh, ProductScene, StandardView, surface::OpaqueSurface};
    use solidcraft_geom::{Aabb3, Mesh};
    use std::sync::Arc;

    fn light(input: LedStripOptions) -> RectangleLight {
        let r = input.rectangle().unwrap();
        let vec = |[x, y, z]: [f64; 3]| Vec3::new(x, y, z);
        RectangleLight::new(vec(r.center_mm), vec(r.edge_u_mm), vec(r.edge_v_mm), vec(r.color), r.intensity).unwrap()
    }

    #[test]
    fn analytic_area_weight_inverse_square_and_orientation() {
        let source = light(strip());
        assert_eq!(source.area_mm2(), 4.0);
        assert_eq!(source.normal(), -Vec3::Z);
        let sample = source.sample(Vec3::ZERO, 0, 0, 0, 1, 0).unwrap().unwrap();
        assert!((sample.incident() - Vec3::new(0.08, 0.04, 0.02)).len() < 1e-14);
        let far = light(LedStripOptions { center_mm: [0.0, 0.0, 20.0], ..strip() }).sample(Vec3::ZERO, 0, 0, 0, 1, 0).unwrap().unwrap();
        assert!((far.incident() - sample.incident() * 0.25).len() < 1e-14);
        assert!(source.sample(Vec3::new(0.0, 0.0, 20.0), 0, 0, 0, 1, 0).unwrap().is_none());
        assert!(source.sample(Vec3::new(20.0, 0.0, 10.0), 0, 0, 0, 1, 0).unwrap().is_none());
    }

    #[test]
    fn total_sampling_weight_does_not_grow_with_sample_count() {
        let source = light(strip());
        // Undo each sample's geometric factor: sum incident*r²/cos must be Le*A,
        // for EVERY N, rather than Le*A*N or Le*A/N.
        for n in [1, 4, 16, 64] {
            let mut sum = Vec3::ZERO;
            for index in 0..n {
                let sample = source.sample(Vec3::ZERO, 7, 0, index, n, 19).unwrap().unwrap();
                let cosine = (-source.normal()).dot(sample.direction());
                sum += sample.incident() * (sample.distance_mm().powi(2) / cosine);
            }
            assert!((sum - Vec3::new(8.0, 4.0, 2.0)).len() < 1e-12);
        }
    }

    fn scene() -> ProductScene {
        let mesh = Mesh {
            positions: vec![Vec3::new(-20.0, -20.0, 0.0), Vec3::new(20.0, -20.0, 0.0), Vec3::new(20.0, 20.0, 0.0), Vec3::new(-20.0, 20.0, 0.0)],
            normals: vec![Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![0, 0],
            ..Default::default()
        };
        let bounds = mesh.bounds();
        ProductScene {
            bounds,
            meshes: vec![ProductMesh {
                name: "original LED receiver".into(),
                mesh: Arc::new(mesh),
                color: [255; 3],
                opacity: 1.0,
                surface: Some(OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 0.0, 1.0, 1.0).unwrap()),
                faces: Vec::new(),
            }],
        }
    }
    fn request(options: RaytraceOptions) -> ImageRequest {
        let mut camera = Camera { distance: 50.0, ..Default::default() };
        camera.set_view(StandardView::Top);
        ImageRequest {
            integrator: Integrator::Raytrace,
            raytrace: Some(options),
            camera,
            width: 1,
            height: 1,
            background: [0; 3],
            transparent_background: false,
        }
    }
    fn render(input: LedStripOptions, units: usize) -> Vec<u8> {
        let mut options = RaytraceOptions { lights: Some(Vec::new()), ..Default::default() };
        input.append_to(&mut options, 1).unwrap();
        let mut task = ImageTask::new(scene(), request(options.clone()), CancelToken::default(), options).unwrap();
        for _ in 0..10000 {
            if let TaskPoll::Complete(png) = task.step(units).unwrap() {
                return png;
            }
        }
        panic!("bounded LED fixture did not finish");
    }

    #[test]
    fn real_consumer_png_matches_analytic_lambert_pixel_and_partition() {
        let png = render(LedStripOptions { color: [1.0; 3], ..strip() }, 1);
        assert_eq!(png, render(LedStripOptions { color: [1.0; 3], ..strip() }, 128));
        let actual = ::image::load_from_memory(&png).unwrap().to_rgba8();
        // Center ray, area4, Le2, r10, Lambert1/pi; one shared tone-map.
        let linear = 0.08 / std::f64::consts::PI;
        let mapped = linear / (1.0 + linear);
        let expected = ((1.055 * mapped.powf(1.0 / 2.4) - 0.055) * 255.0).round() as u8;
        assert_eq!(actual.get_pixel(0, 0).0, [expected, expected, expected, 255]);
        let dark = ::image::load_from_memory(&render(LedStripOptions { width_axis: [0.0, 1.0, 0.0], ..strip() }, 1)).unwrap().to_rgba8();
        assert_eq!(dark.get_pixel(0, 0).0, [0, 0, 0, 255]);
    }

    #[test]
    fn same_task_cancellation_refuses_partial_output() {
        let mut options = RaytraceOptions { lights: Some(Vec::new()), ..Default::default() };
        strip().append_to(&mut options, 64).unwrap();
        let cancel = CancelToken::default();
        let mut task = ImageTask::new(scene(), request(options.clone()), cancel.clone(), options).unwrap();
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        cancel.cancel();
        assert_eq!(task.step(1).err().unwrap().code, ErrorCode::Cancelled);
    }

    #[test]
    fn admission_accounts_the_existing_rectangle_storage() {
        let empty = ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY };
        let baseline = RaytraceOptions { lights: Some(Vec::new()), ..Default::default() };
        let before = crate::raytrace::image::admit(&empty, &request(baseline.clone()), &baseline).unwrap();
        let mut options = baseline;
        strip().append_to(&mut options, 64).unwrap();
        let after = crate::raytrace::image::admit(&empty, &request(options.clone()), &options).unwrap();
        assert!(after.estimated_total_bytes > before.estimated_total_bytes);
    }
}
