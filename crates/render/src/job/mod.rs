//! Product-image work and its device-independent, session-scoped protocol.
//! Hosts own execution; documents and history never contain these values.

use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{BackendId, Camera, ExecutorId};

mod host;
mod image;
mod png;
pub use host::{CpuHost, HostKind, ImageHost, JobHandle, JobStatus, RenderFrame};
pub use image::{Admission, ImageTask, TaskPoll, admit, admit_geometry};
pub use png::PngTask;

/// Runtime sampling intent, separate from saved documents and device resources.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JobOptions {
    pub samples: u32,
    pub max_depth: u32,
    pub seed: u32,
    pub exposure_ev: f64,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self { samples: 64, max_depth: 8, seed: 0, exposure_ev: 0.0 }
    }
}

impl JobOptions {
    pub fn validate(&self) -> Result<()> {
        if !(1..=4096).contains(&self.samples)
            || !(1..=16).contains(&self.max_depth)
            || !self.exposure_ev.is_finite()
            || !(-32.0..=32.0).contains(&self.exposure_ev)
        {
            return Err(JobError::new(ErrorCode::InvalidRequest, "samples must be 1..4096, depth 1..16 and exposure -32..32 EV"));
        }
        Ok(())
    }
}

/// One bounded quantum of image work. Implementations check cancellation between quanta.
pub trait ImageWork: Send {
    fn step(&mut self, units: usize) -> Result<TaskPoll>;
}
impl ImageWork for ImageTask {
    fn step(&mut self, units: usize) -> Result<TaskPoll> {
        self.step(units)
    }
}

/// Device-independent executor boundary. The host owns admission leases and publication;
/// an executor owns computation only. GPU implementations live above the render crate.
pub trait ImageExecutor: Send + Sync {
    fn descriptor(&self) -> Result<crate::ExecutorDescriptor>;
    fn admit(&self, scene: &crate::ProductScene, request: &ImageRequest, options: JobOptions) -> Result<Admission>;
    fn begin(&self, scene: crate::ProductScene, request: ImageRequest, options: JobOptions, cancel: CancelToken) -> Result<Box<dyn ImageWork>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobId {
    pub session: u64,
    pub serial: u64,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Queued,
    Preparing,
    Running,
    Encoding,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
}

impl Phase {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Completed | Self::Failed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    LimitExceeded,
    AllocationFailed,
    HostUnavailable,
    QueueFull,
    TimedOut,
    Cancelled,
    InvalidScene,
    EncodingFailed,
    WrongOwner,
    UnknownJob,
    NotCompleted,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JobError {
    pub code: ErrorCode,
    pub message: String,
}

impl JobError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for JobError {}

pub type Result<T> = std::result::Result<T, JobError>;

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() { Err(JobError::new(ErrorCode::Cancelled, "render cancelled")) } else { Ok(()) }
    }
}

/// Fully resolved input; no mutable session, topology graph or GPU resources.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRequest {
    pub camera: Camera,
    pub width: u32,
    pub height: u32,
    pub background: [u8; 3],
    pub transparent_background: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColourEncoding {
    Srgb,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaConvention {
    Straight,
}

#[derive(Clone, Debug, Serialize)]
pub struct FrameInfo {
    pub job: JobId,
    pub scene_generation: u64,
    pub backend: BackendId,
    pub executor: ExecutorId,
    pub width: u32,
    pub height: u32,
    pub encoding: ColourEncoding,
    pub alpha: AlphaConvention,
}
