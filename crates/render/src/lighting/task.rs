use super::{HdrError, HdrErrorCode, HdrInfo, HdrSource, MAX_HDR_PIXELS, Result, check_cancelled, hdr::Rows};

#[cfg(test)]
#[path = "environment_input_tests.rs"]
mod environment_input_tests;

/// Linear RGB in the source's colour space, without gamut conversion or tone mapping.
pub struct DecodedHdr {
    info: HdrInfo,
    pixels: Vec<[f32; 3]>,
}
impl DecodedHdr {
    pub fn info(&self) -> &HdrInfo {
        &self.info
    }
    pub fn pixels(&self) -> &[[f32; 3]] {
        &self.pixels
    }
    /// Original allocation capacity, including spare retained slots, for caller admission.
    pub fn retained_pixel_bytes(&self) -> Result<u64> {
        (self.pixels.capacity() as u64).checked_mul(12).ok_or_else(|| HdrError::new(HdrErrorCode::LimitExceeded, "decoded HDR capacity overflow"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HdrProgress {
    pub decoded_pixels: u64,
    pub total_pixels: u64,
}
pub enum HdrTaskPoll {
    Pending(HdrProgress),
    Complete(DecodedHdr),
}

/// Caller-owned cooperative preparation; no scheduler, GPU or document dependency.
pub struct HdrDecodeTask {
    rows: Option<Rows>,
    pixels: Vec<[f32; 3]>,
    x: usize,
    work: u64,
}
impl HdrDecodeTask {
    pub fn new(source: HdrSource) -> Result<Self> {
        let count = u64::from(source.info().width) * u64::from(source.info().height);
        let count = usize::try_from(count).map_err(|_| HdrError::new(HdrErrorCode::LimitExceeded, "HDR pixel count overflow"))?;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(count).map_err(|_| HdrError::new(HdrErrorCode::AllocationFailed, "could not reserve HDR pixels"))?;
        if pixels.capacity() != count {
            return Err(HdrError::new(HdrErrorCode::LimitExceeded, "HDR decoded capacity exceeds admission"));
        }
        Ok(Self { rows: Some(Rows::new(source)?), pixels, x: 0, work: 0 })
    }

    /// Each unit consumes one RLE packet (at most 128 bytes) or one RGB pixel.
    /// Error/cancellation consumes the task and releases its partial buffers.
    pub fn step(&mut self, units: usize, cancelled: impl Fn() -> bool) -> Result<HdrTaskPoll> {
        let result = self.advance(units, cancelled);
        if result.is_err() {
            self.rows = None;
            self.pixels = Vec::new();
        }
        result
    }
    fn advance(&mut self, units: usize, cancelled: impl Fn() -> bool) -> Result<HdrTaskPoll> {
        let rows = self.rows.as_mut().ok_or_else(|| HdrError::new(HdrErrorCode::Consumed, "HDR task already consumed"))?;
        for _ in 0..units.clamp(1, 16384) {
            check_cancelled(&cancelled)?;
            self.work = self
                .work
                .checked_add(1)
                .filter(|n| *n <= MAX_HDR_PIXELS * 6 + 4096)
                .ok_or_else(|| HdrError::new(HdrErrorCode::LimitExceeded, "HDR decode work budget exceeded"))?;
            if rows.ready() {
                self.pixels.push(rows.linear(self.x)?);
                self.x += 1;
                if self.x == rows.info().width as usize {
                    rows.next_row()?;
                    self.x = 0;
                }
            } else {
                rows.step()?;
            }
            if rows.done() {
                check_cancelled(&cancelled)?;
                let info = rows.info().clone();
                self.rows = None;
                return Ok(HdrTaskPoll::Complete(DecodedHdr { info, pixels: std::mem::take(&mut self.pixels) }));
            }
        }
        Ok(HdrTaskPoll::Pending(HdrProgress {
            decoded_pixels: self.pixels.len() as u64,
            total_pixels: u64::from(rows.info().width) * u64::from(rows.info().height),
        }))
    }
}
