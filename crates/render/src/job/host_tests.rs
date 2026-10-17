use super::*;
use solidcraft_geom::{Aabb3, Mesh};

fn request() -> ImageRequest {
    ImageRequest {
        textures: None,
        environment: None,
        camera: Default::default(),
        width: 16,
        height: 16,
        background: [40, 80, 120],
        transparent_background: false,
    }
}
fn empty() -> ProductScene {
    ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY }
}
fn id(owner: u64, serial: u64) -> JobId {
    JobId { session: owner, serial, generation: serial }
}
fn drain(host: &CpuHost) {
    for _ in 0..10_000 {
        if !host.pump(4096).unwrap() {
            return;
        }
    }
    panic!("small cooperative jobs did not drain");
}

#[test]
fn queue_is_bounded_and_cancelled_entries_release_their_reservations() {
    let host = CpuHost::cooperative();
    let a = host.submit(id(10, 1), 100, empty(), request()).unwrap();
    let b = host.submit(id(20, 1), 200, empty(), request()).unwrap();
    let before = host.admitted_bytes();
    assert_eq!(host.submit(id(30, 1), 300, empty(), request()).err().unwrap().code, ErrorCode::QueueFull);
    assert_eq!(host.admitted_bytes(), before);
    assert_eq!(a.cancel().unwrap().progress.phase, Phase::Cancelled);
    let c = host.submit(id(30, 1), 300, empty(), request()).unwrap();
    drain(&host);
    assert_eq!(a.status().unwrap().progress.phase, Phase::Cancelled);
    for (handle, owner, generation) in [(&b, 20, 200), (&c, 30, 300)] {
        let frame = handle.frame().unwrap().unwrap();
        assert_eq!(frame.info.job, id(owner, 1));
        assert_eq!(frame.info.scene_generation, generation);
        assert_eq!(frame.info.executor.as_str(), "cpu-product");
        assert_eq!(image::load_from_memory(frame.png()).unwrap().width(), 16);
    }
    assert!(host.admitted_bytes() > 0 && host.admitted_bytes() < before);
    let frame = b.frame().unwrap().unwrap();
    drop(b);
    drop(c);
    assert!(host.admitted_bytes() > 0, "an externally retained frame retains its lease");
    drop(frame);
    assert_eq!(host.admitted_bytes(), 0);
}

#[test]
fn cancellation_while_encoding_cannot_publish_a_late_frame() {
    let host = CpuHost::cooperative();
    let job = host.submit(id(1, 1), 8, empty(), request()).unwrap();
    for _ in 0..10_000 {
        host.pump(64).unwrap();
        let status = job.status().unwrap();
        if status.progress.phase == Phase::Encoding && status.progress.done > 0 {
            break;
        }
    }
    assert_eq!(job.status().unwrap().progress.phase, Phase::Encoding);
    assert_eq!(job.cancel().unwrap().progress.phase, Phase::Cancelling);
    drain(&host);
    assert_eq!(job.status().unwrap().progress.phase, Phase::Cancelled);
    assert!(job.frame().unwrap().is_none());
    assert_eq!(host.admitted_bytes(), 0);
}

#[test]
fn malformed_preparation_fails_the_job_without_stopping_the_host() {
    let host = CpuHost::cooperative();
    let scene = ProductScene {
        bounds: Aabb3::EMPTY,
        meshes: vec![crate::ProductMesh {
            name: "Broken".into(),
            mesh: Arc::new(Mesh { triangles: vec![[99, 99, 99]], ..Default::default() }),
            color: [0; 3],
            opacity: 1.0,
            surface: None,
            transmission: None,
            faces: Vec::new(),
        }],
    };
    let broken = host.submit(id(1, 1), 1, scene, request()).unwrap();
    drain(&host);
    let status = broken.status().unwrap();
    assert_eq!(status.progress.phase, Phase::Failed);
    assert_eq!(status.error.unwrap().code, ErrorCode::InvalidScene);
    assert!(broken.frame().unwrap().is_none());
    assert!(host.is_available());
    let healthy = host.submit(id(1, 2), 1, empty(), request()).unwrap();
    drain(&host);
    assert_eq!(healthy.status().unwrap().progress.phase, Phase::Completed);
}

#[test]
fn closing_the_host_cancels_active_and_queued_work() {
    let host = CpuHost::cooperative();
    let budget = host.0.shared.budget.clone();
    let active = host.submit(id(1, 1), 1, empty(), request()).unwrap();
    let queued = host.submit(id(2, 1), 1, empty(), request()).unwrap();
    host.pump(1).unwrap();
    drop(host);
    assert_eq!(active.status().unwrap().progress.phase, Phase::Cancelled);
    assert_eq!(queued.status().unwrap().progress.phase, Phase::Cancelled);
    assert_eq!(budget.used.load(Ordering::Acquire), 0);
}

#[test]
fn a_foreign_frame_is_rejected_even_if_a_broken_host_puts_it_in_the_result_slot() {
    let host = CpuHost::cooperative();
    let job = host.submit(id(1, 1), 1, empty(), request()).unwrap();
    drain(&host);
    {
        let mut data = job.0.data.lock().unwrap();
        let frame = Arc::get_mut(data.frame.as_mut().unwrap()).unwrap();
        frame.info.job.session = 2;
    }
    assert_eq!(job.frame().err().unwrap().code, ErrorCode::WrongOwner);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_job_finishes_without_any_poll_or_ui_pump() {
    let host = CpuHost::for_platform().unwrap();
    assert_eq!(host.kind(), HostKind::NativeWorker);
    let job = host.submit(id(1, 1), 12, empty(), request()).unwrap();
    // A generous deadlock watchdog, not a performance assertion.
    let status = job.wait_timeout(std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(status.progress.phase, Phase::Completed);
    assert_eq!(job.frame().unwrap().unwrap().info.scene_generation, 12);
}

#[test]
fn environment_payload_is_owned_by_admitted_work_not_retained_status() {
    struct ResourceExecutor;
    impl ImageExecutor for ResourceExecutor {
        fn descriptor(&self) -> Result<ExecutorDescriptor> {
            CpuExecutor.descriptor()
        }
        fn admit(&self, scene: &ProductScene, request: &ImageRequest, _: JobOptions) -> Result<Admission> {
            let mut admission = super::super::admit_geometry(scene, request, 262_144)?;
            admission.estimated_total_bytes += request.environment.as_ref().map_or(0, |source| source.admitted_bytes());
            Ok(admission)
        }
        fn begin(&self, scene: ProductScene, mut request: ImageRequest, _: JobOptions, cancel: CancelToken) -> Result<Box<dyn ImageWork>> {
            request.environment = None;
            Ok(Box::new(ImageTask::new(scene, request, cancel)?))
        }
    }
    for cancelled in [false, true] {
        let host = ImageHost(Arc::new(Owner {
            executor: Arc::new(ResourceExecutor),
            shared: ImageHost::shared(),
            kind: HostKind::Cooperative,
            running: Mutex::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            _worker: None,
        }));
        let source = Arc::new(crate::lighting::environment::EnvironmentSource::sky(Default::default(), 0.0, 1.0).unwrap());
        let weak = Arc::downgrade(&source);
        let mut input = request();
        input.environment = Some(source);
        let job = host.submit(id(1, 1), 1, empty(), input).unwrap();
        let status = job.status().unwrap();
        assert!(status.request.environment.is_none());
        assert_eq!(status.environment.as_ref().unwrap().kind, "original_gradient_sky");
        assert!(weak.upgrade().is_some(), "queuedwork owns source");
        if cancelled {
            job.cancel().unwrap();
        }
        drain(&host);
        assert!(weak.upgrade().is_none(), "completed/cancelled work releases captured source despite retained status/job");
        assert_eq!(job.status().unwrap().progress.phase, if cancelled { Phase::Cancelled } else { Phase::Completed });
    }
}

#[test]
fn texture_payload_is_owned_by_admitted_work_not_retained_status() {
    struct ResourceExecutor;
    impl ImageExecutor for ResourceExecutor {
        fn descriptor(&self) -> Result<ExecutorDescriptor> {
            CpuExecutor.descriptor()
        }
        fn admit(&self, scene: &ProductScene, request: &ImageRequest, _: JobOptions) -> Result<Admission> {
            let mut admission = super::super::admit_geometry(scene, request, 262_144)?;
            admission.estimated_total_bytes += request.textures.as_ref().map_or(0, |source| source.working_bytes());
            Ok(admission)
        }
        fn begin(&self, scene: ProductScene, mut request: ImageRequest, _: JobOptions, cancel: CancelToken) -> Result<Box<dyn ImageWork>> {
            request.textures = None;
            Ok(Box::new(ImageTask::new(scene, request, cancel)?))
        }
    }
    for cancelled in [false, true] {
        let host = ImageHost(Arc::new(Owner {
            executor: Arc::new(ResourceExecutor),
            shared: ImageHost::shared(),
            kind: HostKind::Cooperative,
            running: Mutex::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            _worker: None,
        }));
        use crate::job::textures::{PlanarFrame, RuntimeTextures, TextureBinding, TextureMap};
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes).write_image(&[128, 128, 255, 255], 1, 1, image::ExtendedColorType::Rgba8).unwrap();
        let source = Arc::new(
            RuntimeTextures::new(vec![TextureBinding {
                body_index: 0,
                body_name: "unused test resource".into(),
                frame: PlanarFrame { origin: [0.; 3], u: [1., 0., 0.], v: [0., 1., 0.] },
                normal_strength: 1.,
                base_color: Some(TextureMap::new(bytes, crate::texture_input::MapRole::BaseColor).unwrap()),
                normal: None,
            }])
            .unwrap(),
        );
        let weak = Arc::downgrade(&source);
        let mut input = request();
        input.textures = Some(source);
        let job = host.submit(id(1, 1), 1, empty(), input).unwrap();
        let status = job.status().unwrap();
        assert!(status.request.textures.is_none());
        assert_eq!(status.textures.as_ref().unwrap().bodies, 1);
        assert!(weak.upgrade().is_some(), "queuedwork owns source");
        if cancelled {
            job.cancel().unwrap();
        }
        drain(&host);
        assert!(weak.upgrade().is_none(), "completed/cancelled work releases captured source despite retained status/job");
        assert_eq!(job.status().unwrap().progress.phase, if cancelled { Phase::Cancelled } else { Phase::Completed });
    }
}
/// An executor whose task reports, as it is dropped, how much of the pool is admitted, and
/// completes after `steps` steps (or never, cancelled on its last one with `cancel_last`).
struct DropProbe {
    admitted_at_drop: Arc<AtomicU64>,
    budget: Arc<std::sync::OnceLock<Arc<Budget>>>,
    estimated: u64,
}
struct ProbeTask {
    admitted_at_drop: Arc<AtomicU64>,
    budget: Arc<std::sync::OnceLock<Arc<Budget>>>,
    steps: u32,
}
impl Drop for ProbeTask {
    fn drop(&mut self) {
        let used = self.budget.get().map_or(u64::MAX, |b| b.used.load(Ordering::Acquire));
        self.admitted_at_drop.store(used, Ordering::Release);
    }
}
impl ImageWork for ProbeTask {
    fn step(&mut self, _units: usize) -> Result<TaskPoll> {
        self.steps += 1;
        if self.steps < 3 {
            return Ok(TaskPoll::Pending(Progress { phase: Phase::Running, done: u64::from(self.steps), total: 3 }));
        }
        Ok(TaskPoll::Complete(vec![0u8; 100]))
    }
}
impl ImageExecutor for DropProbe {
    fn descriptor(&self) -> Result<ExecutorDescriptor> {
        CpuExecutor.descriptor()
    }
    fn admit(&self, _: &ProductScene, _: &ImageRequest, _: JobOptions) -> Result<Admission> {
        Ok(Admission {
            triangles: 0,
            face_appearances: 0,
            geometry_bytes: 0,
            pixel_scratch_bytes: self.estimated,
            preparation_bytes: 0,
            estimated_total_bytes: self.estimated,
            encoded_limit: 4096,
        })
    }
    fn begin(&self, _: ProductScene, _: ImageRequest, _: JobOptions, _: CancelToken) -> Result<Box<dyn ImageWork>> {
        Ok(Box::new(ProbeTask { admitted_at_drop: self.admitted_at_drop.clone(), budget: self.budget.clone(), steps: 0 }))
    }
}

fn probe_host(estimated: u64) -> (CpuHost, Arc<AtomicU64>) {
    let admitted_at_drop = Arc::new(AtomicU64::new(0));
    let budget = Arc::new(std::sync::OnceLock::new());
    let host = CpuHost::cooperative_with(Arc::new(DropProbe { admitted_at_drop: admitted_at_drop.clone(), budget: budget.clone(), estimated }));
    let _ = budget.set(host.0.shared.budget.clone());
    (host, admitted_at_drop)
}

#[test]
fn a_completed_task_is_dropped_while_its_full_reservation_still_pays_for_it() {
    let estimated = 1 << 20;
    let (host, admitted_at_drop) = probe_host(estimated);
    let job = host.submit(id(1, 1), 1, empty(), request()).unwrap();
    drain(&host);
    assert_eq!(job.status().unwrap().progress.phase, Phase::Completed);
    // While the task (and its scratch) was being dropped, its whole charge was still held.
    assert_eq!(admitted_at_drop.load(Ordering::Acquire), estimated);
    // Afterwards only the PNG's lease remains, and the frame keeps it.
    let frame = job.frame().unwrap().unwrap();
    assert!(host.admitted_bytes() > 0 && host.admitted_bytes() < estimated, "{}", host.admitted_bytes());
    drop(job);
    drop(frame);
    assert_eq!(host.admitted_bytes(), 0);
}

#[test]
fn a_cancel_on_the_final_step_publishes_no_frame_and_drops_the_task() {
    let estimated = 1 << 20;
    let (host, admitted_at_drop) = probe_host(estimated);
    let job = host.submit(id(1, 1), 1, empty(), request()).unwrap();
    // Two pending steps, then cancel before the step that would complete.
    host.pump(1).unwrap();
    host.pump(1).unwrap();
    job.cancel().unwrap();
    drain(&host);
    assert_eq!(job.status().unwrap().progress.phase, Phase::Cancelled);
    assert!(job.frame().unwrap().is_none());
    assert_eq!(admitted_at_drop.load(Ordering::Acquire), estimated, "the task went before its charge");
    assert_eq!(host.admitted_bytes(), 0);
}
