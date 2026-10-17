use super::*;
use crate::job::{AnisotropicFaceOptions, AnisotropicOrientation, ReflectionBody, ReflectionMode, ReflectionOptions};
use crate::{ProductMesh, StandardView};
use solidcraft_geom::Mesh;
use std::sync::Arc;

fn scene() -> ProductScene {
    let reflector = ProductMesh {
        name: "explicit planar reflector".into(),
        mesh: Arc::new(Mesh {
            positions: vec![Vec3::new(-20.0, -20.0, -20.0), Vec3::new(20.0, -20.0, -20.0), Vec3::new(20.0, 20.0, 20.0), Vec3::new(-20.0, 20.0, 20.0)],
            normals: vec![super::super::unit(Vec3::new(0.0, -1.0, 1.0)).unwrap(); 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![7, 7],
            ..Default::default()
        }),
        color: [255; 3],
        opacity: 1.0,
        surface: Some(OpaqueSurface::new(Vec3::new(0.8, 0.8, 0.8), 1.0, 0.4, 1.5).unwrap()),
        faces: Vec::new(),
    };
    let object = ProductMesh {
        name: "off-axis orange object".into(),
        mesh: Arc::new(Mesh {
            positions: vec![Vec3::new(-3.0, -12.0, 10.0), Vec3::new(-3.0, 12.0, 10.0), Vec3::new(3.0, 12.0, 10.0), Vec3::new(3.0, -12.0, 10.0)],
            normals: vec![-Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![0, 0],
            ..Default::default()
        }),
        color: [240, 80, 20],
        opacity: 1.0,
        surface: None,
        faces: Vec::new(),
    };
    let meshes = vec![reflector, object];
    let bounds = meshes.iter().map(|b| b.mesh.bounds()).reduce(|a, b| a.union(&b)).unwrap();
    ProductScene { meshes, bounds }
}

fn request(orientation: AnisotropicOrientation, reflect: bool) -> ImageRequest {
    let mut camera = crate::Camera { distance: 50.0, ..Default::default() };
    camera.set_view(StandardView::Front);
    let target = AnisotropicFaceOptions {
        body_index: 0,
        body_name: "explicit planar reflector".into(),
        face: 7,
        tangent: [1.0, 0.0, 0.0],
        orientation,
        roughness_x: 0.2,
        roughness_y: 0.55,
    };
    let options = RaytraceOptions {
        anisotropic_faces: [Some(target), None],
        reflections: reflect.then(|| ReflectionOptions {
            bodies: vec![ReflectionBody { body: "explicit planar reflector".into(), mode: ReflectionMode::RoughSpecular }],
            max_bounces: 1,
            samples: 64,
        }),
        lights: Some(vec![job::DirectionalLightOptions { direction: [0.0, 0.0, -1.0], color: [1.0; 3], intensity: 2.0 }]),
        shadows: false,
        ..Default::default()
    };
    ImageRequest {
        integrator: job::Integrator::Raytrace,
        raytrace: Some(options),
        camera,
        width: 1,
        height: 1,
        background: [13, 27, 41],
        transparent_background: false,
    }
}

fn render(request: ImageRequest) -> [u8; 4] {
    let options = request.raytrace.clone().unwrap();
    let mut task = ImageTask::new(scene(), request, CancelToken::default(), options).unwrap();
    for _ in 0..200_000 {
        if let TaskPoll::Complete(png) = task.step(16384).unwrap() {
            return ::image::load_from_memory(&png).unwrap().to_rgba8().get_pixel(0, 0).0;
        }
    }
    panic!("bounded reflected scene PNG did not finish");
}

#[test]
fn actual_reflected_object_changes_with_the_selected_planar_orientation() {
    let baseline = render(request(AnisotropicOrientation::Zero, false));
    assert_eq!(baseline, [0, 0, 0, 255]); // Light is below the reflector; no direct term.
    let a = render(request(AnisotropicOrientation::Zero, true));
    let b = render(request(AnisotropicOrientation::QuarterTurn, true));
    assert!(a[0] > a[1] && a[1] > a[2], "actual orange object must contribute via a secondary hit: {a:?}");
    assert_ne!(a, b, "rotation must affect actual reflected scene transport");
    assert_eq!(a, render(request(AnisotropicOrientation::Zero, true)));
}

fn union_environment() -> crate::lighting::environment_input::EnvironmentInput {
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nPRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n\n-Y 2 +X 4\n".to_vec();
    for _ in 0..8 {
        bytes.extend_from_slice(&[128, 64, 32, 129]);
    }
    crate::lighting::environment_input::EnvironmentInput::new(crate::lighting::HdrSource::new(bytes).unwrap(), 0.0, 1.0, 4).unwrap()
}

fn secondary_union(orientation: AnisotropicOrientation, intensity: f64) -> ImageTask {
    let mut req = request(orientation, true);
    let options = req.raytrace.as_mut().unwrap();
    options.lights = Some(Vec::new());
    options.punctual_lights = Some(vec![job::PunctualLightOptions {
        position_mm: [0.0, 0.0, -10.0],
        color: [1.0; 3],
        intensity,
        reference_distance_mm: 20.0,
        minimum_distance_mm: 0.001,
        cone: None,
    }]);
    job::led_strip::LedStripOptions {
        center_mm: [0.0, 0.0, -10.0],
        length_mm: 8.0,
        width_mm: 0.5,
        length_axis: [1.0, 0.0, 0.0],
        width_axis: [0.0, 1.0, 0.0],
        color: [1.0; 3],
        intensity,
    }
    .append_to(options, 4)
    .unwrap();
    ImageTask::new_with_environment(scene(), req.clone(), CancelToken::default(), req.raytrace.unwrap(), Some(union_environment())).unwrap()
}

fn render_secondary_union(mut task: ImageTask, units: usize) -> (Vec<u8>, bool, bool, bool) {
    let mut reset = false;
    let mut point = false;
    let mut area = false;
    for _ in 0..500_000 {
        if let Some(p) = task.pending.as_ref().filter(|p| p.bounce == 1) {
            if p.hit.is_none() {
                assert_eq!(p.punctual_index, 0);
                assert_eq!(p.area_light, 0);
                assert_eq!(p.area_index, 0);
                assert!(!p.punctual_shadow && !p.punctual_ready && !p.area_shadow && !p.area_ready);
                assert_eq!(p.punctual_value, Vec3::ZERO);
                assert_eq!(p.area_value, Vec3::ZERO);
                reset = true;
            } else {
                point |= p.punctual_value.len2() > 0.0;
                area |= p.area_value.len2() > 0.0;
            }
        }
        if let TaskPoll::Complete(png) = task.step(units).unwrap() {
            return (png, reset, point, area);
        }
    }
    panic!("bounded HDR/anisotropic secondary union did not finish");
}

#[test]
fn hdr_anisotropic_secondary_point_led_reset_and_partition_are_real_consumer_events() {
    let (lit, reset, point, area) = render_secondary_union(secondary_union(AnisotropicOrientation::Zero, 4.0), 1);
    assert!(reset && point && area, "secondary light state was not reset and consumed");
    let (dark, _, _, _) = render_secondary_union(secondary_union(AnisotropicOrientation::Zero, 0.0), 1);
    let rgb = |png: &[u8]| ::image::load_from_memory(png).unwrap().to_rgba8().get_pixel(0, 0).0;
    let a = rgb(&lit);
    let b = rgb(&dark);
    assert!(a[0] > b[0] && a[0] > a[1] && a[1] > a[2], "secondary direct lights must add orange response: {a:?} vs {b:?}");
    let (partitioned, _, _, _) = render_secondary_union(secondary_union(AnisotropicOrientation::Zero, 4.0), 128);
    assert_eq!(lit, partitioned);
    let (rotated, _, _, _) = render_secondary_union(secondary_union(AnisotropicOrientation::QuarterTurn, 4.0), 1);
    assert_ne!(lit, rotated);
}

#[test]
fn equal_axes_in_the_image_consumer_match_isotropic_reflection_bytes() {
    let mut isotropic = request(AnisotropicOrientation::Zero, true);
    isotropic.raytrace.as_mut().unwrap().anisotropic_faces = [None, None];
    let expected = render(isotropic);
    for orientation in [AnisotropicOrientation::Zero, AnisotropicOrientation::QuarterTurn] {
        let mut explicit = request(orientation, true);
        let target = explicit.raytrace.as_mut().unwrap().anisotropic_faces[0].as_mut().unwrap();
        target.roughness_x = 0.4;
        target.roughness_y = 0.4;
        assert_eq!(render(explicit), expected);
    }
}

#[test]
fn mirrors_missing_or_ambiguous_reflection_targets_refuse_before_pixels() {
    for kind in 0..3 {
        let mut scene = scene();
        let mut request = request(AnisotropicOrientation::Zero, true);
        let reflection = &mut request.raytrace.as_mut().unwrap().reflections.as_mut().unwrap().bodies[0];
        match kind {
            0 => reflection.mode = ReflectionMode::Mirror,
            1 => reflection.body = "missing".into(),
            _ => scene.meshes[1].name = "explicit planar reflector".into(),
        }
        assert!(admit(&scene, &request, request.raytrace.as_ref().unwrap()).is_err());
    }
}

#[test]
fn secondary_anisotropic_traversal_shares_job_work_and_cancellation() {
    for cancel_now in [true, false] {
        let request = request(AnisotropicOrientation::Zero, true);
        let options = request.raytrace.clone().unwrap();
        let cancel = CancelToken::default();
        let mut task = ImageTask::new(scene(), request, cancel.clone(), options).unwrap();
        let mut reached = false;
        for _ in 0..100_000 {
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
            if task.pending.as_ref().is_some_and(|p| p.bounce > 0) {
                reached = true;
                break;
            }
        }
        assert!(reached, "actual consumer never continued the sampled anisotropic ray");
        if cancel_now {
            cancel.cancel();
            assert_eq!(task.step(1).err().unwrap().code, ErrorCode::Cancelled);
        } else {
            task.work.limit = task.work.used();
            assert_eq!(task.step(1).err().unwrap().code, ErrorCode::LimitExceeded);
        }
    }
}

#[test]
fn primary_last_shadow_and_reflected_continuations_each_charge_at_most_one_unit() {
    for shadows in [false, true] {
        let mut request = request(AnisotropicOrientation::Zero, true);
        request.raytrace.as_mut().unwrap().shadows = shadows;
        let options = request.raytrace.clone().unwrap();
        let mut task = ImageTask::new(scene(), request, CancelToken::default(), options).unwrap();
        for _ in 0..10_000 {
            if task.bvh.is_some() {
                break;
            }
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        }
        assert!(task.bvh.is_some());
        let mut primary = false;
        let mut last_shadow = false;
        let mut secondary = false;
        let mut secondary_hit = false;
        let mut secondary_terminal = false;
        for _ in 0..200_000 {
            if task.pixel == 1 {
                break;
            }
            let before_work = task.work.used();
            let before_sample = task.sample;
            let before = task.pending.as_ref().map(|p| (p.bounce, p.hit.is_some(), p.light));
            task.pixel_one().unwrap();
            assert!(task.work.used() - before_work <= 1, "one pixel continuation consumed traversal and shading work together");
            let after = task.pending.as_ref().map(|p| (p.bounce, p.hit.is_some(), p.light));
            primary |= matches!(before, Some((0, false, _))) && matches!(after, Some((0, true, _)));
            last_shadow |= shadows && matches!(before, Some((_, true, 0))) && matches!(after, Some((_, true, 1)));
            secondary |= after.is_some_and(|(bounce, _, _)| bounce > 0);
            secondary_hit |= before.is_some_and(|(bounce, hit, _)| bounce > 0 && !hit) && after.is_some_and(|(bounce, hit, _)| bounce > 0 && hit);
            secondary_terminal |= before.is_some_and(|(bounce, _, _)| bounce > 0) && (task.sample != before_sample || task.pixel == 1);
        }
        assert_eq!(task.pixel, 1);
        assert!(primary && secondary && secondary_hit && secondary_terminal);
        assert!(!shadows || last_shadow);
    }
    // The public poll groups at most four of those continuations.
    let request = request(AnisotropicOrientation::Zero, true);
    let options = request.raytrace.clone().unwrap();
    let mut task = ImageTask::new(scene(), request, CancelToken::default(), options).unwrap();
    for _ in 0..10_000 {
        if task.bvh.is_some() {
            break;
        }
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
    }
    let mut completed = false;
    for _ in 0..200_000 {
        let before = task.work.used();
        let poll = task.step(4).unwrap();
        assert!(task.work.used() - before <= 4);
        if matches!(poll, TaskPoll::Complete(_)) {
            completed = true;
            break;
        }
    }
    assert!(completed);
}

#[test]
fn cancellation_after_last_shadow_is_seen_before_shading_or_sampling() {
    for secondary in [false, true] {
        let mut request = request(AnisotropicOrientation::Zero, true);
        request.raytrace.as_mut().unwrap().shadows = true;
        let options = request.raytrace.clone().unwrap();
        let cancel = CancelToken::default();
        let mut task = ImageTask::new(scene(), request, cancel.clone(), options).unwrap();
        let mut reached = false;
        for _ in 0..100_000 {
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
            if task.pending.as_ref().is_some_and(|p| p.hit.is_some() && p.light == task.lights.lights().len() && (p.bounce > 0) == secondary) {
                reached = true;
                break;
            }
        }
        assert!(reached && task.pixels.is_empty());
        let before_sample = task.sample;
        cancel.cancel();
        assert_eq!(task.pixel_one().err().unwrap().code, ErrorCode::Cancelled);
        assert_eq!(task.sample, before_sample);
        assert!(task.pixels.is_empty());
    }
}
