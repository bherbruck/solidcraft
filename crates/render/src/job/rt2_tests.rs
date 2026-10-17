//! Runtime-option and admission regressions; no render timing assumptions.

use super::*;
use crate::ProductScene;
use solidcraft_geom::Aabb3;

fn request(integrator: Integrator) -> ImageRequest {
    ImageRequest { integrator, raytrace: None, camera: Default::default(), width: 16, height: 16, background: [0; 3], transparent_background: false }
}

#[test]
fn options_reject_retained_capacity_even_when_no_lights_are_active() {
    let options = RaytraceOptions { lights: Some(Vec::with_capacity(64)), ..Default::default() };
    assert_eq!(options.validate().unwrap_err().code, ErrorCode::InvalidRequest);
    let serialized = serde_json::to_value(&options).unwrap();
    let decoded: RaytraceOptions = serde_json::from_value(serialized).unwrap();
    assert!(decoded.validate().is_ok(), "wire input retains no oversized source allocation");
}

#[cfg(feature = "raytrace-cpu")]
#[test]
fn raytrace_admission_validates_retained_options_before_cloning() {
    let scene = ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY };
    let mut request = request(Integrator::Raytrace);
    request.raytrace = Some(RaytraceOptions { lights: Some(Vec::with_capacity(64)), ..Default::default() });
    let original = request.raytrace.as_ref().unwrap();
    assert_eq!(original.validate().unwrap_err().code, ErrorCode::InvalidRequest);
    assert!(original.clone().validate().is_ok(), "cloning an empty Vec discards its retained capacity");
    let error = admit(&scene, &request).err().unwrap();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(request.raytrace.as_ref().unwrap().lights.as_ref().unwrap().capacity(), 64);
}

#[test]
fn options_preserve_the_eight_light_boundary_and_refuse_the_ninth() {
    let light = serde_json::json!({"direction": [0.0, 0.0, -1.0], "color": [1.0, 1.0, 1.0], "intensity": 1.0});
    let accepted: RaytraceOptions = serde_json::from_value(serde_json::json!({"lights": vec![light.clone(); 8]})).unwrap();
    assert!(accepted.validate().is_ok());
    assert!(serde_json::from_value::<RaytraceOptions>(serde_json::json!({"lights": vec![light; 9]})).is_err());
}

#[test]
fn exposure_refuses_nonfinite_and_out_of_range_values() {
    for exposure_ev in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -16.01, 16.01] {
        let options = RaytraceOptions { exposure_ev, ..Default::default() };
        assert_eq!(options.validate().unwrap_err().code, ErrorCode::InvalidRequest);
    }
    for exposure_ev in [-16.0, 16.0] {
        assert!(RaytraceOptions { exposure_ev, ..Default::default() }.validate().is_ok());
    }
}

#[test]
fn rejected_raytrace_submission_leaves_existing_raster_reservation_intact() {
    let host = CpuHost::cooperative();
    let scene = || ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY };
    let raster = host.submit(JobId { session: 1, serial: 1, generation: 1 }, 1, scene(), request(Integrator::Raster)).unwrap();
    let before = host.admitted_bytes();
    assert!(before > 0);
    let error = host.submit(JobId { session: 1, serial: 2, generation: 2 }, 1, scene(), request(Integrator::Raytrace)).err().unwrap();
    assert_eq!(error.code, ErrorCode::HostUnavailable);
    assert_eq!(host.admitted_bytes(), before);
    assert_eq!(raster.status().unwrap().progress.phase, Phase::Queued);
    assert!(raster.frame().unwrap().is_none());
    assert_eq!(raster.cancel().unwrap().progress.phase, Phase::Cancelled);
    // Pump only to discard cancelled queued input; it must not produce a frame.
    assert!(!host.pump(1).unwrap());
    assert_eq!(host.admitted_bytes(), 0);
    assert!(raster.frame().unwrap().is_none());
}

#[cfg(all(feature = "raytrace-cpu", not(target_arch = "wasm32")))]
#[test]
fn native_integrators_share_one_host_and_retained_frame_budget() {
    let host = CpuHost::for_platform().unwrap();
    let scene = || ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY };
    let raster_id = JobId { session: 10, serial: 1, generation: 1 };
    let raytrace_id = JobId { session: 20, serial: 1, generation: 1 };
    let raster = host.submit(raster_id, 100, scene(), request(Integrator::Raster)).unwrap();
    let raytrace = host.submit(raytrace_id, 200, scene(), request(Integrator::Raytrace)).unwrap();
    // Deadlock watchdog only; there is no rendering-speed assertion.
    for job in [&raster, &raytrace] {
        assert_eq!(job.wait_timeout(std::time::Duration::from_secs(60)).unwrap().progress.phase, Phase::Completed);
    }
    let raster_frame = raster.frame().unwrap().unwrap();
    let raytrace_frame = raytrace.frame().unwrap().unwrap();
    assert_eq!(raster_frame.info.job, raster_id);
    assert_eq!(raytrace_frame.info.job, raytrace_id);
    assert_eq!(raster_frame.info.scene_generation, 100);
    assert_eq!(raytrace_frame.info.scene_generation, 200);
    assert_eq!(raster_frame.info.executor.as_str(), "cpu-product");
    assert_eq!(raytrace_frame.info.executor.as_str(), "cpu-raytrace");
    let both = host.admitted_bytes();
    assert!(both > 0);
    drop(raster);
    drop(raytrace);
    assert_eq!(host.admitted_bytes(), both, "external frames retain both leases");
    drop(raster_frame);
    assert!(host.admitted_bytes() > 0 && host.admitted_bytes() < both);
    drop(raytrace_frame);
    assert_eq!(host.admitted_bytes(), 0);
}

#[cfg(feature = "raytrace-cpu")]
#[test]
fn cancelled_raytrace_task_never_returns_a_frame() {
    let cancel = CancelToken::default();
    let scene = ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY };
    let mut task = crate::raytrace::image::ImageTask::new(scene, request(Integrator::Raytrace), cancel.clone(), RaytraceOptions::default()).unwrap();
    cancel.cancel();
    assert_eq!(task.step(4).err().unwrap().code, ErrorCode::Cancelled);
}

#[cfg(feature = "raytrace-cpu")]
#[test]
fn completed_raytrace_task_releases_captured_mesh_before_returning_png() {
    let mesh = Arc::new(solidcraft_geom::Mesh::default());
    let captured = Arc::downgrade(&mesh);
    let scene = ProductScene {
        meshes: vec![crate::ProductMesh { name: "Lease probe".into(), mesh, color: [0; 3], opacity: 1.0, surface: None, faces: Vec::new() }],
        bounds: Aabb3::EMPTY,
    };
    let mut task =
        crate::raytrace::image::ImageTask::new(scene, request(Integrator::Raytrace), CancelToken::default(), RaytraceOptions::default()).unwrap();
    assert!(captured.upgrade().is_some());
    let mut completed = false;
    for _ in 0..10_000 {
        if let TaskPoll::Complete(png) = task.step(4).unwrap() {
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
            completed = true;
            break;
        }
    }
    assert!(completed, "bounded empty-scene fixture must complete");
    assert!(captured.upgrade().is_none(), "the host may shrink the lease as soon as Complete is returned, even while the task remains alive");
    assert_eq!(task.progress().phase, Phase::Completed);
}
