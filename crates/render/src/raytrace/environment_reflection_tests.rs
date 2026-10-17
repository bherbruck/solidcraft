use super::*;
use crate::job::{ReflectionBody, ReflectionMode, ReflectionOptions};
use crate::lighting::{HdrSource, environment_input::EnvironmentInput};
use solidcraft_geom::Mesh;
use std::sync::Arc;

fn environment(bright: bool, rotation: f64) -> EnvironmentInput {
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nPRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n\n-Y 2 +X 4\n".to_vec();
    for i in 0..8 {
        bytes.extend_from_slice(&if bright { if i == 0 { [128, 64, 16, 133] } else { [0; 4] } } else { [128, 64, 32, 129] });
    }
    EnvironmentInput::new(HdrSource::new(bytes).unwrap(), rotation, 1.0, 64).unwrap()
}
fn scene(normal: Vec3) -> ProductScene {
    let normal = normal / normal.len();
    let u = normal.cross(Vec3::X);
    let u = u / u.len() * 10.0;
    let v = normal.cross(u);
    let mesh = Arc::new(Mesh {
        positions: vec![-u - v, u - v, u + v, -u + v],
        normals: vec![normal; 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        tri_face: vec![0, 0],
        ..Default::default()
    });
    ProductScene {
        bounds: mesh.bounds(),
        meshes: vec![crate::ProductMesh {
            name: "Metal".into(),
            mesh,
            color: [255; 3],
            opacity: 1.0,
            surface: Some(OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 1.0, 1.5).unwrap()),
            faces: Vec::new(),
        }],
    }
}
fn request(mode: ReflectionMode) -> ImageRequest {
    let mut camera = crate::Camera { distance: 50.0, ..Default::default() };
    camera.set_view(crate::StandardView::Top);
    ImageRequest {
        integrator: job::Integrator::Raytrace,
        raytrace: Some(RaytraceOptions {
            lights: Some(Vec::new()),
            reflections: Some(ReflectionOptions { bodies: vec![ReflectionBody { body: "Metal".into(), mode }], max_bounces: 1, samples: 64 }),
            ..Default::default()
        }),
        camera,
        width: 9,
        height: 9,
        background: [13, 27, 41],
        transparent_background: false,
    }
}
fn render(scene: ProductScene, request: ImageRequest, input: Option<EnvironmentInput>) -> Vec<u8> {
    let options = request.raytrace.clone().unwrap();
    let mut task = ImageTask::new_with_environment(scene, request, CancelToken::default(), options, input).unwrap();
    for _ in 0..200_000 {
        if let TaskPoll::Complete(png) = task.step(16384).unwrap() {
            return png;
        }
    }
    panic!("bounded environment reflection did not finish");
}
fn centre(bytes: &[u8]) -> [u8; 4] {
    ::image::load_from_memory(bytes).unwrap().to_rgba8().get_pixel(4, 4).0
}
fn expected(radiance: [f64; 3]) -> [u8; 4] {
    let c = radiance.map(|v| encode(v / (1.0 + v)));
    [c[0], c[1], c[2], 255]
}

#[test]
fn ideal_metal_mirror_receives_environment_once_and_camera_background_stays_independent() {
    let req = request(ReflectionMode::Mirror);
    assert_eq!(centre(&render(scene(Vec3::Z), req.clone(), None)), [0, 0, 0, 255]);
    assert_eq!(centre(&render(scene(Vec3::Z), req.clone(), Some(environment(false, 0.0)))), expected([1.0, 0.5, 0.25]));
    let mut empty = req;
    empty.raytrace.as_mut().unwrap().reflections = Some(ReflectionOptions::default());
    let image = ::image::load_from_memory(&render(
        ProductScene { meshes: Vec::new(), bounds: solidcraft_geom::Aabb3::EMPTY },
        empty,
        Some(environment(false, 0.0)),
    ))
    .unwrap()
    .to_rgba8();
    assert!(image.pixels().all(|p| p.0 == [13, 27, 41, 255]));
}

#[test]
fn rough_metal_environment_matches_independent_sample_weights_without_extra_nee() {
    let mut factor = 0.0;
    // Normal-view white conductor at roughness1: null for z<=0, weight=2z/(1+z).
    // Only the deterministic variate key is shared; no BRDF/PDF implementation is reused.
    for sample in 0..64 {
        let u = reflection::variates(40, sample, 0, 64).unwrap();
        let z = 1.0 - 2.0 * u[0];
        if z > 0.0 {
            factor += 2.0 * z / (1.0 + z) / 64.0;
        }
    }
    let req = request(ReflectionMode::RoughSpecular);
    let png = render(scene(Vec3::Z), req.clone(), Some(environment(false, 0.0)));
    assert_eq!(centre(&png), expected([factor, factor * 0.5, factor * 0.25]));
    assert_eq!(png, render(scene(Vec3::Z), req, Some(environment(false, 0.0))));
}

#[test]
fn combined_hdr_equal_anisotropy_point_and_led_match_independent_radiance_sum() {
    let mut req = anisotropic_request(ReflectionMode::RoughSpecular, true);
    let options = req.raytrace.as_mut().unwrap();
    options.punctual_lights = Some(vec![job::PunctualLightOptions {
        position_mm: [0.0, 0.0, 10.0],
        color: [1.0, 0.5, 0.25],
        intensity: 1.0,
        reference_distance_mm: 10.0,
        minimum_distance_mm: 0.001,
        cone: None,
    }]);
    job::led_strip::LedStripOptions {
        center_mm: [0.0, 0.0, 10.0],
        length_mm: 8.0,
        width_mm: 0.5,
        length_axis: [1.0, 0.0, 0.0],
        width_axis: [0.0, -1.0, 0.0],
        color: [1.0, 0.5, 0.25],
        intensity: 2.0,
    }
    .append_to(options, 1)
    .unwrap();
    let mut reflected = 0.0;
    for sample in 0..64 {
        let u = reflection::variates(40, sample, 0, 64).unwrap();
        let z = 1.0 - 2.0 * u[0];
        if z > 0.0 {
            reflected += 2.0 * z / (1.0 + z) / 64.0;
        }
    }
    // Normal-view white conductor, roughness1: f*cos=1/(4*pi).
    // Point at its reference distance contributes Li=1; LED contributes Le*A/r²=.08.
    // Selected reflection receives HDR through BSDF misses only, never a second NEE term.
    let total = reflected + 1.08 / (4.0 * std::f64::consts::PI);
    let combined = render(scene(Vec3::Z), req.clone(), Some(environment(false, 0.0)));
    assert_eq!(centre(&combined), expected([total, total * 0.5, total * 0.25]));
    req.raytrace.as_mut().unwrap().anisotropic_faces = [None, None];
    assert_eq!(combined, render(scene(Vec3::Z), req, Some(environment(false, 0.0))));
}

#[test]
fn mirror_bright_cell_rotates_in_world_space_without_changing_background() {
    let reflected = Vec3::new(0.5, 0.5, 0.5_f64.sqrt());
    let normal = Vec3::Z + reflected;
    let req = request(ReflectionMode::Mirror);
    assert_eq!(centre(&render(scene(normal), req.clone(), Some(environment(true, 0.0)))), expected([16.0, 8.0, 2.0]));
    assert_eq!(centre(&render(scene(normal), req, Some(environment(true, std::f64::consts::FRAC_PI_2)))), [0, 0, 0, 255]);
}

#[test]
fn unsupported_diffuse_rough_hdr_refuses_and_selector_capacities_remain_accounted() {
    let req = request(ReflectionMode::RoughSpecular);
    let options = req.raytrace.clone().unwrap();
    let mut invalid = scene(Vec3::Z);
    invalid.meshes[0].surface = Some(OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 0.5, 1.0, 1.5).unwrap());
    assert!(ImageTask::new_with_environment(invalid, req.clone(), CancelToken::default(), options.clone(), Some(environment(false, 0.0))).is_err());
    let mut invalid = scene(Vec3::Z);
    invalid.meshes[0].faces.push(crate::FaceAppearance { face: 0, color: [255; 3], opacity: 1.0, surface: None });
    assert!(options.validate_environment_reflections(&invalid).is_err());
    let selectors = options.reflections.as_ref().unwrap();
    assert_eq!(
        options.retained_bytes().unwrap(),
        (selectors.bodies.capacity() * std::mem::size_of::<ReflectionBody>() + selectors.bodies[0].body.capacity()) as u64
    );
    assert!(std::mem::size_of::<ImageTask>() <= 4096, "combined task state must fit the host's admitted slot");
}

#[test]
fn secondary_environment_miss_obeys_shared_work_limit_and_cancel() {
    for cancel_now in [false, true] {
        let req = request(ReflectionMode::Mirror);
        let options = req.raytrace.clone().unwrap();
        let cancel = CancelToken::default();
        let mut task = ImageTask::new_with_environment(scene(Vec3::Z), req, cancel.clone(), options, Some(environment(false, 0.0))).unwrap();
        for _ in 0..100_000 {
            assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
            if task.pending.as_ref().is_some_and(|p| p.bounce > 0) {
                break;
            }
        }
        assert!(task.pending.as_ref().is_some_and(|p| p.bounce > 0));
        if cancel_now {
            cancel.cancel();
            assert_eq!(task.step(1).err().unwrap().code, ErrorCode::Cancelled);
        } else {
            task.work.limit = task.work.used();
            assert_eq!(task.step(1).err().unwrap().code, ErrorCode::LimitExceeded);
        }
    }
}

fn anisotropic_request(mode: ReflectionMode, equal_axes: bool) -> ImageRequest {
    let mut request = request(mode);
    request.raytrace.as_mut().unwrap().anisotropic_faces = [
        Some(job::AnisotropicFaceOptions {
            body_index: 0,
            body_name: "Metal".into(),
            face: 0,
            tangent: [1.0, 0.0, 0.0],
            orientation: job::AnisotropicOrientation::Zero,
            roughness_x: if equal_axes { 1.0 } else { 0.2 },
            roughness_y: if equal_axes { 1.0 } else { 0.6 },
        }),
        None,
    ];
    request
}

#[test]
fn hdr_equal_anisotropic_axes_match_isotropic_reflected_environment_bytes() {
    let plain = render(scene(Vec3::Z), request(ReflectionMode::RoughSpecular), Some(environment(false, 0.0)));
    let request = anisotropic_request(ReflectionMode::RoughSpecular, true);
    assert_eq!(plain, render(scene(Vec3::Z), request, Some(environment(false, 0.0))));
}

#[test]
fn hdr_anisotropic_direct_and_mirror_combinations_refuse_before_pixels() {
    for kind in 0..2 {
        let mut request = anisotropic_request(ReflectionMode::RoughSpecular, false);
        if kind == 0 {
            request.raytrace.as_mut().unwrap().reflections = None;
        } else {
            request.raytrace.as_mut().unwrap().reflections.as_mut().unwrap().bodies[0].mode = ReflectionMode::Mirror;
        }
        let options = request.raytrace.clone().unwrap();
        assert!(ImageTask::new_with_environment(scene(Vec3::Z), request, CancelToken::default(), options, Some(environment(false, 0.0))).is_err());
    }
}

#[test]
fn hdr_anisotropic_secondary_hit_yields_before_another_debit_and_observes_cancel() {
    let mut source = scene(Vec3::new(0.0, -1.0, 1.0));
    // Original planar reflector z=y, with a broad separate horizontal receiver.
    let mesh = Arc::new(Mesh {
        positions: vec![
            Vec3::new(-100.0, -100.0, 10.0),
            Vec3::new(-100.0, 100.0, 10.0),
            Vec3::new(100.0, 100.0, 10.0),
            Vec3::new(100.0, -100.0, 10.0),
        ],
        normals: vec![-Vec3::Z; 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        tri_face: vec![0, 0],
        ..Default::default()
    });
    source.bounds = source.bounds.union(&mesh.bounds());
    source.meshes.push(crate::ProductMesh { name: "Receiver".into(), mesh, color: [240, 80, 20], opacity: 1.0, surface: None, faces: Vec::new() });
    let mut request = anisotropic_request(ReflectionMode::RoughSpecular, false);
    request.camera.set_view(crate::StandardView::Front);
    request.width = 1;
    request.height = 1;
    let options = request.raytrace.clone().unwrap();
    let cancel = CancelToken::default();
    let mut task = ImageTask::new_with_environment(source, request, cancel.clone(), options, Some(environment(false, 0.0))).unwrap();
    while task.bvh.is_none() {
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
    }
    let mut secondary_hit = false;
    for _ in 0..200_000 {
        let tracing_secondary = task.pending.as_ref().is_some_and(|p| p.bounce > 0 && p.hit.is_none());
        let before = task.work.used();
        assert!(matches!(task.step(1).unwrap(), TaskPoll::Pending(_)));
        assert!(task.work.used() - before <= 1, "secondary hit completion fell through into shading");
        if tracing_secondary && task.pending.as_ref().is_some_and(|p| p.hit.is_some()) {
            secondary_hit = true;
            break;
        }
    }
    assert!(secondary_hit);
    let before = task.work.used();
    assert!(matches!(task.step(4).unwrap(), TaskPoll::Pending(_)));
    assert!(task.work.used() - before <= 4);
    cancel.cancel();
    assert_eq!(task.step(4).err().unwrap().code, ErrorCode::Cancelled);
}
