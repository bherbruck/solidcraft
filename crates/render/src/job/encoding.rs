//! Shared bounded, cancellable sRGB PNG row encoder for CPU image integrators.
use super::{CancelToken, ErrorCode, JobError, Result};
use std::io::Write;
use std::sync::{Arc, Mutex};

struct BufferWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
    limit: usize,
    cancel: CancelToken,
}

impl Write for BufferWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::other("render cancelled"));
        }
        let mut out = self.bytes.lock().map_err(|_| std::io::Error::other("PNG output lock poisoned"))?;
        if out.len().checked_add(bytes.len()).is_none_or(|n| n > self.limit) {
            return Err(std::io::Error::other("PNG output exceeds its byte budget"));
        }
        out.try_reserve_exact(bytes.len()).map_err(|_| std::io::Error::other("PNG allocation failed"))?;
        out.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.cancel.is_cancelled() { Err(std::io::Error::other("render cancelled")) } else { Ok(()) }
    }
}

pub(crate) struct PngEncoding {
    bytes: Arc<Mutex<Vec<u8>>>,
    writer: Option<png::StreamWriter<'static, BufferWriter>>,
    width: usize,
    height: usize,
    row: usize,
    cancel: CancelToken,
}

impl PngEncoding {
    pub(crate) fn new(width: u32, height: u32, limit: u64, cancel: CancelToken) -> Result<Self> {
        cancel.check()?;
        let limit = usize::try_from(limit).map_err(|_| JobError::new(ErrorCode::LimitExceeded, "PNG byte budget exceeds addressable memory"))?;
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = BufferWriter { bytes: bytes.clone(), limit, cancel: cancel.clone() };
        let mut encoder = png::Encoder::new(writer, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let writer =
            encoder.write_header().and_then(|w| w.into_stream_writer()).map_err(|e| JobError::new(ErrorCode::EncodingFailed, e.to_string()))?;
        Ok(Self { bytes, writer: Some(writer), width: width as usize, height: height as usize, row: 0, cancel })
    }

    pub(crate) fn row(&self) -> usize {
        self.row
    }

    /// Writes at most one row; callers retain the four-row-per-quantum limit.
    pub(crate) fn step(&mut self, rgba: &[u8]) -> Result<Option<Vec<u8>>> {
        self.cancel.check()?;
        if self.row < self.height {
            let length = self.width.checked_mul(4).ok_or_else(|| JobError::new(ErrorCode::LimitExceeded, "PNG row overflow"))?;
            let start = self.row.checked_mul(length).ok_or_else(|| JobError::new(ErrorCode::LimitExceeded, "PNG row offset overflow"))?;
            let end = start.checked_add(length).ok_or_else(|| JobError::new(ErrorCode::LimitExceeded, "PNG row end overflow"))?;
            let row = rgba.get(start..end).ok_or_else(|| JobError::new(ErrorCode::InvalidScene, "missing PNG row"))?;
            self.writer
                .as_mut()
                .ok_or_else(|| JobError::new(ErrorCode::Internal, "missing PNG writer"))?
                .write_all(row)
                .map_err(|e| JobError::new(ErrorCode::EncodingFailed, e.to_string()))?;
            self.row += 1;
            return Ok(None);
        }
        self.writer
            .take()
            .ok_or_else(|| JobError::new(ErrorCode::Internal, "missing PNG writer"))?
            .finish()
            .map_err(|e| JobError::new(ErrorCode::EncodingFailed, e.to_string()))?;
        self.cancel.check()?;
        let mut bytes = self.bytes.lock().map_err(|_| JobError::new(ErrorCode::Internal, "PNG output lock poisoned"))?;
        let png = std::mem::take(&mut *bytes);
        if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(JobError::new(ErrorCode::EncodingFailed, "invalid PNG output"));
        }
        Ok(Some(png))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_roundtrip_and_cancel_before_finish_never_returns_png() {
        let rgba = [20, 40, 60, 255, 80, 100, 120, 255];
        let cancel = CancelToken::default();
        let mut encoder = PngEncoding::new(1, 2, 65536, cancel.clone()).unwrap();
        assert!(encoder.step(&rgba).unwrap().is_none());
        assert_eq!(encoder.row(), 1);
        assert!(encoder.step(&rgba).unwrap().is_none());
        assert_eq!(encoder.row(), 2);
        cancel.cancel();
        assert_eq!(encoder.step(&rgba).unwrap_err().code, ErrorCode::Cancelled);

        let mut encoder = PngEncoding::new(1, 2, 65536, CancelToken::default()).unwrap();
        assert!(encoder.step(&rgba).unwrap().is_none());
        assert!(encoder.step(&rgba).unwrap().is_none());
        let png = encoder.step(&rgba).unwrap().unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (1, 2));
        assert_eq!(decoded.as_raw(), &rgba);
    }

    #[test]
    fn encoded_byte_budget_and_missing_rows_are_errors() {
        assert!(PngEncoding::new(1, 1, 1, CancelToken::default()).is_err());
        let mut encoder = PngEncoding::new(1, 1, 65536, CancelToken::default()).unwrap();
        assert_eq!(encoder.step(&[]).unwrap_err().code, ErrorCode::InvalidScene);
    }
    #[test]
    fn png_output_budget_and_cancellation_fail_without_partial_publication() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancelToken::default();
        let mut writer = BufferWriter { bytes: bytes.clone(), limit: 4, cancel: cancel.clone() };
        writer.write_all(b"PNG").unwrap();
        assert!(writer.write_all(b"too large").is_err());
        assert_eq!(*bytes.lock().unwrap(), b"PNG");
        cancel.cancel();
        assert!(writer.write_all(b"x").is_err());
        assert!(writer.flush().is_err());
        assert_eq!(*bytes.lock().unwrap(), b"PNG");
    }
}
