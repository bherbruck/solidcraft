//! Original bounded specular sampling. All values are linear-light quantities.
use super::{Result, TraceError};
use crate::surface::{OpaqueSurface, unit_direction};
use solidcraft_geom::Vec3;
use std::f64::consts::{PI, TAU};
#[derive(Clone, Copy, Debug)]
pub struct ReflectionSample {
    pub direction: Vec3,
    pub weight: Vec3,
    pub pdf: f64,
}
fn fresnel(surface: OpaqueSurface, cosine: f64) -> Vec3 {
    let f0 = surface.normal_reflectance();
    f0 + (Vec3::new(1.0, 1.0, 1.0) - f0) * (1.0 - cosine.clamp(0.0, 1.0)).powi(5)
}
pub fn mirror(surface: OpaqueSurface, normal: Vec3, view: Vec3) -> Result<Option<ReflectionSample>> {
    let n = unit_direction(normal).map_err(|_| TraceError::InvalidInput("mirror normal"))?;
    let v = unit_direction(view).map_err(|_| TraceError::InvalidInput("mirror view"))?;
    let nv = n.dot(v);
    if nv <= 0.0 {
        return Ok(None);
    }
    Ok(Some(ReflectionSample { direction: n * (2.0 * nv) - v, weight: fresnel(surface, nv), pdf: 1.0 }))
}
/// GGX normal-distribution sampling; rejected below-surface events contribute zero.
/// PDF is D(h)*n.h/(4*v.h), matching this sampler, without retry/renormalization.
pub fn rough_specular(surface: OpaqueSurface, normal: Vec3, view: Vec3, u: [f64; 2]) -> Result<Option<ReflectionSample>> {
    if !u.into_iter().all(|v| v.is_finite() && (0.0..1.0).contains(&v)) {
        return Err(TraceError::InvalidInput("GGX variates outside [0,1)"));
    }
    let n = unit_direction(normal).map_err(|_| TraceError::InvalidInput("GGX normal"))?;
    let v = unit_direction(view).map_err(|_| TraceError::InvalidInput("GGX view"))?;
    let nv = n.dot(v);
    if nv <= 0.0 {
        return Ok(None);
    }
    let alpha = surface.roughness().powi(2);
    let a2 = alpha * alpha;
    let cos = ((1.0 - u[0]) / (1.0 + (a2 - 1.0) * u[0])).sqrt();
    let sin = (1.0 - cos * cos).max(0.0).sqrt();
    let phi = TAU * u[1];
    let axis = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let tangent = unit_direction(axis.cross(n)).map_err(|_| TraceError::InvalidInput("GGX tangent"))?;
    let bitangent = n.cross(tangent);
    let h = unit_direction(tangent * (sin * phi.cos()) + bitangent * (sin * phi.sin()) + n * cos)
        .map_err(|_| TraceError::InvalidInput("GGX half vector"))?;
    let vh = v.dot(h);
    if vh <= 0.0 {
        return Ok(None);
    }
    let direction = h * (2.0 * vh) - v;
    let nl = n.dot(direction);
    if nl <= 0.0 {
        return Ok(None);
    }
    let nh = n.dot(h).clamp(0.0, 1.0);
    let denominator = (1.0 - nh * nh) + nh * nh * a2;
    let distribution = a2 / (PI * denominator * denominator);
    let pdf = distribution * nh / (4.0 * vh);
    let visibility = |c: f64| 2.0 / (c + (a2 + (1.0 - a2) * c * c).sqrt());
    let specular = distribution * 0.25 * visibility(nv) * visibility(nl);
    let weight = fresnel(surface, vh) * (specular * nl / pdf);
    if !pdf.is_finite() || pdf <= 0.0 || !weight.is_finite() {
        return Err(TraceError::InvalidInput("nonfinite GGX sample"));
    }
    Ok(Some(ReflectionSample { direction, weight, pdf }))
}
/// Bounded stratified reflection sequence, keyed by captured pixel/bounce identity.
/// The first coordinate visits each of R radial strata exactly once. The second
/// uses a digitally shifted base-two radical inverse: exact dyadic coverage only
/// for power-of-two R. This is not a progressive prefix or a variance guarantee.
/// BSDF/PDF, null-event accounting, R-attempt and bounce bounds are unchanged.
/// Actual outgoing paths, null samples and work can differ with the directions.
pub fn variates(pixel: u64, sample: u64, bounce: u64, samples: u64) -> Result<[f64; 2]> {
    if !(1..=64).contains(&samples) || sample >= samples {
        return Err(TraceError::InvalidInput("reflection sample ordinal/count outside 1..64 strata"));
    }
    fn mix(mut x: u64) -> u64 {
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58476d1ce4e5b9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94d049bb133111eb);
        x ^ (x >> 31)
    }
    let key = pixel.wrapping_mul(0x9e3779b97f4a7c15) ^ bounce.wrapping_mul(0x632be59bd9b4e019);
    // A 32-bit midpoint jitter stays at least 2^-33 from either radial-cell
    // edge. Even adding ordinal63 and dividing by64 cannot round to an edge.
    let radial_jitter = ((mix(key) >> 32) as f64 + 0.5) / (1u64 << 32) as f64;
    let radial = (sample as f64 + radial_jitter) / samples as f64;
    let azimuth_bits = (sample.reverse_bits() >> 12) ^ (mix(key ^ 0xa0761d6478bd642f) >> 12);
    let azimuth = (azimuth_bits as f64 + 0.5) / (1u64 << 52) as f64;
    Ok([radial, azimuth])
}

// Preserve the original hashed sequence as a test-only comparison control.
#[cfg(test)]
fn hash_variates(pixel: u64, sample: u64, bounce: u64) -> [f64; 2] {
    fn mix(mut x: u64) -> u64 {
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58476d1ce4e5b9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94d049bb133111eb);
        x ^ (x >> 31)
    }
    let key = pixel.wrapping_mul(0x9e3779b97f4a7c15) ^ sample.wrapping_mul(0xd1b54a32d192ed03) ^ bounce.wrapping_mul(0x632be59bd9b4e019);
    let value = |x: u64| ((mix(x) >> 12) as f64 + 0.5) / (1u64 << 52) as f64;
    [value(key), value(key ^ 0xa0761d6478bd642f)]
}

#[cfg(test)]
#[path = "reflection_stratified_tests.rs"]
mod stratified_tests;
#[cfg(test)]
mod tests {
    use super::{OpaqueSurface, PI, Vec3, hash_variates as variates, mirror, rough_specular};
    fn white() -> OpaqueSurface {
        OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 1.0, 1.5).unwrap()
    }
    #[test]
    fn mirror_is_explicit_delta_with_normal_fresnel_and_reflection_law() {
        let surface = OpaqueSurface::new(Vec3::new(0.8, 0.5, 0.2), 0.0, 0.045, 1.5).unwrap();
        let sample = mirror(surface, Vec3::Z, Vec3::Z).unwrap().unwrap();
        assert_eq!(sample.direction, Vec3::Z);
        assert!((sample.weight.x - 0.04).abs() < 1e-14);
        let view = Vec3::new(0.6, 0.0, 0.8);
        let sample = mirror(surface, Vec3::Z, view).unwrap().unwrap();
        assert!((sample.direction - Vec3::new(-0.6, 0.0, 0.8)).len() < 1e-14);
        assert!(rough_specular(surface, Vec3::Z, view, [0.4, 0.1]).unwrap().is_some());
    }
    #[test]
    fn fully_rough_conductor_has_analytic_pdf_weight_and_null_probability() {
        let sample = rough_specular(white(), Vec3::Z, Vec3::Z, [0.25, 0.2]).unwrap().unwrap();
        assert!((sample.direction.z - 0.5).abs() < 1e-14);
        assert!((sample.pdf - 1.0 / (4.0 * PI)).abs() < 1e-14);
        assert!((sample.weight.x - 2.0 / 3.0).abs() < 1e-14);
        assert!(rough_specular(white(), Vec3::Z, Vec3::Z, [0.75, 0.2]).unwrap().is_none());
        let mut reflectance = 0.0;
        for i in 0..4096 {
            reflectance += rough_specular(white(), Vec3::Z, Vec3::Z, [(i as f64 + 0.5) / 4096.0, 0.3]).unwrap().map_or(0.0, |s| s.weight.x);
        }
        // Independent exact hemispherical integral for alpha=1, F=1, normal view.
        assert!((reflectance / 4096.0 - (1.0 - 2.0_f64.ln())).abs() < 1e-6);
    }
    #[test]
    fn invalid_variates_directions_and_deterministic_open_samples() {
        for bad in [f64::NAN, f64::INFINITY, -0.1, 1.0] {
            assert!(rough_specular(white(), Vec3::Z, Vec3::Z, [bad, 0.2]).is_err());
        }
        assert!(mirror(white(), Vec3::ZERO, Vec3::Z).is_err());
        for pixel in 0..100 {
            let u = variates(pixel, 17, 4);
            assert_eq!(u, variates(pixel, 17, 4));
            assert!(u.into_iter().all(|v| v > 0.0 && v < 1.0));
        }
    }
}
