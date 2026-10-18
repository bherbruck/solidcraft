use crate::scenario::Harness;
use serde_json::json;
use solidcraft_engine::{
    Sel,
    doc::{FaceAttachmentStatus, FeatureKind, PlaneRef},
};

fn shell() -> Harness {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length":40,"width":30,"height":20,"body_name":"Case"})).unwrap();
    h.app.run("solid.shell", json!({"faces":[[20,15,20]],"thickness":2})).unwrap();
    h.call("ui.view", json!({"view":"front"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.settle();
    h
}

fn assert_attached(h: &Harness) {
    let id = h.app.session.active_sketch.expect("a combined certified wall remains a sketch target");
    let FeatureKind::Sketch { plane: PlaneRef::Face { anchor: Some(anchor), .. }, .. } = &h.app.session.doc.feature(id).unwrap().kind else {
        panic!("wall sketch must have a durable attachment");
    };
    assert_eq!(anchor.target.body, "Case");
    assert_eq!(serde_json::to_value(&anchor.target.certificate).unwrap()["kind"], "shell_planar");
    assert!(matches!(h.app.session.doc.sketch_face_attachment(id, &h.app.session.model), FaceAttachmentStatus::Healthy { .. }));
    let world = h.app.session.world_state();
    let solved = world.sketch(id).unwrap();
    assert!(solved.plane.origin.y.abs() < 1e-8);
    assert!(solved.plane.normal().y.abs() > 0.999);
    assert!(h.app.session.model.result(id).unwrap().error.is_none());
}

fn selected_wall(h: &Harness) -> usize {
    assert_eq!(h.app.session.selection.len(), 1, "the exterior wall is one physical face, not a grouped lower strip");
    let Sel::Face { body, index, .. } = &h.app.session.selection[0] else {
        panic!("actual wall click must select a face");
    };
    assert_eq!(body, "Case");
    *index
}

fn assert_one_physical_wall(h: &mut Harness) -> usize {
    h.step(&json!({"click":{"world":[20,0,1]}})).unwrap();
    let lower = selected_wall(h);
    h.call("ui.click", json!({"x":300,"y":200}));
    assert!(h.app.session.selection.is_empty());
    h.step(&json!({"click":{"world":[20,0,10]}})).unwrap();
    assert_eq!(selected_wall(h), lower, "below and above the floor height must hit the same BRep face index");
    lower
}

#[test]
fn selected_shell_wall_patch_still_creates_a_certified_sketch() {
    let mut h = shell();
    assert_one_physical_wall(&mut h);
    let undo = h.app.session.undo.len();
    h.step(&json!({"start":"sketch.create"})).unwrap();
    h.settle();
    assert_attached(&h);
    assert_eq!(h.app.session.undo.len(), undo + 1);
}

#[test]
fn pending_create_sketch_accepts_an_actual_shell_wall_patch_click() {
    let mut h = shell();
    assert_one_physical_wall(&mut h);
    h.call("ui.click", json!({"x":300,"y":200}));
    assert!(h.app.session.selection.is_empty());
    let undo = h.app.session.undo.len();
    h.step(&json!({"start":"sketch.create"})).unwrap();
    assert!(h.app.session.active_sketch.is_none());
    h.step(&json!({"click":{"world":[20,0,1]}})).unwrap();
    h.settle();
    assert_attached(&h);
    assert_eq!(h.app.session.undo.len(), undo + 1);
}

#[test]
fn rounded_shell_corner_is_one_physical_face_above_and_below_the_floor() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length":80,"width":60,"height":30,"corner":[-40,-30,0],"body_name":"Case"})).unwrap();
    h.app.run("solid.fillet", json!({"edges":[[40,30,15],[-40,30,15],[-40,-30,15],[40,-30,15]],"radius":6})).unwrap();
    h.app.run("solid.shell", json!({"faces":[[0,0,30]],"thickness":2})).unwrap();
    h.call("ui.view", json!({"view":"iso"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.settle();
    let state = h.app.session.world_state();
    assert_eq!(state.body("Case").unwrap().body.face_count(), 19, "no artificial lower corner faces remain in the BRep");
    let document = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    let redo = h.app.session.redo.len();
    h.step(&json!({"click":{"world":[38.242640687,-28.242640687,1]}})).unwrap();
    let lower = selected_wall(&h);
    h.call("ui.click", json!({"x":300,"y":200}));
    assert!(h.app.session.selection.is_empty());
    h.step(&json!({"click":{"world":[38.242640687,-28.242640687,15]}})).unwrap();
    assert_eq!(selected_wall(&h), lower, "lower and upper corner clicks hit the same actual face, not two grouped pieces");
    assert_eq!(h.app.session.doc.to_json(), document);
    assert_eq!(h.app.session.undo.len(), undo);
    assert_eq!(h.app.session.redo.len(), redo);
}
