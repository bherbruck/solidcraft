use super::*;

fn hdr(extra: &str, pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut bytes = format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n{extra}\n-Y 2 +X 4\n").into_bytes();
    for pixel in pixels {
        bytes.extend_from_slice(pixel);
    }
    bytes
}
#[test]
fn captured_hdr_requires_explicit_color_policy_and_prepares_without_clipping() {
    let bytes = hdr("", &[[128, 64, 32, 130]; 8]);
    assert!(EnvironmentSource::hdr(bytes.clone(), 0.0, 1.0, false).is_err());
    let source = EnvironmentSource::hdr(bytes, 0.3, 2.0, true).unwrap();
    assert_eq!(source.info().source_sha256.len(), 64);
    assert_eq!(source.info().color_policy, "explicit_assumed_linear_srgb_d65");
    let prepared = source.prepare(&CancelToken::default()).unwrap();
    assert_eq!(prepared.dimensions(), [4, 2]);
    assert!(prepared.texels().iter().all(|t| t.rgb == [2.0, 1.0, 0.5]));
    let wrong = hdr("PRIMARIES=0.67 0.33 0.21 0.71 0.14 0.08 0.31 0.33\n", &[[128, 128, 128, 129]; 8]);
    assert!(EnvironmentSource::hdr(wrong, 0.0, 1.0, true).is_err());
    let declared = hdr("PRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n", &[[128, 128, 128, 129]; 8]);
    assert!(EnvironmentSource::hdr(declared, 0.0, 1.0, false).is_ok());
    assert!(EnvironmentSource::sky(SkyGradient::default(), f64::NAN, 1.0).is_err());
    assert!(EnvironmentSource::sky(SkyGradient::default(), 0.0, 101.0).is_err());
    let token = CancelToken::default();
    token.cancel();
    assert_eq!(source.prepare(&token).err().unwrap().code, ErrorCode::Cancelled);
}
#[test]
fn exact_solid_angle_distribution_handles_poles_black_and_white_maps() {
    for color in [[0.0; 3], [1.0; 3]] {
        let map = PreparedEnvironment::new(64, 32, vec![color; 2048], &CancelToken::default()).unwrap();
        let total: f64 = map.texels().iter().enumerate().map(|(i, t)| f64::from(t.pdf) * row_solid_angle(64, 32, i / 64)).sum();
        assert!((total - 1.0).abs() < 1e-7);
        for t in map.texels() {
            assert!((f64::from(t.pdf) - 1.0 / (4.0 * PI)).abs() < 1e-8);
        }
        for d in [Vec3::Z, -Vec3::Z, Vec3::X, Vec3::new(1.0, -1e-12, 0.0)] {
            let (value, pdf) = map.evaluate(d).unwrap();
            assert_eq!(value, color);
            assert!((f64::from(pdf) - 1.0 / (4.0 * PI)).abs() < 1e-8);
        }
    }
}
#[test]
fn alias_samples_match_unequal_texel_probabilities_and_lookup_pdf() {
    let mut pixels = vec![[0.0; 3]; 32];
    pixels[2] = [1.0; 3];
    pixels[11] = [4.0; 3];
    pixels[29] = [2.0; 3];
    let map = PreparedEnvironment::new(8, 4, pixels, &CancelToken::default()).unwrap();
    let mut counts = [0usize; 32];
    let mut rng = 17u64;
    let mut random = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let count = 200_000;
    for _ in 0..count {
        let (direction, rgb, pdf) = map.sample(random(), random(), random(), random()).unwrap();
        let (actual, actual_pdf) = map.evaluate(direction).unwrap();
        assert_eq!(rgb, actual);
        assert_eq!(pdf, actual_pdf);
        let index = if rgb == [1.0; 3] {
            2
        } else if rgb == [4.0; 3] {
            11
        } else {
            assert_eq!(rgb, [2.0; 3]);
            29
        };
        counts[index] += 1;
    }
    for i in [2, 11, 29] {
        let expected = f64::from(map.texels()[i].pdf) * row_solid_angle(8, 4, i / 8);
        assert!((counts[i] as f64 / f64::from(count) - expected).abs() < 0.004);
    }
}
#[test]
fn original_sky_is_bounded_cancellable_and_reproducible() {
    let source = EnvironmentSource::sky(SkyGradient::default(), 0.0, 1.0).unwrap();
    let a = source.prepare(&CancelToken::default()).unwrap();
    assert_eq!(a.dimensions(), [512, 256]);
    let (zenith, _) = a.evaluate(Vec3::Z).unwrap();
    let (horizon, _) = a.evaluate(Vec3::X).unwrap();
    let (ground, _) = a.evaluate(-Vec3::Z).unwrap();
    assert!(zenith[2] > zenith[0]);
    assert!(horizon[0] > zenith[0]);
    assert!(ground[0] > ground[2]);
    assert!(source.admitted_bytes() > a.texels().len() as u64 * 64);
    assert!(EnvironmentSource::sky(SkyGradient { zenith: [f32::INFINITY; 3], ..Default::default() }, 0.0, 1.0).is_err());
}

#[test]
fn hdr_admission_counts_retained_capacity_not_just_file_length() {
    let bytes = hdr("", &[[128, 128, 128, 129]; 8]);
    let mut overallocated = Vec::with_capacity(super::super::MAX_HDR_BYTES);
    overallocated.extend_from_slice(&bytes);
    let retained = overallocated.capacity() as u64;
    let source = EnvironmentSource::hdr(overallocated, 0.0, 1.0, true).unwrap();
    assert_eq!(source.info().encoded_bytes, bytes.len() as u64);
    assert_eq!(source.admitted_bytes(), retained + 8 * 160 + 65536);
}
