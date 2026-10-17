use super::*;

fn frame() -> VisualFrame {
    VisualFrame::new(Vec3::Z, Vec3::X).unwrap()
}

fn surface(rx: f64, ry: f64) -> AnisotropicSurface {
    AnisotropicSurface::new(Vec3::new(0.8, 0.5, 0.2), 1.0, rx, ry, 1.5).unwrap()
}

fn close(a: f64, b: f64, tolerance: f64) {
    assert!((a - b).abs() <= tolerance * (1.0 + a.abs().max(b.abs())), "{a} != {b}");
}

fn close_rgb(a: Vec3, b: Vec3, tolerance: f64) {
    close(a.x, b.x, tolerance);
    close(a.y, b.y, tolerance);
    close(a.z, b.z, tolerance);
}

fn hemisphere(mu: f64, phi: f64) -> Vec3 {
    let radius = (1.0 - mu * mu).sqrt();
    Vec3::new(radius * phi.cos(), radius * phi.sin(), mu)
}

#[test]
fn constructor_rejects_hostile_parameters_on_both_axes() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, MIN_ROUGHNESS.next_down(), 1.01] {
        assert!(AnisotropicSurface::new(Vec3::X, 1.0, bad, 0.5, 1.5).is_err());
        assert!(AnisotropicSurface::new(Vec3::X, 1.0, 0.5, bad, 1.5).is_err());
    }
    for bad in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        assert!(AnisotropicSurface::new(Vec3::new(bad, 0.0, 0.0), 1.0, 0.5, 0.8, 1.5).is_err());
        assert!(AnisotropicSurface::new(Vec3::X, bad, 0.5, 0.8, 1.5).is_err());
    }
    for bad in [f64::NAN, 0.99, 3.01] {
        assert!(AnisotropicSurface::new(Vec3::X, 1.0, 0.5, 0.8, bad).is_err());
    }
    assert_eq!(surface(MIN_ROUGHNESS, 1.0).roughness(), [MIN_ROUGHNESS, 1.0]);
}

#[test]
fn frame_is_right_handed_and_requires_an_explicit_orthogonal_axis() {
    let f = VisualFrame::new(Vec3::new(0.0, 0.0, 1e308), Vec3::new(1e-300, 0.0, 0.0)).unwrap();
    assert_eq!(f.normal(), Vec3::Z);
    assert_eq!(f.tangent(), Vec3::X);
    assert_eq!(f.bitangent(), Vec3::Y);
    for (n, t) in [(Vec3::ZERO, Vec3::X), (Vec3::Z, Vec3::Z), (Vec3::Z, Vec3::X + Vec3::Z), (Vec3::Z, Vec3::new(f64::NAN, 0.0, 0.0))] {
        assert!(VisualFrame::new(n, t).is_err());
    }
    let tilted = VisualFrame::new(Vec3::new(0.3, 0.4, 1.0), Vec3::new(0.8, -0.6, 0.0)).unwrap();
    close(tilted.tangent().dot(tilted.normal()), 0.0, 1e-14);
    close_rgb(tilted.tangent().cross(tilted.bitangent()), tilted.normal(), 1e-14);
}

#[test]
fn normal_incidence_matches_independent_ggx_peak() {
    let s = surface(0.3, 0.7);
    let d = 1.0 / (PI * 0.3_f64.powi(2) * 0.7_f64.powi(2));
    close(s.distribution(frame(), Vec3::Z).unwrap(), d, 1e-14);
    close(s.normal_pdf(frame(), Vec3::Z).unwrap(), d, 1e-14);
    close_rgb(s.brdf(frame(), Vec3::Z, Vec3::Z, Vec3::Z).unwrap(), Vec3::new(0.8, 0.5, 0.2) * (d / 4.0), 1e-14);
}

#[test]
fn isotropic_limit_matches_existing_opaque_contract() {
    for r in [MIN_ROUGHNESS, 0.3, 0.7, 1.0] {
        for metallic in [0.0, 0.5, 1.0] {
            let color = Vec3::new(0.8, 0.5, 0.2);
            let a = AnisotropicSurface::new(color, metallic, r, r, 1.5).unwrap();
            let opaque = OpaqueSurface::new(color, metallic, r, 1.5).unwrap();
            for (v, l) in [(Vec3::Z, Vec3::Z), (Vec3::new(0.4, 0.2, 0.8), Vec3::new(-0.2, 0.7, 0.6))] {
                assert_eq!(a.brdf(frame(), Vec3::Z, v, l).unwrap(), opaque.brdf(Vec3::Z, v, l).unwrap());
                if metallic == 1.0 {
                    close_rgb(a.specular_brdf(frame(), Vec3::Z, v, l).unwrap(), opaque.brdf(Vec3::Z, v, l).unwrap(), 1e-10);
                }
            }
        }
    }
}

#[test]
fn reciprocal_for_unequal_axes_and_mixed_metal_fraction() {
    for metallic in [0.0, 0.4, 1.0] {
        let s = AnisotropicSurface::new(Vec3::new(0.8, 0.5, 0.2), metallic, 0.2, 0.9, 2.0).unwrap();
        for (v, l) in [(Vec3::new(0.4, 0.1, 1.0), Vec3::new(-0.3, 0.2, 0.8)), (Vec3::new(1.0, 0.0, 1e-300), Vec3::new(-1.0, 0.0, 1e-300))] {
            close_rgb(s.brdf(frame(), Vec3::Z, v, l).unwrap(), s.brdf(frame(), Vec3::Z, l, v).unwrap(), 1e-12);
        }
    }
}

#[test]
fn ninety_degree_visual_rotation_swaps_the_brushed_response() {
    let s = surface(0.25, 0.8);
    let swapped = surface(0.8, 0.25);
    let rotated = VisualFrame::new(Vec3::Z, Vec3::Y).unwrap();
    let light = Vec3::new(0.6, 0.0, 0.8);
    let initial = s.brdf(frame(), Vec3::Z, Vec3::Z, light).unwrap();
    let turned = s.brdf(rotated, Vec3::Z, Vec3::Z, light).unwrap();
    assert!((initial - turned).len() > 0.01);
    close_rgb(turned, swapped.brdf(frame(), Vec3::Z, Vec3::Z, light).unwrap(), 1e-13);
    let h = s.sample_normal(frame(), [0.3, 0.2]).unwrap();
    let rotated_h = s.sample_normal(rotated, [0.3, 0.2]).unwrap();
    close_rgb(rotated_h, Vec3::new(-h.y, h.x, h.z), 1e-14);
}

#[test]
fn geometric_and_shading_hemispheres_are_independent() {
    let shading = VisualFrame::new(Vec3::new(0.8, 0.0, 0.6), Vec3::Y).unwrap();
    let light = Vec3::new(0.99, 0.0, -0.1);
    let s = surface(0.5, 0.8);
    assert!(shading.normal().dot(light) > 0.0);
    assert_eq!(s.brdf(shading, Vec3::Z, Vec3::Z, light).unwrap(), Vec3::ZERO);
    assert_eq!(s.reflection_pdf(shading, Vec3::Z, Vec3::Z, light).unwrap(), 0.0);
    assert!(s.brdf(frame(), -Vec3::Z, Vec3::Z, Vec3::Z).is_err());
    assert_eq!(s.brdf(frame(), Vec3::Z, Vec3::Z, Vec3::X).unwrap(), Vec3::ZERO);
    assert_eq!(s.brdf(frame(), Vec3::Z, -Vec3::Z, Vec3::Z).unwrap(), Vec3::ZERO);
}

#[test]
fn grazing_and_extreme_finite_direction_magnitudes_remain_finite() {
    for (rx, ry) in [(MIN_ROUGHNESS, 1.0), (1.0, MIN_ROUGHNESS), (MIN_ROUGHNESS, MIN_ROUGHNESS)] {
        let s = surface(rx, ry);
        for (v, l) in [(Vec3::new(1.0, 0.0, 1e-300), Vec3::new(-1.0, 0.0, 1e-300)), (Vec3::new(1e308, 1e308, 1e308), Vec3::new(-1e-300, 0.0, 1e-300))]
        {
            let b = s.brdf(frame(), Vec3::Z, v, l).unwrap();
            assert!(b.is_finite() && b.x >= 0.0 && b.y >= 0.0 && b.z >= 0.0);
        }
        for u in [0.0, f64::from_bits(1), 0.5, 1.0_f64.next_down()] {
            let h = s.sample_normal(frame(), [u, 0.5]).unwrap();
            assert!(h.is_finite() && h.z > 0.0);
            assert!(s.normal_pdf(frame(), h).unwrap().is_finite());
        }
        let view = Vec3::new(1.0, 0.0, 1e-300);
        let sample = s.sample_reflection(frame(), Vec3::Z, view, [0.0, 0.0]).unwrap().unwrap();
        assert!(sample.pdf.is_finite() && sample.pdf > 0.0 && sample.weight.is_finite());
        // A mathematical density beyond f64 is a typed refusal, never clamped.
        assert_eq!(s.sample_reflection(frame(), Vec3::Z, Vec3::new(1.0, 0.0, f64::from_bits(1)), [0.0, 0.0]), Err(AnisotropicError::Arithmetic));
    }
}

#[test]
fn malformed_directions_and_variates_fail_without_clamping() {
    let s = surface(0.5, 0.8);
    for bad in [Vec3::ZERO, Vec3::new(f64::NAN, 0.0, 1.0), Vec3::new(f64::INFINITY, 0.0, 1.0)] {
        assert!(s.brdf(frame(), Vec3::Z, bad, Vec3::Z).is_err());
        assert!(s.brdf(frame(), bad, Vec3::Z, Vec3::Z).is_err());
        assert!(s.brdf(frame(), Vec3::Z, Vec3::Z, bad).is_err());
        assert!(s.normal_pdf(frame(), bad).is_err());
    }
    for bad in [-0.1, 1.0, f64::NAN, f64::INFINITY] {
        assert!(s.sample_reflection(frame(), Vec3::Z, Vec3::Z, [bad, 0.5]).is_err());
        assert!(s.sample_reflection(frame(), Vec3::Z, Vec3::Z, [0.5, bad]).is_err());
    }
}

#[test]
fn sampled_slopes_follow_the_independent_inverse_radial_cdf() {
    let s = surface(0.3, 0.7);
    for u in [0.0, 0.01, 0.3, 0.8, 0.9999] {
        for azimuth in [0.0, 0.2, 0.5, 0.8] {
            let h = s.sample_normal(frame(), [u, azimuth]).unwrap();
            let sx = h.x / (0.3_f64.powi(2) * h.z);
            let sy = h.y / (0.7_f64.powi(2) * h.z);
            let radius2 = sx * sx + sy * sy;
            close(radius2 / (1.0 + radius2), u, 1e-13);
            close_rgb(h, s.sample_normal(frame(), [u, azimuth]).unwrap(), 0.0);
        }
    }
}

#[test]
fn reflection_samples_match_pdf_and_specular_throughput() {
    let s = surface(0.4, 0.8);
    let view = unit_direction(Vec3::new(0.2, 0.3, 1.0)).unwrap();
    let mut accepted = 0;
    let mut null = 0;
    for i in 0..32 {
        for j in 0..32 {
            let uv = [(i as f64 + 0.5) / 32.0, (j as f64 + 0.5) / 32.0];
            match s.sample_reflection(frame(), Vec3::Z, view, uv).unwrap() {
                Some(sample) => {
                    accepted += 1;
                    close(sample.pdf, s.reflection_pdf(frame(), Vec3::Z, view, sample.direction).unwrap(), 1e-13);
                    let expected = s.specular_brdf(frame(), Vec3::Z, view, sample.direction).unwrap() * (sample.direction.z / sample.pdf);
                    close_rgb(sample.weight, expected, 1e-10);
                }
                None => null += 1,
            }
        }
    }
    assert!(accepted > 0 && null > 0);
    assert_eq!(accepted + null, 1024);
}

#[test]
fn unit_roughness_normal_view_has_half_null_mass_and_uniform_reflection_pdf() {
    let s = surface(1.0, 1.0);
    assert!(s.sample_reflection(frame(), Vec3::Z, Vec3::Z, [0.75, 0.3]).unwrap().is_none());
    let sample = s.sample_reflection(frame(), Vec3::Z, Vec3::Z, [0.25, 0.3]).unwrap().unwrap();
    close(sample.pdf, 1.0 / (4.0 * PI), 1e-14);
    let mut accepted = 0;
    for i in 0..1024 {
        accepted += usize::from(s.sample_reflection(frame(), Vec3::Z, Vec3::Z, [(i as f64 + 0.5) / 1024.0, 0.3]).unwrap().is_some());
    }
    assert_eq!(accepted, 512);
    // p(l)=1/(4*pi) over the upper hemisphere: valid mass exactly1/2.
    close(s.reflection_pdf(frame(), Vec3::Z, Vec3::Z, Vec3::Z).unwrap() * (2.0 * PI), 0.5, 1e-14);
    assert_eq!(s.reflection_pdf(frame(), Vec3::Z, Vec3::Z, -Vec3::Z).unwrap(), 0.0);
}

#[test]
fn invisible_microfacets_are_null_and_weights_exclude_the_diffuse_substrate() {
    let s = AnisotropicSurface::new(Vec3::new(0.8, 0.5, 0.2), 0.0, 1.0, 1.0, 1.5).unwrap();
    let view = unit_direction(Vec3::new(0.99, 0.0, 0.1)).unwrap();
    assert!(s.sample_reflection(frame(), Vec3::Z, view, [0.9, 0.5]).unwrap().is_none());
    let sample = s.sample_reflection(frame(), Vec3::Z, Vec3::Z, [0.1, 0.2]).unwrap().unwrap();
    let scale = sample.direction.z / sample.pdf;
    let specular = s.specular_brdf(frame(), Vec3::Z, Vec3::Z, sample.direction).unwrap() * scale;
    let total = s.brdf(frame(), Vec3::Z, Vec3::Z, sample.direction).unwrap() * scale;
    close_rgb(sample.weight, specular, 1e-13);
    assert!(total.x > sample.weight.x + 0.1);
    assert_eq!(s.normal_pdf(frame(), -Vec3::Z).unwrap(), 0.0);
    assert_eq!(s.normal_pdf(frame(), Vec3::X).unwrap(), 0.0);
}

#[test]
fn quadrature_distinguishes_normal_pdf_mass_from_valid_reflection_mass() {
    // Finite midpoint grids resolve these broad lobes only. Narrowest admitted
    // roughness is tested analytically and with inverse-CDF samples elsewhere.
    const N: usize = 256;
    for (rx, ry) in [(1.0, 1.0), (0.6, 0.9)] {
        let s = surface(rx, ry);
        let mut normal_mass = 0.0;
        let mut valid_mass = 0.0;
        let mut accepted = 0;
        for i in 0..N {
            for j in 0..N {
                let u = (i as f64 + 0.5) / N as f64;
                let azimuth = (j as f64 + 0.5) / N as f64;
                let l = hemisphere(u, TAU * azimuth);
                normal_mass += s.normal_pdf(frame(), l).unwrap();
                valid_mass += s.reflection_pdf(frame(), Vec3::Z, Vec3::Z, l).unwrap();
                accepted += usize::from(s.sample_reflection(frame(), Vec3::Z, Vec3::Z, [u, azimuth]).unwrap().is_some());
            }
        }
        let domega = TAU / (N * N) as f64;
        close(normal_mass * domega, 1.0, 0.001);
        close(valid_mass * domega, accepted as f64 / (N * N) as f64, 0.003);
        assert!(valid_mass * domega < 1.0);
    }
}

#[test]
fn broad_lobe_importance_estimate_agrees_with_integrated_energy() {
    // White metal, normal view, deterministic256² grids. A scoped consistency
    // check, not universal energy certification or a variance/convergence claim.
    const N: usize = 256;
    let s = AnisotropicSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 0.6, 0.9, 1.5).unwrap();
    let mut integrated = Vec3::ZERO;
    let mut sampled = Vec3::ZERO;
    for i in 0..N {
        for j in 0..N {
            let uv = [(i as f64 + 0.5) / N as f64, (j as f64 + 0.5) / N as f64];
            let light = hemisphere(uv[0], TAU * uv[1]);
            integrated += s.specular_brdf(frame(), Vec3::Z, Vec3::Z, light).unwrap() * light.z;
            if let Some(sample) = s.sample_reflection(frame(), Vec3::Z, Vec3::Z, uv).unwrap() {
                sampled += sample.weight;
            }
        }
    }
    integrated = integrated * (TAU / (N * N) as f64);
    sampled = sampled / (N * N) as f64; // Includes ALL null attempts.
    assert!(integrated.x > 0.0 && integrated.x <= 1.001);
    assert!(sampled.x > 0.0 && sampled.x <= 1.001);
    close_rgb(integrated, sampled, 0.003);
}
