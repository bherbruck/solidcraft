//! Device-independent, bounded linear HDR input preparation.

pub mod environment;
mod hdr;
mod task;
pub use hdr::{HdrInfo, HdrSource, MAX_HDR_BYTES, MAX_HDR_HEIGHT, MAX_HDR_PIXELS, MAX_HDR_WIDTH};
pub use task::{DecodedHdr, HdrDecodeTask, HdrProgress, HdrTaskPoll};

use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HdrErrorCode {
    InvalidInput,
    LimitExceeded,
    AllocationFailed,
    Cancelled,
    Consumed,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HdrError {
    pub code: HdrErrorCode,
    pub message: String,
}

impl HdrError {
    pub fn new(code: HdrErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}
impl fmt::Display for HdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for HdrError {}
pub type Result<T> = std::result::Result<T, HdrError>;

fn check_cancelled(cancelled: &impl Fn() -> bool) -> Result<()> {
    if cancelled() { Err(HdrError::new(HdrErrorCode::Cancelled, "HDR decoding cancelled")) } else { Ok(()) }
}
