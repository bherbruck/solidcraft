//! Transient, bounded point/spot options; never design persistence.
use serde::{Deserialize, Serialize};

use super::{ErrorCode, JobError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotConeOptions {
    /// World direction FROM the source, never camera-local.
    pub emitted_axis: [f64; 3],
    pub inner_half_angle_radians: f64,
    pub outer_half_angle_radians: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PunctualLightOptions {
    pub position_mm: [f64; 3],
    pub color: [f64; 3],
    pub intensity: f64,
    pub reference_distance_mm: f64,
    pub minimum_distance_mm: f64,
    /// Omitted means a point source with no directional cone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cone: Option<SpotConeOptions>,
}

impl PunctualLightOptions {
    pub fn validate(&self) -> Result<()> {
        let invalid = || JobError::new(ErrorCode::InvalidRequest, "invalid bounded point/spot light");
        if !self.position_mm.iter().all(|v| v.is_finite() && v.abs() <= 1e12)
            || !self.color.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            || !self.intensity.is_finite()
            || !(0.0..=64.0).contains(&self.intensity)
            || !self.reference_distance_mm.is_finite()
            || !(0.001..=1e6).contains(&self.reference_distance_mm)
            || !self.minimum_distance_mm.is_finite()
            || !(0.001..=self.reference_distance_mm).contains(&self.minimum_distance_mm)
        {
            return Err(invalid());
        }
        if let Some(cone) = self.cone {
            let inner = cone.inner_half_angle_radians;
            let outer = cone.outer_half_angle_radians;
            if !cone.emitted_axis.iter().all(|v| v.is_finite())
                || !cone.emitted_axis.iter().any(|v| *v != 0.0)
                || !inner.is_finite()
                || !outer.is_finite()
                || inner < 0.0
                || inner >= outer
                || outer > std::f64::consts::FRAC_PI_2
                || inner.cos() <= outer.cos()
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    #[cfg(feature = "raytrace-cpu")]
    pub(crate) fn light(self) -> crate::raytrace::Result<crate::raytrace::punctual::PunctualLight> {
        use crate::raytrace::punctual::{PunctualLight, SpotCone};
        use solidcraft_geom::Vec3;
        let [x, y, z] = self.position_mm;
        let [r, g, b] = self.color;
        let cone = self
            .cone
            .map(|c| {
                let [x, y, z] = c.emitted_axis;
                SpotCone::new(Vec3::new(x, y, z), c.inner_half_angle_radians, c.outer_half_angle_radians)
            })
            .transpose()?;
        PunctualLight::new(Vec3::new(x, y, z), Vec3::new(r, g, b), self.intensity, self.reference_distance_mm, self.minimum_distance_mm, cone)
    }
}

pub(super) fn bounded_punctual<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<Vec<PunctualLightOptions>>, D::Error> {
    struct Lights;
    impl<'de> serde::de::Visitor<'de> for Lights {
        type Value = Vec<PunctualLightOptions>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most eight point/spot sources")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
            let mut lights = Vec::new();
            while let Some(light) = seq.next_element()? {
                if lights.len() == 8 {
                    return Err(serde::de::Error::custom("at most eight point/spot sources"));
                }
                lights.try_reserve_exact(1).map_err(serde::de::Error::custom)?;
                lights.push(light);
            }
            if lights.capacity() > 8 {
                return Err(serde::de::Error::custom("point/spot capacity exceeds eight"));
            }
            Ok(lights)
        }
    }
    struct Optional;
    impl<'de> serde::de::Visitor<'de> for Optional {
        type Value = Option<Vec<PunctualLightOptions>>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("optional bounded point/spot sources")
        }
        fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<Self::Value, D::Error> {
            d.deserialize_seq(Lights).map(Some)
        }
    }
    d.deserialize_option(Optional)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::RaytraceOptions;

    fn light() -> PunctualLightOptions {
        PunctualLightOptions {
            position_mm: [0.0, 0.0, 100.0],
            color: [1.0; 3],
            intensity: 1.0,
            reference_distance_mm: 100.0,
            minimum_distance_mm: 0.001,
            cone: None,
        }
    }

    #[test]
    fn omitted_empty_point_and_spot_roundtrip_without_null_sequence() {
        let mut spot = light();
        spot.cone = Some(SpotConeOptions { emitted_axis: [0.0, 0.0, -1.0], inner_half_angle_radians: 0.2, outer_half_angle_radians: 0.5 });
        for lights in [None, Some(Vec::new()), Some(vec![light(), spot])] {
            let options = RaytraceOptions { punctual_lights: lights, ..Default::default() };
            let json = serde_json::to_value(&options).unwrap();
            assert_eq!(serde_json::from_value::<RaytraceOptions>(json).unwrap(), options);
        }
    }

    #[test]
    fn counts_original_capacity_and_rejects_ninth_source_before_runtime() {
        let options = RaytraceOptions { punctual_lights: Some(vec![light(); 8]), ..Default::default() };
        assert_eq!(options.retained_bytes().unwrap(), (8 * std::mem::size_of::<PunctualLightOptions>()) as u64);
        assert!(options.validate().is_ok());
        let nine = RaytraceOptions { punctual_lights: Some(vec![light(); 9]), ..Default::default() };
        assert!(nine.validate().is_err());
        assert!(serde_json::from_value::<RaytraceOptions>(serde_json::to_value(nine).unwrap()).is_err());
        let spare = RaytraceOptions { punctual_lights: Some(Vec::with_capacity(9)), ..Default::default() };
        assert!(spare.retained_bytes().is_err());
    }

    #[test]
    fn directionals_keep_their_own_eight_slots_when_punctual_slots_are_full() {
        let options = RaytraceOptions {
            lights: Some(vec![crate::job::DirectionalLightOptions { direction: [0.0, 0.0, 1.0], color: [1.0; 3], intensity: 1.0 }; 8]),
            punctual_lights: Some(vec![light(); 8]),
            ..Default::default()
        };
        assert!(options.validate().is_ok());
    }

    #[test]
    fn invalid_cone_and_nonfinite_inputs_refuse() {
        let mut input = light();
        input.cone = Some(SpotConeOptions { emitted_axis: [0.0; 3], inner_half_angle_radians: 0.0, outer_half_angle_radians: 0.5 });
        assert!(input.validate().is_err());
        input = light();
        input.position_mm[0] = f64::NAN;
        assert!(input.validate().is_err());
        input = light();
        input.minimum_distance_mm = 0.0;
        assert!(input.validate().is_err());
        input = light();
        input.reference_distance_mm = f64::INFINITY;
        assert!(input.validate().is_err());
    }
}
