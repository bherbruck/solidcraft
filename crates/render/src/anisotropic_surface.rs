//! Original anisotropic GGX reflection with separable Smith masking and Schlick Fresnel.
//!
//! Mathematics: Heitz, JCGT 7(4), 2018, equations 1 and 2,
//! <https://jcgt.org/published/0007/04/01/>. No external implementation adapted.
//! Sampling here uses the **NDF**, not Heitz's visible-normal (VNDF) sampler:
//! `p(h) = D(h) * n.h`, then `p(l) = p(h) / (4 * v.h)`.
//! Rejected reflections are null events, never resampled or renormalized.
//!
//! This numerical module does not install a renderer, persist a material or alter
//! engineering axes. The visual frame's normal may be a shading normal, but the
//! original geometric normal is required separately to reject below-surface rays.
//! Consumers retain geometric normals for ray spawning and optical boundaries.
//! No shading-normal energy correction, multiple scattering or transmission is modeled.

use std::f64::consts::{PI, TAU};

use solidcraft_geom::Vec3;

use crate::surface::{MIN_ROUGHNESS, OpaqueSurface, SurfaceError, unit_direction};

/// Admission or finite-arithmetic failure in the anisotropic numerical contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnisotropicError {
    /// An existing opaque-surface parameter or direction was invalid.
    Surface(SurfaceError),
    /// A visual frame was not orthogonal, or opposed its geometric normal.
    Frame,
    /// A variate was not finite in the half-open interval [0, 1).
    Variates,
    /// A computed density, response or sample weight was not finite/nonnegative.
    Arithmetic,
}

impl From<SurfaceError> for AnisotropicError {
    fn from(value: SurfaceError) -> Self {
        Self::Surface(value)
    }
}

impl std::fmt::Display for AnisotropicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Surface(e) => std::fmt::Display::fmt(e, f),
            Self::Frame => f.write_str("visual frame must be orthogonal and face the geometric normal"),
            Self::Variates => f.write_str("anisotropic variates must be finite in [0, 1)"),
            Self::Arithmetic => f.write_str("anisotropic result must be finite and nonnegative"),
        }
    }
}
impl std::error::Error for AnisotropicError {}

type Result<T> = std::result::Result<T, AnisotropicError>;

/// Right-handed orthonormal visual axes; independent of engineering material axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisualFrame {
    normal: Vec3,
    tangent: Vec3,
    bitangent: Vec3,
}

impl VisualFrame {
    /// Normalize finite nonzero orthogonal inputs. An arbitrary oblique tangent is
    /// refused rather than silently choosing a different brushed direction.
    /// Roundoff within 1e-10 of perpendicular is removed by orthogonalization.
    pub fn new(normal: Vec3, tangent: Vec3) -> Result<Self> {
        let normal = unit_direction(normal)?;
        let tangent = unit_direction(tangent)?;
        let dot = normal.dot(tangent);
        if dot.abs() > 1e-10 {
            return Err(AnisotropicError::Frame);
        }
        let tangent = unit_direction(tangent - normal * dot)?;
        let bitangent = unit_direction(normal.cross(tangent))?;
        Ok(Self { normal, tangent, bitangent })
    }

    /// Shading normal, never a replacement for an original geometric normal.
    pub fn normal(self) -> Vec3 {
        self.normal
    }

    /// Visual X axis, associated with the first roughness.
    pub fn tangent(self) -> Vec3 {
        self.tangent
    }

    /// Visual Y axis, associated with the second roughness.
    pub fn bitangent(self) -> Vec3 {
        self.bitangent
    }

    fn local(self, direction: Vec3) -> Vec3 {
        Vec3::new(self.tangent.dot(direction), self.bitangent.dot(direction), self.normal.dot(direction))
    }

    fn world(self, local: Vec3) -> Vec3 {
        self.tangent * local.x + self.bitangent * local.y + self.normal * local.z
    }

    fn geometric(self, normal: Vec3) -> Result<Vec3> {
        let normal = unit_direction(normal)?;
        if normal.dot(self.normal) <= 0.0 {
            return Err(AnisotropicError::Frame);
        }
        Ok(normal)
    }
}

/// A bounded opaque response with two perceptual roughness axes in [0.045, 1].
/// Alpha is roughness squared, matching [`OpaqueSurface`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnisotropicSurface {
    base: OpaqueSurface,
    roughness_y: f64,
}

/// One reflected **specular-only** NDF sample. Fields are in world coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReflectionSample {
    /// Outgoing direction, pointing away from the surface.
    pub direction: Vec3,
    /// Unconditional density per steradian, without renormalizing null events.
    pub pdf: f64,
    /// `specular_brdf * shading_cosine / pdf`, in linear RGB.
    /// Diffuse response is excluded; its sampling strategy belongs to the consumer.
    pub weight: Vec3,
}

impl AnisotropicSurface {
    /// Linear-sRGB reflectance, metal fraction and IOR use the opaque contract.
    /// Perfect delta mirrors and out-of-range roughness are refused, not clamped.
    pub fn new(base_color: Vec3, metallic: f64, roughness_x: f64, roughness_y: f64, dielectric_ior: f64) -> Result<Self> {
        let base = OpaqueSurface::new(base_color, metallic, roughness_x, dielectric_ior)?;
        if !roughness_y.is_finite() || !(MIN_ROUGHNESS..=1.0).contains(&roughness_y) {
            return Err(SurfaceError::Roughness.into());
        }
        Ok(Self { base, roughness_y })
    }

    /// Admitted perceptual roughness along visual X and Y.
    pub fn roughness(self) -> [f64; 2] {
        [self.base.roughness(), self.roughness_y]
    }

    fn alpha(self) -> [f64; 2] {
        [self.base.roughness().powi(2), self.roughness_y.powi(2)]
    }

    fn fresnel(self, cosine: f64) -> Vec3 {
        let f0 = self.base.normal_reflectance();
        f0 + (Vec3::new(1.0, 1.0, 1.0) - f0) * (1.0 - cosine.clamp(0.0, 1.0)).powi(5)
    }

    fn distribution_local(self, h: Vec3) -> f64 {
        if h.z <= 0.0 {
            return 0.0;
        }
        let [ax, ay] = self.alpha();
        let q = (h.x / ax).powi(2) + (h.y / ay).powi(2) + h.z * h.z;
        1.0 / (PI * ax * ay * q * q)
    }

    // Smith G1 / cosine, avoiding division by tiny cosines and tan(theta).
    fn visibility(self, w: Vec3) -> f64 {
        let [ax, ay] = self.alpha();
        2.0 / (w.z + (w.z * w.z + (ax * w.x).powi(2) + (ay * w.y).powi(2)).sqrt())
    }

    /// GGX normal distribution D, per steradian of microfacet normal.
    /// This is not a normalized probability density: [`Self::normal_pdf`] is.
    pub fn distribution(self, frame: VisualFrame, half: Vec3) -> Result<f64> {
        finite_scalar(self.distribution_local(frame.local(unit_direction(half)?)))
    }

    /// Projected NDF density `D(h) * n.h` on the upper shading hemisphere.
    /// Its integral over that hemisphere is one.
    pub fn normal_pdf(self, frame: VisualFrame, half: Vec3) -> Result<f64> {
        let h = frame.local(unit_direction(half)?);
        finite_scalar(self.distribution_local(h) * h.z.max(0.0))
    }

    /// Sample the projected NDF by stretching a radial GGX slope distribution.
    /// The inverse radial CDF is `r = sqrt(u/(1-u))`; homogeneous coordinates
    /// `(alpha_x*sqrt(u)*cos(phi), alpha_y*sqrt(u)*sin(phi), sqrt(1-u))`
    /// avoid a large intermediate radius. No heap, rejection loop or RNG is used.
    pub fn sample_normal(self, frame: VisualFrame, variates: [f64; 2]) -> Result<Vec3> {
        let [u, azimuth] = variates;
        if !variates.into_iter().all(|v| v.is_finite() && (0.0..1.0).contains(&v)) {
            return Err(AnisotropicError::Variates);
        }
        let [ax, ay] = self.alpha();
        let phi = TAU * azimuth;
        let radial = u.sqrt();
        let h = unit_direction(Vec3::new(ax * radial * phi.cos(), ay * radial * phi.sin(), (1.0 - u).sqrt()))?;
        Ok(unit_direction(frame.world(h))?)
    }

    fn directions(self, frame: VisualFrame, geometric: Vec3, view: Vec3, light: Vec3) -> Result<Option<(Vec3, Vec3, Vec3)>> {
        let geometric = frame.geometric(geometric)?;
        let view = unit_direction(view)?;
        let light = unit_direction(light)?;
        let (v, l) = (frame.local(view), frame.local(light));
        if v.z <= 0.0 || l.z <= 0.0 || geometric.dot(view) <= 0.0 || geometric.dot(light) <= 0.0 {
            return Ok(None);
        }
        let h = unit_direction(view + light)?;
        Ok(Some((v, l, frame.local(h))))
    }

    fn specular_local(self, v: Vec3, l: Vec3, h: Vec3) -> Result<Vec3> {
        finite_rgb(self.fresnel(v.dot(h)) * (self.distribution_local(h) * 0.25 * self.visibility(v) * self.visibility(l)))
    }

    /// Specular component only, with independent geometric hemisphere rejection.
    /// All input directions point away from the surface.
    pub fn specular_brdf(self, frame: VisualFrame, geometric: Vec3, view: Vec3, light: Vec3) -> Result<Vec3> {
        let Some((v, l, h)) = self.directions(frame, geometric, view, light)? else { return Ok(Vec3::ZERO) };
        self.specular_local(v, l, h)
    }

    /// Full opaque BRDF, including the same symmetric attenuated Lambert term as
    /// [`OpaqueSurface`]. No radiance, cosine, exposure or display transform included.
    /// Equal roughness axes delegate to the existing isotropic numerical contract.
    pub fn brdf(self, frame: VisualFrame, geometric: Vec3, view: Vec3, light: Vec3) -> Result<Vec3> {
        let Some((v, l, h)) = self.directions(frame, geometric, view, light)? else { return Ok(Vec3::ZERO) };
        if self.base.roughness() == self.roughness_y {
            return Ok(self.base.brdf(frame.normal, view, light)?);
        }
        let specular = self.specular_local(v, l, h)?;
        let fv = self.fresnel(v.z);
        let fl = self.fresnel(l.z);
        let diffuse = self.base.base_color() * ((1.0 - self.base.metallic()) / PI);
        finite_rgb(
            specular
                + Vec3::new(
                    (1.0 - fv.x) * (1.0 - fl.x) * diffuse.x,
                    (1.0 - fv.y) * (1.0 - fl.y) * diffuse.y,
                    (1.0 - fv.z) * (1.0 - fl.z) * diffuse.z,
                ),
        )
    }

    /// Unconditional reflected-direction density for the specular NDF sampler.
    /// Integrates to the acceptance probability, not necessarily one: invalid
    /// microfacets and below-geometric/shading-hemisphere reflections are null events.
    /// A mixture with a diffuse or environment sampler must use its own mixture PDF.
    pub fn reflection_pdf(self, frame: VisualFrame, geometric: Vec3, view: Vec3, light: Vec3) -> Result<f64> {
        let Some((v, _, h)) = self.directions(frame, geometric, view, light)? else { return Ok(0.0) };
        finite_scalar(self.distribution_local(h) * h.z.max(0.0) / (4.0 * v.dot(h).max(0.0)))
    }

    /// One specular NDF attempt. `None` is a zero-contribution event and must
    /// still count towards the consumer's sample denominator. Never retry until
    /// success. Ray origins/visibility use the consumer's original geometric data.
    pub fn sample_reflection(self, frame: VisualFrame, geometric: Vec3, view: Vec3, variates: [f64; 2]) -> Result<Option<ReflectionSample>> {
        let geometric = frame.geometric(geometric)?;
        let view = unit_direction(view)?;
        let half = self.sample_normal(frame, variates)?;
        let v = frame.local(view);
        if v.z <= 0.0 || geometric.dot(view) <= 0.0 {
            return Ok(None);
        }
        let vh = view.dot(half);
        if vh <= 0.0 {
            return Ok(None);
        }
        let light = unit_direction(half * (2.0 * vh) - view)?;
        let l = frame.local(light);
        if l.z <= 0.0 || geometric.dot(light) <= 0.0 {
            return Ok(None);
        }
        let h = frame.local(half);
        let pdf = self.reflection_pdf(frame, geometric, view, light)?;
        if pdf <= 0.0 {
            return Err(AnisotropicError::Arithmetic);
        }
        // Cancel D algebraically in f_spec * cos / pdf. No tiny-cosine division.
        let weight = finite_rgb(self.fresnel(vh) * (self.visibility(v) * (self.visibility(l) * l.z) * vh / h.z))?;
        Ok(Some(ReflectionSample { direction: light, pdf, weight }))
    }
}

fn finite_scalar(value: f64) -> Result<f64> {
    if value.is_finite() && value >= 0.0 { Ok(value) } else { Err(AnisotropicError::Arithmetic) }
}

fn finite_rgb(value: Vec3) -> Result<Vec3> {
    if value.is_finite() && [value.x, value.y, value.z].into_iter().all(|c| c >= 0.0) { Ok(value) } else { Err(AnisotropicError::Arithmetic) }
}

#[cfg(test)]
#[path = "anisotropic_surface_tests.rs"]
mod tests;
