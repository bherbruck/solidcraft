use serde_json::json;
use solidcraft_engine::Sel;
use solidcraft_engine::geom::Vec3;

use crate::dialogs::{Kind, apply_commands};
use crate::scenario::Harness;

#[test]
fn sketch_move_mode_retains_entities_and_waits_for_two_points() {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane": "YZ"})).unwrap();
    h.app.run("sketch.line", json!({"points": [[5,7],[15,7]], "ids": ["l1"]})).unwrap();
    h.app.session.selection = vec![Sel::SketchCurve { id: "l1".into() }];
    let before = h.app.session.doc.to_json();
    h.app.start("sketch.move");
    let mut d = h.app.dialog.take().unwrap();
    crate::sketch_modify_ui::set_point_to_point(&mut d, true);
    assert_eq!(d.inputs[0].items.len(), 1);
    assert_eq!(d.active, 1);
    assert!(d.primary().is_none());
    assert!(apply_commands(&h.app, &d).is_err());
    assert!(crate::sketch_modify_ui::pick_position(&mut d, &h.app.session, Vec3::new(0., 5., 7.)));
    assert_eq!(d.active, 2);
    assert!(apply_commands(&h.app, &d).is_err());
    assert!(crate::sketch_modify_ui::pick_position(&mut d, &h.app.session, Vec3::new(0., 25., 37.)));
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(cmds[0].1["from"], json!([5., 7.]));
    assert_eq!(cmds[0].1["to"], json!([25., 37.]));
    assert_eq!(h.app.session.doc.to_json(), before);
}

#[test]
fn paste_dialog_ignores_preselection_and_cancel_changes_nothing() {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.run("sketch.line", json!({"points": [[5,7],[15,7]], "ids": ["l1"]})).unwrap();
    h.app.run("sketch.copy", json!({"entities": ["l1"]})).unwrap();
    h.app.session.selection = vec![Sel::SketchPoint { id: "l1.start".into() }];
    let before = h.app.session.doc.to_json();
    h.app.start("edit.paste.point_to_point");
    let mut d = h.app.dialog.take().unwrap();
    assert!(matches!(d.kind, Kind::PointTransfer(_)));
    assert!(d.inputs.iter().all(|input| input.items.is_empty()));
    crate::dialogs_point_transfer::pick_position(&mut d, &h.app.session, Vec3::new(5., 7., 0.));
    crate::dialogs_point_transfer::pick_position(&mut d, &h.app.session, Vec3::new(45., 27., 0.));
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(cmds[0].0, "edit.paste.point_to_point");
    assert_eq!(cmds[0].1, json!({"from": [5.,7.], "to": [45.,27.]}));
    h.app.dialog = Some(d);
    crate::dialogs::cancel(&mut h.app);
    assert_eq!(h.app.session.doc.to_json(), before);
}

#[test]
fn component_move_dialog_keeps_exact_selected_occurrence_path() {
    let mut h = Harness::new();
    let parent = h.app.run("component.create", json!({"name": "Parent"})).unwrap()["occurrence"].as_u64().unwrap();
    let child = h.app.run("component.create", json!({"name": "Child"})).unwrap()["occurrence"].as_u64().unwrap();
    h.app.session.selection = vec![Sel::Occurrence { path: vec![parent, child] }];
    h.app.start("objects.move.point_to_point");
    let mut d = h.app.dialog.take().unwrap();
    assert_eq!(d.inputs[0].items, vec![Sel::Occurrence { path: vec![parent, child] }]);
    assert_eq!(d.active, 1);
    d.pick(&h.app.session, Sel::Vertex { body: String::new(), point: Vec3::ZERO });
    d.pick(&h.app.session, Sel::Vertex { body: String::new(), point: Vec3::new(10., 20., 30.) });
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(cmds[0].1["objects"], json!([{"type": "occurrence", "path": [parent,child]}]));
    assert_eq!(cmds[0].1["to"], json!([10., 20., 30.]));
}

#[test]
fn canvas_picks_preview_then_apply_sketch_move_and_paste_once() {
    for paste in [false, true] {
        let mut h = Harness::new();
        h.app.preferences.look_at_anim = false;
        h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
        h.app.run("sketch.line", json!({"points": [[-20,10],[-10,10]], "ids": ["l1"]})).unwrap();
        h.app.run("sketch.copy", json!({"entities": ["l1"]})).unwrap();
        h.call("ui.view", json!({"view": "top"}));
        h.call("ui.camera", json!({"target": [0,0,0], "distance": 200}));
        h.app.session.selection = vec![Sel::SketchCurve { id: "l1".into() }];
        let before = h.app.session.doc.to_json();
        let undo = h.app.session.undo.len();
        if paste {
            h.step(&json!({"key": "V", "cmd": true, "shift": true})).unwrap();
        } else {
            h.app.start("sketch.move");
            crate::sketch_modify_ui::set_point_to_point(h.app.dialog.as_mut().unwrap(), true);
        }
        h.settle();
        h.step(&json!({"click": {"sketch": [-20,10]}})).unwrap();
        h.step(&json!({"click": {"sketch": [20,-10]}})).unwrap();
        h.step(&json!({"until": {"count": {"name": "sketch_preview_curves", "value": 1}}})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), before);
        assert_eq!(h.app.session.undo.len(), undo);
        h.step(&json!({"widget": "OK"})).unwrap();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
        let sk = h.app.session.doc.sketch(h.app.session.active_sketch.unwrap()).unwrap();
        assert_eq!(sk.curves.len(), if paste { 2 } else { 1 });
        assert!(sk.points.iter().any(|p| p.pos.dist(solidcraft_engine::geom::Vec2::new(20., -10.)) < 0.5));
        assert_eq!(h.app.session.undo.len(), undo + 1);
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), before);
    }
}

#[test]
fn source_anchor_from_another_plane_uses_the_clipboard_sketch_coordinates() {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane": "YZ"})).unwrap();
    let source = h.app.session.active_sketch.unwrap();
    h.app.run("sketch.line", json!({"points": [[5,7],[15,7]], "ids": ["l1"]})).unwrap();
    h.app.run("sketch.copy", json!({"entities": ["l1"]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.start("edit.paste.point_to_point");
    let mut d = h.app.dialog.take().unwrap();
    d.pick(&h.app.session, Sel::SketchPoint { id: format!("{source}:l1.start") });
    d.pick(&h.app.session, Sel::Vertex { body: String::new(), point: Vec3::new(45., 27., 0.) });
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(cmds[0].1["from"], json!([5., 7.]));
    assert_eq!(cmds[0].1["to"], json!([45., 27.]));
}

#[test]
fn copying_a_body_then_pasting_opens_anchor_picks_without_early_changes() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
    let name = h.app.session.world_state().bodies[0].name.clone();
    h.app.session.selection = vec![Sel::Face { body: name, index: 0, point: Vec3::ZERO }];
    assert!(crate::edit_clipboard::can_copy(&h.app));
    assert!(crate::edit_clipboard::copy(&mut h.app));
    assert!(crate::edit_clipboard::can_paste(&h.app));
    let before = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    assert!(crate::edit_clipboard::paste(&mut h.app, None));
    let mut d = h.app.dialog.take().unwrap();
    assert_eq!(d.inputs.iter().map(|i| i.label).collect::<Vec<_>>(), vec!["From", "To"]);
    d.pick(&h.app.session, Sel::Vertex { body: String::new(), point: Vec3::ZERO });
    d.pick(&h.app.session, Sel::Vertex { body: String::new(), point: Vec3::new(20., 30., 40.) });
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(h.app.session.doc.to_json(), before);
    for (command, params) in cmds {
        h.app.run(&command, params).unwrap();
    }
    assert_eq!(h.app.session.world_state().bodies.len(), 2);
    assert_eq!(h.app.session.undo.len(), undo + 1);
}
