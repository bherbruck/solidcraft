use super::*;
use crate::job::{AnisotropicFaceOptions, CancelToken, ImageRequest, Integrator, TaskPoll};
use crate::{Camera, ProductMesh, StandardView};
use solidcraft_geom::Mesh;
use std::sync::Arc;

fn body(name: &str, x: f64) -> ProductMesh {
    ProductMesh {
        name: name.into(),
        mesh: Arc::new(Mesh {
            positions: vec![Vec3::new(x - 2.0, -2.0, 0.0), Vec3::new(x + 2.0, -2.0, 0.0), Vec3::new(x + 2.0, 2.0, 0.0), Vec3::new(x - 2.0, 2.0, 0.0)],
            normals: vec![Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![7, 7],
            ..Default::default()
        }),
        color: [255; 3],
        opacity: 1.0,
        surface: Some(OpaqueSurface::new(Vec3::new(0.8, 0.8, 0.8), 1.0, 0.5, 1.5).unwrap()),
        faces: Vec::new(),
    }
}

fn scene() -> ProductScene {
    let meshes = vec![body("left@occurrence", -5.0), body("right@occurrence", 0.0), body("scalar control@occurrence", 5.0)];
    let bounds = meshes.iter().map(|b| b.mesh.bounds()).reduce(|a, b| a.union(&b)).unwrap();
    ProductScene { meshes, bounds }
}

fn options() -> RaytraceOptions {
    let target = |body_index, name: &str, orientation| {
        Some(AnisotropicFaceOptions {
            body_index,
            body_name: name.into(),
            face: 7,
            tangent: [1.0, 0.0, 0.0],
            orientation,
            roughness_x: 0.25,
            roughness_y: 0.8,
        })
    };
    RaytraceOptions {
        anisotropic_faces: [
            target(0, "left@occurrence", AnisotropicOrientation::Zero),
            target(1, "right@occurrence", AnisotropicOrientation::QuarterTurn),
        ],
        lights: Some(vec![job::DirectionalLightOptions { direction: [0.6, 0.0, 0.8], color: [1.0; 3], intensity: 1.0 }]),
        shadows: false,
        ..Default::default()
    }
}

#[test]
fn explicit_capture_identity_face_and_frame_are_required() {
    let scene = scene();
    let baseline = options();
    assert!(prepare(&scene, &baseline, || Ok(())).is_ok());
    for change in 0..4 {
        let mut options = baseline.clone();
        let target = options.anisotropic_faces[0].as_mut().unwrap();
        match change {
            0 => target.body_name = "wrong occurrence".into(),
            1 => target.body_index = 50,
            2 => target.face = 99,
            _ => target.tangent = [0.0, 0.0, 1.0],
        }
        assert!(prepare(&scene, &options, || Ok(())).is_err());
    }
}

#[test]
fn curved_inconsistent_missing_and_excessive_target_meshes_refuse() {
    for change in 0..4 {
        let mut scene = scene();
        let mesh = Arc::make_mut(&mut scene.meshes[0].mesh);
        match change {
            0 => mesh.positions[3].z = 0.1,
            1 => mesh.triangles[1].swap(1, 2),
            2 => mesh.tri_face.clear(),
            _ => {
                mesh.triangles = vec![[0, 1, 2]; MAX_TARGET_BODY_TRIANGLES + 1];
                mesh.tri_face = vec![7; MAX_TARGET_BODY_TRIANGLES + 1];
            }
        }
        assert!(prepare(&scene, &options(), || Ok(())).is_err());
    }
}

#[test]
fn bounded_dto_roundtrip_duplicates_and_original_capacity() {
    let options = options();
    assert!(options.validate().is_ok());
    let encoded = serde_json::to_value(&options).unwrap();
    assert_eq!(serde_json::from_value::<RaytraceOptions>(encoded.clone()).unwrap(), options);
    let expected = options.anisotropic_faces.iter().flatten().map(|t| t.body_name.capacity() as u64).sum::<u64>()
        + options.lights.as_ref().unwrap().capacity() as u64 * std::mem::size_of::<job::DirectionalLightOptions>() as u64;
    assert_eq!(options.retained_bytes().unwrap(), expected);
    let mut duplicate = options.clone();
    duplicate.anisotropic_faces[1] = duplicate.anisotropic_faces[0].clone();
    assert!(duplicate.validate().is_err());
    let mut excessive_capacity = options.clone();
    let target = excessive_capacity.anisotropic_faces[0].as_mut().unwrap();
    let mut name = String::with_capacity(2048);
    name.push_str(&target.body_name);
    target.body_name = name;
    assert!(excessive_capacity.retained_bytes().is_err());
    let mut excessive_array = encoded;
    excessive_array["anisotropic_faces"] = serde_json::json!([null, null, null]);
    assert!(serde_json::from_value::<RaytraceOptions>(excessive_array).is_err());
    assert!(serde_json::from_value::<RaytraceOptions>(serde_json::json!({"anisotropic_faces":[{"body_index":0,"body_name":"x".repeat(1025),"face":7,"tangent":[1,0,0],"orientation":"zero","roughness_x":0.25,"roughness_y":0.8},null]})).is_err());
    assert!(serde_json::from_value::<RaytraceOptions>(serde_json::json!({"environment":{}})).is_err());
}

#[test]
fn target_certification_uses_caller_work_and_cancellation() {
    let mut charged = 0;
    prepare(&scene(), &options(), || {
        charged += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(charged, 4);
    let error = prepare(&scene(), &options(), || Err(JobError::new(ErrorCode::Cancelled, "cancelled"))).err().unwrap();
    assert_eq!(error.code, ErrorCode::Cancelled);
    let error = prepare(&scene(), &options(), || Err(JobError::new(ErrorCode::LimitExceeded, "same work budget"))).err().unwrap();
    assert_eq!(error.code, ErrorCode::LimitExceeded);
}

#[test]
fn direct_response_respects_geometry_visibility_and_quarter_turn() {
    let scene = scene();
    let options = options();
    let targets = prepare(&scene, &options, || Ok(())).unwrap();
    let lights =
        Lighting::new(vec![super::super::lighting::DirectionalLight::new(Vec3::new(0.6, 0.0, 0.8), Vec3::new(1.0, 1.0, 1.0), 1.0).unwrap()]).unwrap();
    let surface = scene.meshes[0].surface.unwrap();
    let first = targets[0].unwrap().shade(&lights, surface, Vec3::Z, Vec3::Z, |_, _| Ok(true)).unwrap();
    let turned = targets[1].unwrap().shade(&lights, surface, Vec3::Z, Vec3::Z, |_, _| Ok(true)).unwrap();
    assert!(turned.x > first.x + 0.1);
    assert_eq!(targets[0].unwrap().shade(&lights, surface, Vec3::Z, Vec3::Z, |_, _| Ok(false)).unwrap(), Vec3::ZERO);
    assert_eq!(targets[0].unwrap().shade(&lights, surface, Vec3::Z, -Vec3::Z, |_, _| Ok(true)).unwrap(), Vec3::ZERO);
}

#[test]
fn decoded_cpu_png_rotates_only_the_two_explicit_planar_targets() {
    fn render(scene: ProductScene, camera: Camera, options: RaytraceOptions) -> ::image::RgbaImage {
        let request = ImageRequest {
            integrator: Integrator::Raytrace,
            raytrace: Some(options.clone()),
            camera,
            width: 64,
            height: 32,
            background: [10, 20, 30],
            transparent_background: false,
        };
        let mut task = super::super::image::ImageTask::new(scene, request, CancelToken::default(), options).unwrap();
        for _ in 0..100_000 {
            if let TaskPoll::Complete(png) = task.step(16384).unwrap() {
                return ::image::load_from_memory(&png).unwrap().to_rgba8();
            }
        }
        panic!("bounded planar anisotropic PNG fixture failed to finish");
    }
    let scene = scene();
    let original = scene.meshes.iter().map(|b| b.mesh.positions.clone()).collect::<Vec<_>>();
    let mut camera = Camera::default();
    camera.set_view(StandardView::Top);
    camera.fit(&scene.bounds);
    let first = render(scene.clone(), camera, options());
    let mut rotated = options();
    rotated.anisotropic_faces[0].as_mut().unwrap().orientation = AnisotropicOrientation::QuarterTurn;
    rotated.anisotropic_faces[1].as_mut().unwrap().orientation = AnisotropicOrientation::Zero;
    let second = render(scene.clone(), camera, rotated);
    let radius = scene.bounds.diagonal().max(10.0) + scene.bounds.center().dist(camera.target);
    for (index, x) in [-5.0, 0.0, 5.0].into_iter().enumerate() {
        let (sx, sy, _) = camera.to_screen(Vec3::new(x, 0.0, 0.0), 64.0, 32.0, radius).unwrap();
        let a = first.get_pixel(sx.floor() as u32, sy.floor() as u32).0;
        let b = second.get_pixel(sx.floor() as u32, sy.floor() as u32).0;
        assert_ne!(a, [10, 20, 30, 255], "target centre missed fixture geometry");
        if index < 2 {
            assert!(i16::from(a[0]).abs_diff(i16::from(b[0])) > 20, "selected face failed to rotate: {a:?} {b:?}");
        } else {
            assert_eq!(a, b, "unselected scalar control changed");
        }
    }
    assert_eq!(first.get_pixel(0, 0).0, [10, 20, 30, 255]);
    assert_eq!(first.get_pixel(0, 0), second.get_pixel(0, 0));
    assert_eq!(original, scene.meshes.iter().map(|b| b.mesh.positions.clone()).collect::<Vec<_>>());
}

#[test]
fn reflection_bridge_preserves_anisotropic_pdf_weight_and_visual_rotation() {
    let targets = prepare(&scene(), &options(), || Ok(())).unwrap();
    let surface = scene().meshes[0].surface.unwrap();
    let a = targets[0].unwrap().sample(surface, Vec3::Z, Vec3::Z, [0.4, 0.2]).unwrap().unwrap();
    let b = targets[1].unwrap().sample(surface, Vec3::Z, Vec3::Z, [0.4, 0.2]).unwrap().unwrap();
    let core = AnisotropicSurface::new(surface.base_color(), surface.metallic(), 0.25, 0.8, surface.dielectric_ior()).unwrap();
    let reference = core.sample_reflection(VisualFrame::new(Vec3::Z, Vec3::X).unwrap(), Vec3::Z, Vec3::Z, [0.4, 0.2]).unwrap().unwrap();
    assert_eq!(a.direction, reference.direction);
    assert_eq!(a.pdf, reference.pdf);
    assert_eq!(a.weight, reference.weight);
    assert!((b.direction - Vec3::new(-a.direction.y, a.direction.x, a.direction.z)).len() < 1e-13);
    assert!((b.pdf - a.pdf).abs() < 1e-13);
    assert!(
        (core.brdf(VisualFrame::new(Vec3::Z, Vec3::X).unwrap(), Vec3::Z, Vec3::Z, a.direction).unwrap()
            - core.brdf(VisualFrame::new(Vec3::Z, Vec3::X).unwrap(), Vec3::Z, a.direction, Vec3::Z).unwrap())
        .len()
            < 1e-13
    );
}

#[test]
fn equal_axes_use_the_existing_isotropic_samples_and_null_mass() {
    let mut options = options();
    for input in options.anisotropic_faces.iter_mut().flatten() {
        input.roughness_x = 1.0;
        input.roughness_y = 1.0;
    }
    let targets = prepare(&scene(), &options, || Ok(())).unwrap();
    let surface = scene().meshes[0].surface.unwrap();
    let isotropic = OpaqueSurface::new(surface.base_color(), surface.metallic(), 1.0, surface.dielectric_ior()).unwrap();
    let mut accepted = 0;
    for i in 0..1024 {
        let u = [(i as f64 + 0.5) / 1024.0, 0.3];
        let a = targets[0].unwrap().sample(surface, Vec3::Z, Vec3::Z, u).unwrap();
        let b = super::super::reflection::rough_specular(isotropic, Vec3::Z, Vec3::Z, u).unwrap();
        match (a, b) {
            (Some(a), Some(b)) => {
                accepted += 1;
                assert_eq!(a.direction, b.direction);
                assert_eq!(a.pdf, b.pdf);
                assert_eq!(a.weight, b.weight);
                assert!((a.pdf - 1.0 / (4.0 * std::f64::consts::PI)).abs() < 1e-13);
            }
            (None, None) => {}
            _ => panic!("isotropic sample/null mismatch"),
        }
    }
    assert_eq!(accepted, 512); // Remaining half is null, never renormalized.
}
