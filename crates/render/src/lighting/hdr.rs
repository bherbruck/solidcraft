//! Incremental RGBE scanline adapter for image 0.25.10's Radiance codec.
//! Row/RLE handling is adapted from its `read_scanline`, `decode_component` and
//! `decode_old_rle` (MIT OR Apache-2.0; see ATTRIBUTION.md and NOTICE). Metadata still
//! uses the library. Changes add cooperative steps, checked runs, width validation
//! and bounded radiance; no third-party asserts or input indexing are retained.

use super::{HdrError, HdrErrorCode as ErrorCode, Result, check_cancelled};
use serde::Serialize;
use std::{io::Cursor, sync::Arc};

pub const MAX_HDR_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_HDR_WIDTH: u32 = 2048;
pub const MAX_HDR_HEIGHT: u32 = 1024;
pub const MAX_HDR_PIXELS: u64 = 2 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_RADIANCE: f64 = 1e8;

fn invalid(message: impl Into<String>) -> HdrError {
    HdrError::new(ErrorCode::InvalidInput, message)
}
fn limit(message: impl Into<String>) -> HdrError {
    HdrError::new(ErrorCode::LimitExceeded, message)
}

#[derive(Clone, Debug, Serialize)]
pub struct HdrInfo {
    pub width: u32,
    pub height: u32,
    pub encoded_bytes: u64,
    pub decoded_rgb_bytes: u64,
    /// Stored RGBE values are divided by these declared header corrections.
    pub exposure: f64,
    pub color_correction: [f64; 3],
    /// Source chromaticities (Rx Ry Gx Gy Bx By Wx Wy); absent means unspecified.
    pub primaries: Option<[f64; 8]>,
    #[serde(skip)]
    offset: usize,
}

/// Immutable input. Construction only inspects a bounded header; `validate` also checks
/// every encoded row and corrected radiance before an import commits it to a document.
#[derive(Clone)]
pub struct HdrSource {
    bytes: Arc<Vec<u8>>,
    info: HdrInfo,
}

impl HdrSource {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_HDR_BYTES || bytes.capacity() > MAX_HDR_BYTES {
            return Err(limit("HDR input must contain at most 16 MiB"));
        }
        let mut cursor = Cursor::new(bytes.get(..bytes.len().min(MAX_HEADER_BYTES)).ok_or_else(|| invalid("invalid HDR header range"))?);
        let metadata = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            image::codecs::hdr::HdrDecoder::new(&mut cursor).map(|decoder| decoder.metadata())
        }))
        .map_err(|_| invalid("HDR metadata decoder failed"))?
        .map_err(|e| invalid(format!("invalid or oversized Radiance header: {e}")))?;
        let pixels = u64::from(metadata.width).checked_mul(u64::from(metadata.height)).ok_or_else(|| limit("HDR dimensions overflow"))?;
        if metadata.width == 0
            || metadata.height == 0
            || metadata.width > MAX_HDR_WIDTH
            || metadata.height > MAX_HDR_HEIGHT
            || pixels > MAX_HDR_PIXELS
        {
            return Err(limit("HDR preview input exceeds 2048x1024 / 2 million pixels"));
        }
        if metadata.width != metadata.height.saturating_mul(2) || metadata.orientation != ((1, 0), (0, 1)) {
            return Err(invalid("HDR preview requires a 2:1 equirectangular image with standard -Y +X orientation"));
        }
        let exposure = f64::from(metadata.exposure.unwrap_or(1.0));
        let (r, g, b) = metadata.color_correction.unwrap_or((1.0, 1.0, 1.0));
        let correction = [f64::from(r), f64::from(g), f64::from(b)];
        if [exposure].into_iter().chain(correction).any(|x| !x.is_finite() || x <= 0.0)
            || metadata.pixel_aspect_ratio.is_some_and(|x| !x.is_finite() || (x - 1.0).abs() > 1e-6)
        {
            return Err(invalid("HDR exposure, colour correction and square-pixel metadata must be finite and positive"));
        }
        let offset = usize::try_from(cursor.position()).map_err(|_| invalid("HDR data offset overflow"))?;
        if offset.checked_add(4).is_none_or(|end| end > bytes.len()) {
            return Err(invalid("HDR input has no pixel data"));
        }
        let mut primaries = None;
        for (key, value) in &metadata.custom_attributes {
            if key == "PRIMARIES" {
                if primaries.is_some() {
                    return Err(invalid("duplicate HDR primaries"));
                }
                let mut parsed = [0.0; 8];
                let mut values = value.split_whitespace();
                for out in &mut parsed {
                    *out = values
                        .next()
                        .ok_or_else(|| invalid("missing HDR chromaticity"))?
                        .parse::<f64>()
                        .map_err(|_| invalid("invalid HDR chromaticity"))?;
                    if !out.is_finite() || !(0.0..=1.0).contains(out) {
                        return Err(invalid("invalid HDR chromaticity"));
                    }
                }
                if values.next().is_some() {
                    return Err(invalid("extra HDR chromaticity"));
                }
                primaries = Some(parsed);
            }
        }
        let info = HdrInfo {
            width: metadata.width,
            height: metadata.height,
            encoded_bytes: bytes.len() as u64,
            decoded_rgb_bytes: pixels.checked_mul(12).ok_or_else(|| limit("HDR decoded size overflow"))?,
            exposure,
            color_correction: correction,
            primaries,
            offset,
        };
        Ok(Self { bytes: Arc::new(bytes), info })
    }
    pub fn info(&self) -> &HdrInfo {
        &self.info
    }
    pub fn encoded_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn retained_encoded_bytes(&self) -> u64 {
        self.bytes.capacity() as u64
    }
    pub fn validate(&self, cancelled: impl Fn() -> bool) -> Result<()> {
        check_cancelled(&cancelled)?;
        let mut rows = Rows::new(self.clone())?;
        let mut steps = 0u64;
        while !rows.done() {
            check_cancelled(&cancelled)?;
            if rows.ready() {
                for x in 0..self.info.width as usize {
                    check_cancelled(&cancelled)?;
                    rows.linear(x)?;
                }
                rows.next_row()?;
            } else {
                rows.step()?;
            }
            steps = steps.checked_add(1).ok_or_else(|| limit("HDR decode work overflow"))?;
            if steps > MAX_HDR_PIXELS * 6 + 4096 {
                return Err(limit("HDR decode work budget exceeded"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "hdr_tests.rs"]
mod tests;

enum Stage {
    Start,
    Channel { channel: usize, x: usize },
    Old { x: usize, previous: [u8; 4], shift: u32, repeat: usize },
    Ready,
    Done,
}

pub(super) struct Rows {
    source: HdrSource,
    offset: usize,
    row: u32,
    pixels: Vec<[u8; 4]>,
    scales: [f64; 256],
    stage: Stage,
}

impl Rows {
    pub(super) fn new(source: HdrSource) -> Result<Self> {
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(source.info.width as usize)
            .map_err(|_| HdrError::new(ErrorCode::AllocationFailed, "could not reserve HDR scanline"))?;
        if pixels.capacity() != source.info.width as usize {
            return Err(limit("HDR scanline capacity exceeds admission"));
        }
        pixels.resize(source.info.width as usize, [0; 4]);
        Ok(Self {
            offset: source.info.offset,
            source,
            row: 0,
            pixels,
            scales: std::array::from_fn(|e| 2f64.powi(e as i32 - 136)),
            stage: Stage::Start,
        })
    }
    pub(super) fn info(&self) -> &HdrInfo {
        &self.source.info
    }
    #[cfg(test)]
    pub(super) fn row(&self) -> u32 {
        self.row
    }
    pub(super) fn ready(&self) -> bool {
        matches!(self.stage, Stage::Ready)
    }
    pub(super) fn done(&self) -> bool {
        matches!(self.stage, Stage::Done)
    }
    fn byte(&mut self) -> Result<u8> {
        let b = self.source.bytes.get(self.offset).copied().ok_or_else(|| invalid("truncated HDR scanline"))?;
        self.offset = self.offset.checked_add(1).ok_or_else(|| invalid("HDR offset overflow"))?;
        Ok(b)
    }
    fn pixel(&mut self) -> Result<[u8; 4]> {
        Ok([self.byte()?, self.byte()?, self.byte()?, self.byte()?])
    }
    fn put(&mut self, x: usize, pixel: [u8; 4]) -> Result<()> {
        *self.pixels.get_mut(x).ok_or_else(|| invalid("HDR run exceeds its row"))? = pixel;
        Ok(())
    }
    /// One packet (at most 128 component writes), or one old-style repeated pixel.
    pub(super) fn step(&mut self) -> Result<()> {
        let width = self.pixels.len();
        let stage = std::mem::replace(&mut self.stage, Stage::Done);
        self.stage = match stage {
            Stage::Start => {
                let first = self.pixel()?;
                if first.get(..2) == Some(&[2, 2]) && first.get(2).is_some_and(|v| *v < 128) {
                    let declared = usize::from(*first.get(2).ok_or_else(|| invalid("missing HDR width"))?) * 256
                        + usize::from(*first.get(3).ok_or_else(|| invalid("missing HDR width"))?);
                    if declared != width {
                        return Err(invalid("HDR scanline width differs from its header"));
                    }
                    Stage::Channel { channel: 0, x: 0 }
                } else {
                    if first.get(..3) == Some(&[1, 1, 1]) {
                        return Err(invalid("HDR row starts with an old run marker"));
                    }
                    self.put(0, first)?;
                    if width == 1 { Stage::Ready } else { Stage::Old { x: 1, previous: first, shift: 0, repeat: 0 } }
                }
            }
            Stage::Channel { channel, x } => {
                let code = self.byte()?;
                if code == 0 {
                    return Err(invalid("zero-length HDR RLE packet"));
                }
                let run = code > 128;
                let count = usize::from(if run { code - 128 } else { code });
                let end = x.checked_add(count).filter(|end| *end <= width).ok_or_else(|| invalid("HDR RLE packet exceeds its row"))?;
                let repeated = if run { self.byte()? } else { 0 };
                for at in x..end {
                    let value = if run { repeated } else { self.byte()? };
                    *self.pixels.get_mut(at).and_then(|p| p.get_mut(channel)).ok_or_else(|| invalid("invalid HDR component address"))? = value;
                }
                if end == width {
                    if channel == 3 { Stage::Ready } else { Stage::Channel { channel: channel + 1, x: 0 } }
                } else {
                    Stage::Channel { channel, x: end }
                }
            }
            Stage::Old { x, previous, shift, repeat } => {
                if repeat > 0 {
                    self.put(x, previous)?;
                    if x + 1 == width { Stage::Ready } else { Stage::Old { x: x + 1, previous, shift, repeat: repeat - 1 } }
                } else {
                    let pixel = self.pixel()?;
                    if pixel.get(..3) == Some(&[1, 1, 1]) {
                        let count = u64::from(*pixel.get(3).ok_or_else(|| invalid("missing HDR repeat count"))?)
                            .checked_shl(shift)
                            .filter(|n| *n <= (width - x) as u64)
                            .ok_or_else(|| invalid("old HDR run exceeds its row"))?;
                        let next_shift = shift.checked_add(8).filter(|s| *s <= 32).ok_or_else(|| invalid("old HDR run multiplier overflow"))?;
                        Stage::Old { x, previous, shift: next_shift, repeat: count as usize }
                    } else {
                        self.put(x, pixel)?;
                        if x + 1 == width { Stage::Ready } else { Stage::Old { x: x + 1, previous: pixel, shift: 0, repeat: 0 } }
                    }
                }
            }
            Stage::Ready => return Err(invalid("HDR row must be consumed before decoding another")),
            Stage::Done => return Err(invalid("HDR input already consumed")),
        };
        Ok(())
    }
    pub(super) fn linear(&self, x: usize) -> Result<[f32; 3]> {
        if !self.ready() {
            return Err(invalid("HDR row is not ready"));
        }
        let pixel = self.pixels.get(x).ok_or_else(|| invalid("invalid HDR pixel address"))?;
        let exponent = *pixel.get(3).ok_or_else(|| invalid("missing RGBE exponent"))?;
        if exponent == 0 {
            return Ok([0.0; 3]);
        }
        let scale = self.scales.get(usize::from(exponent)).copied().ok_or_else(|| invalid("invalid RGBE exponent"))? / self.source.info.exposure;
        let mut rgb = [0.0; 3];
        for (channel, out) in rgb.iter_mut().enumerate() {
            let value = f64::from(*pixel.get(channel).ok_or_else(|| invalid("invalid RGBE channel"))?) * scale
                / self.source.info.color_correction.get(channel).copied().ok_or_else(|| invalid("invalid HDR correction"))?;
            if !value.is_finite() || !(0.0..=MAX_RADIANCE).contains(&value) {
                return Err(limit("corrected HDR radiance exceeds 100 million"));
            }
            *out = value as f32;
        }
        Ok(rgb)
    }
    pub(super) fn next_row(&mut self) -> Result<()> {
        if !self.ready() {
            return Err(invalid("HDR row is not ready"));
        }
        self.row = self.row.checked_add(1).ok_or_else(|| invalid("HDR row overflow"))?;
        self.stage = if self.row == self.source.info.height { Stage::Done } else { Stage::Start };
        Ok(())
    }
}
