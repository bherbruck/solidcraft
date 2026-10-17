#![allow(clippy::unwrap_used)]
use crate::scenario::Harness;
use crate::sketch_modify_ui::{Mod, candidate};
use crate::viewport::Hit;
use serde_json::{Value, json};
use solidcraft_engine::Sel;

fn step(h: &mut Harness, value: Value) {
    h.step(&value).unwrap();
}
fn fixture() -> Harness {
    let mut h = Harness::new();
    h.app.preferences.look_at_anim = false;
    for (command, params) in [
        ("sketch.create", json!({"plane":"XY"})),
        ("sketch.line", json!({"points":[[-30,10],[-15,10]]})),
        ("sketch.line", json!({"points":[[-15,10],[-15,25]]})),
        ("sketch.line", json!({"points":[[0,-10],[0,40]],"construction":true})),
    ] {
        step(&mut h, json!({"call":"engine.execute","params":{"command":command,"params":params}}));
    }
    step(&mut h, json!({"call":"ui.view","params":{"view":"top"}}));
    h
}
fn pick(h: &mut Harness, xy: [f64; 2], ctrl: bool) {
    step(h, json!({"click":{"sketch":xy},"ctrl":ctrl}));
}
fn preview(h: &mut Harness, curves: usize) {
    step(h, json!({"until":{"count":{"name":"sketch_preview_curves","value":curves}}}));
}

#[test]
fn sketch_dialog_mirror_toggles_clears_references_previews_and_cancels() {
    let mut h = fixture();
    // Keep the source curves and reference between the floating dialog and Sketch Palette.
    step(&mut h, json!({"call":"ui.camera","params":{"target":[-40,0,0],"distance":150}}));
    let original = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    let revision = h.app.session.revision;
    step(&mut h, json!({"start":"sketch.mirror"}));
    assert!(h.app.tool.is_none());
    pick(&mut h, [-22.5, 10.0], false);
    pick(&mut h, [-15.0, 17.5], false);
    pick(&mut h, [-22.5, 10.0], true);
    assert_eq!(h.app.dialog.as_ref().unwrap().inputs[0].items.len(), 1);
    pick(&mut h, [-22.5, 10.0], true);
    step(&mut h, json!({"call":"ui.dialogInput","params":{"index":1}}));
    pick(&mut h, [0.0, 20.0], false);
    preview(&mut h, 2);
    assert_eq!(h.app.session.doc.to_json(), original);
    assert_eq!(h.app.session.undo.len(), undo);
    assert_eq!(h.app.session.revision, revision);
    step(&mut h, json!({"click":{"handle":"dialog:clear:Mirror line"}}));
    assert!(h.app.dialog.as_ref().unwrap().inputs[1].items.is_empty());
    assert_eq!(h.app.dialog.as_ref().unwrap().inputs[0].items.len(), 2);
    pick(&mut h, [0.0, 20.0], false);
    preview(&mut h, 2);
    step(&mut h, json!({"key":"Escape"}));
    h.frames(3);
    assert!(h.app.dialog.is_none());
    assert!(!h.app.preview.active);
    assert_eq!(h.app.session.doc.to_json(), original);
    assert_eq!(h.app.session.undo.len(), undo);
}

#[test]
fn sketch_dialog_patterns_update_counts_and_place_an_unsnapped_centre() {
    let mut h = fixture();
    // Keep the picked geometry clear of the two-column rectangular-pattern form.
    step(&mut h, json!({"call":"ui.camera","params":{"target":[-60,0,0],"distance":120}}));
    step(&mut h, json!({"start":"sketch.pattern.rectangular"}));
    pick(&mut h, [-22.5, 10.0], false);
    if let crate::dialogs::Kind::SketchModify(Mod::PatternRect { count, count2, .. }) = &mut h.app.dialog.as_mut().unwrap().kind {
        *count = "4".into();
        *count2 = "2".into();
    }
    h.frames(3);
    preview(&mut h, 7);
    step(&mut h, json!({"click":{"handle":"dialog:OK"}}));
    assert_eq!(h.app.session.world_state().sketch(1).unwrap().sketch.curves.len(), 10);
    step(&mut h, json!({"start":"sketch.pattern.circular"}));
    pick(&mut h, [-22.5, 10.0], false);
    step(&mut h, json!({"call":"ui.dialogInput","params":{"index":1}}));
    pick(&mut h, [-40.0, -20.0], false);
    if let crate::dialogs::Kind::SketchModify(Mod::PatternCirc { count, angle, .. }) = &mut h.app.dialog.as_mut().unwrap().kind {
        *count = "4".into();
        *angle = "180 deg".into();
    }
    h.frames(3);
    preview(&mut h, 3);
    step(&mut h, json!({"click":{"handle":"dialog:OK"}}));
    assert_eq!(h.app.session.world_state().sketch(1).unwrap().sketch.curves.len(), 13);
}

#[test]
fn sketch_dialog_offset_precise_distance_flip_and_editable_dimension() {
    let mut h = fixture();
    // The floating browser shifts the dialog over the source curve at the default camera.
    step(&mut h, json!({"call":"ui.camera","params":{"target":[-40,0,0],"distance":150}}));
    step(&mut h, json!({"start":"sketch.offset"}));
    pick(&mut h, [-22.5, 10.0], false);
    if let crate::dialogs::Kind::SketchModify(Mod::Offset { distance, flip, chain, .. }) = &mut h.app.dialog.as_mut().unwrap().kind {
        *distance = "2 mm".into();
        *flip = true;
        *chain = false;
    }
    h.frames(3);
    preview(&mut h, 1);
    step(&mut h, json!({"click":{"handle":"dialog:OK"}}));
    let param = h.app.session.doc.params.iter().find(|p| p.name.starts_with('d')).unwrap().name.clone();
    step(&mut h, json!({"call":"engine.execute","params":{"command":"parameters.change","params":{"name":param,"expression":"3 mm"}}}));
    let state = h.app.session.world_state();
    let sk = &state.sketch(1).unwrap().sketch;
    assert_eq!(sk.offsets[0].distance, -3.0);
    assert!(sk.constraints.iter().any(|c| matches!(c.kind, solidcraft_engine::sketch::ConstraintKind::OffsetDistance { .. })));
}

#[test]
fn sketch_dialog_mirror_refuses_external_and_curved_references() {
    let mut h = fixture();
    h.app.start("sketch.mirror");
    let d = h.app.dialog.as_mut().unwrap();
    d.active = 1;
    assert!(candidate(d, &h.app.session, &Hit::SketchCurve { sketch: 2, id: "l1".into(), straight: true }).is_none());
    assert!(candidate(d, &h.app.session, &Hit::SketchCurve { sketch: 1, id: "c1".into(), straight: false }).is_none());
}

#[test]
fn sketch_dialog_box_selection_adds_replaces_and_preserves_reference() {
    let mut h = fixture();
    // Put every selection box between the left dialog and the right Sketch Palette.
    step(&mut h, json!({"call":"ui.camera","params":{"target":[-40,0,0],"distance":150}}));
    step(&mut h, json!({"call":"engine.execute","params":{"command":"sketch.line","params":{"points":[[-50,-20],[-40,-20]]}}}));
    step(&mut h, json!({"start":"sketch.mirror"}));
    step(&mut h, json!({"call":"ui.dialogInput","params":{"index":1}}));
    pick(&mut h, [0.0, 20.0], false);
    step(&mut h, json!({"call":"ui.dialogInput","params":{"index":0}}));
    step(&mut h, json!({"drag":[{"sketch":[-32,8]},{"sketch":[-13,27]}]}));
    let sources = |h: &Harness| {
        h.app.dialog.as_ref().unwrap().inputs[0]
            .items
            .iter()
            .filter_map(|s| if let Sel::SketchCurve { id } = s { Some(id.clone()) } else { None })
            .collect::<Vec<_>>()
    };
    assert_eq!(sources(&h), vec!["l1", "l2"]);
    step(&mut h, json!({"drag":[{"sketch":[-52,-22]},{"sketch":[-38,-18]}],"shift":true}));
    assert_eq!(sources(&h), vec!["l1", "l2", "l4"]);
    step(&mut h, json!({"drag":[{"sketch":[-32,8]},{"sketch":[-13,12]}]}));
    assert_eq!(sources(&h), vec!["l1"]);
    assert_eq!(h.app.dialog.as_ref().unwrap().inputs[1].items, vec![Sel::SketchCurve { id: "l3".into() }]);
    step(&mut h, json!({"key":"Escape"}));
    step(&mut h, json!({"start":"sketch.offset"}));
    step(&mut h, json!({"drag":[{"sketch":[-32,8]},{"sketch":[-13,27]}]}));
    assert_eq!(h.app.dialog.as_ref().unwrap().inputs[0].items.len(), 2, "Offset accepts curves, not their endpoints or dimensions");
}

#[test]
fn sketch_modify_inputs_refuse_other_sketches_without_changing_hole_points() {
    let mut h = fixture();
    for command in ["sketch.mirror", "sketch.pattern.rectangular", "sketch.pattern.circular", "sketch.offset", "sketch.move", "sketch.scale"] {
        h.app.start(command);
        let d = h.app.dialog.as_ref().unwrap();
        assert!(candidate(d, &h.app.session, &Hit::SketchCurve { sketch: 2, id: "l1".into(), straight: true }).is_none());
        assert!(candidate(d, &h.app.session, &Hit::SketchPoint { sketch: 2, id: "p1".into(), at: solidcraft_engine::geom::Vec2::ZERO }).is_none());
        if command != "sketch.offset" {
            assert!(d.inputs[0].accept & crate::selection::SKETCH_POINTS != 0);
            assert_eq!(d.inputs[0].accept & crate::selection::POINTS, 0);
        }
    }
    let input = crate::selection::SelInput::new("Hole points", crate::selection::POINTS, true);
    assert_eq!(
        input.accepts(&Hit::SketchPoint { sketch: 2, id: "p1".into(), at: solidcraft_engine::geom::Vec2::ZERO }, |_, _| false),
        Some(Sel::SketchPoint { id: "2:p1".into() })
    );
    h.app.start("sketch.pattern.circular");
    let d = h.app.dialog.as_mut().unwrap();
    d.inputs[0].items = vec![Sel::SketchCurve { id: "l1".into() }];
    d.inputs[1].items = vec![Sel::SketchPoint { id: "2:p1".into() }];
    assert!(crate::dialogs::apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).is_err());
}

#[test]
fn sketch_preselected_points_toggle_once_and_empty_shift_box_keeps_reference() {
    let mut h = fixture();
    let state = h.app.session.world_state();
    let point = state.sketch(1).unwrap().sketch.points[1].id.clone();
    h.app.session.selection = vec![Sel::SketchPoint { id: point.clone() }];
    h.app.start("sketch.mirror");
    let d = h.app.dialog.as_mut().unwrap();
    assert_eq!(d.inputs[0].items, vec![Sel::SketchPoint { id: point.clone() }]);
    d.active = 0;
    let selected = candidate(d, &h.app.session, &Hit::SketchPoint { sketch: 1, id: point, at: solidcraft_engine::geom::Vec2::ZERO }).unwrap();
    d.pick(&h.app.session, selected);
    assert!(d.inputs[0].items.is_empty(), "clicking a preselection removes it instead of duplicating it");
    d.active = 1;
    let reference = Sel::SketchCurve { id: "l3".into() };
    d.pick(&h.app.session, reference.clone());
    d.take_box(&h.app.session, Vec::new(), true);
    assert_eq!(d.inputs[1].items, vec![reference]);
    d.take_box(&h.app.session, vec![Sel::SketchCurve { id: "l1".into() }, Sel::SketchCurve { id: "l2".into() }], true);
    assert_eq!(d.inputs[1].items, vec![Sel::SketchCurve { id: "l1".into() }], "a box replaces a single reference with one item");
}

#[test]
fn sketch_modify_previews_use_the_active_occurrences_world_placement() {
    let mut h = Harness::new();
    let part = h.app.run("component.create", json!({"name":"Part"})).unwrap();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    let sid = h.app.session.active_sketch.unwrap();
    h.app.run("sketch.line", json!({"points":[[-30,0],[-20,0]]})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,-10],[0,10]],"construction":true})).unwrap();
    h.app
        .run("occurrence.move", json!({"occurrence":part["occurrence"],"translate":[100,30,20],"axis":[0,0,1],"angle":"35 deg","capture":false}))
        .unwrap();
    let original = h.app.session.doc.to_json();
    let colors = crate::preview::Colors::from_tokens(&crate::theme::Tokens::get());
    for (command, params) in [
        ("sketch.mirror", json!({"entities":["l1"],"line":"l2"})),
        ("sketch.pattern.rectangular", json!({"entities":["l1"],"count":3,"spacing":10})),
        ("sketch.pattern.circular", json!({"entities":["l1"],"center":[0,0],"count":4})),
        ("sketch.offset", json!({"curves":["l1"],"distance":2,"chain":false})),
    ] {
        let built = crate::preview::compute(&h.app.session, &[(command.into(), params.clone())], colors).unwrap();
        let mut scratch = h.app.session.scratch();
        scratch.execute(command, &params).unwrap();
        let state = scratch.world_state();
        let expected = state.sketch(sid).unwrap().plane;
        assert!(expected.origin.len() > 50.0, "the fixture must exercise a moved occurrence");
        assert_eq!(built.sketches.len(), 1);
        assert!(!built.sketches[0].curves.is_empty());
        assert_eq!(built.sketches[0].sketch.plane, expected, "{command}");
        assert_eq!(h.app.session.doc.to_json(), original);
    }
}

#[test]
fn sketch_corner_preselected_point_uses_the_same_identity_as_corner_picking() {
    let mut h = fixture();
    let state = h.app.session.world_state();
    let sk = &state.sketch(1).unwrap().sketch;
    let index = sk.resolve_point("l1.end").unwrap();
    let point = sk.points[index].id.clone();
    h.app.session.selection = vec![Sel::SketchPoint { id: point }];
    h.app.start("sketch.fillet");
    assert_eq!(h.app.dialog.as_ref().unwrap().inputs[0].items.len(), 1);
    pick(&mut h, [-15.0, 10.0], true);
    assert!(h.app.dialog.as_ref().unwrap().inputs[0].items.is_empty());
}

#[test]
fn sketch_modify_keeps_custom_point_ids_that_contain_colons() {
    let mut h = fixture();
    h.app.run("sketch.point", json!({"point":[-40,0],"id":"2:centre"})).unwrap();
    h.app.start("sketch.pattern.circular");
    let d = h.app.dialog.as_mut().unwrap();
    d.inputs[0].items = vec![Sel::SketchCurve { id: "l1".into() }];
    d.inputs[1].items = vec![Sel::SketchPoint { id: "2:centre".into() }];
    let commands = crate::dialogs::apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    assert_eq!(commands[0].1["center"], "2:centre");
    for (command, params) in commands {
        h.app.run(&command, params).unwrap();
    }
    assert_eq!(h.app.session.world_state().sketch(1).unwrap().sketch.curves.len(), 8);
}
