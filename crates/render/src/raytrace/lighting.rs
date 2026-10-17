//! Bounded direct lighting for RT2. World directions point from a hit towards each light.
//! Colours are linear sRGB; intensity is relative radiance, not watts or illuminance.
//! Visibility is supplied separately for every light by the ray-tracing consumer.

use solidcraft_geom::Vec3;

use crate::surface::{OpaqueSurface, unit_direction};

use super::{Result, TraceError};

pub const MAX_LIGHTS: usize = 8;
pub const MAX_LIGHT_INTENSITY: f64 = 64.0;
/// Refuse unrepresentable or excessive output rather than silently clip the transport.
pub const MAX_LINEAR_RADIANCE: f64 = 1.0e12;
pub const MAX_EXPOSURE_EV: f64 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectionalLight {
    direction: Vec3,
    color: Vec3,
    intensity: f64,
}

impl DirectionalLight {
    pub fn new(direction: Vec3, linear_color: Vec3, intensity: f64) -> Result<Self> {
        let direction = unit_direction(direction).map_err(|_| TraceError::InvalidInput("finite nonzero light direction required"))?;
        if !linear_color.is_finite() || !channels(linear_color).into_iter().all(|c| (0.0..=1.0).contains(&c)) {
            return Err(TraceError::InvalidInput("light colour must be finite linear sRGB in [0, 1]"));
        }
        if !intensity.is_finite() || !(0.0..=MAX_LIGHT_INTENSITY).contains(&intensity) {
            return Err(TraceError::InvalidInput("light intensity must be finite relative radiance in [0, 64]"));
        }
        Ok(Self { direction, color: linear_color, intensity })
    }

    pub fn direction(self) -> Vec3 {
        self.direction
    }

    pub fn color(self) -> Vec3 {
        self.color
    }

    pub fn intensity(self) -> f64 {
        self.intensity
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StudioPreset {
    /// The existing three neutral lights, with unchanged directions and intensities.
    Neutral,
    /// Original warm key / cool fill colour composition, without a Kelvin claim.
    WarmCool,
    /// Original opposing key / rim composition. These are hard directional lights.
    KeyRim,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    lights: Vec<DirectionalLight>,
}

impl Default for Lighting {
    fn default() -> Self {
        Self::studio_default()
    }
}

impl Lighting {
    pub fn new(lights: Vec<DirectionalLight>) -> Result<Self> {
        if lights.len() > MAX_LIGHTS || lights.capacity() > MAX_LIGHTS {
            return Err(TraceError::InvalidInput("RT2 admits at most eight directional lights and eight reserved light slots"));
        }
        // An empty list deliberately means no direct illumination.
        Ok(Self { lights })
    }

    pub fn studio_default() -> Self {
        Self {
            lights: crate::studio::LIGHTS
                .into_iter()
                .map(|(direction, intensity)| DirectionalLight { direction, color: Vec3::new(1.0, 1.0, 1.0), intensity })
                .collect(),
        }
    }

    pub fn preset(preset: StudioPreset) -> Result<Self> {
        match preset {
            StudioPreset::Neutral => Ok(Self::studio_default()),
            StudioPreset::WarmCool => Self::new(vec![
                DirectionalLight::new(Vec3::new(0.6, 0.0, 0.8), Vec3::new(1.0, 0.72, 0.48), 3.0)?,
                DirectionalLight::new(Vec3::new(-0.8, 0.0, 0.6), Vec3::new(0.45, 0.65, 1.0), 1.0)?,
                DirectionalLight::new(Vec3::new(0.0, -0.6, 0.8), Vec3::new(1.0, 1.0, 1.0), 1.5)?,
            ]),
            StudioPreset::KeyRim => Self::new(vec![
                DirectionalLight::new(Vec3::new(0.6, -0.4, 0.8), Vec3::new(1.0, 1.0, 1.0), 3.0)?,
                DirectionalLight::new(Vec3::new(-0.6, 0.4, 0.8), Vec3::new(0.6, 0.8, 1.0), 2.0)?,
            ]),
        }
    }

    pub fn lights(&self) -> &[DirectionalLight] {
        &self.lights
    }

    /// Sum BRDF * cosine * incident colour/intensity for independently visible lights.
    /// `visible` returns true for an unobstructed shadow ray; its errors propagate.
    /// No ambient term, exposure, tone map, display encoding or indirect bounce occurs here.
    pub fn shade_linear(
        &self,
        surface: OpaqueSurface,
        normal: Vec3,
        view: Vec3,
        mut visible: impl FnMut(usize, Vec3) -> Result<bool>,
    ) -> Result<Vec3> {
        let n = unit_direction(normal).map_err(|_| TraceError::InvalidInput("finite nonzero shading normal required"))?;
        let v = unit_direction(view).map_err(|_| TraceError::InvalidInput("finite nonzero shading view required"))?;
        let mut sum = Vec3::ZERO;
        if n.dot(v) <= 0.0 {
            return Ok(sum);
        }
        for (index, light) in self.lights.iter().enumerate() {
            let cosine = n.dot(light.direction).max(0.0);
            if cosine == 0.0 || light.intensity == 0.0 || light.color == Vec3::ZERO || !visible(index, light.direction)? {
                continue;
            }
            let brdf = surface.brdf(n, v, light.direction).map_err(|_| TraceError::InvalidInput("invalid opaque surface lighting directions"))?;
            let weight = light.intensity * cosine;
            sum += Vec3::new(brdf.x * light.color.x, brdf.y * light.color.y, brdf.z * light.color.z) * weight;
            validate_radiance(sum)?;
        }
        Ok(sum)
    }
}

fn channels(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

pub(super) fn validate_radiance(radiance: Vec3) -> Result<()> {
    if !radiance.is_finite() || !channels(radiance).into_iter().all(|c| (0.0..=MAX_LINEAR_RADIANCE).contains(&c)) {
        return Err(TraceError::InvalidInput("RT2 linear radiance must be finite and in [0, 1e12]"));
    }
    Ok(())
}

/// The output boundary applies exposure once, then Reinhard once. The returned value
/// is still linear sRGB: the image consumer must encode sRGB exactly once afterwards.
/// Call this once after summing all direct lighting, never once per light.
pub fn map_to_display(radiance: Vec3, exposure_ev: f64) -> Result<Vec3> {
    validate_radiance(radiance)?;
    if !exposure_ev.is_finite() || !(-MAX_EXPOSURE_EV..=MAX_EXPOSURE_EV).contains(&exposure_ev) {
        return Err(TraceError::InvalidInput("RT2 exposure must be finite and in [-16, 16] EV"));
    }
    crate::studio::tone_map(radiance * exposure_ev.exp2()).map_err(|_| TraceError::InvalidInput("invalid exposed radiance"))
}

#[cfg(test)]
#[path = "lighting_tests.rs"]
mod tests;
