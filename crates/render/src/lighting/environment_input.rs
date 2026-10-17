//! Captured existing HDR source, moved through a job under its ONE work lease.
#[cfg(feature = "raytrace-cpu")]
use super::environment_sampling::Environment;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentInfo {
    pub source_sha256: [u8; 32],
    pub header: super::HdrInfo,
    pub rotation_radians: f64,
    pub intensity: f64,
    pub samples: u32,
    pub color_policy: EnvironmentColorPolicy,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentColorPolicy {
    ExplicitLinearSrgbD65,
}

#[cfg(feature = "raytrace-cpu")]
use super::{DecodedHdr, HdrDecodeTask, HdrTaskPoll};
use super::{HdrError, HdrErrorCode, HdrSource, Result};

/// Transient captured input, never serialized into status or saved design data.
pub struct EnvironmentInput {
    source: HdrSource,
    info: EnvironmentInfo,
}
impl EnvironmentInput {
    pub fn new(source: HdrSource, rotation: f64, intensity: f64, samples: u32) -> Result<Self> {
        if !rotation.is_finite() || !intensity.is_finite() || !(0.0..=64.0).contains(&intensity) || !matches!(samples, 1 | 4 | 16 | 64) {
            return Err(HdrError::new(HdrErrorCode::InvalidInput, "invalid environment controls"));
        }
        if source.info().primaries != Some([0.64, 0.33, 0.30, 0.60, 0.15, 0.06, 0.3127, 0.3290]) {
            return Err(HdrError::new(HdrErrorCode::InvalidInput, "environment requires explicit sRGB/D65 primaries"));
        }
        let info = EnvironmentInfo {
            source_sha256: Sha256::digest(source.encoded_bytes()).into(),
            header: source.info().clone(),
            rotation_radians: rotation,
            intensity,
            samples,
            color_policy: EnvironmentColorPolicy::ExplicitLinearSrgbD65,
        };
        Ok(Self { source, info })
    }
    pub fn metadata(&self) -> EnvironmentInfo {
        self.info.clone()
    }

    /// Source capacity + exact decoded allocation + scanline + conservative task state.
    /// The host adds this BEFORE reserving; this is no separate memory pool.
    pub fn required_bytes(&self) -> Result<u64> {
        self.source
            .retained_encoded_bytes()
            .checked_add(self.source.info().decoded_rgb_bytes)
            .and_then(|v| v.checked_add(u64::from(self.source.info().width) * 4))
            .and_then(|v| v.checked_add(8192))
            .ok_or_else(|| HdrError::new(HdrErrorCode::LimitExceeded, "environment admission overflow"))
    }
    #[cfg(feature = "raytrace-cpu")]
    pub(crate) fn begin(self) -> Result<EnvironmentState> {
        if std::mem::size_of::<HdrDecodeTask>() > 4096 {
            return Err(HdrError::new(HdrErrorCode::LimitExceeded, "HDR decoder state exceeds admission"));
        }
        let mut decoder = Vec::new();
        decoder.try_reserve_exact(1).map_err(|_| HdrError::new(HdrErrorCode::AllocationFailed, "HDR decoder state allocation failed"))?;
        if decoder.capacity() != 1 {
            return Err(HdrError::new(HdrErrorCode::LimitExceeded, "HDR decoder state capacity exceeds admission"));
        }
        decoder.push(HdrDecodeTask::new(self.source)?);
        Ok(EnvironmentState {
            decoder,
            decoded: None,
            rotation: self.info.rotation_radians,
            intensity: self.info.intensity,
            samples: self.info.samples,
        })
    }
}

#[cfg(feature = "raytrace-cpu")]
pub(crate) struct EnvironmentState {
    decoder: Vec<HdrDecodeTask>,
    decoded: Option<DecodedHdr>,
    rotation: f64,
    intensity: f64,
    pub samples: u32,
}
#[cfg(feature = "raytrace-cpu")]
impl EnvironmentState {
    pub fn ready(&self) -> bool {
        self.decoded.is_some()
    }
    /// Caller debits one shared unit before each invocation; decoder also observes cancellation.
    pub fn prepare_one(&mut self, cancelled: impl Fn() -> bool) -> Result<()> {
        let decoder = self.decoder.first_mut().ok_or_else(|| HdrError::new(HdrErrorCode::Consumed, "environment decoder consumed"))?;
        if let HdrTaskPoll::Complete(decoded) = decoder.step(1, cancelled)? {
            let expected = decoded.info().decoded_rgb_bytes;
            if decoded.retained_pixel_bytes()? != expected {
                return Err(HdrError::new(HdrErrorCode::LimitExceeded, "environment decoded capacity exceeds admission"));
            }
            Environment::from_decoder_output(&decoded, self.rotation, self.intensity)?.require_linear_srgb()?;
            self.decoded = Some(decoded);
            self.decoder = Vec::new();
        }
        Ok(())
    }
    pub fn view(&self) -> Result<Environment<'_>> {
        let decoded = self.decoded.as_ref().ok_or_else(|| HdrError::new(HdrErrorCode::Consumed, "environment not ready"))?;
        Environment::from_decoder_output(decoded, self.rotation, self.intensity)
    }
}
