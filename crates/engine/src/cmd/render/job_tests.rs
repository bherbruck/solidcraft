use super::*;
use solidcraft_render::job::{CpuHost, JobId};

fn session() -> (Session, CpuHost) {
    let host = CpuHost::cooperative();
    let mut s = Session::default();
    s.set_render_job_host(Some(host.clone())).unwrap();
    (s, host)
}

fn request() -> Value {
    json!({"settings":{"width":16,"height":16}})
}
fn start(s: &mut Session) -> JobId {
    serde_json::from_value(s.execute("render.start", &request()).unwrap()["job"].clone()).unwrap()
}
fn finish(host: &CpuHost) {
    for _ in 0..1000 {
        if !host.pump(4096).unwrap() {
            return;
        }
    }
    panic!("cooperative job did not finish");
}

#[test]
fn missing_image_host_is_rejected_before_stale_scene_capture() {
    let mut s = Session::default();
    for (command, params) in [
        ("sketch.create", json!({"plane":"XY"})),
        ("sketch.rectangle.two_point", json!({"p0":[5,5],"p1":[25,20]})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance":5})),
        ("sketch.edit", json!({"sketch":"Sketch1"})),
        ("sketch.drag", json!({"entity":"l1.start","from":[5,5],"to":[4,5],"live":true})),
    ] {
        s.execute(command, &params).unwrap();
    }
    assert!(s.model.is_stale());
    let doc = s.doc.clone();
    let before = (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len());
    for command in ["render.preflight", "render.start"] {
        let error = s.execute(command, &request()).unwrap_err().to_string();
        assert!(error.contains("no image host is installed"), "{error}");
        assert!(s.model.is_stale());
        assert!(Arc::ptr_eq(&doc, &s.doc));
        assert_eq!(before, (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len()));
    }
}

#[test]
fn runtime_commands_preserve_document_history_and_evaluation() {
    let (mut s, host) = session();
    let doc = s.doc.clone();
    let before = (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len());
    s.execute("render.preflight", &request()).unwrap();
    assert_eq!(host.admitted_bytes(), 0);
    let id = start(&mut s);
    assert_eq!(s.execute("render.status", &json!({"job":id})).unwrap()["progress"]["phase"], "queued");
    finish(&host);
    let status = s.execute("render.status", &json!({"job":id})).unwrap();
    assert_eq!(status["progress"]["phase"], "completed");
    assert_eq!(status["frame"]["job"], json!(id));
    assert!(Arc::ptr_eq(&doc, &s.doc));
    assert_eq!(before, (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len()));
    assert_eq!(s.execute("render.cancel", &json!({"job":id})).unwrap()["progress"]["phase"], "completed");
}

#[test]
fn omitted_integrator_remains_raster_and_options_do_not_change_viewport_quality() {
    let (mut s, host) = session();
    let implicit = s.execute("render.preflight", &request()).unwrap();
    let explicit = s.execute("render.preflight", &json!({"integrator":"raster","settings":{"width":16,"height":16}})).unwrap();
    assert_eq!(implicit, explicit);
    assert_eq!(implicit["request"]["integrator"], "raster");
    assert_eq!(implicit["resolved"]["executor"], "cpu-product");
    assert!(s.execute("render.preflight", &json!({"raytrace":{"shadows":true}})).is_err());
    assert!(s.execute("render.preflight", &json!({"integrator":"quality"})).is_err());
    assert_eq!(host.admitted_bytes(), 0);
}

#[cfg(not(feature = "raytrace-cpu"))]
#[test]
fn compiled_out_raytrace_is_rejected_before_scene_capture() {
    let (mut s, host) = session();
    for (command, params) in [
        ("sketch.create", json!({"plane":"XY"})),
        ("sketch.rectangle.two_point", json!({"p0":[5,5],"p1":[25,20]})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance":5})),
        ("sketch.edit", json!({"sketch":"Sketch1"})),
        ("sketch.drag", json!({"entity":"l1.start","from":[5,5],"to":[4,5],"live":true})),
    ] {
        s.execute(command, &params).unwrap();
    }
    assert!(s.model.is_stale());
    let doc = s.doc.clone();
    let before = (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len());
    for command in ["render.preflight", "render.start"] {
        let error = s.execute(command, &json!({"integrator":"raytrace","replace":true})).unwrap_err().to_string();
        assert!(error.contains("not compiled in"), "{error}");
        assert!(s.model.is_stale());
        assert!(Arc::ptr_eq(&doc, &s.doc));
        assert_eq!(before, (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len()));
        assert_eq!(host.admitted_bytes(), 0);
    }
}

#[test]
fn identities_are_isolated_across_documents_scratch_and_new() {
    let (mut a, host) = session();
    let mut b = Session::default();
    b.set_render_job_host(Some(host.clone())).unwrap();
    let a_id = start(&mut a);
    let b_id = start(&mut b);
    assert_eq!(a_id.serial, b_id.serial);
    assert_ne!(a_id.session, b_id.session);
    assert!(b.execute("render.cancel", &json!({"job":a_id})).is_err());
    assert!(a.scratch().execute("render.status", &json!({"job":a_id})).is_err());
    a.execute("file.new", &json!({})).unwrap();
    assert!(a.render_job_host().unwrap().same_host(&host));
    assert!(a.execute("render.status", &json!({"job":a_id})).is_err());
    finish(&host);
    assert_eq!(b.execute("render.status", &json!({"job":b_id})).unwrap()["progress"]["phase"], "completed");
}

#[test]
fn queue_cancellation_and_replacement_keep_the_last_successful_image() {
    let (mut s, host) = session();
    let good = start(&mut s);
    finish(&host);
    s.execute("render.status", &json!({"job":good})).unwrap();
    let a = start(&mut s);
    host.pump(1).unwrap();
    start(&mut s);
    assert!(s.execute("render.start", &request()).is_err());
    s.execute("render.cancel", &json!({"job":a})).unwrap();
    let replacement = s.execute("render.start", &json!({"settings":{"width":16,"height":16},"replace":true})).unwrap();
    s.execute("render.cancel", &json!({"job":replacement["job"]})).unwrap();
    finish(&host);
    assert!(s.render_jobs.frame(good).is_ok());
    assert_eq!(s.execute("render.status", &json!({"job":a})).unwrap()["progress"]["phase"], "cancelled");
}

#[test]
fn unsupported_intent_is_rejected_and_export_preserves_existing_files() {
    let (mut s, host) = session();
    for params in [
        json!({"settings":{"mode":"final"}}),
        json!({"settings":{"format":"exr"}}),
        json!({"settings":{"backend":"missing"}}),
        json!({"settings":{"exposure_ev":1}}),
        json!({"unknown":true}),
    ] {
        assert!(s.execute("render.preflight", &params).is_err());
        assert_eq!(host.admitted_bytes(), 0);
    }
    let id = start(&mut s);
    let path = std::env::temp_dir().join(format!("solidcraft-job-{}-{}.png", std::process::id(), id.session));
    assert!(s.execute("render.export", &json!({"job":id,"path":path})).is_err());
    finish(&host);
    let missing = path.with_file_name("solidcraft-render-missing-directory").join("image.png");
    assert!(s.execute("render.export", &json!({"job":id,"path":missing})).is_err());
    assert!(s.render_jobs.frame(id).is_ok());
    s.execute("render.export", &json!({"job":id,"path":path})).unwrap();
    let png = std::fs::read(&path).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(s.execute("render.export", &json!({"job":id,"path":path})).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), png);
    s.execute("render.export", &json!({"job":id,"path":path,"overwrite":true})).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), png);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn open_close_and_host_replacement_cancel_work_and_reject_old_ids() {
    let (mut s, host) = session();
    let path = std::env::temp_dir().join(format!("solidcraft-job-open-{}.solidcraft", std::process::id()));
    s.execute("file.save", &json!({"path":path})).unwrap();
    let id = start(&mut s);
    let handle = s.render_job_handle(id).unwrap();
    s.execute("doc.open", &json!({"path":path})).unwrap();
    assert_eq!(handle.status().unwrap().progress.phase, solidcraft_render::job::Phase::Cancelled);
    assert!(s.execute("render.status", &json!({"job":id})).is_err());
    assert!(s.render_job_host().unwrap().same_host(&host));
    let id = start(&mut s);
    let handle = s.render_job_handle(id).unwrap();
    s.replace_design(Session::default());
    assert_eq!(handle.status().unwrap().progress.phase, solidcraft_render::job::Phase::Cancelled);
    assert!(s.execute("render.status", &json!({"job":id})).is_err());
    assert!(s.render_job_host().unwrap().same_host(&host));
    let id = start(&mut s);
    let handle = s.render_job_handle(id).unwrap();
    let replacement = CpuHost::cooperative();
    s.set_render_job_host(Some(replacement.clone())).unwrap();
    assert_eq!(handle.status().unwrap().progress.phase, solidcraft_render::job::Phase::Cancelled);
    assert!(s.execute("render.status", &json!({"job":id})).is_err());
    let id = start(&mut s);
    let handle = s.render_job_handle(id).unwrap();
    drop(s);
    assert_eq!(handle.status().unwrap().progress.phase, solidcraft_render::job::Phase::Cancelled);
    finish(&host);
    finish(&replacement);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn captured_image_is_immutable_across_edit_and_undo_and_stale_preflight_fails() {
    let (mut s, host) = session();
    s.execute("solid.box", &json!({"length":10,"width":10,"height":10})).unwrap();
    let revision = s.revision;
    let id = start(&mut s);
    s.execute("solid.box", &json!({"length":3,"width":4,"height":5})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    finish(&host);
    let status = s.execute("render.status", &json!({"job":id})).unwrap();
    assert_eq!(status["scene_generation"], revision);
    assert_eq!(status["progress"]["phase"], "completed");
    assert_ne!(s.revision, revision);
    // Reuse the real live-gesture regression rather than manufacturing model state.
    for (command, params) in [
        ("sketch.create", json!({"plane":"XY"})),
        ("sketch.rectangle.two_point", json!({"p0":[5,5],"p1":[25,20]})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance":5})),
        ("sketch.edit", json!({"sketch":"Sketch1"})),
        ("sketch.drag", json!({"entity":"l1.start","from":[5,5],"to":[4,5],"live":true})),
    ] {
        s.execute(command, &params).unwrap();
    }
    assert!(s.model.is_stale());
    let evaluations = s.evaluations;
    assert!(s.execute("render.preflight", &request()).is_err());
    assert!(s.execute("render.start", &request()).is_err());
    assert!(s.model.is_stale());
    assert_eq!(s.evaluations, evaluations);
}

#[test]
fn native_start_and_cancel_measure_cached_capture_without_timing_assertions() {
    use std::time::{Duration, Instant};
    let mut s = Session::default();
    s.set_render_job_host(Some(CpuHost::for_platform().unwrap())).unwrap();
    s.execute("solid.box", &json!({"length":40,"width":30,"height":20})).unwrap();
    s.execute("render.scene", &json!({})).unwrap();
    let before = Instant::now();
    let reply = s.execute("render.start", &json!({})).unwrap();
    let start_time = before.elapsed();
    let id: JobId = serde_json::from_value(reply["job"].clone()).unwrap();
    let before = Instant::now();
    s.execute("render.cancel", &json!({"job":id})).unwrap();
    let cancel_request_time = before.elapsed();
    let status = s.render_job_handle(id).unwrap().wait_timeout(Duration::from_secs(60)).unwrap();
    let cancellation_time = before.elapsed();
    assert!(matches!(status.progress.phase, solidcraft_render::job::Phase::Cancelled | solidcraft_render::job::Phase::Completed));
    if status.progress.phase == solidcraft_render::job::Phase::Cancelled {
        assert!(status.frame.is_none());
    }
    eprintln!(
        "cached one-box 1024x768: start={start_time:?}; cancel request={cancel_request_time:?}; cancellation acknowledged={cancellation_time:?}"
    );
    let reply = s.execute("render.start", &json!({})).unwrap();
    let id: JobId = serde_json::from_value(reply["job"].clone()).unwrap();
    let handle = s.render_job_handle(id).unwrap();
    let watch = Instant::now();
    let observed = loop {
        let phase = handle.status().unwrap().progress.phase;
        if phase == solidcraft_render::job::Phase::Running || phase.terminal() {
            break phase;
        }
        assert!(watch.elapsed() < Duration::from_secs(60), "native worker did not advance");
        std::thread::yield_now();
    };
    let before = Instant::now();
    s.execute("render.cancel", &json!({"job":id})).unwrap();
    let request_time = before.elapsed();
    let status = handle.wait_timeout(Duration::from_secs(60)).unwrap();
    eprintln!(
        "native cancellation observed phase={observed:?}: request={request_time:?}; acknowledged={:?}; terminal={:?}",
        before.elapsed(),
        status.progress.phase
    );
    assert!(status.progress.phase.terminal());
}
#[cfg(all(feature = "raytrace-cpu", not(target_arch = "wasm32")))]
#[test]
fn native_raytrace_preflight_start_and_frame_preserve_document_and_roundtrip_request() {
    let mut s = Session::default();
    s.set_render_job_host(Some(CpuHost::for_platform().unwrap())).unwrap();
    s.execute("solid.box", &json!({"length":20,"width":15,"height":10})).unwrap();
    s.execute("render.scene", &json!({})).unwrap();
    let doc = s.doc.clone();
    let before = (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len());
    let params = json!({"integrator":"raytrace","settings":{"width":32,"height":32},"raytrace":{"preset":"neutral","shadows":true}});
    let preflight = s.execute("render.preflight", &params).unwrap();
    let request: solidcraft_render::job::ImageRequest = serde_json::from_value(preflight["request"].clone()).unwrap();
    assert_eq!(request.integrator, solidcraft_render::job::Integrator::Raytrace);
    assert_eq!(preflight["resolved"]["executor"], "cpu-raytrace");
    let started = s.execute("render.start", &params).unwrap();
    let id: JobId = serde_json::from_value(started["job"].clone()).unwrap();
    let handle = s.render_job_handle(id).unwrap();
    let status = handle.wait_timeout(std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(status.progress.phase, solidcraft_render::job::Phase::Completed);
    let frame = handle.frame().unwrap().unwrap();
    assert_eq!(frame.info.job, id);
    assert_eq!(frame.info.executor.as_str(), "cpu-raytrace");
    assert_eq!((frame.info.width, frame.info.height), (32, 32));
    assert!(frame.png().starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(Arc::ptr_eq(&doc, &s.doc));
    assert_eq!(before, (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len()));
}

#[cfg(all(feature = "raytrace-cpu", not(target_arch = "wasm32")))]
#[test]
fn captured_hdr_input_lights_geometry_after_source_removal_and_preserves_history() {
    let host = CpuHost::for_platform().unwrap();
    let mut s = Session::default();
    s.set_render_job_host(Some(host.clone())).unwrap();
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("solidcraft-hdri-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let source = dir.join("original.hdr");
    s.execute("solid.box", &json!({"length":20,"width":20,"height":20})).unwrap();
    s.execute("render.scene", &json!({})).unwrap();
    let bytes = |primaries: &str| {
        let mut b = format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n{primaries}\n-Y 2 +X 4\n").into_bytes();
        for _ in 0..8 {
            b.extend_from_slice(&[128, 64, 32, 129]);
        }
        b
    };
    std::fs::write(&source, bytes("")).unwrap();
    let params = json!({"integrator":"raytrace","raytrace":{"lights":[]},"settings":{"width":9,"height":9,"background":[13,27,41]},"environment":{"path":source.to_str().unwrap(),"samples":64}});
    let doc = s.doc.clone();
    let before = (s.revision, s.undo.len(), s.redo.len(), s.is_dirty());
    assert!(s.execute("render.preflight", &params).unwrap_err().to_string().contains("primaries"));
    assert_eq!(host.admitted_bytes(), 0);
    std::fs::write(&source, bytes("PRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n")).unwrap();
    let preflight = s.execute("render.preflight", &params).unwrap();
    assert_eq!(preflight["environment"]["color_policy"], "explicit_linear_srgb_d65");
    assert_eq!(preflight["environment"]["header"]["width"], 4);
    assert!(preflight["resolved"]["profile"]["features"].as_array().unwrap().contains(&json!("hdri")));
    assert_eq!(preflight["environment"]["source_sha256"].as_array().unwrap().len(), 32);
    assert!(!preflight["environment"].to_string().contains(dir.to_str().unwrap()));
    assert_eq!(host.admitted_bytes(), 0);
    let mut unlit = params.clone();
    unlit.as_object_mut().unwrap().remove("environment");
    let base = s.execute("render.start", &unlit).unwrap();
    let base_id: JobId = serde_json::from_value(base["job"].clone()).unwrap();
    let base_handle = s.render_jobs.handle(base_id).unwrap();
    base_handle.wait_timeout(std::time::Duration::from_secs(60)).unwrap();
    let baseline = base_handle.frame().unwrap().unwrap().png().to_vec();
    let reply = s.execute("render.start", &params).unwrap();
    assert_eq!(reply["environment"], preflight["environment"]);
    let id: JobId = serde_json::from_value(reply["job"].clone()).unwrap();
    // Captured bytes remain sufficient after the source disappears.
    std::fs::remove_file(source).unwrap();
    let handle = s.render_jobs.handle(id).unwrap();
    assert_eq!(handle.wait_timeout(std::time::Duration::from_secs(60)).unwrap().progress.phase, solidcraft_render::job::Phase::Completed);
    assert_ne!(handle.frame().unwrap().unwrap().png(), baseline.as_slice(), "captured HDR must illuminate actual geometry");
    assert_eq!(serde_json::to_value(handle.status().unwrap()).unwrap()["environment"], preflight["environment"]);
    let output = dir.join("background.png");
    s.execute("render.export", &json!({"job":id,"path":output.to_str().unwrap()})).unwrap();
    assert!(std::fs::metadata(output).unwrap().len() > 0);
    assert!(Arc::ptr_eq(&doc, &s.doc));
    assert_eq!(before, (s.revision, s.undo.len(), s.redo.len(), s.is_dirty()));
    std::fs::remove_dir_all(dir).unwrap();
}

// Spatial-AA transport regressions: WRITTEN NOT RUN in this source epoch.
#[cfg(all(feature = "raytrace-cpu", not(target_arch = "wasm32")))]
#[test]
fn native_spatial_aa_preflight_and_host_forwarding_change_actual_edge_png() {
    let mut s = Session::default();
    let host = CpuHost::for_platform().unwrap();
    s.set_render_job_host(Some(host.clone())).unwrap();
    s.execute("solid.box", &json!({"length":20,"width":15,"height":10})).unwrap();
    s.execute("render.scene", &json!({})).unwrap();
    let scene = crate::product_scene::extract(&s).unwrap();
    let center = scene.bounds.center();
    let mut camera = solidcraft_render::Camera { distance: 2.0, fov: 0.0, ..Default::default() };
    camera.set_view(solidcraft_render::StandardView::Top);
    camera.target = solidcraft_geom::Vec3::new(scene.bounds.min.x, center.y, center.z);
    let doc = s.doc.clone();
    let before = (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len());
    let mut legacy: Option<Vec<u8>> = None;
    let mut spatial: Option<Vec<u8>> = None;
    for samples in [1, 4, 16] {
        // Darkness makes covered geometry black; the tiny orthographic pixel
        // straddles the box's left edge against an exact white background.
        let params = json!({"integrator":"raytrace", "settings":{"width":1,"height":1,"camera":camera,"background":[255,255,255]},
            "raytrace":{"primary_samples":samples,"lights":[],"shadows":false}});
        let admitted = host.admitted_bytes();
        let checked = s.execute("render.preflight", &params).unwrap();
        let request: solidcraft_render::job::ImageRequest = serde_json::from_value(checked["request"].clone()).unwrap();
        assert_eq!(request.raytrace.as_ref().unwrap().primary_samples, samples);
        assert_eq!(checked["resolved"]["executor"], "cpu-raytrace");
        assert_eq!(host.admitted_bytes(), admitted, "preflight reserved host memory");
        let started = s.execute("render.start", &params).unwrap();
        let id: JobId = serde_json::from_value(started["job"].clone()).unwrap();
        let handle = s.render_job_handle(id).unwrap();
        let status = handle.wait_timeout(std::time::Duration::from_secs(60)).unwrap();
        assert_eq!(status.progress.phase, solidcraft_render::job::Phase::Completed);
        assert_eq!(status.request.raytrace.as_ref().unwrap().primary_samples, samples);
        let frame = handle.frame().unwrap().unwrap();
        assert_eq!(frame.info.executor.as_str(), "cpu-raytrace");
        assert_eq!((frame.info.width, frame.info.height), (1, 1));
        let png = frame.png().to_vec();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        if samples == 1 {
            legacy = Some(png);
        } else {
            assert_ne!(png.as_slice(), legacy.as_ref().unwrap().as_slice(), "AA was not forwarded into actual work");
            if let Some(previous) = &spatial {
                assert_eq!(&png, previous, "half coverage differs between4 and16");
            }
            spatial = Some(png);
        }
        assert!(Arc::ptr_eq(&doc, &s.doc));
        assert_eq!(before, (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len()));
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn spatial_aa_invalid_or_compiled_out_requests_refuse_before_stale_capture_and_reservation() {
    let mut s = Session::default();
    let host = CpuHost::for_platform().unwrap();
    s.set_render_job_host(Some(host.clone())).unwrap();
    for (command, params) in [
        ("sketch.create", json!({"plane":"XY"})),
        ("sketch.rectangle.two_point", json!({"p0":[5,5],"p1":[25,20]})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance":5})),
        ("sketch.edit", json!({"sketch":"Sketch1"})),
        ("sketch.drag", json!({"entity":"l1.start","from":[5,5],"to":[4,5],"live":true})),
    ] {
        s.execute(command, &params).unwrap();
    }
    assert!(s.model.is_stale());
    let doc = s.doc.clone();
    let before = (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len());
    #[cfg(feature = "raytrace-cpu")]
    let counts: &[u8] = &[0, 2, 8, 17, 255];
    #[cfg(not(feature = "raytrace-cpu"))]
    let counts: &[u8] = &[0, 2, 4, 8, 16, 17, 255];
    for samples in counts {
        for command in ["render.preflight", "render.start"] {
            let params = json!({"integrator":"raytrace","replace":true,"settings":{"width":1,"height":1},"raytrace":{"primary_samples":samples}});
            let error = s.execute(command, &params).unwrap_err().to_string();
            #[cfg(feature = "raytrace-cpu")]
            assert!(error.contains("primary samples"), "{error}");
            #[cfg(not(feature = "raytrace-cpu"))]
            assert!(error.contains("not compiled in"), "{error}");
            assert!(s.model.is_stale());
            assert!(Arc::ptr_eq(&doc, &s.doc));
            assert_eq!(before, (s.revision, s.evaluations, s.is_dirty(), s.undo.len(), s.redo.len()));
            assert_eq!(host.admitted_bytes(), 0);
        }
    }
}
