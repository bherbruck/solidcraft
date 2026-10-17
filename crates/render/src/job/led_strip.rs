//! Original continuous diffuse LED recipe, lowered to the existing rectangle consumer.
//! No source mesh, visible emission, spectral/CCT calibration or separate transport.
use super::{ErrorCode, JobError, RaytraceOptions, RectangleLightOptions, Result};
use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

/// Illustrative linear RGB choices, not measured LED spectra or calibrated CCT.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StripTint {
    Warm,
    Cool,
    Red,
    Green,
    Blue,
}

impl StripTint {
    pub fn linear_rgb(self) -> [f64; 3] {
        match self {
            Self::Warm => [1.0, 0.68, 0.38],
            Self::Cool => [0.65, 0.80, 1.0],
            Self::Red => [1.0, 0.0, 0.0],
            Self::Green => [0.0, 1.0, 0.0],
            Self::Blue => [0.0, 0.0, 1.0],
        }
    }
}

/// Full dimensions in millimetres; unit axes are explicit world orientation.
/// `length_axis cross width_axis` is the emitting side. Intensity is illustrative
/// emitted radiance (0..=64), not lumens or total strip power.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedStripOptions {
    pub center_mm: [f64; 3],
    pub length_mm: f64,
    pub width_mm: f64,
    pub length_axis: [f64; 3],
    pub width_axis: [f64; 3],
    pub color: [f64; 3],
    pub intensity: f64,
}

fn invalid(message: &'static str) -> JobError {
    JobError::new(ErrorCode::InvalidRequest, message)
}

impl LedStripOptions {
    /// Allocation-free conversion. One strip is exactly one rectangular source;
    /// the existing uniform-area PDF and A*cos(theta)/(r^2*N) weight are unchanged.
    pub fn rectangle(self) -> Result<RectangleLightOptions> {
        let vec = |[x, y, z]: [f64; 3]| Vec3::new(x, y, z);
        let u = vec(self.length_axis);
        let v = vec(self.width_axis);
        if ![self.length_mm, self.width_mm].into_iter().all(|n| n.is_finite() && (1e-6..=1e6).contains(&n))
            || !u.is_finite()
            || !v.is_finite()
            || (u.len() - 1.0).abs() > 1e-12
            || (v.len() - 1.0).abs() > 1e-12
            || u.dot(v).abs() > 1e-10
            || !self.color.into_iter().all(|n| n.is_finite() && (0.0..=1.0).contains(&n))
            || !self.intensity.is_finite()
            || !(0.0..=64.0).contains(&self.intensity)
        {
            return Err(invalid("invalid LED dimensions, unit orientation, linear color or radiance"));
        }
        // Normalize within the admitted axis tolerance, then validate the actual
        // rounded edges with RT3's length and relative-cosine predicates.
        let edge_u = u * (self.length_mm / u.len());
        let edge_v = v * (self.width_mm / v.len());
        if !edge_u.is_finite()
            || !edge_v.is_finite()
            || !(1e-6..=1e6).contains(&edge_u.len())
            || !(1e-6..=1e6).contains(&edge_v.len())
            || (edge_u.dot(edge_v) / (edge_u.len() * edge_v.len())).abs() > 1e-10
        {
            return Err(invalid("normalized LED edges exceed rectangle length or orthogonality bounds"));
        }
        let center = vec(self.center_mm);
        if !center.is_finite() || [center.x, center.y, center.z].into_iter().any(|n| n.abs() > 1e12) {
            return Err(invalid("LED center exceeds finite world bounds"));
        }
        for a in [-0.5, 0.5] {
            for b in [-0.5, 0.5] {
                let p = center + edge_u * a + edge_v * b;
                if !p.is_finite() || [p.x, p.y, p.z].into_iter().any(|n| n.abs() > 1e12) {
                    return Err(invalid("LED corner exceeds finite world bounds"));
                }
            }
        }
        Ok(RectangleLightOptions {
            center_mm: self.center_mm,
            edge_u_mm: [edge_u.x, edge_u.y, edge_u.z],
            edge_v_mm: [edge_v.x, edge_v.y, edge_v.z],
            color: self.color,
            intensity: self.intensity,
        })
    }

    /// Lower into an existing request BEFORE host admission/capture. No retained
    /// recipe, extra pool or light vector: original rectangle capacity is billed
    /// by RT3's request lease. Rejection leaves the request unchanged.
    /// RT3 shares one N among its rectangles; conflicting N is explicitly refused.
    pub fn append_to(self, options: &mut RaytraceOptions, samples: u32) -> Result<()> {
        options.validate()?;
        if !matches!(samples, 1 | 4 | 16 | 64) {
            return Err(invalid("LED samples must be 1, 4, 16 or 64 for the whole strip"));
        }
        let light = self.rectangle()?;
        let count = options.area_lights.as_ref().map_or(0, Vec::len);
        // The union admits eight directional, eight punctual and four rectangle/LED sources.
        if count >= 4 {
            return Err(invalid("LED exceeds existing rectangle or combined RT3 slots"));
        }
        if count != 0 && options.area_samples.unwrap_or(16) != samples {
            return Err(invalid("all rectangle and LED sources must share area_samples"));
        }
        let allocate = |e| JobError::new(ErrorCode::AllocationFailed, format!("LED rectangle storage: {e}"));
        // Stage at most four inline DTOs before host admission, so allocator
        // over-reservation cannot invalidate or partially mutate the caller.
        let mut lights = Vec::new();
        lights.try_reserve_exact(count + 1).map_err(allocate)?;
        if lights.capacity() > 4 {
            return Err(JobError::new(ErrorCode::LimitExceeded, "LED allocator exceeded rectangle slot capacity"));
        }
        if let Some(previous) = &options.area_lights {
            lights.extend(previous.iter().cloned());
        }
        lights.push(light);
        options.area_lights = Some(lights);
        options.area_samples = Some(samples);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
