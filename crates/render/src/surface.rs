//! Bounded opaque metallic/roughness surfaces in linear sRGB.
//! This numerical contract does not enable a renderer or persist appearances.

use std::f64::consts::PI;

use solidcraft_geom::Vec3;

/// Lowest admitted perceptual roughness. Perfect delta mirrors need another integrator.
pub const MIN_ROUGHNESS: f64 = 0.045;

/// Original single-scattering GGX/Smith/Schlick preview model, with attenuated Lambert diffuse.
/// Private fields keep every instance finite and bounded; deserializers must use `new`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpaqueSurface {
    base_color: Vec3,
    metallic: f64,
    roughness: f64,
    dielectric_ior: f64,
    clear_coat: Option<crate::clear_coat::ClearCoat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceError {
    BaseColor,
    Metallic,
    CoatWeight,
    Roughness,
    DielectricIor,
    Direction,
    Radiance,
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BaseColor => "linear base color channels must be finite and in [0, 1]",
            Self::CoatWeight => "clear-coat weight must be finite and in [0, 1]",
            Self::Metallic => "metallic fraction must be finite and in [0, 1]",
            Self::Roughness => "perceptual roughness must be finite and in [0.045, 1]",
            Self::DielectricIor => "dielectric IOR must be finite and in [1, 3]",
            Self::Direction => "surface directions must be finite and nonzero",
            Self::Radiance => "radiance must be finite and nonnegative",
        })
    }
}
impl std::error::Error for SurfaceError {}

impl OpaqueSurface {
    /// `base_color` is a linear-sRGB reflectance, never encoded sRGB or an HDR emitter.
    /// IOR is relative to air; it only controls dielectric normal-incidence reflectance.
    pub fn new(base_color: Vec3, metallic: f64, roughness: f64, dielectric_ior: f64) -> Result<Self, SurfaceError> {
        if !base_color.is_finite() || ![base_color.x, base_color.y, base_color.z].into_iter().all(|c| (0.0..=1.0).contains(&c)) {
            return Err(SurfaceError::BaseColor);
        }
        if !metallic.is_finite() || !(0.0..=1.0).contains(&metallic) {
            return Err(SurfaceError::Metallic);
        }
        if !roughness.is_finite() || !(MIN_ROUGHNESS..=1.0).contains(&roughness) {
            return Err(SurfaceError::Roughness);
        }
        if !dielectric_ior.is_finite() || !(1.0..=3.0).contains(&dielectric_ior) {
            return Err(SurfaceError::DielectricIor);
        }
        Ok(Self { base_color, metallic, roughness, dielectric_ior, clear_coat: None })
    }

    /// Runtime optical layer; never serialized into a document appearance.
    pub fn with_clear_coat(mut self, coat: crate::clear_coat::ClearCoat) -> Self {
        self.clear_coat = if coat.weight() == 0.0 { None } else { Some(coat) };
        self
    }
    pub fn clear_coat(self) -> Option<crate::clear_coat::ClearCoat> {
        self.clear_coat
    }

    pub fn base_color(self) -> Vec3 {
        self.base_color
    }
    pub fn metallic(self) -> f64 {
        self.metallic
    }
    pub fn roughness(self) -> f64 {
        self.roughness
    }
    pub fn dielectric_ior(self) -> f64 {
        self.dielectric_ior
    }

    pub fn normal_reflectance(self) -> Vec3 {
        let ratio = (self.dielectric_ior - 1.0) / (self.dielectric_ior + 1.0);
        Vec3::new(ratio * ratio, ratio * ratio, ratio * ratio).lerp(self.base_color, self.metallic)
    }

    /// BRDF in inverse steradians. Directions point away from the surface.
    /// No incident radiance, cosine weight, exposure or display transfer is included.
    /// Back-facing/tangent directions return zero; malformed directions are errors.
    pub fn brdf(self, normal: Vec3, view: Vec3, light: Vec3) -> Result<Vec3, SurfaceError> {
        let n = unit_direction(normal)?;
        let v = unit_direction(view)?;
        let l = unit_direction(light)?;
        let nv = n.dot(v).clamp(-1.0, 1.0);
        let nl = n.dot(l).clamp(-1.0, 1.0);
        if nv <= 0.0 || nl <= 0.0 {
            return Ok(Vec3::ZERO);
        }
        let h = unit_direction(v + l)?;
        let nh = n.dot(h).clamp(0.0, 1.0);
        let vh = v.dot(h).clamp(0.0, 1.0);
        let alpha = self.roughness * self.roughness;
        let a2 = alpha * alpha;
        // This form avoids subtracting two nearly equal values at nh = 1.
        let d_denom = (1.0 - nh * nh) + nh * nh * a2;
        let distribution = a2 / (PI * d_denom * d_denom);
        // G1 / cos(theta) has a finite grazing limit: no epsilon cosine clamp.
        let visibility = |cos: f64| 2.0 / (cos + (a2 + (1.0 - a2) * cos * cos).sqrt());
        let specular = distribution * 0.25 * visibility(nv) * visibility(nl);
        let f0 = self.normal_reflectance();
        let fresnel = |cos: f64| f0 + (Vec3::new(1.0, 1.0, 1.0) - f0) * (1.0 - cos).powi(5);
        let f = fresnel(vh);
        // Symmetric entry/exit attenuation avoids adding a full Lambert substrate
        // to grazing reflection. Internal reflection / multi-scattering is omitted.
        let fv = fresnel(nv);
        let fl = fresnel(nl);
        let diffuse = self.base_color * ((1.0 - self.metallic) / PI);
        let base = Vec3::new(
            f.x * specular + (1.0 - fv.x) * (1.0 - fl.x) * diffuse.x,
            f.y * specular + (1.0 - fv.y) * (1.0 - fl.y) * diffuse.y,
            f.z * specular + (1.0 - fv.z) * (1.0 - fl.z) * diffuse.z,
        );
        Ok(self.clear_coat.map_or(base, |coat| coat.layer(base, nv, nl, nh, vh)))
    }
}

/// Exact piecewise sRGB decoding of an existing 8-bit color channel.
pub fn srgb8_to_linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Normalize finite directions without overflow or an arbitrary magnitude threshold.
pub fn unit_direction(v: Vec3) -> Result<Vec3, SurfaceError> {
    if !v.is_finite() {
        return Err(SurfaceError::Direction);
    }
    let scale = v.x.abs().max(v.y.abs()).max(v.z.abs());
    if scale == 0.0 {
        return Err(SurfaceError::Direction);
    }
    let scaled = v / scale;
    Ok(scaled / scaled.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(metallic: f64, roughness: f64, ior: f64) -> OpaqueSurface {
        OpaqueSurface::new(Vec3::new(0.8, 0.5, 0.2), metallic, roughness, ior).unwrap()
    }

    #[test]
    fn admission_rejects_nonfinite_out_of_range_and_delta_mirrors() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01, 1.01] {
            assert!(OpaqueSurface::new(Vec3::new(bad, 0.0, 0.0), 0.0, 0.5, 1.5).is_err());
            assert!(OpaqueSurface::new(Vec3::ZERO, bad, 0.5, 1.5).is_err());
        }
        for bad in [f64::NAN, f64::INFINITY, 0.0, MIN_ROUGHNESS - 1e-6, 1.01] {
            assert!(OpaqueSurface::new(Vec3::ZERO, 0.0, bad, 1.5).is_err());
        }
        for bad in [f64::NAN, f64::INFINITY, 0.99, 3.01] {
            assert!(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.5, bad).is_err());
        }
        assert!(OpaqueSurface::new(Vec3::ZERO, 0.0, MIN_ROUGHNESS, 1.0).is_ok());
    }

    #[test]
    fn analytic_normal_incidence_dielectric_and_conductor_limits() {
        let dielectric = surface(0.0, 1.0, 1.5);
        assert!((dielectric.normal_reflectance().x - 0.04).abs() < 1e-14);
        let conductor = surface(1.0, 1.0, 1.5);
        assert_eq!(conductor.normal_reflectance(), conductor.base_color());
        let actual = conductor.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap();
        assert!((actual - conductor.base_color() / (4.0 * PI)).len() < 1e-14);
        let index_matched = surface(0.0, 1.0, 1.0);
        assert!((index_matched.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap() - index_matched.base_color() / PI).len() < 1e-14);
    }

    #[test]
    fn reciprocal_and_finite_at_grazing_and_extreme_direction_scales() {
        for metallic in [0.0, 0.5, 1.0] {
            for roughness in [MIN_ROUGHNESS, 0.5, 1.0] {
                let s = surface(metallic, roughness, 3.0);
                for (v, l) in [
                    (Vec3::new(0.4, 0.1, 1.0), Vec3::new(-0.3, 0.2, 0.8)),
                    (Vec3::new(1.0, 0.0, 1e-300), Vec3::new(-1.0, 0.0, 1e-300)),
                    (Vec3::new(1e308, 1e308, 1e308), Vec3::new(-1e-300, 0.0, 1e-300)),
                ] {
                    let a = s.brdf(Vec3::Z, v, l).unwrap();
                    let b = s.brdf(Vec3::Z, l, v).unwrap();
                    assert!(a.is_finite() && a.x >= 0.0 && a.y >= 0.0 && a.z >= 0.0);
                    assert!((a - b).len() <= 1e-10 * (1.0 + a.len()));
                }
            }
        }
        let s = surface(0.0, 0.5, 1.5);
        assert_eq!(s.brdf(Vec3::Z, Vec3::Z, -Vec3::Z).unwrap(), Vec3::ZERO);
        for bad in [Vec3::ZERO, Vec3::new(f64::NAN, 0.0, 1.0)] {
            assert_eq!(s.brdf(bad, Vec3::Z, Vec3::Z), Err(SurfaceError::Direction));
            assert_eq!(s.brdf(Vec3::Z, bad, Vec3::Z), Err(SurfaceError::Direction));
            assert_eq!(s.brdf(Vec3::Z, Vec3::Z, bad), Err(SurfaceError::Direction));
        }
    }

    #[test]
    fn deterministic_hemisphere_reflectance_does_not_create_energy() {
        // Midpoint quadrature of f * cos(theta) d(omega), not a universal proof.
        // Roughness >= .25 resolves the lobe on this grid; narrower lobes are not
        // certified by this quadrature. Analytic and finite-input tests are separate.
        const MU: usize = 256;
        const PHI: usize = 256;
        for metallic in [0.0, 0.5, 1.0] {
            for roughness in [0.25, 0.5, 1.0] {
                for ior in [1.0, 1.5, 3.0] {
                    for cosine in [0.2_f64, 1.0] {
                        let s = OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), metallic, roughness, ior).unwrap();
                        let v = Vec3::new((1.0 - cosine * cosine).sqrt(), 0.0, cosine);
                        let mut energy = Vec3::ZERO;
                        for i in 0..MU {
                            let mu = (i as f64 + 0.5) / MU as f64;
                            let radius = (1.0 - mu * mu).sqrt();
                            for j in 0..PHI {
                                let phi = 2.0 * PI * (j as f64 + 0.5) / PHI as f64;
                                energy += s.brdf(Vec3::Z, v, Vec3::new(radius * phi.cos(), radius * phi.sin(), mu)).unwrap() * mu;
                            }
                        }
                        energy = energy * (2.0 * PI / (MU * PHI) as f64);
                        assert!(energy.is_finite() && energy.x <= 1.025, "m={metallic}, r={roughness}, ior={ior}, nv={cosine}: {energy:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn srgb_conversion_includes_the_linear_toe() {
        assert_eq!(srgb8_to_linear(0), 0.0);
        assert_eq!(srgb8_to_linear(255), 1.0);
        assert!((srgb8_to_linear(10) - (10.0 / 255.0) / 12.92).abs() < 1e-15);
        assert!((srgb8_to_linear(128) - 0.21586050011389926).abs() < 1e-14);
    }
}
