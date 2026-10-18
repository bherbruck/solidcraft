//! Keyboard-authored intent must survive stage transitions and later model edits.
use crate::scenario::Harness;
use serde_json::json;
use solidcraft_engine::sketch::{ConstraintKind, CurveKind};

fn start(command: &str) -> Harness {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.step(&json!({"start":command})).unwrap();
    h.step(&json!({"click":{"world":[0,0,0]}})).unwrap();
    h.step(&json!({"move":{"world":[20,0,0]}})).unwrap();
    h
}
fn type_value(h: &mut Harness, expression: &str) {
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":expression})).unwrap();
}
fn enter(h: &mut Harness) {
    h.step(&json!({"key":"Enter"})).unwrap();
}
fn constraints(h: &Harness) -> Vec<ConstraintKind> {
    h.app.session.world_state().sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch.constraints.iter().map(|c| c.kind.clone()).collect()
}

#[test]
fn typed_arc_radius_and_signed_sweep_survive_stages_parameter_edit_and_reopen() {
    let mut h = start("sketch.arc.center_point");
    h.app.run("parameters.add", json!({"name":"arc_radius","expression":"5 mm"})).unwrap();
    type_value(&mut h, "arc_radius");
    enter(&mut h);
    let tool = h.app.tool.as_ref().unwrap();
    assert_eq!(tool.pts.len(), 2);
    assert_eq!(tool.dim_history[0].value, "arc_radius");
    assert_eq!(tool.dims[0].label, "Sweep");
    type_value(&mut h, "-90 deg");
    enter(&mut h);
    assert!(h.app.tool.as_ref().unwrap().dim_history.is_empty());
    let kinds = constraints(&h);
    assert!(kinds.iter().any(|k| matches!(k, ConstraintKind::Radius{value,..} if (*value-5.0).abs()<1e-7)));
    assert!(kinds.iter().any(|k| matches!(k, ConstraintKind::ArcSweep{value,..} if (*value+std::f64::consts::FRAC_PI_2).abs()<1e-7)));
    h.app.run("parameters.change", json!({"name":"arc_radius","expression":"8 mm"})).unwrap();
    let path = h.dir.join("typed-arc.solidcraft");
    h.app.run("file.save", json!({"path":path})).unwrap();
    h.app.run("doc.open", json!({"path":path})).unwrap();
    let state = h.app.session.world_state();
    let sketch = &state.sketches[0].sketch;
    let curve = &sketch.curves[0];
    assert!(curve.reversed, "negative typed sweep must remain clockwise");
    let CurveKind::Arc { c, a, .. } = curve.kind else { panic!("expected arc") };
    assert!((sketch.points[c].pos.dist(sketch.points[a].pos) - 8.0).abs() < 1e-6);
    assert!(sketch.constraints.iter().any(|c| matches!(c.kind, ConstraintKind::ArcSweep{value,..} if value<0.0)));
}

#[test]
fn slots_keep_typed_length_and_width_as_dimensions_and_start_fresh() {
    for command in ["sketch.slot.center_to_center", "sketch.slot.overall"] {
        let mut h = start(command);
        type_value(&mut h, "30");
        enter(&mut h);
        assert_eq!(h.app.tool.as_ref().unwrap().dim_history[0].value, "30");
        h.step(&json!({"move":{"world":[15,5,0]}})).unwrap();
        type_value(&mut h, "6");
        enter(&mut h);
        let kinds = constraints(&h);
        assert!(kinds.iter().any(|k| matches!(k,ConstraintKind::Diameter{value,..} if (*value-6.0).abs()<1e-7)), "{command}");
        if command.ends_with("overall") {
            assert!(kinds.iter().any(|k| matches!(k,ConstraintKind::SlotOverallLength{value,..} if (*value-30.0).abs()<1e-7)));
        } else {
            assert!(kinds.iter().any(|k| matches!(k,ConstraintKind::Length{value,..} if (*value-30.0).abs()<1e-7)));
        }
        h.step(&json!({"click":{"world":[50,20,0]}})).unwrap();
        h.step(&json!({"move":{"world":[70,20,0]}})).unwrap();
        let tool = h.app.tool.as_ref().unwrap();
        assert!(tool.dim_history.is_empty());
        assert!(tool.dims.iter().all(|d| !d.locked));
        type_value(&mut h, "12");
        h.step(&json!({"key":"Escape"})).unwrap();
        assert!(h.app.tool.as_ref().unwrap().dim_history.is_empty());
        assert!(h.app.tool.as_ref().unwrap().pts.is_empty());
    }
}

#[test]
fn invalid_stage_values_remain_editable_without_partial_geometry() {
    let mut h = start("sketch.arc.center_point");
    type_value(&mut h, "-3");
    enter(&mut h);
    assert_eq!(h.app.tool.as_ref().unwrap().pts.len(), 1);
    assert_eq!(h.app.tool.as_ref().unwrap().dims[0].value, "-3");
    assert!(constraints(&h).is_empty());
    type_value(&mut h, "5");
    enter(&mut h);
    type_value(&mut h, "360 deg");
    enter(&mut h);
    assert_eq!(h.app.tool.as_ref().unwrap().pts.len(), 2);
    assert_eq!(h.app.tool.as_ref().unwrap().dims[0].value, "360 deg");
    assert!(constraints(&h).is_empty());
    type_value(&mut h, "90 deg");
    enter(&mut h);
    assert_eq!(constraints(&h).iter().filter(|c| c.is_dimension()).count(), 2);

    let mut slot = start("sketch.slot.overall");
    type_value(&mut slot, "4");
    enter(&mut slot);
    type_value(&mut slot, "6");
    enter(&mut slot);
    assert_eq!(slot.app.tool.as_ref().unwrap().pts.len(), 2);
    assert_eq!(slot.app.tool.as_ref().unwrap().dim_history[0].value, "4");
    assert_eq!(slot.app.tool.as_ref().unwrap().dims[0].value, "6");
    assert!(constraints(&slot).is_empty());
    type_value(&mut slot, "2");
    enter(&mut slot);
    assert_eq!(constraints(&slot).iter().filter(|c| c.is_dimension()).count(), 2);
}

#[test]
fn polygon_radius_is_persistent_and_fractional_sides_are_rejected() {
    let mut h = start("sketch.polygon.inscribed");
    type_value(&mut h, "10");
    h.step(&json!({"key":"Tab"})).unwrap();
    type_value(&mut h, "5.5");
    enter(&mut h);
    assert_eq!(h.app.tool.as_ref().unwrap().pts.len(), 1);
    assert!(constraints(&h).is_empty());
    type_value(&mut h, "5");
    enter(&mut h);
    assert!(constraints(&h).iter().any(|c| matches!(c,ConstraintKind::Radius{value,..} if (*value-10.0).abs()<1e-7)));
    let state = h.app.session.world_state();
    let sketch = &state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch;
    assert_eq!(sketch.curves.iter().filter(|c| !c.construction).count(), 5);
}

#[test]
fn line_angle_survives_drag_and_one_undo_removes_geometry_and_dimensions() {
    let mut h = start("sketch.line");
    let undo = h.app.session.undo.len();
    type_value(&mut h, "10");
    h.step(&json!({"key":"Tab"})).unwrap();
    type_value(&mut h, "30 deg");
    enter(&mut h);
    assert_eq!(h.app.session.undo.len(), undo + 1);
    assert!(h.app.tool.as_ref().unwrap().dims.iter().all(|d| !d.locked), "next chained segment starts without stale locks");
    h.step(&json!({"key":"Escape"})).unwrap();
    h.step(&json!({"key":"Escape"})).unwrap();
    h.step(&json!({"drag":[{"world":[8.660254037844386,5,0]},{"world":[16,14,0]}]})).unwrap();
    let state = h.app.session.world_state();
    let sketch = &state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch;
    let CurveKind::Line { a, b } = sketch.curves[0].kind else { panic!("expected line") };
    let vector = sketch.points[b].pos - sketch.points[a].pos;
    assert!((vector.len() - 10.0).abs() < 1e-6);
    assert!((vector.angle() - 30f64.to_radians()).abs() < 1e-6);
    // A drag may itself add an undo entry. Undo that before the original atomic draw.
    while h.app.session.undo.len() > undo + 1 {
        h.app.run("edit.undo", json!({})).unwrap();
    }
    h.app.run("edit.undo", json!({})).unwrap();
    assert!(constraints(&h).is_empty());
    assert!(h.app.session.world_state().sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch.curves.is_empty());
}

#[test]
fn typed_drawing_preserves_construction_and_centerline_modes_at_undo_limit() {
    for mode in [1, 2] {
        let mut h = start("sketch.line");
        crate::sketch_palette::set_mode(&mut h.app, mode);
        let snapshot = h.app.session.undo.last().unwrap().clone();
        h.app.session.undo.resize(200, snapshot);
        type_value(&mut h, "10");
        h.step(&json!({"key":"Tab"})).unwrap();
        type_value(&mut h, "30 deg");
        enter(&mut h);
        assert_eq!(h.app.session.undo.len(), 200);
        let state = h.app.session.world_state();
        let sketch = &state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch;
        assert_eq!(sketch.curves[0].construction, mode == 1);
        assert_eq!(sketch.curves[0].centerline, mode == 2);
        h.app.run("edit.undo", json!({})).unwrap();
        let state = h.app.session.world_state();
        let sketch = &state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch;
        assert!(sketch.curves.is_empty());
        assert!(sketch.constraints.is_empty());
        crate::sketch_palette::set_mode(&mut h.app, 0);
    }
}

/// The third pick of a Center Point Arc reaches existing geometry under the pointer: the value
/// box (the arc's sweep, a slot's width) stays clear of the cursor instead of sitting on it
/// (#2201).
#[test]
fn the_arc_end_pick_snaps_to_a_point_under_the_value_box() {
    for (command, picks, end) in [
        ("sketch.arc.center_point", [[0.0, 0.0], [10.0, 0.0]], [0.0, 10.0]),
        ("sketch.slot.center_to_center", [[-10.0, 0.0], [10.0, 0.0]], [0.0, 10.0]),
    ] {
        let mut h = Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.line", json!({"points":[[0, 0], end], "infer": false})).unwrap();
        h.app.run("sketch.construction", json!({"curves":["l1"]})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 60.0;
        h.app.cam.target = solidcraft_engine::geom::Vec3::new(0.0, 5.0, 0.0);
        h.settle();
        h.step(&json!({"start":command})).unwrap();
        for p in picks {
            h.step(&json!({"click":{"world":[p[0], p[1], 0]}})).unwrap();
        }
        h.step(&json!({"move":{"world":[end[0], end[1], 0]}})).unwrap();
        let cursor = h.at(&json!({"world":[end[0], end[1], 0]})).unwrap();
        let cursor = egui::pos2(cursor[0] as f32, cursor[1] as f32);
        let blocking = h.ctx.memory(|m| m.area_rect(egui::Id::new(("sc_dim_area", 0usize)))).filter(|r| r.expand(2.0).contains(cursor));
        assert!(blocking.is_none(), "{command}: the value box {blocking:?} sits under the cursor {cursor:?}");
        h.step(&json!({"click":{"world":[end[0], end[1], 0]}})).unwrap();
        // A slot's third pick sets its width only; the arc's end takes the point it lands on.
        if command != "sketch.arc.center_point" {
            continue;
        }
        let state = h.app.session.world_state();
        let sketch = &state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch;
        let line_end = sketch.curves.iter().find(|c| c.id == "l1").and_then(|c| match c.kind {
            CurveKind::Line { b, .. } => Some(b),
            _ => None,
        });
        let snapped =
            sketch.constraints.iter().any(|c| matches!(&c.kind, ConstraintKind::Coincident { p, q } if Some(*p) == line_end || Some(*q) == line_end))
                || sketch.curves.iter().filter(|c| c.id != "l1").any(|c| match c.kind {
                    CurveKind::Arc { a, b, .. } => Some(a) == line_end || Some(b) == line_end,
                    _ => false,
                });
        assert!(snapped, "{command}: the third pick snapped to the line's end: {:?}", sketch.constraints.iter().map(|c| &c.kind).collect::<Vec<_>>());
    }
}
