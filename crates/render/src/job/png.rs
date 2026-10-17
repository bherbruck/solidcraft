//! Bounded PNG encoding shared by native image executors.
use super::{CancelToken, ErrorCode, ImageRequest, ImageWork, JobError, Phase, Progress, Result, TaskPoll};
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

struct Output {
    bytes: Arc<Mutex<Vec<u8>>>,
    limit: usize,
    cancel: CancelToken,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::other("render cancelled"));
        }
        let mut out = self.bytes.lock().map_err(|_| std::io::Error::other("PNG lock poisoned"))?;
        if out.len().checked_add(bytes.len()).is_none_or(|n| n > self.limit) {
            return Err(std::io::Error::other("PNG byte limit exceeded"));
        }
        out.try_reserve_exact(bytes.len()).map_err(|_| std::io::Error::other("PNG allocation failed"))?;
        out.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Owns admitted RGBA bytes and yields after at most four scanlines.
pub struct PngTask {
    rgba: Vec<u8>,
    row: usize,
    width: usize,
    height: usize,
    bytes: Arc<Mutex<Vec<u8>>>,
    writer: Option<png::StreamWriter<'static, Output>>,
    cancel: CancelToken,
}
impl PngTask {
    pub fn new(request: &ImageRequest, rgba: Vec<u8>, cancel: CancelToken) -> Result<Self> {
        let pixels = u64::from(request.width) * u64::from(request.height);
        if !(1..=4096).contains(&request.width) || !(1..=4096).contains(&request.height) || pixels > 4_194_304 || rgba.len() as u64 != pixels * 4 {
            return Err(JobError::new(ErrorCode::InvalidRequest, "invalid PNG dimensions or RGBA length"));
        }
        cancel.check()?;
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let output = Output { bytes: bytes.clone(), limit: (pixels * 8 + 65_536) as usize, cancel: cancel.clone() };
        let mut encoder = png::Encoder::new(output, request.width, request.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let writer = encoder.write_header().and_then(|w| w.into_stream_writer()).map_err(encoding)?;
        Ok(Self { rgba, row: 0, width: request.width as usize, height: request.height as usize, bytes, writer: Some(writer), cancel })
    }
}
fn encoding(e: impl std::fmt::Display) -> JobError {
    JobError::new(ErrorCode::EncodingFailed, e.to_string())
}
impl ImageWork for PngTask {
    fn step(&mut self, _units: usize) -> Result<TaskPoll> {
        self.cancel.check()?;
        for _ in 0..4 {
            if self.row == self.height {
                break;
            }
            let start = self.row * self.width * 4;
            let row = self.rgba.get(start..start + self.width * 4).ok_or_else(|| encoding("missing PNG row"))?;
            self.writer.as_mut().ok_or_else(|| encoding("PNG already completed"))?.write_all(row).map_err(encoding)?;
            self.row += 1;
        }
        if self.row != self.height {
            return Ok(TaskPoll::Pending(Progress { phase: Phase::Encoding, done: self.row as u64, total: self.height as u64 }));
        }
        self.writer.take().ok_or_else(|| encoding("PNG already completed"))?.finish().map_err(encoding)?;
        self.cancel.check()?;
        let mut bytes = self.bytes.lock().map_err(|_| encoding("PNG lock poisoned"))?;
        Ok(TaskPoll::Complete(std::mem::take(&mut *bytes)))
    }
}
