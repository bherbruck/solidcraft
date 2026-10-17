//! Runtime environment inputs. Captures bytes through the caller's scoped VFS.
use crate::{Result, params::bad};
use serde::Deserialize;
use solidcraft_render::lighting::{
    MAX_HDR_BYTES,
    environment::{EnvironmentResource, EnvironmentSource, SkyGradient},
};
use std::sync::Arc;
fn unit() -> f64 {
    1.0
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Input {
    Hdr {
        path: String,
        #[serde(default)]
        rotation_rad: f64,
        #[serde(default = "unit")]
        intensity: f64,
        #[serde(default)]
        assume_linear_srgb: bool,
    },
    Sky {
        #[serde(default)]
        gradient: SkyGradient,
        #[serde(default)]
        rotation_rad: f64,
        #[serde(default = "unit")]
        intensity: f64,
    },
}
impl Input {
    pub(super) fn capture(&self, command: &str) -> Result<EnvironmentResource> {
        let source = match self {
            Self::Hdr { path, rotation_rad, intensity, assume_linear_srgb } => {
                if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
                    return Err(bad(command, "bounded HDR path required"));
                }
                let bytes = solidcraft_io::vfs::read(path, MAX_HDR_BYTES).map_err(|e| bad(command, format!("could not capture HDR: {e}")))?;
                EnvironmentSource::hdr(bytes, *rotation_rad, *intensity, *assume_linear_srgb)
            }
            Self::Sky { gradient, rotation_rad, intensity } => EnvironmentSource::sky(*gradient, *rotation_rad, *intensity),
        }
        .map_err(|e| bad(command, e.to_string()))?;
        Ok(Arc::new(source))
    }
}
