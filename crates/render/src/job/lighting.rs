//! Runtime rectangular emitters and constant environment radiance. Coordinates are
//! world-space millimetres; radiance is linear RGB. No saved-document contract.
use super::{ErrorCode, JobError, Result};
use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

pub const MAX_AREA_LIGHTS: usize = 4;

/// A one-sided rectangle: `center + u*s + v*t`, with s,t in [-1,1].
/// Its emitting normal is `u cross v`; u and v are orthogonal half-edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AreaLight {
    pub center: [f64; 3],
    pub u: [f64; 3],
    pub v: [f64; 3],
    pub radiance: [f64; 3],
}

fn vector(a: [f64; 3]) -> Vec3 {
    let [x, y, z] = a;
    Vec3::new(x, y, z)
}

impl AreaLight {
    pub fn validate(self) -> Result<()> {
        let u = vector(self.u);
        let v = vector(self.v);
        let ul = u.len();
        let vl = v.len();
        if self.center.into_iter().any(|x| !x.is_finite() || x.abs() > 1e12)
            || self.u.into_iter().chain(self.v).any(|x| !x.is_finite())
            || !(1e-4..=1e6).contains(&ul)
            || !(1e-4..=1e6).contains(&vl)
            || u.dot(v).abs() > 1e-6 * ul * vl
            || self.radiance.into_iter().any(|x| !x.is_finite() || !(0.0..=1e6).contains(&x))
        {
            return Err(JobError::new(
                ErrorCode::InvalidRequest,
                "area light requires finite center, orthogonal half-edges of 0.0001..1000000 mm and radiance 0..1000000",
            ));
        }
        Ok(())
    }

    pub fn area(self) -> f64 {
        4.0 * vector(self.u).cross(vector(self.v)).len()
    }
}

/// Uniform-radiance finite angular disk at infinity. Direction points toward the sun.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sun {
    pub direction: [f64; 3],
    pub angular_radius_rad: f64,
    pub radiance: [f64; 3],
}
impl Sun {
    pub fn validate(self) -> Result<()> {
        if self.direction.iter().any(|x| !x.is_finite())
            || !(1e-8..=1e8).contains(&vector(self.direction).len())
            || !self.angular_radius_rad.is_finite()
            || !(0.001..=std::f64::consts::FRAC_PI_2).contains(&self.angular_radius_rad)
            || self.radiance.iter().any(|x| !x.is_finite() || !(0.0..=1e8).contains(x))
        {
            return Err(JobError::new(
                ErrorCode::InvalidRequest,
                "sun requires finite nonzero direction, radius 0.001..pi/2 radians and radiance 0..100000000",
            ));
        }
        Ok(())
    }
    pub fn solid_angle(self) -> f64 {
        4.0 * std::f64::consts::PI * (0.5 * self.angular_radius_rad).sin().powi(2)
    }
    pub fn direction(self) -> Vec3 {
        let d = vector(self.direction);
        d / d.len()
    }
}

/// A small copyable light list, serialized as an ordinary bounded JSON array.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct AreaLights {
    entries: [AreaLight; MAX_AREA_LIGHTS],
    len: usize,
}

impl AreaLights {
    pub fn new(lights: &[AreaLight]) -> Result<Self> {
        if lights.len() > MAX_AREA_LIGHTS {
            return Err(JobError::new(ErrorCode::InvalidRequest, "at most four area lights are supported"));
        }
        let mut result = Self::default();
        for (slot, light) in result.entries.iter_mut().zip(lights) {
            light.validate()?;
            *slot = *light;
        }
        result.len = lights.len();
        Ok(result)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AreaLight> {
        self.entries.iter().take(self.len)
    }
}

impl Serialize for AreaLights {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for AreaLights {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = AreaLights;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an array of at most four rectangular area lights")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
                let mut result = AreaLights::default();
                while let Some(light) = seq.next_element::<AreaLight>()? {
                    let slot =
                        result.entries.get_mut(result.len).ok_or_else(|| serde::de::Error::custom("at most four area lights are supported"))?;
                    light.validate().map_err(serde::de::Error::custom)?;
                    *slot = light;
                    result.len += 1;
                }
                Ok(result)
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StudioLighting {
    pub sun: Option<Sun>,
    pub environment: [f64; 3],
    pub area_lights: AreaLights,
}

impl StudioLighting {
    pub fn validate(self) -> Result<()> {
        if let Some(sun) = self.sun {
            sun.validate()?;
        }
        if self.environment.into_iter().any(|x| !x.is_finite() || !(0.0..=1e6).contains(&x)) {
            return Err(JobError::new(ErrorCode::InvalidRequest, "environment radiance must be finite and 0..1000000"));
        }
        for light in self.area_lights.iter() {
            light.validate()?;
        }
        Ok(())
    }
}

/// Output-only transform, after linear accumulation and exposure, before sRGB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneMap {
    #[default]
    Reinhard,
    AcesFitted,
}

impl ToneMap {
    pub fn map(self, linear: f64) -> f64 {
        let x = linear.max(0.0);
        match self {
            Self::Reinhard => x / (1.0 + x),
            // Narkowicz (2016), published rational approximation to the ACES RRT/ODT.
            // This is a per-channel fit, not the full ACES colour-management pipeline.
            // https://knarkowicz.wordpress.com/2016/01/06/aces-filmic-tone-mapping-curve/
            Self::AcesFitted => ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light() -> AreaLight {
        AreaLight { center: [0.0, 0.0, 4.0], u: [2.0, 0.0, 0.0], v: [0.0, -3.0, 0.0], radiance: [5.0; 3] }
    }

    #[test]
    fn bounded_lights_validate_geometry_and_round_trip() {
        let lights = AreaLights::new(&[light(); 4]).unwrap();
        assert_eq!(light().area(), 24.0);
        assert_eq!(serde_json::from_str::<AreaLights>(&serde_json::to_string(&lights).unwrap()).unwrap(), lights);
        assert!(AreaLights::new(&[light(); 5]).is_err());
        assert!(serde_json::from_str::<AreaLights>(&serde_json::to_string(&vec![light(); 5]).unwrap()).is_err());
        for bad in [
            AreaLight { u: [0.0; 3], ..light() },
            AreaLight { v: [1.0, 1.0, 0.0], ..light() },
            AreaLight { radiance: [-1.0; 3], ..light() },
            AreaLight { center: [f64::NAN; 3], ..light() },
        ] {
            assert!(bad.validate().is_err());
        }
        assert!(StudioLighting { environment: [f64::INFINITY; 3], ..Default::default() }.validate().is_err());
    }

    #[test]
    fn finite_sun_validates_and_integrates_its_solid_angle() {
        let sun = Sun { direction: [0.0, 0.0, 2.0], angular_radius_rad: 0.2, radiance: [3.0; 3] };
        sun.validate().unwrap();
        assert_eq!(sun.direction(), Vec3::Z);
        assert!((sun.solid_angle() - 2.0 * std::f64::consts::PI * (1.0 - 0.2_f64.cos())).abs() < 1e-14);
        for radius in [0.0, 0.00001, f64::NAN, 2.0] {
            assert!(Sun { angular_radius_rad: radius, ..sun }.validate().is_err());
        }
        assert!(Sun { direction: [0.0; 3], ..sun }.validate().is_err());
    }

    #[test]
    fn filmic_is_bounded_monotone_and_preserves_black() {
        assert_eq!(ToneMap::default(), ToneMap::Reinhard);
        assert_eq!(ToneMap::AcesFitted.map(0.0), 0.0);
        assert!((ToneMap::AcesFitted.map(1.0) - 0.8037974683544302).abs() < 1e-14);
        let mut previous = 0.0;
        for i in 0..1000 {
            let value = ToneMap::AcesFitted.map(f64::from(i) / 100.0);
            assert!((previous..=1.0).contains(&value));
            previous = value;
        }
    }
}
