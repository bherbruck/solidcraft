use super::*;
use crate::ProductMesh;
use crate::job::{ReflectionBody, ReflectionMode, ReflectionOptions};
use solidcraft_geom::Mesh;
use std::sync::Arc;
fn scene() -> ProductScene {
    fn quad(name: &str, positions: Vec<Vec3>, normal: Vec3, color: [u8; 3], surface: Option<OpaqueSurface>) -> ProductMesh {
        ProductMesh {
            name: name.into(),
            mesh: Arc::new(Mesh {
                positions,
                normals: vec![normal; 4],
                triangles: vec![[0, 1, 2], [0, 2, 3]],
                tri_face: vec![0, 0],
                ..Default::default()
            }),
            color,
            opacity: 1.0,
            surface,
            faces: Vec::new(),
        }
    }
    let mirror = quad(
        "Mirror",
        vec![Vec3::new(-20.0, -20.0, -20.0), Vec3::new(20.0, -20.0, -20.0), Vec3::new(20.0, 20.0, 20.0), Vec3::new(-20.0, 20.0, 20.0)],
        Vec3::new(0.0, -1.0, 1.0),
        [255; 3],
        Some(OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 0.2, 1.5).unwrap()),
    );
    let object = quad(
        "Orange target",
        vec![Vec3::new(-8.0, -8.0, 10.0), Vec3::new(-8.0, 8.0, 10.0), Vec3::new(8.0, 8.0, 10.0), Vec3::new(8.0, -8.0, 10.0)],
        -Vec3::Z,
        [240, 80, 20],
        None,
    );
    let meshes = vec![mirror, object];
    let bounds = meshes.iter().fold(solidcraft_geom::Aabb3::EMPTY, |b, m| b.union(&m.mesh.bounds()));
    ProductScene { meshes, bounds }
}
fn request(reflections: Option<ReflectionOptions>) -> ImageRequest {
    let mut camera = crate::Camera { distance: 50.0, ..Default::default() };
    camera.set_view(crate::StandardView::Front);
    ImageRequest {
        integrator: job::Integrator::Raytrace,
        raytrace: Some(RaytraceOptions {
            reflections,
            shadows: false,
            lights: Some(vec![job::DirectionalLightOptions { direction: [0.0, 0.0, -1.0], color: [1.0; 3], intensity: 2.0 }]),
            ..Default::default()
        }),
        camera,
        width: 9,
        height: 9,
        background: [13, 27, 41],
        transparent_background: false,
    }
}
fn reflection_options(mode: ReflectionMode) -> ReflectionOptions {
    ReflectionOptions { bodies: vec![ReflectionBody { body: "Mirror".into(), mode }], max_bounces: 2, samples: 16 }
}
fn render(request: ImageRequest) -> Vec<u8> {
    let options = request.raytrace.clone().unwrap();
    let mut task = ImageTask::new(scene(), request, CancelToken::default(), options).unwrap();
    for _ in 0..200_000 {
        if let TaskPoll::Complete(png) = task.step(16384).unwrap() {
            return png;
        }
    }
    panic!("bounded reflection fixture did not complete");
}
#[test]
fn mirror_png_contains_other_body_and_rough_transport_is_reproducible() {
    let baseline = ::image::load_from_memory(&render(request(None))).unwrap().to_rgba8();
    let mirrored = ::image::load_from_memory(&render(request(Some(reflection_options(ReflectionMode::Mirror))))).unwrap().to_rgba8();
    let b = baseline.get_pixel(4, 4).0;
    let p = mirrored.get_pixel(4, 4).0;
    assert!(p[0] > p[1] && p[1] > p[2], "actual orange object must be reflected: {p:?}");
    assert_ne!(p, b, "reflection must change the scene image, not only metadata");
    let rough = request(Some(reflection_options(ReflectionMode::RoughSpecular)));
    assert_eq!(render(rough.clone()), render(rough));
}
#[test]
fn explicit_empty_overrides_preserve_baseline_bytes_and_validate_names_atomically() {
    assert_eq!(render(request(None)), render(request(Some(ReflectionOptions::default()))));
    let mut options = reflection_options(ReflectionMode::Mirror);
    options.bodies[0].body = "missing".into();
    let request = request(Some(options));
    assert_eq!(admit(&scene(), &request, request.raytrace.as_ref().unwrap()).err().unwrap().code, ErrorCode::InvalidRequest);
    let hostile = ReflectionOptions { bodies: Vec::with_capacity(65), ..ReflectionOptions::default() };
    assert!(hostile.validate().is_err());
}
#[test]
fn reflection_cancel_and_work_exhaustion_never_publish_partial_frame() {
    for mode in [ReflectionMode::Mirror, ReflectionMode::RoughSpecular] {
        for cancel_now in [false, true] {
            let request = request(Some(reflection_options(mode)));
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
            assert!(reached);
            if cancel_now {
                cancel.cancel();
                assert_eq!(task.step(1).err().unwrap().code, ErrorCode::Cancelled);
            } else {
                task.work.limit = task.work.used();
                assert_eq!(task.step(1).err().unwrap().code, ErrorCode::LimitExceeded);
            }
        }
    }
}
#[test]
fn prepared_hit_preserves_original_winding_identity_and_never_certifies_glass() {
    let source = scene();
    let primitive = &source.meshes[0];
    let indices = primitive.mesh.triangles[0];
    let vertices = indices.map(|i| primitive.mesh.positions[i as usize]);
    let triangle = Triangle::new(vertices).unwrap();
    let ray = Ray::new(Vec3::new(2.0, -50.0, 0.0), Vec3::Y, 0.0, 100.0).unwrap();
    let geometry = triangle.intersect(&ray).unwrap();
    let cancel = CancelToken::default();
    let mut builder = BvhBuilder::new(vec![triangle], limits(), &cancel).unwrap();
    while !builder.step(1, &cancel).unwrap().complete {}
    let bvh = builder.finish().unwrap();
    let primitives =
        [PreparedPrimitive { body_index: 0, face: Some(0), vertices, normals: [geometry.geometric_normal; 3], surface: primitive.surface.unwrap() }];
    let prepared = PreparedScene { bvh: &bvh, primitives: &primitives, source: &source };
    let shared = prepared.resolve_hit(ray, super::super::Hit { primitive_index: 0, geometry }).unwrap();
    assert_eq!(shared.body_name, "Mirror");
    assert_eq!(shared.geometric_normal, geometry.geometric_normal);
    assert_eq!(shared.front_face, geometry.front_face);
    assert_eq!(prepared.boundary_status(0), super::super::prepared::BoundaryStatus::Unknown);
}
#[test]
fn shared_spawn_excludes_signed_zero_self_hit_without_skipping_thin_blockers() {
    for x in [0.0, 1e12 - 100.0] {
        let vertices = [Vec3::new(x, 0.0, 0.0), Vec3::new(x + 20.0, 0.0, 0.0), Vec3::new(x, 20.0, 0.0)];
        let triangle = Triangle::new(vertices).unwrap();
        let ray = Ray::new(Vec3::new(x + 5.0, 5.0, 10.0), -Vec3::Z, 0.0, 20.0).unwrap();
        let geometry = triangle.intersect(&ray).unwrap();
        let primitive = PreparedPrimitive {
            body_index: 0,
            face: Some(0),
            vertices,
            normals: [Vec3::Z; 3],
            surface: OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 0.0, 0.7, 1.5).unwrap(),
        };
        let source = ProductScene {
            meshes: vec![ProductMesh {
                name: "Plane".into(),
                mesh: Arc::new(Mesh::default()),
                color: [255; 3],
                opacity: 1.0,
                surface: None,
                faces: Vec::new(),
            }],
            bounds: Default::default(),
        };
        let cancel = CancelToken::default();
        let mut builder = BvhBuilder::new(vec![triangle], limits(), &cancel).unwrap();
        while !builder.step(1, &cancel).unwrap().complete {}
        let bvh = builder.finish().unwrap();
        let primitives = [primitive];
        let prepared = PreparedScene { bvh: &bvh, primitives: &primitives, source: &source };
        let hit = prepared.resolve_hit(ray, super::super::Hit { primitive_index: 0, geometry }).unwrap();
        let secondary = hit.spawn(Vec3::Z).unwrap();
        assert!(secondary.interval()[0] > 0.0);
        assert!(triangle.intersect(&secondary).is_none());
        assert!(Triangle::new(vertices.map(|v| v + Vec3::Z * 0.005)).unwrap().intersect(&secondary).is_some());
    }
}
