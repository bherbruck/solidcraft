//! White-box hostile decoded input tests; production has no unchecked constructor.
use super::*;
use crate::lighting::environment_sampling::EnvironmentPreparation;

fn info() -> HdrInfo {
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
    bytes.extend_from_slice(&[128, 64, 32, 129]);
    HdrSource::new(bytes).unwrap().info().clone()
}

#[test]
fn malformed_decoded_dimensions_lengths_and_original_capacity_refuse() {
    for dimensions in [(0, 2), (4, 0), (3, 2), (4096, 2048)] {
        let mut metadata = info();
        metadata.width = dimensions.0;
        metadata.height = dimensions.1;
        let decoded = DecodedHdr { info: metadata, pixels: vec![[1.0; 3]; 8] };
        assert!(EnvironmentPreparation::new(&decoded, 0.0, 1.0).is_err());
    }
    let decoded = DecodedHdr { info: info(), pixels: vec![[1.0; 3]; 7] };
    assert!(EnvironmentPreparation::new(&decoded, 0.0, 1.0).is_err());
    let mut pixels = Vec::with_capacity(MAX_HDR_PIXELS as usize + 1);
    pixels.resize(8, [1.0; 3]);
    let decoded = DecodedHdr { info: info(), pixels };
    assert!(decoded.retained_pixel_bytes().unwrap() > MAX_HDR_PIXELS * 12);
    assert!(EnvironmentPreparation::new(&decoded, 0.0, 1.0).is_err());
}

#[test]
fn invalid_source_radiance_consumes_preparation_before_exposing_lookup() {
    for channel in [f32::NAN, f32::INFINITY, -1.0, 1e9] {
        let mut pixels = vec![[1.0; 3]; 8];
        pixels[3][1] = channel;
        let decoded = DecodedHdr { info: info(), pixels };
        let mut preparation = EnvironmentPreparation::new(&decoded, 0.0, 1.0).unwrap();
        assert_eq!(preparation.step(8, || Ok(())).unwrap_err().code, HdrErrorCode::InvalidInput);
        assert!(preparation.finish().is_err());
    }
}
