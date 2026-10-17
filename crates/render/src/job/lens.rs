//! Runtime thin-lens camera settings. Focus is an axial distance from the eye,
//! not distance along an off-axis ray. No document or camera-navigation changes.
use super::{ErrorCode, JobError, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThinLens {
    pub aperture_radius_mm: f64,
    pub focus_distance_mm: f64,
}

impl ThinLens {
    pub fn validate(self) -> Result<()> {
        if !self.aperture_radius_mm.is_finite()
            || !(0.0..=1e7).contains(&self.aperture_radius_mm)
            || !self.focus_distance_mm.is_finite()
            || !(1e-6..=1e9).contains(&self.focus_distance_mm)
        {
            return Err(JobError::new(
                ErrorCode::InvalidRequest,
                "lens aperture radius must be 0..10000000 mm and axial focus distance 0.000001..1000000000 mm",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lens_admission_rejects_nonfinite_and_unbounded_values() {
        for aperture in [f64::NAN, f64::INFINITY, -1.0, 1e7 + 1.0] {
            assert!(ThinLens { aperture_radius_mm: aperture, focus_distance_mm: 10.0 }.validate().is_err());
        }
        for focus in [f64::NAN, f64::INFINITY, 0.0, -1.0, 1e9 + 1.0] {
            assert!(ThinLens { aperture_radius_mm: 1.0, focus_distance_mm: focus }.validate().is_err());
        }
        assert!(ThinLens { aperture_radius_mm: 0.0, focus_distance_mm: 1.0 }.validate().is_ok());
        assert!(serde_json::from_str::<ThinLens>(r#"{"aperture_radius_mm":1,"focus_distance_mm":10,"f_stop":2}"#).is_err());
    }
}
