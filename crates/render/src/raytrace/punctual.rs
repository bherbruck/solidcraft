//! Original relative point/spot response. Coordinates/reference distances are mm.
//! No lumen, lux, watts, measured CCT, IES or glTF photometry is implied.

use solidcraft_geom::Vec3;

use super::{Ray, Result, TraceError, bounded, lighting::MAX_LIGHT_INTENSITY, prepared::SharedHit, unit};

/// Explicit preview lower bound, not a geometry/self-intersection epsilon.
pub const MIN_SOURCE_DISTANCE_MM: f64 = 0.001;
pub const MAX_REFERENCE_DISTANCE_MM: f64 = 1.0e6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpotCone {
    axis: Vec3,
    inner_cos: f64,
    outer_cos: f64,
}

impl SpotCone {
    /// Emitted world axis points FROM the light. Half-cones are radians.
    /// Distinct representable cosine bounds are required, including at zero.
    pub fn new(axis: Vec3, inner: f64, outer: f64) -> Result<Self> {
        let axis = unit(axis).ok_or(TraceError::InvalidInput("spot emitted axis"))?;
        if !inner.is_finite() || !outer.is_finite() || inner < 0.0 || inner >= outer || outer > std::f64::consts::FRAC_PI_2 {
            return Err(TraceError::InvalidInput("spot half-cones require 0 <= inner < outer <= pi/2"));
        }
        let inner_cos = inner.cos();
        let outer_cos = outer.cos();
        if inner_cos <= outer_cos {
            return Err(TraceError::InvalidInput("spot cosine feather is unrepresentable"));
        }
        Ok(Self { axis, inner_cos, outer_cos })
    }

    fn weight(self, emitted_direction: Vec3) -> f64 {
        let cosine = self.axis.dot(emitted_direction);
        let t = ((cosine - self.outer_cos) / (self.inner_cos - self.outer_cos)).clamp(0.0, 1.0);
        // Smoothstep in cosine space; zero slope at both cone boundaries.
        t * t * (3.0 - 2.0 * t)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PunctualLight {
    position: Vec3,
    color: Vec3,
    intensity: f64,
    reference_mm: f64,
    minimum_mm: f64,
    cone: Option<SpotCone>,
}

#[derive(Clone, Copy, Debug)]
pub struct PunctualSample {
    pub direction: Vec3,
    pub incident_linear_srgb: Vec3,
    position: Vec3,
}

impl PunctualLight {
    /// Intensity equals the relative on-axis incident value at reference_mm.
    /// Distances below minimum_mm REFUSE; the inverse square is never clamped.
    pub fn new(position: Vec3, color: Vec3, intensity: f64, reference_mm: f64, minimum_mm: f64, cone: Option<SpotCone>) -> Result<Self> {
        if !bounded(position) || !color.is_finite() || [color.x, color.y, color.z].into_iter().any(|x| !(0.0..=1.0).contains(&x)) {
            return Err(TraceError::InvalidInput("punctual position or linear sRGB colour"));
        }
        if !intensity.is_finite()
            || !(0.0..=MAX_LIGHT_INTENSITY).contains(&intensity)
            || !reference_mm.is_finite()
            || !(MIN_SOURCE_DISTANCE_MM..=MAX_REFERENCE_DISTANCE_MM).contains(&reference_mm)
            || !minimum_mm.is_finite()
            || !(MIN_SOURCE_DISTANCE_MM..=reference_mm).contains(&minimum_mm)
        {
            return Err(TraceError::InvalidInput("punctual relative intensity or explicit reference/minimum distances"));
        }
        Ok(Self { position, color, intensity, reference_mm, minimum_mm, cone })
    }

    pub fn sample(self, hit_position: Vec3) -> Result<PunctualSample> {
        if !bounded(hit_position) {
            return Err(TraceError::InvalidInput("punctual hit position"));
        }
        let delta = self.position - hit_position;
        let distance = delta.len();
        if !distance.is_finite() || distance < self.minimum_mm {
            return Err(TraceError::InvalidInput("punctual source nearer than explicit minimum distance"));
        }
        let direction = delta / distance;
        let cone = self.cone.map_or(1.0, |cone| cone.weight(-direction));
        let ratio = self.reference_mm / distance;
        let incident_linear_srgb = self.color * (self.intensity * ratio * ratio * cone);
        super::lighting::validate_radiance(incident_linear_srgb)?;
        Ok(PunctualSample { direction, incident_linear_srgb, position: self.position })
    }
}

impl PunctualSample {
    /// Re-aim from the common robust geometric offset, then exclude the source.
    /// An offset that crossed the source refuses; no backwards/unbounded ray.
    pub fn shadow_ray(self, hit: SharedHit<'_>) -> Result<Ray> {
        let origin = hit.spawn(self.direction)?.origin();
        let delta = self.position - origin;
        let distance = delta.len();
        if !distance.is_finite() || distance <= 0.0 || delta.dot(self.direction) <= 0.0 {
            return Err(TraceError::InvalidInput("punctual shadow offset crossed source"));
        }
        Ray::new(origin, delta, 0.0_f64.next_up(), distance.next_down())
    }
}

#[cfg(test)]
#[path = "punctual_tests.rs"]
mod tests;
