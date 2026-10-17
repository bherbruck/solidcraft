use super::*;
use crate::job::{ReflectionBody, ReflectionMode, ReflectionOptions};
use crate::lighting::{HdrSource, environment_input::EnvironmentInput};
use solidcraft_geom::{Aabb3, Mesh};
use std::sync::Arc;

fn environment() -> EnvironmentInput {
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nPRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n\n-Y 2 +X 4\n".to_vec();
    for _ in 0..8 {
        bytes.extend_from_slice(&[128, 64, 32, 129]);
    }
    EnvironmentInput::new(HdrSource::new(bytes).unwrap(), 0.0, 1.0, 4).unwrap()
}

fn scene(face_override: bool) -> ProductScene {
    let mut meshes = Vec::new();
    for (name, x) in [("Visible", 0.0), ("Hidden", 1000.0)] {
        let mesh = Mesh {
            positions: vec![
                Vec3::new(x - 20.0, -20.0, 0.0),
                Vec3::new(x + 20.0, -20.0, 0.0),
                Vec3::new(x + 20.0, 20.0, 0.0),
                Vec3::new(x - 20.0, 20.0, 0.0),
            ],
            normals: vec![Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![0, 0],
            ..Default::default()
        };
        meshes.push(crate::ProductMesh {
            name: name.into(),
            mesh: Arc::new(mesh),
            color: [255; 3],
            opacity: 1.0,
            surface: Some(OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 1.0, 1.5).unwrap()),
            faces: Vec::new(),
        });
    }
    if face_override {
        meshes[0].faces.push(crate::FaceAppearance {
            face: 0,
            color: [0; 3],
            opacity: 1.0,
            surface: Some(OpaqueSurface::new(Vec3::ZERO, 1.0, 1.0, 1.5).unwrap()),
        });
    }
    let bounds = meshes.iter().fold(Aabb3::EMPTY, |b, m| b.union(&m.mesh.bounds()));
    ProductScene { meshes, bounds }
}

fn task(samples: u8, selected: bool, face_override: bool, shadows: bool) -> ImageTask {
    let mut camera = crate::Camera { distance: 50.0, ..Default::default() };
    camera.set_view(crate::StandardView::Top);
    let options = RaytraceOptions {
        lights: Some(Vec::new()),
        shadows,
        reflections: Some(ReflectionOptions {
            bodies: vec![ReflectionBody { body: if selected { "Visible" } else { "Hidden" }.into(), mode: ReflectionMode::RoughSpecular }],
            max_bounces: 2,
            samples,
        }),
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
    ImageTask::new_with_environment(scene(face_override), request, CancelToken::default(), options, Some(environment())).unwrap()
}

fn render(mut task: ImageTask, revoke_fixed_primary_proof: bool) -> (Vec<u8>, u64, usize) {
    let mut primaries = 0;
    for _ in 0..200_000 {
        // Test-only reference: revoking the generator proof keeps the original R
        // path/accumulation. No product switch, sampler, work limit or input changes.
        if revoke_fixed_primary_proof && let Some(p) = task.pending.as_mut() {
            p.repeatable_primary = false;
        }
        let absent = task.pending.is_none();
        match task.step(1).unwrap() {
            TaskPoll::Complete(png) => return (png, task.work.used(), primaries),
            TaskPoll::Pending(_) => {
                if absent && task.pending.is_some() {
                    primaries += 1;
                }
            }
        }
    }
    panic!("bounded single-pixel fixture did not finish");
}

#[test]
fn nonselected_fixed_primary_matches_analytic_color_and_legacy_r_with_less_work() {
    for samples in [1, 4, 16, 64] {
        for shadows in [false, true] {
            let (fast, work, primaries) = render(task(samples, false, false, shadows), false);
            let (legacy, legacy_work, legacy_primaries) = render(task(samples, false, false, shadows), true);
            assert_eq!(fast, legacy, "same radiance without repeated identical primary samples");
            assert_eq!(primaries, 1);
            assert_eq!(legacy_primaries, usize::from(samples));
            if samples == 1 {
                assert_eq!(work, legacy_work);
            } else {
                assert!(work < legacy_work);
            }
            if !shadows {
                // White roughness-one conductor, constant RGB(1,.5,.25), fixed
                // 2x2 solid-angle strata: two z=.5 contributions average to1/3.
                let expected = [1.0 / 3.0, 1.0 / 6.0, 1.0 / 12.0].map(|v| encode(v / (1.0 + v)));
                let image = ::image::load_from_memory(&fast).unwrap().to_rgba8();
                assert_eq!(image.get_pixel(0, 0).0, [expected[0], expected[1], expected[2], 255]);
            }
        }
    }
}

#[test]
fn selected_rough_primary_retains_every_sample_including_null_attempts() {
    for samples in [4, 16, 64] {
        let (actual, work, primaries) = render(task(samples, true, false, false), false);
        let (reference, reference_work, reference_primaries) = render(task(samples, true, false, false), true);
        assert_eq!(actual, reference);
        assert_eq!(work, reference_work);
        assert_eq!(primaries, usize::from(samples));
        assert_eq!(primaries, reference_primaries);
    }
}

#[test]
fn captured_face_override_is_resolved_before_nonselected_fast_path_finishes() {
    let (png, work, count) = render(task(64, false, true, false), false);
    let (reference, reference_work, reference_count) = render(task(64, false, true, false), true);
    assert_eq!(png, reference);
    assert_eq!(::image::load_from_memory(&png).unwrap().to_rgba8().get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert!(work < reference_work);
    assert_eq!(count, 1);
    assert_eq!(reference_count, 64);
}

#[test]
fn nonselected_visibility_keeps_shared_cancel_and_work_limit_refusal() {
    for cancel_now in [false, true] {
        let mut task = task(64, false, false, true);
        for _ in 0..10000 {
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
            if task.pending.as_ref().is_some_and(|p| p.environment_cursor.is_some()) {
                break;
            }
        }
        assert!(task.pending.as_ref().is_some_and(|p| p.environment_cursor.is_some()));
        if cancel_now {
            task.cancel.cancel();
        } else {
            task.work.limit = task.work.used();
        }
        assert_eq!(task.step(1).err().unwrap().code, if cancel_now { ErrorCode::Cancelled } else { ErrorCode::LimitExceeded });
        assert!(!task.done);
    }
}

#[test]
fn selected_sampling_cancellation_does_not_turn_partial_samples_into_png() {
    let mut task = task(64, true, false, false);
    for _ in 0..10000 {
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        if task.sample >= 1 {
            break;
        }
    }
    assert!(task.sample >= 1 && task.sample < 64);
    task.cancel.cancel();
    assert_eq!(task.step(1).err().unwrap().code, ErrorCode::Cancelled);
    assert!(!task.done);
}

#[test]
fn reused_shading_preserves_original_per_add_radiance_bound() {
    let mut task = task(64, false, false, false);
    for _ in 0..10000 {
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        if task.pending.as_ref().is_some_and(|p| p.hit.is_some() && p.repeatable_primary) {
            break;
        }
    }
    assert!(task.pending.as_ref().is_some_and(|p| p.hit.is_some() && p.repeatable_primary));
    let error = task.finish_sample(Vec3::new(1e12 / 32.0, 0.0, 0.0)).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidScene);
    assert_eq!(task.sample, 32);
    assert!(!task.done);
    assert!(task.pixels.is_empty());
}

#[test]
fn request_admission_and_task_slot_caps_are_not_expanded() {
    let fast = task(64, false, false, false);
    let legacy = task(64, false, false, false);
    assert_eq!(fast.options, legacy.options);
    assert_eq!(fast.admission.estimated_total_bytes, legacy.admission.estimated_total_bytes);
    assert_eq!(fast.options.reflections.as_ref().unwrap().samples, 64);
    assert!(std::mem::size_of::<ImageTask>() <= 4096);
    assert_eq!(MAX_WORK, 1_000_000_000);
    assert_eq!(job::image::MAX_JOB_BYTES, 384 * 1024 * 1024);
}
