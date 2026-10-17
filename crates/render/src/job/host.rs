//! Caller-owned CPU execution host. One worker, two bounded queued inputs; browser pumping
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
    Admission, AlphaConvention, CancelToken, ColourEncoding, ErrorCode, FrameInfo, ImageRequest, ImageTask, Integrator, JobError, JobId, Phase,
    Progress, Result, TaskPoll, admit,
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
                .ok_or_else(|| JobError::new(ErrorCode::LimitExceeded, "CPU image host's 512 MiB admission pool is full"))?;
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

/// Retained runtime input capacities survive work/PNG eviction through status handles.
fn retained_request_bytes(request: &ImageRequest) -> Result<u64> {
    request.raytrace.as_ref().map_or(Ok(0), |options| options.retained_bytes())
}

#[cfg(all(test, feature = "raytrace-cpu", not(target_arch = "wasm32")))]
mod light_union_lease_tests {
    use super::super::{DirectionalLightOptions, PunctualLightOptions, RaytraceOptions, led_strip::LedStripOptions};
    use super::*;
    fn request() -> Result<ImageRequest> {
        let mut options = RaytraceOptions {
            primary_samples: 16,
            lights: Some(vec![DirectionalLightOptions { direction: [0.0, 0.0, 1.0], color: [1.0; 3], intensity: 1.0 }; 8]),
            punctual_lights: Some(vec![
                PunctualLightOptions {
                    position_mm: [0.0, 0.0, 10.0],
                    color: [1.0; 3],
                    intensity: 1.0,
                    reference_distance_mm: 10.0,
                    minimum_distance_mm: 0.001,
                    cone: None
                };
                8
            ]),
            ..Default::default()
        };
        let strip = LedStripOptions {
            center_mm: [0.0, 0.0, 10.0],
            length_mm: 8.0,
            width_mm: 0.5,
            length_axis: [1.0, 0.0, 0.0],
            width_axis: [0.0, -1.0, 0.0],
            color: [1.0; 3],
            intensity: 2.0,
        };
        for _ in 0..4 {
            strip.append_to(&mut options, 64)?;
        }
        Ok(ImageRequest {
            integrator: Integrator::Raytrace,
            raytrace: Some(options),
            camera: Default::default(),
            width: 8,
            height: 8,
            background: [0; 3],
            transparent_background: false,
        })
    }
    fn scene() -> ProductScene {
        ProductScene { meshes: Vec::new(), bounds: solidcraft_geom::Aabb3::EMPTY }
    }
    #[test]
    fn normalized_union_uses_one_pool_and_keeps_record_lease_after_frame_eviction() {
        let mut host = CpuHost::for_platform().unwrap();
        let budget = host.0.shared.budget.clone();
        let request = request().unwrap();
        let bytes = retained_request_bytes(&request).unwrap();
        let capacities = request
            .raytrace
            .as_ref()
            .map(|o| {
                (o.lights.as_ref().unwrap().capacity(), o.punctual_lights.as_ref().unwrap().capacity(), o.area_lights.as_ref().unwrap().capacity())
            })
            .unwrap();
        let job = host.submit(JobId { session: 1724, serial: 1, generation: 1 }, 1, scene(), request).unwrap();
        assert_eq!(job.wait_timeout(std::time::Duration::from_secs(60)).unwrap().progress.phase, Phase::Completed);
        // Synchronize this test's completed worker before asserting final Arc release;
        // publication wakes waiters just before Running drops its temporary handle.
        host.0.shared.stop.store(true, Ordering::Release);
        host.0.shared.wake.notify_all();
        Arc::get_mut(&mut host.0).unwrap()._worker.take().unwrap().join().unwrap();
        let retained = job.0.request.raytrace.as_ref().unwrap();
        assert_eq!(
            capacities,
            (
                retained.lights.as_ref().unwrap().capacity(),
                retained.punctual_lights.as_ref().unwrap().capacity(),
                retained.area_lights.as_ref().unwrap().capacity()
            )
        );
        assert!(Arc::ptr_eq(&job.0._request_reservation.as_ref().unwrap().budget, &budget));
        assert_eq!(job.0._request_reservation.as_ref().unwrap().bytes, bytes);
        let frame = job.frame().unwrap().unwrap();
        job.0.data.lock().unwrap().frame = None;
        assert!(host.admitted_bytes() > bytes);
        drop(frame);
        assert_eq!(host.admitted_bytes(), bytes);
        let held = job.clone();
        drop(job);
        assert_eq!(host.admitted_bytes(), bytes);
        drop(held);
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
    }
    #[test]
    fn failed_union_metadata_reservation_releases_work_and_does_not_enqueue() {
        let host = CpuHost::for_platform().unwrap();
        let request = request().unwrap();
        let work = admit(&scene(), &request).unwrap().estimated_total_bytes;
        let fill = host.0.shared.budget.reserve(MEMORY_LIMIT - work).unwrap();
        let before = host.admitted_bytes();
        let result = host.submit(JobId { session: 1724, serial: 2, generation: 1 }, 1, scene(), request);
        assert_eq!(result.err().unwrap().code, ErrorCode::LimitExceeded);
        assert_eq!(host.admitted_bytes(), before);
        assert!(host.0.shared.queue.lock().unwrap().is_empty());
        drop(fill);
        assert_eq!(host.admitted_bytes(), 0);
    }
}

fn admit_request_sum(work_bytes: u64, request_bytes: u64) -> Result<()> {
    if work_bytes.checked_add(request_bytes).is_none_or(|sum| sum > super::image::MAX_JOB_BYTES) {
        return Err(JobError::new(ErrorCode::LimitExceeded, "render work and retained request exceed 384 MiB per-job admission"));
    }
    Ok(())
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<crate::lighting::environment_input::EnvironmentInfo>,
    pub job: JobId,
    pub scene_generation: u64,
    pub request: ImageRequest,
    pub resolved: crate::ResolvedRequest,
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
    environment: Option<crate::lighting::environment_input::EnvironmentInfo>,
    _request_reservation: Option<Reservation>,
    id: JobId,
    scene_generation: u64,
    request: ImageRequest,
    resolved: crate::ResolvedRequest,
    cancel: CancelToken,
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
            environment: self.0.environment.clone(),
            job: self.0.id,
            scene_generation: self.0.scene_generation,
            request: self.0.request.clone(),
            resolved: self.0.resolved.clone(),
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
    environment: Option<crate::lighting::environment_input::EnvironmentInput>,
    scene: ProductScene,
    handle: JobHandle,
    reservation: Reservation,
}
struct Running {
    task: Option<WorkTask>,
    handle: JobHandle,
    reservation: Option<Reservation>,
}

/// One fallibly allocated task, keeping the dispatch enum small in either build.
/// Its bounded metadata allocation fits the existing per-job task allowance.
struct TaskSlot<T>(Vec<T>);
impl<T> TaskSlot<T> {
    fn new(task: T) -> Result<Self> {
        if std::mem::size_of::<T>() > 4096 {
            return Err(JobError::new(ErrorCode::LimitExceeded, "render task metadata exceeds its admission allowance"));
        }
        let mut tasks = Vec::new();
        tasks.try_reserve_exact(1).map_err(|_| JobError::new(ErrorCode::AllocationFailed, "could not reserve render task slot"))?;
        if tasks.capacity() != 1 {
            return Err(JobError::new(ErrorCode::LimitExceeded, "render task slot exceeds its bounded capacity"));
        }
        tasks.push(task);
        Ok(Self(tasks))
    }
    fn get_mut(&mut self) -> Result<&mut T> {
        self.0.first_mut().ok_or_else(|| JobError::new(ErrorCode::Internal, "missing render task slot"))
    }
}
enum WorkTask {
    Raster(TaskSlot<ImageTask>),
    #[cfg(feature = "raytrace-cpu")]
    Raytrace(TaskSlot<crate::raytrace::image::ImageTask>),
}

impl WorkTask {
    fn new(
        scene: ProductScene,
        request: ImageRequest,
        cancel: CancelToken,
        environment: Option<crate::lighting::environment_input::EnvironmentInput>,
    ) -> Result<Self> {
        let _ = &environment;
        match request.integrator {
            Integrator::Raster => ImageTask::new(scene, request, cancel).and_then(TaskSlot::new).map(Self::Raster),
            Integrator::Raytrace => {
                #[cfg(feature = "raytrace-cpu")]
                {
                    let options = request.raytrace.clone().unwrap_or_default();
                    crate::raytrace::image::ImageTask::new_with_environment(scene, request, cancel, options, environment)
                        .and_then(TaskSlot::new)
                        .map(Self::Raytrace)
                }
                #[cfg(not(feature = "raytrace-cpu"))]
                {
                    Err(JobError::new(ErrorCode::HostUnavailable, "CPU raytracing is not compiled in; enable raytrace-cpu"))
                }
            }
        }
    }

    fn step(&mut self, units: usize) -> Result<TaskPoll> {
        match self {
            Self::Raster(task) => task.get_mut()?.step(units),
            #[cfg(feature = "raytrace-cpu")]
            Self::Raytrace(task) => task.get_mut()?.step(units),
        }
    }
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
        let task = WorkTask::new(work.scene, work.handle.0.request.clone(), work.handle.0.cancel.clone(), work.environment);
        match task {
            Ok(task) => Some(Self { task: Some(task), handle: work.handle, reservation: Some(work.reservation) }),
            Err(e) => {
                terminal(&work.handle, e);
                None
            }
        }
    }

    fn advance(&mut self, units: usize) -> Result<bool> {
        let result = self.task.as_mut().ok_or_else(|| JobError::new(ErrorCode::Internal, "missing render task"))?.step(units)?;
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
                // Release all integrator scratch before shrinking the shared lease
                // to the completed PNG. Retained task metadata also owns memory.
                self.task = None;
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
                terminal(&self.handle, JobError::new(ErrorCode::Internal, "CPU image host encountered an internal error"));
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
pub struct CpuHost(Arc<Owner>);

impl CpuHost {
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
        Self(Arc::new(Owner {
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
            let shared = Self::shared();
            let worker_shared = shared.clone();
            let worker = std::thread::Builder::new()
                .name("solidcraft-render".into())
                .spawn(move || native_worker(worker_shared))
                .map_err(|e| JobError::new(ErrorCode::HostUnavailable, format!("could not start CPU image worker: {e}")))?;
            Ok(Self(Arc::new(Owner { shared, kind: HostKind::NativeWorker, running: Mutex::new(None), _worker: Some(worker) })))
        }
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
        self.descriptor_for(Integrator::Raster)
    }

    /// Check the execution choice without capturing geometry or reserving memory.
    pub fn validate_integrator(&self, integrator: Integrator) -> Result<()> {
        if !self.is_available() {
            return Err(JobError::new(ErrorCode::HostUnavailable, "CPU image host has stopped"));
        }
        if integrator == Integrator::Raytrace && !cfg!(feature = "raytrace-cpu") {
            return Err(JobError::new(ErrorCode::HostUnavailable, "CPU raytracing is not compiled in; enable raytrace-cpu"));
        }
        if integrator == Integrator::Raytrace && self.kind() != HostKind::NativeWorker {
            return Err(JobError::new(ErrorCode::HostUnavailable, "CPU raytracing requires the native worker host"));
        }
        Ok(())
    }

    /// Select exactly one executor for this request. Never rely on registry order
    /// to choose between raster and raytracing implementations.
    pub fn descriptor_for(&self, integrator: Integrator) -> Result<ExecutorDescriptor> {
        if integrator == Integrator::Raytrace && !cfg!(feature = "raytrace-cpu") {
            return Err(JobError::new(ErrorCode::HostUnavailable, "CPU raytracing is not compiled in; enable raytrace-cpu"));
        }
        Ok(ExecutorDescriptor {
            id: ExecutorId::new(match integrator {
                Integrator::Raster => "cpu-product",
                Integrator::Raytrace => "cpu-raytrace",
            })
            .map_err(|e| JobError::new(ErrorCode::Internal, e.to_string()))?,
            availability: if self.0.shared.stop.load(Ordering::Acquire) {
                Availability::Unavailable { reason: "CPU image host has stopped".into() }
            } else if integrator == Integrator::Raytrace && self.kind() != HostKind::NativeWorker {
                Availability::Unavailable { reason: "CPU raytracing requires the native worker host".into() }
            } else {
                Availability::Available
            },
            profiles: vec![RenderProfile {
                mode: RenderMode::Preview,
                operation: RenderOperation::Image,
                formats: vec![RenderFormat::Png],
                features: if integrator == Integrator::Raytrace && cfg!(feature = "raytrace-cpu") && self.kind() == HostKind::NativeWorker {
                    vec![crate::RenderFeature::Pbr, crate::RenderFeature::Hdri]
                } else {
                    vec![crate::RenderFeature::Pbr]
                },
            }],
            limits: RenderLimits::default(),
            scratch: ScratchLayout { sample_scale: 2, sample_bytes_per_pixel: 8, output_bytes_per_pixel: 8, encoding_bytes_per_pixel: 8 },
        })
    }

    pub fn submit(&self, id: JobId, scene_generation: u64, scene: ProductScene, request: ImageRequest) -> Result<JobHandle> {
        self.submit_with_environment(id, scene_generation, scene, request, None)
    }

    /// Captured HDR ownership stays in work, never serialized/cloned into status.
    pub fn submit_with_environment(
        &self,
        id: JobId,
        scene_generation: u64,
        scene: ProductScene,
        request: ImageRequest,
        environment: Option<crate::lighting::environment_input::EnvironmentInput>,
    ) -> Result<JobHandle> {
        if environment.is_some() && request.integrator != Integrator::Raytrace {
            return Err(JobError::new(ErrorCode::InvalidRequest, "environment lighting requires CPU raytrace"));
        }
        self.validate_integrator(request.integrator)?;
        if environment.is_some() {
            request.raytrace.as_ref().unwrap_or(&super::RaytraceOptions::default()).validate_environment_reflections(&scene)?;
        }
        if id.session == 0 || id.serial == 0 || id.generation == 0 {
            return Err(JobError::new(ErrorCode::InvalidRequest, "invalid render job identity"));
        }
        let environment_info = environment.as_ref().map(|e| e.metadata());
        let metadata_bytes =
            if environment_info.is_some() { std::mem::size_of::<crate::lighting::environment_input::EnvironmentInfo>() as u64 } else { 0 };
        let request_bytes = retained_request_bytes(&request)?
            .checked_add(metadata_bytes)
            .ok_or_else(|| JobError::new(ErrorCode::LimitExceeded, "environment metadata overflow"))?;
        let mut features = scene.required_features();
        if environment.is_some() {
            features.push(crate::RenderFeature::Hdri);
        }
        let Admission { estimated_total_bytes, .. } = admit(&scene, &request)?;
        let environment_bytes =
            environment.as_ref().map_or(Ok(0), |e| e.required_bytes()).map_err(|e| JobError::new(ErrorCode::InvalidRequest, e.to_string()))?;
        let estimated_total_bytes = estimated_total_bytes
            .checked_add(environment_bytes)
            .ok_or_else(|| JobError::new(ErrorCode::LimitExceeded, "environment admission overflow"))?;
        admit_request_sum(estimated_total_bytes, request_bytes)?;
        let mut backend = crate::builtin_backend();
        let descriptor = self.descriptor_for(request.integrator)?;
        let expected_executor = descriptor.id.clone();
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
                features,
                policy: Default::default(),
            },
        )
        .map_err(|e| JobError::new(ErrorCode::HostUnavailable, e.to_string()))?;
        if resolved.executor != expected_executor {
            return Err(JobError::new(ErrorCode::HostUnavailable, "CPU product executor is unavailable"));
        }
        let mut queue = self.0.shared.queue.lock().map_err(|_| JobError::new(ErrorCode::HostUnavailable, "render queue lock poisoned"))?;
        if self.0.shared.stop.load(Ordering::Acquire) {
            return Err(JobError::new(ErrorCode::HostUnavailable, "CPU image host has stopped"));
        }
        queue.retain(|work| !work.handle.0.cancel.is_cancelled());
        if queue.len() >= QUEUE_LIMIT {
            return Err(JobError::new(ErrorCode::QueueFull, "CPU image queue is full"));
        }
        admit_request_sum(estimated_total_bytes, request_bytes)?;
        let reservation = self.0.shared.budget.reserve(estimated_total_bytes)?;
        let request_reservation = if request_bytes == 0 { None } else { Some(self.0.shared.budget.reserve(request_bytes)?) };
        let handle = JobHandle(Arc::new(Record {
            environment: environment_info,
            _request_reservation: request_reservation,
            id,
            scene_generation,
            request,
            resolved,
            cancel: CancelToken::default(),
            data: Mutex::new(Data { progress: Progress { phase: Phase::Queued, done: 0, total: 1 }, error: None, frame: None }),
            #[cfg(not(target_arch = "wasm32"))]
            done: Condvar::new(),
        }));
        queue.push_back(Work { scene, handle: handle.clone(), reservation, environment });
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
#[cfg(test)]
mod retained_request_tests {
    use super::*;
    fn request() -> ImageRequest {
        let mut lights = Vec::with_capacity(8);
        lights.push(super::super::DirectionalLightOptions { direction: [0.0, 0.0, 1.0], color: [1.0; 3], intensity: 1.0 });
        ImageRequest {
            integrator: Integrator::Raytrace,
            raytrace: Some(super::super::RaytraceOptions { lights: Some(lights), ..Default::default() }),
            camera: Default::default(),
            width: 8,
            height: 8,
            background: [0; 3],
            transparent_background: false,
        }
    }
    #[test]
    fn retained_lights_are_billed_by_original_capacity_and_sum_is_checked() {
        let request = request();
        let bytes = retained_request_bytes(&request).unwrap();
        assert_eq!(bytes, 8 * std::mem::size_of::<super::super::DirectionalLightOptions>() as u64);
        let cap = super::super::image::MAX_JOB_BYTES;
        assert!(admit_request_sum(cap - bytes, bytes).is_ok());
        assert_eq!(admit_request_sum(cap - bytes + 1, bytes).unwrap_err().code, ErrorCode::LimitExceeded);
        assert_eq!(admit_request_sum(u64::MAX, 1).unwrap_err().code, ErrorCode::LimitExceeded);
    }
    #[cfg(all(feature = "raytrace-cpu", not(target_arch = "wasm32")))]
    #[test]
    fn retained_lights_keep_the_same_pool_lease_after_png_eviction() {
        let host = CpuHost::for_platform().unwrap();
        let request = request();
        let expected = retained_request_bytes(&request).unwrap();
        let job = host
            .submit(
                JobId { session: 99, serial: 1, generation: 1 },
                1,
                ProductScene { meshes: Vec::new(), bounds: solidcraft_geom::Aabb3::EMPTY },
                request,
            )
            .unwrap();
        assert_eq!(job.wait_timeout(std::time::Duration::from_secs(60)).unwrap().progress.phase, Phase::Completed);
        let frame = job.frame().unwrap().unwrap();
        let held_bytes = host.admitted_bytes();
        assert!(held_bytes > expected);
        job.0.data.lock().unwrap().frame = None;
        assert_eq!(host.admitted_bytes(), held_bytes);
        drop(frame);
        assert_eq!(host.admitted_bytes(), expected);
        let held = job.clone();
        drop(job);
        assert_eq!(host.admitted_bytes(), expected);
        drop(held);
    }
}

#[cfg(all(test, feature = "raytrace-cpu", not(target_arch = "wasm32")))]
mod environment_metadata_lease_tests {
    use super::*;
    #[test]
    fn captured_environment_metadata_survives_png_eviction_without_source_bytes() {
        use crate::lighting::{
            HdrSource,
            environment_input::{EnvironmentInfo, EnvironmentInput},
        };
        let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nPRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n\n-Y 2 +X 4\n".to_vec();
        for _ in 0..8 {
            bytes.extend_from_slice(&[128, 64, 32, 129]);
        }
        let input = EnvironmentInput::new(HdrSource::new(bytes).unwrap(), 0.5, 2.0, 16).unwrap();
        let metadata = input.metadata();
        let expected = std::mem::size_of::<EnvironmentInfo>() as u64;
        let host = CpuHost::for_platform().unwrap();
        let budget = host.0.shared.budget.clone();
        let request = ImageRequest {
            integrator: Integrator::Raytrace,
            raytrace: None,
            camera: Default::default(),
            width: 8,
            height: 8,
            background: [0; 3],
            transparent_background: false,
        };
        let job = host
            .submit_with_environment(
                JobId { session: 1693, serial: 7, generation: 1 },
                1,
                ProductScene { meshes: Vec::new(), bounds: solidcraft_geom::Aabb3::EMPTY },
                request,
                Some(input),
            )
            .unwrap();
        let status = job.wait_timeout(std::time::Duration::from_secs(60)).unwrap();
        assert_eq!(status.progress.phase, Phase::Completed);
        assert_eq!(status.environment.unwrap().source_sha256, metadata.source_sha256);
        let frame = job.frame().unwrap().unwrap();
        job.0.data.lock().unwrap().frame = None;
        assert_eq!(host.admitted_bytes(), frame._reservation.bytes + expected);
        drop(frame);
        assert_eq!(host.admitted_bytes(), expected);
        drop(host);
        drop(job);
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
    }
}
