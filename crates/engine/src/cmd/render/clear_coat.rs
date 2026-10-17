//! Request-only coat overrides on immutable captured geometry; no saved appearance changes.
use crate::{Result, params::bad};
use serde::Deserialize;
use solidcraft_geom::Vec3;
use solidcraft_render::{
    ProductScene,
    clear_coat::ClearCoat,
    surface::{OpaqueSurface, srgb8_to_linear},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Override {
    body_index: usize,
    body_name: String,
    weight: f64,
    roughness: f64,
    ior: f64,
}
pub(super) fn apply(scene: &mut ProductScene, overrides: &[Override], command: &str) -> Result<()> {
    if overrides.len() > 64 {
        return Err(bad(command, "at most 64 runtime clear-coat overrides are supported"));
    }
    let mut ordinals = std::collections::BTreeSet::new();
    for input in overrides {
        if input.body_name.is_empty()
            || input.body_name.len() > 1024
            || input.body_name.chars().any(char::is_control)
            || !ordinals.insert(input.body_index)
        {
            return Err(bad(command, "clear coats require bounded names and distinct captured body ordinals"));
        }
        let coat = ClearCoat::new(input.weight, input.roughness, input.ior).map_err(|e| bad(command, e.to_string()))?;
        let body = scene
            .meshes
            .get_mut(input.body_index)
            .filter(|m| m.name == input.body_name)
            .ok_or_else(|| bad(command, "clear-coat body ordinal and exact name must both match render.scene"))?;
        // A zero-weight override is a true no-op, including legacy appearances.
        if coat.weight() == 0.0 {
            continue;
        }
        if body.transmission.is_some() || body.faces.iter().any(|f| f.transmission.is_some()) {
            return Err(bad(command, "clear coat and transmission cannot share a body"));
        }
        let surface = |color: [u8; 3], existing: Option<OpaqueSurface>| -> Result<OpaqueSurface> {
            let [r, g, b] = color.map(srgb8_to_linear);
            let base =
                existing.map(Ok).unwrap_or_else(|| OpaqueSurface::new(Vec3::new(r, g, b), 0.0, 0.5, 1.5)).map_err(|e| bad(command, e.to_string()))?;
            Ok(base.with_clear_coat(coat))
        };
        body.surface = Some(surface(body.color, body.surface)?);
        for face in &mut body.faces {
            face.surface = Some(surface(face.color, face.surface)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn coat_is_runtime_only_and_validates_exact_body_and_values() {
        let mut s = crate::Session::default();
        s.execute("solid.box", &json!({"length":10,"width":10,"height":2,"body_name":"Housing"})).unwrap();
        let saved = s.doc.to_json();
        let original = crate::product_scene::extract(&s).unwrap();
        let input = json!({"body_index":0,"body_name":"Housing","weight":1,"roughness":0.12,"ior":1.5});
        let mut scene = original.clone();
        apply(&mut scene, &[serde_json::from_value(input.clone()).unwrap()], "render.start").unwrap();
        assert!(scene.meshes[0].surface.unwrap().clear_coat().is_some());
        assert!(std::sync::Arc::ptr_eq(&scene.meshes[0].mesh, &original.meshes[0].mesh));
        assert_eq!(s.doc.to_json(), saved);
        assert!(crate::product_scene::extract(&s).unwrap().meshes[0].surface.is_none_or(|s| s.clear_coat().is_none()));
        for (key, value) in
            [("weight", json!(-0.1)), ("roughness", json!(0)), ("ior", json!(4)), ("body_index", json!(1)), ("body_name", json!("Wrong"))]
        {
            let mut bad_input = input.clone();
            bad_input[key] = value;
            assert!(apply(&mut original.clone(), &[serde_json::from_value(bad_input).unwrap()], "render.start").is_err());
        }
        let duplicate = serde_json::from_value::<Vec<Override>>(json!([input, input])).unwrap();
        assert!(apply(&mut original.clone(), &duplicate, "render.start").is_err());
    }
}
