//! Bounded optical data and original f64 dielectric transport oracles. Units are mm.
use solidcraft_geom::Vec3;

use crate::surface::{SurfaceError, unit_direction};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transmission {
    absorption_per_mm: Vec3,
    thin_wall_thickness_mm: Option<f64>,
    shadow_transmission: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransmissionError {
    Absorption,
    Distance,
    Thickness,
    Angle,
    Ior,
    Direction,
}
impl std::fmt::Display for TransmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Absorption => "absorption_per_mm must be finite RGB coefficients in [0, 100] per mm",
            Self::Distance => "geometric absorption distance must be finite and nonnegative",
            Self::Thickness => "thin-sheet thickness must be finite and in [0, 10000] mm",
            Self::Angle => "incident cosine must be finite and in [0, 1]",
            Self::Ior => "medium IOR must be finite and in [1, 3]",
            Self::Direction => "incident direction must face the finite nonzero interface normal",
        })
    }
}
impl std::error::Error for TransmissionError {}

impl Transmission {
    pub fn new(absorption_per_mm: Vec3, thin_wall_thickness_mm: Option<f64>) -> Result<Self, TransmissionError> {
        if !absorption_per_mm.is_finite()
            || ![absorption_per_mm.x, absorption_per_mm.y, absorption_per_mm.z].into_iter().all(|v| (0.0..=100.0).contains(&v))
        {
            return Err(TransmissionError::Absorption);
        }
        if thin_wall_thickness_mm.is_some_and(|v| !v.is_finite() || !(0.0..=10000.0).contains(&v)) {
            return Err(TransmissionError::Thickness);
        }
        Ok(Self { absorption_per_mm, thin_wall_thickness_mm, shadow_transmission: thin_wall_thickness_mm.is_some() })
    }
    /// Thin sheets enable straight shadow transmission by default; solids opt in.
    pub fn with_shadow_transmission(mut self, enabled: bool) -> Self {
        self.shadow_transmission = enabled;
        self
    }
    pub fn shadow_transmission(self) -> bool {
        self.shadow_transmission
    }
    pub fn thin_wall_thickness_mm(self) -> Option<f64> {
        self.thin_wall_thickness_mm
    }
    pub fn extinction(self) -> Vec3 {
        self.absorption_per_mm
    }
    pub fn attenuation(self, distance_mm: f64) -> Result<Vec3, TransmissionError> {
        if !distance_mm.is_finite() || distance_mm < 0.0 {
            return Err(TransmissionError::Distance);
        }
        let s = self.extinction();
        Ok(Vec3::new((-s.x * distance_mm).exp(), (-s.y * distance_mm).exp(), (-s.z * distance_mm).exp()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fresnel {
    pub s: f64,
    pub p: f64,
    /// None at total internal reflection.
    pub transmitted_cosine: Option<f64>,
}
impl Fresnel {
    pub fn reflectance(self) -> f64 {
        (self.s + self.p) * 0.5
    }
}

/// Exact unpolarized power reflectance at a lossless dielectric interface.
pub fn fresnel(cosine: f64, incident_ior: f64, transmitted_ior: f64) -> Result<Fresnel, TransmissionError> {
    if !cosine.is_finite() || !(0.0..=1.0).contains(&cosine) {
        return Err(TransmissionError::Angle);
    }
    if [incident_ior, transmitted_ior].into_iter().any(|v| !v.is_finite() || !(1.0..=3.0).contains(&v)) {
        return Err(TransmissionError::Ior);
    }
    if incident_ior == transmitted_ior {
        return Ok(Fresnel { s: 0.0, p: 0.0, transmitted_cosine: Some(cosine) });
    }
    let eta = incident_ior / transmitted_ior;
    let sin2 = eta * eta * (1.0 - cosine * cosine).max(0.0);
    if sin2 >= 1.0 {
        return Ok(Fresnel { s: 1.0, p: 1.0, transmitted_cosine: None });
    }
    let ct = (1.0 - sin2).sqrt();
    let rs = (incident_ior * cosine - transmitted_ior * ct) / (incident_ior * cosine + transmitted_ior * ct);
    let rp = (transmitted_ior * cosine - incident_ior * ct) / (transmitted_ior * cosine + incident_ior * ct);
    Ok(Fresnel { s: rs * rs, p: rp * rp, transmitted_cosine: Some(ct) })
}

/// Incident direction points toward the surface; normal points into the incident medium.
pub fn refract(incident: Vec3, normal: Vec3, incident_ior: f64, transmitted_ior: f64) -> Result<Option<Vec3>, TransmissionError> {
    let direction_error = |_: SurfaceError| TransmissionError::Direction;
    let d = unit_direction(incident).map_err(direction_error)?;
    let n = unit_direction(normal).map_err(direction_error)?;
    let cosine = -d.dot(n);
    if cosine < 0.0 {
        return Err(TransmissionError::Direction);
    }
    let f = fresnel(cosine.min(1.0), incident_ior, transmitted_ior)?;
    Ok(f.transmitted_cosine.map(|ct| {
        let eta = incident_ior / transmitted_ior;
        d * eta + n * (eta * cosine - ct)
    }))
}

/// Incoherent parallel slab, including all repeated internal reflections. The two
/// external media have the same IOR. Returns linear RGB (reflectance, transmittance).
pub fn slab(
    transmission: Transmission,
    thickness_mm: f64,
    cosine: f64,
    outside_ior: f64,
    inside_ior: f64,
) -> Result<(Vec3, Vec3), TransmissionError> {
    if !thickness_mm.is_finite() || !(0.0..=10000.0).contains(&thickness_mm) {
        return Err(TransmissionError::Thickness);
    }
    let f = fresnel(cosine, outside_ior, inside_ior)?;
    let Some(ct) = f.transmitted_cosine.filter(|v| *v > 0.0) else {
        return Ok((Vec3::new(1.0, 1.0, 1.0), Vec3::ZERO));
    };
    let a = transmission.attenuation(thickness_mm / ct)?;
    let channel = |r: f64, a: f64| {
        let denominator = 1.0 - r * r * a * a;
        if denominator <= 0.0 {
            return (1.0, 0.0);
        }
        (r + (1.0 - r).powi(2) * r * a * a / denominator, (1.0 - r).powi(2) * a / denominator)
    };
    // A parallel slab preserves polarization. Sum each geometric series before
    // averaging s/p, rather than repeatedly applying the unpolarized mean.
    let average = |a: f64| {
        let (rs, ts) = channel(f.s, a);
        let (rp, tp) = channel(f.p, a);
        ((rs + rp) * 0.5, (ts + tp) * 0.5)
    };
    let (rx, tx) = average(a.x);
    let (ry, ty) = average(a.y);
    let (rz, tz) = average(a.z);
    Ok((Vec3::new(rx, ry, rz), Vec3::new(tx, ty, tz)))
}

/// Explicit straight-through solid shadow approximation. Both interface losses
/// use reciprocal external-angle Fresnel; `distance_mm` is the actual geometric
/// entry/exit interval. This omits refraction and repeated internal reflections.
pub fn solid_shadow(transmission: Transmission, distance_mm: f64, entry_cosine: f64, exit_cosine: f64, ior: f64) -> Result<Vec3, TransmissionError> {
    let entry = fresnel(entry_cosine, 1.0, ior)?.reflectance();
    let exit = fresnel(exit_cosine, 1.0, ior)?.reflectance();
    Ok(transmission.attenuation(distance_mm)? * ((1.0 - entry) * (1.0 - exit)))
}

#[cfg(test)]
mod shadow_tests {
    use super::*;
    #[test]
    fn defaults_and_reciprocal_solid_shadow_match_analytic_beer() {
        let sigma = Vec3::new(-0.5_f64.ln(), -0.8_f64.ln(), 0.0);
        let solid = Transmission::new(sigma, None).unwrap();
        let sheet = Transmission::new(sigma, Some(1.0)).unwrap();
        assert!(!solid.shadow_transmission());
        assert!(sheet.shadow_transmission());
        assert!(solid.with_shadow_transmission(true).shadow_transmission());
        assert!(!sheet.with_shadow_transmission(false).shadow_transmission());
        assert!((solid_shadow(solid, 1.0, 1.0, 1.0, 1.5).unwrap() - Vec3::new(0.5, 0.8, 1.0) * 0.96 * 0.96).len() < 1e-14);
        assert_eq!(solid_shadow(solid, 1.0, 0.2, 0.8, 1.5).unwrap(), solid_shadow(solid, 1.0, 0.8, 0.2, 1.5).unwrap());
        for ior in [1.0, 1.5, 3.0] {
            for cosine in [0.0, 0.02, 0.5, 1.0] {
                let shadow = solid_shadow(solid, 1.0, cosine, cosine, ior).unwrap();
                assert!(shadow.is_finite() && shadow.x >= 0.0 && shadow.z <= 1.0);
            }
        }
        assert!(solid_shadow(solid, f64::NAN, 1.0, 1.0, 1.5).is_err());
    }
}

#[cfg(test)]
#[path = "dielectric_oracle_tests.rs"]
mod tests;
