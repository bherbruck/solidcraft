//! Bounded immutable file capture; decoding is deferred to the admitted worker.
use crate::{Result, params::bad};
use serde::Deserialize;
use solidcraft_render::{
    ProductScene,
    job::textures::{MAX_TEXTURE_BINDINGS, MAX_TEXTURE_SOURCE_BYTES, PlanarFrame, RuntimeTextures, TextureBinding, TextureMap},
    texture_input::MapRole,
};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Override {
    body_index: usize,
    body_name: String,
    frame: PlanarFrame,
    #[serde(default)]
    base_color_srgb: Option<String>,
    #[serde(default)]
    normal_opengl: Option<String>,
    #[serde(default = "normal_strength")]
    normal_strength: f64,
}
fn normal_strength() -> f64 {
    1.0
}
fn load(path: &str, role: MapRole, remaining: &mut u64, command: &str) -> Result<TextureMap> {
    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(bad(command, "texture path must be a bounded local path"));
    }
    let bytes = solidcraft_io::vfs::read(path, *remaining as usize).map_err(|e| bad(command, format!("could not capture texture: {e}")))?;
    if bytes.is_empty() {
        return Err(bad(command, "texture file must not be empty"));
    }
    let map = TextureMap::new(bytes, role).map_err(|e| bad(command, e.to_string()))?;
    *remaining = remaining.checked_sub(map.retained_bytes() as u64).ok_or_else(|| bad(command, "encoded texture capacity exceeds budget"))?;
    Ok(map)
}
pub(super) fn capture(scene: &ProductScene, overrides: &[Override], command: &str) -> Result<Option<Arc<RuntimeTextures>>> {
    if overrides.is_empty() {
        return Ok(None);
    }
    if overrides.len() > MAX_TEXTURE_BINDINGS {
        return Err(bad(command, "at most 16 runtime texture bodies are supported"));
    }
    let mut remaining = MAX_TEXTURE_SOURCE_BYTES;
    let mut bindings = Vec::new();
    for input in overrides {
        input.frame.validate().map_err(|e| bad(command, e.to_string()))?;
        // Refuse stale identity or unsupported optics before touching any file.
        let body = scene
            .meshes
            .get(input.body_index)
            .filter(|b| b.name == input.body_name)
            .ok_or_else(|| bad(command, "texture ordinal and name must both match render.scene"))?;
        if body.transmission.is_some() || body.opacity != 1.0 || body.faces.iter().any(|f| f.transmission.is_some() || f.opacity != 1.0) {
            return Err(bad(command, "runtime maps support opaque bodies only"));
        }
        let base_color = input.base_color_srgb.as_ref().map(|p| load(p, MapRole::BaseColor, &mut remaining, command)).transpose()?;
        let normal = input.normal_opengl.as_ref().map(|p| load(p, MapRole::NormalOpenGl, &mut remaining, command)).transpose()?;
        bindings.push(TextureBinding {
            body_index: input.body_index,
            body_name: input.body_name.clone(),
            frame: input.frame,
            normal_strength: input.normal_strength,
            base_color,
            normal,
        });
    }
    let resources = RuntimeTextures::new(bindings).map_err(|e| bad(command, e.to_string()))?;
    resources.validate_scene(scene).map_err(|e| bad(command, e.to_string()))?;
    Ok(Some(Arc::new(resources)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use solidcraft_geom::{Aabb3, Mesh};
    #[test]
    fn unsupported_or_stale_map_targets_fail_before_file_io() {
        let scene = ProductScene {
            bounds: Aabb3::EMPTY,
            meshes: vec![solidcraft_render::ProductMesh {
                name: "Glass".into(),
                mesh: Arc::new(Mesh::default()),
                color: [255; 3],
                opacity: 1.,
                surface: None,
                transmission: Some(solidcraft_render::transmission::Transmission::new(solidcraft_geom::Vec3::ZERO, Some(1.)).unwrap()),
                faces: vec![],
            }],
        };
        let value =
            json!({"body_index":0,"body_name":"Glass","frame":{"origin":[0,0,0],"u":[1,0,0],"v":[0,1,0]},"base_color_srgb":"nonexistent-image.png"});
        let input: Override = serde_json::from_value(value.clone()).unwrap();
        assert!(capture(&scene, &[input], "render.start").unwrap_err().to_string().contains("opaque bodies"));
        let mut stale = value.clone();
        stale["body_name"] = json!("Stale");
        let input: Override = serde_json::from_value(stale).unwrap();
        assert!(capture(&scene, &[input], "render.start").unwrap_err().to_string().contains("ordinal and name"));
        let mut unknown = value;
        unknown["metallic_map"] = json!("foo");
        assert!(serde_json::from_value::<Override>(unknown).is_err());
    }
}
