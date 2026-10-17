//! Original single-scattering GGX dielectric coat over the opaque surface oracle.
//! The substrate is attenuated at entry and exit; internal inter-layer scattering
//! and refraction of substrate directions are omitted. This is a bounded preview
//! model, not a full layered random walk. See docs/render-clear-coat.md.
use crate::surface::{MIN_ROUGHNESS, OpaqueSurface, SurfaceError, unit_direction};
use solidcraft_geom::Vec3;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClearCoat {
    weight: f64,
    roughness: f64,
    ior: f64,
}

impl ClearCoat {
    pub fn new(weight: f64, roughness: f64, ior: f64) -> Result<Self, SurfaceError> {
        if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
            return Err(SurfaceError::CoatWeight);
        }
        if !roughness.is_finite() || !(MIN_ROUGHNESS..=1.0).contains(&roughness) {
            return Err(SurfaceError::Roughness);
        }
        if !ior.is_finite() || !(1.0..=3.0).contains(&ior) {
            return Err(SurfaceError::DielectricIor);
        }
        Ok(Self { weight, roughness, ior })
    }
    pub fn weight(self) -> f64 {
        self.weight
    }
    pub fn roughness(self) -> f64 {
        self.roughness
    }
    pub fn ior(self) -> f64 {
        self.ior
    }
    fn fresnel(self, c: f64) -> f64 {
        let ratio = (self.ior - 1.0) / (self.ior + 1.0);
        let f0 = ratio * ratio;
        f0 + (1.0 - f0) * (1.0 - c).powi(5)
    }
    /// Upper bound on coat directional reflectance: over the upper hemisphere,
    /// v.h >= nv/sqrt(2*(1+sqrt(1-nv^2))). Schlick decreases with v.h and
    /// the unit-Fresnel single-scattering Smith GGX lobe reflects at most one.
    fn energy_bound(self, cosine: f64) -> f64 {
        self.fresnel(cosine / (2.0 * (1.0 + (1.0 - cosine * cosine).max(0.0).sqrt())).sqrt())
    }
    pub(crate) fn layer(self, base: Vec3, nv: f64, nl: f64, nh: f64, vh: f64) -> Vec3 {
        let a2 = self.roughness.powi(4);
        let denom = (1.0 - nh * nh) + nh * nh * a2;
        let d = a2 / (PI * denom * denom);
        let visibility = |c: f64| 2.0 / (c + (a2 + (1.0 - a2) * c * c).sqrt());
        let coat = self.weight * self.fresnel(vh) * d * 0.25 * visibility(nv) * visibility(nl);
        let attenuation = (1.0 - self.weight * self.energy_bound(nv)) * (1.0 - self.weight * self.energy_bound(nl));
        base * attenuation + Vec3::new(coat, coat, coat)
    }
}

/// Conditional opaque-lobe PDF, including the coat and base mixture. Rejected
/// below-surface microfacet samples retain their mass; never renormalize them.
pub fn surface_pdf(surface: OpaqueSurface, normal: Vec3, view: Vec3, light: Vec3) -> Result<f64, SurfaceError> {
    let n = unit_direction(normal)?;
    let v = unit_direction(view)?;
    let l = unit_direction(light)?;
    let nl = n.dot(l);
    if nl <= 0.0 || n.dot(v) <= 0.0 {
        return Ok(0.0);
    }
    let h = unit_direction(v + l)?;
    let nh = n.dot(h).clamp(0.0, 1.0);
    let vh = v.dot(h).max(1e-20);
    let ggx = |roughness: f64| {
        let a2 = roughness.powi(4);
        let d = (1.0 - nh * nh) + nh * nh * a2;
        a2 / (PI * d * d) * nh / (4.0 * vh)
    };
    let base = 0.5 * nl / PI + 0.5 * ggx(surface.roughness());
    Ok(surface.clear_coat().map_or(base, |coat| {
        let p = coat.weight() * 0.5;
        (1.0 - p) * base + p * ggx(coat.roughness())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_zero_coat_and_reciprocity() {
        for bad in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(ClearCoat::new(bad, 0.2, 1.5).is_err());
        }
        for bad in [0.0, 0.044, 1.1, f64::NAN] {
            assert!(ClearCoat::new(1.0, bad, 1.5).is_err());
        }
        for bad in [0.99, 3.01, f64::NAN] {
            assert!(ClearCoat::new(1.0, 0.2, bad).is_err());
        }
        let base = OpaqueSurface::new(Vec3::new(0.01, 0.02, 0.03), 0.0, 0.3, 1.5).unwrap();
        assert_eq!(base, base.with_clear_coat(ClearCoat::new(0.0, 0.1, 1.5).unwrap()));
        let coated = base.with_clear_coat(ClearCoat::new(1.0, 0.12, 1.5).unwrap());
        let v = Vec3::new(0.5, 0.0, 0.8);
        let l = Vec3::new(-0.3, 0.2, 0.6);
        assert!((coated.brdf(Vec3::Z, v, l).unwrap() - coated.brdf(Vec3::Z, l, v).unwrap()).len() < 1e-12);
        assert!(coated.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap().x > base.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap().x);
        // Normal incidence: D=1/(pi*roughness^4), G=1, Fresnel=.04.
        let expected = base.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap() * (1.0 - ClearCoat::new(1.0, 0.12, 1.5).unwrap().energy_bound(1.0)).powi(2)
            + Vec3::new(1.0, 1.0, 1.0) * (0.04 / (4.0 * PI * 0.12_f64.powi(4)));
        assert!((coated.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap() - expected).len() < 1e-10);
    }
    #[test]
    fn layered_white_furnace_and_pdf_quadrature() {
        // Deterministic quadrature resolves these lobes; it is not a proof for
        // arbitrary roughness. Narrow coat lobes have separate GPU oracle checks.
        for roughness in [0.25, 0.5, 1.0] {
            for metallic in [0.0, 1.0] {
                for cosine in [0.2_f64, 1.0] {
                    let s = OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), metallic, 0.5, 1.5)
                        .unwrap()
                        .with_clear_coat(ClearCoat::new(1.0, roughness, 1.5).unwrap());
                    let v = Vec3::new((1.0 - cosine * cosine).sqrt(), 0.0, cosine);
                    let mut energy = 0.0;
                    let mut probability = 0.0;
                    for i in 0..256 {
                        let mu = (f64::from(i) + 0.5) / 256.0;
                        let r = (1.0 - mu * mu).sqrt();
                        for j in 0..256 {
                            let phi = 2.0 * PI * (f64::from(j) + 0.5) / 256.0;
                            let l = Vec3::new(r * phi.cos(), r * phi.sin(), mu);
                            energy += s.brdf(Vec3::Z, v, l).unwrap().x * mu;
                            probability += surface_pdf(s, Vec3::Z, v, l).unwrap();
                        }
                    }
                    energy *= 2.0 * PI / 65536.0;
                    probability *= 2.0 * PI / 65536.0;
                    assert!(energy <= 1.025, "roughness={roughness} metal={metallic} cosine={cosine} energy={energy}");
                    assert!((0.2..=1.005).contains(&probability), "{probability}");
                }
            }
        }
    }

    #[test]
    fn conservative_bound_and_layer_furnace_include_shiny_bases_and_grazing() {
        const COUNT: u32 = 4096;
        let mut maximum = 0.0_f64;
        for base_roughness in [0.045, 0.12, 0.5, 1.0] {
            for coat_roughness in [0.12, 0.5, 1.0] {
                for weight in [0.25, 1.0] {
                    for ior in [1.0, 1.5, 3.0] {
                        for cosine in [0.02_f64, 0.2, 1.0] {
                            for metallic in [0.0, 1.0] {
                                let coat = ClearCoat::new(weight, coat_roughness, ior).unwrap();
                                let surface =
                                    OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), metallic, base_roughness, 1.5).unwrap().with_clear_coat(coat);
                                let v = Vec3::new((1.0 - cosine * cosine).sqrt(), 0.0, cosine);
                                let p = weight * 0.5;
                                let mut energy = 0.0;
                                // Integrate each proposal with its mixture probability.
                                // NDF importance quadrature resolves sharp lobes, unlike
                                // a uniform hemisphere grid at roughness 0.045.
                                for (component, probability) in [(0, (1.0 - p) * 0.5), (1, (1.0 - p) * 0.5), (2, p)] {
                                    for i in 0..COUNT {
                                        let x = (f64::from(i) + 0.5) / f64::from(COUNT);
                                        let phi = 2.0 * PI * f64::from(i.reverse_bits()) / 4294967296.0;
                                        let l = if component == 0 {
                                            Vec3::new(x.sqrt() * phi.cos(), x.sqrt() * phi.sin(), (1.0 - x).sqrt())
                                        } else {
                                            let r = if component == 1 { base_roughness } else { coat_roughness };
                                            let c = ((1.0 - x) / (1.0 + (r.powi(4) - 1.0) * x)).sqrt();
                                            let radius = (1.0 - c * c).sqrt();
                                            let h = Vec3::new(radius * phi.cos(), radius * phi.sin(), c);
                                            h * (2.0 * v.dot(h)) - v
                                        };
                                        if l.z <= 0.0 {
                                            continue;
                                        }
                                        let h = unit_direction(v + l).unwrap();
                                        assert!(coat.fresnel(v.dot(h).clamp(0.0, 1.0)) <= coat.energy_bound(cosine) + 1e-12);
                                        let pdf = surface_pdf(surface, Vec3::Z, v, l).unwrap();
                                        energy += probability * surface.brdf(Vec3::Z, v, l).unwrap().x * l.z / pdf / f64::from(COUNT);
                                    }
                                }
                                maximum = maximum.max(energy);
                                assert!(
                                    energy.is_finite() && energy <= 1.003,
                                    "base={base_roughness}, coat={coat_roughness}, w={weight}, ior={ior}, nv={cosine}, m={metallic}: {energy}"
                                );
                            }
                        }
                    }
                }
            }
        }
        eprintln!("maximum conservative layered furnace reflectance: {maximum}");
        for cosine in [0.0, 1e-300, 1e-20, 0.02, 1.0] {
            let bound = ClearCoat::new(1.0, 0.12, 1.5).unwrap().energy_bound(cosine);
            assert!(bound.is_finite() && (0.0..=1.0).contains(&bound));
        }
    }
}
