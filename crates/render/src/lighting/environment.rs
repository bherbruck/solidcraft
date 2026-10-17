//! Captured runtime environment input and original exact-solid-angle importance tables.
//! The HDR decoder retains source primaries: admission below makes interpretation explicit.
use super::{HdrDecodeTask, HdrSource, HdrTaskPoll};
use crate::job::{CancelToken, ErrorCode, JobError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solidcraft_geom::Vec3;
use std::{
    f64::consts::{PI, TAU},
    sync::Arc,
};

fn invalid(message: impl Into<String>) -> JobError {
    JobError::new(ErrorCode::InvalidRequest, message)
}
fn reserve<T>(count: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(count).map_err(|_| JobError::new(ErrorCode::AllocationFailed, "environment allocation failed"))?;
    Ok(v)
}

/// Original artistic gradient, not a calibrated atmosphere. No imported image assets.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkyGradient {
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    pub ground: [f32; 3],
}
impl Default for SkyGradient {
    fn default() -> Self {
        Self { zenith: [0.16, 0.32, 0.7], horizon: [0.8, 0.9, 1.0], ground: [0.12, 0.1, 0.08] }
    }
}
impl SkyGradient {
    pub fn validate(self) -> Result<()> {
        if self.zenith.into_iter().chain(self.horizon).chain(self.ground).any(|v| !v.is_finite() || !(0.0..=1e6).contains(&v)) {
            return Err(invalid("sky radiance must be finite and within 0..1000000"));
        }
        Ok(())
    }
    fn evaluate(self, z: f64) -> [f32; 3] {
        let endpoint = if z >= 0.0 { self.zenith } else { self.ground };
        let blend = z.abs().sqrt() as f32;
        std::array::from_fn(|i| self.horizon[i] * (1.0 - blend) + endpoint[i] * blend)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentInfo {
    pub kind: &'static str,
    pub width: u32,
    pub height: u32,
    pub encoded_bytes: u64,
    pub source_sha256: String,
    pub color_policy: &'static str,
    pub rotation_rad: f64,
    pub intensity: f64,
}
#[derive(Clone)]
enum Source {
    Hdr(HdrSource),
    Sky(SkyGradient),
}
/// Immutable captured input. Preparation happens in the image worker, with cancellation.
#[derive(Clone)]
pub struct EnvironmentSource {
    source: Source,
    info: EnvironmentInfo,
    retained_bytes: u64,
}
impl std::fmt::Debug for EnvironmentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentSource").field("info", &self.info).finish()
    }
}
impl EnvironmentSource {
    pub fn hdr(bytes: Vec<u8>, rotation_rad: f64, intensity: f64, assume_linear_srgb: bool) -> Result<Self> {
        let retained_bytes = bytes.capacity() as u64;
        let source = HdrSource::new(bytes).map_err(|e| invalid(e.to_string()))?;
        let primaries = [0.64, 0.33, 0.30, 0.60, 0.15, 0.06, 0.3127, 0.3290];
        let color_policy = match source.info().primaries {
            Some(actual) if actual.iter().zip(primaries).all(|(a, b)| (a - b).abs() < 1e-5) => "declared_linear_srgb_d65",
            Some(_) => return Err(invalid("HDR primaries must be sRGB/D65; convert the source before rendering")),
            None if assume_linear_srgb => "explicit_assumed_linear_srgb_d65",
            None => {
                return Err(invalid("HDR primaries are unspecified: explicitly set assume_linear_srgb or supply a converted source with PRIMARIES"));
            }
        };
        let info = EnvironmentInfo {
            kind: "radiance_hdr",
            width: source.info().width,
            height: source.info().height,
            encoded_bytes: source.info().encoded_bytes,
            source_sha256: format!("{:x}", Sha256::digest(source.encoded_bytes())),
            color_policy,
            rotation_rad,
            intensity,
        };
        let value = Self { source: Source::Hdr(source), info, retained_bytes };
        value.validate()?;
        Ok(value)
    }
    pub fn sky(sky: SkyGradient, rotation_rad: f64, intensity: f64) -> Result<Self> {
        sky.validate()?;
        let mut hash = Sha256::new();
        hash.update(b"solidcraft.original-gradient-sky/1");
        for v in sky.zenith.into_iter().chain(sky.horizon).chain(sky.ground) {
            hash.update(v.to_le_bytes());
        }
        let value = Self {
            source: Source::Sky(sky),
            retained_bytes: 0,
            info: EnvironmentInfo {
                kind: "original_gradient_sky",
                width: 512,
                height: 256,
                encoded_bytes: 0,
                source_sha256: format!("{:x}", hash.finalize()),
                color_policy: "authored_linear_srgb_d65",
                rotation_rad,
                intensity,
            },
        };
        value.validate()?;
        Ok(value)
    }
    pub fn info(&self) -> &EnvironmentInfo {
        &self.info
    }
    pub fn validate(&self) -> Result<()> {
        if !self.info.rotation_rad.is_finite()
            || self.info.rotation_rad.abs() > 1e6
            || !self.info.intensity.is_finite()
            || !(0.0..=100.0).contains(&self.info.intensity)
        {
            return Err(invalid("environment rotation must be finite within +/-1000000 radians and intensity 0..100"));
        }
        Ok(())
    }
    /// Conservative additional peak: encoded bytes, decode/table/alias work, staging and GPU storage.
    pub fn admitted_bytes(&self) -> u64 {
        self.retained_bytes + u64::from(self.info.width) * u64::from(self.info.height) * 160 + 65536
    }
    pub fn prepare(&self, cancel: &CancelToken) -> Result<PreparedEnvironment> {
        self.validate()?;
        cancel.check()?;
        let pixels = match &self.source {
            Source::Hdr(source) => {
                let mut task = HdrDecodeTask::new(source.clone()).map_err(|e| invalid(e.to_string()))?;
                let decoded = loop {
                    cancel.check()?;
                    match task.step(16384, || cancel.is_cancelled()).map_err(|e| {
                        if cancel.is_cancelled() {
                            JobError::new(ErrorCode::Cancelled, "environment preparation cancelled")
                        } else {
                            invalid(e.to_string())
                        }
                    })? {
                        HdrTaskPoll::Pending(_) => {}
                        HdrTaskPoll::Complete(decoded) => break decoded,
                    }
                };
                let mut out = reserve(decoded.pixels().len())?;
                out.extend_from_slice(decoded.pixels());
                out
            }
            Source::Sky(sky) => {
                let count = (self.info.width * self.info.height) as usize;
                let mut out = reserve(count)?;
                for y in 0..self.info.height {
                    cancel.check()?;
                    let z = (PI * (f64::from(y) + 0.5) / f64::from(self.info.height)).cos();
                    for _ in 0..self.info.width {
                        out.push(sky.evaluate(z));
                    }
                }
                out
            }
        };
        PreparedEnvironment::new(self.info.width, self.info.height, pixels, cancel)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EnvironmentTexel {
    pub rgb: [f32; 3],
    /// Density per steradian, not probability per texel.
    pub pdf: f32,
    pub threshold: f32,
    pub alias: u32,
}
pub struct PreparedEnvironment {
    width: u32,
    height: u32,
    texels: Vec<EnvironmentTexel>,
}
impl PreparedEnvironment {
    fn new(width: u32, height: u32, pixels: Vec<[f32; 3]>, cancel: &CancelToken) -> Result<Self> {
        let count = (width * height) as usize;
        if count != pixels.len() || count == 0 {
            return Err(invalid("invalid environment storage"));
        }
        let mut masses = reserve(count)?;
        let mut total = 0.0;
        for (i, rgb) in pixels.iter().enumerate() {
            if i % 1024 == 0 {
                cancel.check()?;
            }
            let mass = (0.2126 * f64::from(rgb[0]) + 0.7152 * f64::from(rgb[1]) + 0.0722 * f64::from(rgb[2]))
                * row_solid_angle(width, height, i / width as usize);
            total += mass;
            masses.push(mass);
        }
        if total == 0.0 {
            for (i, mass) in masses.iter_mut().enumerate() {
                *mass = row_solid_angle(width, height, i / width as usize);
            }
            total = 4.0 * PI;
        }
        let mut small = reserve(count)?;
        let mut large = reserve(count)?;
        let mut texels = reserve(count)?;
        for (i, (rgb, mass)) in pixels.into_iter().zip(masses.iter_mut()).enumerate() {
            if i % 1024 == 0 {
                cancel.check()?;
            }
            let probability = *mass / total;
            texels.push(EnvironmentTexel {
                rgb,
                pdf: (probability / row_solid_angle(width, height, i / width as usize)) as f32,
                threshold: 1.0,
                alias: i as u32,
            });
            *mass = probability * count as f64;
            if *mass < 1.0 {
                small.push(i);
            } else {
                large.push(i);
            }
        }
        while !small.is_empty() && !large.is_empty() {
            cancel.check()?;
            let low = small.pop().ok_or_else(|| invalid("invalid alias table"))?;
            let high = large.pop().ok_or_else(|| invalid("invalid alias table"))?;
            let value = *masses.get(low).ok_or_else(|| invalid("invalid alias mass"))?;
            let entry = texels.get_mut(low).ok_or_else(|| invalid("invalid alias index"))?;
            entry.threshold = value as f32;
            entry.alias = high as u32;
            let remaining = masses.get_mut(high).ok_or_else(|| invalid("invalid alias mass"))?;
            *remaining = (*remaining + value - 1.0).max(0.0);
            if *remaining < 1.0 {
                small.push(high);
            } else {
                large.push(high);
            }
        }
        cancel.check()?;
        Ok(Self { width, height, texels })
    }
    pub fn dimensions(&self) -> [u32; 2] {
        [self.width, self.height]
    }
    pub fn texels(&self) -> &[EnvironmentTexel] {
        &self.texels
    }
    /// CPU oracle of the same original piecewise-constant sphere mapping as WGSL.
    pub fn sample(&self, choice: f64, coin: f64, azimuth: f64, vertical: f64) -> Result<(Vec3, [f32; 3], f32)> {
        if [choice, coin, azimuth, vertical].into_iter().any(|v| !v.is_finite() || !(0.0..1.0).contains(&v)) {
            return Err(invalid("environment samples must be in [0,1)"));
        }
        let scaled = choice * self.texels.len() as f64;
        let index = scaled.floor() as usize;
        let entry = self.texels.get(index).ok_or_else(|| invalid("environment sample index"))?;
        let index = if coin < f64::from(entry.threshold) { index } else { entry.alias as usize };
        let selected = self.texels.get(index).ok_or_else(|| invalid("environment alias index"))?;
        let row = index / self.width as usize;
        let column = index % self.width as usize;
        let top = (PI * row as f64 / f64::from(self.height)).cos();
        let bottom = (PI * (row + 1) as f64 / f64::from(self.height)).cos();
        let z = top + (bottom - top) * vertical;
        let phi = TAU * (column as f64 + azimuth) / f64::from(self.width);
        let radius = (1.0 - z * z).max(0.0).sqrt();
        Ok((Vec3::new(radius * phi.cos(), radius * phi.sin(), z), selected.rgb, selected.pdf))
    }
    pub fn evaluate(&self, direction: Vec3) -> Result<([f32; 3], f32)> {
        let d = crate::surface::unit_direction(direction).map_err(|_| invalid("invalid environment direction"))?;
        let column = ((d.y.atan2(d.x).rem_euclid(TAU) / TAU * f64::from(self.width)) as usize).min(self.width as usize - 1);
        let row = ((d.z.clamp(-1.0, 1.0).acos() / PI * f64::from(self.height)) as usize).min(self.height as usize - 1);
        let texel = self.texels.get(row * self.width as usize + column).ok_or_else(|| invalid("environment lookup index"))?;
        Ok((texel.rgb, texel.pdf))
    }
}
fn row_solid_angle(width: u32, height: u32, row: usize) -> f64 {
    // Stable difference of cosines at polar rows.
    let half = PI / (2.0 * f64::from(height));
    (TAU / f64::from(width)) * 2.0 * (PI * (row as f64 + 0.5) / f64::from(height)).sin() * half.sin()
}

/// Shared captured source, cheap to clone into queued jobs.
pub type EnvironmentResource = Arc<EnvironmentSource>;

#[cfg(test)]
mod tests;
