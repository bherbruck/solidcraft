use std::sync::Arc;

use serde_json::json;
use solidcraft_engine::Sel;

use crate::scenario::Harness;

fn fixture() -> (Harness, u64, u64) {
    let mut h = Harness::new();
    let part = h.app.run("component.create", json!({"name":"Block"})).unwrap();
    let component = part["component"].as_u64().unwrap();
    let occurrence = part["occurrence"].as_u64().unwrap();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
    h.app.run("component.activate", json!({"component":0})).unwrap();
    (h, component, occurrence)
}

#[test]
fn navigating_and_selecting_reuse_placements_but_rows_read_live_view_state() {
    let (mut h, component, occurrence) = fixture();
    let first = super::model_placement(&mut h.app, &[occurrence]).unwrap();
    h.app.cam.yaw += 0.25;
    h.app.run("select.set", json!({"items":[Sel::Body{name:"Body1".into()}]})).unwrap();
    h.app.session.visibility.hidden_bodies.push("Body1".into());
    h.app.session.visibility.locked_bodies.push("Body1".into());
    let after = super::model_placement(&mut h.app, &[occurrence]).unwrap();
    assert!(Arc::ptr_eq(&first, &after), "view-only changes resolved the same placement again");
    let rows = super::entries(&h.app, component, "bodies");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].selected && rows[0].locked && !rows[0].visible);
    h.frames(2);
    assert!(Arc::ptr_eq(&after, &super::model_placement(&mut h.app, &[occurrence]).unwrap()));
}

#[test]
fn pending_and_reverted_moves_replace_cached_transforms() {
    let (mut h, _, occurrence) = fixture();
    let first = super::model_placement(&mut h.app, &[occurrence]).unwrap();
    h.app.run("occurrence.move", json!({"occurrence":occurrence,"translate":[20,0,0],"capture":false})).unwrap();
    let moved = super::model_placement(&mut h.app, &[occurrence]).unwrap();
    assert!(!Arc::ptr_eq(&first, &moved));
    assert!((solidcraft_engine::doc::apply_point(&moved.world, solidcraft_engine::geom::Vec3::ZERO).x - 20.0).abs() < 1e-9);
    h.app.run("component.revert_position", json!({"occurrences":[occurrence]})).unwrap();
    let reverted = super::model_placement(&mut h.app, &[occurrence]).unwrap();
    assert!(!Arc::ptr_eq(&moved, &reverted));
    assert_eq!(first.world, reverted.world);
}

#[test]
fn feature_indices_follow_component_edits_and_undo() {
    let (mut h, component, _) = fixture();
    super::prepare_model(&mut h.app);
    assert!(super::model_feature_indices(&h.app, 0, true).is_empty());
    h.app.run("sketch.create", json!({"plane":"XY", "name":"Root sketch"})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    super::prepare_model(&mut h.app);
    assert_eq!(super::entries(&h.app, 0, "sketches")[0].label, "Root sketch");
    assert!(super::entries(&h.app, component, "sketches").is_empty());
    h.app.run("component.activate", json!({"component":component})).unwrap();
    h.app.run("sketch.create", json!({"plane":"YZ", "name":"Part sketch"})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    super::prepare_model(&mut h.app);
    assert_eq!(super::entries(&h.app, component, "sketches")[0].label, "Part sketch");
    assert_eq!(super::entries(&h.app, 0, "sketches").len(), 1);
    h.app.run("edit.undo", json!({})).unwrap();
    super::prepare_model(&mut h.app);
    assert!(super::entries(&h.app, component, "sketches").is_empty());
    assert_eq!(super::entries(&h.app, 0, "sketches").len(), 1);
}

#[test]
fn body_index_and_placements_follow_geometry_undo_and_document_identity() {
    let (mut h, component, occurrence) = fixture();
    let first = super::model_placement(&mut h.app, &[occurrence]).unwrap();
    let copy = h.app.run("occurrence.copy", json!({"occurrence":occurrence,"translate":[50,0,0]})).unwrap()["occurrence"].as_u64().unwrap();
    let second = super::model_placement(&mut h.app, &[copy]).unwrap();
    assert_ne!(first.path, second.path);
    assert_ne!(first.bodies[0].world, second.bodies[0].world);
    assert_eq!(super::model_children(&h.app, 0), [occurrence, copy]);
    h.app.run("component.activate", json!({"component":component})).unwrap();
    h.app.run("solid.box", json!({"length":2,"width":2,"height":2,"corner":[20,0,0]})).unwrap();
    let changed = super::model_placement(&mut h.app, &[occurrence]).unwrap();
    assert_eq!(changed.bodies.len(), 2);
    assert_eq!(super::entries(&h.app, component, "bodies").len(), 2);
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(super::model_placement(&mut h.app, &[occurrence]).unwrap().bodies.len(), 1);
    assert_eq!(super::entries(&h.app, component, "bodies").len(), 1);
    // A separate session can have the same revision and the same occurrence ids.
    h.app.session = solidcraft_engine::Session::default();
    assert!(super::model_placement(&mut h.app, &[occurrence]).is_none());
    assert!(super::model_children(&h.app, 0).is_empty());
    assert!(super::entries(&h.app, component, "bodies").is_empty());
}
