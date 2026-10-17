//! Agreed #1659 runtime wire adapter, coordinated in Mailbox 9075.
//! Keep wire details here until the CPU contract is published. Never save them.
use crate::{Result, params::bad};
use serde::Deserialize;
use solidcraft_geom::Vec3;
use solidcraft_render::{
    ProductScene,
    surface::{OpaqueSurface, srgb8_to_linear},
    transmission::Transmission,
};
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Raytrace {
    pub dielectrics: Vec<Override>,
    pub clear_coats: Vec<super::clear_coat::Override>,
    pub textures: Vec<super::textures::Override>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Override {
    body_index: usize,
    body_name: String,
    ior: f64,
    absorption_per_mm: [f64; 3],
    boundary: Boundary,
    thickness_mm: f64,
    #[serde(default)]
    shadow_transmission: Option<bool>,
}
#[derive(Deserialize)]
#[serde(try_from = "String")]
enum Boundary {
    ThinSheet,
    ClosedBox,
    ClosedMesh,
}
impl TryFrom<String> for Boundary {
    type Error = &'static str;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        match value.as_str() {
            "thin_sheet" => Ok(Self::ThinSheet),
            "closed_box" => Ok(Self::ClosedBox),
            "closed_mesh" => Ok(Self::ClosedMesh),
            _ => Err("boundary must be thin_sheet, closed_box or closed_mesh"),
        }
    }
}
pub(super) fn apply(scene: &mut ProductScene, overrides: &[Override], command: &str) -> Result<()> {
    if overrides.len() > 8 {
        return Err(bad(command, "at most eight runtime dielectric overrides are supported"));
    }
    let mut ordinals = std::collections::BTreeSet::new();
    for input in overrides {
        if input.body_name.is_empty()
            || input.body_name.len() > 1024
            || input.body_name.chars().any(char::is_control)
            || !ordinals.insert(input.body_index)
        {
            return Err(bad(command, "dielectrics require bounded names and distinct captured body ordinals"));
        }
        let thickness = match input.boundary {
            Boundary::ThinSheet => Some(input.thickness_mm),
            _ => None,
        };
        if !matches!(input.boundary, Boundary::ThinSheet) && input.thickness_mm != 0.0 {
            return Err(bad(command, "closed dielectric thickness_mm must be zero; distance comes from geometry"));
        }
        let [r, g, b] = input.absorption_per_mm;
        let mut transmission = Transmission::new(Vec3::new(r, g, b), thickness).map_err(|e| bad(command, e.to_string()))?;
        if let Some(enabled) = input.shadow_transmission {
            transmission = transmission.with_shadow_transmission(enabled);
        }
        OpaqueSurface::new(Vec3::ZERO, 0.0, 0.5, input.ior).map_err(|e| bad(command, e.to_string()))?;
        let body = scene
            .meshes
            .get_mut(input.body_index)
            .filter(|m| m.name == input.body_name)
            .ok_or_else(|| bad(command, "dielectric body ordinal and exact name must both match render.scene"))?;
        if matches!(input.boundary, Boundary::ClosedBox) {
            certify_box(&body.mesh, command)?;
        }
        // Full dielectric override replaces optical metalness in the job snapshot only.
        let surface = |color: [u8; 3], existing: Option<OpaqueSurface>| {
            let [r, g, b] = color.map(srgb8_to_linear);
            OpaqueSurface::new(Vec3::new(r, g, b), 0.0, existing.map_or(0.5, |s| s.roughness()), input.ior).map_err(|e| bad(command, e.to_string()))
        };
        body.surface = Some(surface(body.color, body.surface)?);
        body.transmission = Some(transmission);
        for face in &mut body.faces {
            face.surface = Some(surface(face.color, face.surface)?);
            face.transmission = Some(transmission);
        }
    }
    Ok(())
}
fn certify_box(mesh: &solidcraft_geom::Mesh, command: &str) -> Result<()> {
    let bounds = mesh.bounds();
    if mesh.triangles.is_empty()
        || !bounds.min.is_finite()
        || !bounds.max.is_finite()
        || bounds.min.x >= bounds.max.x
        || bounds.min.y >= bounds.max.y
        || bounds.min.z >= bounds.max.z
    {
        return Err(bad(command, "closed_box requires a nonempty axis-aligned box"));
    }
    for triangle in &mesh.triangles {
        let points = triangle.map(|i| mesh.positions.get(i as usize).copied());
        let [Some(a), Some(b), Some(c)] = points else {
            return Err(bad(command, "invalid closed_box triangle"));
        };
        let on_plane = |x: f64, y: f64, z: f64, lo: f64, hi: f64| {
            [lo, hi].into_iter().any(|plane| [x, y, z].into_iter().all(|v| v.is_finite() && (v - plane).abs() <= 1e-6))
        };
        if !(on_plane(a.x, b.x, c.x, bounds.min.x, bounds.max.x)
            || on_plane(a.y, b.y, c.y, bounds.min.y, bounds.max.y)
            || on_plane(a.z, b.z, c.z, bounds.min.z, bounds.max.z))
        {
            return Err(bad(command, "closed_box requires axis-aligned bounding planes; use GPU-only closed_mesh for other closed geometry"));
        }
    }
    // GPU preparation additionally certifies closed, outward two-manifold topology.
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    fn fixture() -> crate::Session {
        let mut s = crate::Session::default();
        s.execute("solid.box", &json!({"length":10,"width":10,"height":2,"body_name":"Glass"})).unwrap();
        s
    }
    fn input() -> serde_json::Value {
        json!({"body_index":0,"body_name":"Glass","ior":1.5,"absorption_per_mm":[0.1,0.02,0],"boundary":"closed_box","thickness_mm":0})
    }
    #[test]
    fn request_glass_leaves_document_geometry_and_preview_unchanged() {
        let mut s = fixture();
        s.execute("appearance.assign", &json!({"bodies":["Glass"],"color":"#c0e0ff","opacity":0.2})).unwrap();
        let saved = s.doc.to_json();
        let mut scene = crate::product_scene::extract(&s).unwrap();
        let mesh = scene.meshes[0].mesh.clone();
        apply(&mut scene, &[serde_json::from_value(input()).unwrap()], "render.start").unwrap();
        assert!(scene.has_transmission());
        assert_eq!(scene.meshes[0].opacity, 0.2);
        assert!(Arc::ptr_eq(&mesh, &scene.meshes[0].mesh));
        assert_eq!(scene.meshes[0].surface.unwrap().dielectric_ior(), 1.5);
        assert_eq!(saved, s.doc.to_json());
        let reopened = crate::Session::new(solidcraft_doc::Document::from_json(&saved).unwrap());
        assert!(!crate::product_scene::extract(&reopened).unwrap().has_transmission());
        assert!(
            s.execute(
                "appearance.assign",
                &json!({"bodies":["Glass"],"color":"#ffffff",
            "pbr":{"metallic":0,"roughness":0.1,"dielectric_ior":1.5,"transmission":{}}})
            )
            .is_err()
        );
    }
    #[test]
    fn request_glass_rejects_unknown_fractional_duplicate_and_mismatched_inputs() {
        let s = fixture();
        for (key, value) in [
            ("weight", json!(0.5)),
            ("ior", json!(0.9)),
            ("absorption_per_mm", json!([100.01, 0, 0])),
            ("body_index", json!(1)),
            ("body_name", json!("missing")),
            ("thickness_mm", json!(-1)),
            ("boundary", json!({"thin_sheet":null})),
            ("extra", json!(true)),
        ] {
            let mut value_input = input();
            value_input[key] = value;
            let rejected = match serde_json::from_value::<Override>(value_input) {
                Ok(value) => apply(&mut crate::product_scene::extract(&s).unwrap(), &[value], "render.start").is_err(),
                Err(_) => true,
            };
            assert!(rejected, "{key}");
        }
        for count in [2, 9] {
            let overrides = serde_json::from_value::<Vec<Override>>(json!(vec![input(); count])).unwrap();
            assert!(apply(&mut crate::product_scene::extract(&s).unwrap(), &overrides, "render.start").is_err());
        }
        let mut thin = input();
        thin["boundary"] = json!("thin_sheet");
        assert!(apply(&mut crate::product_scene::extract(&s).unwrap(), &[serde_json::from_value(thin).unwrap()], "render.start").is_ok());
    }
    #[test]
    fn closed_box_certificate_rejects_deformed_mesh_but_closed_mesh_is_explicit() {
        let mut scene = crate::product_scene::extract(&fixture()).unwrap();
        Arc::make_mut(&mut scene.meshes[0].mesh).positions[0] += Vec3::new(0.2, 0.3, 0.4);
        assert!(apply(&mut scene.clone(), &[serde_json::from_value(input()).unwrap()], "render.start").is_err());
        let mut value = input();
        value["boundary"] = json!("closed_mesh");
        assert!(apply(&mut scene, &[serde_json::from_value(value).unwrap()], "render.start").is_ok());
    }
    #[test]
    fn shadow_flags_follow_boundary_defaults_and_explicit_runtime_overrides() {
        let s = fixture();
        let saved = s.doc.to_json();
        for thin in [false, true] {
            for flag in [None, Some(false), Some(true)] {
                let mut value = input();
                if thin {
                    value["boundary"] = json!("thin_sheet");
                }
                if let Some(flag) = flag {
                    value["shadow_transmission"] = json!(flag);
                }
                let mut scene = crate::product_scene::extract(&s).unwrap();
                apply(&mut scene, &[serde_json::from_value(value).unwrap()], "render.start").unwrap();
                assert_eq!(scene.meshes[0].transmission.unwrap().shadow_transmission(), flag.unwrap_or(thin));
                assert_eq!(s.doc.to_json(), saved);
            }
        }
        let mut malformed = input();
        malformed["shadow_transmission"] = json!(0.5);
        assert!(serde_json::from_value::<Override>(malformed).is_err());
    }
}
