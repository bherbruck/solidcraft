//! Product extraction shares evaluated placement and appearance resolution, not viewport policy.

use crate::geom::Aabb3;
use crate::render::{FaceAppearance, ProductMesh, ProductScene};
use crate::{EngineError, Result, Session};

pub const MAX_PRODUCT_BODIES: usize = 16_384;

/// Extract authoritative, visible geometry. Does not settle a live gesture implicitly.
/// `render.scene` is the explicit model-reading command for that boundary.
pub fn extract(s: &Session) -> Result<ProductScene> {
    if s.model.is_stale() {
        return Err(EngineError::Other("render scene is stale; finish the modelling gesture before previewing".into()));
    }
    let world = s.world_state();
    let mut scene = ProductScene { meshes: Vec::new(), bounds: Aabb3::EMPTY };
    for body in world.bodies.iter().filter(|b| s.body_visible(&b.name)) {
        if scene.meshes.len() >= MAX_PRODUCT_BODIES {
            return Err(EngineError::Other("render scene exceeds 16384 visible bodies".into()));
        }
        let mesh = body.mesh();
        scene.bounds = scene.bounds.union(&mesh.bounds());
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let color = s.doc.body_color(body).map(|c| c.map(to8)).unwrap_or([176, 186, 198]);
        let faces = s
            .doc
            .face_colors(body)
            .into_iter()
            .map(|(face, look)| {
                Ok(FaceAppearance {
                    face: u32::try_from(face).map_err(|_| EngineError::Other("render face index exceeds the mesh contract".into()))?,
                    color: look.color,
                    opacity: look.opacity.clamp(0.0, 1.0),
                    surface: look.opaque_surface().map_err(|e| EngineError::Other(e.to_string()))?,
                    transmission: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let surface = s.doc.model_body_look(body).map(|l| l.opaque_surface()).transpose().map_err(|e| EngineError::Other(e.to_string()))?.flatten();
        scene.meshes.push(ProductMesh {
            name: body.name.clone(),
            mesh,
            color,
            opacity: s.doc.model_body_opacity(body).clamp(0.0, 1.0),
            surface,
            transmission: None,
            faces,
        });
    }
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sel;
    use crate::geom::Vec3;
    use crate::render::*;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn product_scene_keeps_placed_appearances_and_meshes_without_design_overlays() {
        let mut s = Session::default();
        let component = s.execute("component.create", &json!({"name":"Part"})).unwrap();
        s.execute("solid.box", &json!({"length":10,"width":10,"height":10,"body_name":"Block"})).unwrap();
        s.execute("appearance.assign", &json!({"bodies":["Block"],"color":"#20a080","opacity":0.4})).unwrap();
        s.execute("appearance.assign", &json!({"faces":[[5,5,10]],"color":"#ff0000","opacity":0.7})).unwrap();
        s.execute("component.activate", &json!({"component":0})).unwrap();
        s.execute("occurrence.copy", &json!({"component":component["component"],"translate":[30,0,0]})).unwrap();
        s.execute("occurrence.move", &json!({"occurrence":component["occurrence"],"translate":[10,0,0]})).unwrap();
        let doc = s.doc.clone();
        s.evaluations = 0;
        let first = extract(&s).unwrap();
        assert_eq!(first.meshes.len(), 2);
        assert_eq!(first.bounds.min, Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(first.bounds.max, Vec3::new(40.0, 10.0, 10.0));
        for m in &first.meshes {
            assert_eq!(m.color, [32, 160, 128]);
            assert_eq!(m.opacity, 0.4);
            assert!(m.faces.iter().any(|f| f.color == [255, 0, 0] && f.opacity == 0.7));
            assert!(Arc::ptr_eq(&m.mesh, &s.world_state().body(&m.name).unwrap().mesh()));
        }
        s.selection = vec![Sel::Body { name: first.meshes[0].name.clone() }];
        s.section = Some((Vec3::ZERO, Vec3::X));
        s.analysis = Some(crate::SurfaceAnalysis::Zebra { stripes: 10.0 });
        let after = extract(&s).unwrap();
        for (a, b) in first.meshes.iter().zip(&after.meshes) {
            assert!(Arc::ptr_eq(&a.mesh, &b.mesh));
            assert_eq!((a.color, a.opacity, &a.faces), (b.color, b.opacity, &b.faces));
        }
        s.active_component = component["component"].as_u64().unwrap();
        let active = extract(&s).unwrap();
        assert_eq!(active.bounds.min, first.bounds.min);
        assert_eq!(active.meshes[1].opacity, 0.4, "no active-component fading");
        s.visibility.hidden_bodies.push(first.meshes[1].name.clone());
        assert_eq!(extract(&s).unwrap().meshes.len(), 1);
        assert!(Arc::ptr_eq(&doc, &s.doc));
        assert_eq!(s.evaluations, 0);
    }

    #[test]
    fn stale_geometry_requires_an_explicit_model_read_and_camera_apply_preserves_staleness() {
        let mut s = Session::default();
        for (id, p) in [
            ("sketch.create", json!({"plane":"XY"})),
            ("sketch.rectangle.two_point", json!({"p0":[5,5],"p1":[25,20]})),
            ("sketch.dimension", json!({"entities":["l1"],"value":20})),
            ("sketch.dimension", json!({"entities":["l2"],"value":15})),
            ("sketch.finish", json!({})),
            ("solid.extrude", json!({"distance":5})),
            ("sketch.edit", json!({"sketch":"Sketch1"})),
            ("sketch.drag", json!({"entity":"l1.start","from":[5,5],"to":[4,5],"live":true})),
        ] {
            s.execute(id, &p).unwrap();
        }
        assert!(s.model.is_stale());
        assert!(extract(&s).is_err());
        s.evaluations = 0;
        let camera = Camera { up: UpAxis::Y, roll: 0.25, ..Default::default() };
        s.execute("render.settings.set", &json!({"name":"Photo","set":{"camera":camera}})).unwrap();
        let doc = s.doc.clone();
        let undo = s.undo.len();
        s.execute("render.settings.apply", &json!({"name":"Photo"})).unwrap();
        assert_eq!(s.camera, Some(camera));
        assert!(s.model.is_stale());
        assert_eq!(s.evaluations, 0);
        assert!(Arc::ptr_eq(&doc, &s.doc));
        assert_eq!(s.undo.len(), undo);
        let metadata = s.execute("render.scene", &json!({})).unwrap();
        assert!(!s.model.is_stale());
        assert!(s.evaluations > 0);
        assert_eq!(metadata["overlays"], false);
        assert_eq!(metadata["bodies"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn host_capabilities_are_validated_runtime_only_and_survive_new_design() {
        let mut s = Session::default();
        let viewport = ExecutorDescriptor {
            id: ExecutorId::new("gpu-viewport").unwrap(),
            availability: Availability::Available,
            profiles: vec![RenderProfile {
                mode: RenderMode::Preview,
                operation: RenderOperation::Viewport,
                formats: Vec::new(),
                features: Vec::new(),
            }],
            limits: RenderLimits::default(),
            scratch: ScratchLayout { sample_scale: 1, sample_bytes_per_pixel: 8, output_bytes_per_pixel: 8, encoding_bytes_per_pixel: 0 },
        };
        let doc = s.doc.clone();
        s.set_render_viewport(Some(viewport.clone())).unwrap();
        assert!(Arc::ptr_eq(&doc, &s.doc));
        assert_eq!(s.evaluations, 0);
        assert!(!s.is_dirty());
        let backend = s.render_backend();
        s.replace_design(Session::default());
        assert_eq!(s.render_viewport(), Some(&viewport));
        assert_eq!(s.render_backend(), backend);
        assert_eq!(backend.executors.len(), 2);
        assert_eq!(s.scratch().render_backend(), backend);
        let mut invalid = viewport;
        invalid.id = ExecutorId::new("cpu-raster").unwrap();
        assert!(s.set_render_viewport(Some(invalid)).is_err());
        assert_eq!(s.render_backend(), backend);
        s.execute("file.new", &json!({})).unwrap();
        assert_eq!(s.render_backend(), backend);
        assert!(!s.doc.to_json().contains("gpu-viewport"));
        s.set_render_viewport(None).unwrap();
        assert_eq!(s.render_backend(), builtin_backend());
    }

    #[test]
    fn imported_looks_survive_product_extraction_and_design_overrides() {
        let body = crate::kernel::box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0))
            .unwrap()
            .with_color(Some([0.2, 0.6, 0.8]))
            .with_paint(Some(crate::kernel::Paint { opacity: 0.4, faces: Vec::new() }));
        let step = crate::kernel::step_export_bodies(
            &[crate::kernel::ExportBody { name: "Imported glass".into(), body: &body, color: body.color() }],
            &Default::default(),
        )
        .unwrap();
        let mut document = crate::doc::Document::default();
        document
            .add_feature(
                crate::doc::FeatureKind::Import {
                    file: "original-generated.step".into(),
                    step: step.into(),
                    components: Vec::new(),
                    skip: Vec::new(),
                },
                None,
            )
            .unwrap();
        let mut s = Session::new(document);
        let first = extract(&s).unwrap();
        assert_eq!(first.meshes[0].color, [51, 153, 204]);
        assert!((first.meshes[0].opacity - 0.4).abs() < 1e-6);
        let name = first.meshes[0].name.clone();
        s.execute("appearance.assign", &json!({"bodies":[name],"color":"#336699","opacity":0.7})).unwrap();
        let changed = extract(&s).unwrap();
        assert_eq!(changed.meshes[0].color, [51, 102, 153]);
        assert_eq!(changed.meshes[0].opacity, 0.7);
        assert!(Arc::ptr_eq(&first.meshes[0].mesh, &changed.meshes[0].mesh));
    }
}
