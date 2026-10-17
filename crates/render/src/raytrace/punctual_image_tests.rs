use super::*;
use crate::{ProductMesh, StandardView, job::PunctualLightOptions};
use solidcraft_geom::{Aabb3, Mesh};
use std::sync::Arc;

fn plane(name: &str, x: f64, z: f64, half: f64) -> ProductMesh {
    ProductMesh {
        name: name.into(),
        mesh: Arc::new(Mesh {
            positions: vec![Vec3::new(x - half, -half, z), Vec3::new(x + half, -half, z), Vec3::new(x + half, half, z), Vec3::new(x - half, half, z)],
            normals: vec![Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![0, 0],
            ..Default::default()
        }),
        color: [255; 3],
        opacity: 1.0,
        surface: Some(OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 1.0, 1.5).unwrap()),
        faces: Vec::new(),
    }
}

fn task(blocker: Option<(f64, f64)>, shadows: bool) -> ImageTask {
    let mut meshes = vec![plane("receiver", 0.0, 0.0, 30.0)];
    if let Some((x, z)) = blocker {
        meshes.push(plane("blocker", x, z, 2.0));
    }
    let bounds = meshes.iter().fold(Aabb3::EMPTY, |b, m| b.union(&m.mesh.bounds()));
    let scene = ProductScene { meshes, bounds };
    let mut camera = crate::Camera { distance: 80.0, fov: 0.0, ..Default::default() };
    camera.set_view(StandardView::Top);
    let options = RaytraceOptions {
        lights: Some(Vec::new()),
        punctual_lights: Some(vec![PunctualLightOptions {
            position_mm: [20.0, 0.0, 10.0],
            color: [1.0, 0.5, 0.25],
            intensity: 1.0,
            reference_distance_mm: 10.0,
            minimum_distance_mm: 0.001,
            cone: None,
        }]),
        shadows,
        ..Default::default()
    };
    let request = ImageRequest {
        integrator: job::Integrator::Raytrace,
        raytrace: Some(options.clone()),
        camera,
        width: 1,
        height: 1,
        background: [13, 27, 41],
        transparent_background: false,
    };
    ImageTask::new(scene, request, CancelToken::default(), options).unwrap()
}

fn render(mut task: ImageTask) -> [u8; 4] {
    for _ in 0..100_000 {
        let before = task.work.used();
        let poll = task.step(1).unwrap();
        assert!(task.work.used() - before <= 1);
        if let TaskPoll::Complete(png) = poll {
            return ::image::load_from_memory(&png).unwrap().to_rgba8().get_pixel(0, 0).0;
        }
    }
    panic!("bounded punctual PNG never completed");
}

#[test]
fn actual_consumer_matches_analytic_rgb_and_finite_source_occlusion() {
    let lit = render(task(None, true));
    // White rough conductor, normal view: f*cos=cos/[2*pi*(1+cos)].
    let cosine = 1.0 / 5.0_f64.sqrt();
    let l = 0.2 * cosine / (2.0 * std::f64::consts::PI * (1.0 + cosine));
    for (actual, factor) in lit.into_iter().take(3).zip([1.0, 0.5, 0.25]) {
        let expected = encode(l * factor / (1.0 + l * factor));
        assert!(actual.abs_diff(expected) <= 1, "actual={actual}, expected={expected}");
    }
    assert_eq!(render(task(Some((10.0, 5.0)), true)), [0, 0, 0, 255]);
    assert_eq!(render(task(Some((40.0, 20.0)), true)), lit, "geometry behind source cast a shadow");
    assert_eq!(render(task(Some((10.0, 5.0)), false)), lit);
}

#[test]
fn actual_spot_consumer_uses_emitted_world_axis_and_cone() {
    let point = render(task(None, true));
    for (axis, expected) in [([-20.0, 0.0, -10.0], point), ([20.0, 0.0, 10.0], [0, 0, 0, 255])] {
        let previous = task(None, true);
        let mut options = previous.options.clone();
        options.punctual_lights.as_mut().unwrap()[0].cone =
            Some(job::SpotConeOptions { emitted_axis: axis, inner_half_angle_radians: 0.2, outer_half_angle_radians: 0.5 });
        let mut request = previous.request.clone();
        request.raytrace = Some(options.clone());
        let actual = ImageTask::new(previous.scene.clone(), request, CancelToken::default(), options).unwrap();
        assert_eq!(render(actual), expected);
    }
}

#[test]
fn cancellation_between_final_shadow_and_sum_prevents_publication() {
    let mut task = task(None, true);
    for _ in 0..100_000 {
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        if task.pending.as_ref().is_some_and(|p| p.punctual_ready) {
            assert!(task.pixels.is_empty());
            let before = task.work.used();
            task.cancel.cancel();
            assert_eq!(task.step(1).err().unwrap().code, ErrorCode::Cancelled);
            assert_eq!(task.work.used(), before);
            assert!(task.pixels.is_empty());
            return;
        }
    }
    panic!("final punctual shadow continuation was not reached");
}

#[test]
fn punctual_work_exhaustion_is_shared_and_task_metadata_remains_bounded() {
    assert!(std::mem::size_of::<ImageTask>() <= 4096);
    let mut task = task(None, true);
    for _ in 0..100_000 {
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        if task.pending.as_ref().is_some_and(|p| p.punctual_shadow) {
            task.work.limit = task.work.used();
            assert_eq!(task.step(1).err().unwrap().code, ErrorCode::LimitExceeded);
            assert!(task.pixels.is_empty());
            return;
        }
    }
    panic!("punctual shadow traversal was not reached");
}

fn mixed(n: u32, shadows: bool) -> ImageTask {
    let previous = task(None, shadows);
    let mut options = previous.options.clone();
    options.lights = Some(vec![job::DirectionalLightOptions { direction: [0.0, 0.0, 1.0], color: [1.0; 3], intensity: 1.0 }; 8]);
    options.punctual_lights = Some(vec![options.punctual_lights.as_ref().unwrap()[0]; 8]);
    options.area_lights = Some(vec![
        job::RectangleLightOptions {
            center_mm: [0.0, 0.0, 10.0],
            edge_u_mm: [8.0, 0.0, 0.0],
            edge_v_mm: [0.0, -0.5, 0.0],
            color: [1.0, 0.5, 0.25],
            intensity: 2.0
        };
        4
    ]);
    options.area_samples = Some(n);
    let mut request = previous.request.clone();
    request.raytrace = Some(options.clone());
    ImageTask::new(previous.scene.clone(), request, CancelToken::default(), options).unwrap()
}

#[test]
fn max_union_observes_272_direct_attempts_in_one_budget_and_partition_independent_png() {
    for n in [1, 4, 16, 64] {
        let mut task = mixed(n, false);
        let mut point_attempts = 0;
        let mut area_attempts = 0;
        let mut actual = None;
        for _ in 0..100_000 {
            if let Some(p) = task.pending.as_ref().filter(|p| p.hit.is_some() && p.light == 8) {
                if p.punctual_index < 8 && !p.punctual_ready && !p.punctual_shadow {
                    point_attempts += 1;
                }
                if p.punctual_index == 8 && p.area_light < 4 && p.area_index < n && !p.area_ready && !p.area_shadow {
                    area_attempts += 1;
                }
            }
            let before = task.work.used();
            let poll = task.step(1).unwrap();
            assert!(task.work.used() - before <= 1);
            if let TaskPoll::Complete(png) = poll {
                actual = Some(png);
                break;
            }
        }
        let actual = actual.unwrap();
        assert_eq!(point_attempts, 8);
        assert_eq!(area_attempts, 4 * n);
        assert_eq!(8 + point_attempts + area_attempts, 16 + 4 * n);
        if n == 64 {
            assert_eq!(8 + point_attempts + area_attempts, 272);
        }
        let mut other = mixed(n, false);
        let mut complete = None;
        for _ in 0..100_000 {
            if let TaskPoll::Complete(png) = other.step(128).unwrap() {
                complete = Some(png);
                break;
            }
        }
        assert_eq!(actual, complete.unwrap());
    }
}

#[test]
fn mixed_area_final_shadow_cancellation_and_shared_exhaustion_prevent_publication() {
    for cancel in [false, true] {
        let mut task = mixed(64, true);
        let mut reached = false;
        for _ in 0..100_000 {
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
            if task.pending.as_ref().is_some_and(|p| p.area_light == 3 && p.area_index == 63 && p.area_ready) {
                reached = true;
                assert!(task.pixels.is_empty());
                let before = task.work.used();
                if cancel {
                    task.cancel.cancel();
                } else {
                    task.work.limit = before;
                }
                assert_eq!(task.step(1).err().unwrap().code, if cancel { ErrorCode::Cancelled } else { ErrorCode::LimitExceeded });
                assert_eq!(task.work.used(), before);
                assert!(task.pixels.is_empty());
                break;
            }
        }
        assert!(reached, "last rectangle sample continuation was not reached");
    }
}

#[test]
fn mixed_normal_incidence_radiance_is_summed_before_single_display_transform() {
    let mut task = mixed(1, false);
    let options = &mut task.options;
    for point in options.punctual_lights.as_mut().unwrap() {
        point.position_mm = [0.0, 0.0, 10.0];
        point.color = [1.0; 3];
        point.reference_distance_mm = 10.0;
    }
    for area in options.area_lights.as_mut().unwrap() {
        area.color = [1.0; 3];
    }
    task.request.raytrace = Some(options.clone());
    // White rough conductor at normal incidence: f*cos=1/(4*pi).
    // Eight unit directionals + eight unit point sources + four Le*A/r²=.08 rectangles.
    let linear = (8.0 + 8.0 + 4.0 * 0.08) / (4.0 * std::f64::consts::PI);
    let expected = encode(linear / (1.0 + linear));
    assert_eq!(render(task), [expected, expected, expected, 255]);
}
