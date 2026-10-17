use super::*;
use crate::{Camera, ProductMesh, StandardView};
use solidcraft_geom::{Aabb3, Mesh};

fn request() -> ImageRequest {
    let mut camera = Camera { distance: 50.0, ..Default::default() };
    camera.set_view(StandardView::Front);
    ImageRequest { camera, width: 64, height: 64, background: [20, 40, 60], transparent_background: true }
}

fn square(name: &str, colour: [u8; 3], opacity: f64, offset: Vec3) -> ProductMesh {
    let mesh = Mesh {
        positions: vec![
            Vec3::new(-10.0, 0.0, -10.0) + offset,
            Vec3::new(10.0, 0.0, -10.0) + offset,
            Vec3::new(10.0, 0.0, 10.0) + offset,
            Vec3::new(-10.0, 0.0, 10.0) + offset,
        ],
        normals: vec![Vec3::new(0.0, -1.0, 0.0); 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        tri_face: vec![0, 1],
        ..Default::default()
    };
    ProductMesh { name: name.into(), mesh: Arc::new(mesh), color: colour, opacity, surface: None, faces: Vec::new() }
}

fn scene(meshes: Vec<ProductMesh>) -> ProductScene {
    let bounds = meshes.iter().fold(Aabb3::EMPTY, |b, m| b.union(&m.mesh.bounds()));
    ProductScene { meshes, bounds }
}

fn finish(mut task: ImageTask) -> Vec<u8> {
    for _ in 0..100_000 {
        match task.step(4096).unwrap() {
            TaskPoll::Complete(png) => return png,
            TaskPoll::Pending(p) => assert!(p.done <= p.total, "{p:?}"),
        }
    }
    panic!("bounded fixture did not finish");
}

#[test]
fn transparent_png_has_straight_srgb_colour_without_a_shared_edge_seam() {
    let scene = scene(vec![square("Glass", [220, 40, 20], 0.5, Vec3::ZERO)]);
    let png = finish(ImageTask::new(scene, request(), CancelToken::default()).unwrap());
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(image.get_pixel(0, 0).0, [0; 4]);
    for (x, y) in [(32, 32), (31, 31), (30, 32), (32, 30)] {
        let p = image.get_pixel(x, y).0;
        assert!((i16::from(p[3]) - 128).abs() <= 1, "a shared diagonal must not double-blend: {p:?}");
        assert!(p[0] > 180 && p[0] > p[1] + 60, "RGB is straight, not multiplied by 0.5: {p:?}");
    }
    let reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
    assert_eq!(reader.info().srgb, Some(png::SrgbRenderingIntent::Perceptual));
}

#[test]
fn glass_is_sorted_back_to_front_independently_of_body_order() {
    let request = request();
    let far = square("Far", [240, 20, 20], 0.5, request.camera.back() * -5.0);
    let near = square("Near", [20, 20, 240], 0.5, request.camera.back() * 5.0);
    let a = finish(ImageTask::new(scene(vec![far.clone(), near.clone()]), request.clone(), CancelToken::default()).unwrap());
    let b = finish(ImageTask::new(scene(vec![near, far]), request, CancelToken::default()).unwrap());
    assert_eq!(a, b);
    let image = image::load_from_memory(&a).unwrap().to_rgba8();
    let p = image.get_pixel(32, 32).0;
    assert!((i16::from(p[3]) - 192).abs() <= 1, "two half-transparent layers: {p:?}");
    assert!(p[2] > p[0] + 30, "the near blue layer contributes more: {p:?}");
}

#[test]
fn cancellation_is_observed_during_preparation_sort_pixels_and_encoding() {
    for stage in [Stage::Looks, Stage::Prepare, Stage::Sort, Stage::Clear, Stage::Glass, Stage::Downsample, Stage::Encode] {
        let cancel = CancelToken::default();
        let mut task = ImageTask::new(scene(vec![square("Glass", [40, 180, 100], 0.4, Vec3::ZERO)]), request(), cancel.clone()).unwrap();
        let mut reached = false;
        for _ in 0..100_000 {
            if task.stage == stage {
                reached = true;
                break;
            }
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        }
        assert!(reached);
        cancel.cancel();
        assert_eq!(task.step(4096).err().unwrap().code, ErrorCode::Cancelled);
    }
}

#[test]
fn encoding_yields_between_rows_and_completed_result_is_consumed_once() {
    let mut task = ImageTask::new(scene(Vec::new()), request(), CancelToken::default()).unwrap();
    while task.stage != Stage::Encode {
        assert!(matches!(task.step(64).unwrap(), TaskPoll::Pending(_)));
    }
    let before = task.row;
    assert!(matches!(task.step(usize::MAX).unwrap(), TaskPoll::Pending(_)));
    assert!(task.row - before <= 4);
    loop {
        if let TaskPoll::Complete(bytes) = task.step(4096).unwrap() {
            assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 64);
            break;
        }
    }
    assert_eq!(task.step(1).err().unwrap().code, ErrorCode::NotCompleted);
}

#[test]
fn invalid_sizes_indices_opacity_and_large_work_lists_fail_without_a_frame() {
    let body = square("Product", [100, 120, 140], 1.0, Vec3::ZERO);
    let mut bad_request = request();
    bad_request.width = u32::MAX;
    assert!(ImageTask::new(scene(vec![body.clone()]), bad_request, CancelToken::default()).is_err());
    for bad in [0, 1] {
        let mut body = body.clone();
        if bad == 0 {
            Arc::make_mut(&mut body.mesh).triangles[0][0] = u32::MAX;
        } else {
            body.faces.push(FaceAppearance { face: 0, color: [0; 3], opacity: f64::NAN, surface: None });
        }
        let mut task = ImageTask::new(scene(vec![body]), request(), CancelToken::default()).unwrap();
        let error = loop {
            if let Err(error) = task.step(64) {
                break error;
            }
        };
        assert_eq!(error.code, ErrorCode::InvalidScene);
    }
    let mut body = body;
    Arc::make_mut(&mut body.mesh).triangles.resize(MAX_TRIANGLES as usize + 1, [0, 1, 2]);
    assert_eq!(ImageTask::new(scene(vec![body]), request(), CancelToken::default()).err().unwrap().code, ErrorCode::LimitExceeded);
    assert_eq!(std::mem::size_of::<Draw>(), 16, "admission accounts for each of the three lists");
}

#[test]
fn png_output_budget_and_cancellation_fail_without_partial_publication() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let cancel = CancelToken::default();
    let mut writer = BufferWriter { bytes: bytes.clone(), limit: 4, cancel: cancel.clone() };
    writer.write_all(b"PNG").unwrap();
    assert!(writer.write_all(b"too large").is_err());
    assert_eq!(*bytes.lock().unwrap(), b"PNG");
    cancel.cancel();
    assert!(writer.write_all(b"x").is_err());
    assert!(writer.flush().is_err());
    assert_eq!(*bytes.lock().unwrap(), b"PNG");
}

#[test]
fn registered_executor_geometry_admission_excludes_the_cpu_raster_estimate() {
    // A library-bounded source: about 100 MiB of face appearances on a small mesh at the
    // largest image. Its CPU raster estimate is above 384 MiB at a registered executor's cap.
    let count = (100 * 1024 * 1024 / std::mem::size_of::<FaceAppearance>()).min(1_000_000);
    let mut body = square("appearances", [200; 3], 1.0, Vec3::ZERO);
    body.faces = vec![FaceAppearance { face: 0, color: [200; 3], opacity: 1.0, surface: None }; count];
    let source = scene(vec![body]);
    let mut r = request();
    r.width = 2048;
    r.height = 2048;
    let cpu = admit_raster(admit_source(&source, &r, 1_048_576).unwrap()).unwrap_err();
    assert_eq!(cpu.code, ErrorCode::LimitExceeded);
    assert!(cpu.message.contains("384 MiB"), "{}", cpu.message);
    let geometry = admit_geometry(&source, &r, 1_048_576).unwrap();
    assert_eq!((geometry.pixel_scratch_bytes, geometry.preparation_bytes), (0, 0));
    assert_eq!(geometry.estimated_total_bytes, geometry.geometry_bytes);
    assert_eq!(geometry.face_appearances, count as u64);
    // The CPU host itself keeps rejecting the source.
    assert_eq!(admit(&source, &r).unwrap_err().code, ErrorCode::LimitExceeded);
}
