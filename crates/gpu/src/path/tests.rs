use super::*;
use solidcraft_geom::{Aabb3, Mesh};
use solidcraft_render::{Camera, ProductMesh, StandardView, surface::OpaqueSurface};

fn executor() -> Option<Executor> {
    let (adapter, device, queue) = match crate::open_headless() {
        Ok(device) => device,
        Err(e) => {
            assert!(std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none(), "required GPU unavailable: {e}");
            eprintln!("gpu-path device test skipped: {e}");
            return None;
        }
    };
    eprintln!("gpu-path tests on {} ({:?}, {:?})", adapter.get_info().name, adapter.get_info().device_type, adapter.get_info().backend);
    Some(Executor::new(device, queue).unwrap())
}
fn request(size: u32) -> ImageRequest {
    let mut camera = Camera { distance: 4.0, ..Default::default() };
    camera.set_view(StandardView::Top);
    ImageRequest { camera, width: size, height: size, background: [10, 20, 30], transparent_background: false }
}
fn sphere(surface: OpaqueSurface) -> ProductScene {
    sphere_grid(surface, 48, 96)
}
fn sphere_grid(surface: OpaqueSurface, rows: u32, columns: u32) -> ProductScene {
    sphere_section(surface, rows, columns, rows)
}
fn sphere_section(surface: OpaqueSurface, rows: u32, columns: u32, rings: u32) -> ProductScene {
    // Original unit sphere, deliberately triangulated here rather than imported test data.
    let mut mesh = Mesh::default();
    for i in 0..=rings {
        let theta = f64::from(i) * std::f64::consts::PI / f64::from(rows);
        for j in 0..=columns {
            let phi = f64::from(j) * std::f64::consts::TAU / f64::from(columns);
            let n = Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos());
            mesh.positions.push(n);
            mesh.normals.push(n);
        }
    }
    for i in 0..rings {
        for j in 0..columns {
            let a = i * (columns + 1) + j;
            let b = a + columns + 1;
            mesh.triangles.extend([[a, b, a + 1], [a + 1, b, b + 1]]);
            mesh.tri_face.extend([0, 0]);
        }
    }
    let bounds = mesh.bounds();
    ProductScene {
        meshes: vec![ProductMesh {
            name: "original sphere".into(),
            mesh: Arc::new(mesh),
            color: [255; 3],
            opacity: 1.0,
            surface: Some(surface),
            faces: Vec::new(),
        }],
        bounds,
    }
}
fn white() -> OpaqueSurface {
    OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 0.0, 1.0, 1.0).unwrap()
}

#[test]
fn shader_validates_without_a_device() {
    let module = naga::front::wgsl::parse_str(SHADER).unwrap();
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap();
}

#[test]
fn admission_rejects_hostile_parameters_and_transparency() {
    let mut scene = sphere(white());
    let mut r = request(16);
    for size in [0, 4097, u32::MAX] {
        r.width = size;
        assert!(admission(&scene, &r, JobOptions::default()).is_err());
    }
    r.width = 16;
    for samples in [0, 4097, u32::MAX] {
        assert!(admission(&scene, &r, JobOptions { samples, ..Default::default() }).is_err());
    }
    for depth in [0, 17, u32::MAX] {
        assert!(admission(&scene, &r, JobOptions { max_depth: depth, ..Default::default() }).is_err());
    }
    for ev in [f64::NAN, f64::INFINITY, 33.0] {
        assert!(admission(&scene, &r, JobOptions { exposure_ev: ev, ..Default::default() }).is_err());
    }
    scene.meshes[0].opacity = 0.5;
    assert!(admission(&scene, &r, JobOptions::default()).is_err());
    scene.meshes[0].opacity = 1.0;
    Arc::make_mut(&mut scene.meshes[0].mesh).triangles[0][0] = u32::MAX;
    assert!(scene::flatten(&scene, Vec3::ZERO, &CancelToken::default()).is_err());
}

#[test]
fn admission_applies_the_gpu_estimate_instead_of_the_cpu_raster_budget() {
    // A library-bounded source (not an ordinary UI scene): about 100 MiB of face appearances
    // on a small mesh at the largest image. Its CPU raster estimate is above 384 MiB (covered
    // in the render crate); the GPU's own estimate stays within its 512 MiB job cap.
    let count = (100 * 1024 * 1024 / std::mem::size_of::<solidcraft_render::FaceAppearance>()).min(1_000_000);
    let mut scene = sphere(white());
    let appearance = solidcraft_render::FaceAppearance { face: 0, color: [255; 3], opacity: 1.0, surface: None };
    scene.meshes[0].faces = vec![appearance; count];
    let r = request(2048);
    let a = admission(&scene, &r, JobOptions::default()).unwrap();
    assert!(a.estimated_total_bytes <= 512 * 1024 * 1024, "{a:?}");
    assert_eq!(a.face_appearances, count as u64);
    // The CPU host still rejects the same source.
    assert_eq!(solidcraft_render::job::admit(&scene, &r).unwrap_err().code, ErrorCode::LimitExceeded);
}

#[test]
fn bvh_handles_empty_and_cancelled_scenes() {
    let empty = ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY };
    let upload = scene::flatten(&empty, Vec3::ZERO, &CancelToken::default()).unwrap();
    assert_eq!(upload.nodes.len(), 32);
    assert_eq!(&upload.nodes[12..16], &u32::MAX.to_le_bytes());
    let cancel = CancelToken::default();
    cancel.cancel();
    assert_eq!(scene::flatten(&sphere(white()), Vec3::ZERO, &cancel).err().unwrap().code, ErrorCode::Cancelled);
}

#[test]
fn gpu_brdf_matches_f64_surface_oracle() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("path BRDF reference"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    for metallic in [0.0, 0.5, 1.0] {
        for roughness in [0.045, 0.25, 1.0] {
            for ior in [1.0, 1.5, 3.0] {
                let surface = OpaqueSurface::new(Vec3::new(0.8, 0.5, 0.2), metallic, roughness, ior).unwrap();
                let mut task = Task::new(context.clone(), sphere(surface), request(1), JobOptions::default(), CancelToken::default()).unwrap();
                for (view, light) in [
                    (Vec3::Z, Vec3::Z),
                    (Vec3::new(0.5, 0.2, 1.0), Vec3::new(-0.4, 0.3, 0.8)),
                    (Vec3::new(1.0, 0.0, 0.02), Vec3::new(-1.0, 0.1, 0.02)),
                ] {
                    let view = view.normalized().unwrap();
                    let light = light.normalized().unwrap();
                    let mut params = vec![0; 48];
                    scene::vector(&mut params, light, 0.0);
                    scene::vector(&mut params, Vec3::Z, 0.0);
                    scene::vector(&mut params, view, 0.0);
                    scene::vector(&mut params, Vec3::ZERO, 0.0);
                    context.queue.write_buffer(&task.uniform, 0, &params);
                    let mut encoder = context.device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_compute_pass(&Default::default());
                        pass.set_pipeline(&pipeline);
                        pass.set_bind_group(0, &task.bind, &[]);
                        pass.dispatch_workgroups(1, 1, 1);
                    }
                    context.queue.submit([encoder.finish()]);
                    task.sample = 1;
                    let actual = task.read_linear().unwrap()[0];
                    let expected = surface.brdf(Vec3::Z, view, light).unwrap();
                    for (a, b) in actual.iter().zip([expected.x, expected.y, expected.z]) {
                        assert!((f64::from(*a) - b).abs() <= 3e-3 * b.abs() + 2e-5, "m={metallic}, r={roughness}, ior={ior}: {a} vs {b}");
                    }
                }
            }
        }
    }
}

#[test]
fn gpu_tiny_scene_matches_cpu_studio_with_face_appearance() {
    let Some(executor) = executor() else { return };
    let surface = OpaqueSurface::new(Vec3::new(0.7, 0.2, 0.1), 0.5, 0.3, 1.5).unwrap();
    let mesh = Mesh {
        positions: vec![Vec3::new(-3.0, -3.0, 0.0), Vec3::new(3.0, -3.0, 0.0), Vec3::new(3.0, 3.0, 0.0), Vec3::new(-3.0, 3.0, 0.0)],
        normals: vec![Vec3::Z; 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        tri_face: vec![7, 9],
        ..Default::default()
    };
    let bounds = mesh.bounds();
    let scene = ProductScene {
        meshes: vec![ProductMesh {
            name: "reference plane".into(),
            mesh: Arc::new(mesh),
            color: [255; 3],
            opacity: 1.0,
            surface: Some(white()),
            faces: [7, 9].map(|face| solidcraft_render::FaceAppearance { face, color: [0; 3], opacity: 1.0, surface: Some(surface) }).to_vec(),
        }],
        bounds,
    };
    let mut task =
        Task::new(executor.context.clone(), scene, request(4), JobOptions { samples: 4, max_depth: 1, ..Default::default() }, CancelToken::default())
            .unwrap();
    for _ in 0..4 {
        task.dispatch().unwrap();
    }
    let actual = task.read_linear().unwrap();
    let expected = solidcraft_render::studio::shade(surface, Vec3::Z, Vec3::Z).unwrap();
    for pixel in actual {
        assert_eq!(pixel[3], 1.0);
        for (a, b) in pixel.iter().zip([expected.x, expected.y, expected.z]) {
            assert!((f64::from(*a) - b).abs() < 3e-3 * b.abs() + 2e-5, "{a} vs {b}");
        }
    }
}

#[test]
fn gpu_furnace_fixed_seed_and_cancellation() {
    let Some(executor) = executor() else { return };
    let options = JobOptions { samples: 4096, max_depth: 8, seed: 37, ..Default::default() };
    let mut task = Task::new(executor.context.clone(), sphere(white()), request(8), options, CancelToken::default()).unwrap();
    task.environment = 1.0;
    task.studio = 0.0;
    for _ in 0..options.samples {
        task.dispatch().unwrap();
    }
    let result = task.read_linear().unwrap();
    eprintln!("furnace centre {:?}; BVH {:.3} ms; upload {} bytes / {:.3} ms", result[27], task.metrics.0, task.metrics.1, task.metrics.2);
    // The oracle includes Schlick entry/exit attenuation even at IOR=1, plus single-
    // scattering GGX. It is near-white, not an exactly energy-preserving Lambert model.
    for index in [27, 28, 35, 36] {
        assert!((result[index][0] - 1.0).abs() < 0.07, "furnace radiance: {:?}", result[index]);
    }
    let mut repeat = Task::new(executor.context.clone(), sphere(white()), request(8), options, CancelToken::default()).unwrap();
    repeat.environment = 1.0;
    repeat.studio = 0.0;
    for _ in 0..options.samples {
        repeat.dispatch().unwrap();
    }
    assert_eq!(result, repeat.read_linear().unwrap(), "fixed sample indices and seed must repeat on the same adapter");
    let token = CancelToken::default();
    let mut cancelled = Task::new(executor.context.clone(), sphere(white()), request(16), options, token.clone()).unwrap();
    cancelled.dispatch().unwrap();
    token.cancel();
    assert_eq!(cancelled.step(4096).err().unwrap().code, ErrorCode::Cancelled);
}

#[test]
fn skinny_polar_triangles_do_not_shadow_themselves() {
    let Some(executor) = executor() else { return };
    let mut image = request(32);
    image.camera.distance = 16.0;
    image.camera.fov = 0.0005;
    let mut task = Task::new(
        executor.context.clone(),
        sphere_section(white(), 512, 1024, 1),
        image,
        JobOptions { samples: 1, max_depth: 1, ..Default::default() },
        CancelToken::default(),
    )
    .unwrap();
    task.dispatch().unwrap();
    let pixels = task.read_linear().unwrap();
    for (index, pixel) in pixels.iter().enumerate() {
        assert_eq!(pixel[3], 1.0, "polar cap covers the image: {index} {pixel:?}");
        assert!(pixel[0] > 0.1, "outward-facing cap must receive unobstructed studio light: {index} {pixel:?}");
    }
}

#[test]
fn native_host_cancel_never_publishes_a_frame() {
    let Some(executor) = executor() else { return };
    let host = ImageHost::with_executor(Arc::new(executor)).unwrap();
    let job = host
        .submit_with_options(
            solidcraft_render::job::JobId { session: 1, serial: 1, generation: 1 },
            1,
            sphere(white()),
            request(64),
            JobOptions { samples: 4096, ..Default::default() },
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = job.status().unwrap();
        if status.progress.phase == Phase::Running {
            break;
        }
        assert!(!status.progress.phase.terminal(), "{status:?}");
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    job.cancel().unwrap();
    assert_eq!(job.wait_timeout(Duration::from_secs(30)).unwrap().progress.phase, Phase::Cancelled);
    assert!(job.frame().unwrap().is_none());
}

/// Explicit benchmark: large allocation and sustained GPU load must not run in ordinary CI.
#[test]
#[ignore = "manual 1M-triangle 1920x1080 GPU performance measurement"]
fn path_performance() {
    let executor = executor().expect("benchmark requires an adapter");
    let samples = std::env::var("SOLIDCRAFT_PATH_BENCH_SAMPLES").ok().map(|s| s.parse::<u32>().unwrap()).unwrap_or(1);
    let options = JobOptions { samples, ..Default::default() };
    options.validate().unwrap();
    let surface = OpaqueSurface::new(Vec3::new(0.8, 0.4, 0.2), 0.25, 0.4, 1.5).unwrap();
    let scene = sphere_grid(surface, 512, 1024);
    let mut request = request(1920);
    request.height = 1080;
    let admitted = executor.admit(&scene, &request, options).unwrap();
    let mut task = Task::new(executor.context.clone(), scene, request, options, CancelToken::default()).unwrap();
    let started = Instant::now();
    while task.sample < samples {
        task.dispatch().unwrap();
    }
    let seconds = started.elapsed().as_secs_f64();
    let linear = task.read_linear().unwrap();
    assert!(linear.iter().all(|p| p.iter().all(|v| v.is_finite())));
    let coverage = linear.iter().map(|p| f64::from(p[3])).sum::<f64>() / linear.len() as f64;
    assert!((0.16..0.19).contains(&coverage), "sphere must occupy the expected projected area: {coverage}");
    let centre = linear[540 * 1920 + 960];
    let lit = linear.iter().filter(|p| p[0] > 0.01).count();
    eprintln!(
        "PATH_BENCH_DIAGNOSTIC coverage={coverage:.6} centre={centre:?} lit_pixels={lit} nearby={:?}",
        [linear[500 * 1920 + 960], linear[540 * 1920 + 1000], linear[580 * 1920 + 960]]
    );
    assert_eq!(centre[3], 1.0);
    assert!(centre[0] > 0.01, "benchmark must actually shade the sphere: {centre:?}");
    eprintln!("PATH_BENCH_IMAGE coverage={coverage:.6} centre={centre:?}");
    eprintln!(
        "PATH_BENCH input_triangles={} resolution=1920x1080 spp={} bvh_ms={:.3} upload_bytes={} upload_ms={:.3} admitted_bytes={} render_seconds={:.6} ms_per_spp={:.3}",
        admitted.triangles,
        samples,
        task.metrics.0,
        task.metrics.1,
        task.metrics.2,
        admitted.estimated_total_bytes,
        seconds,
        seconds * 1000.0 / f64::from(samples)
    );
}
