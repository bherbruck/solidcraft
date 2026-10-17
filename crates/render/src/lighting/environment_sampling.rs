//! Original Z-up equirectangular sampling; RGB remains in the decoded SOURCE space.
//! Uniform solid-angle sampling deliberately has no luminance CDF or extra pixel copy.

use super::{DecodedHdr, HdrError, HdrErrorCode, MAX_HDR_HEIGHT, MAX_HDR_PIXELS, MAX_HDR_WIDTH, Result};
use solidcraft_geom::Vec3;
use std::f64::consts::{PI, TAU};

fn invalid(message: &str) -> HdrError {
    HdrError::new(HdrErrorCode::InvalidInput, message)
}

/// Borrowed decoded storage. Its owner and existing admission lease must outlive this view.
/// This is not a declaration that source RGB is linear sRGB.
pub struct Environment<'a> {
    decoded: &'a DecodedHdr,
    rotation: f64,
    intensity: f64,
}

/// Incident SOURCE-space linear RGB and a density with respect to steradians.
#[derive(Clone, Copy, Debug)]
pub struct EnvironmentSample {
    pub direction: Vec3,
    pub radiance_source_rgb: Vec3,
    pub pdf_solid_angle: f64,
}

/// One unoccluded linear-sRGB direct-light estimate. The consumer owns visibility
/// and averages these estimates once; no display transfer or sample-count division.
pub struct EnvironmentDirectSample {
    pub direction: Vec3,
    pub contribution_linear_srgb: Vec3,
    pub pdf_solid_angle: f64,
}

/// Cooperative validation with no allocations. The caller charges each inspected texel
/// to its existing cumulative budget, and can return a typed cancellation/limit error.
pub struct EnvironmentPreparation<'a> {
    environment: Option<Environment<'a>>,
    inspected: usize,
}

impl<'a> EnvironmentPreparation<'a> {
    pub fn new(decoded: &'a DecodedHdr, rotation_radians: f64, intensity: f64) -> Result<Self> {
        let info = decoded.info();
        let count = u64::from(info.width).checked_mul(u64::from(info.height));
        if info.width == 0
            || info.height == 0
            || info.width > MAX_HDR_WIDTH
            || info.height > MAX_HDR_HEIGHT
            || info.width != info.height.saturating_mul(2)
            || count.is_none_or(|n| n > MAX_HDR_PIXELS || n != decoded.pixels().len() as u64)
            || decoded.retained_pixel_bytes()? > MAX_HDR_PIXELS * 12
        {
            return Err(invalid("invalid bounded decoded environment dimensions or retained capacity"));
        }
        if !rotation_radians.is_finite() || !intensity.is_finite() || !(0.0..=64.0).contains(&intensity) {
            return Err(invalid("environment rotation must be finite and intensity within 0..64"));
        }
        Ok(Self { environment: Some(Environment { decoded, rotation: rotation_radians.rem_euclid(TAU), intensity }), inspected: 0 })
    }

    /// Returns true when validated. Any error consumes preparation; no partial view escapes.
    pub fn step(&mut self, units: usize, mut charge_and_check_cancel: impl FnMut() -> Result<()>) -> Result<bool> {
        let result = self.advance(units, &mut charge_and_check_cancel);
        if result.is_err() {
            self.environment = None;
        }
        result
    }

    fn advance(&mut self, units: usize, charge: &mut impl FnMut() -> Result<()>) -> Result<bool> {
        let environment = self.environment.as_ref().ok_or_else(|| HdrError::new(HdrErrorCode::Consumed, "environment preparation consumed"))?;
        // Even an already-complete poll observes cancellation without inspecting more texels.
        for _ in 0..units.clamp(1, 16384) {
            charge()?;
            let Some(pixel) = environment.decoded.pixels().get(self.inspected) else {
                return Ok(true);
            };
            if pixel.iter().any(|c| !c.is_finite() || !(0.0..=1e8).contains(c)) {
                return Err(invalid("environment source radiance must be finite and within 0..1e8"));
            }
            self.inspected += 1;
            if self.inspected == environment.decoded.pixels().len() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn finish(mut self) -> Result<Environment<'a>> {
        let environment = self.environment.take().ok_or_else(|| HdrError::new(HdrErrorCode::Consumed, "environment preparation consumed"))?;
        if self.inspected != environment.decoded.pixels().len() {
            return Err(invalid("environment preparation is incomplete"));
        }
        Ok(environment)
    }
}

impl Environment<'_> {
    /// Only decoder-owned output can reach this internal consumer fast path.
    /// HdrDecodeTask already validates every corrected texel incrementally.
    #[cfg(feature = "raytrace-cpu")]
    pub(crate) fn from_decoder_output(decoded: &DecodedHdr, rotation: f64, intensity: f64) -> Result<Environment<'_>> {
        let mut preparation = EnvironmentPreparation::new(decoded, rotation, intensity)?;
        preparation.inspected = decoded.pixels().len();
        preparation.finish()
    }
    /// First lighting adapter policy: only explicitly declared sRGB/D65 primaries.
    /// Missing Radiance primaries are NOT assumed to be sRGB; other gamuts need
    /// an independently validated conversion before this adapter can admit them.
    pub fn require_linear_srgb(&self) -> Result<()> {
        if self.decoded.info().primaries != Some([0.64, 0.33, 0.30, 0.60, 0.15, 0.06, 0.3127, 0.3290]) {
            return Err(invalid("environment lighting requires explicit sRGB/D65 primaries; source gamut conversion is not supported yet"));
        }
        Ok(())
    }

    pub fn radiance_linear_srgb(&self, direction: Vec3) -> Result<Vec3> {
        self.require_linear_srgb()?;
        self.radiance_source_rgb(direction)
    }

    pub fn direct_sample_linear_srgb(
        &self,
        u: [f64; 2],
        surface: crate::surface::OpaqueSurface,
        shading_normal: Vec3,
        geometric_normal: Vec3,
        view: Vec3,
    ) -> Result<EnvironmentDirectSample> {
        self.require_linear_srgb()?;
        let sample = self.sample(u)?;
        let normal = normalized(shading_normal)?;
        let geometric = normalized(geometric_normal)?;
        let view = normalized(view)?;
        let brdf = surface.brdf(normal, view, sample.direction).map_err(|_| invalid("invalid environment BRDF directions"))?;
        let cosine = if geometric.dot(sample.direction) > 0.0 && geometric.dot(view) > 0.0 { normal.dot(sample.direction).max(0.0) } else { 0.0 };
        let li = sample.radiance_source_rgb;
        let contribution = Vec3::new(brdf.x * li.x, brdf.y * li.y, brdf.z * li.z) * (cosine / sample.pdf_solid_angle);
        if !contribution.is_finite() || [contribution.x, contribution.y, contribution.z].iter().any(|c| !(0.0..=1e12).contains(c)) {
            return Err(invalid("environment direct contribution exceeds finite radiance bounds"));
        }
        Ok(EnvironmentDirectSample { direction: sample.direction, contribution_linear_srgb: contribution, pdf_solid_angle: sample.pdf_solid_angle })
    }

    /// Original allocation capacity, not logical slice length; charge the owner only once.
    pub fn retained_pixel_bytes(&self) -> Result<u64> {
        self.decoded.retained_pixel_bytes()
    }

    /// +Z north; phi=atan2(y,x), u=phi/2pi wrapped; v=acos(z)/pi.
    /// Positive rotation turns the environment about +Z toward +Y (inverse for lookup).
    /// Nearest texel uses half-open angular cells, centres ((x+.5)/W,(y+.5)/H).
    /// The seam wraps, south clamps to the last row; exact poles choose longitude zero.
    pub fn uv(&self, direction: Vec3) -> Result<[f64; 2]> {
        let d = normalized(direction)?;
        let phi = if d.x == 0.0 && d.y == 0.0 { 0.0 } else { d.y.atan2(d.x) };
        Ok([((phi - self.rotation) / TAU).rem_euclid(1.0), d.z.clamp(-1.0, 1.0).acos() / PI])
    }

    /// Exposure/color-correction were applied by the decoder; do not repeat them.
    /// No gamut conversion, tone mapping or background interpretation takes place here.
    pub fn radiance_source_rgb(&self, direction: Vec3) -> Result<Vec3> {
        let [u, v] = self.uv(direction)?;
        let info = self.decoded.info();
        let x = ((u * f64::from(info.width)) as usize).min(info.width as usize - 1);
        let y = ((v * f64::from(info.height)) as usize).min(info.height as usize - 1);
        let index = y.checked_mul(info.width as usize).and_then(|n| n.checked_add(x)).ok_or_else(|| invalid("environment index overflow"))?;
        let [r, g, b] = *self.decoded.pixels().get(index).ok_or_else(|| invalid("missing environment texel"))?;
        Ok(Vec3::new(f64::from(r), f64::from(g), f64::from(b)) * self.intensity)
    }

    pub fn pdf_solid_angle(&self, direction: Vec3) -> Result<f64> {
        normalized(direction)?;
        Ok(1.0 / (4.0 * PI))
    }

    /// Open variates avoid mathematical boundary samples (f64 may still round to a pole).
    /// z is uniform on (-1,1), phi on (0,2pi): dOmega=dphi dz, PDF=1/(4pi).
    /// Black and constant maps have the same normalized PDF; there is no zero-total CDF.
    pub fn sample(&self, u: [f64; 2]) -> Result<EnvironmentSample> {
        if u.iter().any(|v| !v.is_finite() || *v <= 0.0 || *v >= 1.0) {
            return Err(invalid("environment variates must be finite and strictly between zero and one"));
        }
        let z = 1.0 - 2.0 * u[0];
        let radius = ((1.0 - z) * (1.0 + z)).max(0.0).sqrt();
        let phi = TAU * u[1];
        let direction = Vec3::new(radius * phi.cos(), radius * phi.sin(), z);
        Ok(EnvironmentSample { direction, radiance_source_rgb: self.radiance_source_rgb(direction)?, pdf_solid_angle: 1.0 / (4.0 * PI) })
    }
}

fn normalized(direction: Vec3) -> Result<Vec3> {
    let scale = direction.x.abs().max(direction.y.abs()).max(direction.z.abs());
    if !direction.is_finite() || scale <= 0.0 {
        return Err(invalid("environment direction must be finite and nonzero"));
    }
    let scaled = direction / scale;
    Ok(scaled / scaled.len())
}

#[cfg(test)]
#[path = "environment_sampling_tests.rs"]
mod tests;
