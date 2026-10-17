//! CPU product-image work and its device-independent, session-scoped protocol.
//! Hosts own execution; documents and history never contain these values.

use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{BackendId, Camera, ExecutorId};

pub(crate) mod encoding;
mod host;
pub(crate) mod image;
pub mod led_strip;
#[cfg(test)]
mod light_union_tests;
mod punctual;
#[cfg(test)]
mod rt2_tests;
pub use host::{CpuHost, HostKind, JobHandle, JobStatus, RenderFrame};
pub use image::{Admission, ImageTask, TaskPoll, admit};
pub use punctual::{PunctualLightOptions, SpotConeOptions};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobId {
    pub session: u64,
    pub serial: u64,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Queued,
    Preparing,
    Running,
    Encoding,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
}

impl Phase {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Completed | Self::Failed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    LimitExceeded,
    AllocationFailed,
    HostUnavailable,
    QueueFull,
    TimedOut,
    Cancelled,
    InvalidScene,
    EncodingFailed,
    WrongOwner,
    UnknownJob,
    NotCompleted,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JobError {
    pub code: ErrorCode,
    pub message: String,
}

impl JobError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for JobError {}

pub type Result<T> = std::result::Result<T, JobError>;

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() { Err(JobError::new(ErrorCode::Cancelled, "render cancelled")) } else { Ok(()) }
    }
}

/// Per-request execution choice, independent of viewport graphics presets.
/// Keep both variants parseable in builds without optional raytracing, so those
/// builds return an explicit capability error before capturing model geometry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrator {
    #[default]
    Raster,
    Raytrace,
}

/// Explicit scene-reflection transport, separate from opaque roughness bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionMode {
    Mirror,
    RoughSpecular,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionBody {
    pub body: String,
    pub mode: ReflectionMode,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReflectionOptions {
    #[serde(deserialize_with = "bounded_reflection_bodies")]
    pub bodies: Vec<ReflectionBody>,
    pub max_bounces: u8,
    pub samples: u8,
}
impl Default for ReflectionOptions {
    fn default() -> Self {
        Self { bodies: Vec::new(), max_bounces: 2, samples: 4 }
    }
}
fn bounded_reflection_bodies<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<ReflectionBody>, D::Error> {
    struct Bodies;
    impl<'de> serde::de::Visitor<'de> for Bodies {
        type Value = Vec<ReflectionBody>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("at most64 reflection body overrides")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == 64 {
                    return Err(serde::de::Error::custom("at most64 reflection bodies"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Bodies)
}
impl ReflectionOptions {
    pub fn validate(&self) -> Result<()> {
        if !(1..=8).contains(&self.max_bounces) || !(1..=64).contains(&self.samples) || self.bodies.len() > 64 || self.bodies.capacity() > 64 {
            return Err(JobError::new(ErrorCode::InvalidRequest, "reflection bounds: 1..8 bounces, 1..64 samples, at most64 body slots"));
        }
        for (index, body) in self.bodies.iter().enumerate() {
            if body.body.is_empty()
                || body.body.len() > 128
                || body.body.capacity() > 128
                || self.bodies.iter().take(index).any(|other| other.body == body.body)
            {
                return Err(JobError::new(ErrorCode::InvalidRequest, "reflection body names must be unique, nonempty and bounded to128 bytes"));
            }
        }
        Ok(())
    }
}

/// Linear-light runtime inputs. These are never design settings or GPU presets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionalLightOptions {
    pub direction: [f64; 3],
    pub color: [f64; 3],
    pub intensity: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingPreset {
    Neutral,
    WarmCool,
    KeyRim,
}

/// Full world-space side vectors; U cross V is the emitting side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RectangleLightOptions {
    pub center_mm: [f64; 3],
    pub edge_u_mm: [f64; 3],
    pub edge_v_mm: [f64; 3],
    pub color: [f64; 3],
    pub intensity: f64,
}

/// Explicit visual rotation about a certified planar face's outward normal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnisotropicOrientation {
    #[default]
    Zero,
    QuarterTurn,
}

/// Runtime-only captured planar face selector; no persistent naming is invented.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnisotropicFaceOptions {
    pub body_index: usize,
    #[serde(deserialize_with = "bounded_anisotropic_body_name")]
    pub body_name: String,
    pub face: u32,
    pub tangent: [f64; 3],
    pub orientation: AnisotropicOrientation,
    pub roughness_x: f64,
    pub roughness_y: f64,
}

fn bounded_anisotropic_body_name<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    struct Name;
    impl serde::de::Visitor<'_> for Name {
        type Value = String;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("captured body name of 1..1024 bytes")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<String, E> {
            if value.is_empty() || value.len() > 1024 {
                return Err(E::custom("captured body name of 1..1024 bytes required"));
            }
            let mut name = String::new();
            name.try_reserve_exact(value.len()).map_err(E::custom)?;
            name.push_str(value);
            Ok(name)
        }
    }
    d.deserialize_str(Name)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RaytraceOptions {
    /// Transient spatial pixel-box samples; independent of reflection attempts.
    #[serde(skip_serializing_if = "single_primary_sample")]
    pub primary_samples: u8,
    pub reflections: Option<ReflectionOptions>,
    pub anisotropic_faces: [Option<AnisotropicFaceOptions>; 2],
    #[serde(deserialize_with = "punctual::bounded_punctual", skip_serializing_if = "Option::is_none")]
    pub punctual_lights: Option<Vec<PunctualLightOptions>>,
    #[serde(deserialize_with = "bounded_area_lights", skip_serializing_if = "Option::is_none")]
    pub area_lights: Option<Vec<RectangleLightOptions>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_samples: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    pub preset: Option<LightingPreset>,
    #[serde(deserialize_with = "bounded_lights", skip_serializing_if = "Option::is_none")]
    pub lights: Option<Vec<DirectionalLightOptions>>,
    pub exposure_ev: f64,
    pub shadows: bool,
}

impl Default for RaytraceOptions {
    fn default() -> Self {
        Self {
            punctual_lights: None,
            area_lights: None,
            area_samples: None,
            seed: None,
            primary_samples: 1,
            anisotropic_faces: [None, None],
            reflections: None,
            preset: None,
            lights: None,
            exposure_ev: 0.0,
            shadows: true,
        }
    }
}

fn single_primary_sample(samples: &u8) -> bool {
    *samples == 1
}

fn bounded_lights<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<Vec<DirectionalLightOptions>>, D::Error> {
    struct Lights;
    impl<'de> serde::de::Visitor<'de> for Lights {
        type Value = Vec<DirectionalLightOptions>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("at most eight directional lights")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error> {
            let mut lights = Vec::new();
            while let Some(light) = sequence.next_element()? {
                if lights.len() == 8 {
                    return Err(serde::de::Error::custom("at most eight directional lights"));
                }
                lights.push(light);
            }
            Ok(lights)
        }
    }
    deserializer.deserialize_seq(Lights).map(Some)
}

fn bounded_area_lights<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<Vec<RectangleLightOptions>>, D::Error> {
    struct AreaLights;
    impl<'de> serde::de::Visitor<'de> for AreaLights {
        type Value = Vec<RectangleLightOptions>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("at most four rectangular lights")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
            let mut lights = Vec::new();
            while let Some(light) = seq.next_element()? {
                if lights.len() == 4 {
                    return Err(serde::de::Error::custom("at most four rectangular lights"));
                }
                lights.push(light);
            }
            Ok(lights)
        }
    }
    struct Optional;
    impl<'de> serde::de::Visitor<'de> for Optional {
        type Value = Option<Vec<RectangleLightOptions>>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an optional bounded rectangular-light list")
        }
        fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<Self::Value, D::Error> {
            d.deserialize_seq(AreaLights).map(Some)
        }
    }
    deserializer.deserialize_option(Optional)
}

impl RaytraceOptions {
    /// One capacity calculation shared by host and engine preflight.
    pub fn retained_bytes(&self) -> Result<u64> {
        self.validate()?;
        let limit = || JobError::new(ErrorCode::LimitExceeded, "raytrace request capacity overflow");
        let mut bytes = self
            .lights
            .as_ref()
            .map_or(Some(0), |l| (l.capacity() as u64).checked_mul(std::mem::size_of::<DirectionalLightOptions>() as u64))
            .ok_or_else(limit)?;
        for (capacity, size) in [
            (self.area_lights.as_ref().map_or(0, Vec::capacity), std::mem::size_of::<RectangleLightOptions>()),
            (self.punctual_lights.as_ref().map_or(0, Vec::capacity), std::mem::size_of::<PunctualLightOptions>()),
        ] {
            bytes = bytes.checked_add((capacity as u64).checked_mul(size as u64).ok_or_else(limit)?).ok_or_else(limit)?;
        }
        if let Some(reflections) = &self.reflections {
            let slots = (reflections.bodies.capacity() as u64).checked_mul(std::mem::size_of::<ReflectionBody>() as u64).ok_or_else(limit)?;
            bytes = bytes.checked_add(slots).ok_or_else(limit)?;
            for body in &reflections.bodies {
                bytes = bytes.checked_add(body.body.capacity() as u64).ok_or_else(limit)?;
            }
        }
        for target in self.anisotropic_faces.iter().flatten() {
            bytes = bytes.checked_add(target.body_name.capacity() as u64).ok_or_else(limit)?;
        }
        Ok(bytes)
    }
    /// No environment NEE on selected reflectors; rough transport must be purely metallic.
    /// Rejecting every nonmetallic effective face prevents silently dropping its diffuse HDR.
    pub fn validate_environment_reflections(&self, scene: &crate::ProductScene) -> Result<()> {
        self.validate()?;
        if let Some(reflections) = &self.reflections {
            for selector in &reflections.bodies {
                let mut bodies = scene.meshes.iter().filter(|b| b.name == selector.body);
                let body = bodies.next().ok_or_else(|| JobError::new(ErrorCode::InvalidRequest, "environment reflection body missing"))?;
                if bodies.next().is_some() {
                    return Err(JobError::new(ErrorCode::InvalidRequest, "ambiguous environment reflection body"));
                }
                if selector.mode == ReflectionMode::RoughSpecular
                    && (body.surface.is_none_or(|s| s.metallic() != 1.0) || body.faces.iter().any(|f| f.surface.is_none_or(|s| s.metallic() != 1.0)))
                {
                    return Err(JobError::new(
                        ErrorCode::InvalidRequest,
                        "HDR rough reflection requires fully metallic body and face surfaces; diffuse/specular partition is not supported",
                    ));
                }
            }
        }
        for target in self.anisotropic_faces.iter().flatten() {
            let mode = self.reflections.as_ref().and_then(|r| r.bodies.iter().find(|b| b.body == target.body_name).map(|b| b.mode));
            if mode != Some(ReflectionMode::RoughSpecular) {
                return Err(JobError::new(
                    ErrorCode::InvalidRequest,
                    "HDR anisotropy requires a selected fully metallic rough reflector; anisotropic HDR direct lighting is not supported",
                ));
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        let area_count = self.area_lights.as_ref().map_or(0, Vec::len);
        if self.area_lights.as_ref().is_some_and(|v| v.len() > 4 || v.capacity() > 4) {
            return Err(JobError::new(ErrorCode::InvalidRequest, "at most four rectangular lights and reserved slots"));
        }
        if area_count == 0 && (self.area_samples.is_some() || self.seed.is_some()) {
            return Err(JobError::new(ErrorCode::InvalidRequest, "area_samples and seed require nonempty area_lights"));
        }
        if self.area_samples.is_some_and(|n| !matches!(n, 1 | 4 | 16 | 64)) {
            return Err(JobError::new(ErrorCode::InvalidRequest, "area_samples must be 1, 4, 16 or 64"));
        }
        let directional_count = self.lights.as_ref().map_or_else(|| if self.preset == Some(LightingPreset::KeyRim) { 2 } else { 3 }, Vec::len);
        if directional_count
            .checked_add(area_count)
            .and_then(|n| n.checked_add(self.punctual_lights.as_ref().map_or(0, Vec::len)))
            .is_none_or(|n| n > 20)
        {
            return Err(JobError::new(ErrorCode::InvalidRequest, "at most twenty combined directional, punctual and rectangular lights"));
        }
        if let Some(lights) = &self.area_lights {
            for light in lights {
                use solidcraft_geom::Vec3;
                let vec = |v: [f64; 3]| Vec3::new(v[0], v[1], v[2]);
                let c = vec(light.center_mm);
                let u = vec(light.edge_u_mm);
                let v = vec(light.edge_v_mm);
                if !c.is_finite()
                    || !u.is_finite()
                    || !v.is_finite()
                    || !light.color.iter().all(|x| x.is_finite() && (0.0..=1.0).contains(x))
                    || !light.intensity.is_finite()
                    || !(0.0..=64.0).contains(&light.intensity)
                    || !(1e-6..=1e6).contains(&u.len())
                    || !(1e-6..=1e6).contains(&v.len())
                    || (u.dot(v) / (u.len() * v.len())).abs() > 1e-10
                {
                    return Err(JobError::new(ErrorCode::InvalidRequest, "invalid finite rectangular light"));
                }
                for a in [-0.5, 0.5] {
                    for b in [-0.5, 0.5] {
                        let p = c + u * a + v * b;
                        if !p.is_finite() || [p.x, p.y, p.z].into_iter().any(|x| x.abs() > 1e12) {
                            return Err(JobError::new(ErrorCode::InvalidRequest, "rectangular light corner exceeds world bounds"));
                        }
                    }
                }
            }
        }
        if let Some(lights) = &self.punctual_lights {
            if lights.len() > 8 || lights.capacity() > 8 {
                return Err(JobError::new(ErrorCode::InvalidRequest, "at most eight point/spot sources and reserved slots"));
            }
            for light in lights {
                light.validate()?;
            }
        }
        if !matches!(self.primary_samples, 1 | 4 | 16) {
            return Err(JobError::new(ErrorCode::InvalidRequest, "primary samples must be exactly 1, 4 or 16"));
        }

        if let Some(reflections) = &self.reflections {
            reflections.validate()?;
        }
        for (i, target) in self.anisotropic_faces.iter().enumerate().filter_map(|(i, target)| target.as_ref().map(|target| (i, target))) {
            if target.body_name.is_empty()
                || target.body_name.len() > 1024
                || target.body_name.capacity() > 1024
                || !target.tangent.iter().all(|v| v.is_finite())
                || !target.tangent.iter().any(|v| *v != 0.0)
                || ![target.roughness_x, target.roughness_y].into_iter().all(|v| v.is_finite() && (crate::surface::MIN_ROUGHNESS..=1.0).contains(&v))
            {
                return Err(JobError::new(ErrorCode::InvalidRequest, "invalid bounded anisotropic planar face options"));
            }
            if self.anisotropic_faces.iter().skip(i + 1).flatten().any(|other| target.body_index == other.body_index && target.face == other.face) {
                return Err(JobError::new(ErrorCode::InvalidRequest, "duplicate anisotropic captured face"));
            }
        }
        if self.reflections.as_ref().is_some_and(|r| {
            r.bodies.iter().any(|b| b.mode == ReflectionMode::Mirror && self.anisotropic_faces.iter().flatten().any(|t| t.body_name == b.body))
        }) {
            return Err(JobError::new(ErrorCode::InvalidRequest, "delta mirror cannot have anisotropic rough axes"));
        }
        if self.preset.is_some() && self.lights.is_some() {
            return Err(JobError::new(ErrorCode::InvalidRequest, "choose either a lighting preset or custom lights"));
        }
        if !self.exposure_ev.is_finite() || !(-16.0..=16.0).contains(&self.exposure_ev) {
            return Err(JobError::new(ErrorCode::InvalidRequest, "raytrace exposure_ev must be finite and within -16..16"));
        }
        if let Some(lights) = &self.lights {
            if lights.len() > 8 || lights.capacity() > 8 {
                return Err(JobError::new(ErrorCode::InvalidRequest, "at most eight directional lights"));
            }
            for light in lights {
                let direction_valid = light.direction.iter().all(|n| n.is_finite()) && light.direction.iter().any(|n| *n != 0.0);
                if !direction_valid
                    || !light.color.iter().all(|n| n.is_finite() && (0.0..=1.0).contains(n))
                    || !light.intensity.is_finite()
                    || !(0.0..=64.0).contains(&light.intensity)
                {
                    return Err(JobError::new(ErrorCode::InvalidRequest, "invalid bounded directional light"));
                }
            }
        }
        Ok(())
    }
}

/// Fully resolved input; no mutable session, topology graph or GPU resources.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRequest {
    #[serde(default)]
    pub integrator: Integrator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raytrace: Option<RaytraceOptions>,
    pub camera: Camera,
    pub width: u32,
    pub height: u32,
    pub background: [u8; 3],
    pub transparent_background: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColourEncoding {
    Srgb,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaConvention {
    Straight,
}

#[derive(Clone, Debug, Serialize)]
pub struct FrameInfo {
    pub job: JobId,
    pub scene_generation: u64,
    pub backend: BackendId,
    pub executor: ExecutorId,
    pub width: u32,
    pub height: u32,
    pub encoding: ColourEncoding,
    pub alpha: AlphaConvention,
}
#[cfg(test)]
mod runtime_request_roundtrip_tests {
    use super::*;
    #[test]
    fn default_preset_and_empty_lights_requests_roundtrip_without_invalid_null_sequence() {
        for options in [
            RaytraceOptions::default(),
            RaytraceOptions { preset: Some(LightingPreset::Neutral), ..Default::default() },
            RaytraceOptions { lights: Some(Vec::new()), ..Default::default() },
        ] {
            let value = serde_json::to_value(&options).unwrap();
            if options.lights.is_none() {
                assert!(value.get("lights").is_none());
            } else {
                assert_eq!(value.get("lights").unwrap(), &serde_json::json!([]));
            }
            let decoded: RaytraceOptions = serde_json::from_value(value).unwrap();
            assert_eq!(decoded, options);
            assert!(decoded.validate().is_ok());
        }
        assert_eq!(serde_json::from_value::<RaytraceOptions>(serde_json::json!({})).unwrap(), RaytraceOptions::default());
    }
}
