use super::*;
use solidcraft_geom::Aabb3;
fn request() -> ImageRequest {
    ImageRequest {
        integrator: job::Integrator::Raytrace,
        raytrace: None,
        camera: Default::default(),
        width: 8,
        height: 8,
        background: [10, 20, 30],
        transparent_background: false,
    }
}
#[test]
fn image_work_exhaustion_is_typed_and_never_publishes_partial_png() {
    let mut task =
        ImageTask::new(ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY }, request(), CancelToken::default(), Default::default()).unwrap();
    task.work = WorkBudget::new(2).unwrap();
    let mut previous = 0;
    for _ in 0..20 {
        match task.step(1) {
            Ok(TaskPoll::Pending(_)) => {
                assert!(task.work.used() >= previous);
                previous = task.work.used();
            }
            Ok(TaskPoll::Complete(_)) => panic!("exhausted task published a frame"),
            Err(e) => {
                assert_eq!(e.code, ErrorCode::LimitExceeded);
                assert_eq!(task.work.used(), 2);
                return;
            }
        }
    }
    panic!("tiny budget did not stop image");
}
#[test]
fn thin_shadow_occluder_survives_large_unrelated_translation() {
    for x in [0.0, 1.0e12 - 100.0] {
        let vertices = [Vec3::new(x, 0.0, 0.0), Vec3::new(x + 20.0, 0.0, 0.0), Vec3::new(x, 20.0, 0.0)];
        let receiver = Triangle::new(vertices).unwrap();
        let primary = Ray::new(Vec3::new(x + 5.0, 5.0, 10.0), -Vec3::Z, 0.0, 20.0).unwrap();
        let geometry = receiver.intersect(&primary).unwrap();
        let shadow = shadow_ray(primary, super::super::Hit { primitive_index: 0, geometry }, Vec3::Z, vertices).unwrap();
        assert!(shadow.interval()[0] > 0.0, "secondary rays exclude signed-zero distance after underflow");
        assert!(receiver.intersect(&shadow).is_none(), "self-shadow at translation {x}");
        let occluder = Triangle::new(vertices.map(|v| v + Vec3::Z * 0.005)).unwrap();
        assert!(occluder.intersect(&shadow).is_some(), "thin blocker was skipped at translation {x}");
    }
}
#[test]
fn primary_intervals_follow_camera_clip_planes_for_both_projections_and_up_axes() {
    for fov in [0.0, 0.7] {
        for up in [crate::UpAxis::Z, crate::UpAxis::Y] {
            let mut request = request();
            request.camera.fov = fov;
            request.camera.up = up;
            let clip = request.camera.clip(20.0);
            let ray = primary_ray(&request, 4.0, 4.0, clip).unwrap();
            let forward = -request.camera.back();
            let near_depth = (ray.origin() - request.camera.eye()).dot(forward);
            let far_depth = (ray.at(ray.interval()[1]).unwrap() - request.camera.eye()).dot(forward);
            assert!((near_depth - clip.0).abs() < 1e-9);
            assert!((far_depth - clip.1).abs() < 1e-9);
        }
    }
}
#[test]
fn resumable_cursor_matches_original_visibility_and_charges_each_unit() {
    let cancel = CancelToken::default();
    let triangle = Triangle::new([Vec3::ZERO, Vec3::X, Vec3::Y]).unwrap();
    let mut builder = BvhBuilder::new(vec![triangle], limits(), &cancel).unwrap();
    while !builder.step(1, &cancel).unwrap().complete {}
    let bvh = builder.finish().unwrap();
    let ray = Ray::new(Vec3::new(0.2, 0.2, 1.0), -Vec3::Z, 0.0, 2.0).unwrap();
    let expected = bvh.closest_hit(&ray, &mut WorkBudget::new(100).unwrap(), &cancel).unwrap();
    let mut cursor = TraceCursor::new(ray, false);
    let mut work = WorkBudget::new(100).unwrap();
    for _ in 0..100 {
        let before = work.used();
        let result = cursor.step(&bvh, &mut work, &cancel).unwrap();
        assert_eq!(work.used(), before + 1);
        if let Some(actual) = result {
            assert_eq!(actual, expected);
            return;
        }
    }
    panic!("cursor failed to finish");
}
#[test]
fn actual_png_has_analytic_lit_and_cast_shadow_pixels() {
    use crate::ProductMesh;
    use solidcraft_geom::Mesh;
    use std::sync::Arc;
    fn plane(z: f64, x0: f64, x1: f64) -> ProductMesh {
        let mesh = Mesh {
            positions: vec![Vec3::new(x0, -10.0, z), Vec3::new(x1, -10.0, z), Vec3::new(x1, 10.0, z), Vec3::new(x0, 10.0, z)],
            normals: vec![Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![0, 0],
            ..Default::default()
        };
        ProductMesh {
            name: format!("plane {z}"),
            mesh: Arc::new(mesh),
            color: [255; 3],
            opacity: 1.0,
            surface: Some(OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 1.0, 1.5).unwrap()),
            faces: Vec::new(),
        }
    }
    let meshes = vec![plane(0.0, -20.0, 20.0), plane(5.0, -5.0, 5.0)];
    let bounds = meshes.iter().fold(Aabb3::EMPTY, |b, m| b.union(&m.mesh.bounds()));
    let scene = ProductScene { meshes, bounds };
    let mut request = request();
    request.width = 40;
    request.height = 40;
    request.camera.distance = 50.0;
    request.camera.set_view(crate::StandardView::Top);
    let options = RaytraceOptions {
        lights: Some(vec![job::DirectionalLightOptions { direction: [-1.0, 0.0, 1.0], color: [1.0; 3], intensity: 1.0 }]),
        ..Default::default()
    };
    fn render(scene: ProductScene, mut request: ImageRequest, options: RaytraceOptions) -> ::image::RgbaImage {
        request.raytrace = Some(options.clone());
        let mut task = ImageTask::new(scene, request, CancelToken::default(), options).unwrap();
        for _ in 0..100_000 {
            if let TaskPoll::Complete(png) = task.step(16384).unwrap() {
                return ::image::load_from_memory(&png).unwrap().to_rgba8();
            }
        }
        panic!("small analytic render did not complete");
    }
    let shadowed = render(scene.clone(), request.clone(), options.clone());
    let lit = render(scene, request, RaytraceOptions { shadows: false, ..options });
    // Top view: screen x maps world x, width40 spans[-20,20]. Blocker[-5,5]
    // at z5 casts a footprint[0,10] for the light toward(-1,0,+1).
    let shadow = shadowed.get_pixel(27, 20).0;
    let unshadowed = lit.get_pixel(27, 20).0;
    assert_eq!(shadow, [0, 0, 0, 255]);
    // White fully rough conductor: F=1, D=1/pi, Smith visibility
    // at normal view yields BRDF=1/[2*pi*(1+cosL)].
    let cosine = (0.5_f64).sqrt();
    let radiance = cosine / (2.0 * std::f64::consts::PI * (1.0 + cosine));
    let expected = encode(radiance / (1.0 + radiance));
    assert_eq!(unshadowed, [expected, expected, expected, 255]);
    assert_eq!(shadowed.get_pixel(7, 20), lit.get_pixel(7, 20), "unoccluded receiver changes when toggling shadows");
}
fn triangle_task(cancel: CancelToken) -> ImageTask {
    use std::sync::Arc;
    let mesh = solidcraft_geom::Mesh {
        positions: vec![Vec3::new(-20.0, -20.0, 0.0), Vec3::new(20.0, -20.0, 0.0), Vec3::new(0.0, 20.0, 0.0)],
        normals: vec![Vec3::Z; 3],
        triangles: vec![[0, 1, 2]],
        tri_face: vec![0],
        ..Default::default()
    };
    let bounds = mesh.bounds();
    let scene = ProductScene {
        meshes: vec![crate::ProductMesh {
            name: "work fixture".into(),
            mesh: Arc::new(mesh),
            color: [220; 3],
            opacity: 1.0,
            surface: None,
            faces: Vec::new(),
        }],
        bounds,
    };
    let mut request = request();
    request.camera.distance = 50.0;
    request.camera.set_view(crate::StandardView::Top);
    ImageTask::new(scene, request, cancel, Default::default()).unwrap()
}
#[test]
fn build_primary_and_shadows_share_cumulative_work_and_exhaust_atomically() {
    let mut task = triangle_task(CancelToken::default());
    let mut previous = 0;
    let mut built = false;
    for _ in 0..10_000 {
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        assert!(task.work.used() >= previous);
        previous = task.work.used();
        built |= task.builder.is_some();
        if task.pending.as_ref().is_some_and(|p| p.hit.is_some() && p.light < task.lights.lights().len()) {
            assert!(built);
            assert!(task.build_used > 0);
            assert!(task.work.used() > task.build_used);
            task.work.limit = task.work.used() + 1;
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
            assert_eq!(task.step(1).err().unwrap().code, ErrorCode::LimitExceeded);
            return;
        }
    }
    panic!("triangle fixture never reached shadow traversal");
}
#[test]
fn cancellation_is_observed_in_every_ray_image_phase() {
    for phase in 0..5 {
        let cancel = CancelToken::default();
        let mut task = triangle_task(cancel.clone());
        let mut reached = false;
        for _ in 0..100_000 {
            let current = match phase {
                0 => task.builder.is_none() && task.bvh.is_none(),
                1 => task.builder.is_some(),
                2 => task.pending.as_ref().is_some_and(|p| p.hit.is_none()),
                3 => task.pending.as_ref().is_some_and(|p| p.hit.is_some()),
                _ => task.encoder.is_some(),
            };
            if current {
                reached = true;
                break;
            }
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        }
        assert!(reached, "phase {phase}");
        cancel.cancel();
        assert_eq!(task.step(4).err().unwrap().code, ErrorCode::Cancelled);
    }
}
#[test]
fn runtime_presets_reach_the_lighting_consumer() {
    for (runtime, expected) in [
        (job::LightingPreset::Neutral, StudioPreset::Neutral),
        (job::LightingPreset::WarmCool, StudioPreset::WarmCool),
        (job::LightingPreset::KeyRim, StudioPreset::KeyRim),
    ] {
        let options = RaytraceOptions { preset: Some(runtime), ..Default::default() };
        assert_eq!(lighting(&options).unwrap(), Lighting::preset(expected).unwrap());
    }
    let conflict = RaytraceOptions { preset: Some(job::LightingPreset::Neutral), lights: Some(Vec::new()), ..Default::default() };
    assert_eq!(lighting(&conflict).err().unwrap().code, ErrorCode::InvalidRequest);
}
#[test]
fn warm_cool_preset_changes_actual_triangle_png() {
    fn render(preset: job::LightingPreset) -> Vec<u8> {
        let initial = triangle_task(CancelToken::default());
        let options = RaytraceOptions { preset: Some(preset), ..Default::default() };
        let mut request = initial.request.clone();
        request.raytrace = Some(options.clone());
        let mut task = ImageTask::new(initial.scene.clone(), request, CancelToken::default(), options).unwrap();
        for _ in 0..100_000 {
            if let TaskPoll::Complete(bytes) = task.step(16384).unwrap() {
                return bytes;
            }
        }
        panic!("preset render did not complete");
    }
    let neutral = ::image::load_from_memory(&render(job::LightingPreset::Neutral)).unwrap().to_rgba8();
    let warm = ::image::load_from_memory(&render(job::LightingPreset::WarmCool)).unwrap().to_rgba8();
    assert_ne!(neutral.get_pixel(4, 4), warm.get_pixel(4, 4));
    assert_eq!(neutral.get_pixel(0, 0), warm.get_pixel(0, 0), "background must remain independent of lighting preset");
}
