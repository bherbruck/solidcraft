//! The sketch modify tool windows. Sketch Scale's toolbar button used to reach no tool and no
//! dialog, so a press ran `sketch.scale` with no parameters and the engine refused it. Mirror
//! ran as a click flow that collected entities until a right-click, took the next click as the
//! mirror line and committed at once, with nothing on screen saying what was picked.
//! `sketch.move` had no toolbar placement, no tool and no dialog either. Its only route into
//! the UI was the sketch context menu, and that went through the same empty-parameter
//! fallback, so the item reported an engine error. The rectangular pattern's click tool sent
//! a hard-coded count of 3 and could reach neither the command's second count nor either
//! spacing.

use serde_json::json;
use solidcraft_engine::Sel;

use crate::dialogs::{Kind, apply_commands};
use crate::scenario::Harness;
use crate::selection::{AXES, CURVES, POINTS, SelInput};
use crate::sketch_modify_ui::{Mod, commands};

#[test]
fn existing_patterns_reopen_from_menu_and_badge_and_edit_without_changing_surviving_ids() {
    for circular in [false, true] {
        let mut h = Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.line", json!({"points":[[10,0],[20,0]],"ids":["l1"]})).unwrap();
        let cmd = if circular { "sketch.pattern.circular" } else { "sketch.pattern.rectangular" };
        let p = if circular {
            json!({"entities":["l1"],"center":"origin","count":4})
        } else {
            json!({"entities":["l1"],"dir":[0,1],"count":4,"spacing":12})
        };
        let out = h.app.run(cmd, p).unwrap();
        let before = h.app.session.doc.to_json();
        h.app.session.selection = vec![Sel::SketchCurve { id: "l2".into() }];
        h.settle();
        let menu = crate::context_menu::items(&h.app, &crate::context_menu::Target::Viewport);
        let item = menu.iter().find(|item| item.id == "ui.editSketchPattern").expect("a copy offers Edit Sketch Pattern");
        crate::context_menu::run_item(&mut h.app, item, egui::pos2(0.0, 0.0));
        assert_eq!(h.app.session.doc.to_json(), before, "opening the editor commits nothing");
        let d = h.app.dialog.as_ref().unwrap();
        assert_eq!(d.extra["pattern"], out["pattern"]);
        let cmds = apply_commands(&h.app, d).unwrap();
        if !circular {
            assert_eq!(cmds[0].1["dir"], json!([0.0, 1.0]));
        }
        h.app.dialog = None;
        h.settle();
        let cid = out["constraints"][0].as_str().unwrap();
        let at = crate::sketch_tools::glyph_positions().into_iter().find(|(id, _)| id == cid).expect("pattern has a badge").1;
        assert!(crate::sketch_modify_ui::double_click_pattern(&mut h.app, at));
        let d = h.app.dialog.as_mut().unwrap();
        match &mut d.kind {
            Kind::SketchModify(Mod::PatternCirc { count, .. }) | Kind::SketchModify(Mod::PatternRect { count, .. }) => *count = "5".into(),
            _ => panic!("wrong pattern editor"),
        }
        let d = h.app.dialog.clone().unwrap();
        let cmds = apply_commands(&h.app, &d).unwrap();
        let result = h.app.run(&cmds[0].0, cmds[0].1.clone()).unwrap();
        assert_eq!(result["curves"][0], "l2");
        assert_eq!(result["curves"].as_array().unwrap().len(), 4);
    }
}

/// A sketch on XY holding one 20 by 10 rectangle at the origin.
fn rectangle() -> Harness {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [20, 10]})).unwrap();
    h.settle();
    h
}

fn curve_ids(h: &Harness) -> Vec<String> {
    let sid = h.app.session.active_sketch.unwrap();
    let st = h.app.session.model.state();
    st.sketch(sid).unwrap().sketch.curves.iter().map(|c| c.id.clone()).collect()
}

#[test]
fn the_toolbar_button_opens_a_window_instead_of_reporting_an_engine_error() {
    let mut h = rectangle();
    let before = curve_ids(&h).len();
    h.step(&json!({"start": "sketch.scale"})).unwrap();
    let d = h.app.dialog.as_ref().expect("sketch.scale opens a tool window");
    assert!(matches!(d.kind, Kind::SketchModify(Mod::Scale { .. })), "{:?}", d.kind);
    assert_eq!(d.inputs.iter().map(|i| i.label).collect::<Vec<_>>(), vec!["Entities", "Base point"]);
    // Opening the window commits nothing, and the old dead button's error is gone.
    assert_eq!(curve_ids(&h).len(), before);
    assert!(!h.app.status.as_ref().is_some_and(|(_, _, error)| *error), "{:?}", h.app.status);
}

#[test]
fn the_typed_factor_reaches_the_command_unchanged() {
    let h = rectangle();
    let ids = curve_ids(&h);
    let mut inputs = vec![SelInput::new("Entities", crate::selection::CURVES, true), SelInput::new("Base point", crate::selection::POINTS, false)];
    inputs[0].items = ids.iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
    let k = Mod::Scale { factor: "2.5".into() };
    let out = commands(&h.app, &k, &inputs, &serde_json::Map::new()).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, "sketch.scale");
    assert_eq!(out[0].1["factor"], json!(2.5));
    assert_eq!(out[0].1["entities"], json!(ids));
    // No base point picked: the command's own default, the sketch origin, stands.
    assert!(out[0].1.get("base").is_none(), "{}", out[0].1);
}

#[test]
fn an_expression_in_the_factor_field_is_evaluated() {
    let mut h = rectangle();
    h.app.run("parameters.add", json!({"name": "growth", "expression": "3", "unit": ""})).unwrap();
    let ids = curve_ids(&h);
    let mut inputs = vec![SelInput::new("Entities", crate::selection::CURVES, true)];
    inputs[0].items = ids.iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
    let out = commands(&h.app, &Mod::Scale { factor: "growth * 2".into() }, &inputs, &serde_json::Map::new()).unwrap();
    assert_eq!(out[0].1["factor"], json!(6.0));
}

#[test]
fn a_picked_base_point_loses_the_sketch_prefix_that_picking_adds() {
    let h = rectangle();
    let sid = h.app.session.active_sketch.unwrap();
    let ids = curve_ids(&h);
    let mut inputs = vec![SelInput::new("Entities", crate::selection::CURVES, true), SelInput::new("Base point", crate::selection::POINTS, false)];
    inputs[0].items = vec![Sel::SketchCurve { id: ids[0].clone() }];
    let point = {
        let st = h.app.session.model.state();
        st.sketch(sid).unwrap().sketch.points.first().unwrap().id.clone()
    };
    inputs[1].items = vec![Sel::SketchPoint { id: format!("{sid}:{point}") }];
    let out = commands(&h.app, &Mod::Scale { factor: "2".into() }, &inputs, &serde_json::Map::new()).unwrap();
    assert_eq!(out[0].1["base"], json!(point));
}

#[test]
fn an_empty_selection_or_a_bad_factor_is_an_error_the_window_shows() {
    let h = rectangle();
    let ids = curve_ids(&h);
    let empty = vec![SelInput::new("Entities", crate::selection::CURVES, true)];
    assert!(commands(&h.app, &Mod::Scale { factor: "2".into() }, &empty, &serde_json::Map::new()).is_err());
    let mut filled = empty.clone();
    filled[0].items = vec![Sel::SketchCurve { id: ids[0].clone() }];
    for bad in ["0", "-2", "", "nonsense"] {
        let r = commands(&h.app, &Mod::Scale { factor: bad.into() }, &filled, &serde_json::Map::new());
        assert!(r.is_err(), "factor `{bad}` should not reach the command: {r:?}");
    }
}

#[test]
fn the_window_scales_the_picked_geometry_and_leaves_the_rest_alone() {
    let mut h = rectangle();
    h.app.run("sketch.circle.center", json!({"center": [60, 0], "radius": 5})).unwrap();
    h.settle();
    let sid = h.app.session.active_sketch.unwrap();
    let ids = curve_ids(&h);
    let rect: Vec<String> = ids.iter().take(4).cloned().collect();
    h.step(&json!({"start": "sketch.scale"})).unwrap();
    {
        let d = h.app.dialog.as_mut().unwrap();
        d.inputs[0].items = rect.iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
        d.kind = Kind::SketchModify(Mod::Scale { factor: "2".into() });
    }
    // A frame with the picks in place: the canvas value box takes focus on the frame after a
    // pick, and Enter only applies once it has had it.
    h.frames(2);
    h.step(&json!({"key": "Enter"})).unwrap();
    h.settle();
    assert!(h.app.dialog.is_none(), "Enter applies and closes the window");
    let st = h.app.session.model.state();
    let sk = &st.sketch(sid).unwrap().sketch;
    let xs: Vec<f64> = sk.points.iter().map(|p| p.pos.x).collect();
    let ys: Vec<f64> = sk.points.iter().map(|p| p.pos.y).collect();
    // The rectangle doubled about the sketch origin; the circle at x = 60 did not move.
    assert!(xs.iter().any(|x| (x - 40.0).abs() < 1e-6), "{xs:?}");
    assert!(ys.iter().any(|y| (y - 20.0).abs() < 1e-6), "{ys:?}");
    assert!(xs.iter().any(|x| (x - 60.0).abs() < 1e-6), "{xs:?}");
    assert_eq!(sk.curves.len(), 5);
}

/// A sketch on XY holding a vertical line on the Y axis and a circle to its right.
fn line_and_circle() -> Harness {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.run("sketch.line", json!({"points": [[0, -20], [0, 20]]})).unwrap();
    h.app.run("sketch.circle.center", json!({"center": [10, 0], "radius": 3})).unwrap();
    h.settle();
    h
}

fn mirror_inputs() -> Vec<SelInput> {
    vec![SelInput::new("Entities", CURVES | POINTS, true), SelInput::new("Mirror line", AXES, false)]
}

#[test]
fn the_toolbar_button_opens_a_window_with_both_picks_visible() {
    let mut h = line_and_circle();
    let before = curve_ids(&h).len();
    h.step(&json!({"start": "sketch.mirror"})).unwrap();
    let d = h.app.dialog.as_ref().expect("sketch.mirror opens a tool window");
    assert!(matches!(d.kind, Kind::SketchModify(Mod::Mirror)), "{:?}", d.kind);
    assert_eq!(d.inputs.iter().map(|i| i.label).collect::<Vec<_>>(), vec!["Entities", "Mirror line"]);
    assert!(h.app.tool.is_none(), "the dialog replaces the old click tool");
    // Opening the window commits nothing.
    assert_eq!(curve_ids(&h).len(), before);
}

#[test]
fn the_two_picks_become_the_command() {
    let h = line_and_circle();
    let ids = curve_ids(&h);
    let (line, circle) = (ids[0].clone(), ids[1].clone());
    let mut inputs = mirror_inputs();
    inputs[0].items = vec![Sel::SketchCurve { id: circle.clone() }];
    inputs[1].items = vec![Sel::SketchCurve { id: line.clone() }];
    let out = commands(&h.app, &Mod::Mirror, &inputs, &serde_json::Map::new()).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, "sketch.mirror");
    assert_eq!(out[0].1, json!({"entities": [circle], "line": line}));
}

#[test]
fn a_half_filled_window_is_an_error_instead_of_a_command() {
    let h = line_and_circle();
    let ids = curve_ids(&h);
    let mut only_entities = mirror_inputs();
    only_entities[0].items = vec![Sel::SketchCurve { id: ids[1].clone() }];
    assert!(commands(&h.app, &Mod::Mirror, &only_entities, &serde_json::Map::new()).is_err());
    let mut only_line = mirror_inputs();
    only_line[1].items = vec![Sel::SketchCurve { id: ids[0].clone() }];
    assert!(commands(&h.app, &Mod::Mirror, &only_line, &serde_json::Map::new()).is_err());
    // The AXES mask also takes an origin axis, which is no curve of this sketch.
    let mut origin_axis = mirror_inputs();
    origin_axis[0].items = vec![Sel::SketchCurve { id: ids[1].clone() }];
    origin_axis[1].items = vec![Sel::Axis { name: "Y".into() }];
    assert!(commands(&h.app, &Mod::Mirror, &origin_axis, &serde_json::Map::new()).is_err());
    // The line itself must not be in the entities: the old click flow could not express that
    // either, and the engine silently dropped it.
    let mut line_in_both = mirror_inputs();
    line_in_both[0].items = vec![Sel::SketchCurve { id: ids[0].clone() }];
    line_in_both[1].items = vec![Sel::SketchCurve { id: ids[0].clone() }];
    assert!(commands(&h.app, &Mod::Mirror, &line_in_both, &serde_json::Map::new()).is_err());
}

#[test]
fn the_window_reflects_the_picked_circle_across_the_line() {
    let mut h = line_and_circle();
    let sid = h.app.session.active_sketch.unwrap();
    let ids = curve_ids(&h);
    h.step(&json!({"start": "sketch.mirror"})).unwrap();
    {
        let d = h.app.dialog.as_mut().unwrap();
        d.inputs[0].items = vec![Sel::SketchCurve { id: ids[1].clone() }];
        d.inputs[1].items = vec![Sel::SketchCurve { id: ids[0].clone() }];
    }
    // A frame with the picks in place: the dialog reads them when it next draws.
    h.frames(2);
    h.step(&json!({"key": "Enter"})).unwrap();
    h.settle();
    assert!(h.app.dialog.is_none(), "Enter applies and closes the window");
    let st = h.app.session.model.state();
    let sk = &st.sketch(sid).unwrap().sketch;
    assert_eq!(sk.curves.len(), 3, "{:?}", h.app.status);
    let made = sk.curves.last().unwrap();
    // `CurveKind` is not Copy: match the field behind its reference.
    let solidcraft_engine::sketch::CurveKind::Circle { c, r } = &made.kind else { panic!("{:?}", made.kind) };
    let at = sk.point(*c).unwrap();
    assert!((at.x + 10.0).abs() < 1e-6 && at.y.abs() < 1e-6, "{at:?}");
    assert!((*r - 3.0).abs() < 1e-6, "{r}");
}

fn move_inputs(h: &Harness) -> Vec<SelInput> {
    let mut v = vec![SelInput::new("Entities", CURVES | POINTS, true), SelInput::new("Center", POINTS, false)];
    v[0].items = curve_ids(h).iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
    v
}

/// A fresh Move window's values, with one field replaced. Functional record update (`..base`)
/// is a struct feature, so an enum variant has to spell every field out.
fn moved(x: &str, y: &str, angle: &str) -> Mod {
    Mod::Move { x: x.into(), y: y.into(), angle: angle.into(), copy: false }
}

#[test]
fn the_command_now_has_a_toolbar_placement_and_opens_a_window() {
    let spec = solidcraft_engine::find_command("sketch.move").expect("sketch.move is registered");
    assert_eq!((spec.tab, spec.panel), ("SKETCH", "MODIFY"));
    assert_eq!(spec.icon, "sketch_move");
    assert_eq!(crate::icon_svg::resolve(spec.icon).master, "move", "reuse the finished Move drawing");

    let mut h = rectangle();
    let before = curve_ids(&h).len();
    h.step(&json!({"start": "sketch.move"})).unwrap();
    let d = h.app.dialog.as_ref().expect("sketch.move opens a tool window");
    assert!(matches!(d.kind, Kind::SketchModify(Mod::Move { .. })), "{:?}", d.kind);
    assert_eq!(d.inputs.iter().map(|i| i.label).collect::<Vec<_>>(), vec!["Entities", "Center"]);
    // Opening the window commits nothing, and the context menu's engine error is gone.
    assert_eq!(curve_ids(&h).len(), before);
    assert!(!h.app.status.as_ref().is_some_and(|(_, _, error)| *error), "{:?}", h.app.status);
}

#[test]
fn the_context_menu_item_fills_the_window_from_the_selection() {
    let mut h = rectangle();
    let ids = curve_ids(&h);
    h.app.run("select.set", json!({"items": [{"type": "sketch_curve", "id": ids[0]}]})).unwrap();
    h.step(&json!({"start": "sketch.move"})).unwrap();
    let d = h.app.dialog.as_ref().unwrap();
    assert_eq!(d.inputs[0].items, vec![Sel::SketchCurve { id: ids[0].clone() }]);
}

#[test]
fn the_typed_distances_and_angle_reach_the_command() {
    let h = rectangle();
    let inputs = move_inputs(&h);
    let k = Mod::Move { x: "10 mm".into(), y: "-4 mm".into(), angle: "90 deg".into(), copy: false };
    let out = commands(&h.app, &k, &inputs, &serde_json::Map::new()).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, "sketch.move");
    assert_eq!(out[0].1["translate"], json!([10.0, -4.0]));
    assert!((out[0].1["angle"].as_f64().unwrap() - 90.0).abs() < 1e-9, "{}", out[0].1["angle"]);
    assert_eq!(out[0].1["copy"], json!(false));
    // No centre picked: the command's own default, the sketch origin, stands.
    assert!(out[0].1.get("center").is_none(), "{}", out[0].1);
}

#[test]
fn an_expression_and_a_bare_angle_are_evaluated() {
    let mut h = rectangle();
    h.app.run("parameters.add", json!({"name": "step", "expression": "7 mm", "unit": "mm"})).unwrap();
    let inputs = move_inputs(&h);
    let k = Mod::Move { x: "step * 2".into(), y: String::new(), angle: "45".into(), copy: false };
    let out = commands(&h.app, &k, &inputs, &serde_json::Map::new()).unwrap();
    assert_eq!(out[0].1["translate"], json!([14.0, 0.0]));
    assert!((out[0].1["angle"].as_f64().unwrap() - 45.0).abs() < 1e-9, "{}", out[0].1["angle"]);
}

#[test]
fn an_empty_window_or_a_zero_move_is_an_error_instead_of_a_command() {
    let h = rectangle();
    let empty = vec![SelInput::new("Entities", CURVES | POINTS, true)];
    assert!(commands(&h.app, &moved("5 mm", "0 mm", "0 deg"), &empty, &serde_json::Map::new()).is_err());
    // Nothing typed: the dialog says so, and no move of zero reaches the command.
    assert!(commands(&h.app, &moved("0 mm", "0 mm", "0 deg"), &move_inputs(&h), &serde_json::Map::new()).is_err());
    assert!(commands(&h.app, &moved("nonsense", "0 mm", "0 deg"), &move_inputs(&h), &serde_json::Map::new()).is_err());
}

#[test]
fn the_window_moves_the_rectangle_and_the_copy_row_leaves_the_original_behind() {
    let mut h = rectangle();
    let sid = h.app.session.active_sketch.unwrap();
    h.step(&json!({"start": "sketch.move"})).unwrap();
    // Read the ids before the dialog is borrowed mutably: `curve_ids` borrows the harness.
    let picks: Vec<Sel> = curve_ids(&h).iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
    {
        let d = h.app.dialog.as_mut().unwrap();
        d.inputs[0].items = picks;
        d.kind = Kind::SketchModify(Mod::Move { x: "30 mm".into(), y: "0 mm".into(), angle: "0 deg".into(), copy: true });
    }
    // A frame with the picks in place: the canvas value box takes focus on the frame after a
    // pick, and Enter only applies once it has had it.
    h.frames(2);
    h.step(&json!({"key": "Enter"})).unwrap();
    h.settle();
    assert!(h.app.dialog.is_none(), "Enter applies and closes the window");
    let st = h.app.session.model.state();
    let sk = &st.sketch(sid).unwrap().sketch;
    assert_eq!(sk.curves.len(), 8, "a copy keeps the original: {:?}", h.app.status);
    let xs: Vec<f64> = sk.points.iter().map(|p| p.pos.x).collect();
    assert!(xs.iter().any(|x| x.abs() < 1e-6), "the original stays at x = 0: {xs:?}");
    assert!(xs.iter().any(|x| (x - 50.0).abs() < 1e-6), "the copy's far corner is at x = 50: {xs:?}");
}

#[test]
fn a_plain_move_takes_the_geometry_with_it() {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    // Clear of the sketch origin. A rectangle drawn from the origin keeps a corner tied to it,
    // and the solver pulls that corner back after the move, which measures the solver rather
    // than this window.
    let made = h.app.run("sketch.rectangle.two_point", json!({"p0": [10, 10], "p1": [30, 20]})).unwrap();
    let corners: Vec<String> = made["corners"].as_object().into_iter().flatten().filter_map(|(_, v)| v.as_str().map(str::to_string)).collect();
    assert_eq!(corners.len(), 4, "{made}");
    h.settle();
    let sid = h.app.session.active_sketch.unwrap();
    h.step(&json!({"start": "sketch.move"})).unwrap();
    // Read the ids before the dialog is borrowed mutably: `curve_ids` borrows the harness.
    let picks: Vec<Sel> = curve_ids(&h).iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
    {
        let d = h.app.dialog.as_mut().unwrap();
        d.inputs[0].items = picks;
        d.kind = Kind::SketchModify(Mod::Move { x: "0 mm".into(), y: "5 mm".into(), angle: "0 deg".into(), copy: false });
    }
    // A frame with the picks in place: the canvas value box takes focus on the frame after a
    // pick, and Enter only applies once it has had it.
    h.frames(2);
    h.step(&json!({"key": "Enter"})).unwrap();
    h.settle();
    let st = h.app.session.model.state();
    let sk = &st.sketch(sid).unwrap().sketch;
    assert_eq!(sk.curves.len(), 4, "a plain move makes nothing new: {:?}", h.app.status);
    // The rectangle's own corners, not the sketch origin, which no move touches.
    let ys: Vec<f64> = sk.points.iter().filter(|p| corners.contains(&p.id)).map(|p| p.pos.y).collect();
    assert_eq!(ys.len(), 4, "{ys:?}");
    assert!(ys.iter().all(|y| (*y - 15.0).abs() < 1e-6 || (*y - 25.0).abs() < 1e-6), "the rectangle moved up by 5 mm: {ys:?}");
}

#[test]
fn escape_leaves_the_sketch_exactly_as_it_was() {
    let mut h = line_and_circle();
    let sid = h.app.session.active_sketch.unwrap();
    let undo = h.app.session.undo.len();
    let before = curve_ids(&h);
    let constraints = h.app.session.model.state().sketch(sid).unwrap().sketch.constraints.len();
    h.step(&json!({"start": "sketch.mirror"})).unwrap();
    {
        let d = h.app.dialog.as_mut().unwrap();
        d.inputs[0].items = vec![Sel::SketchCurve { id: before[1].clone() }];
        d.inputs[1].items = vec![Sel::SketchCurve { id: before[0].clone() }];
    }
    h.frames(2);
    h.step(&json!({"key": "Escape"})).unwrap();
    h.settle();
    assert!(h.app.dialog.is_none());
    assert_eq!(curve_ids(&h), before);
    assert_eq!(h.app.session.undo.len(), undo);
    assert_eq!(h.app.session.model.state().sketch(sid).unwrap().sketch.constraints.len(), constraints);
}

#[test]
fn the_x_distance_gets_a_drag_arrow_along_the_sketch_x_axis() {
    let mut h = rectangle();
    h.step(&json!({"start": "sketch.move"})).unwrap();
    // Read the ids before the dialog is borrowed mutably: `curve_ids` borrows the harness.
    let picks: Vec<Sel> = curve_ids(&h).iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
    {
        let d = h.app.dialog.as_mut().unwrap();
        d.inputs[0].items = picks;
    }
    let sid = h.app.session.active_sketch.unwrap();
    let x = h.app.session.world_state().sketch(sid).unwrap().plane.x;
    let d = h.app.dialog.as_ref().unwrap();
    let (_, dir) = crate::canvas::anchor(&h.app, d).expect("a picked sketch curve anchors the manipulator");
    let dir = dir.expect("a sketch move's X Distance has a direction to drag along");
    assert!((dir - x).len() < 1e-9, "{dir:?} is not the sketch x axis {x:?}");
}

/// A sketch holding one circle, with the window for `cmd` open on it.
fn open(cmd: &str) -> (crate::scenario::Harness, String) {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.run("sketch.circle.center", json!({"center": [0, 0], "radius": 5})).unwrap();
    h.settle();
    let sid = h.app.session.active_sketch.unwrap();
    let id = h.app.session.model.state().sketch(sid).unwrap().sketch.curves[0].id.clone();
    h.step(&json!({"start": cmd})).unwrap();
    (h, id)
}

fn rect(h: &mut crate::scenario::Harness, count: &str, spacing: &str, count2: &str, spacing2: &str) {
    let d = h.app.dialog.as_mut().expect("the window opened");
    if let Kind::SketchModify(Mod::PatternRect { count: c1, spacing: s1, count2: c2, spacing2: s2, .. }) = &mut d.kind {
        *c1 = count.into();
        *s1 = spacing.into();
        *c2 = count2.into();
        *s2 = spacing2.into();
    } else {
        panic!("not the rectangular pattern window: {:?}", d.kind);
    }
}

#[test]
fn rectangular_pattern_window_opens_instead_of_a_click_tool() {
    let (h, _) = open("sketch.pattern.rectangular");
    assert!(h.app.tool.is_none(), "the click tool still takes the command");
    let d = h.app.dialog.as_ref().expect("the window opened");
    assert!(matches!(d.kind, Kind::SketchModify(Mod::PatternRect { .. })), "{:?}", d.kind);
    assert_eq!(d.inputs.iter().map(|i| i.label).collect::<Vec<_>>(), vec!["Entities", "Direction", "Direction 2"]);
    // Nothing is committed by opening the window.
    let sid = h.app.session.active_sketch.unwrap();
    assert_eq!(h.app.session.model.state().sketch(sid).unwrap().sketch.curves.len(), 1);
}

#[test]
fn rectangular_columns_flip_and_center_independently_and_restore_even_counts_when_editing() {
    use solidcraft_engine::sketch::PatternAlignment;
    let (mut h, id) = open("sketch.pattern.rectangular");
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::SketchCurve { id }];
    rect(&mut h, "4", "20", "2", "10");
    h.settle();
    let first = crate::scenario::handle_at("dialog:Spacing").unwrap();
    let second = crate::scenario::handle_at("dialog:Spacing 2").unwrap();
    assert!(second.x - first.x > 180.0, "the directions should occupy separate columns: {first:?}, {second:?}");
    assert!((second.y - first.y).abs() < 2.0, "the independent spacing fields should share a row");
    for label in ["Alignment", "Alignment 2"] {
        let left = crate::scenario::handle_at(&format!("dialog:{label}:Flip direction")).unwrap();
        let right = crate::scenario::handle_at(&format!("dialog:{label}:Center")).unwrap();
        assert!((left.y - right.y).abs() < 2.0, "alignment choices should fit on a single line: {left:?}, {right:?}");
    }
    h.step(&json!({"click":{"handle":"dialog:Alignment:Center"}})).unwrap();
    h.step(&json!({"click":{"handle":"dialog:Alignment 2:Flip direction"}})).unwrap();
    let d = h.app.dialog.as_ref().unwrap();
    let commands = apply_commands(&h.app, d).unwrap();
    assert_eq!(commands[0].1["alignment"], "center");
    assert_eq!(commands[0].1["alignment2"], "right");
    assert_eq!(commands[0].1["reversed2"], true);
    assert_eq!(commands[0].1["count"], 4);
    h.step(&json!({"click":{"handle":"dialog:OK"}})).unwrap();
    h.settle();
    assert!(h.app.dialog.is_none());
    let sid = h.app.session.active_sketch.unwrap();
    let pattern = h.app.session.world_state().sketch(sid).unwrap().sketch.patterns[0].id.clone();
    assert!(crate::sketch_modify_ui::edit_pattern(&mut h.app, &pattern));
    assert!(matches!(
        h.app.dialog.as_ref().unwrap().kind,
        Kind::SketchModify(Mod::PatternRect { alignment: PatternAlignment::Center, alignment2: PatternAlignment::Right, reversed2: true, .. })
    ));
    rect(&mut h, "4", "20", "3", "10");
    assert!(apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).is_ok());
    h.settle();
    h.step(&json!({"click":{"handle":"dialog:Alignment:Flip direction"}})).unwrap();
    h.step(&json!({"click":{"handle":"dialog:Alignment 2:Center"}})).unwrap();
    let commands = apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    assert_eq!(commands[0].1["alignment"], "center");
    assert_eq!(commands[0].1["reversed"], true);
    assert_eq!(commands[0].1["alignment2"], "center");
    assert_eq!(commands[0].1["reversed2"], true);
}

#[test]
fn typed_counts_and_spacings_reach_the_command_with_the_second_direction() {
    let (mut h, id) = open("sketch.pattern.rectangular");
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::SketchCurve { id }];
    rect(&mut h, "5", "20", "2", "15 mm");
    let d = h.app.dialog.clone().unwrap();
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].0, "sketch.pattern.rectangular");
    // The click tool could only ever send a count of 3 and no second count at all.
    assert_eq!(cmds[0].1["count"], json!(5));
    assert_eq!(cmds[0].1["spacing"], json!(20.0));
    assert_eq!(cmds[0].1["count2"], json!(2));
    assert_eq!(cmds[0].1["spacing2"], json!(15.0));
    assert!(cmds[0].1.get("dir").is_none(), "an unpicked direction leaves the command's own default");
}

#[test]
fn a_picked_axis_becomes_an_in_plane_direction() {
    let (mut h, id) = open("sketch.pattern.rectangular");
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::SketchCurve { id }];
    h.app.dialog.as_mut().unwrap().inputs[1].items = vec![Sel::Axis { name: "Y".into() }];
    rect(&mut h, "3", "10", "1", "10");
    let d = h.app.dialog.clone().unwrap();
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(cmds[0].1["dir"], json!([0.0, 1.0]), "sketch y on the XY plane");
}

#[test]
fn two_picked_lines_are_sent_as_live_references_and_restored_in_the_editor() {
    let (mut h, id) = open("sketch.pattern.rectangular");
    h.app.run("sketch.line", json!({"points":[[0,-20],[10,-20]],"ids":["dir1"]})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,-40],[6,-32]],"ids":["dir2"]})).unwrap();
    let d = h.app.dialog.as_mut().unwrap();
    d.inputs[0].items = vec![Sel::SketchCurve { id }];
    d.inputs[1].items = vec![Sel::SketchCurve { id: "dir1".into() }];
    d.inputs[2].items = vec![Sel::SketchCurve { id: "dir2".into() }];
    rect(&mut h, "3", "20", "2", "10");
    let d = h.app.dialog.clone().unwrap();
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(cmds[0].1["direction"], "dir1");
    assert_eq!(cmds[0].1["direction2"], "dir2");
    let out = h.app.run(&cmds[0].0, cmds[0].1.clone()).unwrap();
    assert!(crate::sketch_modify_ui::edit_pattern(&mut h.app, out["pattern"].as_str().unwrap()));
    let d = h.app.dialog.as_ref().unwrap();
    assert_eq!(d.inputs[1].items, vec![Sel::SketchCurve { id: "dir1".into() }]);
    assert_eq!(d.inputs[2].items, vec![Sel::SketchCurve { id: "dir2".into() }]);
    let reopened = apply_commands(&h.app, d).unwrap();
    assert_eq!(reopened[0].1["direction2"], "dir2");
}

#[test]
fn sketch_pattern_tools_are_promoted_together_and_open_from_the_toolbar() {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    let panels = crate::toolbar_custom::panel_lists("SKETCH");
    let (_, cmds, promoted) = panels.iter().find(|(panel, ..)| *panel == "PATTERN").unwrap();
    let ids: Vec<_> = cmds.iter().take(*promoted).map(|c| c.id).collect();
    assert_eq!(ids, ["sketch.mirror", "sketch.pattern.circular", "sketch.pattern.rectangular"]);
    for id in ids {
        h.settle();
        h.step(&json!({"click":{"handle":format!("toolbar:{id}")}})).unwrap();
        assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::SketchModify(_))), "{id}");
        h.step(&json!({"key":"Escape"})).unwrap();
    }
}

#[test]
fn an_axis_perpendicular_to_the_sketch_is_reported_not_silently_replaced() {
    let (mut h, id) = open("sketch.pattern.rectangular");
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::SketchCurve { id }];
    h.app.dialog.as_mut().unwrap().inputs[1].items = vec![Sel::Axis { name: "Z".into() }];
    rect(&mut h, "3", "10", "1", "10");
    let d = h.app.dialog.clone().unwrap();
    let e = apply_commands(&h.app, &d).unwrap_err();
    assert!(e.contains("sketch plane"), "{e}");
}

/// Upstream #218 is the same shape: `solid.pattern.circular` took expressions in its axis origin
/// and stored `[0, 0, 0]`. The sketch patterns read `count` and `angle` with `num`, which takes a
/// JSON number only, so the window evaluates every field itself.
#[test]
fn a_count_expression_is_evaluated_rather_than_sent_as_a_string() {
    let (mut h, id) = open("sketch.pattern.rectangular");
    h.app.run("parameters.add", json!({"name": "n", "expression": "4", "unit": ""})).unwrap();
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::SketchCurve { id }];
    rect(&mut h, "n", "10", "1", "10");
    let d = h.app.dialog.clone().unwrap();
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(cmds[0].1["count"], json!(4));
}

#[test]
fn bad_counts_and_an_empty_selection_are_refused_before_the_command_runs() {
    let (mut h, id) = open("sketch.pattern.rectangular");
    rect(&mut h, "5", "20", "1", "20");
    let d = h.app.dialog.clone().unwrap();
    assert!(apply_commands(&h.app, &d).unwrap_err().contains("Select"));
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::SketchCurve { id }];
    for bad in ["0", "2.5", "501", "wat"] {
        rect(&mut h, bad, "20", "1", "20");
        let d = h.app.dialog.clone().unwrap();
        assert!(apply_commands(&h.app, &d).is_err(), "count {bad} was accepted");
    }
    rect(&mut h, "5", "0", "1", "20");
    let d = h.app.dialog.clone().unwrap();
    assert!(apply_commands(&h.app, &d).unwrap_err().contains("Spacing"));
}

/// The spacing is a geometric value, so it gets the viewport's value box and a drag arrow
/// through the existing `canvas::anchor` machinery: along the sketch's own x by default, along
/// the picked direction once one is picked.
#[test]
fn the_spacing_gets_a_canvas_arrow_along_the_pattern_direction() {
    let (mut h, id) = open("sketch.pattern.rectangular");
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::SketchCurve { id }];
    let d = h.app.dialog.clone().unwrap();
    let (_, dir) = crate::canvas::anchor(&h.app, &d).expect("an anchor on the picked curve");
    assert_eq!(dir, Some(solidcraft_engine::geom::Vec3::X));
    h.app.dialog.as_mut().unwrap().inputs[1].items = vec![Sel::Axis { name: "Y".into() }];
    let d = h.app.dialog.clone().unwrap();
    let (_, dir) = crate::canvas::anchor(&h.app, &d).expect("an anchor on the picked curve");
    assert_eq!(dir, Some(solidcraft_engine::geom::Vec3::Y));
}

/// A sketch holding one circle 30 mm out and a lone point at the origin, with the circular
/// pattern window open on it and both inputs filled. The point is the centre the pattern turns
/// around; the command has no default for it. Returns the centre's bare point id.
fn open_circ() -> (crate::scenario::Harness, String) {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.run("sketch.point", json!({"point": [0, 0]})).unwrap();
    h.app.run("sketch.circle.center", json!({"center": [30, 0], "radius": 10})).unwrap();
    h.settle();
    let sid = h.app.session.active_sketch.unwrap();
    let (curve, centre) = {
        let st = h.app.session.model.state();
        let sk = &st.sketch(sid).unwrap().sketch;
        let curve = sk.curves[0].id.clone();
        let centre = sk.points.iter().find(|p| p.pos.len() < 1e-9).unwrap().id.clone();
        (curve, centre)
    };
    h.step(&json!({"start": "sketch.pattern.circular"})).unwrap();
    let d = h.app.dialog.as_mut().unwrap();
    d.inputs[0].items = vec![Sel::SketchCurve { id: curve }];
    d.inputs[1].items = vec![Sel::SketchPoint { id: format!("{sid}:{centre}") }];
    (h, centre)
}

fn circ(h: &mut crate::scenario::Harness, count: &str, angle: &str) {
    let d = h.app.dialog.as_mut().expect("the window opened");
    if let Kind::SketchModify(Mod::PatternCirc { count: c, angle: a, .. }) = &mut d.kind {
        *c = count.into();
        *a = angle.into();
    } else {
        panic!("not the circular pattern window: {:?}", d.kind);
    }
}

#[test]
fn circular_pattern_window_opens_instead_of_a_click_tool() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.settle();
    h.step(&json!({"start": "sketch.pattern.circular"})).unwrap();
    assert!(h.app.tool.is_none(), "the click tool still takes the command");
    let d = h.app.dialog.as_ref().expect("the window opened");
    assert!(matches!(d.kind, Kind::SketchModify(Mod::PatternCirc { .. })), "{:?}", d.kind);
    assert_eq!(d.inputs.iter().map(|i| i.label).collect::<Vec<_>>(), vec!["Entities", "Center"]);
}

#[test]
fn the_typed_count_and_total_angle_reach_the_command() {
    let (mut h, centre) = open_circ();
    circ(&mut h, "4", "180");
    let d = h.app.dialog.clone().unwrap();
    let cmds = apply_commands(&h.app, &d).unwrap();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].0, "sketch.pattern.circular");
    // The click flow could only ever send a count of 6, and never any angle.
    assert_eq!(cmds[0].1["count"], json!(4));
    assert_eq!(cmds[0].1["angle"], json!(180.0));
    assert_eq!(cmds[0].1["center"], json!(centre), "the bare point id the command resolves");
    circ(&mut h, "4", "90 deg");
    let d = h.app.dialog.clone().unwrap();
    assert_eq!(apply_commands(&h.app, &d).unwrap()[0].1["angle"], json!(90.0));
}

#[test]
fn circular_flip_and_center_buttons_apply_and_reopen_the_saved_placement() {
    use solidcraft_engine::sketch::{PatternAlignment, SketchPatternKind};
    let (mut h, _) = open_circ();
    circ(&mut h, "6", "90 deg");
    h.frames(3);
    h.step(&json!({"click":{"handle":"dialog:Alignment:Center"}})).unwrap();
    h.step(&json!({"click":{"handle":"dialog:Alignment:Flip direction"}})).unwrap();
    let d = h.app.dialog.clone().unwrap();
    let params = &apply_commands(&h.app, &d).unwrap()[0].1;
    assert_eq!(params["count"], json!(6));
    assert_eq!(params["angle"], json!(90.));
    assert_eq!(params["alignment"], json!("center"));
    assert_eq!(params["reversed"], json!(true));
    h.step(&json!({"click":{"handle":"dialog:OK"}})).unwrap();
    let sid = h.app.session.active_sketch.unwrap();
    let id = {
        let state = h.app.session.world_state();
        let pattern = &state.sketch(sid).unwrap().sketch.patterns[0];
        assert!(matches!(pattern.kind, SketchPatternKind::Circular { alignment: PatternAlignment::Center, reversed: true, .. }));
        pattern.id.clone()
    };
    assert!(crate::sketch_modify_ui::edit_pattern(&mut h.app, &id));
    assert!(matches!(
        h.app.dialog.as_ref().unwrap().kind,
        Kind::SketchModify(Mod::PatternCirc { alignment: PatternAlignment::Center, reversed: true, .. })
    ));
    h.frames(3);
    h.step(&json!({"click":{"handle":"dialog:Alignment:Center"}})).unwrap();
    let d = h.app.dialog.clone().unwrap();
    let params = &apply_commands(&h.app, &d).unwrap()[0].1;
    assert_eq!(params["alignment"], json!("right"));
    assert_eq!(params["reversed"], json!(true));
}

/// The same lesson as upstream #218: `num` reads a JSON number only, and `sketch.pattern.circular`
/// falls back to a full turn when `angle` is anything else. An expression has to be evaluated
/// here or it disappears in silence.
#[test]
fn an_angle_expression_is_evaluated_rather_than_sent_as_a_string() {
    let (mut h, _) = open_circ();
    h.app.run("parameters.add", json!({"name": "sweep", "expression": "120", "unit": "deg"})).unwrap();
    circ(&mut h, "3", "sweep");
    let d = h.app.dialog.clone().unwrap();
    let angle = apply_commands(&h.app, &d).unwrap()[0].1["angle"].as_f64().expect("a number, not the text `sweep`");
    assert!((angle - 120.0).abs() < 1e-9, "{angle}");
}

#[test]
fn a_missing_centre_a_short_count_and_a_bad_angle_are_all_refused() {
    let (mut h, _) = open_circ();
    for bad in ["0", "400 deg", "wat"] {
        circ(&mut h, "4", bad);
        let d = h.app.dialog.clone().unwrap();
        assert!(apply_commands(&h.app, &d).unwrap_err().contains("Total angle"), "angle {bad} was accepted");
    }
    circ(&mut h, "1", "360 deg");
    let d = h.app.dialog.clone().unwrap();
    assert!(apply_commands(&h.app, &d).unwrap_err().contains("Quantity"));
    circ(&mut h, "4", "360 deg");
    h.app.dialog.as_mut().unwrap().inputs[1].items.clear();
    let d = h.app.dialog.clone().unwrap();
    assert!(apply_commands(&h.app, &d).unwrap_err().contains("centre"));
}
