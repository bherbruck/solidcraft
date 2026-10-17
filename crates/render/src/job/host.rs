//! Caller-owned image execution host. One worker, two bounded queued inputs; browser pumping
//! uses the same incremental task without threads. Shared ownership never includes CAD state.

use std::collections::VecDeque;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Condvar;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use serde::Serialize;

use super::{
    Admission, AlphaConvention, CancelToken, ColourEncoding, ErrorCode, FrameInfo, ImageExecutor, ImageRequest, ImageTask, ImageWork, JobError,
    JobId, JobOptions, Phase, Progress, Result, TaskPoll, admit,
};
use crate::{
    Availability, ExecutorDescriptor, ExecutorId, ProductScene, RenderFormat, RenderLimits, RenderMode, RenderOperation, RenderProfile, ScratchLayout,
};

const MEMORY_LIMIT: u64 = 512 * 1024 * 1024;
const QUEUE_LIMIT: usize = 2;

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKind {
    NativeWorker,
    Cooperative,
}

struct Budget {
    used: AtomicU64,
}
struct Reservation {
    budget: Arc<Budget>,
    bytes: u64,
}

impl Budget {
    fn reserve(self: &Arc<Self>, bytes: u64) -> Result<Reservation> {
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            let next = used
                .checked_add(bytes)
                .filter(|n| *n <= MEMORY_LIMIT)
                .ok_or_else(|| JobError::new(ErrorCode::LimitExceeded, "image host's 512 MiB admission pool is full"))?;
            match self.used.compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(current) => used = current,
            }
        }
        Ok(Reservation { budget: self.clone(), bytes })
    }
}
impl Reservation {
    fn shrink(&mut self, bytes: u64) -> Result<()> {
        if bytes > self.bytes {
            return Err(JobError::new(ErrorCode::LimitExceeded, "completed PNG exceeds its reservation"));
        }
        self.budget.used.fetch_sub(self.bytes - bytes, Ordering::AcqRel);
        self.bytes = bytes;
        Ok(())
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Borrow encoded bytes through the frame, so retaining the data also retains its admission lease.
pub struct RenderFrame {
    pub info: FrameInfo,
    png: Vec<u8>,
    _reservation: Reservation,
}
impl RenderFrame {
    pub fn png(&self) -> &[u8] {
        &self.png
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct JobStatus {
    pub job: JobId,
    pub scene_generation: u64,
    pub request: ImageRequest,
    pub resolved: crate::ResolvedRequest,
    pub options: JobOptions,
    pub progress: Progress,
    pub error: Option<JobError>,
    pub frame: Option<FrameInfo>,
}

struct Data {
    progress: Progress,
    error: Option<JobError>,
    frame: Option<Arc<RenderFrame>>,
}
struct Record {
    id: JobId,
    scene_generation: u64,
    request: ImageRequest,
    resolved: crate::ResolvedRequest,
    cancel: CancelToken,
    options: JobOptions,
    data: Mutex<Data>,
    #[cfg(not(target_arch = "wasm32"))]
    done: Condvar,
}

impl Record {
    fn notify(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.done.notify_all();
    }
}

#[derive(Clone)]
pub struct JobHandle(Arc<Record>);

impl JobHandle {
    pub fn id(&self) -> JobId {
        self.0.id
    }
    pub fn status(&self) -> Result<JobStatus> {
        let data = self.0.data.lock().map_err(|_| JobError::new(ErrorCode::Internal, "render status lock poisoned"))?;
        Ok(JobStatus {
            job: self.0.id,
            scene_generation: self.0.scene_generation,
            request: self.0.request.clone(),
            resolved: self.0.resolved.clone(),
            options: self.0.options,
            progress: data.progress,
            error: data.error.clone(),
            frame: data.frame.as_ref().map(|f| f.info.clone()),
        })
    }
    pub fn cancel(&self) -> Result<JobStatus> {
        {
            let mut data = self.0.data.lock().map_err(|_| JobError::new(ErrorCode::Internal, "render status lock poisoned"))?;
            if !data.progress.phase.terminal() {
                self.0.cancel.cancel();
                data.progress.phase = if data.progress.phase == Phase::Queued { Phase::Cancelled } else { Phase::Cancelling };
            }
        }
        self.0.notify();
        self.status()
    }

    /// Native application helper; engine status commands remain non-blocking.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn wait_timeout(&self, timeout: std::time::Duration) -> Result<JobStatus> {
        let data = self.0.data.lock().map_err(|_| JobError::new(ErrorCode::Internal, "render status lock poisoned"))?;
        let (data, timed) = self
            .0
            .done
            .wait_timeout_while(data, timeout, |d| !d.progress.phase.terminal())
            .map_err(|_| JobError::new(ErrorCode::Internal, "render completion lock poisoned"))?;
        if timed.timed_out() && !data.progress.phase.terminal() {
            return Err(JobError::new(ErrorCode::TimedOut, "render wait timed out"));
        }
        drop(data);
        self.status()
    }
    pub fn frame(&self) -> Result<Option<Arc<RenderFrame>>> {
        let data = self.0.data.lock().map_err(|_| JobError::new(ErrorCode::Internal, "render result lock poisoned"))?;
        let frame = data.frame.clone();
        if frame.as_ref().is_some_and(|f| f.info.job != self.0.id || f.info.scene_generation != self.0.scene_generation) {
            return Err(JobError::new(ErrorCode::WrongOwner, "render frame identity does not match its job"));
        }
        Ok(frame)
    }
}

struct Work {
    executor: Arc<dyn ImageExecutor>,
    scene: ProductScene,
    handle: JobHandle,
    reservation: Reservation,
}
struct Running {
    /// Taken and dropped as the frame completes, before the reservation shrinks to the PNG: the
    /// task owns its scratch (pixels, device buffers), which the full reservation pays for.
    task: Option<Box<dyn ImageWork>>,
    handle: JobHandle,
    reservation: Option<Reservation>,
}

fn terminal(handle: &JobHandle, error: JobError) {
    if let Ok(mut data) = handle.0.data.lock() {
        if data.progress.phase == Phase::Completed {
            return;
        }
        if handle.0.cancel.is_cancelled() || error.code == ErrorCode::Cancelled {
            data.progress.phase = Phase::Cancelled;
            data.error = None;
        } else {
            data.progress.phase = Phase::Failed;
            data.error = Some(error);
        }
        data.frame = None;
    }
    handle.0.notify();
}

impl Running {
    fn begin(work: Work) -> Option<Self> {
        if work.handle.0.cancel.is_cancelled() {
            terminal(&work.handle, JobError::new(ErrorCode::Cancelled, "render cancelled"));
            return None;
        }
        if let Ok(mut data) = work.handle.0.data.lock()
            && !work.handle.0.cancel.is_cancelled()
        {
            data.progress.phase = Phase::Preparing;
        }
        let task = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            work.executor.begin(work.scene, work.handle.0.request.clone(), work.handle.0.options, work.handle.0.cancel.clone())
        }))
        .unwrap_or_else(|_| Err(JobError::new(ErrorCode::Internal, "image executor failed while preparing the job")));
        match task {
            Ok(task) => Some(Self { task: Some(task), handle: work.handle, reservation: Some(work.reservation) }),
            Err(e) => {
                terminal(&work.handle, e);
                None
            }
        }
    }

    fn advance(&mut self, units: usize) -> Result<bool> {
        let task = self.task.as_mut().ok_or_else(|| JobError::new(ErrorCode::Internal, "render task already finished"))?;
        let result = task.step(units)?;
        let mut data = self.handle.0.data.lock().map_err(|_| JobError::new(ErrorCode::Internal, "render status lock poisoned"))?;
        // The final publication check is under the same lock as cancellation.
        if self.handle.0.cancel.is_cancelled() {
            data.progress.phase = Phase::Cancelled;
            data.frame = None;
            self.handle.0.notify();
            return Ok(false);
        }
        match result {
            TaskPoll::Pending(progress) => {
                data.progress = progress;
                Ok(true)
            }
            TaskPoll::Complete(png) => {
                // The task's scratch goes before its charge does (still under the publication lock).
                drop(self.task.take());
                let mut reservation = self.reservation.take().ok_or_else(|| JobError::new(ErrorCode::Internal, "missing render reservation"))?;
                reservation.shrink(png.capacity() as u64 + 1024)?;
                let info = FrameInfo {
                    job: self.handle.0.id,
                    scene_generation: self.handle.0.scene_generation,
                    backend: self.handle.0.resolved.backend.clone(),
                    executor: self.handle.0.resolved.executor.clone(),
                    width: self.handle.0.request.width,
                    height: self.handle.0.request.height,
                    encoding: ColourEncoding::Srgb,
                    alpha: AlphaConvention::Straight,
                };
                data.frame = Some(Arc::new(RenderFrame { info, png, _reservation: reservation }));
                data.progress = Progress { phase: Phase::Completed, done: 1, total: 1 };
                self.handle.0.notify();
                Ok(false)
            }
        }
    }

    fn guarded_step(&mut self, units: usize) -> bool {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.advance(units)));
        match result {
            Ok(Ok(more)) => more,
            Ok(Err(e)) => {
                terminal(&self.handle, e);
                false
            }
            Err(_) => {
                terminal(&self.handle, JobError::new(ErrorCode::Internal, "image host encountered an internal error"));
                false
            }
        }
    }
}

struct Shared {
    queue: Mutex<VecDeque<Work>>,
    active: Mutex<Option<Weak<Record>>>,
    stop: AtomicBool,
    budget: Arc<Budget>,
    #[cfg(not(target_arch = "wasm32"))]
    wake: Condvar,
}

struct Owner {
    executor: Arc<dyn ImageExecutor>,
    shared: Arc<Shared>,
    kind: HostKind,
    running: Mutex<Option<Running>>,
    #[cfg(not(target_arch = "wasm32"))]
    _worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Ok(active) = self.shared.active.lock()
            && let Some(record) = active.as_ref().and_then(Weak::upgrade)
        {
            let _ = JobHandle(record).cancel();
        }
        if let Ok(mut queue) = self.shared.queue.lock() {
            for work in queue.iter() {
                let _ = work.handle.cancel();
            }
            queue.clear();
        }
        if let Ok(mut running) = self.running.lock()
            && let Some(work) = running.take()
        {
            let _ = work.handle.cancel();
            terminal(&work.handle, JobError::new(ErrorCode::Cancelled, "render host closed"));
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.shared.wake.notify_all();
        // Detaching cannot stall the UI; the bounded worker observes stop/cancel at its next unit.
    }
}

#[derive(Clone)]
pub struct ImageHost(Arc<Owner>);

/// Backwards-compatible CPU host constructors. Additional executors use `ImageHost::with_executor`.
pub type CpuHost = ImageHost;

impl ImageHost {
    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            active: Mutex::new(None),
            stop: AtomicBool::new(false),
            budget: Arc::new(Budget { used: AtomicU64::new(0) }),
            #[cfg(not(target_arch = "wasm32"))]
            wake: Condvar::new(),
        })
    }

    pub fn cooperative() -> Self {
        Self::cooperative_with(Arc::new(CpuExecutor))
    }

    /// A cooperative host over `executor` (pumped by its caller).
    fn cooperative_with(executor: Arc<dyn ImageExecutor>) -> Self {
        Self(Arc::new(Owner {
            executor,
            shared: Self::shared(),
            kind: HostKind::Cooperative,
            running: Mutex::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            _worker: None,
        }))
    }

    pub fn for_platform() -> Result<Self> {
        #[cfg(target_arch = "wasm32")]
        {
            Ok(Self::cooperative())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::with_executor(Arc::new(CpuExecutor))
        }
    }

    /// Start an isolated bounded native worker. GPU discovery occurs before registration.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_executor(executor: Arc<dyn ImageExecutor>) -> Result<Self> {
        let descriptor = executor.descriptor()?;
        if descriptor.profiles.len() != 1
            || descriptor
                .profiles
                .iter()
                .any(|p| p.mode != RenderMode::Preview || p.operation != RenderOperation::Image || p.formats != [RenderFormat::Png])
        {
            return Err(JobError::new(ErrorCode::InvalidRequest, "image worker currently accepts one Preview/Image/PNG profile"));
        }
        let shared = Self::shared();
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("solidcraft-render".into())
            .spawn(move || native_worker(worker_shared))
            .map_err(|e| JobError::new(ErrorCode::HostUnavailable, format!("could not start image worker: {e}")))?;
        Ok(Self(Arc::new(Owner { executor, shared, kind: HostKind::NativeWorker, running: Mutex::new(None), _worker: Some(worker) })))
    }

    pub fn kind(&self) -> HostKind {
        self.0.kind
    }

    pub fn same_host(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn is_available(&self) -> bool {
        !self.0.shared.stop.load(Ordering::Acquire)
    }
    pub fn admitted_bytes(&self) -> u64 {
        self.0.shared.budget.used.load(Ordering::Acquire)
    }

    pub fn descriptor(&self) -> Result<ExecutorDescriptor> {
        let mut descriptor = self.0.executor.descriptor()?;
        if !self.is_available() {
            descriptor.availability = Availability::Unavailable { reason: "image host has stopped".into() };
        }
        Ok(descriptor)
    }

    pub fn admit(&self, scene: &ProductScene, request: &ImageRequest, options: JobOptions) -> Result<Admission> {
        options.validate()?;
        self.0.executor.admit(scene, request, options)
    }

    pub fn submit(&self, id: JobId, scene_generation: u64, scene: ProductScene, request: ImageRequest) -> Result<JobHandle> {
        self.submit_with_options(id, scene_generation, scene, request, JobOptions::default())
    }

    pub fn submit_with_options(
        &self,
        id: JobId,
        scene_generation: u64,
        scene: ProductScene,
        request: ImageRequest,
        options: JobOptions,
    ) -> Result<JobHandle> {
        if id.session == 0 || id.serial == 0 || id.generation == 0 {
            return Err(JobError::new(ErrorCode::InvalidRequest, "invalid render job identity"));
        }
        let Admission { estimated_total_bytes, .. } = self.admit(&scene, &request, options)?;
        let mut backend = crate::builtin_backend();
        let descriptor = self.descriptor()?;
        let executor = descriptor.id.clone();
        backend.executors = vec![descriptor];
        let resolved = crate::resolve_request(
            &[backend],
            &crate::RenderRequest {
                backend: None,
                mode: RenderMode::Preview,
                operation: RenderOperation::Image,
                width: u64::from(request.width),
                height: u64::from(request.height),
                format: Some(RenderFormat::Png),
                features: scene.required_features(),
                policy: Default::default(),
            },
        )
        .map_err(|e| JobError::new(ErrorCode::HostUnavailable, e.to_string()))?;
        if resolved.executor != executor {
            return Err(JobError::new(ErrorCode::HostUnavailable, "image executor is unavailable"));
        }
        let mut queue = self.0.shared.queue.lock().map_err(|_| JobError::new(ErrorCode::HostUnavailable, "render queue lock poisoned"))?;
        if self.0.shared.stop.load(Ordering::Acquire) {
            return Err(JobError::new(ErrorCode::HostUnavailable, "image host has stopped"));
        }
        queue.retain(|work| !work.handle.0.cancel.is_cancelled());
        if queue.len() >= QUEUE_LIMIT {
            return Err(JobError::new(ErrorCode::QueueFull, "image queue is full"));
        }
        let reservation = self.0.shared.budget.reserve(estimated_total_bytes)?;
        let handle = JobHandle(Arc::new(Record {
            id,
            scene_generation,
            request,
            resolved,
            cancel: CancelToken::default(),
            options,
            data: Mutex::new(Data { progress: Progress { phase: Phase::Queued, done: 0, total: 1 }, error: None, frame: None }),
            #[cfg(not(target_arch = "wasm32"))]
            done: Condvar::new(),
        }));
        queue.push_back(Work { executor: self.0.executor.clone(), scene, handle: handle.clone(), reservation });
        drop(queue);
        #[cfg(not(target_arch = "wasm32"))]
        self.0.shared.wake.notify_one();
        Ok(handle)
    }

    /// Cooperative callers pump a small quantum independently of polling a job's status.
    /// Native jobs run even when the UI is idle or a headless caller is not polling.
    pub fn pump(&self, units: usize) -> Result<bool> {
        if self.0.kind != HostKind::Cooperative {
            return Ok(false);
        }
        let mut running = self.0.running.lock().map_err(|_| JobError::new(ErrorCode::HostUnavailable, "cooperative host lock poisoned"))?;
        if running.is_none() {
            let work = self.0.shared.queue.lock().map_err(|_| JobError::new(ErrorCode::HostUnavailable, "render queue lock poisoned"))?.pop_front();
            if let Some(work) = work {
                *running = Running::begin(work);
            }
        }
        if let Some(work) = running.as_mut() {
            if let Ok(mut active) = self.0.shared.active.lock() {
                *active = Some(Arc::downgrade(&work.handle.0));
            }
            if !work.guarded_step(units) {
                *running = None;
                if let Ok(mut active) = self.0.shared.active.lock() {
                    *active = None;
                }
            }
        }
        Ok(running.is_some()
            || !self.0.shared.queue.lock().map_err(|_| JobError::new(ErrorCode::HostUnavailable, "render queue lock poisoned"))?.is_empty())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn native_worker(shared: Arc<Shared>) {
    loop {
        let mut queue = match shared.queue.lock() {
            Ok(q) => q,
            Err(_) => {
                shared.stop.store(true, Ordering::Release);
                return;
            }
        };
        while queue.is_empty() && !shared.stop.load(Ordering::Acquire) {
            queue = match shared.wake.wait(queue) {
                Ok(q) => q,
                Err(_) => {
                    shared.stop.store(true, Ordering::Release);
                    return;
                }
            };
        }
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let work = queue.pop_front();
        drop(queue);
        let Some(mut running) = work.and_then(Running::begin) else { continue };
        if let Ok(mut active) = shared.active.lock() {
            *active = Some(Arc::downgrade(&running.handle.0));
        }
        loop {
            if shared.stop.load(Ordering::Acquire) {
                let _ = running.handle.cancel();
            }
            if !running.guarded_step(4096) {
                break;
            }
        }
        if let Ok(mut active) = shared.active.lock() {
            *active = None;
        }
    }
}

struct CpuExecutor;
impl ImageExecutor for CpuExecutor {
    fn descriptor(&self) -> Result<ExecutorDescriptor> {
        Ok(ExecutorDescriptor {
            id: ExecutorId::new("cpu-product").map_err(|e| JobError::new(ErrorCode::Internal, e.to_string()))?,
            availability: Availability::Available,
            profiles: vec![RenderProfile {
                mode: RenderMode::Preview,
                operation: RenderOperation::Image,
                formats: vec![RenderFormat::Png],
                features: vec![crate::RenderFeature::Pbr],
            }],
            limits: RenderLimits::default(),
            scratch: ScratchLayout { sample_scale: 2, sample_bytes_per_pixel: 8, output_bytes_per_pixel: 8, encoding_bytes_per_pixel: 8 },
        })
    }

    fn admit(&self, scene: &ProductScene, request: &ImageRequest, options: JobOptions) -> Result<Admission> {
        if options != JobOptions::default() {
            return Err(JobError::new(ErrorCode::InvalidRequest, "CPU product does not support sampling or exposure controls"));
        }
        admit(scene, request)
    }
    fn begin(&self, scene: ProductScene, request: ImageRequest, _options: JobOptions, cancel: CancelToken) -> Result<Box<dyn ImageWork>> {
        Ok(Box::new(ImageTask::new(scene, request, cancel)?))
    }
}
