use crate::scenario::Harness;
use serde_json::{Value, json};
use solidcraft_engine::doc::{FaceAttachmentStatus, FeatureKind, PlaneRef};

fn picked_sketch() -> (Harness, u64) {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.session.auto_project = false;
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"BaseBox","name":"Original"})).unwrap();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"corner":[0,0,20],"body_name":"ControlBox","name":"Control"})).unwrap();
    h.app.run("browser.visibility", json!({"items":["ControlBox"],"visible":false})).unwrap();
    h.call("ui.view", json!({"view":"top"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.settle();
    h.step(&json!({"start":"sketch.create"})).unwrap();
    h.step(&json!({"click":{"world":[3,4,10]}})).unwrap();
    h.settle();
    let sketch = h.app.session.active_sketch.expect("actual picked face creates the sketch");
    let FeatureKind::Sketch { plane: PlaneRef::Face { anchor: Some(anchor), .. }, .. } = &h.app.session.doc.feature(sketch).unwrap().kind else {
        panic!("picked face must create a certified attachment");
    };
    assert_eq!(anchor.target.body, "BaseBox");
    assert!(anchor.path.is_empty());
    h.app.run("parameters.add", json!({"name":"badge_diameter","expression":"2 mm"})).unwrap();
    h.step(&json!({"start":"sketch.circle.center"})).unwrap();
    h.step(&json!({"click":{"world":[5,5,10]}})).unwrap();
    h.step(&json!({"move":{"world":[7,5,10]}})).unwrap();
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":"badge_diameter"})).unwrap();
    h.step(&json!({"key":"Enter"})).unwrap();
    h.app.finish_sketch();
    h.app.run("browser.visibility", json!({"items":["ControlBox"],"visible":true})).unwrap();
    assert!(!h.app.session.doc.sketch(sketch).unwrap().constraints.is_empty());
    (h, sketch)
}

fn raw_sketch(h: &Harness, id: u64) -> Value {
    serde_json::to_value(h.app.session.doc.sketch(id).unwrap()).unwrap()
}

#[test]
fn actual_picked_face_and_noop_edit_preserve_exact_anchor_and_dimensions() {
    let (mut h, sketch) = picked_sketch();
    let before = serde_json::to_value(&h.app.session.doc.feature(sketch).unwrap().kind).unwrap();
    h.app.edit_sketch(sketch);
    h.settle();
    h.app.finish_sketch();
    assert_eq!(serde_json::to_value(&h.app.session.doc.feature(sketch).unwrap().kind).unwrap(), before);
    let path = h.dir.join("picked-face.solidcraft");
    h.app.run("file.save", json!({"path":path})).unwrap();
    h.app.run("doc.open", json!({"path":path})).unwrap();
    assert_eq!(serde_json::to_value(&h.app.session.doc.feature(sketch).unwrap().kind).unwrap(), before);
    assert!(matches!(h.app.session.doc.sketch_face_attachment(sketch, &h.app.session.model), FaceAttachmentStatus::Healthy { .. }));
}

#[test]
fn lost_attachment_ghost_rebind_preview_cancel_apply_and_undo_preserve_sketch() {
    let (mut h, sketch) = picked_sketch();
    h.app.run("solid.extrude", json!({"sketch":sketch,"distance":2,"operation":"new_body","body_name":"Boss"})).unwrap();
    h.app.run("timeline.suppress", json!({"feature":"Original","suppressed":true})).unwrap();
    assert!(h.app.session.model.result(sketch).unwrap().error.is_some());
    assert!(h.app.session.world_state().body("Boss").is_none(), "lost attachment cannot produce a Boss on ControlBox");
    assert!(h.app.session.world_state().sketch(sketch).is_none(), "saved repair guides are not solved sketch geometry");
    let raw = raw_sketch(&h, sketch);
    let before = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    let redo = h.app.session.redo.len();
    h.app.edit_sketch(sketch);
    h.frames(4);
    assert_eq!(h.app.tree.redefine, Some(sketch));
    assert!(h.widget("Face attachment lost").is_some());
    let guides = crate::sketch_attachment_ui::guide_lines(&h.app);
    assert!(!guides.is_empty());
    assert!(guides.iter().flatten().all(|point| (point.z - 10.0).abs() < 1e-8));
    h.call("ui.view", json!({"view":"top"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.settle();
    h.app.run("browser.visibility", json!({"items":["ControlBox"],"visible":false})).unwrap();
    h.app.session.selection.clear();
    h.frames(4);
    h.step(&json!({"click":{"world":[5,5,10]}})).unwrap();
    assert!(h.app.session.selection.is_empty(), "saved lost-plane guides must not be selectable geometry");
    assert!(h.app.session.active_sketch.is_none());
    h.app.run("browser.visibility", json!({"items":["ControlBox"],"visible":true})).unwrap();
    h.frames(4);
    h.step(&json!({"click":{"world":[3,4,30]}})).unwrap();
    assert!(matches!(h.app.session.selection.first(), Some(solidcraft_engine::Sel::Face { body, .. }) if body=="ControlBox"));
    h.frames(2);
    h.step(&json!({"widget":"Reattach selected face"})).unwrap();
    let commands = crate::sketch_attachment_ui::commands(&h.app)
        .unwrap_or_else(|| panic!("repair draft {:?}, selection {:?}", h.app.tree.redefine_draft, h.app.session.selection));
    assert_eq!(commands[0].1["plane"]["body"], "ControlBox");
    assert!(commands[0].1["plane"]["index"].as_u64().is_some());
    let preview = h.app.session.preview_visible_world(&commands).unwrap();
    let boss = preview.after.body("Boss").unwrap().mesh().bounds();
    assert!((boss.min.z - 30.0).abs() < 1e-7 && (boss.max.z - 32.0).abs() < 1e-7);
    assert_eq!(h.app.session.doc.to_json(), before, "preview is scratch only");
    h.step(&json!({"widget":"Cancel"})).unwrap();
    assert_eq!(h.app.session.doc.to_json(), before);
    assert_eq!(h.app.session.undo.len(), undo);
    assert_eq!(h.app.session.redo.len(), redo);
    assert!(crate::sketch_attachment_ui::guide_lines(&h.app).is_empty());

    crate::browser::redefine_sketch(&mut h.app, sketch);
    h.frames(4);
    h.step(&json!({"widget":"Reattach selected face"})).unwrap();
    h.frames(8);
    h.step(&json!({"widget":"Apply"})).unwrap();
    assert!(h.app.tree.redefine.is_none(), "{:?}", h.app.tree.redefine_draft);
    assert_eq!(raw_sketch(&h, sketch), raw, "curves, dimensions and expressions remain exact");
    assert_eq!(h.app.session.undo.len(), undo + 1);
    assert!(h.app.session.model.result(sketch).unwrap().error.is_none());
    let boss = h.app.session.world_state().body("Boss").unwrap().mesh().bounds();
    assert!((boss.min.z - 30.0).abs() < 1e-7 && (boss.max.z - 32.0).abs() < 1e-7);
    let repaired = h.app.session.doc.to_json();
    let path = h.dir.join("rebound-face.solidcraft");
    h.app.run("file.save", json!({"path":path})).unwrap();
    // Saving the untitled design names it after its file; everything else is exact.
    let named = |doc: &str| {
        let mut doc = solidcraft_engine::doc::Document::from_json(doc).unwrap();
        doc.name = "rebound-face".into();
        doc.to_json()
    };
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.doc.to_json(), named(&before));
    h.app.run("doc.open", json!({"path":path})).unwrap();
    assert_eq!(h.app.session.doc.to_json(), named(&repaired));
    assert_eq!(raw_sketch(&h, sketch), raw);
    assert!(h.app.session.model.result(sketch).unwrap().error.is_none());
}

#[test]
fn attachment_drafts_leave_on_document_replacement_but_survive_failed_open() {
    let (mut h, sketch) = picked_sketch();
    crate::browser::redefine_sketch(&mut h.app, sketch);
    h.frames(2);
    h.step(&json!({"widget":"Use XZ plane"})).unwrap();
    assert!(crate::sketch_attachment_ui::commands(&h.app).is_some());
    assert!(h.app.run("doc.open", json!({"path":h.dir.join("missing.solidcraft")})).is_err());
    assert!(crate::sketch_attachment_ui::commands(&h.app).is_some());
    h.app.run("file.new", json!({})).unwrap();
    assert!(h.app.tree.redefine.is_none());
    assert!(h.app.tree.redefine_draft.is_none());
}

#[test]
fn unsupported_modified_face_offers_explicit_fixed_plane_without_live_links() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Modified"})).unwrap();
    // An intentional split gives the +z source two distinct planar pieces. A
    // strict attachment cannot claim the original whole-face certificate.
    h.app.run("solid.split_face", json!({"body":"Modified","faces":[[5,5,10]],"plane":{"origin":[5,0,0],"normal":[1,0,0]}})).unwrap();
    let before = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    h.call("ui.view", json!({"view":"top"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.settle();
    h.step(&json!({"start":"sketch.create"})).unwrap();
    h.step(&json!({"click":{"world":[2.5,5,10]}})).unwrap();
    assert!(h.app.session.active_sketch.is_none());
    assert!(h.app.dialog.as_ref().unwrap().error.is_some());
    assert_eq!(h.app.session.doc.to_json(), before);
    assert_eq!(h.app.session.undo.len(), undo);
    h.frames(4);
    assert!(h.widget("Use fixed plane").is_some());
    h.step(&json!({"widget":"Use fixed plane"})).unwrap();
    let sketch = h.app.session.active_sketch.expect("explicit fixed choice succeeds for the actual planar surface");
    let FeatureKind::Sketch { plane: PlaneRef::Custom { plane }, sketch: geometry } = &h.app.session.doc.feature(sketch).unwrap().kind else {
        panic!("fixed choice must never create a durable-looking Face reference");
    };
    assert!((plane.origin.z - 10.0).abs() < 1e-7);
    assert!(geometry.links.is_empty(), "a fixed snapshot cannot carry live source projections");
    assert_eq!(h.app.session.undo.len(), undo + 1);
}

fn occurrence_click(h: &mut Harness, path: &[u64], control: &str) {
    h.step(&json!({"click":{"occurrence":path,"control":control}})).unwrap();
}

#[test]
fn actual_nested_second_placement_face_pick_keeps_target_and_author_paths() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.session.auto_project = false;
    let parent = h.app.run("component.create", json!({"name":"Parent"})).unwrap();
    let parent_component = parent["component"].as_u64().unwrap();
    let first_parent = parent["occurrence"].as_u64().unwrap();
    let child = h.app.run("component.create", json!({"name":"Child","parent":parent_component,"parent_path":[first_parent]})).unwrap();
    let child_component = child["component"].as_u64().unwrap();
    let child_occurrence = child["occurrence"].as_u64().unwrap();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"NestedBase"})).unwrap();
    h.app.run("component.activate", json!({"component":0})).unwrap();
    let second_parent =
        h.app.run("occurrence.copy", json!({"occurrence":first_parent,"translate":[40,0,0]})).unwrap()["occurrence"].as_u64().unwrap();
    h.frames(4);
    let path = vec![second_parent, child_occurrence];
    if !crate::browser::drawn_occurrences().iter().any(|(drawn, _, _, _)| drawn == &path) {
        occurrence_click(&mut h, &[second_parent], "fold");
    }
    occurrence_click(&mut h, &path, "activate");
    assert_eq!(h.app.session.active_component, child_component);
    assert_eq!(h.app.session.active_occurrence_path.as_ref(), Some(&path));
    h.call("ui.view", json!({"view":"top"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.settle();
    h.step(&json!({"start":"sketch.create"})).unwrap();
    h.step(&json!({"click":{"world":[43,4,10]}})).unwrap();
    let sketch = h.app.session.active_sketch.expect("actual second nested face pick");
    let kind = h.app.session.doc.feature(sketch).unwrap().kind.clone();
    let FeatureKind::Sketch { plane: PlaneRef::Face { anchor: Some(anchor), .. }, .. } = &kind else {
        panic!("strict face attachment");
    };
    assert_eq!(anchor.target.body, "NestedBase");
    assert_eq!(anchor.path, path);
    assert_eq!(anchor.author_path, path);
    assert_eq!(anchor.component, child_component);
    let FaceAttachmentStatus::Healthy { plane, .. } = h.app.session.placed_document().sketch_face_attachment(sketch, &h.app.session.model) else {
        panic!("selected saved placement resolves");
    };
    assert!((plane.origin.x - 40.0).abs() < 1e-7 && (plane.origin.z - 10.0).abs() < 1e-7);
    h.app.run("sketch.rectangle.two_point", json!({"p0":[1,1],"p1":[3,3]})).unwrap();
    h.app.run("sketch.dimension", json!({"entities":["l1"],"value":2})).unwrap();
    h.app.finish_sketch();
    let kind = h.app.session.doc.feature(sketch).unwrap().kind.clone();
    h.app.run("component.activate", json!({"component":0})).unwrap();
    h.app.edit_sketch(sketch);
    assert_eq!(h.app.session.active_component, child_component);
    assert_eq!(h.app.session.active_occurrence_path.as_ref(), Some(&path));
    h.app.finish_sketch();
    assert_eq!(h.app.session.doc.feature(sketch).unwrap().kind, kind);
    let file = h.dir.join("nested-face.solidcraft");
    h.app.run("file.save", json!({"path":file})).unwrap();
    h.app.run("doc.open", json!({"path":file})).unwrap();
    assert_eq!(h.app.session.doc.feature(sketch).unwrap().kind, kind);
    h.app.run("selection.delete", json!({"occurrences":[second_parent]})).unwrap();
    assert!(h.app.session.model.result(sketch).unwrap().error.is_some());
    let FaceAttachmentStatus::Lost { repair_plane, .. } = h.app.session.placed_document().sketch_face_attachment(sketch, &h.app.session.model) else {
        panic!("deleted second placement must not substitute first");
    };
    assert!(repair_plane.is_none(), "a missing saved author placement cannot be drawn at its sibling");
    h.app.edit_sketch(sketch);
    h.frames(4);
    assert!(crate::sketch_attachment_ui::guide_lines(&h.app).is_empty());
    assert_eq!(h.app.tree.redefine, Some(sketch));
}

#[test]
fn nested_anchor_saved_fixed_conversion_preserves_rotated_second_author_plane() {
    use solidcraft_engine::doc::{EmbossFace, apply_point, apply_vector};
    use solidcraft_engine::geom::Plane;
    use solidcraft_engine::geom::Vec3;
    let mut h = Harness::new();
    h.app.session.auto_project = false;
    let created = h.app.run("component.create", json!({"name":"Author"})).unwrap();
    let component = created["component"].as_u64().unwrap();
    let first = created["occurrence"].as_u64().unwrap();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Base"})).unwrap();
    let second = h.app.run("occurrence.copy", json!({"occurrence":first,"translate":[40,0,0]})).unwrap()["occurrence"].as_u64().unwrap();
    h.app.run("occurrence.move", json!({"occurrence":second,"axis":[0,1,0],"angle":"90 deg","capture":true})).unwrap();
    h.app.run("component.activate", json!({"component":component,"path":[second],"occurrence":second})).unwrap();
    let doc = h.app.session.placed_document();
    let body_name = doc.body_frames_with_source(&h.app.session.model.state()).into_iter().find(|(_, _, _, path, _)| path == &[second]).unwrap().0;
    let world = h.app.session.world_state();
    let face = world
        .body(&body_name)
        .unwrap()
        .body
        .faces(0.01)
        .unwrap()
        .into_iter()
        .find(|face| world.body(&body_name).unwrap().body.face_plane(face.index).is_some_and(|plane| plane.normal().dot(Vec3::X) > 0.999))
        .unwrap();
    let id = h
        .app
        .run("sketch.create", json!({"plane":{"body":body_name,"index":face.index,"face":face.centroid,"path":[second]},"project_edges":false}))
        .unwrap()["sketch"]
        .as_u64()
        .unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[1,1],"p1":[3,3]})).unwrap();
    h.app.finish_sketch();
    // Exercise a durable anchor under both public plane wrappers, as imported
    // documents can contain these without an interactive wrapper editor.
    let mut document = (*h.app.session.doc).clone();
    let FeatureKind::Sketch { plane, .. } = &mut document.feature_mut(id).unwrap().kind else {
        panic!("sketch");
    };
    *plane = PlaneRef::AtAngle {
        base: Box::new(PlaneRef::Offset { base: Box::new(plane.clone()), distance: "2 mm".into() }),
        axis_origin: Vec3::ZERO,
        axis_dir: Vec3::X,
        angle: "15 deg".into(),
    };
    h.app.session = solidcraft_engine::Session::new(document);
    let raw = h.app.session.doc.sketch(id).unwrap().clone();
    let before = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    let FaceAttachmentStatus::Healthy { plane: expected, .. } = h.app.session.placed_document().sketch_face_attachment(id, &h.app.session.model)
    else {
        panic!("wrapped anchor must resolve");
    };
    crate::browser::redefine_sketch(&mut h.app, id);
    h.frames(4);
    assert_eq!(h.app.tree.redefine_draft.as_ref().unwrap().author_path, Some(vec![second]));
    h.step(&json!({"widget":"Use saved fixed plane"})).unwrap();
    let commands = crate::sketch_attachment_ui::commands(&h.app).unwrap();
    let preview = h.app.session.preview_visible_world(&commands).unwrap();
    // The shared sketch's primary world entry belongs to the untransformed
    // first placement. Verify that local plane at the specifically chosen
    // second placement, rather than treating the primary entry as that path.
    let frame = EmbossFace::component_frame(&h.app.session.placed_document(), component, &[second]).unwrap();
    let selected_plane =
        |local: Plane| Plane::new(apply_point(&frame, local.origin), apply_vector(&frame, local.x), apply_vector(&frame, local.y)).unwrap();
    let shown = selected_plane(preview.after.sketch(id).unwrap().plane);
    assert!(
        shown.origin.dist(expected.origin) < 1e-7,
        "expected {expected:?}, shown {shown:?}, primary {:?}, commands {commands:?}",
        preview.after.sketch(id).unwrap().plane
    );
    assert!(shown.x.dist(expected.x) < 1e-7 && shown.y.dist(expected.y) < 1e-7);
    assert_eq!(h.app.session.doc.to_json(), before);
    h.frames(6);
    h.step(&json!({"widget":"Apply"})).unwrap();
    assert!(h.app.tree.redefine.is_none());
    assert_eq!(h.app.session.doc.sketch(id).unwrap(), &raw);
    assert_eq!(h.app.session.undo.len(), undo + 1);
    let shown = selected_plane(h.app.session.model.state().sketch(id).unwrap().plane);
    assert!(shown.origin.dist(expected.origin) < 1e-7);
    assert!(shown.x.dist(expected.x) < 1e-7 && shown.y.dist(expected.y) < 1e-7);
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.doc.to_json(), before);
}

/// Issue #489: extrude, sketch a rectangle over the whole top face (its corners snap to the
/// face's projected boundary), extrude it up as a Join, then pick the new top face for a sketch.
#[test]
fn picked_top_face_of_a_stacked_join_extrude_takes_a_sketch() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("sketch.create", json!({"plane":"XY","name":"Base"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[40,40]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("solid.extrude", json!({"distance":10,"name":"First"})).unwrap();
    h.call("ui.view", json!({"view":"top"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.settle();
    let pick_face = |h: &mut Harness, z: f64| {
        h.step(&json!({"start":"sketch.create"})).unwrap();
        h.step(&json!({"click":{"world":[20,20,z]}})).unwrap();
        h.settle();
        let error = h.app.dialog.as_ref().and_then(|d| d.error.clone());
        let sketch = h.app.session.active_sketch.unwrap_or_else(|| panic!("top face at z={z} must take a sketch: {error:?}"));
        let FeatureKind::Sketch { plane: PlaneRef::Face { anchor: Some(_), .. }, .. } = &h.app.session.doc.feature(sketch).unwrap().kind else {
            panic!("the top face at z={z} must give a certified attachment");
        };
        sketch
    };
    pick_face(&mut h, 10.);
    h.step(&json!({"start":"sketch.rectangle.two_point"})).unwrap();
    h.step(&json!({"click":{"world":[0,0,10]}})).unwrap();
    h.step(&json!({"click":{"world":[40,40,10]}})).unwrap();
    h.step(&json!({"key":"Escape"})).unwrap();
    h.app.finish_sketch();
    h.app.run("solid.extrude", json!({"distance":10,"operation":"join","name":"Second"})).unwrap();
    assert_eq!(h.app.session.model.state().bodies.len(), 1);
    h.settle();
    let top = pick_face(&mut h, 20.);
    let FaceAttachmentStatus::Healthy { plane, .. } = h.app.session.placed_document().sketch_face_attachment(top, &h.app.session.model) else {
        panic!("attachment must be healthy");
    };
    assert!((plane.origin.z - 20.).abs() < 1e-9);
}
