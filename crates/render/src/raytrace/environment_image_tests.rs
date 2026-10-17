use super::*;
use crate::lighting::{HdrSource, environment_input::EnvironmentInput};
use solidcraft_geom::{Aabb3, Mesh};
use std::sync::Arc;

fn input() -> EnvironmentInput {
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nPRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n\n-Y 2 +X 4\n".to_vec();
    for _ in 0..8 {
        bytes.extend_from_slice(&[128, 64, 32, 129]);
    }
    EnvironmentInput::new(HdrSource::new(bytes).unwrap(), 0.0, 1.0, 64).unwrap()
}
fn scene() -> ProductScene {
    let mesh = Arc::new(Mesh {
        positions: vec![Vec3::new(-10.0, -10.0, 0.0), Vec3::new(10.0, -10.0, 0.0), Vec3::new(10.0, 10.0, 0.0), Vec3::new(-10.0, 10.0, 0.0)],
        normals: vec![Vec3::Z; 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        tri_face: vec![0, 0],
        ..Default::default()
    });
    ProductScene {
        bounds: mesh.bounds(),
        meshes: vec![crate::ProductMesh {
            name: "Original conductor plate".into(),
            mesh,
            color: [255; 3],
            opacity: 1.0,
            surface: Some(OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 1.0, 1.5).unwrap()),
            faces: Vec::new(),
        }],
    }
}
fn request() -> ImageRequest {
    let mut camera = crate::Camera { distance: 50.0, ..Default::default() };
    camera.set_view(crate::StandardView::Top);
    ImageRequest {
        integrator: job::Integrator::Raytrace,
        raytrace: Some(RaytraceOptions { lights: Some(Vec::new()), ..Default::default() }),
        camera,
        width: 9,
        height: 9,
        background: [13, 27, 41],
        transparent_background: false,
    }
}
fn png(source: ProductScene, environment: Option<EnvironmentInput>) -> Vec<u8> {
    let request = request();
    let options = request.raytrace.clone().unwrap();
    let mut task = ImageTask::new_with_environment(source, request, CancelToken::default(), options, environment).unwrap();
    for _ in 0..200_000 {
        if let TaskPoll::Complete(png) = task.step(16384).unwrap() {
            return png;
        }
    }
    panic!("bounded environment fixture did not complete");
}

#[test]
fn real_cpu_png_conductor_matches_independent_environment_quadrature_once() {
    let baseline = ::image::load_from_memory(&png(scene(), None)).unwrap().to_rgba8();
    let rendered = png(scene(), Some(input()));
    assert_eq!(rendered, png(scene(), Some(input())));
    let image = ::image::load_from_memory(&rendered).unwrap().to_rgba8();
    // Independent roughness-one white-conductor f*cos= z/(2*pi*(1+z)).
    // The fixed 8x8 solid-angle strata integrate 4 positive z rows, each 8 samples.
    let factor = [0.125, 0.375, 0.625, 0.875].into_iter().map(|z| z / (1.0 + z)).sum::<f64>() / 4.0;
    let expected = [factor, factor * 0.5, factor * 0.25].map(|r| encode(r / (1.0 + r)));
    assert_eq!(image.get_pixel(4, 4).0, [expected[0], expected[1], expected[2], 255]);
    assert_eq!(baseline.get_pixel(4, 4).0, [0, 0, 0, 255]);
    let empty = ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY };
    let background = ::image::load_from_memory(&png(empty, Some(input()))).unwrap().to_rgba8();
    assert!(background.pixels().all(|p| p.0 == [13, 27, 41, 255]));
}

#[test]
fn environment_decode_and_secondary_visibility_share_cancellation_and_work_budget() {
    for during_visibility in [false, true] {
        let request = request();
        let options = request.raytrace.clone().unwrap();
        let cancel = CancelToken::default();
        let mut task = ImageTask::new_with_environment(scene(), request, cancel.clone(), options, Some(input())).unwrap();
        if during_visibility {
            for _ in 0..100_000 {
                assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
                if task.pending.as_ref().is_some_and(|p| p.environment_cursor.is_some()) {
                    break;
                }
            }
            assert!(task.pending.as_ref().is_some_and(|p| p.environment_cursor.is_some()));
        }
        // Cancellation must win before any image publication, including decode preparation.
        cancel.cancel();
        assert_eq!(task.step(1).err().unwrap().code, ErrorCode::Cancelled);
    }
    let request = request();
    let options = request.raytrace.clone().unwrap();
    let mut task = ImageTask::new_with_environment(scene(), request, CancelToken::default(), options, Some(input())).unwrap();
    task.work = WorkBudget::new(1).unwrap();
    assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
    assert_eq!(task.step(1).err().unwrap().code, ErrorCode::LimitExceeded);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_host_captured_source_completes_and_releases_work_lease() {
    let host = job::CpuHost::for_platform().unwrap();
    assert!(host.descriptor_for(job::Integrator::Raytrace).unwrap().profiles[0].features.contains(&crate::RenderFeature::Hdri));
    assert!(!host.descriptor_for(job::Integrator::Raster).unwrap().profiles[0].features.contains(&crate::RenderFeature::Hdri));
    let handle = host.submit_with_environment(job::JobId { session: 1693, serial: 1, generation: 1 }, 1, scene(), request(), Some(input())).unwrap();
    assert_eq!(handle.wait_timeout(std::time::Duration::from_secs(60)).unwrap().progress.phase, Phase::Completed);
    let frame = handle.frame().unwrap().unwrap();
    assert!(host.admitted_bytes() >= frame.png().len() as u64 + 1024);
    drop(host);
    drop(frame);
    drop(handle);
}

#[test]
fn environment_visibility_respects_four_work_units_per_quantum_and_cancel() {
    let request = request();
    let options = request.raytrace.clone().unwrap();
    let cancel = CancelToken::default();
    let mut task = ImageTask::new_with_environment(scene(), request, cancel.clone(), options, Some(input())).unwrap();
    for _ in 0..100_000 {
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        if task.pending.as_ref().is_some_and(|p| p.environment_cursor.is_some()) {
            break;
        }
    }
    assert!(task.pending.as_ref().is_some_and(|p| p.environment_cursor.is_some()));
    for _ in 0..32 {
        let before = task.work.used();
        assert!(matches!(task.step(4).unwrap(), TaskPoll::Pending(_)));
        assert!(task.work.used() - before <= 4, "visibility continuation double-charged shading and traversal");
    }
    cancel.cancel();
    assert_eq!(task.step(4).err().unwrap().code, ErrorCode::Cancelled);
}

#[test]
fn all_pixel_traversal_to_shading_transitions_debit_at_most_one_unit() {
    for (directional, shadows) in [(false, true), (true, true), (true, false)] {
        let mut request = request();
        let mut options = request.raytrace.clone().unwrap();
        options.shadows = shadows;
        if directional {
            options.lights = Some(vec![job::DirectionalLightOptions { direction: [0.0, 0.0, 1.0], color: [1.0; 3], intensity: 1.0 }]);
        }
        request.raytrace = Some(options.clone());
        let cancel = CancelToken::default();
        let mut task = ImageTask::new_with_environment(scene(), request, cancel.clone(), options, Some(input())).unwrap();
        while task.bvh.is_none() {
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        }
        let mut primary_transitions = 0;
        let mut directional_transitions = 0;
        for _ in 0..200_000 {
            if task.pixel == 81 {
                break;
            }
            let primary = task.pending.as_ref().is_some_and(|p| p.hit.is_none());
            let light_before = task.pending.as_ref().map(|p| p.light);
            let before = task.work.used();
            task.pixel_one().unwrap();
            assert!(task.work.used() - before <= 1, "traversal completion must yield before shading/sample debit");
            if primary && task.pending.as_ref().is_some_and(|p| p.hit.is_some()) {
                primary_transitions += 1;
            }
            if light_before == Some(0) && task.pending.as_ref().is_some_and(|p| p.light == 1) {
                directional_transitions += 1;
            }
        }
        assert_eq!(task.pixel, 81);
        assert!(primary_transitions > 0);
        if directional && shadows {
            assert!(directional_transitions > 0);
        }
        cancel.cancel();
        assert_eq!(task.step(4).err().unwrap().code, ErrorCode::Cancelled);
    }
}
