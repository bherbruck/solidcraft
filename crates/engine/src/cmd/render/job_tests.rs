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

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn additional_executor_routes_by_capability_and_survives_document_adoption() {
    use solidcraft_render::job::{Admission, CancelToken, ImageExecutor, ImageHost, ImageRequest, ImageTask, ImageWork, JobOptions};
    struct Progressive;
    impl ImageExecutor for Progressive {
        fn descriptor(&self) -> solidcraft_render::job::Result<solidcraft_render::ExecutorDescriptor> {
            let mut descriptor = CpuHost::cooperative().descriptor()?;
            descriptor.id = solidcraft_render::ExecutorId::new("test-progressive").unwrap();
            descriptor.profiles[0].features.push(solidcraft_render::RenderFeature::Progressive);
            Ok(descriptor)
        }
        fn admit(
            &self,
            scene: &solidcraft_render::ProductScene,
            request: &ImageRequest,
            options: JobOptions,
        ) -> solidcraft_render::job::Result<Admission> {
            options.validate()?;
            solidcraft_render::job::admit(scene, request)
        }
        fn begin(
            &self,
            scene: solidcraft_render::ProductScene,
            request: ImageRequest,
            _: JobOptions,
            cancel: CancelToken,
        ) -> solidcraft_render::job::Result<Box<dyn ImageWork>> {
            Ok(Box::new(ImageTask::new(scene, request, cancel)?))
        }
    }
    let (mut session, cpu) = session();
    let additional = ImageHost::with_executor(Arc::new(Progressive)).unwrap();
    session.register_render_job_host(additional.clone()).unwrap();
    assert!(
        session.register_render_job_host(ImageHost::with_executor(Arc::new(Progressive)).unwrap()).is_err(),
        "duplicate executor IDs must be rejected"
    );
    let cpu_id = start(&mut session);
    let request = json!({"settings":{"width":16,"height":16},"progressive":true,"samples":3});
    let preflight = session.execute("render.preflight", &request).unwrap();
    assert_eq!(preflight["resolved"]["executor"], "test-progressive");
    assert_eq!(additional.admitted_bytes(), 0);
    let started = session.execute("render.start", &request).unwrap();
    let id: JobId = serde_json::from_value(started["job"].clone()).unwrap();
    let status = session.render_job_handle(id).unwrap().wait_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(status.resolved.executor.as_str(), "test-progressive");
    assert_eq!(status.options.samples, 3);
    assert_eq!(status.progress.phase, solidcraft_render::job::Phase::Completed);
    finish(&cpu);
    assert_eq!(session.execute("render.status", &json!({"job":cpu_id})).unwrap()["frame"]["executor"], "cpu-product");
    assert!(session.scratch().execute("render.status", &json!({"job":id})).is_err());
    session.execute("file.new", &json!({})).unwrap();
    assert_eq!(session.execute("render.preflight", &request).unwrap()["resolved"]["executor"], "test-progressive");
    assert!(session.execute("render.status", &json!({"job":id})).is_err());
    for bad in [json!({"samples":0}), json!({"max_depth":17}), json!({"seed":-1}), json!({"samples":4097})] {
        let mut params = request.clone();
        for (key, value) in bad.as_object().unwrap() {
            params[key] = value.clone();
        }
        assert!(session.execute("render.start", &params).is_err());
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
