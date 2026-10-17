//! Original MIT OR Apache-2.0 analytic fixtures for the shared CPU/GPU dielectric contract.
//! Provenance: elementary Fresnel s/p equations, Snell law and Beer-Lambert attenuation.
//! No external assets or renderer outputs. Tolerances are absolute f64 errors stated per assertion.
//! Import this test module with the optical math module; only the API adapter needs changing.
use super::*;

#[test]
fn oblique_polarizations_and_snell_match_independent_angle_equations() {
    let brewster = 1.5_f64.atan();
    for angle in [0.3, brewster - 1e-4, brewster, brewster + 1e-4, 1.2] {
        let transmitted_angle = (angle.sin() / 1.5).asin();
        let expected_s = ((angle - transmitted_angle).sin() / (angle + transmitted_angle).sin()).powi(2);
        let expected_p = ((angle - transmitted_angle).tan() / (angle + transmitted_angle).tan()).powi(2);
        let actual = fresnel(angle.cos(), 1.0, 1.5).unwrap();
        assert!((actual.s - expected_s).abs() < 1e-14);
        assert!((actual.p - expected_p).abs() < 1e-14);
        let incident = Vec3::new(angle.sin(), 0.0, -angle.cos());
        let outgoing = refract(incident, Vec3::Z, 1.0, 1.5).unwrap().unwrap();
        assert!((outgoing.x - transmitted_angle.sin()).abs() < 1e-14);
        assert!((outgoing.z + transmitted_angle.cos()).abs() < 1e-14);
        let reversed = refract(-outgoing, -Vec3::Z, 1.5, 1.0).unwrap().unwrap();
        assert!((reversed + incident).len() < 1e-13);
    }
    let critical = (1.0_f64 / 1.5).asin();
    assert!(fresnel((critical - 1e-5).cos(), 1.5, 1.0).unwrap().transmitted_cosine.is_some());
    assert!(fresnel((critical + 1e-5).cos(), 1.5, 1.0).unwrap().transmitted_cosine.is_none());
    assert!(refract(Vec3::new(0.9, 0.0, -(1.0_f64 - 0.81).sqrt()), Vec3::Z, 1.5, 1.0).unwrap().is_none());
    let zero_sheet = Transmission::new(Vec3::new(100.0, 100.0, 100.0), Some(0.0)).unwrap();
    let (r, t) = slab(zero_sheet, 0.0, 1.0, 1.0, 1.5).unwrap();
    assert!((r.x + t.x - 1.0).abs() < 1e-14);
    assert!((t.x - 0.96_f64.powi(2) / (1.0 - 0.04_f64.powi(2))).abs() < 1e-14);
}

#[test]
fn exact_fresnel_brewster_critical_and_index_matched_limits() {
    for n in [1.0_f64, 1.33, 1.5, 3.0] {
        assert!((fresnel(1.0, 1.0, n).unwrap().reflectance() - ((n - 1.0) / (n + 1.0)).powi(2)).abs() < 1e-15);
        assert!(fresnel(n.atan().cos(), 1.0, n).unwrap().p < 1e-30);
    }
    assert_eq!(fresnel(0.5, 1.5, 1.0).unwrap().transmitted_cosine, None);
    assert_eq!(fresnel(0.0, 1.0, 1.0).unwrap().reflectance(), 0.0);
    assert_eq!(fresnel(0.0, 1.0, 1.5).unwrap().reflectance(), 1.0);
    for cosine in [0.01, 0.3, 0.9, 1.0] {
        let f = fresnel(cosine, 1.0, 1.5).unwrap();
        let reverse = fresnel(f.transmitted_cosine.unwrap(), 1.5, 1.0).unwrap();
        assert!((f.reflectance() - reverse.reflectance()).abs() < 1e-12);
    }
}

#[test]
fn beer_lambert_slab_and_lossless_energy() {
    let t = Transmission::new(Vec3::new(-0.5_f64.ln(), -0.8_f64.ln(), 0.0) / 10.0, None).unwrap();
    assert!((t.attenuation(20.0).unwrap() - Vec3::new(0.25, 0.64, 1.0)).len() < 1e-15);
    let (r, through) = slab(t, 10.0, 1.0, 1.0, 1.5).unwrap();
    assert!((through.x - 0.96_f64.powi(2) * 0.5 / (1.0 - 0.04_f64.powi(2) * 0.25)).abs() < 1e-15);
    assert!((r.z + through.z - 1.0).abs() < 1e-15);
    assert!(r.x + through.x < 1.0);
    let clear = Transmission::new(Vec3::ZERO, None).unwrap();
    for cosine in [0.001, 0.1, 0.5, 1.0] {
        let (r, t) = slab(clear, 1.0, cosine, 1.0, 1.5).unwrap();
        assert!((r.x + t.x - 1.0).abs() < 1e-13);
    }
}

#[test]
fn hostile_optics_are_rejected_without_clamping() {
    for v in [f64::NAN, f64::INFINITY, -1.0, 100.01] {
        assert!(Transmission::new(Vec3::new(v, 0.0, 0.0), None).is_err());
    }
    for v in [f64::NAN, f64::INFINITY, -1.0, 10000.01] {
        assert!(Transmission::new(Vec3::ZERO, Some(v)).is_err());
    }
    assert!(Transmission::new(Vec3::ZERO, Some(0.0)).is_ok());
    assert!(fresnel(f64::NAN, 1.0, 1.5).is_err());
    assert!(fresnel(0.5, f64::INFINITY, 1.5).is_err());
    assert!(refract(Vec3::ZERO, Vec3::Z, 1.0, 1.5).is_err());
    assert!(refract(Vec3::Z, Vec3::Z, 1.0, 1.5).is_err());
}
