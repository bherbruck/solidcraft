//! Render capability discovery and persistent intent. No rendering or host probing.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use solidcraft_doc::render_settings::{RenderSettings, RenderSetup};

use super::CommandSpec;
use crate::params::bad;
use crate::{Result, Session};

mod jobs;

pub static COMMANDS: &[CommandSpec] = &[
CommandSpec::new("render.preflight", "Check Render Request", jobs::preflight).icon("inspect").noundo().doc_only()
    .params("setup?: saved name OR settings?: full RenderSetup; transparent_background?: false; replace?: false. Validates the installed image host and bounded captured product scene without starting work. Stale geometry is rejected. See docs/render-jobs.md"),
CommandSpec::new("render.start", "Render Image", jobs::start).noundo().doc_only().at("RENDER", "IMAGE").icon("render_image")
    .params("Same request as render.preflight. Captures an immutable product scene and queues a bounded image job. Returns its session-scoped job identity, captured scene_generation and progress. Does not change the design. See docs/render-jobs.md"),
CommandSpec::new("render.status", "Render Progress", jobs::status).icon("list").noundo().doc_only()
    .params("job: {session, serial, generation}. Non-blocking progress, error and completed frame metadata; no image bytes and no rendering in this call."),
CommandSpec::new("render.cancel", "Cancel Render", jobs::cancel).icon("delete").noundo().doc_only()
    .params("job: {session, serial, generation}. Requests cooperative cancellation; completed jobs remain completed. Late cancelled frames cannot be published."),
CommandSpec::new("render.export", "Export Render Image", jobs::export).icon("export").noundo().doc_only()
    .params("job: {session, serial, generation}, path: PNG destination, overwrite?: false. Exports a retained completed image atomically; an existing destination is preserved on failure. See docs/render-jobs.md"),
CommandSpec::new("render.backends", "Render Backends", backends).icon("list")
    .noundo()
    .doc_only()
    .params("No parameters. Returns {schema_version: 1, default_backend: builtin, backends: [{id, label, availability, executors: [{id, availability, profiles, limits, scratch}]}]}. Metadata only; does not start rendering or probe a GPU. See docs/render-backends.md"),
CommandSpec::new("render.settings.get", "Render Settings", get_settings).icon("settings").noundo().doc_only()
    .params("name?: saved setup. Without name returns {setups, defaults}; with name returns {name, setup}. Does not evaluate the model or apply a camera. See docs/render-settings.md"),
CommandSpec::new("render.settings.set", "Save Render Settings", set_settings).icon("settings").doc_only()
    .params("name: setup name, set: object of fields to replace (backend, mode, camera, width, height, format, exposure_ev, environment_rotation, environment_intensity, background). Creates from defaults, validates atomically, one undo step; identical edits are no-ops. See docs/render-settings.md"),
CommandSpec::new("render.scene", "Inspect Render Scene", scene).noundo().at("RENDER", "SCENE").icon("render_scene")
    .params("No parameters. Authoritative product-scene metadata, without modelling overlays or mesh payloads. Explicitly settles stale geometry; does not render an image. See docs/render-workspace.md"),
CommandSpec::new("render.settings.apply", "Apply Render Camera", apply_settings).icon("camera").noundo().doc_only()
    .params("name: saved setup. Applies its existing Camera snapshot including roll/up as view state, without modifying the document or starting a render."),
];

fn scene(s: &mut Session, params: &Value) -> Result<Value> {
    if !params.as_object().is_some_and(|p| p.is_empty()) {
        return Err(bad("render.scene", "expected an empty parameter object"));
    }
    let scene = crate::product_scene::extract(s)?;
    let bodies: Vec<_> = scene
        .meshes
        .iter()
        .map(|m| json!({"name":m.name,"triangles":m.mesh.triangles.len(),"color":m.color,"opacity":m.opacity,"face_appearances":m.faces.len()}))
        .collect();
    Ok(
        json!({"bodies":bodies,"bounds":if scene.bounds.is_empty() { Value::Null } else { json!({"min":scene.bounds.min,"max":scene.bounds.max}) },"overlays":false}),
    )
}

fn apply_settings(s: &mut Session, params: &Value) -> Result<Value> {
    let request: GetSettings = serde_json::from_value(params.clone()).map_err(|e| bad("render.settings.apply", e.to_string()))?;
    let name = request.name.ok_or_else(|| bad("render.settings.apply", "`name` is required"))?;
    RenderSettings::validate_name(&name).map_err(|e| bad("render.settings.apply", e.to_string()))?;
    let setup = s.doc.render.setups.get(&name).ok_or_else(|| bad("render.settings.apply", "no saved render setup with that name"))?;
    setup.validate().map_err(|e| bad("render.settings.apply", e.to_string()))?;
    s.camera = Some(setup.camera);
    Ok(json!({"name":name,"camera":setup.camera}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetSettings {
    name: Option<String>,
}

fn get_settings(s: &mut Session, params: &Value) -> Result<Value> {
    let cmd = "render.settings.get";
    let request: GetSettings = serde_json::from_value(params.clone()).map_err(|e| bad(cmd, e.to_string()))?;
    match request.name {
        None => Ok(json!({"setups": s.doc.render.setups, "defaults": RenderSetup::default()})),
        Some(name) => {
            RenderSettings::validate_name(&name).map_err(|e| bad(cmd, e.to_string()))?;
            let setup = s.doc.render.setups.get(&name).ok_or_else(|| bad(cmd, "no saved render setup with that name"))?;
            Ok(json!({"name": name, "setup": setup}))
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetSettings {
    name: String,
    set: Value,
}

fn set_settings(s: &mut Session, params: &Value) -> Result<Value> {
    let cmd = "render.settings.set";
    let request: SetSettings = serde_json::from_value(params.clone()).map_err(|e| bad(cmd, e.to_string()))?;
    RenderSettings::validate_name(&request.name).map_err(|e| bad(cmd, e.to_string()))?;
    let patch = request.set.as_object().filter(|p| p.len() <= 10).ok_or_else(|| bad(cmd, "`set` must be an object with at most 10 fields"))?;
    let current = s.doc.render.setups.get(&request.name).cloned().unwrap_or_default();
    let mut value = serde_json::to_value(&current).map_err(|e| bad(cmd, e.to_string()))?;
    let object = value.as_object_mut().ok_or_else(|| bad(cmd, "invalid render setup"))?;
    for (key, value) in patch {
        object.insert(key.clone(), value.clone());
    }
    let setup: RenderSetup = serde_json::from_value(value).map_err(|e| bad(cmd, e.to_string()))?;
    setup.validate().map_err(|e| bad(cmd, e.to_string()))?;
    let reply = json!({"name": request.name, "setup": setup});
    if s.doc.render.setups.get(&request.name) == Some(&setup) {
        return Ok(reply);
    }
    let mut settings = (*s.doc.render).clone();
    settings.setups.insert(request.name, setup);
    settings.validate().map_err(|e| bad(cmd, e.to_string()))?;
    s.doc_mut().render = Arc::new(settings);
    Ok(reply)
}

fn backends(s: &mut Session, params: &Value) -> Result<Value> {
    if !params.as_object().is_some_and(|p| p.is_empty()) {
        return Err(bad("render.backends", "expected an empty parameter object"));
    }
    Ok(json!({"schema_version": 1, "default_backend": "builtin", "backends": [s.render_backend()]}))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn assert_unchanged(s: &mut Session, params: Value) -> Result<Value> {
        let doc = s.doc.clone();
        let before = (s.is_dirty(), s.revision, s.evaluations, s.model.is_stale());
        let camera = s.camera;
        let undo = s.undo.clone();
        let redo = s.redo.clone();
        let result = s.execute("render.backends", &params);
        assert!(Arc::ptr_eq(&doc, &s.doc));
        assert_eq!(before, (s.is_dirty(), s.revision, s.evaluations, s.model.is_stale()));
        assert_eq!(camera, s.camera);
        for (before, after) in [(&undo, &s.undo), (&redo, &s.redo)] {
            assert_eq!(before.len(), after.len());
            for (a, b) in before.iter().zip(after.iter()) {
                assert!(Arc::ptr_eq(&a.doc, &b.doc));
                assert_eq!((&a.label, a.active_sketch, &a.pending_moves), (&b.label, b.active_sketch, &b.pending_moves));
            }
        }
        result
    }

    #[test]
    fn discovery_preserves_clean_dirty_and_undo_redo_state() {
        let mut s = Session::default();
        let r = assert_unchanged(&mut s, Value::Null).unwrap();
        assert_eq!(r["schema_version"], 1);
        assert_eq!(r["default_backend"], "builtin");
        assert_eq!(r["backends"].as_array().unwrap().len(), 1);
        let descriptor: crate::render::BackendDescriptor = serde_json::from_value(r["backends"][0].clone()).unwrap();
        assert_eq!(descriptor, crate::render::builtin_backend());
        s.execute("solid.box", &json!({"length": 10, "width": 20, "height": 5})).unwrap();
        s.execute("view.apply", &json!({"view": "top"})).unwrap();
        assert!(s.is_dirty());
        assert_unchanged(&mut s, json!({})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(!s.redo.is_empty());
        assert_unchanged(&mut s, json!({})).unwrap();
        for p in [json!([]), json!(3), json!({"backend": "missing"}), json!({"width": "bad"})] {
            assert!(assert_unchanged(&mut s, p).is_err());
        }
    }

    #[test]
    fn discovery_does_not_settle_a_live_drag_or_rebuild_geometry() {
        let mut s = Session::default();
        for (id, p) in [
            ("sketch.create", json!({"plane": "XY"})),
            ("sketch.rectangle.two_point", json!({"p0": [5, 5], "p1": [25, 20]})),
            ("sketch.dimension", json!({"entities": ["l1"], "value": 20})),
            ("sketch.dimension", json!({"entities": ["l2"], "value": 15})),
            ("sketch.finish", json!({})),
            ("solid.extrude", json!({"distance": 5})),
            ("sketch.edit", json!({"sketch": "Sketch1"})),
            ("sketch.drag", json!({"entity": "l1.start", "from": [5, 5], "to": [4, 5], "live": true})),
        ] {
            s.execute(id, &p).unwrap();
        }
        assert!(s.model.is_stale());
        s.evaluations = 0;
        let model = s.model.state();
        assert_unchanged(&mut s, json!({})).unwrap();
        assert_unchanged(&mut s, json!({"unexpected": true})).unwrap_err();
        assert!(Arc::ptr_eq(&model, &s.model.state()));
        assert_eq!(s.evaluations, 0);
    }
}

#[cfg(test)]
mod settings_tests;

#[cfg(test)]
mod job_tests;
