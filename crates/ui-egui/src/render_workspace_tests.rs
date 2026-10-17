use super::*;
use crate::scenario::Harness;
use solidcraft_engine::Session;

#[test]
fn image_method_defaults_and_request_building_leave_document_and_physics_unchanged() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length":10,"width":20,"height":30,"body_name":"Product"})).unwrap();
    let physical = h.app.session.execute("component.properties", &json!({})).unwrap();
    let doc = h.app.session.doc.clone();
    let history = (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty());
    assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raster);
    h.app.render_workspace.view.transparent = true;
    let check = image_params(&h.app).unwrap();
    let start = image_params(&h.app).unwrap();
    assert_eq!(check, start);
    assert!(check.get("integrator").is_none() && check.get("raytrace").is_none());
    assert_eq!(check["transparent_background"], true);
    h.app.render_workspace.view.raytrace.exposure_ev = 3.0;
    h.app.render_workspace.view.raytrace.shadows = false;
    assert_eq!(image_params(&h.app).unwrap(), check, "CPU-only draft does not leak into raster");
    h.app.render_workspace.view.integrator = Integrator::Raytrace;
    let _ = image_params(&h.app);
    assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
    assert_eq!(history, (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty()));
    assert_eq!(h.app.session.execute("component.properties", &json!({})).unwrap(), physical);
}

#[test]
fn image_method_rejects_missing_and_cooperative_hosts_without_fallback() {
    let mut h = Harness::new();
    h.app.session.set_render_job_host(Some(CpuHost::cooperative())).unwrap();
    h.app.render_workspace.host = None;
    assert!(image_params(&h.app).is_ok(), "raster does not require a GPU");
    h.app.render_workspace.view.integrator = Integrator::Raytrace;
    assert!(image_params(&h.app).is_err());
    assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raytrace);
    h.app.session.set_render_job_host(None).unwrap();
    assert!(image_params(&h.app).is_err());
    h.app.render_workspace.view.integrator = Integrator::Raster;
    assert!(image_params(&h.app).is_err(), "absence of any image host is explicit");
}

#[test]
fn image_check_invalidates_on_request_visibility_revision_or_host_change() {
    let mut h = Harness::new();
    let params = image_params(&h.app).unwrap();
    remember_image_check(&mut h.app, params.clone());
    invalidate_image_check(&mut h.app, Some(&params));
    assert!(h.app.render_workspace.view.checked_image.is_some());
    h.app.cam.yaw += 0.1;
    let changed = image_params(&h.app).unwrap();
    invalidate_image_check(&mut h.app, Some(&changed));
    assert!(h.app.render_workspace.view.checked_image.is_none());
    remember_image_check(&mut h.app, changed.clone());
    h.app.session.visibility.hidden_bodies.push("Product".into());
    invalidate_image_check(&mut h.app, Some(&changed));
    assert!(h.app.render_workspace.view.checked_image.is_none());
    remember_image_check(&mut h.app, changed.clone());
    h.app.session.revision += 1;
    invalidate_image_check(&mut h.app, Some(&changed));
    assert!(h.app.render_workspace.view.checked_image.is_none());
    remember_image_check(&mut h.app, changed.clone());
    h.app.session.set_render_job_host(Some(CpuHost::cooperative())).unwrap();
    invalidate_image_check(&mut h.app, Some(&changed));
    assert!(h.app.render_workspace.view.checked_image.is_none());
    remember_image_check(&mut h.app, changed.clone());
    h.app.session.doc = Arc::new((*h.app.session.doc).clone());
    invalidate_image_check(&mut h.app, Some(&changed));
    assert!(h.app.render_workspace.view.checked_image.is_none(), "equal contents in a different document invalidate Check");
    remember_image_check(&mut h.app, changed);
    invalidate_image_check(&mut h.app, None);
    assert!(h.app.render_workspace.view.checked_image.is_none());
}

#[test]
fn image_method_document_reset_clears_runtime_options_and_keeps_existing_host() {
    let mut h = Harness::new();
    let host = h.app.session.render_job_host().unwrap().clone();
    let params = image_params(&h.app).unwrap();
    remember_image_check(&mut h.app, params);
    h.app.render_workspace.view.integrator = Integrator::Raytrace;
    h.app.render_workspace.view.raytrace.exposure_ev = 4.0;
    h.app.render_workspace.view.raytrace.shadows = false;
    reset_document(&mut h.app);
    assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raster);
    assert_eq!(h.app.render_workspace.view.raytrace, RaytraceOptions::default());
    assert!(h.app.render_workspace.view.checked_image.is_none());
    assert!(h.app.session.render_job_host().unwrap().same_host(&host));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn cpu_image_request_uses_actual_host_and_refuses_unsupported_ui_options() {
    let mut h = Harness::new();
    let native = CpuHost::for_platform().unwrap();
    h.app.session.set_render_job_host(Some(native.clone())).unwrap();
    h.app.render_workspace.image_host = Some(native);
    h.app.render_workspace.host = None;
    h.app.session.set_render_viewport(None).unwrap();
    h.app.render_workspace.view.integrator = Integrator::Raytrace;
    let available = h.app.session.render_job_host().unwrap().validate_integrator(Integrator::Raytrace).is_ok();
    if !available {
        assert!(image_params(&h.app).is_err(), "feature-OFF cannot silently select raster");
        return;
    }
    h.app.render_workspace.view.draft.width = 16;
    h.app.render_workspace.view.draft.height = 16;
    h.app.render_workspace.view.raytrace.preset = Some(LightingPreset::WarmCool);
    h.app.render_workspace.view.raytrace.shadows = false;
    h.app.render_workspace.view.raytrace.exposure_ev = 2.0;
    let params = image_params(&h.app).unwrap();
    assert_eq!(params, image_params(&h.app).unwrap());
    assert_eq!(params["integrator"], "raytrace");
    assert_eq!(params["raytrace"]["preset"], "warm_cool");
    assert_eq!(params["raytrace"]["shadows"], false);
    assert_eq!(params["raytrace"]["exposure_ev"], 2.0);
    let preflight = h.app.session.execute("render.preflight", &params).unwrap();
    assert_eq!(preflight["resolved"]["executor"], "cpu-raytrace");
    remember_image_check(&mut h.app, params);
    h.app.render_workspace.view.raytrace.shadows = true;
    let changed = image_params(&h.app).unwrap();
    invalidate_image_check(&mut h.app, Some(&changed));
    assert!(h.app.render_workspace.view.checked_image.is_none());
    h.app.render_workspace.view.transparent = true;
    assert!(image_params(&h.app).unwrap_err().contains(h.app.tr("render.cpu_opaque_required")));
    h.app.render_workspace.view.integrator = Integrator::Raster;
    assert_eq!(image_params(&h.app).unwrap()["transparent_background"], true);
    h.app.render_workspace.view.integrator = Integrator::Raytrace;
    h.app.render_workspace.view.transparent = false;
    h.app.render_workspace.view.raytrace.exposure_ev = f64::NAN;
    assert!(image_params(&h.app).is_err());
    h.app.render_workspace.view.raytrace.exposure_ev = 0.0;
    h.app.render_workspace.view.raytrace.lights = Some(Vec::new());
    assert!(image_params(&h.app).unwrap_err().contains(h.app.tr("render.cpu_options_pending")));
    for options in [
        RaytraceOptions { punctual_lights: Some(Vec::new()), ..Default::default() },
        RaytraceOptions { area_lights: Some(Vec::new()), ..Default::default() },
        RaytraceOptions { area_samples: Some(4), ..Default::default() },
        RaytraceOptions { seed: Some(0), ..Default::default() },
    ] {
        h.app.render_workspace.view.raytrace = options;
        assert!(image_params(&h.app).unwrap_err().contains(h.app.tr("render.cpu_options_pending")));
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn clicked_cpu_method_check_and_start_use_the_same_native_request_without_gpu() {
    let mut h = Harness::new();
    let native = CpuHost::for_platform().unwrap();
    h.app.session.set_render_job_host(Some(native.clone())).unwrap();
    h.app.render_workspace.image_host = Some(native);
    h.call("ui.resize", json!({"width":1600,"height":1400}));
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Product"})).unwrap();
    h.app.session.mark_saved();
    enter(&mut h.app, true).unwrap();
    h.app.render_workspace.host = None;
    h.app.session.set_render_viewport(None).unwrap();
    h.app.render_workspace.view.draft.width = 16;
    h.app.render_workspace.view.draft.height = 16;
    h.app.render_workspace.view.dirty = true;
    h.frames(3);
    let available = image_host(&h.app, Integrator::Raytrace).is_ok();
    h.step(&json!({"click":{"handle":"render:method"}})).unwrap();
    h.step(&json!({"click":{"handle":"render:method-cpu"}})).unwrap();
    if !available {
        assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raster, "feature-OFF choice is disabled");
        return;
    }
    assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raytrace);
    let doc = h.app.session.doc.clone();
    let history = (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty());
    let check = h.app.tr("render.image_check").to_string();
    let start = h.app.tr("render.image_start").to_string();
    h.step(&json!({"widget":check})).unwrap();
    let params = h.app.render_workspace.view.checked_image.as_ref().unwrap().params.clone();
    h.step(&json!({"widget":start})).unwrap();
    let job = h.app.render_workspace.view.job.unwrap();
    let status = h.app.session.render_job_handle(job).unwrap().wait_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(status.progress.phase, solidcraft_engine::render::job::Phase::Completed);
    assert_eq!(status.resolved.executor.as_str(), "cpu-raytrace");
    assert_eq!(serde_json::to_value(status.request.integrator).unwrap(), params["integrator"]);
    assert_eq!(serde_json::to_value(status.request.raytrace).unwrap(), params["raytrace"]);
    h.frames(3);
    let export = h.app.tr("render.image_export").to_string();
    h.step(&json!({"widget":export})).unwrap();
    assert_eq!(&std::fs::read(h.dir.join("render.png")).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
    assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
    assert_eq!(history, (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty()));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn opaque_pbr_product_gpu_and_cpu_share_color_and_material_semantics() {
    use solidcraft_engine::render::{
        StandardView,
        job::{CancelToken, ImageRequest, ImageTask, TaskPoll},
    };
    let mut s = Session::default();
    s.execute("solid.sphere", &json!({"radius":10,"body_name":"Original sphere"})).unwrap();
    const SIZE: u32 = 256;
    let mut previous = None;
    for (metallic, roughness) in [(0.0, 0.2), (0.0, 0.65), (1.0, 0.2), (1.0, 0.65)] {
        s.execute(
            "appearance.assign",
            &json!({"bodies":["Original sphere"],"color":"#b47846",
            "pbr":{"metallic":metallic,"roughness":roughness,"dielectric_ior":1.5}}),
        )
        .unwrap();
        let scene = solidcraft_engine::product_scene::extract(&s).unwrap();
        let (key, gpu) = gpu_scene(&scene).unwrap();
        assert!(!gpu.pbr.is_empty() && gpu.tris.is_empty());
        assert_ne!(previous, Some(key));
        previous = Some(key);
        let mut camera = Camera { fov: 0.0, ..Default::default() };
        camera.set_view(StandardView::Iso);
        camera.fit(&scene.bounds);
        let radius = scene.bounds.diagonal().max(10.0) + scene.bounds.center().dist(camera.target);
        let eye = camera.eye().to_f32();
        let mut task = ImageTask::new(
            scene,
            ImageRequest {
                integrator: Default::default(),
                raytrace: None,
                camera,
                width: SIZE,
                height: SIZE,
                background: [255; 3],
                transparent_background: false,
            },
            CancelToken::default(),
        )
        .unwrap();
        let mut png = None;
        for _ in 0..100_000 {
            if let TaskPoll::Complete(bytes) = task.step(4096).unwrap() {
                png = Some(bytes);
                break;
            }
        }
        let cpu = image::load_from_memory(&png.expect("bounded original fixture completes")).unwrap().to_rgba8();
        assert_eq!(cpu.dimensions(), (SIZE, SIZE));
        let cpu = cpu.into_raw();
        for srgb in [false, true] {
            let Some(gpu) = crate::gpu::render_product_offscreen(
                gpu.clone(),
                camera.view_proj(1.0, radius).to_f32(),
                camera.back().to_f32(),
                [eye[0], eye[1], eye[2], 0.0],
                SIZE,
                srgb,
            ) else {
                eprintln!("skipping: no GPU adapter");
                return;
            };
            let at = |pixels: &[u8], x: usize, y: usize| {
                let i = (y * SIZE as usize + x) * 4;
                [pixels[i], pixels[i + 1], pixels[i + 2]]
            };
            let mut core = 0usize;
            let mut error = 0usize;
            let mut outliers = 0usize;
            for y in 2..SIZE as usize - 2 {
                for x in 2..SIZE as usize - 2 {
                    // Exclude antialiased silhouettes: CPU uses 2x supersampling, GPU uses 1x.
                    if (y - 1..=y + 1).any(|ny| (x - 1..=x + 1).any(|nx| at(&cpu, nx, ny) == [255; 3] || at(&gpu, nx, ny) == [255; 3])) {
                        continue;
                    }
                    let delta = at(&cpu, x, y).into_iter().zip(at(&gpu, x, y)).map(|(a, b)| usize::from(a.abs_diff(b))).collect::<Vec<_>>();
                    error += delta.iter().sum::<usize>();
                    outliers += usize::from(delta.iter().any(|d| *d > 12));
                    core += 1;
                }
            }
            assert!(core > 1000);
            let mean = error as f64 / (3 * core) as f64;
            eprintln!("PBR m={metallic} r={roughness} srgb={srgb}: core={core}, mean={mean}, outliers={outliers}");
            assert!(mean < 2.0 && outliers * 100 < core * 2, "CPU/GPU color or material interpretation differs");
        }
    }
}

#[test]
fn control_render_export_respects_upstream_file_scope() {
    let mut h = Harness::new();
    let root = h.dir.join("allowed");
    let outside = h.dir.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let roots = json!([root.to_string_lossy()]);
    let response = h.call("ui.workspace", json!({"workspace":"render","allow_paths":roots}));
    assert_eq!(response["ok"], true, "{response}");
    let response = h.call("engine.execute", json!({"command":"render.start","params":{"settings":{"width":16,"height":16}},"allow_paths":roots}));
    assert_eq!(response["ok"], true, "{response}");
    let job = response["result"]["job"].clone();
    for _ in 0..50 {
        h.frames(1);
    }
    assert_eq!(h.app.session.execute("render.status", &json!({"job":job})).unwrap()["progress"]["phase"], "completed");
    for path in [outside.join("denied.png"), root.join("../outside/traversal.png")] {
        for overwrite in [false, true] {
            let response = h.call(
                "engine.execute",
                json!({"command":"render.export","params":{"job":job,"path":path,"overwrite":overwrite},"allow_paths":roots}),
            );
            assert_eq!(response["ok"], false, "{response}");
            assert!(response.to_string().contains("outside the folders this agent may use"), "{response}");
        }
    }
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    let path = root.join("allowed.png");
    let response = h.call("engine.execute", json!({"command":"render.export","params":{"job":job,"path":path},"allow_paths":roots}));
    assert_eq!(response["ok"], true, "{response}");
    assert!(path.is_file());
    let original = std::fs::read(&path).unwrap();
    let response = h.call("engine.execute", json!({"command":"render.export","params":{"job":job,"path":path,"overwrite":true},"allow_paths":roots}));
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn adopting_into_blank_document_resets_render_draft_and_preserves_hosts() {
    let mut h = Harness::new();
    h.app.render_workspace.host = Some(host_executor(2048, true, 4).unwrap());
    sync_backend(&mut h.app);
    h.app.render_workspace.view.name = "Old draft".into();
    enter(&mut h.app, true).unwrap();
    let mut incoming = Session::default();
    incoming.execute("solid.box", &json!({"length":10,"width":10,"height":10})).unwrap();
    crate::documents::adopt(&mut h.app, incoming);
    assert!(!active(&h.app));
    assert_eq!(h.app.render_workspace.view.name, "Studio");
    assert_eq!(h.app.session.render_backend().executors.len(), 3);
    assert!(h.app.session.render_job_host().is_some());
    assert_eq!(h.app.session.model.state().bodies.len(), 1);
}

#[test]
fn clicked_image_controls_complete_cancel_and_export_without_dirtying_the_design() {
    for language in ["en", "de", "ko", "ru"] {
        let mut h = Harness::new();
        h.app.preferences.language = language.into();
        h.frames(1);
        let check = h.app.tr("render.image_check").to_string();
        let start = h.app.tr("render.image_start").to_string();
        let export = h.app.tr("render.image_export").to_string();
        let cancel = h.app.tr("render.image_cancel").to_string();
        h.call("ui.resize", json!({"width":1600,"height":1400}));
        enter(&mut h.app, true).unwrap();
        h.app.render_workspace.view.draft.width = 16;
        h.app.render_workspace.view.draft.height = 16;
        h.app.render_workspace.view.dirty = true;
        h.frames(3);
        let doc = h.app.session.doc.clone();
        let undo = h.app.session.undo.len();
        h.step(&json!({"widget":check})).unwrap();
        h.step(&json!({"widget":start})).unwrap();
        for _ in 0..50 {
            h.frames(1);
            if h.app.render_workspace.view.completed.is_some() {
                break;
            }
        }
        let good = h.app.render_workspace.view.completed.unwrap();
        h.step(&json!({"widget":export})).unwrap();
        assert!(h.dir.join("render.png").is_file());
        h.app.render_workspace.view.draft.width = 1024;
        h.app.render_workspace.view.draft.height = 768;
        // Apply Check invalidation and layout changes before resolving Start's position.
        h.frames(1);
        h.step(&json!({"widget":start})).unwrap();
        h.step(&json!({"widget":cancel})).unwrap();
        h.frames(5);
        assert_eq!(h.app.render_workspace.view.completed, Some(good));
        assert_eq!(
            h.app.session.execute("render.status", &json!({"job":h.app.render_workspace.view.job})).unwrap()["progress"]["phase"],
            "cancelled"
        );
        assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
        assert_eq!(h.app.session.undo.len(), undo);
        assert!(!h.app.session.is_dirty());
    }
}

#[test]
fn saved_camera_survives_frames_and_product_fit_ignores_selection() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length":10,"width":20,"height":5,"body_name":"Product"})).unwrap();
    h.app.run("solid.box", json!({"corner":[100,0,0],"length":10,"width":20,"height":5,"body_name":"Other"})).unwrap();
    let camera = Camera { up: solidcraft_engine::render::UpAxis::Y, roll: 0.2, ..Default::default() };
    h.app.run("render.settings.set", json!({"name":"Studio","set":{"camera":camera,"background":[21,43,65]}})).unwrap();
    h.frames(3);
    let design = h.app.cam;
    enter(&mut h.app, true).unwrap();
    h.frames(3);
    h.step(&json!({"widget":"Restore Saved Camera"})).unwrap();
    h.frames(3);
    assert_eq!(h.app.cam, camera);
    assert_eq!(h.app.render_workspace.view.draft.background, [21, 43, 65]);
    h.app.session.selection = vec![solidcraft_engine::Sel::Body { name: "Product".into() }];
    let bounds = solidcraft_engine::product_scene::extract(&h.app.session).unwrap().bounds;
    let mut expected = camera;
    expected.fit(&bounds);
    h.app.animate_view("fit");
    h.frames(3);
    assert_eq!(h.app.cam, expected);
    enter(&mut h.app, false).unwrap();
    assert_eq!(h.app.cam, design);
}

#[test]
fn clicked_render_workspace_saves_one_setup_and_restores_design_view() {
    for language in ["en", "de", "ko", "ru"] {
        let mut h = Harness::new();
        h.app.preferences.language = language.into();
        h.app.run("solid.box", json!({"length":10,"width":20,"height":5})).unwrap();
        h.app.session.mark_saved();
        h.frames(3);
        let camera = h.app.cam;
        let flags = (h.app.ui.show_grid, h.app.ui.show_origin, h.app.ui.visual_style, h.app.ui.perspective);
        let undo = h.app.session.undo.len();
        let at = crate::scenario::rect_of("workspace_switcher").unwrap().center();
        h.step(&json!({"click":[at.x,at.y]})).unwrap();
        h.step(&json!({"click":{"handle":"menu:ui.workspace.render"}})).unwrap();
        assert!(active(&h.app));
        let heading = h.app.tr("cmd.render_settings_get").to_string();
        let perspective = h.app.tr("render.perspective").to_string();
        let save = h.app.tr("render.save_setup").to_string();
        let back = h.app.tr("render.return_design").to_string();
        assert!(h.widget(&heading).is_some(), "{language}");
        h.step(&json!({"widget":perspective})).unwrap();
        assert!(!h.app.session.is_dirty(), "preview edits are staged");
        h.step(&json!({"widget":save})).unwrap();
        assert_eq!(h.app.session.undo.len(), undo + 1);
        assert_eq!(h.app.session.doc.render.setups["Studio"].camera, h.app.cam);
        h.step(&json!({"widget":back})).unwrap();
        assert!(!active(&h.app));
        assert_eq!(h.app.cam, camera);
        assert_eq!(flags, (h.app.ui.show_grid, h.app.ui.show_origin, h.app.ui.visual_style, h.app.ui.perspective));
        h.app.run("edit.undo", json!({})).unwrap();
        assert!(!h.app.session.is_dirty());
    }
}

#[test]
fn render_drafts_and_capabilities_are_isolated_across_document_switches() {
    let mut h = Harness::new();
    h.app.render_workspace.host = Some(host_executor(2048, true, 4).unwrap());
    sync_backend(&mut h.app);
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
    h.app.render_workspace.view.name = "First draft".into();
    h.app.render_workspace.view.draft.background = [20, 40, 60];
    h.app.render_workspace.view.dirty = true;
    enter(&mut h.app, true).unwrap();
    crate::documents::open_tab(&mut h.app, Session::default()).unwrap();
    assert!(!active(&h.app));
    assert_eq!(h.app.render_workspace.view.name, "Studio");
    assert_eq!(h.app.session.render_backend().executors.len(), 3);
    crate::documents::switch(&mut h.app, 0);
    assert_eq!(h.app.render_workspace.view.name, "First draft");
    assert_eq!(h.app.render_workspace.view.draft.background, [20, 40, 60]);
    assert!(h.app.render_workspace.view.dirty);
    assert_eq!(h.app.session.render_backend().executors.len(), 3);
    h.app.run("file.new", json!({})).unwrap();
    assert_eq!(h.app.render_workspace.view.name, "Studio");
    assert_eq!(h.app.session.render_backend().executors.len(), 3);
}

#[test]
fn product_gpu_buffers_preserve_opacity_and_never_contain_overlays() {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length":10,"width":10,"height":10,"body_name":"Glass"})).unwrap();
    s.execute("appearance.assign", &json!({"bodies":["Glass"],"color":"#336699","opacity":0.4})).unwrap();
    let scene = solidcraft_engine::product_scene::extract(&s).unwrap();
    let (key, gpu) = gpu_scene(&scene).unwrap();
    assert!(gpu.tris.is_empty());
    assert!(!gpu.glass.is_empty());
    assert!(gpu.glass.as_chunks::<{ crate::gpu::TRI_SIZE }>().0.iter().all(|v| v[24..] == [51, 102, 153, 102]));
    assert!(gpu.lines.is_empty() && gpu.overlay.is_empty() && gpu.under.is_empty() && gpu.hidden.is_empty() && gpu.ghost.is_empty());
    s.selection = vec![solidcraft_engine::Sel::Body { name: "Glass".into() }];
    s.section = Some((solidcraft_engine::geom::Vec3::ZERO, solidcraft_engine::geom::Vec3::X));
    let after = solidcraft_engine::product_scene::extract(&s).unwrap();
    assert_eq!(gpu_scene(&after).unwrap().0, key);
    s.execute("appearance.assign", &json!({"bodies":["Glass"],"color":"#336699","opacity":0.0})).unwrap();
    let invisible = solidcraft_engine::product_scene::extract(&s).unwrap();
    assert_eq!(invisible.meshes[0].opacity, 0.0);
    let (_, empty) = gpu_scene(&invisible).unwrap();
    assert!(empty.tris.is_empty() && empty.glass.is_empty());
}

#[test]
fn host_profiles_advertise_only_installed_opaque_pbr_not_hdri_refraction_or_final() {
    use solidcraft_engine::render::*;
    let host = host_executor(512, true, 4).unwrap();
    assert_eq!(host.limits.max_dimension, 512);
    assert_eq!(host.profiles.len(), 1);
    assert_eq!(host.profiles[0].features, vec![RenderFeature::Pbr]);
    assert!(host.profiles[0].formats.is_empty());
    let mut s = Session::default();
    s.set_render_viewport(Some(host)).unwrap();
    let mut request = RenderRequest {
        backend: None,
        mode: RenderMode::Preview,
        operation: RenderOperation::Viewport,
        width: 512,
        height: 512,
        format: None,
        features: Vec::new(),
        policy: Default::default(),
    };
    assert_eq!(resolve_request(&[s.render_backend()], &request).unwrap().executor.as_str(), "gpu-viewport");
    request.features = vec![RenderFeature::Pbr];
    assert!(resolve_request(&[s.render_backend()], &request).is_ok());
    for unsupported in [RenderFeature::Hdri, RenderFeature::Refraction, RenderFeature::Progressive] {
        request.features = vec![unsupported];
        assert!(resolve_request(&[s.render_backend()], &request).is_err());
    }
    request.features.clear();
    request.width = 513;
    assert!(resolve_request(&[s.render_backend()], &request).is_err());
    request.width = 512;
    request.mode = RenderMode::Final;
    assert!(resolve_request(&[s.render_backend()], &request).is_err());
    assert!(host_executor(1024, false, 1).unwrap().availability != Availability::Available);
    assert!(host_executor(1024, true, 0).is_err());
}

fn completed_preview(h: &mut Harness, background: [u8; 3]) -> solidcraft_engine::render::job::JobId {
    let reply = h.app.session.execute("render.start", &json!({"settings":{"width":16,"height":16,"background":background}})).unwrap();
    let job = serde_json::from_value(reply["job"].clone()).unwrap();
    let host = h.app.session.render_job_host().unwrap().clone();
    for _ in 0..100 {
        host.pump(16384).unwrap();
        if h.app.session.execute("render.status", &json!({"job":job})).unwrap()["progress"]["phase"] == "completed" {
            return job;
        }
    }
    panic!("small cooperative image did not complete within the work budget");
}

fn async_export_fixture() -> (Harness, solidcraft_engine::render::job::JobId) {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length":10,"width":20,"height":5})).unwrap();
    h.app.session.mark_saved();
    let job = completed_preview(&mut h, [200, 210, 220]);
    (h, job)
}

#[test]
fn delayed_image_export_holds_its_document_without_changing_history() {
    use crate::picker_test_support::{Slot, save};
    let (mut h, job) = async_export_fixture();
    let path = h.dir.join("delayed.png");
    let doc = h.app.session.doc.clone();
    let undo = h.app.session.undo.len();
    let ctx = egui::Context::default();
    let slot: Slot = Default::default();
    save(&mut h.app, &slot);
    export_image(&mut h.app, job, false);
    crate::file_picker::tick(&mut h.app, &ctx);
    assert!(h.app.file_picker_busy() && !path.exists());
    assert_eq!(h.call("ui.inspect", json!({}))["result"]["file_dialog_open"], true);
    crate::documents::new_design(&mut h.app);
    let other = h.app.session.doc.clone();
    *slot.borrow_mut() = Some(Some(path.to_string_lossy().into_owned()));
    crate::file_picker::tick(&mut h.app, &ctx);
    assert_eq!(h.app.docs.active, 0, "upstream holds tab changes while a picker is pending");
    assert_eq!(h.call("ui.inspect", json!({}))["result"]["file_dialog_open"], false);
    assert!(Arc::ptr_eq(&other, &h.app.session.doc));
    assert_eq!(&std::fs::read(&path).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
    crate::documents::switch(&mut h.app, 0);
    assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
    assert_eq!(h.app.session.undo.len(), undo);
    assert!(!h.app.session.is_dirty());
}

#[test]
fn delayed_image_export_cancel_and_overwrite_choice_preserve_existing_files() {
    use crate::picker_test_support::{Slot, save};
    let (mut h, job) = async_export_fixture();
    let path = h.dir.join("existing.png");
    std::fs::write(&path, b"existing contents").unwrap();
    let doc = h.app.session.doc.clone();
    let ctx = egui::Context::default();
    let slot: Slot = Default::default();
    save(&mut h.app, &slot);
    export_image(&mut h.app, job, false);
    crate::file_picker::tick(&mut h.app, &ctx);
    *slot.borrow_mut() = Some(None);
    crate::file_picker::tick(&mut h.app, &ctx);
    assert!(!h.app.file_picker_busy());
    assert_eq!(std::fs::read(&path).unwrap(), b"existing contents");
    export_image(&mut h.app, job, false);
    crate::file_picker::tick(&mut h.app, &ctx);
    h.app.render_workspace.view.overwrite = true;
    *slot.borrow_mut() = Some(Some(path.to_string_lossy().into_owned()));
    crate::file_picker::tick(&mut h.app, &ctx);
    assert_eq!(std::fs::read(&path).unwrap(), b"existing contents", "a later checkbox change is not overwrite consent for the pending export");
    assert!(matches!(&h.app.status, Some((_, _, true))));
    assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
    assert_eq!(h.app.session.execute("render.status", &json!({"job":job})).unwrap()["progress"]["phase"], "completed");
}

#[test]
fn delayed_image_export_never_substitutes_a_newer_retained_image() {
    use crate::picker_test_support::{Slot, save};
    let (mut h, job) = async_export_fixture();
    let path = h.dir.join("old-image.png");
    let ctx = egui::Context::default();
    let slot: Slot = Default::default();
    save(&mut h.app, &slot);
    export_image(&mut h.app, job, false);
    crate::file_picker::tick(&mut h.app, &ctx);
    let newer = completed_preview(&mut h, [20, 30, 40]);
    h.app.render_workspace.view.completed = Some(newer);
    *slot.borrow_mut() = Some(Some(path.to_string_lossy().into_owned()));
    crate::file_picker::tick(&mut h.app, &ctx);
    assert!(!path.exists(), "the evicted requested image must fail instead of exporting the newer image");
    assert!(matches!(&h.app.status, Some((_, _, true))));
    assert_eq!(h.app.render_workspace.view.completed, Some(newer));
    let newer_path = h.dir.join("new-image.png");
    h.app.run("render.export", json!({"job":newer,"path":newer_path})).unwrap();
    assert!(newer_path.is_file());
}

#[test]
fn delayed_image_export_holds_close_and_new_document_until_cancel() {
    use crate::picker_test_support::{Slot, save};
    let (mut h, job) = async_export_fixture();
    let path = h.dir.join("closed.png");
    let ctx = egui::Context::default();
    let slot: Slot = Default::default();
    save(&mut h.app, &slot);
    export_image(&mut h.app, job, false);
    crate::file_picker::tick(&mut h.app, &ctx);
    crate::documents::new_design(&mut h.app);
    crate::documents::close(&mut h.app, 0, true);
    let other = h.app.session.doc.clone();
    assert_eq!(h.app.docs.count(), 1);
    assert!(h.app.file_picker_busy());
    *slot.borrow_mut() = Some(None);
    crate::file_picker::tick(&mut h.app, &ctx);
    assert!(!path.exists() && !h.app.file_picker_busy());
    assert!(Arc::ptr_eq(&other, &h.app.session.doc));
}

#[test]
fn delayed_image_export_refuses_an_internally_replaced_document_even_with_overwrite() {
    use crate::picker_test_support::{Slot, save};
    for overwrite in [false, true] {
        let (mut h, job) = async_export_fixture();
        let path = h.dir.join("keep.png");
        std::fs::write(&path, b"keep this file").unwrap();
        let slot: Slot = Default::default();
        save(&mut h.app, &slot);
        export_image(&mut h.app, job, overwrite);
        assert!(h.app.run("appearance.assign", json!({"bodies":["Body1"],"color":[1,2,3]})).is_err());
        // Internal host/session changes are outside the input hold; never export them by accident.
        h.app.session.execute("appearance.assign", &json!({"bodies":["Body1"],"color":[1,2,3]})).unwrap();
        let changed = h.app.session.doc.clone();
        let history = (h.app.session.undo.len(), h.app.session.revision);
        *slot.borrow_mut() = Some(Some(path.to_string_lossy().into_owned()));
        crate::file_picker::tick(&mut h.app, &egui::Context::default());
        assert_eq!(std::fs::read(path).unwrap(), b"keep this file");
        assert!(Arc::ptr_eq(&changed, &h.app.session.doc));
        assert_eq!((h.app.session.undo.len(), h.app.session.revision), history);
        assert!(matches!(&h.app.status, Some((_, _, true))));
    }
}

/// One frame of either the modelling viewport or the Render preview, over the same rectangle.
fn nav_frame(app: &mut crate::SolveApp, ctx: &egui::Context, render: bool, time: &mut f64, mods: egui::Modifiers, events: Vec<egui::Event>) {
    *time += 1.0 / 60.0;
    let events = std::iter::once(egui::Event::ModifiersChanged(mods)).chain(events).collect();
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
        time: Some(*time),
        focused: true,
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| {
        if render {
            show(app, ui);
        } else {
            crate::viewport::show(app, ui);
        }
    });
    out.textures_delta.clear();
}

fn nav_app(render: bool, preset: &str, around: crate::prefs::OrbitAround) -> crate::SolveApp {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length":40,"width":30,"height":20,"body_name":"Product"})).unwrap();
    let mut app = crate::SolveApp::new(s, crate::Services::default());
    if render {
        enter(&mut app, true).unwrap();
        assert!(active(&app));
    }
    app.preferences.nav = preset.into();
    app.preferences.orbit_around = around;
    app.preferences.zoom_to_cursor = true;
    app.ui.perspective = true;
    let mut cam = Camera::default();
    cam.set_view(solidcraft_engine::render::StandardView::Iso);
    cam.fit(&solidcraft_engine::product_scene::extract(&app.session).unwrap().bounds);
    app.cam = cam;
    app
}

/// #1454: the same mouse, wheel and modifier input moves the Render preview's camera exactly as
/// it moves the modelling viewport's, under every navigation preset and Orbit around setting.
#[test]
fn render_preview_navigates_like_the_modelling_viewport_in_every_preset() {
    use egui::{Event, Modifiers, PointerButton, pos2, vec2};
    let none = Modifiers::NONE;
    let chords = [none, Modifiers::SHIFT, Modifiers::CTRL, Modifiers::ALT, Modifiers::SHIFT | Modifiers::ALT];
    let arounds = [crate::prefs::OrbitAround::ViewCentre, crate::prefs::OrbitAround::Cursor, crate::prefs::OrbitAround::Origin];
    for preset in crate::prefs::NAV_PRESETS.iter().map(|p| p.id) {
        for around in arounds {
            let (design_ctx, render_ctx) = (egui::Context::default(), egui::Context::default());
            let mut design = nav_app(false, preset, around);
            let mut render = nav_app(true, preset, around);
            let (mut design_time, mut render_time) = (0.0, 0.0);
            let mut both = |events: Vec<Event>, mods: Modifiers, design: &mut crate::SolveApp, render: &mut crate::SolveApp| {
                nav_frame(design, &design_ctx, false, &mut design_time, mods, events.clone());
                nav_frame(render, &render_ctx, true, &mut render_time, mods, events);
            };
            both(vec![], none, &mut design, &mut render);
            assert_eq!(design.viewport.rect, render.viewport.rect);
            // Presses start on empty space (the Render preview has nothing to pick), and the wheel
            // turns over the model, so zoom to the cursor anchors on its surface.
            let empty = pos2(90.0, 520.0);
            let proj = crate::viewport::projection(&design, design.viewport.rect.unwrap());
            assert!(crate::viewport::pick(&design, &proj, empty).is_empty());
            let model = proj.to_screen(design.cam.target).unwrap();
            assert!(!crate::viewport::pick(&design, &proj, model).is_empty());
            let mut moved = 0;
            for button in [PointerButton::Primary, PointerButton::Middle, PointerButton::Secondary] {
                for mods in chords {
                    let before = design.cam;
                    both(vec![Event::PointerMoved(empty)], mods, &mut design, &mut render);
                    both(vec![Event::PointerButton { pos: empty, button, pressed: true, modifiers: mods }], mods, &mut design, &mut render);
                    let mut at = empty;
                    for _ in 0..5 {
                        at += vec2(14.0, -9.0);
                        both(vec![Event::PointerMoved(at)], mods, &mut design, &mut render);
                    }
                    both(vec![Event::PointerButton { pos: at, button, pressed: false, modifiers: mods }], mods, &mut design, &mut render);
                    for _ in 0..30 {
                        both(vec![], none, &mut design, &mut render);
                    }
                    assert_eq!(design.cam, render.cam, "{preset} {around:?}: {button:?} drag with {mods:?}");
                    moved += usize::from(design.cam != before);
                    // Let any snap settle and start the next gesture from the same view.
                    design.cam = render.cam;
                }
            }
            assert!(moved >= 2, "{preset}: the preset's drags move the camera ({moved})");
            // Wheel over the model, then modifier-only motion (Touchpad: Shift pans, Alt orbits).
            let before = design.cam;
            both(vec![Event::PointerMoved(model)], none, &mut design, &mut render);
            let wheel = Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, 3.0), modifiers: none, phase: egui::TouchPhase::Move };
            both(vec![wheel], none, &mut design, &mut render);
            for _ in 0..30 {
                both(vec![], none, &mut design, &mut render);
            }
            assert_ne!(design.cam, before, "{preset}: the wheel zooms");
            assert_eq!(design.cam, render.cam, "{preset} {around:?}: wheel zoom to the cursor");
            for mods in [Modifiers::SHIFT, Modifiers::ALT] {
                both(vec![Event::PointerMoved(empty)], mods, &mut design, &mut render);
                both(vec![Event::PointerMoved(empty + vec2(25.0, 15.0))], mods, &mut design, &mut render);
                both(vec![Event::PointerMoved(empty)], none, &mut design, &mut render);
                assert_eq!(design.cam, render.cam, "{preset} {around:?}: modifier motion with {mods:?}");
            }
            assert!(render.session.selection.is_empty(), "the Render preview never selects");
        }
    }
}

/// #1454: Fit (F6), a middle double-click, and the axis view's and navigation cube's Home and
/// view controls all work in the Render workspace, framing the product scene.
#[test]
fn render_workspace_view_keys_double_middle_click_and_navigators_work() {
    for style in [crate::prefs::NavigatorStyle::Axes, crate::prefs::NavigatorStyle::Cube] {
        let mut h = Harness::new();
        h.app.preferences.navigator_style = style;
        h.app.preferences.look_at_anim = false;
        h.app.run("solid.box", json!({"length":40,"width":30,"height":20,"body_name":"Product"})).unwrap();
        enter(&mut h.app, true).unwrap();
        h.frames(3);
        let bounds = solidcraft_engine::product_scene::extract(&h.app.session).unwrap().bounds;
        let mut home = h.app.cam;
        home.set_view(solidcraft_engine::render::StandardView::Iso);
        home.fit(&bounds);
        let off = |h: &mut Harness| {
            h.app.cam.yaw += 0.7;
            h.app.cam.pitch = 0.1;
            h.app.cam.distance *= 3.0;
            h.app.cam.target.x += 25.0;
            h.frames(2);
        };
        // F6 fits, keeping the direction.
        off(&mut h);
        let mut fitted = h.app.cam;
        fitted.fit(&bounds);
        h.step(&json!({"key":"F6"})).unwrap();
        h.settle();
        assert_eq!(h.app.cam, fitted, "{style:?}: F6");
        // A middle double-click goes Home.
        off(&mut h);
        let centre = h.app.viewport.rect.unwrap().center();
        h.step(&json!({"click":[centre.x, centre.y], "double":true, "button":"middle"})).unwrap();
        h.settle();
        assert_eq!(h.app.cam, home, "{style:?}: middle double-click");
        // The navigator's Home control.
        off(&mut h);
        let handle = match style {
            crate::prefs::NavigatorStyle::Axes => "axis_view:home",
            crate::prefs::NavigatorStyle::Cube => "navigation_cube:control:home",
        };
        h.step(&json!({"click":{"handle":handle}})).unwrap();
        h.settle();
        assert!(
            (h.app.cam.yaw - home.yaw).abs() < 1e-6 && (h.app.cam.pitch - home.pitch).abs() < 1e-6,
            "{style:?}: navigator Home turns to the home view"
        );
        // A standard view from the navigator.
        h.step(&json!({"click":{"handle":"axis_view:z"}})).unwrap();
        h.settle();
        assert!(h.app.cam.back().dot(solidcraft_engine::geom::Vec3::Z) > 0.999, "{style:?}: view from +Z, got {:?}", h.app.cam.back());
        assert!(h.app.session.selection.is_empty());
    }
}

/// #1464: a maximised 4K or 5K window is past the preview's pixel budget. The preview draws
/// smaller at the same aspect instead of refusing every frame.
#[test]
fn four_and_five_k_viewports_preview_downscaled_within_the_limits() {
    use solidcraft_engine::render::*;
    for samples in [1, 4, 16] {
        let host = host_executor(16384, true, samples).unwrap();
        let mut s = Session::default();
        s.set_render_viewport(Some(host.clone())).unwrap();
        let request = |width, height| RenderRequest {
            backend: None,
            mode: RenderMode::Preview,
            operation: RenderOperation::Viewport,
            width,
            height,
            format: None,
            features: vec![RenderFeature::Pbr],
            policy: Default::default(),
        };
        if samples <= 4 {
            assert_eq!(preview_size(s.render_viewport(), (1920, 1080)), (1920, 1080), "fits: drawn at full size");
        }
        // 16× MSAA's scratch cost brings even 1080p under the budget.
        let sizes: &[(u64, u64)] = if samples == 16 { &[(1920, 1080), (3840, 2160), (5120, 2880)] } else { &[(3840, 2160), (5120, 2880)] };
        for &full in sizes {
            assert!(resolve_request(&[s.render_backend()], &request(full.0, full.1)).is_err(), "{full:?} at full size is over the limit");
            let (w, h) = preview_size(s.render_viewport(), full);
            assert!(w < full.0 && h < full.1 && w * h <= host.limits.max_pixels, "{full:?} -> {w}x{h}");
            assert!((w as f64 / h as f64 - 16.0 / 9.0).abs() < 0.01, "{w}x{h} keeps the aspect");
            let resolved = resolve_request(&[s.render_backend()], &request(w, h)).unwrap();
            assert_eq!(resolved.executor.as_str(), "gpu-viewport");
            assert!(resolved.estimated_scratch_bytes <= host.limits.max_scratch_bytes);
        }
    }
    assert_eq!(preview_size(None, (5120, 2880)), (5120, 2880), "no executor: resolution reports it");
}

// CPU/AA regressions for runtime choices, request admission and captured jobs.
#[test]
fn selecting_raster_discards_cpu_aa_and_check_without_changing_the_document() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length":10,"width":20,"height":5})).unwrap();
    let physical = h.app.session.execute("component.properties", &json!({})).unwrap();
    let doc = h.app.session.doc.clone();
    let history = (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty());
    let initial = image_params(&h.app).unwrap();
    remember_image_check(&mut h.app, initial);
    h.app.render_workspace.view.integrator = Integrator::Raytrace;
    h.app.render_workspace.view.raytrace.primary_samples = 16;
    h.app.render_workspace.view.raytrace.exposure_ev = 3.0;
    h.app.render_workspace.view.raytrace.shadows = false;
    h.app.render_workspace.view.transparent = true;
    select_image_method(&mut h.app, Integrator::Raster).unwrap();
    assert_eq!(h.app.render_workspace.view.raytrace, RaytraceOptions::default());
    assert_eq!(h.app.render_workspace.view.raytrace.primary_samples, 1);
    assert!(h.app.render_workspace.view.checked_image.is_none());
    let raster = image_params(&h.app).unwrap();
    assert!(raster.get("raytrace").is_none() && raster.get("integrator").is_none());
    assert_eq!(raster["transparent_background"], true);
    assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
    assert_eq!(history, (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty()));
    assert_eq!(h.app.session.execute("component.properties", &json!({})).unwrap(), physical);
}

#[test]
fn unavailable_cpu_selection_does_not_change_method_or_create_a_host() {
    let mut h = Harness::new();
    let host = CpuHost::cooperative();
    h.app.session.set_render_job_host(Some(host.clone())).unwrap();
    let doc = h.app.session.doc.clone();
    assert!(select_image_method(&mut h.app, Integrator::Raytrace).is_err());
    assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raster);
    assert_eq!(h.app.render_workspace.view.raytrace.primary_samples, 1);
    assert!(h.app.session.render_job_host().unwrap().same_host(&host));
    assert_eq!(host.admitted_bytes(), 0);
    h.app.session.set_render_job_host(None).unwrap();
    assert!(select_image_method(&mut h.app, Integrator::Raytrace).is_err());
    assert!(h.app.session.render_job_host().is_none());
    assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn cpu_aa_invalid_counts_refuse_and_changes_invalidate_check_without_reserving() {
    let mut h = Harness::new();
    let native = CpuHost::for_platform().unwrap();
    h.app.session.set_render_job_host(Some(native.clone())).unwrap();
    h.app.render_workspace.image_host = Some(native);
    let host = h.app.session.render_job_host().unwrap().clone();
    if select_image_method(&mut h.app, Integrator::Raytrace).is_err() {
        assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raster);
        assert_eq!(host.admitted_bytes(), 0);
        return;
    }
    let doc = h.app.session.doc.clone();
    let model = h.app.session.model.state();
    let history = (h.app.session.revision, h.app.session.redo.len(), h.app.session.undo.len(), h.app.session.is_dirty());
    for samples in [0, 2, 8, 17, 255] {
        h.app.render_workspace.view.raytrace.primary_samples = samples;
        assert!(image_params(&h.app).is_err());
    }
    for samples in [1, 4, 16] {
        h.app.render_workspace.view.raytrace.primary_samples = samples;
        let params = image_params(&h.app).unwrap();
        let options: RaytraceOptions = serde_json::from_value(params["raytrace"].clone()).unwrap();
        assert_eq!(options.primary_samples, samples);
        h.app.session.execute("render.preflight", &params).unwrap();
        remember_image_check(&mut h.app, params);
        h.app.render_workspace.view.raytrace.primary_samples = if samples == 16 { 1 } else { 16 };
        let changed = image_params(&h.app).unwrap();
        invalidate_image_check(&mut h.app, Some(&changed));
        assert!(h.app.render_workspace.view.checked_image.is_none());
        assert_eq!(host.admitted_bytes(), 0);
    }
    assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
    assert_eq!(history, (h.app.session.revision, h.app.session.redo.len(), h.app.session.undo.len(), h.app.session.is_dirty()));
    assert!(Arc::ptr_eq(&model, &h.app.session.model.state()));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn clicked_cpu_aa_check_and_start_forward_four_and_sixteen_to_native_work() {
    let mut h = Harness::new();
    // Harness defaults to a cooperative host even in CPU-enabled test builds.
    // Exercise the real native worker, with the same owner on both UI paths.
    let native = CpuHost::for_platform().unwrap();
    assert_eq!(native.kind(), solidcraft_engine::render::job::HostKind::NativeWorker);
    assert!(native.is_available());
    let cpu_compiled = native.descriptor_for(Integrator::Raytrace).is_ok();
    h.app.session.set_render_job_host(Some(native.clone())).unwrap();
    h.app.render_workspace.image_host = Some(native);
    assert!(h.app.session.render_job_host().unwrap().same_host(h.app.render_workspace.image_host.as_ref().unwrap()));
    h.call("ui.resize", json!({"width":1600,"height":1400}));
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
    enter(&mut h.app, true).unwrap();
    h.app.render_workspace.host = None;
    h.app.session.set_render_viewport(None).unwrap();
    h.app.render_workspace.view.draft.width = 16;
    h.app.render_workspace.view.draft.height = 16;
    h.app.render_workspace.view.draft.background = [255, 255, 255];
    // Orthographic half-height8 frames the10mm box with visible background.
    h.app.cam.set_view(solidcraft_engine::render::StandardView::Top);
    h.app.cam.target = solidcraft_engine::geom::Vec3::new(5.0, 5.0, 5.0);
    h.app.cam.distance = 20.0;
    h.app.cam.fov = 0.0;
    h.app.render_workspace.view.dirty = true;
    h.frames(3);
    let physical = h.app.session.execute("component.properties", &json!({})).unwrap();
    let doc = h.app.session.doc.clone();
    let revision = h.app.session.revision;
    let history = (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty());
    // The dependency's private cfg(test) evaluation counter is unavailable here.
    // A refresh increments the public revision; also retain the actual model Arc.
    let model = h.app.session.model.state();
    let assert_unchanged = |h: &mut Harness| {
        assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
        assert_eq!(h.app.session.revision, revision);
        assert!(Arc::ptr_eq(&model, &h.app.session.model.state()));
        assert_eq!(history, (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty()));
        assert_eq!(h.app.session.execute("component.properties", &json!({})).unwrap(), physical);
    };
    let available = image_host(&h.app, Integrator::Raytrace).is_ok();
    h.step(&json!({"click":{"handle":"render:method"}})).unwrap();
    h.step(&json!({"click":{"handle":"render:method-cpu"}})).unwrap();
    assert_unchanged(&mut h);
    if !available {
        assert!(!cpu_compiled, "compiled native CPU unexpectedly unavailable");
        assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raster);
        assert_eq!(h.app.session.render_job_host().unwrap().admitted_bytes(), 0);
        return;
    }
    assert!(cpu_compiled);
    assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raytrace);
    assert_eq!(h.app.render_workspace.view.raytrace.primary_samples, 1);
    for samples in [4, 16] {
        h.step(&json!({"click":{"handle":"render:aa"}})).unwrap();
        h.step(&json!({"click":{"handle":format!("render:aa-{samples}")}})).unwrap();
        assert_eq!(h.app.render_workspace.view.raytrace.primary_samples, samples);
        assert_unchanged(&mut h);
        let check = h.app.tr("render.image_check").to_string();
        h.step(&json!({"widget":check})).unwrap();
        let params = h.app.render_workspace.view.checked_image.as_ref().unwrap().params.clone();
        assert_eq!(image_params(&h.app).unwrap(), params);
        assert_unchanged(&mut h);
        let start = h.app.tr("render.image_start").to_string();
        h.step(&json!({"widget":start})).unwrap();
        let id = h.app.render_workspace.view.job.unwrap();
        let handle = h.app.session.render_job_handle(id).unwrap();
        let status = handle.wait_timeout(std::time::Duration::from_secs(60)).unwrap();
        assert_eq!(status.progress.phase, solidcraft_engine::render::job::Phase::Completed);
        assert_eq!(status.scene_generation, revision);
        assert_eq!(status.job, id);
        assert_eq!(status.request.integrator, Integrator::Raytrace);
        assert_eq!(serde_json::to_value(status.request.camera).unwrap(), params["settings"]["camera"]);
        assert_eq!(json!(status.request.width), params["settings"]["width"]);
        assert_eq!(json!(status.request.height), params["settings"]["height"]);
        assert_eq!(json!(status.request.background), params["settings"]["background"]);
        assert_eq!(json!(status.request.transparent_background), params["transparent_background"]);
        assert_eq!(status.request.raytrace.as_ref().unwrap().primary_samples, samples);
        assert_eq!(serde_json::to_value(status.request.raytrace).unwrap(), params["raytrace"]);
        assert!(handle.frame().unwrap().unwrap().png().starts_with(b"\x89PNG\r\n\x1a\n"));
        h.frames(3);
        assert_eq!(h.app.render_workspace.view.job, Some(id));
        assert_eq!(h.app.render_workspace.view.completed, Some(id));
        let frame = handle.frame().unwrap().unwrap();
        assert_eq!(frame.info.job, id);
        assert_eq!(frame.info.scene_generation, revision);
        assert_eq!(frame.info.executor.as_str(), "cpu-raytrace");
        assert_eq!((frame.info.width, frame.info.height), (16, 16));
        let bytes = frame.png().to_vec();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (16, 16));
        assert!(decoded.pixels().any(|p| p.0 == [255, 255, 255, 255]), "fixture must retain uncovered background");
        assert!(decoded.pixels().any(|p| p.0 != [255, 255, 255, 255] && p.0[3] == 255), "actual box must contribute opaque pixels");
        assert_unchanged(&mut h);
        let output = h.dir.join("render.png");
        assert!(!output.exists(), "export witness must not reuse an old file");
        let export = h.app.tr("render.image_export").to_string();
        h.step(&json!({"widget":export})).unwrap();
        h.frames(3);
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
        assert_unchanged(&mut h);
        std::fs::remove_file(output).unwrap();
        eprintln!("UI_CPU_AA_POSITIVE completed primary_samples={samples} executor=cpu-raytrace frame=16x16 export=byte-identical physics=unchanged");
    }
    h.step(&json!({"click":{"handle":"render:method"}})).unwrap();
    h.step(&json!({"click":{"handle":"render:method-raster"}})).unwrap();
    assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raster);
    assert_eq!(h.app.render_workspace.view.raytrace, RaytraceOptions::default());
    assert!(h.app.render_workspace.view.checked_image.is_none());
    assert!(image_params(&h.app).unwrap().get("raytrace").is_none());
    assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
    assert_eq!(history, (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty()));
    assert_unchanged(&mut h);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn cpu_aa_check_start_cancel_keeps_captured_request_and_refuses_stale_recheck() {
    let mut h = Harness::new();
    let native = CpuHost::for_platform().unwrap();
    h.app.session.set_render_job_host(Some(native.clone())).unwrap();
    h.app.render_workspace.image_host = Some(native);
    h.app.run("solid.box", json!({"length":10,"width":20,"height":5})).unwrap();
    h.app.run("render.scene", json!({})).unwrap();
    if select_image_method(&mut h.app, Integrator::Raytrace).is_err() {
        assert_eq!(h.app.render_workspace.view.integrator, Integrator::Raster);
        return;
    }
    h.app.render_workspace.view.raytrace.primary_samples = 16;
    h.app.render_workspace.view.draft.width = 512;
    h.app.render_workspace.view.draft.height = 512;
    let origin = h.app.session.doc.clone();
    let history = (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty());
    let params = image_params(&h.app).unwrap();
    h.app.run("render.preflight", params.clone()).unwrap();
    remember_image_check(&mut h.app, params.clone());
    let revision = h.app.session.revision;
    let current = image_params(&h.app).unwrap();
    let reply = h.app.run("render.start", current).unwrap();
    let id = serde_json::from_value(reply["job"].clone()).unwrap();
    let handle = h.app.session.render_job_handle(id).unwrap();
    select_image_method(&mut h.app, Integrator::Raster).unwrap();
    h.app.run("render.cancel", json!({"job":id})).unwrap();
    let status = handle.wait_timeout(std::time::Duration::from_secs(60)).unwrap();
    assert!(status.progress.phase.terminal());
    assert_eq!(status.scene_generation, revision);
    assert_eq!(status.request.raytrace.as_ref().unwrap().primary_samples, 16);
    if status.progress.phase == solidcraft_engine::render::job::Phase::Cancelled {
        assert!(handle.frame().unwrap().is_none());
    }
    assert!(Arc::ptr_eq(&origin, &h.app.session.doc));
    assert_eq!(history, (h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.is_dirty()));
    for (command, params) in [
        ("sketch.create", json!({"plane":"XY"})),
        ("sketch.rectangle.two_point", json!({"p0":[5,5],"p1":[25,20]})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance":5})),
        ("sketch.edit", json!({"sketch":"Sketch1"})),
        ("sketch.drag", json!({"entity":"l1.start","from":[5,5],"to":[4,5],"live":true})),
    ] {
        h.app.run(command, params).unwrap();
    }
    assert!(h.app.session.model.is_stale());
    let doc = h.app.session.doc.clone();
    let stale_revision = h.app.session.revision;
    let model = h.app.session.model.state();
    assert!(image_params(&h.app).is_err());
    invalidate_image_check(&mut h.app, None);
    assert!(h.app.render_workspace.view.checked_image.is_none());
    assert!(Arc::ptr_eq(&doc, &h.app.session.doc));
    assert_eq!(h.app.session.revision, stale_revision);
    assert!(Arc::ptr_eq(&model, &h.app.session.model.state()));
}
