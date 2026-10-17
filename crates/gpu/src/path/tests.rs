use super::*;
use solidcraft_geom::{Aabb3, Mesh};
use solidcraft_render::{Camera, ProductMesh, StandardView, surface::OpaqueSurface};

fn executor() -> Option<Executor> {
    let (adapter, device, queue) = match open_headless() {
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
    ImageRequest { textures: None, environment: None, camera, width: size, height: size, background: [10, 20, 30], transparent_background: false }
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
            transmission: None,
            faces: Vec::new(),
        }],
        bounds,
    }
}
fn white() -> OpaqueSurface {
    OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 0.0, 1.0, 1.0).unwrap()
}

fn clear_glass() -> solidcraft_render::transmission::Transmission {
    solidcraft_render::transmission::Transmission::new(Vec3::ZERO, None).unwrap()
}

#[test]
fn volume_glass_requires_closed_outward_homogeneous_geometry() {
    let mut glass = sphere(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap());
    glass.meshes[0].transmission = Some(clear_glass());
    assert!(scene::flatten(&glass, Vec3::ZERO, &CancelToken::default()).is_ok());
    let mut open = glass.clone();
    let mesh = Arc::make_mut(&mut open.meshes[0].mesh);
    mesh.triangles.remove(300);
    mesh.tri_face.remove(300);
    assert!(scene::flatten(&open, Vec3::ZERO, &CancelToken::default()).is_err());
    let mut inward = glass;
    for t in &mut Arc::make_mut(&mut inward.meshes[0].mesh).triangles {
        t.swap(1, 2);
    }
    assert!(scene::flatten(&inward, Vec3::ZERO, &CancelToken::default()).is_err());
    open.meshes[0].transmission = Some(solidcraft_render::transmission::Transmission::new(Vec3::ZERO, Some(0.1)).unwrap());
    assert!(scene::flatten(&open, Vec3::ZERO, &CancelToken::default()).is_ok());
}

#[test]
fn nested_and_overlapping_dielectric_bounds_are_rejected_before_upload() {
    let mut outer = sphere(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap());
    outer.meshes[0].transmission = Some(clear_glass());
    for shift in [0.0, 0.5] {
        let mut inner = outer.meshes[0].clone();
        for p in &mut Arc::make_mut(&mut inner.mesh).positions {
            *p = *p * 0.5 + Vec3::new(shift, 0.0, 0.0);
        }
        let mut scene = outer.clone();
        scene.meshes.push(inner);
        assert!(scene::flatten(&scene, Vec3::ZERO, &CancelToken::default()).is_err());
    }
}

#[test]
fn multiple_closed_shells_in_one_dielectric_body_are_rejected() {
    let mut scene = sphere(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap());
    scene.meshes[0].transmission = Some(clear_glass());
    let second = scene.meshes[0].mesh.clone();
    let mesh = Arc::make_mut(&mut scene.meshes[0].mesh);
    let offset = mesh.positions.len() as u32;
    mesh.positions.extend(second.positions.iter().map(|p| *p + Vec3::new(4.0, 0.0, 0.0)));
    mesh.normals.extend(&second.normals);
    mesh.triangles.extend(second.triangles.iter().map(|t| t.map(|i| i + offset)));
    mesh.tri_face.extend(&second.tri_face);
    let error = scene::flatten(&scene, Vec3::ZERO, &CancelToken::default()).err().unwrap();
    assert!(error.to_string().contains("one connected shell"), "{error}");
}

#[test]
fn gpu_exact_dielectric_matches_f64_fresnel() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("dielectric_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut task = Task::new(context.clone(), sphere(white()), request(1), JobOptions::default(), CancelToken::default()).unwrap();
    for (ni, nt) in [(1.0_f64, 1.0_f64), (1.0, 1.5), (1.5, 1.0), (1.0, 3.0)] {
        for cosine in [0.0, 0.01, 0.5, (nt / ni).atan().cos(), 1.0] {
            let mut params = vec![0; 64];
            scene::vector(&mut params, Vec3::new(cosine, ni, nt), 0.0);
            params.resize(PARAMETER_BYTES, 0);
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
            let expected = solidcraft_render::transmission::fresnel(cosine, ni, nt).unwrap();
            for (a, b) in actual.iter().zip([expected.s, expected.p, expected.transmitted_cosine.unwrap_or(0.0)]) {
                assert!((f64::from(*a) - b).abs() < 2e-5, "Fresnel {ni}->{nt} cos={cosine}: {a} != {b}");
            }
        }
    }
}

#[test]
fn gpu_nonabsorbing_glass_sphere_white_furnace_conserves_radiance() {
    let Some(executor) = executor() else { return };
    let mut scene = sphere(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap());
    scene.meshes[0].transmission = Some(clear_glass());
    scene.meshes[0].opacity = 0.2;
    let options = JobOptions { samples: 512, max_depth: 16, seed: 1948, ..Default::default() };
    let mut task = Task::new(executor.context.clone(), scene, request(16), options, CancelToken::default()).unwrap();
    task.environment = 1.0;
    task.studio = 0.0;
    for _ in 0..options.samples {
        task.dispatch().unwrap();
    }
    for (index, pixel) in task.read_linear().unwrap().iter().enumerate() {
        assert!((pixel[0] - 1.0).abs() < 0.025, "glass furnace pixel {index}: {pixel:?}");
    }
}

#[test]
fn gpu_ball_lens_focus_uses_actual_mesh_intersections() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("lens_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut scene = sphere_grid(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap(), 128, 256);
    scene.meshes[0].transmission = Some(clear_glass());
    let mut task = Task::new(context.clone(), scene, request(1), JobOptions::default(), CancelToken::default()).unwrap();
    for height in [0.01, 0.02, 0.03] {
        let mut params = vec![0; 32];
        scene::vector(&mut params, Vec3::new(height, 0.0, 3.0), 0.0);
        scene::vector(&mut params, Vec3::ZERO, 0.0);
        scene::vector(&mut params, Vec3::ZERO, 0.0);
        scene::vector(&mut params, -Vec3::Z, 1.5); // nR/(2(n-1)), measured from centre.
        scene::vector(&mut params, Vec3::new(f64::from(task.epsilon), 0.0, 0.0), 0.0);
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
        assert_eq!(actual[3], 1.0);
        assert!((actual[0] - 1.0).abs() < 5e-5 && (actual[1] - 1.0).abs() < 5e-5, "image-space focal spot at h={height}: {actual:?}");
    }
}

#[test]
fn gpu_camera_inside_volume_fails_without_publishing_fake_radiance() {
    let Some(executor) = executor() else { return };
    let mut scene = sphere(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap());
    scene.meshes[0].transmission = Some(clear_glass());
    let mut request = request(1);
    request.camera.distance = 0.1;
    let mut task = Task::new(executor.context.clone(), scene, request, JobOptions::default(), CancelToken::default()).unwrap();
    task.dispatch().unwrap();
    assert_eq!(task.read_linear().unwrap_err().code, ErrorCode::InvalidScene);
}

#[test]
fn engine_preflight_keeps_cpu_alpha_and_routes_physical_glass_to_the_native_host() {
    use serde_json::json;
    let Some(executor) = executor() else { return };
    let mut s = solidcraft_engine::Session::default();
    s.set_render_job_host(Some(solidcraft_render::job::CpuHost::cooperative())).unwrap();
    s.execute("solid.box", &json!({"length":10,"width":10,"height":2,"body_name":"Glass"})).unwrap();
    s.execute("appearance.assign", &json!({"bodies":["Glass"],"color":"#c0e0ff","opacity":0.2})).unwrap();
    let cpu = s.execute("render.preflight", &json!({"settings":{"width":16,"height":16}})).unwrap();
    assert_eq!(cpu["refraction"], false);
    assert_eq!(cpu["transparency"], "sorted_alpha_approximation");
    assert!(s.execute("render.preflight", &json!({"progressive":true,"settings":{"width":16,"height":16}})).is_err());
    s.register_render_job_host(ImageHost::with_executor(Arc::new(executor)).unwrap()).unwrap();
    let snapshot = s.doc.to_json();
    let mut params = json!({"progressive":true,"settings":{"width":16,"height":16},
        "raytrace":{"dielectrics":[{"body_index":0,"body_name":"Glass","ior":1.5,"absorption_per_mm":[0.1,0.02,0],"boundary":"closed_box","thickness_mm":0}]}});
    let gpu = s.execute("render.preflight", &params).unwrap();
    assert_eq!(gpu["refraction"], true);
    assert_eq!(gpu["dielectric_boundaries"], json!(["thin_sheet", "closed_box", "closed_mesh"]));
    assert_eq!(gpu["transparency"], "physical_transmission");
    assert_eq!(gpu["resolved"]["executor"], "gpu-path");
    params["settings"]["camera"] = json!(Camera { distance: 40.0, fov: 0.7, ..Default::default() });
    params["lens"] = json!({"aperture_radius_mm":0.5,"focus_distance_mm":40});
    let lens = s.execute("render.preflight", &params).unwrap();
    assert_eq!(lens["options"]["lens"]["aperture_radius_mm"], 0.5);
    assert_eq!(snapshot, s.doc.to_json());
    params["progressive"] = json!(false);
    assert!(s.execute("render.preflight", &params).is_err());
    assert!(!solidcraft_engine::product_scene::extract(&s).unwrap().has_transmission());
}

#[test]
fn gpu_thin_lens_focus_and_defocus_match_geometric_oracle() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("camera_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    for (yaw, pitch, roll) in [(0.0, 0.0, 0.0), (-0.45, 0.5, 0.7)] {
        let mut request = request(2);
        request.camera = Camera { distance: 12.0, fov: 0.7, yaw, pitch, roll, ..Default::default() };
        let camera = request.camera;
        let lens = solidcraft_render::job::ThinLens { aperture_radius_mm: 0.4, focus_distance_mm: 9.0 };
        let options = JobOptions { lens: Some(lens), ..Default::default() };
        let mut task = Task::new(context.clone(), sphere(white()), request, options, CancelToken::default()).unwrap();
        let (right, up, back) = camera.basis();
        let eye = back * camera.distance;
        for (ux, uy) in [(0.0_f64, 0.0_f64), (0.7, -0.6)] {
            let focal =
                eye + (right * ux + up * uy) * (camera.half_height() * lens.focus_distance_mm / camera.distance) - back * lens.focus_distance_mm;
            for (radial, azimuth) in [(0.0_f64, 0.0_f64), (0.25, 0.125), (0.75, 0.625), (0.999, 0.999)] {
                let mut params = task.parameters(1);
                params.truncate(96);
                scene::vector(&mut params, Vec3::new(ux, uy, radial), azimuth as f32);
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
                let rays = task.read_linear().unwrap();
                let vec = |p: [f32; 4]| Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
                let origin = vec(rays[0]) - Vec3::new(16.0, 16.0, 16.0);
                let direction = vec(rays[1]) - Vec3::new(2.0, 2.0, 2.0);
                let disk = (right * (azimuth * std::f64::consts::TAU).cos() + up * (azimuth * std::f64::consts::TAU).sin())
                    * (radial.sqrt() * lens.aperture_radius_mm);
                assert!((origin - eye - disk).len() < 3e-6);
                let focus_hit = origin + direction * ((focal - origin).dot(back) / direction.dot(back));
                assert!((focus_hit - focal).len() < 5e-6, "focus {focus_hit:?} != {focal:?}");
                // At axial depth z, signed displacement from the pinhole hit is
                // disk*(1-z/focus). Check both sides, including the flip beyond focus.
                for depth in [4.5, 18.0] {
                    let centre = eye - back * depth;
                    let hit = origin + direction * ((centre - origin).dot(back) / direction.dot(back));
                    let pinhole = eye + (focal - eye) * (depth / lens.focus_distance_mm);
                    assert!((hit - pinhole - disk * (1.0 - depth / lens.focus_distance_mm)).len() < 1e-5);
                }
            }
        }
    }
}

#[test]
fn gpu_zero_aperture_is_bit_identical_and_lens_admission_is_explicit() {
    let Some(executor) = executor() else { return };
    let mut request = request(16);
    request.camera.fov = 0.7;
    let render = |lens, fov| {
        let options = JobOptions { samples: 8, lens, seed: 1948, ..Default::default() };
        let mut image = request.clone();
        image.camera.fov = fov;
        let mut task = Task::new(executor.context.clone(), sphere(white()), image, options, CancelToken::default()).unwrap();
        for _ in 0..options.samples {
            task.dispatch().unwrap();
        }
        task.read_linear().unwrap()
    };
    for fov in [0.0, 0.7] {
        assert_eq!(render(None, fov), render(Some(solidcraft_render::job::ThinLens { aperture_radius_mm: 0.0, focus_distance_mm: 8.0 }), fov));
    }
    let lens = Some(solidcraft_render::job::ThinLens { aperture_radius_mm: 0.3, focus_distance_mm: 4.0 });
    assert_eq!(render(lens, 0.7), render(lens, 0.7));
    request.camera.fov = 0.0;
    assert!(admission(&sphere(white()), &request, JobOptions { lens, ..Default::default() }).is_err());
    request.camera.fov = 1e-100;
    assert!(admission(&sphere(white()), &request, JobOptions { lens, ..Default::default() }).is_err());
}

#[test]
#[ignore = "manual glass transport evidence; set SOLIDCRAFT_GLASS_EVIDENCE to a PNG path"]
fn glass_transport_evidence() {
    let Some(executor) = executor() else { return };
    let output = std::env::var("SOLIDCRAFT_GLASS_EVIDENCE").unwrap();
    let mut meshes = Vec::new();
    for (centre, radius, color) in
        [(Vec3::new(-1.3, 0.0, 1.5), 1.5, Vec3::new(0.75, 0.94, 1.0)), (Vec3::new(2.0, 1.0, 1.0), 1.0, Vec3::new(1.0, 0.7, 0.35))]
    {
        let mut scene = sphere_grid(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap(), 96, 192);
        let mut body = scene.meshes.remove(0);
        for p in &mut Arc::make_mut(&mut body.mesh).positions {
            *p = *p * radius + centre;
        }
        body.transmission =
            Some(solidcraft_render::transmission::Transmission::new(Vec3::new(-color.x.ln(), -color.y.ln(), -color.z.ln()) / 5.0, None).unwrap());
        meshes.push(body);
    }
    for y in -16..16 {
        for x in -16..16 {
            let color = if (x + y) % 2 == 0 { Vec3::new(0.65, 0.7, 0.75) } else { Vec3::new(0.035, 0.055, 0.08) };
            let x = f64::from(x) * 2.0;
            let y = f64::from(y) * 2.0;
            let mesh = Mesh {
                positions: vec![
                    Vec3::new(x, y, -0.01),
                    Vec3::new(x + 2.0, y, -0.01),
                    Vec3::new(x + 2.0, y + 2.0, -0.01),
                    Vec3::new(x, y + 2.0, -0.01),
                ],
                normals: vec![Vec3::Z; 4],
                triangles: vec![[0, 1, 2], [0, 2, 3]],
                tri_face: vec![0, 0],
                ..Default::default()
            };
            meshes.push(ProductMesh {
                name: "original checker tile".into(),
                mesh: Arc::new(mesh),
                color: [200; 3],
                opacity: 1.0,
                surface: Some(OpaqueSurface::new(color, 0.0, 0.65, 1.5).unwrap()),
                transmission: None,
                faces: vec![],
            });
        }
    }
    let bounds = meshes.iter().fold(Aabb3::EMPTY, |a, m| a.union(&m.mesh.bounds()));
    let camera = Camera { target: Vec3::new(0.0, 0.0, 1.0), yaw: -0.45, pitch: 0.5, distance: 13.0, fov: 0.65, ..Default::default() };
    let request = ImageRequest {
        textures: None,
        environment: None,
        camera,
        width: 960,
        height: 720,
        background: [236, 240, 245],
        transparent_background: false,
    };
    let lens =
        std::env::var_os("SOLIDCRAFT_GLASS_DOF").map(|_| solidcraft_render::job::ThinLens { aperture_radius_mm: 0.5, focus_distance_mm: 12.0 });
    let options = JobOptions { samples: 256, max_depth: 16, seed: 1948, lens, ..Default::default() };
    let mut task = Task::new(executor.context.clone(), ProductScene { meshes, bounds }, request, options, CancelToken::default()).unwrap();
    task.environment = 1.0;
    loop {
        if let TaskPoll::Complete(png) = task.step(4096).unwrap() {
            std::fs::write(&output, png).unwrap();
            break;
        }
    }
}

fn glass_slab(thin: bool) -> ProductScene {
    let mut mesh = Mesh::default();
    let faces = [
        ([[-10., -10., 0.5], [10., -10., 0.5], [10., 10., 0.5], [-10., 10., 0.5]], Vec3::Z),
        ([[-10., 10., -0.5], [10., 10., -0.5], [10., -10., -0.5], [-10., -10., -0.5]], -Vec3::Z),
        ([[10., -10., -0.5], [10., 10., -0.5], [10., 10., 0.5], [10., -10., 0.5]], Vec3::X),
        ([[-10., 10., -0.5], [-10., -10., -0.5], [-10., -10., 0.5], [-10., 10., 0.5]], -Vec3::X),
        ([[10., 10., -0.5], [-10., 10., -0.5], [-10., 10., 0.5], [10., 10., 0.5]], Vec3::Y),
        ([[-10., -10., -0.5], [10., -10., -0.5], [10., -10., 0.5], [-10., -10., 0.5]], -Vec3::Y),
    ];
    for (index, (positions, normal)) in faces.into_iter().enumerate() {
        if thin && index > 0 {
            break;
        }
        let offset = mesh.positions.len() as u32;
        for p in positions {
            mesh.positions.push(Vec3::new(p[0], p[1], p[2]));
            mesh.normals.push(normal);
        }
        mesh.triangles.extend([[offset, offset + 1, offset + 2], [offset, offset + 2, offset + 3]]);
        mesh.tri_face.extend([0, 0]);
    }
    let bounds = mesh.bounds();
    ProductScene {
        bounds,
        meshes: vec![ProductMesh {
            name: "original slab".into(),
            mesh: Arc::new(mesh),
            color: [255; 3],
            opacity: 1.0,
            surface: Some(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap()),
            transmission: Some(
                solidcraft_render::transmission::Transmission::new(Vec3::new(-0.5_f64.ln(), -0.8_f64.ln(), 0.0), thin.then_some(1.0)).unwrap(),
            ),
            faces: vec![],
        }],
    }
}

#[test]
fn gpu_solid_and_thin_slabs_match_absorbing_interreflection_oracle() {
    let Some(executor) = executor() else { return };
    for (thin, thickness) in [(false, 1.0), (true, 1.0), (true, 0.0)] {
        let mut scene = glass_slab(thin);
        if thin {
            let sigma = scene.meshes[0].transmission.unwrap().extinction();
            scene.meshes[0].transmission = Some(solidcraft_render::transmission::Transmission::new(sigma, Some(thickness)).unwrap());
        }
        let (_, expected) = solidcraft_render::transmission::slab(scene.meshes[0].transmission.unwrap(), thickness, 1.0, 1.0, 1.5).unwrap();
        let mut request = request(1);
        request.camera.fov = 0.0001;
        let options = JobOptions { samples: 4096, max_depth: 16, seed: 1948, ..Default::default() };
        let mut task = Task::new(executor.context.clone(), scene, request, options, CancelToken::default()).unwrap();
        task.environment = 1.0;
        task.environment_bottom_only = true;
        // Deliberately large offset: Beer uses unshifted geometric endpoints.
        task.epsilon = 0.05;
        task.studio = 0.0;
        for _ in 0..options.samples {
            task.dispatch().unwrap();
        }
        let actual = task.read_linear().unwrap()[0];
        for (a, b) in actual.iter().zip([expected.x, expected.y, expected.z]) {
            assert!((f64::from(*a) - b).abs() < 0.015, "slab thin={thin} thickness={thickness}: {actual:?} vs {expected:?}");
        }
    }
}

#[test]
fn shader_validates_without_a_device() {
    let module = naga::front::wgsl::parse_str(SHADER).unwrap();
    let parameters = module.types.iter().find(|(_, ty)| ty.name.as_deref() == Some("Params")).unwrap().1;
    let naga::TypeInner::Struct { span, .. } = &parameters.inner else { panic!("Params must be a struct") };
    assert_eq!(*span as usize, PARAMETER_BYTES);
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
    let appearance = solidcraft_render::FaceAppearance { face: 0, color: [255; 3], opacity: 1.0, surface: None, transmission: None };
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
            transmission: None,
            faces: [7, 9]
                .map(|face| solidcraft_render::FaceAppearance { face, color: [0; 3], opacity: 1.0, surface: Some(surface), transmission: None })
                .to_vec(),
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

#[test]
fn gpu_clear_coat_brdf_matches_f64_surface_oracle() {
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
    for metallic in [0.0, 1.0] {
        for roughness in [0.045, 0.12, 0.5] {
            for ior in [1.5, 3.0] {
                let surface = OpaqueSurface::new(Vec3::new(0.8, 0.5, 0.2), metallic, 0.3, 1.5)
                    .unwrap()
                    .with_clear_coat(solidcraft_render::clear_coat::ClearCoat::new(1.0, roughness, ior).unwrap());
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
fn gpu_clear_coat_pdf_matches_f64_surface_oracle() {
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
        entry_point: Some("pdf_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    for metallic in [0.0, 1.0] {
        for roughness in [0.045, 0.12, 0.5] {
            for ior in [1.5, 3.0] {
                let surface = OpaqueSurface::new(Vec3::new(0.8, 0.5, 0.2), metallic, 0.3, 1.5)
                    .unwrap()
                    .with_clear_coat(solidcraft_render::clear_coat::ClearCoat::new(1.0, roughness, ior).unwrap());
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
                    let expected = solidcraft_render::clear_coat::surface_pdf(surface, Vec3::Z, view, light).unwrap();
                    for (a, b) in actual.iter().zip([expected, 0.0, 0.0]) {
                        assert!((f64::from(*a) - b).abs() <= 3e-3 * b.abs() + 2e-5, "m={metallic}, r={roughness}, ior={ior}: {a} vs {b}");
                    }
                }
            }
        }
    }
}

#[test]
fn gpu_clear_coat_samples_the_pdf_mixture_components() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("coat sampling oracle"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("surface_sample_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    for weight in [0.0, 0.4, 1.0] {
        let surface = OpaqueSurface::new(Vec3::new(0.02, 0.02, 0.02), 0.0, 0.45, 1.5)
            .unwrap()
            .with_clear_coat(solidcraft_render::clear_coat::ClearCoat::new(weight, 0.12, 1.5).unwrap());
        let mut task = Task::new(context.clone(), sphere(surface), request(1), JobOptions::default(), CancelToken::default()).unwrap();
        let p = weight * 0.5;
        // One variate strictly inside each nonempty mixture interval.
        for component in [(1.0 - p) * 0.25, (1.0 - p) * 0.75, 1.0 - p * 0.5] {
            if component >= 1.0 {
                continue;
            }
            for x in [0.125_f64, 0.75] {
                let mut params = vec![0; 48];
                scene::vector(&mut params, Vec3::ZERO, 0.0);
                scene::vector(&mut params, Vec3::Z, 0.0);
                scene::vector(&mut params, Vec3::Z, 0.0);
                scene::vector(&mut params, Vec3::new(component, x, 0.25), 0.0);
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
                let expected = if component < (1.0 - p) * 0.5 {
                    Vec3::new(0.0, x.sqrt(), (1.0 - x).sqrt())
                } else {
                    let r = if component >= 1.0 - p { 0.12_f64 } else { 0.45_f64 };
                    let c = ((1.0 - x) / (1.0 + (r.powi(4) - 1.0) * x)).sqrt();
                    Vec3::new(0.0, 2.0 * c * (1.0 - c * c).sqrt(), 2.0 * c * c - 1.0)
                } + Vec3::new(2.0, 2.0, 2.0);
                for (a, b) in actual.iter().zip([expected.x, expected.y, expected.z]) {
                    assert!((f64::from(*a) - b).abs() < 2e-5, "weight={weight}, component={component}, x={x}: {actual:?}, {expected:?}");
                }
            }
        }
    }
}

#[test]
fn gpu_studio_area_mis_matches_independent_quadrature_and_distance_visibility() {
    use solidcraft_render::job::{AreaLight, AreaLights, StudioLighting};
    let Some(executor) = executor() else { return };
    let surface = white();
    let plane = |z: f64| {
        let mesh = Mesh {
            positions: vec![Vec3::new(-10.0, -10.0, z), Vec3::new(10.0, -10.0, z), Vec3::new(10.0, 10.0, z), Vec3::new(-10.0, 10.0, z)],
            normals: vec![Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![0, 0],
            ..Default::default()
        };
        ProductMesh {
            name: "original quadrature plane".into(),
            mesh: Arc::new(mesh),
            color: [255; 3],
            opacity: 1.0,
            surface: Some(surface),
            transmission: None,
            faces: Vec::new(),
        }
    };
    let light = AreaLight { center: [2.0, 0.0, 2.0], u: [1.0, 0.0, 0.0], v: [0.0, -1.0, 0.0], radiance: [3.0; 3] };
    let mut expected = 0.0;
    for y in 0..256 {
        for x in 0..256 {
            let delta = Vec3::new(1.0 + (f64::from(x) + 0.5) / 128.0, -1.0 + (f64::from(y) + 0.5) / 128.0, 2.0);
            let l = delta.normalized().unwrap();
            expected += surface.brdf(Vec3::Z, Vec3::Z, l).unwrap().x * 3.0 * l.z * l.z / delta.len2() * 4.0 / (256.0 * 256.0);
        }
    }
    for depth in [1, 2] {
        for blocker in [None, Some(4.0), Some(1.0)] {
            let mut scene = ProductScene { meshes: vec![plane(0.0)], bounds: Aabb3::EMPTY };
            if let Some(z) = blocker {
                scene.meshes.push(plane(z));
            }
            // Camera sits below any blocker and sees the receiver. Geometry behind
            // the emitter must not shadow it; the nearer blocker must.
            let mut image = request(1);
            image.camera.distance = 0.1;
            image.camera.fov = 0.001;
            let options = JobOptions {
                samples: 4096,
                max_depth: depth,
                seed: 37,
                lighting: Some(StudioLighting { sun: None, environment: [0.0; 3], area_lights: AreaLights::new(&[light]).unwrap() }),
                ..Default::default()
            };
            let mut task = Task::new(executor.context.clone(), scene, image, options, CancelToken::default()).unwrap();
            assert_eq!(task.parameters(1).len(), PARAMETER_BYTES);
            for _ in 0..options.samples {
                task.dispatch().unwrap();
            }
            let value = f64::from(task.read_linear().unwrap()[0][0]);
            if blocker == Some(1.0) {
                assert!(value < 1e-6, "blocked depth{depth}: {value}");
            } else {
                assert!((value - expected).abs() < 0.04 * expected, "depth{depth}, blocker{blocker:?}: {value} vs {expected}");
            }
        }
    }
}

#[test]
fn gpu_studio_environment_mis_matches_furnace_at_last_and_continuing_bounces() {
    use solidcraft_render::job::StudioLighting;
    let Some(executor) = executor() else { return };
    for depth in [1, 2] {
        let options = JobOptions {
            samples: 4096,
            max_depth: depth,
            seed: 37,
            lighting: Some(StudioLighting { sun: None, environment: [1.0; 3], ..Default::default() }),
            ..Default::default()
        };
        let mut task = Task::new(executor.context.clone(), sphere(white()), request(8), options, CancelToken::default()).unwrap();
        for _ in 0..options.samples {
            task.dispatch().unwrap();
        }
        let pixels = task.read_linear().unwrap();
        for index in [27, 28, 35, 36] {
            assert!((pixels[index][0] - 1.0).abs() < 0.07, "depth{depth}: {:?}", pixels[index]);
        }
    }
}

#[test]
fn gpu_emitter_inside_glass_applies_final_segment_absorption() {
    use solidcraft_render::job::{AreaLight, AreaLights, StudioLighting};
    let Some(executor) = executor() else { return };
    let mut glass = sphere(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.0).unwrap());
    glass.meshes[0].transmission = Some(solidcraft_render::transmission::Transmission::new(Vec3::new(0.7, 0.3, 0.0), None).unwrap());
    let light = AreaLight { center: [0.0; 3], u: [0.4, 0.0, 0.0], v: [0.0, 0.4, 0.0], radiance: [3.0; 3] };
    let options = JobOptions {
        samples: 16,
        max_depth: 2,
        lighting: Some(StudioLighting { sun: None, environment: [0.0; 3], area_lights: AreaLights::new(&[light]).unwrap() }),
        ..Default::default()
    };
    let mut image = request(2);
    image.camera.fov = 0.0001;
    let mut task = Task::new(executor.context.clone(), glass, image, options, CancelToken::default()).unwrap();
    for _ in 0..options.samples {
        task.dispatch().unwrap();
    }
    for pixel in task.read_linear().unwrap() {
        for (value, sigma) in pixel.iter().zip([0.7_f64, 0.3, 0.0]) {
            assert!((f64::from(*value) - 3.0 * (-sigma).exp()).abs() < 0.002, "{pixel:?}");
        }
    }
}

fn environment_plane() -> ProductScene {
    let mesh = Mesh {
        positions: vec![Vec3::new(-3.0, -3.0, 0.0), Vec3::new(3.0, -3.0, 0.0), Vec3::new(3.0, 3.0, 0.0), Vec3::new(-3.0, 3.0, 0.0)],
        normals: vec![Vec3::Z; 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        tri_face: vec![0, 0],
        ..Default::default()
    };
    let bounds = mesh.bounds();
    ProductScene {
        meshes: vec![ProductMesh {
            name: "original environment test plane".into(),
            mesh: Arc::new(mesh),
            color: [255; 3],
            opacity: 1.0,
            surface: Some(white()),
            transmission: None,
            faces: Vec::new(),
        }],
        bounds,
    }
}
fn original_hdr() -> Vec<u8> {
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nPRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n\n-Y 2 +X 4\n".to_vec();
    for _ in 0..8 {
        bytes.extend_from_slice(&[128, 128, 128, 129]);
    }
    bytes
}
#[test]
fn gpu_hdr_environment_matches_constant_furnace_and_resource_admission() {
    use solidcraft_render::lighting::environment::EnvironmentSource;
    let Some(executor) = executor() else { return };
    let source = Arc::new(EnvironmentSource::hdr(original_hdr(), 0.3, 1.0, false).unwrap());
    let mut image = request(8);
    image.environment = Some(source.clone());
    let options = JobOptions { samples: 2048, max_depth: 2, seed: 37, ..Default::default() };
    let plain = admission(&sphere(white()), &request(8), options).unwrap();
    let with_map = admission(&sphere(white()), &image, options).unwrap();
    assert_eq!(with_map.estimated_total_bytes - plain.estimated_total_bytes, source.admitted_bytes());
    let mut task = Task::new(executor.context.clone(), sphere(white()), image, options, CancelToken::default()).unwrap();
    for _ in 0..options.samples {
        task.dispatch().unwrap();
    }
    let pixels = task.read_linear().unwrap();
    for index in [27, 28, 35, 36] {
        assert!((pixels[index][0] - 1.0).abs() < 0.07, "{:?}", pixels[index]);
    }
}
#[test]
fn gpu_finite_sun_matches_cone_quadrature_at_final_and_continuing_bounces() {
    use solidcraft_render::job::{StudioLighting, Sun};
    let Some(executor) = executor() else { return };
    let sun = Sun { direction: [0.0, 0.0, 1.0], angular_radius_rad: 0.4, radiance: [3.0; 3] };
    let mut expected = 0.0;
    for i in 0..4096 {
        let z = 1.0 - (f64::from(i) + 0.5) / 4096.0 * (1.0 - sun.angular_radius_rad.cos());
        let l = Vec3::new((1.0 - z * z).sqrt(), 0.0, z);
        expected += white().brdf(Vec3::Z, Vec3::Z, l).unwrap().x * z * 3.0 * sun.solid_angle() / 4096.0;
    }
    for max_depth in [1, 2] {
        let mut image = request(2);
        image.camera.distance = 0.1;
        image.camera.fov = 0.001;
        let options = JobOptions {
            samples: 2048,
            max_depth,
            seed: 123,
            lighting: Some(StudioLighting { sun: Some(sun), ..Default::default() }),
            ..Default::default()
        };
        let mut task = Task::new(executor.context.clone(), environment_plane(), image, options, CancelToken::default()).unwrap();
        for _ in 0..options.samples {
            task.dispatch().unwrap();
        }
        for pixel in task.read_linear().unwrap() {
            assert!((f64::from(pixel[0]) - expected).abs() < 0.03 * expected, "depth{max_depth}: {pixel:?}, {expected}");
        }
    }
}

#[test]
fn gpu_environment_orientation_rotation_intensity_and_pdf_match_cpu() {
    use solidcraft_render::lighting::environment::EnvironmentSource;
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
    for i in 0..8u8 {
        bytes.extend_from_slice(&[16 * (i + 1), 32, 64, 129]);
    }
    let rotation = 0.7_f64;
    let intensity = 2.5;
    let source = Arc::new(EnvironmentSource::hdr(bytes, rotation, intensity, true).unwrap());
    let cpu = source.prepare(&CancelToken::default()).unwrap();
    let mut image = request(1);
    image.environment = Some(source);
    let mut task = Task::new(context.clone(), sphere(white()), image, JobOptions::default(), CancelToken::default()).unwrap();
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("environment_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    for row in 0..2 {
        for column in 0..4 {
            let theta = std::f64::consts::PI * (f64::from(row) + 0.5) / 2.0;
            let phi = std::f64::consts::TAU * (f64::from(column) + 0.5) / 4.0;
            let local = Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos());
            let direction =
                Vec3::new(rotation.cos() * local.x - rotation.sin() * local.y, rotation.sin() * local.x + rotation.cos() * local.y, local.z);
            let mut params = task.parameters(1);
            for (out, value) in params[48..60].as_chunks_mut::<4>().0.iter_mut().zip([direction.x, direction.y, direction.z]) {
                out.copy_from_slice(&(value as f32).to_le_bytes());
            }
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
            let (rgb, pdf) = cpu.evaluate(local).unwrap();
            for (a, b) in actual.iter().zip(rgb.into_iter().map(|v| v * intensity as f32).chain([pdf])) {
                assert!((a - b).abs() < 1e-5, "{actual:?} vs {rgb:?}/{pdf}");
            }
        }
    }
}

fn shadow_probe(context: &Arc<Context>, scene: ProductScene, origin: Vec3, direction: Vec3, limit: f64) -> [[f32; 4]; 2] {
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadow connection oracle"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("shadow_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut task = Task::new(context.clone(), scene, request(2), JobOptions::default(), CancelToken::default()).unwrap();
    let mut params = vec![0; 32];
    scene::vector(&mut params, origin, 0.0);
    scene::vector(&mut params, Vec3::ZERO, 0.0);
    scene::vector(&mut params, Vec3::ZERO, 0.0);
    scene::vector(&mut params, direction, limit as f32);
    // Large offset catches Beer intervals incorrectly measured from shifted origins.
    scene::vector(&mut params, Vec3::new(0.01, 0.0, 0.0), 0.0);
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
    let pixels = task.read_linear().unwrap();
    [pixels[0], pixels[1]]
}

#[test]
fn gpu_shadow_sheets_solids_endpoints_and_invalid_crossings_match_oracles() {
    use solidcraft_render::transmission::{slab, solid_shadow};
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    for thin in [true, false] {
        let mut scene = glass_slab(thin);
        let optics = scene.meshes[0].transmission.unwrap().with_shadow_transmission(true);
        scene.meshes[0].transmission = Some(optics);
        for cosine in [1.0_f64, 0.8] {
            let d = Vec3::new((1.0 - cosine * cosine).sqrt(), 0.0, -cosine);
            let actual = shadow_probe(context, scene.clone(), Vec3::new(0.0, 0.0, 2.0), d, 8.0);
            let expected =
                if thin { slab(optics, 1.0, cosine, 1.0, 1.5).unwrap().1 } else { solid_shadow(optics, 1.0 / cosine, cosine, cosine, 1.5).unwrap() };
            for (a, b) in actual[0].iter().zip([expected.x, expected.y, expected.z]) {
                assert!((f64::from(*a) - b).abs() < 2e-5, "thin={thin}, cosine={cosine}: {actual:?} vs {expected:?}");
            }
            assert_eq!(actual[1][1], if thin { 1.0 } else { 2.0 });
        }
        let before = shadow_probe(context, scene.clone(), Vec3::new(0.0, 0.0, 2.0), -Vec3::Z, 1.0);
        assert_eq!(before[0], [1.0; 4]);
        if !thin {
            let inside = shadow_probe(context, scene.clone(), Vec3::new(0.0, 0.0, 2.0), -Vec3::Z, 2.0);
            let expected = optics.attenuation(0.5).unwrap() * (0.96 / 2.25);
            for (a, b) in inside[0].iter().zip([expected.x, expected.y, expected.z]) {
                assert!((f64::from(*a) - b).abs() < 2e-5);
            }
            // An unmatched exit is fail-closed even if caller bypassed admission.
            assert_eq!(shadow_probe(context, scene.clone(), Vec3::ZERO, Vec3::Z, 8.0)[0], [0.0, 0.0, 0.0, 1.0]);
        }
        scene.meshes[0].transmission = Some(optics.with_shadow_transmission(false));
        assert_eq!(shadow_probe(context, scene, Vec3::new(0.0, 0.0, 2.0), -Vec3::Z, 8.0)[0], [0.0, 0.0, 0.0, 1.0]);
    }
    let mut opaque = glass_slab(true);
    opaque.meshes[0].transmission = None;
    assert_eq!(shadow_probe(context, opaque, Vec3::new(0.0, 0.0, 2.0), -Vec3::Z, 8.0)[0], [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn gpu_shadow_interface_walk_is_bounded_and_stacked_sheet_pdf_matches() {
    let Some(executor) = executor() else { return };
    for count in [2_u32, 64, 65] {
        let mut scene = glass_slab(true);
        let base = scene.meshes[0].mesh.clone();
        let mut mesh = Mesh::default();
        for layer in 0..count {
            let offset = mesh.positions.len() as u32;
            mesh.positions.extend(base.positions.iter().map(|p| *p - Vec3::Z * f64::from(layer)));
            mesh.normals.extend_from_slice(&base.normals);
            mesh.triangles.extend(base.triangles.iter().map(|t| t.map(|i| i + offset)));
            mesh.tri_face.extend_from_slice(&base.tri_face);
        }
        scene.meshes[0].mesh = Arc::new(mesh);
        scene.meshes[0].transmission = Some(solidcraft_render::transmission::Transmission::new(Vec3::ZERO, Some(0.0)).unwrap());
        let actual = shadow_probe(&executor.context, scene, Vec3::new(0.0, 0.0, 2.0), -Vec3::Z, 100.0);
        if count == 65 {
            assert_eq!(actual[0], [0.0, 0.0, 0.0, 1.0]);
        } else {
            let expected = (0.96_f64 / 1.04).powi(count as i32);
            assert!((f64::from(actual[0][0]) - expected).abs() < 2e-5);
            assert!((f64::from(actual[1][0]) - expected).abs() < 2e-5);
            assert_eq!(actual[1][1], count as f32);
        }
    }
}

#[test]
fn gpu_shadow_mis_does_not_double_direct_energy_through_index_matched_glass() {
    use solidcraft_render::job::{AreaLight, AreaLights, StudioLighting};
    let Some(executor) = executor() else { return };
    let surface = white();
    let receiver = ProductMesh {
        name: "shadow MIS receiver".into(),
        mesh: Arc::new(Mesh {
            positions: vec![Vec3::new(-20.0, -20.0, -2.0), Vec3::new(20.0, -20.0, -2.0), Vec3::new(20.0, 20.0, -2.0), Vec3::new(-20.0, 20.0, -2.0)],
            normals: vec![Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![0, 0],
            ..Default::default()
        }),
        color: [255; 3],
        opacity: 1.0,
        surface: Some(surface),
        transmission: None,
        faces: vec![],
    };
    let light = AreaLight { center: [0.0, 0.0, 2.0], u: [4.0, 0.0, 0.0], v: [0.0, -4.0, 0.0], radiance: [1.0; 3] };
    let mut expected = 0.0;
    for y in 0..128 {
        for x in 0..128 {
            let delta = Vec3::new(-4.0 + (f64::from(x) + 0.5) / 16.0, -4.0 + (f64::from(y) + 0.5) / 16.0, 4.0);
            let l = delta.normalized().unwrap();
            expected += surface.brdf(Vec3::Z, Vec3::Z, l).unwrap().x * l.z * l.z / delta.len2() * 64.0 / 16384.0;
        }
    }
    for (thin, enabled, depth) in [(true, true, 2), (true, true, 4), (true, false, 4), (false, true, 1), (false, true, 4), (false, false, 4)] {
        let mut scene = glass_slab(thin);
        scene.meshes[0].surface = Some(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.0).unwrap());
        scene.meshes[0].transmission =
            Some(solidcraft_render::transmission::Transmission::new(Vec3::ZERO, thin.then_some(0.0)).unwrap().with_shadow_transmission(enabled));
        scene.meshes.push(receiver.clone());
        let options = JobOptions {
            samples: 4096,
            max_depth: depth,
            seed: 37,
            lighting: Some(StudioLighting { sun: None, environment: [0.0; 3], area_lights: AreaLights::new(&[light]).unwrap() }),
            ..Default::default()
        };
        let mut image = request(1);
        image.camera.target = Vec3::new(0.0, 0.0, -2.0);
        image.camera.distance = 0.1;
        image.camera.fov = 0.001;
        let mut task = Task::new(executor.context.clone(), scene, image, options, CancelToken::default()).unwrap();
        for _ in 0..options.samples {
            task.dispatch().unwrap();
        }
        let actual = f64::from(task.read_linear().unwrap()[0][0]);
        assert!((actual - expected).abs() < 0.08 * expected, "thin={thin}, enabled={enabled}, depth={depth}: {actual} vs {expected}");
    }
}

#[test]
fn gpu_absorbing_two_sheet_mis_matches_f64_quadrature() {
    use solidcraft_render::job::{AreaLight, AreaLights, StudioLighting};
    let Some(executor) = executor() else { return };
    let surface = white();
    // A tiny receiver excludes sheet-reflection returns from this direct-only oracle.
    // Its half-width still exceeds the primary ray footprint by two orders of magnitude.
    let receiver = ProductMesh {
        name: "shadow MIS receiver".into(),
        mesh: Arc::new(Mesh {
            positions: vec![Vec3::new(-0.01, -0.01, -2.0), Vec3::new(0.01, -0.01, -2.0), Vec3::new(0.01, 0.01, -2.0), Vec3::new(-0.01, 0.01, -2.0)],
            normals: vec![Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![0, 0],
            ..Default::default()
        }),
        color: [255; 3],
        opacity: 1.0,
        surface: Some(surface),
        transmission: None,
        faces: vec![],
    };
    let light = AreaLight { center: [0.0, 0.0, 2.0], u: [4.0, 0.0, 0.0], v: [0.0, -4.0, 0.0], radiance: [1.0; 3] };
    let optics = solidcraft_render::transmission::Transmission::new(Vec3::new(0.2, 0.1, 0.05), Some(0.7)).unwrap();
    let mut expected = 0.0;
    for y in 0..128 {
        for x in 0..128 {
            let delta = Vec3::new(-4.0 + (f64::from(x) + 0.5) / 16.0, -4.0 + (f64::from(y) + 0.5) / 16.0, 4.0);
            let l = delta.normalized().unwrap();
            let transmission = solidcraft_render::transmission::slab(optics, 0.7, l.z, 1.0, 1.5).unwrap().1.x;
            expected += transmission * transmission * surface.brdf(Vec3::Z, Vec3::Z, l).unwrap().x * l.z * l.z / delta.len2() * 64.0 / 16384.0;
        }
    }
    for (thin, enabled, depth) in [(true, true, 1), (true, true, 4), (true, false, 4)] {
        let mut scene = glass_slab(thin);
        scene.meshes[0].transmission = Some(optics.with_shadow_transmission(enabled));
        let mut second = scene.meshes[0].clone();
        second.name = "second absorbing sheet".into();
        for p in &mut Arc::make_mut(&mut second.mesh).positions {
            p.z -= 1.0;
        }
        scene.meshes.push(second);
        scene.meshes.push(receiver.clone());
        let options = JobOptions {
            samples: 4096,
            max_depth: depth,
            seed: 37,
            lighting: Some(StudioLighting { sun: None, environment: [0.0; 3], area_lights: AreaLights::new(&[light]).unwrap() }),
            ..Default::default()
        };
        let mut image = request(1);
        image.camera.target = Vec3::new(0.0, 0.0, -2.0);
        image.camera.distance = 0.1;
        image.camera.fov = 0.001;
        let mut task = Task::new(executor.context.clone(), scene, image, options, CancelToken::default()).unwrap();
        for _ in 0..options.samples {
            task.dispatch().unwrap();
        }
        let actual = f64::from(task.read_linear().unwrap()[0][0]);
        assert!((actual - expected).abs() < 0.08 * expected, "thin={thin}, enabled={enabled}, depth={depth}: {actual} vs {expected}");
    }
}

#[test]
fn gpu_hdr_and_sun_shadow_mis_preserve_index_matched_energy() {
    use solidcraft_render::job::{StudioLighting, Sun};
    use solidcraft_render::lighting::environment::EnvironmentSource;
    let Some(executor) = executor() else { return };
    let sun = Sun { direction: [0.0, 0.0, 1.0], angular_radius_rad: 0.4, radiance: [3.0; 3] };
    let mut sun_expected = 0.0;
    for i in 0..4096 {
        let z = 1.0 - (f64::from(i) + 0.5) / 4096.0 * (1.0 - sun.angular_radius_rad.cos());
        let l = Vec3::new((1.0 - z * z).sqrt(), 0.0, z);
        sun_expected += white().brdf(Vec3::Z, Vec3::Z, l).unwrap().x * z * 3.0 * sun.solid_angle() / 4096.0;
    }
    for solar in [false, true] {
        for (thin, depth) in [(true, 2), (true, 4), (false, 1), (false, 4)] {
            let mut scene = glass_slab(thin);
            scene.meshes[0].surface = Some(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.0).unwrap());
            scene.meshes[0].transmission =
                Some(solidcraft_render::transmission::Transmission::new(Vec3::ZERO, thin.then_some(0.0)).unwrap().with_shadow_transmission(true));
            let mut receiver = environment_plane().meshes.remove(0);
            for p in &mut Arc::make_mut(&mut receiver.mesh).positions {
                p.z -= 2.0;
            }
            scene.meshes.push(receiver);
            scene.bounds = scene.meshes.iter().fold(Aabb3::EMPTY, |a, m| a.union(&m.mesh.bounds()));
            let mut image = request(1);
            image.camera.target = Vec3::new(0.0, 0.0, -2.0);
            image.camera.distance = 0.1;
            image.camera.fov = 0.001;
            if !solar {
                image.environment = Some(Arc::new(EnvironmentSource::hdr(original_hdr(), 0.3, 1.0, false).unwrap()));
            }
            let options = JobOptions {
                samples: 4096,
                max_depth: depth,
                seed: 37,
                lighting: Some(StudioLighting { sun: solar.then_some(sun), ..Default::default() }),
                ..Default::default()
            };
            let mut task = Task::new(executor.context.clone(), scene, image, options, CancelToken::default()).unwrap();
            for _ in 0..options.samples {
                task.dispatch().unwrap();
            }
            let actual = f64::from(task.read_linear().unwrap()[0][0]);
            let expected = if solar { sun_expected } else { 1.0 };
            assert!((actual - expected).abs() < 0.07 * expected, "sun={solar}, thin={thin}, depth={depth}: {actual} vs {expected}");
        }
    }
}

fn texture_fixture(frame: solidcraft_render::job::textures::PlanarFrame) -> Arc<solidcraft_render::job::textures::RuntimeTextures> {
    use image::ImageEncoder;
    use solidcraft_render::{
        job::textures::{RuntimeTextures, TextureBinding, TextureMap},
        texture_input::MapRole,
    };
    let map = |pixels: &[u8], role| {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes).write_image(pixels, 2, 2, image::ExtendedColorType::Rgba8).unwrap();
        TextureMap::new(bytes, role).unwrap()
    };
    Arc::new(
        RuntimeTextures::new(vec![TextureBinding {
            body_index: 0,
            body_name: "original sphere".into(),
            frame,
            normal_strength: 0.75,
            base_color: Some(map(&[128, 64, 255, 0, 255, 0, 32, 255, 64, 128, 192, 0, 0, 255, 128, 255], MapRole::BaseColor)),
            normal: Some(map(&[128, 128, 255, 0, 192, 128, 224, 255, 128, 192, 224, 0, 64, 96, 224, 255], MapRole::NormalOpenGl)),
        }])
        .unwrap(),
    )
}
#[test]
fn texture_admission_rejects_glass_and_stale_identity_and_honors_cancellation() {
    use solidcraft_render::job::textures::PlanarFrame;
    let source = texture_fixture(PlanarFrame { origin: [0.; 3], u: [1., 0., 0.], v: [0., 1., 0.] });
    let mut input = request(2);
    input.textures = Some(source.clone());
    let mut scene = sphere(white());
    let plain = admission(&scene, &request(2), JobOptions::default()).unwrap();
    let textured = admission(&scene, &input, JobOptions::default()).unwrap();
    assert_eq!(textured.preparation_bytes - plain.preparation_bytes, source.working_bytes());
    assert!(solidcraft_render::job::admit(&scene, &input).is_err());
    let cancel = CancelToken::default();
    cancel.cancel();
    assert_eq!(textures::flatten(Some(&source), 1, Vec3::ZERO, &cancel).unwrap_err().code, ErrorCode::Cancelled);
    scene.meshes[0].name = "stale".into();
    assert!(admission(&scene, &input, JobOptions::default()).is_err());
    scene.meshes[0].name = "original sphere".into();
    scene.meshes[0].transmission = Some(clear_glass());
    assert!(admission(&scene, &input, JobOptions::default()).is_err());
}
#[test]
fn gpu_planar_color_and_normal_maps_match_cpu_sampling_oracle() {
    use solidcraft_render::job::textures::{PlanarFrame, sample_map};
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("texture_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    for handed in [1., -1.] {
        let frame = PlanarFrame { origin: [0.; 3], u: [2., 0., 0.], v: [0., 2. * handed, 0.] };
        let source = texture_fixture(frame);
        let binding = &source.bindings()[0];
        let base = binding.base_color.as_ref().unwrap().decode(&CancelToken::default()).unwrap();
        let normals = binding.normal.as_ref().unwrap().decode(&CancelToken::default()).unwrap();
        for point in [Vec3::new(0.5, 0.5, 0.), Vec3::new(-0.25, 1.125, 0.), Vec3::new(0., 0., 0.)] {
            let mut input = request(2);
            input.textures = Some(source.clone());
            let mut task = Task::new(context.clone(), sphere(white()), input, JobOptions::default(), CancelToken::default()).unwrap();
            let mut params = vec![0; 32];
            scene::vector(&mut params, point, 0.);
            scene::vector(&mut params, Vec3::ZERO, 0.);
            scene::vector(&mut params, Vec3::Z, 0.);
            scene::vector(&mut params, Vec3::Z, 0.);
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
            let actual = task.read_linear().unwrap();
            let uv = frame.uv(point).unwrap();
            let expected = sample_map(&base, [2, 2], uv).unwrap();
            let mapped = frame.mapped_normal(Vec3::Z, Vec3::Z, sample_map(&normals, [2, 2], uv).unwrap(), 0.75).unwrap();
            for (a, b) in actual[0].iter().zip(expected) {
                assert!((f64::from(*a) - b).abs() < 2e-6, "color {actual:?} vs {expected:?}");
            }
            for (a, b) in actual[1].iter().zip([mapped.x + 2., mapped.y + 2., mapped.z + 2.]) {
                assert!((f64::from(*a) - b).abs() < 3e-6, "normal {actual:?} vs {mapped:?}");
            }
        }
    }
}

#[test]
fn gpu_finite_emitter_inside_solid_retains_entry_radiance_eta() {
    use solidcraft_render::job::{AreaLight, AreaLights, StudioLighting};
    let Some(executor) = executor() else { return };
    let surface = white();
    let receiver = ProductMesh {
        name: "shadow MIS receiver".into(),
        mesh: Arc::new(Mesh {
            positions: vec![Vec3::new(-0.01, -0.01, -2.0), Vec3::new(0.01, -0.01, -2.0), Vec3::new(0.01, 0.01, -2.0), Vec3::new(-0.01, 0.01, -2.0)],
            normals: vec![Vec3::Z; 4],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_face: vec![0, 0],
            ..Default::default()
        }),
        color: [255; 3],
        opacity: 1.0,
        surface: Some(surface),
        transmission: None,
        faces: vec![],
    };
    let light = AreaLight { center: [0.0, 0.0, 0.0], u: [9.0, 0.0, 0.0], v: [0.0, -9.0, 0.0], radiance: [1.0; 3] };
    // Broad finite emitter makes edge/refraction disagreement small; the missing eta term is a 125% error.
    let mut expected = 0.0;
    for y in 0..128 {
        for x in 0..128 {
            let delta = Vec3::new(-9.0 + (f64::from(x) + 0.5) * 18.0 / 128.0, -9.0 + (f64::from(y) + 0.5) * 18.0 / 128.0, 2.0);
            let l = delta.normalized().unwrap();
            let r = solidcraft_render::transmission::fresnel(l.z, 1.0, 1.5).unwrap().reflectance();
            expected += (1.0 - r) / 2.25 * surface.brdf(Vec3::Z, Vec3::Z, l).unwrap().x * l.z * l.z / delta.len2() * 324.0 / 16384.0;
        }
    }
    for (thin, enabled, depth) in [(false, true, 1), (false, true, 4), (false, false, 4)] {
        let mut scene = glass_slab(thin);
        scene.meshes[0].surface = Some(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap());
        scene.meshes[0].transmission =
            Some(solidcraft_render::transmission::Transmission::new(Vec3::ZERO, thin.then_some(0.0)).unwrap().with_shadow_transmission(enabled));
        scene.meshes.push(receiver.clone());
        let options = JobOptions {
            samples: 4096,
            max_depth: depth,
            seed: 37,
            lighting: Some(StudioLighting { sun: None, environment: [0.0; 3], area_lights: AreaLights::new(&[light]).unwrap() }),
            ..Default::default()
        };
        let mut image = request(1);
        image.camera.target = Vec3::new(0.0, 0.0, -2.0);
        image.camera.distance = 0.1;
        image.camera.fov = 0.001;
        let mut task = Task::new(executor.context.clone(), scene, image, options, CancelToken::default()).unwrap();
        for _ in 0..options.samples {
            task.dispatch().unwrap();
        }
        let actual = f64::from(task.read_linear().unwrap()[0][0]);
        assert!((actual - expected).abs() < 0.08 * expected, "thin={thin}, enabled={enabled}, depth={depth}: {actual} vs {expected}");
    }
}

#[test]
fn path_device_limits_require_five_storage_buffers_without_changing_raster_defaults() {
    let mut supported = wgpu::Limits::downlevel_defaults();
    assert_eq!(supported.max_storage_buffers_per_shader_stage, 4);
    assert!(device_limits(supported.clone()).is_err());
    supported.max_storage_buffers_per_shader_stage = 8;
    let requested = device_limits(supported.clone()).unwrap();
    let mut expected = wgpu::Limits::downlevel_defaults().using_resolution(supported);
    expected.max_storage_buffers_per_shader_stage = 5;
    assert_eq!(requested, expected);
}

#[test]
fn gpu_secondary_closed_corner_exits_are_not_skipped() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("closed corner boundary oracle"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("boundary_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    for case in 0..3 {
        let pyramid = case == 1;
        let mut scene = glass_slab(false);
        let (origin, direction) = if pyramid {
            let mut mesh = Mesh::default();
            let corners = [Vec3::new(-1., -1., 1.), Vec3::new(1., -1., 1.), Vec3::new(1., 1., 1.), Vec3::new(-1., 1., 1.)];
            let tip = Vec3::new(0., 0., -1.);
            let faces = [
                [corners[0], corners[1], corners[2]],
                [corners[0], corners[2], corners[3]],
                [corners[1], corners[0], tip],
                [corners[2], corners[1], tip],
                [corners[3], corners[2], tip],
                [corners[0], corners[3], tip],
            ];
            for face in faces {
                let n = (face[1] - face[0]).cross(face[2] - face[0]).normalized().unwrap();
                let first = mesh.positions.len() as u32;
                mesh.positions.extend(face);
                mesh.normals.extend([n; 3]);
                mesh.triangles.push([first, first + 1, first + 2]);
                mesh.tri_face.push(0);
            }
            scene.meshes[0].mesh = Arc::new(mesh);
            scene.bounds = scene.meshes[0].mesh.bounds();
            (Vec3::new(0.9999, 0., 2.), -Vec3::Z)
        } else {
            (Vec3::new(9.9999, if case == 2 { 9.9999 } else { 0. }, 2.), Vec3::new(1., 0., -1.).normalized().unwrap())
        };
        let mut task = Task::new(context.clone(), scene, request(2), JobOptions::default(), CancelToken::default()).unwrap();
        let mut params = vec![0; 32];
        scene::vector(&mut params, origin, 0.);
        scene::vector(&mut params, direction, 0.);
        scene::vector(&mut params, Vec3::ZERO, 0.);
        scene::vector(&mut params, -Vec3::Z, 0.);
        scene::vector(&mut params, Vec3::new(0.001, 0., 0.), 0.);
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
        let values = task.read_linear().unwrap();
        let result = values[0];
        let expected_distance = if pyramid { 0.0002 } else { 0.0001 * 2.0_f32.sqrt() };
        assert!((values[1][0] - expected_distance).abs() < 3e-6, "case={case}: {:?}", values);
        assert_eq!(result, if pyramid { [0., 0., 1., 1.] } else { [0., 1., 1., 1.] }, "pyramid={pyramid}");
    }
}

#[test]
fn gpu_triangle_bound_away_rejects_rounded_behind_origin_hit() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("positive triangle oracle"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("positive_triangle_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    // Actual stored f32 tank-edge failure. Ray starts at the minimum x and moves
    // away, yet the projected distance numerator previously rounded positive.
    let vertices = [[218.38487_f32, -3.8715396, -86.], [220.27007, -2.9226143, 85.], [218.38487, -3.8715396, 85.]];
    let origin = [218.38487_f32, -3.871542, -67.5458];
    let direction = [-0.3121568_f32, -0.45083842, 0.8362433];
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let transform =
                |v: [f32; 3]| Vec3::new(f64::from(v[axis]) * sign, f64::from(v[(axis + 1) % 3]) * sign, f64::from(v[(axis + 2) % 3]) * sign);
            let points = vertices.map(transform);
            let d = transform(direction);
            let n = (points[1] - points[0]).cross(points[2] - points[0]).normalized().unwrap();
            let mesh = Mesh { positions: points.to_vec(), normals: vec![n; 3], triangles: vec![[0, 1, 2]], tri_face: vec![0], ..Default::default() };
            let mut scene = sphere_grid(white(), 2, 3);
            scene.bounds = mesh.bounds();
            scene.meshes[0].mesh = Arc::new(mesh);
            for positive in [false, true] {
                // Close positive intersections remain admitted; no epsilon is used.
                let o = if positive { (points[0] + points[1] + points[2]) / 3.0 - d * 0.001 } else { transform(origin) };
                let mut task = Task::new(context.clone(), scene.clone(), request(1), JobOptions::default(), CancelToken::default()).unwrap();
                let mut params = vec![0; 32];
                scene::vector(&mut params, o, 0.);
                scene::vector(&mut params, d, 0.);
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
                let values = task.read_linear().unwrap();
                assert_eq!(values[0][0], if positive { 1.0 } else { 0.0 }, "axis={axis} sign={sign} positive={positive}");
            }
        }
    }
}

#[test]
fn gpu_oblique_coplanar_grazing_reflection_does_not_rehit() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("coplanar boundary oracle"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("coplanar_boundary_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let a = Vec3::new(f64::from(191.3_f32), f64::from(79.2_f32), f64::from(84.7_f32));
    let points = [a, a + Vec3::new(8., 0., 2.), a + Vec3::new(8., 8., 6.), a + Vec3::new(0., 8., 4.)];
    let n = Vec3::new(-0.25, -0.5, 1.).normalized().unwrap();
    let d = Vec3::new(f64::from(-0.69632584_f32), f64::from(0.69628024_f32), f64::from(0.17413844_f32));
    assert!(n.dot(d) > 0.0, "outward reflection");
    let mesh =
        Mesh { positions: points.to_vec(), normals: vec![n; 4], triangles: vec![[0, 1, 2], [0, 2, 3]], tri_face: vec![0, 0], ..Default::default() };
    let mut scene = sphere_grid(white(), 2, 3);
    scene.bounds = mesh.bounds();
    scene.meshes[0].mesh = Arc::new(mesh);
    let mut task = Task::new(context.clone(), scene, request(2), JobOptions::default(), CancelToken::default()).unwrap();
    let mut params = vec![0; 32];
    scene::vector(&mut params, Vec3::new(f64::from(1.734274e-6_f32), f64::from(0.23548841_f32), 0.), 0.);
    scene::vector(&mut params, d, 0.);
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
    let values = task.read_linear().unwrap();
    // A source point on this exact stored plane and an outward ray cannot hit
    // its adjacent coplanar triangle at any strictly positive distance.
    assert_eq!(values[0][0], 0.0, "hit/distance={:?}, reconstructed={:?}", values[0], values[1]);
}

#[test]
fn gpu_shadow_closed_corner_keeps_short_exit_segment() {
    let Some(executor) = executor() else { return };
    let mut scene = glass_slab(false);
    let optics = scene.meshes[0].transmission.unwrap().with_shadow_transmission(true);
    scene.meshes[0].transmission = Some(optics);
    let cosine = std::f64::consts::FRAC_1_SQRT_2;
    let actual = shadow_probe(&executor.context, scene, Vec3::new(8.4999, 0., 2.), Vec3::new(cosine, 0., -cosine), 4.);
    let expected = solidcraft_render::transmission::solid_shadow(optics, 0.0001 * 2.0_f64.sqrt(), cosine, cosine, 1.5).unwrap();
    assert_eq!(actual[1][1], 2.0, "{actual:?}");
    for (a, b) in actual[0].iter().zip([expected.x, expected.y, expected.z]) {
        assert!((f64::from(*a) - b).abs() < 2e-5, "{actual:?} vs {expected:?}");
    }
}

#[test]
fn gpu_shadow_endpoint_near_interfaces_uses_true_distance() {
    let Some(executor) = executor() else { return };
    let mut scene = glass_slab(false);
    let optics = scene.meshes[0].transmission.unwrap().with_shadow_transmission(true);
    scene.meshes[0].transmission = Some(optics);
    // shadow_probe deliberately uses epsilon .01. Both offsets are smaller
    // than its old 2epsilon cutoff, and must still classify the actual endpoint.
    for (limit, length, factor, crossings) in
        [(1.495, 0.0, 1.0, 0.0), (1.505, 0.005, 0.96 / 2.25, 1.0), (2.495, 0.995, 0.96 / 2.25, 1.0), (2.505, 1.0, 0.96 * 0.96, 2.0)]
    {
        let actual = shadow_probe(&executor.context, scene.clone(), Vec3::new(0., 0., 2.), -Vec3::Z, limit);
        let expected = optics.attenuation(length).unwrap() * factor;
        assert_eq!(actual[1][1], crossings, "limit={limit}: {actual:?}");
        for (a, b) in actual[0].iter().zip([expected.x, expected.y, expected.z]) {
            assert!((f64::from(*a) - b).abs() < 2e-5, "limit={limit}: {actual:?} vs {expected:?}");
        }
    }
}

#[test]
fn gpu_long_ray_thin_corner_orders_entry_before_exit() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("long-ray ordering oracle"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("ordering_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let origin = Vec3::new(f64::from(f32::from_bits(0x43ad5626)), f64::from(f32::from_bits(0x44737912)), f64::from(f32::from_bits(0xc2dd2710)));
    let direction = Vec3::new(f64::from(f32::from_bits(0xbe341340)), f64::from(f32::from_bits(0xbf762fdb)), f64::from(f32::from_bits(0x3e577470)));
    // Captured stored-f32 tank side entry and top exit. Independent f64 plane
    // distances are 929.5224413721813 and 929.5225421330013 respectively.
    let side = [Vec3::new(243., 80., -86.), Vec3::new(157., 80., 85.), Vec3::new(243., 80., 85.)];
    let top = [Vec3::new(157., 80., 85.), Vec3::new(160., 77., 85.), Vec3::new(243., 80., 85.)];
    for permutation in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
        let odd = (permutation[0] > permutation[1]) ^ (permutation[0] > permutation[2]) ^ (permutation[1] > permutation[2]);
        for reflection in 0..8_u32 {
            let sign = |axis: u32| if reflection & (1 << axis) == 0 { 1.0 } else { -1.0 };
            let transform = |v: Vec3| {
                let a = [v.x, v.y, v.z];
                Vec3::new(a[permutation[0]] * sign(0), a[permutation[1]] * sign(1), a[permutation[2]] * sign(2))
            };
            let invert = odd ^ (reflection.count_ones() % 2 != 0);
            for reverse in [false, true] {
                let mut mesh = Mesh::default();
                for face in if reverse { [top, side] } else { [side, top] } {
                    let mut face = face.map(transform);
                    if invert {
                        face.swap(1, 2);
                    }
                    let n = (face[1] - face[0]).cross(face[2] - face[0]).normalized().unwrap();
                    let i = mesh.positions.len() as u32;
                    mesh.positions.extend(face);
                    mesh.normals.extend([n; 3]);
                    mesh.triangles.push([i, i + 1, i + 2]);
                    mesh.tri_face.push(0);
                }
                let mut scene = sphere_grid(white(), 2, 3);
                scene.bounds = mesh.bounds();
                scene.meshes[0].mesh = Arc::new(mesh);
                let mut task = Task::new(context.clone(), scene, request(2), JobOptions::default(), CancelToken::default()).unwrap();
                let mut params = vec![0; 32];
                scene::vector(&mut params, transform(origin), 0.);
                scene::vector(&mut params, transform(direction), 0.);
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
                let values = task.read_linear().unwrap();
                assert_eq!(
                    values[0][0],
                    if reverse { 1.0 } else { 0.0 },
                    "permutation={permutation:?}, reflection={reflection}, reverse={reverse}, best/candidates={values:?}"
                );
                assert_eq!(values[1][1], 1.0, "candidate0 rejected: {values:?}");
                assert_eq!(values[2][1], 1.0, "candidate1 rejected: {values:?}");
            }
        }
    }
}
#[test]
fn gpu_long_ray_near_corner_refraction_keeps_real_exit() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("refracted corner oracle"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("refracted_corner_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let origin = Vec3::new(f64::from(f32::from_bits(0x43ad5626)), f64::from(f32::from_bits(0x44737912)), f64::from(f32::from_bits(0xc2dd2710)));
    let direction = Vec3::new(f64::from(f32::from_bits(0xbe341340)), f64::from(f32::from_bits(0xbf762fdb)), f64::from(f32::from_bits(0x3e577470)));
    let ior = f64::from(1.586_f32);
    let ni = direction.x / ior;
    let nk = direction.z / ior;
    let refracted = Vec3::new(ni, -(1.0 - ni * ni - nk * nk).sqrt(), nk);
    let entry_distance = (80.0 - origin.y) / direction.y;
    let entry_z = origin.z + direction.z * entry_distance;
    let exit_distance = (85.0 - entry_z) / refracted.z;
    // Independent f64 plane/refraction oracle: true entry clearance is ~21um.
    assert!(85.0 - entry_z > 0.00002 && 85.0 - entry_z < 0.000022);
    let faces = [
        [[157., 0., 85.], [243., 0., 85.], [243., 80., 85.], [157., 80., 85.]],
        [[157., 80., -86.], [243., 80., -86.], [243., 0., -86.], [157., 0., -86.]],
        [[243., 80., -86.], [157., 80., -86.], [157., 80., 85.], [243., 80., 85.]],
        [[157., 0., -86.], [243., 0., -86.], [243., 0., 85.], [157., 0., 85.]],
        [[243., 0., -86.], [243., 80., -86.], [243., 80., 85.], [243., 0., 85.]],
        [[157., 80., -86.], [157., 0., -86.], [157., 0., 85.], [157., 80., 85.]],
    ];
    for permutation in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
        let odd = (permutation[0] > permutation[1]) ^ (permutation[0] > permutation[2]) ^ (permutation[1] > permutation[2]);
        for reflection in 0..8_u32 {
            let sign = |axis: u32| if reflection & (1 << axis) == 0 { 1.0 } else { -1.0 };
            let transform = |v: Vec3| {
                let a = [v.x, v.y, v.z];
                Vec3::new(a[permutation[0]] * sign(0), a[permutation[1]] * sign(1), a[permutation[2]] * sign(2))
            };
            let invert = odd ^ (reflection.count_ones() % 2 != 0);
            let mut mesh = Mesh::default();
            for face in faces {
                for indices in [[0, 1, 2], [0, 2, 3]] {
                    let mut p = indices.map(|i| {
                        let a = face[i];
                        transform(Vec3::new(a[0], a[1], a[2]))
                    });
                    if invert {
                        p.swap(1, 2);
                    }
                    let n = (p[1] - p[0]).cross(p[2] - p[0]).normalized().unwrap();
                    let i = mesh.positions.len() as u32;
                    mesh.positions.extend(p);
                    mesh.normals.extend([n; 3]);
                    mesh.triangles.push([i, i + 1, i + 2]);
                    mesh.tri_face.push(0);
                }
            }
            let mut scene = glass_slab(false);
            scene.bounds = mesh.bounds();
            scene.meshes[0].mesh = Arc::new(mesh);
            scene.meshes[0].surface = Some(OpaqueSurface::new(Vec3::ZERO, 0., 0.1, ior).unwrap());
            scene.meshes[0].transmission = Some(solidcraft_render::transmission::Transmission::new(Vec3::ZERO, None).unwrap());
            let mut task = Task::new(context.clone(), scene, request(2), JobOptions::default(), CancelToken::default()).unwrap();
            let mut params = vec![0; 32];
            scene::vector(&mut params, transform(origin), 0.);
            scene::vector(&mut params, transform(direction), 0.);
            scene::vector(&mut params, Vec3::ZERO, 0.);
            scene::vector(&mut params, Vec3::ZERO, 0.);
            scene::vector(&mut params, Vec3::new(0.00084, 0., 0.), 0.);
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
            let values = task.read_linear().unwrap();
            assert_eq!(values[0], [1.; 4], "permutation={permutation:?} reflection={reflection}: {values:?}");
            let z_axis = permutation.iter().position(|axis| *axis == 2).unwrap();
            assert!(values[2][z_axis] < 85.0, "rounded entry onto top edge: {values:?}");
            // Two position ULP at z85, propagated through the refracted angle.
            let tolerance = 2.0 * f64::from(f32::from_bits(85.0_f32.to_bits() + 1) - 85.0) / refracted.z;
            assert!(
                (f64::from(values[1][0]) - exit_distance).abs() < tolerance,
                "distance={:?} expected={exit_distance} tolerance={tolerance}",
                values[1]
            );
            let expected = transform(refracted);
            for (a, b) in values[3].iter().zip([expected.x, expected.y, expected.z]) {
                assert!((f64::from(*a) - 2.0 - b).abs() < 2e-6);
            }
        }
    }
}
#[test]
fn gpu_rib_total_internal_reflection_retains_origin_residual() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("rib residual oracle"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("rib_residual_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let a = Vec3::new(-162., -53., -127.);
    let b = Vec3::new(f64::from(-163.7_f32), -55., -55.);
    let c = Vec3::new(f64::from(-163.7_f32), -55., -127.);
    let other = Vec3::new(-165.5, -53., -55.);
    let bary = Vec3::new(f64::from(f32::from_bits(0x369bafd4)), f64::from(f32::from_bits(0x3f59c40b)), f64::from(f32::from_bits(0x3e18ee9d)));
    let incoming = Vec3::new(f64::from(f32::from_bits(0xbeba12cc)), f64::from(f32::from_bits(0xbf6e1a19)), f64::from(f32::from_bits(0xbd5b2b1e)));
    // These are captured GPU barycentrics, not an idealized primary-ray hit.
    let point = b + (a - b) * bary.x + (c - b) * bary.z;
    let normal = (b - a).cross(c - a).normalized().unwrap();
    let outgoing = incoming - normal * (2.0 * incoming.dot(normal));
    let exit_normal = (other - b).cross(c - b).normalized().unwrap();
    let expected = -(point - b).dot(exit_normal) / outgoing.dot(exit_normal);
    assert!(expected > 1e-5 && expected < 1.5e-5);
    let cross_section = [[-162., -53.], [f64::from(-163.7_f32), -55.], [-165.5, -53.]];
    let mut faces = Vec::new();
    let lower = cross_section.map(|p| Vec3::new(p[0], p[1], -127.));
    let upper = cross_section.map(|p| Vec3::new(p[0], p[1], -55.));
    faces.push(lower);
    faces.push([upper[0], upper[2], upper[1]]);
    for i in 0..3 {
        let j = (i + 1) % 3;
        faces.push([lower[i], upper[i], upper[j]]);
        faces.push([lower[i], upper[j], lower[j]]);
    }
    for permutation in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
        let odd = (permutation[0] > permutation[1]) ^ (permutation[0] > permutation[2]) ^ (permutation[1] > permutation[2]);
        for reflection in 0..8_u32 {
            let sign = |axis: u32| if reflection & (1 << axis) == 0 { 1.0 } else { -1.0 };
            let transform = |v: Vec3| {
                let a = [v.x, v.y, v.z];
                Vec3::new(a[permutation[0]] * sign(0), a[permutation[1]] * sign(1), a[permutation[2]] * sign(2))
            };
            let invert = odd ^ (reflection.count_ones() % 2 != 0);
            let mut mesh = Mesh::default();
            for face in &faces {
                let mut p = face.map(transform);
                if invert {
                    p.swap(1, 2);
                }
                let n = (p[1] - p[0]).cross(p[2] - p[0]).normalized().unwrap();
                let i = mesh.positions.len() as u32;
                mesh.positions.extend(p);
                mesh.normals.extend([n; 3]);
                mesh.triangles.push([i, i + 1, i + 2]);
                mesh.tri_face.push(0);
            }
            let mut scene = glass_slab(false);
            scene.bounds = mesh.bounds();
            scene.meshes[0].mesh = Arc::new(mesh);
            scene.meshes[0].surface = Some(OpaqueSurface::new(Vec3::ZERO, 0., 0.1, f64::from(1.49_f32)).unwrap());
            scene.meshes[0].transmission = Some(solidcraft_render::transmission::Transmission::new(Vec3::ZERO, None).unwrap());
            let mut task = Task::new(context.clone(), scene, request(2), JobOptions::default(), CancelToken::default()).unwrap();
            let mut params = vec![0; 32];
            scene::vector(&mut params, transform(a), 0.);
            scene::vector(&mut params, transform(if invert { c } else { b }), 0.);
            scene::vector(&mut params, transform(if invert { b } else { c }), 0.);
            scene::vector(&mut params, transform(incoming), 0.);
            scene::vector(&mut params, Vec3::new(0.00084, 0., 0.), 0.);
            scene::vector(&mut params, if invert { Vec3::new(bary.x, bary.z, bary.y) } else { bary }, 0.);
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
            let values = task.read_linear().unwrap();
            assert_eq!(values[0], [0., 1., 1., 1.], "permutation={permutation:?} reflection={reflection}: {values:?}");
            assert_eq!(values[1][1], 1.0, "must be TIR: {values:?}");
            assert!((f64::from(values[1][0]) - expected).abs() < 2e-7, "expected={expected}: {values:?}");
        }
    }
}
#[test]
fn gpu_integer_binary32_add_sub_matches_hardware_bits() {
    use wgpu::util::DeviceExt;
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let mut pairs = Vec::new();
    let fractions = [0, 1, 0x3fffff, 0x400000, 0x400001, 0x7ffffe, 0x7fffff];
    for exponent in [0u32, 1, 2, 23, 24, 25, 31, 32, 63, 126, 127, 128, 253, 254, 255] {
        for gap in [0, 1, 23, 24, 25, 31, 32, 63, 254] {
            for a_fraction in fractions {
                for b_fraction in fractions {
                    let a = exponent << 23 | a_fraction;
                    let b = exponent.saturating_sub(gap) << 23 | b_fraction;
                    pairs.extend([[a, b], [a ^ 0x80000000, b], [b, a], [b, a ^ 0x80000000]]);
                }
            }
        }
    }
    pairs.extend([[0x80000000, 0x80000000], [0, 0x80000000], [1, 0x80000001]]);
    let mut state = 0x9e3779b9u32;
    while pairs.len() < 65536 {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        let a = state;
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        pairs.push([a, state]);
    }
    assert_eq!(pairs.len(), 65536);
    let mut bytes = Vec::with_capacity(4 + pairs.len() * 8);
    bytes.extend((pairs.len() as u32).to_le_bytes());
    for pair in &pairs {
        for word in pair {
            bytes.extend(word.to_le_bytes());
        }
    }
    let input = context.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("integer binary32 inputs"),
        contents: &bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("integer binary32 oracle"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("integer_float_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut task = Task::new(context.clone(), glass_slab(false), request(256), JobOptions::default(), CancelToken::default()).unwrap();
    let bind = context.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("integer binary32 oracle"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 1, resource: input.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: task.accumulation.as_entire_binding() },
        ],
    });
    let mut encoder = context.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(1024, 1, 1);
    }
    context.queue.submit([encoder.finish()]);
    task.sample = 1;
    let values = task.read_linear().unwrap();
    let bits = |value: f32| if value.is_nan() { 0x7fc00000 } else { value.to_bits() };
    for (i, ([a, b], value)) in pairs.iter().zip(values).enumerate() {
        let a_float = f32::from_bits(*a);
        let b_float = f32::from_bits(*b);
        let sum = value[0] as u32 | (value[1] as u32) << 16;
        let difference = value[2] as u32 | (value[3] as u32) << 16;
        assert_eq!(sum, bits(a_float + b_float), "sum index={i} a={a:08x} b={b:08x}");
        assert_eq!(difference, bits(a_float - b_float), "difference index={i} a={a:08x} b={b:08x}");
    }
}
#[test]
fn gpu_emitter_short_segment_beer_and_area_pdf() {
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("short emitter segment oracle"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = context.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&context.layout)],
        immediate_size: 0,
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("emitter_segment_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut task = Task::new(context.clone(), glass_slab(false), request(1), JobOptions::default(), CancelToken::default()).unwrap();
    let mut encoder = context.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &task.bind, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    context.queue.submit([encoder.finish()]);
    task.sample = 1;
    let result = task.read_linear().unwrap()[0];
    let ray = Vec3::new(f64::from(0.6f32), 0., f64::from(0.8f32));
    let distance = f64::from(0.00001f32);
    let length = ray.len() * distance;
    let cosine = ray.z / ray.len();
    let expected = [length, (-5000. * length).exp(), length * length / (16. * cosine)];
    for i in 0..3 {
        assert!((f64::from(result[i]) / expected[i] - 1.).abs() < 2e-6, "channel={i} expected={expected:?} result={result:?}");
    }
}
#[test]
fn gpu_integer_relative_fast_paths_match_ordered_rounding() {
    use wgpu::util::DeviceExt;
    let Some(executor) = executor() else { return };
    let context = &executor.context;
    let mut quads = Vec::new();
    let fractions = [0, 1, 0x3fffff, 0x400000, 0x400001, 0x7ffffe, 0x7fffff];
    for exponent in [0u32, 1, 2, 23, 24, 25, 126, 127, 128, 253, 254, 255] {
        for fraction in fractions {
            let a = exponent << 23 | fraction;
            for b in [a, a ^ 1, a.wrapping_add(0x800000), a ^ 0x80000000, 0, 0x80000000] {
                for offset in [0, 0x80000000, 1, 0x80000001, 0x3f800000, 0xbf800000, a, b] {
                    quads.extend([[a, 0, b, offset], [a, offset, b, 0], [a, offset, a, b], [a, b, b, a]]);
                }
            }
        }
    }
    let mut state = 0x12345678u32;
    while quads.len() < 65536 {
        let mut next = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            state
        };
        let mut quad = [next(), next(), next(), next()];
        match quads.len() % 5 {
            0 => quad[2] = quad[0],
            1 => {
                quad[1] = 0;
                quad[3] = 0;
            }
            2 => quad[1] = 0,
            3 => {
                quad[0] = 0x3f800000 | (quad[0] & 0x7fffff);
                quad[2] = quad[0] ^ 1;
            }
            _ => {}
        }
        quads.push(quad);
    }
    assert_eq!(quads.len(), 65536);
    let mut bytes = Vec::with_capacity(4 + quads.len() * 16);
    bytes.extend((quads.len() as u32).to_le_bytes());
    for quad in &quads {
        for word in quad {
            bytes.extend(word.to_le_bytes());
        }
    }
    let input = context.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("relative coordinate inputs"),
        contents: &bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("relative coordinate oracle"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("integer_relative_reference"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut task = Task::new(context.clone(), glass_slab(false), request(256), JobOptions::default(), CancelToken::default()).unwrap();
    let bind = context.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("relative coordinate oracle"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 1, resource: input.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: task.accumulation.as_entire_binding() },
        ],
    });
    let mut encoder = context.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(1024, 1, 1);
    }
    context.queue.submit([encoder.finish()]);
    task.sample = 1;
    let values = task.read_linear().unwrap();
    for (i, (quad, value)) in quads.iter().zip(values).enumerate() {
        let reference = value[0] as u32 | (value[1] as u32) << 16;
        let candidate = value[2] as u32 | (value[3] as u32) << 16;
        let [a, al, b, bl] = quad.map(f32::from_bits);
        let anchor = a - b;
        let offset = al - bl;
        let result = anchor + offset;
        let expected = if result.is_nan() { 0x7fc00000 } else { result.to_bits() };
        assert_eq!(reference, expected, "reference index={i} input={quad:08x?}");
        assert_eq!(candidate, expected, "candidate index={i} input={quad:08x?}");
    }
}
