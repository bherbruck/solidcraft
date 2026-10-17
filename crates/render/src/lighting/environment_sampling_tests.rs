use super::*;
use crate::lighting::{HdrDecodeTask, HdrSource, HdrTaskPoll};

fn decoded(pixels: [[u8; 4]; 8], header: &str) -> DecodedHdr {
    let mut bytes = format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n{header}\n-Y 2 +X 4\n").into_bytes();
    for pixel in pixels {
        bytes.extend_from_slice(&pixel);
    }
    let mut task = HdrDecodeTask::new(HdrSource::new(bytes).unwrap()).unwrap();
    for _ in 0..100 {
        if let HdrTaskPoll::Complete(decoded) = task.step(100, || false).unwrap() {
            return decoded;
        }
    }
    panic!("bounded fixture failed to decode");
}
fn prepared(decoded: &DecodedHdr, rotation: f64, intensity: f64) -> Environment<'_> {
    let mut task = EnvironmentPreparation::new(decoded, rotation, intensity).unwrap();
    while !task.step(1, || Ok(())).unwrap() {}
    task.finish().unwrap()
}

#[test]
fn constant_source_has_uniform_solid_angle_pdf_and_exact_radiance_integral() {
    let data = decoded([[128, 64, 32, 129]; 8], "");
    let env = prepared(&data, 0.0, 2.0);
    let expected = Vec3::new(2.0, 1.0, 0.5);
    let mut integral = Vec3::ZERO;
    for i in 0..64 {
        let sample = env.sample([(f64::from(i) + 0.5) / 64.0, 0.37]).unwrap();
        assert!((sample.direction.len() - 1.0).abs() < 1e-14);
        assert_eq!(sample.radiance_source_rgb, expected);
        assert_eq!(env.pdf_solid_angle(sample.direction).unwrap(), sample.pdf_solid_angle);
        integral += sample.radiance_source_rgb / sample.pdf_solid_angle / 64.0;
    }
    assert!((integral - expected * (4.0 * PI)).len() < 1e-12);
    assert!((env.pdf_solid_angle(Vec3::Z).unwrap() * 4.0 * PI - 1.0).abs() < 1e-15);
}

#[test]
fn z_up_uv_centres_seam_poles_and_positive_rotation_are_explicit() {
    let data = decoded([[128, 0, 0, 129]; 8], "");
    let env = prepared(&data, 0.0, 1.0);
    assert_eq!(env.uv(Vec3::X).unwrap(), [0.0, 0.5]);
    assert_eq!(env.uv(Vec3::Y).unwrap(), [0.25, 0.5]);
    assert_eq!(env.uv(Vec3::Z).unwrap(), [0.0, 0.0]);
    assert_eq!(env.uv(-Vec3::Z).unwrap(), [0.0, 1.0]);
    let rotated = prepared(&data, PI / 2.0, 1.0);
    assert_eq!(rotated.uv(Vec3::Y).unwrap(), [0.0, 0.5]);
    assert!(env.uv(Vec3::new(1.0, -1e-8, 0.0)).unwrap()[0] > 0.99);
    let theta = PI / 4.0;
    let phi = PI / 4.0;
    let centre = Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos());
    let uv = env.uv(centre).unwrap();
    assert!((uv[0] - 0.125).abs() < 1e-14 && (uv[1] - 0.25).abs() < 1e-14);
}

#[test]
fn bright_texel_integral_matches_its_independent_spherical_cell_area() {
    let mut pixels = [[0; 4]; 8];
    pixels[0] = [128, 0, 0, 129];
    let data = decoded(pixels, "");
    let env = prepared(&data, 0.0, 1.0);
    let mut integral = 0.0;
    // 4x2: first cell has phi width pi/2 and z width 1, solid angle pi/2.
    for y in 0..8 {
        for x in 0..8 {
            let sample = env.sample([(f64::from(y) + 0.5) / 8.0, (f64::from(x) + 0.5) / 8.0]).unwrap();
            integral += sample.radiance_source_rgb.x / sample.pdf_solid_angle / 64.0;
        }
    }
    assert!((integral - PI / 2.0).abs() < 1e-14);
    let black = decoded([[0; 4]; 8], "");
    let env = prepared(&black, 0.0, 1.0);
    let sample = env.sample([0.3, 0.7]).unwrap();
    assert_eq!(sample.radiance_source_rgb, Vec3::ZERO);
    assert_eq!(sample.pdf_solid_angle, 1.0 / (4.0 * PI));
}

#[test]
fn caller_budget_cancellation_consumes_preparation_and_preserves_owner_capacity() {
    let data = decoded([[128, 0, 0, 129]; 8], "");
    let bytes = data.retained_pixel_bytes().unwrap();
    let mut task = EnvironmentPreparation::new(&data, 0.0, 1.0).unwrap();
    let mut charged = 0;
    assert!(
        !task
            .step(3, || {
                charged += 1;
                Ok(())
            })
            .unwrap()
    );
    assert_eq!(charged, 3);
    assert_eq!(task.step(1, || Err(HdrError::new(HdrErrorCode::Cancelled, "cancel"))).unwrap_err().code, HdrErrorCode::Cancelled);
    assert!(task.finish().is_err());
    assert_eq!(data.retained_pixel_bytes().unwrap(), bytes);
    let mut task = EnvironmentPreparation::new(&data, 0.0, 1.0).unwrap();
    assert_eq!(task.step(1, || Err(HdrError::new(HdrErrorCode::LimitExceeded, "shared work"))).unwrap_err().code, HdrErrorCode::LimitExceeded);
    assert!(task.finish().is_err());
}

#[test]
fn source_colour_is_not_assumed_srgb_and_decoder_corrections_are_not_reapplied() {
    let raw = decoded([[128, 64, 32, 129]; 8], "EXPOSURE=2\nCOLORCORR=2 1 1\n");
    let env = prepared(&raw, 0.0, 1.0);
    assert_eq!(env.radiance_source_rgb(Vec3::X).unwrap(), Vec3::new(0.25, 0.25, 0.125));
    assert!(env.radiance_linear_srgb(Vec3::X).is_err());
    for header in ["PRIMARIES=0.64 0.33 0.29 0.60 0.15 0.06 0.3333 0.3333\n", "PRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n"] {
        let data = decoded([[128, 0, 0, 129]; 8], header);
        let env = prepared(&data, 0.0, 1.0);
        assert_eq!(env.require_linear_srgb().is_ok(), data.info().primaries == Some([0.64, 0.33, 0.30, 0.60, 0.15, 0.06, 0.3127, 0.3290]));
    }
}

#[test]
fn hostile_options_variates_and_directions_refuse_without_panics() {
    let data = decoded([[128, 0, 0, 129]; 8], "");
    for (rotation, intensity) in [(f64::NAN, 1.0), (0.0, f64::INFINITY), (0.0, -1.0), (0.0, 65.0)] {
        assert!(EnvironmentPreparation::new(&data, rotation, intensity).is_err());
    }
    assert!(EnvironmentPreparation::new(&data, 0.0, 1.0).unwrap().finish().is_err());
    let env = prepared(&data, 0.0, 0.0);
    for u in [[0.0, 0.5], [1.0, 0.5], [f64::NAN, 0.5]] {
        assert!(env.sample(u).is_err());
    }
    for d in [Vec3::ZERO, Vec3::new(f64::INFINITY, 0.0, 0.0)] {
        assert!(env.uv(d).is_err());
        assert!(env.pdf_solid_angle(d).is_err());
    }
    assert_eq!(env.radiance_source_rgb(Vec3::new(f64::MAX, f64::MAX, 0.0)).unwrap(), Vec3::ZERO);
}

#[test]
fn direct_conductor_estimates_average_once_and_do_not_cross_geometry() {
    let data = decoded([[128, 64, 32, 129]; 8], "PRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n");
    let env = prepared(&data, 0.0, 1.0);
    let surface = crate::surface::OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 1.0, 1.5).unwrap();
    let mut mean = Vec3::ZERO;
    for i in 0..4096 {
        let u = [(f64::from(i) + 0.5) / 4096.0, 0.375];
        let sample = env.direct_sample_linear_srgb(u, surface, Vec3::Z, Vec3::Z, Vec3::Z).unwrap();
        mean += sample.contribution_linear_srgb / 4096.0;
    }
    // Independent white-conductor roughness=1 normal-view integral:
    // 2*pi*integral_0^1 z/(2*pi*(1+z)) dz = 1-ln(2).
    assert!((mean - Vec3::new(1.0, 0.5, 0.25) * (1.0 - 2.0_f64.ln())).len() < 3e-8);
    let blocked = env.direct_sample_linear_srgb([0.25, 0.375], surface, Vec3::Z, -Vec3::Z, Vec3::Z).unwrap();
    assert_eq!(blocked.contribution_linear_srgb, Vec3::ZERO);
    for view in [Vec3::new(0.99, 0.0, -0.1), Vec3::X] {
        let crossed = env.direct_sample_linear_srgb([0.25, 0.01], surface, Vec3::new(0.8, 0.0, 0.6), Vec3::Z, view).unwrap();
        assert_eq!(crossed.contribution_linear_srgb, Vec3::ZERO);
    }
    assert!(env.direct_sample_linear_srgb([0.25, 0.01], surface, Vec3::Z, Vec3::Z, Vec3::ZERO).is_err());
    let unknown = decoded([[128, 0, 0, 129]; 8], "");
    assert!(prepared(&unknown, 0.0, 1.0).direct_sample_linear_srgb([0.25, 0.375], surface, Vec3::Z, Vec3::Z, Vec3::Z).is_err());
}
