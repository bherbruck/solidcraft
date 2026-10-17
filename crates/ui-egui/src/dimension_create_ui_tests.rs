//! Dimension creation is an editable draft, accepted by the real focused value field.
use crate::scenario::Harness;
use serde_json::json;

fn line(end: [f64; 2]) -> Harness {
    crate::dim_view::cancel();
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,0],end]})).unwrap();
    h.call("ui.view", json!({"view":"top"}));
    // Leave room around the measured shape while testing the draft itself. Separate Fit
    // tests exercise measured dock occlusion without changing the picking projection.
    h.app.cam.distance = 80.0;
    h.app.cam.target = solidcraft_engine::geom::Vec3::new(end[0] * 0.5, end[1] * 0.5, 0.0);
    h.settle();
    h
}
fn place(h: &mut Harness, picks: &[[f64; 2]], at: [f64; 2]) {
    h.step(&json!({"start":"sketch.dimension"})).unwrap();
    for p in picks {
        h.step(&json!({"click":{"world":[p[0],p[1],0]}})).unwrap();
    }
    h.step(&json!({"click":{"world":[at[0],at[1],0]}})).unwrap();
    assert!(crate::dim_view::editing(), "placement opens an editor");
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("sc_dim_edit")));
}
fn type_value(h: &mut Harness, text: &str) {
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    if text.is_empty() {
        h.step(&json!({"key":"Backspace"})).unwrap();
    } else {
        h.step(&json!({"text":text})).unwrap();
    }
}
#[test]
fn dimension_parameters_use_typing_and_ctrl_space_completion() {
    for (prefix, expected) in [("wid", "width"), ("", "width"), ("d", "d1")] {
        let mut h = line([20., 0.]);
        h.app.run("parameters.add", json!({"name":"width","expression":"25 mm"})).unwrap();
        if prefix == "d" {
            h.app.run("sketch.line", json!({"points":[[0,-10],[12,-10]],"infer":false})).unwrap();
            h.app.run("sketch.dimension", json!({"entities":["l2"],"value":12,"text_at":[6,-16]})).unwrap();
            h.settle();
        }
        place(&mut h, &[[10., 0.]], [10., 6.]);
        let baseline = h.app.session.doc.to_json();
        type_value(&mut h, prefix);
        if prefix.is_empty() {
            h.step(&json!({"key":"Space","cmd":true})).unwrap();
        }
        assert!(h.widget(expected).is_some(), "parameter completion is available for {prefix:?}: {}", h.call("ui.inspect", json!({})));
        h.step(&json!({"key":"Tab"})).unwrap();
        assert!(crate::dim_view::editing(), "accepting completion does not apply the dimension");
        assert_eq!(h.app.session.doc.to_json(), baseline);
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(!crate::dim_view::editing());
        assert_eq!(h.app.session.doc.params.iter().rev().find(|p| p.model).unwrap().expr, expected);
        crate::dim_view::cancel();
    }
}

#[test]
fn edge_and_endpoint_creation_commit_once_and_undo_restores_exact_design() {
    for (end, picks, at) in [
        ([20., 0.], vec![[10., 0.]], [10., 5.]),
        ([20., 0.], vec![[0., 0.], [20., 0.]], [10., 5.]),
        ([0., 20.], vec![[0., 0.], [0., 20.]], [-5., 10.]),
        ([12., 16.], vec![[0., 0.], [12., 16.]], [-5., 12.]),
    ] {
        let mut h = line(end);
        let original = h.app.session.doc.to_json();
        let undo = h.app.session.undo.len();
        place(&mut h, &picks, at);
        assert_eq!(h.app.session.doc.to_json(), original);
        assert_eq!(h.app.session.undo.len(), undo);
        type_value(&mut h, "30 mm");
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(!crate::dim_view::editing());
        assert_eq!(h.app.session.undo.len(), undo + 1);
        let state = h.app.session.world_state();
        let sk = &state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch;
        assert!(
            (sk.point(sk.resolve_point("l1.start").unwrap()).unwrap().dist(sk.point(sk.resolve_point("l1.end").unwrap()).unwrap()) - 30.).abs()
                < 1e-6
        );
        assert!(sk.constraints.iter().any(|c| c.param.is_some()));
        let accepted = h.app.session.doc.to_json();
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), original);
        h.app.run("edit.redo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), accepted);
    }
}
#[test]
fn escape_invalid_correction_and_seed_enter_do_not_create_partial_dimensions() {
    let mut h = line([20., 0.]);
    let original = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    place(&mut h, &[[10., 0.]], [10., 5.]);
    h.step(&json!({"key":"Escape"})).unwrap();
    assert!(!crate::dim_view::editing());
    assert_eq!(h.app.session.doc.to_json(), original);
    assert_eq!(h.app.session.undo.len(), undo);
    place(&mut h, &[[0., 0.], [20., 0.]], [10., 5.]);
    type_value(&mut h, "unknown_length");
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(crate::dim_view::editing());
    assert_eq!(h.app.session.doc.to_json(), original);
    assert_eq!(h.app.session.undo.len(), undo);
    type_value(&mut h, "25 mm");
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(!crate::dim_view::editing());
    assert_eq!(h.app.session.undo.len(), undo + 1);
    h.app.run("edit.undo", json!({})).unwrap();
    h.settle();
    place(&mut h, &[[10., 0.]], [10., 5.]);
    h.step(&json!({"key":"Enter"})).unwrap();
    assert_eq!(h.app.session.undo.len(), undo + 1);
    assert!(!crate::dim_view::editing());
}
#[test]
fn switching_documents_cancels_creation_and_existing_parameter_edit_is_separate() {
    let mut h = line([20., 0.]);
    let original = h.app.session.doc.to_json();
    place(&mut h, &[[10., 0.]], [10., 5.]);
    h.call("ui.documents", json!({"action":"new"}));
    assert!(!crate::dim_view::editing());
    h.call("ui.documents", json!({"action":"switch","index":0}));
    assert_eq!(h.app.session.doc.to_json(), original);
    place(&mut h, &[[10., 0.]], [10., 5.]);
    h.step(&json!({"key":"Enter"})).unwrap();
    let undo = h.app.session.undo.len();
    crate::dim_view::edit_param(&h.app, "d1");
    h.settle();
    type_value(&mut h, "24 mm");
    h.step(&json!({"key":"Enter"})).unwrap();
    assert_eq!(h.app.session.undo.len(), undo + 1);
    h.app.run("edit.undo", json!({})).unwrap();
    assert!(h.app.session.doc.param("d1").unwrap().expr.starts_with("20"));
}

#[test]
fn rectangle_side_and_fixed_origin_line_dimensions_preserve_other_constraints() {
    for edge in [false, true] {
        let mut h = line([20., 0.]);
        h.app.run("sketch.constraint.fix", json!({"entity":"l1.start"})).unwrap();
        h.settle();
        let original = h.app.session.doc.to_json();
        let undo = h.app.session.undo.len();
        let picks = if edge { vec![[10., 0.]] } else { vec![[0., 0.], [20., 0.]] };
        place(&mut h, &picks, [10., 5.]);
        type_value(&mut h, "30 mm");
        h.step(&json!({"key":"Enter"})).unwrap();
        let state = h.app.session.world_state();
        let sk = &state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch;
        assert!(sk.point(sk.resolve_point("l1.start").unwrap()).unwrap().len() < 1e-8, "fixed origin remains fixed");
        assert!((sk.point(sk.resolve_point("l1.end").unwrap()).unwrap().x - 30.).abs() < 1e-6);
        assert_eq!(h.app.session.undo.len(), undo + 1);
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), original);
    }
    for (picks, at) in
        [(vec![[0., 0.], [0., 10.]], [-5., 5.]), (vec![[0., 5.]], [-5., 5.]), (vec![[20., 0.], [20., 10.]], [25., 5.]), (vec![[20., 5.]], [25., 5.])]
    {
        let mut h = Harness::new();
        crate::dim_view::cancel();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[20,10]})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 80.;
        h.app.cam.target = solidcraft_engine::geom::Vec3::new(10., 5., 0.);
        h.settle();
        let baseline = h.app.session.doc.to_json();
        let undo = h.app.session.undo.len();
        place(&mut h, &picks, at);
        type_value(&mut h, "12 mm");
        h.step(&json!({"key":"Enter"})).unwrap();
        let state = h.app.session.world_state();
        let solved = state.sketch(h.app.session.active_sketch.unwrap()).unwrap();
        // The sketch's first dimension scales it (#1365): 20 × 10 becomes 24 × 12.
        assert!((solved.profiles[0].area - 288.).abs() < 1e-5, "the first dimension scales the rectangle: {}", solved.profiles[0].area);
        assert_eq!(h.app.session.undo.len(), undo + 1);
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), baseline);
    }
}

#[test]
fn pending_second_nested_plane_uses_world_picks_but_one_local_dimension() {
    use solidcraft_engine::geom::Vec2;
    crate::dim_view::cancel();
    let mut h = Harness::new();
    let parent = h.app.run("component.create", json!({"name":"Assembly"})).unwrap();
    let parent_component = parent["component"].as_u64().unwrap();
    let first = parent["occurrence"].as_u64().unwrap();
    let child = h.app.run("component.create", json!({"name":"Part"})).unwrap();
    let component = child["component"].as_u64().unwrap();
    let leaf = child["occurrence"].as_u64().unwrap();
    h.app.run("component.activate", json!({"component":0})).unwrap();
    let second = h.app.run("occurrence.copy", json!({"component":parent_component})).unwrap()["occurrence"].as_u64().unwrap();
    h.app.run("occurrence.move", json!({"occurrence":second,"translate":[20,30,40],"axis":[1,0,0],"angle":"35 deg","capture":true})).unwrap();
    h.app.run("component.activate", json!({"component":component,"occurrence":leaf,"path":[second,leaf]})).unwrap();
    h.app.run("browser.visibility", json!({"paths":[[first]],"visible":false})).unwrap();
    let sid = h.app.run("sketch.create", json!({"plane":"XY"})).unwrap()["sketch"].as_u64().unwrap();
    // The 2D producer uses sketch-local coordinates; pointer picks below use placed world coordinates.
    h.app.run("sketch.line", json!({"points":[[0,0],[20,0]]})).unwrap();
    h.app.run("occurrence.move", json!({"occurrence":second,"translate":[25,32,40],"axis":[1,0,0],"angle":"35 deg","capture":false})).unwrap();
    h.call("ui.view", json!({"view":"top"}));
    h.app.cam.distance = 90.;
    let plane = h.app.session.world_state().sketch(sid).unwrap().plane;
    h.app.cam.target = plane.to_world(Vec2::new(10., 0.));
    h.settle();
    let baseline = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    let pending = h.app.session.pending_moves.clone();
    h.step(&json!({"start":"sketch.dimension"})).unwrap();
    for p in [Vec2::ZERO, Vec2::new(20., 0.), Vec2::new(10., 5.)] {
        let w = plane.to_world(p);
        h.step(&json!({"click":{"world":[w.x,w.y,w.z]}})).unwrap();
    }
    assert!(crate::dim_view::editing());
    assert_eq!(h.app.session.doc.to_json(), baseline);
    type_value(&mut h, "30 mm");
    h.step(&json!({"key":"Enter"})).unwrap();
    assert_eq!(h.app.session.undo.len(), undo + 1);
    assert_eq!(h.app.session.active_occurrence_path, Some(vec![second, leaf]));
    assert_eq!(h.app.session.pending_moves, pending);
    let state = h.app.session.model.state();
    let sk = &state.sketch(sid).unwrap().sketch;
    assert!(
        (sk.point(sk.resolve_point("l1.start").unwrap()).unwrap().dist(sk.point(sk.resolve_point("l1.end").unwrap()).unwrap()) - 30.).abs() < 1e-6
    );
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.doc.to_json(), baseline);
    assert_eq!(h.app.session.pending_moves, pending);
    h.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(h.app.session.pending_moves, pending);
    assert_eq!(h.app.session.active_occurrence_path, Some(vec![second, leaf]));
}

#[test]
fn invalid_zero_negative_failed_open_and_click_away_leave_no_dimension() {
    let mut h = line([20., 0.]);
    let baseline = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    for invalid in ["0 mm", "-2 mm"] {
        place(&mut h, &[[10., 0.]], [10., 5.]);
        type_value(&mut h, invalid);
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(crate::dim_view::editing());
        assert_eq!(h.app.session.doc.to_json(), baseline);
        assert_eq!(h.app.session.undo.len(), undo);
        h.step(&json!({"key":"Escape"})).unwrap();
    }
    place(&mut h, &[[10., 0.]], [10., 5.]);
    assert!(h.app.run("doc.open", json!({"path":"missing-dimension-draft-design.solidcraft"})).is_err());
    assert!(crate::dim_view::editing(), "failed replacement must preserve the draft");
    assert_eq!(h.app.session.doc.to_json(), baseline);
    h.frames(8);
    h.step(&json!({"click":{"world":[12,-10,0]}})).unwrap();
    assert!(!crate::dim_view::editing());
    assert_eq!(h.app.session.doc.to_json(), baseline);
    assert_eq!(h.app.session.undo.len(), undo);
}

#[test]
fn dimension_to_fillet_virtual_sharp_keeps_the_arc_and_fixed_origin() {
    crate::dim_view::cancel();
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,0],[0,20],[20,20]],"ids":["riser","top"]})).unwrap();
    h.app.run("sketch.constraint.fix", json!({"entity":"riser.start"})).unwrap();
    h.app.run("sketch.constraint.horizontal_vertical", json!({"line":"riser","mode":"vertical"})).unwrap();
    h.app.run("sketch.constraint.horizontal_vertical", json!({"line":"top","mode":"horizontal"})).unwrap();
    // An existing width requires the fillet's retained virtual sharp. Add an independent
    // height through the visible corner while preserving that width and the fillet arc.
    h.app.run("sketch.dimension", json!({"entities":["top"],"value":20})).unwrap();
    let fillet = h.app.run("sketch.fillet", json!({"point":"riser.end","radius":5})).unwrap();
    let corner = fillet["virtual_corners"][0].as_str().unwrap().to_string();
    h.call("ui.view", json!({"view":"top"}));
    h.app.cam.distance = 90.;
    h.app.cam.target = solidcraft_engine::geom::Vec3::new(5., 10., 0.);
    h.settle();
    let baseline = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    place(&mut h, &[[0., 0.], [0., 20.]], [-5., 10.]);
    type_value(&mut h, "30 mm");
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(!crate::dim_view::editing());
    assert_eq!(h.app.session.undo.len(), undo + 1);
    let state = h.app.session.world_state();
    let solved = state.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    assert!(solved.report.ok());
    let sk = &solved.sketch;
    assert!((sk.point(sk.point_index(&corner).unwrap()).unwrap().y - 30.).abs() < 1e-6);
    assert!(sk.point(sk.resolve_point("riser.start").unwrap()).unwrap().len() < 1e-8);
    assert!(sk.curves.iter().any(|c| matches!(c.kind, solidcraft_engine::sketch::CurveKind::Arc { .. })), "the dimension retains the fillet arc");
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.doc.to_json(), baseline);
}

#[test]
fn draft_annotation_does_not_move_committed_dimension_hitboxes() {
    crate::dim_view::cancel();
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[20,10]})).unwrap();
    let width = h.app.run("sketch.dimension", json!({"entities":["l3"],"value":20})).unwrap()["param"].as_str().unwrap().to_string();
    h.call("ui.view", json!({"view":"top"}));
    h.app.cam.distance = 80.;
    h.app.cam.target = solidcraft_engine::geom::Vec3::new(10., 5., 0.);
    h.settle();
    let at = crate::dim_view::text_at(&width).unwrap();
    let drawn = crate::dim_view::drawn();
    let baseline = h.app.session.doc.to_json();
    place(&mut h, &[[0., 0.], [0., 10.]], [-5., 5.]);
    type_value(&mut h, "12 mm");
    h.settle();
    assert_eq!(crate::dim_view::text_at(&width), Some(at));
    assert_eq!(crate::dim_view::drawn(), drawn, "draft annotations must not become committed hit targets");
    assert_eq!(h.app.session.doc.to_json(), baseline);
    h.step(&json!({"key":"Escape"})).unwrap();
    assert_eq!(crate::dim_view::drawn(), drawn);
}

#[test]
fn inch_dimension_entry_survives_unit_changes_undo_and_reopen() {
    let mut h = line([25.4, 0.]);
    h.app.run("document.units", json!({"units":"in"})).unwrap();
    place(&mut h, &[[12.7, 0.]], [12.7, 8.]);
    type_value(&mut h, "1.2345");
    h.step(&json!({"key":"Enter"})).unwrap();
    let sketch = h.app.session.active_sketch.unwrap();
    h.settle();
    crate::dim_view::edit_param(&h.app, "d1");
    h.settle();
    type_value(&mut h, "1.3456");
    h.step(&json!({"key":"Enter"})).unwrap();
    let edited = h.app.session.world_state();
    let edited = &edited.sketch(sketch).unwrap().sketch;
    assert!(edited.constraints.iter().any(|c| c.kind.value().is_some_and(|v| (v - 1.3456 * 25.4).abs() < 1e-6)));
    h.app.run("edit.undo", json!({})).unwrap();
    let before = h.app.session.world_state().sketch(sketch).unwrap().sketch.clone();
    let c = before.constraints.iter().find(|c| c.param.is_some()).unwrap();
    let p = h.app.session.doc.param(c.param.as_deref().unwrap()).unwrap();
    assert_eq!(crate::dim_view::label(&c.kind, Some(&p.expr), false, "in", 2), "1.2345");
    assert!((c.kind.value().unwrap() - 31.3563).abs() < 1e-6);
    h.app.run("document.units", json!({"units":"mm"})).unwrap();
    assert_eq!(h.app.session.world_state().sketch(sketch).unwrap().sketch, before);
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.doc.units, "in");
    let doc = solidcraft_engine::doc::Document::from_json(&h.app.session.doc.to_json()).unwrap();
    let reopened = solidcraft_engine::Session::new(doc);
    assert_eq!(reopened.doc.units, "in");
    assert_eq!(reopened.world_state().sketch(sketch).unwrap().sketch, before);
    crate::dim_view::cancel();
}

/// Placing the dimension of two unaligned points above them gives the horizontal distance,
/// beside them the vertical one, and across from them the distance between them (#257).
#[test]
fn placing_a_two_point_dimension_picks_horizontal_vertical_or_aligned() {
    // What each placement fixes at 10 mm: the X span, the Y span, or the length.
    let spans = |a: solidcraft_engine::geom::Vec2, b: solidcraft_engine::geom::Vec2| [(b.x - a.x).abs(), (b.y - a.y).abs(), a.dist(b)];
    for (at, measured) in [([6., 25.], 0), ([-10., 1.], 1), ([-5., 12.], 2)] {
        let mut h = line([12., 16.]);
        place(&mut h, &[[0., 0.], [12., 16.]], at);
        type_value(&mut h, "10 mm");
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(!crate::dim_view::editing());
        let state = h.app.session.world_state();
        let sk = &state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch;
        let a = sk.point(sk.resolve_point("l1.start").unwrap()).unwrap();
        let b = sk.point(sk.resolve_point("l1.end").unwrap()).unwrap();
        let spans = spans(a, b);
        assert!((spans[measured] - 10.0).abs() < 1e-6, "placed at {at:?}: {a:?} {b:?}");
        // The sketch's first dimension scales it (#1365): the other span keeps its proportion.
        if measured < 2 {
            let drawn = [12.0, 16.0];
            let other = drawn[1 - measured] * 10.0 / drawn[measured];
            assert!((spans[1 - measured] - other).abs() < 1e-6, "placed at {at:?}: {a:?} {b:?}");
        }
    }
}

/// A dimension that would over-constrain the sketch is placed as a driven (reference) one
/// instead of failing: added with no parameter or value editor, and removed again by one
/// undo (#320).
#[test]
fn an_over_constraining_dimension_is_placed_as_a_driven_one() {
    let mut h = line([20., 0.]);
    h.app.run("sketch.dimension", json!({"entities":["l1"],"value":"20 mm"})).unwrap();
    h.settle();
    let original = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    let params = h.app.session.doc.params.len();
    h.step(&json!({"start":"sketch.dimension"})).unwrap();
    h.step(&json!({"click":{"world":[10,0,0]}})).unwrap();
    h.step(&json!({"move":{"world":[10,6,0]}})).unwrap();
    h.settle();
    assert_eq!(crate::dim_view::drawn().len(), 1, "the preview is not a selectable dimension");
    h.step(&json!({"click":{"world":[10,6,0]}})).unwrap();
    assert!(!crate::dim_view::editing(), "a driven dimension has no value to edit");
    assert_eq!(h.app.session.undo.len(), undo + 1);
    assert_eq!(h.app.session.doc.params.len(), params, "a driven dimension adds no parameter");
    let state = h.app.session.world_state();
    let solved = state.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    assert!(solved.report.ok());
    let dims: Vec<_> = solved.sketch.constraints.iter().filter(|c| c.kind.is_dimension()).collect();
    assert_eq!(dims.len(), 2);
    assert_eq!(dims.iter().filter(|c| c.driven && c.param.is_none()).count(), 1, "{dims:?}");
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.doc.to_json(), original);
}

// ---- Type-first: D, click, type, Enter ----

/// A sketch with `setup` drawn, the Top view centred on `centre`, and the Dimension tool on.
fn sketch_with(setup: &[(&str, serde_json::Value)], centre: [f64; 2]) -> Harness {
    crate::dim_view::cancel();
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    for (cmd, params) in setup {
        h.app.run(cmd, params.clone()).unwrap();
    }
    h.call("ui.view", json!({"view":"top"}));
    h.app.cam.distance = 80.0;
    h.app.cam.target = solidcraft_engine::geom::Vec3::new(centre[0], centre[1], 0.0);
    h.settle();
    h.step(&json!({"start":"sketch.dimension"})).unwrap();
    h
}

/// Type as a keyboard does: each character is a key press with its text in the same frame,
/// so a letter that is also a tool shortcut reaches the shortcuts too.
fn keys(h: &mut Harness, text: &str) {
    for ch in text.chars() {
        let key = egui::Key::from_name(&ch.to_ascii_uppercase().to_string());
        let modifiers = egui::Modifiers::default();
        if let Some(key) = key {
            h.app.synthetic.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
        }
        h.app.synthetic.push(egui::Event::Text(ch.to_string()));
        if let Some(key) = key {
            h.app.synthetic.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
        }
        h.frames(1);
    }
    h.settle();
}

fn solved(h: &Harness) -> solidcraft_engine::sketch::Sketch {
    let state = h.app.session.world_state();
    state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch.clone()
}

/// The dimensions added since `before` (constraints with a parameter or a value).
fn added(h: &Harness, before: usize) -> Vec<solidcraft_engine::sketch::Constraint> {
    solved(h).constraints.into_iter().filter(|c| c.kind.is_dimension()).skip(before).collect()
}

fn dimension_count(h: &Harness) -> usize {
    solved(h).constraints.iter().filter(|c| c.kind.is_dimension()).count()
}

/// The value took the keyboard straight after the pick that determined the dimension.
fn assert_live(h: &Harness) {
    assert!(crate::dim_view::editing(), "the pick opens the value at once");
    assert_eq!(crate::dim_view::live(), Some(false));
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("sc_dim_edit")));
}

#[test]
fn type_first_line_circle_arc_distance_and_angle_commit_one_undo_step_each() {
    use solidcraft_engine::sketch::ConstraintKind as K;
    let line = [("sketch.line", json!({"points":[[0,0],[20,0]],"infer":false}))];
    let circle = [("sketch.circle.center", json!({"center":[10,0],"radius":5}))];
    let arc = [("sketch.arc.center_point", json!({"center":[10,0],"start":[18,0],"sweep":90}))];
    // Two lines joined at a fixed corner: the angle turns one about it.
    let corner = [
        ("sketch.line", json!({"points":[[0,20],[0,0],[20,0]],"infer":false})),
        ("sketch.constraint.fix", json!({"entity":"l1.end"})),
        ("sketch.constraint.horizontal_vertical", json!({"line":"l2","mode":"horizontal"})),
    ];
    let half = std::f64::consts::FRAC_1_SQRT_2 * 8.0;
    type Check = fn(&K) -> bool;
    let cases: [(&[(&str, serde_json::Value)], [f64; 2], Vec<[f64; 2]>, &str, Check); 5] = [
        (&line, [10., 0.], vec![[10., 0.]], "25", |k| matches!(k, K::Length { value, .. } if (value - 25.).abs() < 1e-6)),
        (&circle, [10., 0.], vec![[15., 0.]], "12", |k| matches!(k, K::Diameter { value, .. } if (value - 12.).abs() < 1e-6)),
        (&arc, [10., 0.], vec![[10. + half, half]], "6", |k| matches!(k, K::Radius { value, .. } if (value - 6.).abs() < 1e-6)),
        (&line, [10., 0.], vec![[0., 0.], [20., 0.]], "25", |k| k.value().is_some_and(|v| (v - 25.).abs() < 1e-6)),
        (&corner, [8., 8.], vec![[12., 0.], [0., 12.]], "60", |k| k.is_angle() && k.value().is_some_and(|v| (v.to_degrees() - 60.).abs() < 1e-6)),
    ];
    for (setup, centre, picks, typed, check) in cases {
        let mut h = sketch_with(setup, centre);
        let original = h.app.session.doc.to_json();
        let undo = h.app.session.undo.len();
        let dims = dimension_count(&h);
        for p in &picks {
            h.step(&json!({"click":{"world":[p[0],p[1],0]}})).unwrap();
        }
        assert_live(&h);
        assert_eq!(h.app.session.doc.to_json(), original, "nothing changes until Enter");
        // The text follows the pointer while typing: Enter places it there.
        // (For the angle, inside the wedge both before and after it changes.)
        let cursor = if check(&solidcraft_engine::sketch::ConstraintKind::Angle { a: 0, b: 1, value: 60f64.to_radians(), flip: false }) {
            [10.4, 6.0]
        } else {
            [centre[0] + 3.0, centre[1] + 9.0]
        };
        h.step(&json!({"move":{"world":[cursor[0],cursor[1],0]}})).unwrap();
        keys(&mut h, typed);
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(!crate::dim_view::editing(), "{picks:?}");
        assert_eq!(h.app.session.undo.len(), undo + 1, "{picks:?}");
        let new = added(&h, dims);
        assert_eq!(new.len(), 1, "{picks:?}: {new:?}");
        assert!(check(&new[0].kind), "{picks:?}: {:?}", new[0].kind);
        h.settle();
        let shown = crate::dim_view::text_at(&new[0].id).expect("the new dimension is drawn");
        let [x, y] = h.at(&json!({"world":[cursor[0],cursor[1],0]})).unwrap();
        assert!(shown.distance(egui::pos2(x as f32, y as f32)) < 40.0, "{picks:?}: text at {shown:?}, cursor at {x},{y}");
        assert_eq!(h.app.tool.as_ref().map(|t| (t.cmd, t.picks.len())), Some(("sketch.dimension", 0)), "ready for the next dimension");
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), original);
    }
    crate::dim_view::cancel();
}

#[test]
fn type_first_without_moving_uses_the_default_place_and_expressions_work() {
    let mut h = sketch_with(&[("sketch.line", json!({"points":[[0,0],[20,0]],"infer":false}))], [10., 0.]);
    h.app.run("parameters.add", json!({"name":"width","expression":"15 mm"})).unwrap();
    h.settle();
    let dims = dimension_count(&h);
    h.step(&json!({"click":{"world":[10,0,0]}})).unwrap();
    assert_live(&h);
    keys(&mut h, "2*width");
    h.step(&json!({"key":"Enter"})).unwrap();
    let new = added(&h, dims);
    assert_eq!(new.len(), 1);
    assert!(new[0].text.is_none(), "an unmoved pointer leaves the text in its default place: {:?}", new[0].text);
    assert!((new[0].kind.value().unwrap() - 30.).abs() < 1e-6);
    assert_eq!(h.app.session.doc.param(new[0].param.as_deref().unwrap()).unwrap().expr, "2*width");
    crate::dim_view::cancel();
}

/// Letters typed into the value are not tool shortcuts (L Line, D Dimension, C Circle…).
#[test]
fn type_first_parameter_names_are_not_taken_by_shortcuts() {
    for name in ["length", "depth", "clearance", "rim"] {
        let mut h = sketch_with(&[("sketch.line", json!({"points":[[0,0],[20,0]],"infer":false}))], [10., 0.]);
        h.app.run("parameters.add", json!({"name":name,"expression":"22 mm"})).unwrap();
        h.settle();
        let dims = dimension_count(&h);
        h.step(&json!({"click":{"world":[10,0,0]}})).unwrap();
        assert_live(&h);
        keys(&mut h, name);
        assert_eq!(h.app.tool.as_ref().map(|t| t.cmd), Some("sketch.dimension"), "{name}: a letter started another tool");
        assert!(h.app.dialog.is_none(), "{name}");
        h.step(&json!({"key":"Enter"})).unwrap();
        let new = added(&h, dims);
        assert_eq!(new.len(), 1, "{name}");
        assert!((new[0].kind.value().unwrap() - 22.).abs() < 1e-6, "{name}");
        assert_eq!(h.app.session.doc.param(new[0].param.as_deref().unwrap()).unwrap().expr, name);
        crate::dim_view::cancel();
    }
}

#[test]
fn type_first_escape_cancels_with_no_change() {
    let mut h = sketch_with(&[("sketch.line", json!({"points":[[0,0],[20,0]],"infer":false}))], [10., 0.]);
    let original = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    for typed in ["", "25"] {
        h.step(&json!({"click":{"world":[10,0,0]}})).unwrap();
        assert_live(&h);
        keys(&mut h, typed);
        h.step(&json!({"key":"Escape"})).unwrap();
        assert!(!crate::dim_view::editing());
        assert_eq!(h.app.session.doc.to_json(), original);
        assert_eq!(h.app.session.undo.len(), undo);
        assert_eq!(h.app.tool.as_ref().map(|t| (t.cmd, t.picks.len())), Some(("sketch.dimension", 0)), "Esc drops the picks too");
    }
    crate::dim_view::cancel();
}

/// Place first still works: a click before typing puts the text there and keeps the value
/// (selected, so typing replaces it); Tab leaves the picks for a click to place, as before.
#[test]
fn type_first_keeps_the_place_first_flow() {
    let mut h = sketch_with(&[("sketch.line", json!({"points":[[0,0],[20,0]],"infer":false}))], [10., 0.]);
    let dims = dimension_count(&h);
    h.step(&json!({"click":{"world":[10,0,0]}})).unwrap();
    assert_live(&h);
    h.step(&json!({"click":{"world":[10,-7,0]}})).unwrap();
    assert!(crate::dim_view::editing());
    assert_eq!(crate::dim_view::live(), None, "the text is pinned");
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("sc_dim_edit")));
    h.step(&json!({"move":{"world":[0,12,0]}})).unwrap();
    keys(&mut h, "18");
    h.step(&json!({"key":"Enter"})).unwrap();
    let new = added(&h, dims);
    assert_eq!(new.len(), 1);
    assert!((new[0].kind.value().unwrap() - 18.).abs() < 1e-6);
    h.settle();
    let shown = crate::dim_view::text_at(&new[0].id).unwrap();
    let [x, y] = h.at(&json!({"world":[10,-7,0]})).unwrap();
    assert!(shown.distance(egui::pos2(x as f32, y as f32)) < 40.0, "the pinned place wins over the pointer: {shown:?} vs {x},{y}");
    // Tab before typing hands the picks back to the place-first flow.
    h.app.run("edit.undo", json!({})).unwrap();
    h.settle();
    h.step(&json!({"click":{"world":[10,0,0]}})).unwrap();
    assert_live(&h);
    h.frames(4);
    h.step(&json!({"key":"Tab"})).unwrap();
    h.settle();
    assert!(!crate::dim_view::editing());
    assert_eq!(h.app.tool.as_ref().map(|t| t.picks.len()), Some(1));
    h.step(&json!({"click":{"world":[10,6,0]}})).unwrap();
    assert!(crate::dim_view::editing());
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("sc_dim_edit")));
    keys(&mut h, "21");
    h.step(&json!({"key":"Enter"})).unwrap();
    let new = added(&h, dims);
    assert!((new[0].kind.value().unwrap() - 21.).abs() < 1e-6);
    crate::dim_view::cancel();
}
