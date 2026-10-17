use super::*;
use solidcraft_engine::sketch::CurveKind;

#[test]
fn clicked_periodic_fit_closes_at_start_and_right_click_keeps_natural() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.settle();
    h.step(&json!({"start":"sketch.spline.fit_point"})).unwrap();
    for point in [[2, 2, 0], [8, 2, 0], [9, 7, 0], [1, 6, 0]] {
        h.step(&json!({"click":{"world":point}})).unwrap();
    }
    let sid = h.app.session.active_sketch.unwrap();
    assert!(h.app.session.model.state().sketch(sid).unwrap().sketch.curves.is_empty());
    h.step(&json!({"move":{"world":[2,2,0]}})).unwrap();
    assert_eq!(h.app.tool.as_ref().unwrap().hover.as_ref().unwrap().1.as_deref(), Some("Close smooth loop"));
    h.step(&json!({"click":{"world":[2,2,0]}})).unwrap();
    let sk = h.app.session.model.state().sketch(sid).unwrap().sketch.clone();
    assert_eq!(sk.curves.len(), 1, "{:?}", h.app.status);
    assert!(matches!(sk.curves[0].kind, CurveKind::Spline { periodic: true, .. }));
    sk.validate_periodic_splines().unwrap();
    assert_eq!(solidcraft_engine::sketch::find_profiles(&sk).len(), 1);
    assert!(h.app.tool.as_ref().unwrap().pts.is_empty());
    for point in [[15, 2, 0], [20, 2, 0], [20, 6, 0]] {
        h.step(&json!({"click":{"world":point}})).unwrap();
    }
    h.step(&json!({"click":{"world":[20,6,0]},"button":"right"})).unwrap();
    let sk = h.app.session.model.state().sketch(sid).unwrap().sketch.clone();
    assert_eq!(sk.curves.len(), 2);
    assert!(matches!(sk.curves[1].kind, CurveKind::Spline { periodic: false, .. }));
}

/// Curve ids in the open dialog's first selection input.
fn dialog_curves(h: &crate::scenario::Harness) -> Vec<String> {
    h.app
        .dialog
        .as_ref()
        .and_then(|d| d.inputs.first())
        .map(|i| i.items.iter().filter_map(|x| if let solidcraft_engine::Sel::SketchCurve { id } = x { Some(id.clone()) } else { None }).collect())
        .unwrap_or_default()
}

/// Type `value` into the open Offset window's Distance row, then apply with OK.
fn offset_distance(h: &mut crate::scenario::Harness, value: &str) {
    h.step(&json!({"click":{"handle":"dialog:Distance"}})).unwrap();
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":value})).unwrap();
    h.step(&json!({"widget":"OK"})).unwrap();
}

#[test]
fn offset_window_distance_and_flip_offset_an_ellipse_exactly_and_undo_restores() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.ellipse", json!({"center":[0,0],"major":[10,0],"minor_radius":5})).unwrap();
    h.settle();
    let sid = h.app.session.active_sketch.unwrap();
    h.step(&json!({"start":"sketch.offset"})).unwrap();
    // Major tip is also a source handle: Offset must choose the actual curve.
    h.step(&json!({"click":{"world":[10,0,0]}})).unwrap();
    assert_eq!(dialog_curves(&h), vec!["e1".to_string()], "{:?}", h.app.status);
    // Nothing is committed until the window is applied.
    assert!(h.app.session.model.state().sketch(sid).unwrap().sketch.freeform_offsets.is_empty());
    h.step(&json!({"click":{"handle":"dialog:Flip"}})).unwrap();
    offset_distance(&mut h, "1 mm");
    assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
    let sk = h.app.session.model.state().sketch(sid).unwrap().sketch.clone();
    assert_eq!(sk.freeform_offsets.len(), 1, "{:?}", h.app.status);
    assert!((sk.freeform_offsets[0].distance + 1.0).abs() < 1e-4, "{:?}", sk.freeform_offsets[0].distance);
    let first = sk.freeform_offsets[0].output.clone();
    let saved = h.app.session.doc.to_json();
    h.step(&json!({"start":"sketch.offset"})).unwrap();
    h.step(&json!({"click":{"world":[11,0,0]}})).unwrap();
    assert_eq!(dialog_curves(&h), vec![first], "{:?}", h.app.status);
    h.step(&json!({"click":{"handle":"dialog:Flip"}})).unwrap();
    offset_distance(&mut h, "0.5 mm");
    let sk = h.app.session.model.state().sketch(sid).unwrap().sketch.clone();
    assert_eq!(sk.freeform_offsets.len(), 2, "{:?}", h.app.status);
    assert!((sk.freeform_offsets[1].distance + 0.5).abs() < 1e-4, "{:?}", sk.freeform_offsets[1].distance);
    assert!(sk.freeform_offsets.iter().all(|r| r.certificate == 2));
    assert_eq!(solidcraft_engine::sketch::find_profiles(&sk).len(), 3);
    let last = sk.curve_index(&sk.freeform_offsets[1].output).unwrap();
    assert!(sk.exact_segs(last).unwrap()[0].point_at(0.0).dist(Vec2::new(11.5, 0.0)) < 1e-4);
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.doc.to_json(), saved);
}

#[test]
fn offset_window_without_a_curve_reports_it_and_keeps_the_window_open() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.ellipse", json!({"center":[0,0],"major":[10,0],"minor_radius":5})).unwrap();
    h.settle();
    let sid = h.app.session.active_sketch.unwrap();
    h.step(&json!({"start":"sketch.offset"})).unwrap();
    h.step(&json!({"widget":"OK"})).unwrap();
    let error = h.app.dialog.as_ref().and_then(|d| d.error.clone());
    assert_eq!(error.as_deref(), Some("Select the curves to offset first"));
    assert!(h.app.session.model.state().sketch(sid).unwrap().sketch.freeform_offsets.is_empty());
}

#[test]
fn clicked_extend_endpoint_reaches_exact_ellipse_and_preserves_support() {
    let mut h = crate::scenario::Harness::new();
    h.app.ui.show_origin = true;
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.ellipse", json!({"center":[20,0],"major":[30,0],"minor_radius":5})).unwrap();
    h.app.run("sketch.constraint.fix", json!({"entity":"e1"})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,3],[5,3]],"connect":false,"infer":false})).unwrap();
    h.frames(2);
    h.step(&json!({"start":"sketch.extend"})).unwrap();
    h.step(&json!({"click":{"world":[5,3,0]}})).unwrap();
    let st = h.app.session.world_state();
    let ss = st.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    let end = ss.sketch.point(ss.sketch.resolve_point("l1.end").unwrap()).unwrap();
    assert!(end.dist(Vec2::new(12.0, 3.0)) < 1e-7, "clicked endpoint must extend: {end:?}, {:?}", h.app.status);
    assert_eq!(ss.sketch.point(ss.sketch.resolve_point("l1.start").unwrap()), Some(Vec2::new(0.0, 3.0)));
}

#[test]
fn derived_offset_context_action_releases_geometry_and_undo_restores() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.ellipse", json!({"center":[0,0],"major":[10,0],"minor_radius":5})).unwrap();
    let out = h.app.run("sketch.offset", json!({"curves":["e1"],"distance":-1,"chain":false})).unwrap();
    let id = out["curves"][0].as_str().unwrap().to_string();
    h.app.session.selection = vec![solidcraft_engine::Sel::SketchCurve { id: id.clone() }];
    let items = crate::context_menu::items(&h.app, &crate::context_menu::Target::Viewport);
    let item = items.iter().find(|item| item.id == "sketch.offset.break").unwrap();
    assert_eq!(item.params, Some(json!({"curve":id})));
    crate::context_menu::run_item(&mut h.app, item, egui::Pos2::ZERO);
    let sid = h.app.session.active_sketch.unwrap();
    assert!(h.app.session.model.state().sketch(sid).unwrap().sketch.freeform_offsets.is_empty());
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.model.state().sketch(sid).unwrap().sketch.freeform_offsets.len(), 1);
}

/// The size fields of the open Fillet or Chamfer window, as if they had been typed.
fn set_size(app: &mut SolveApp, text: &str) {
    match app.dialog.as_mut().map(|d| &mut d.kind) {
        Some(crate::dialogs::Kind::SketchModify(crate::sketch_modify_ui::Mod::Corner { size, distance2, .. })) => {
            *size = text.to_string();
            *distance2 = text.to_string();
        }
        other => panic!("the corner window is not open: {other:?}"),
    }
}

/// Replace a dialog field's contents by typing, the way a person does.
fn retype(h: &mut crate::scenario::Harness, handle: &str, text: &str) {
    h.step(&json!({"click":{"handle":handle}})).unwrap();
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":text})).unwrap();
    h.frames(2);
}

fn apply(h: &mut crate::scenario::Harness) {
    // Lay the window out once before reading its OK handle. An error line that wraps onto a
    // second row moves the button, and a click on the handle from the frame before then
    // lands above it and commits nothing.
    h.frames(2);
    h.step(&json!({"click":{"handle":"dialog:OK"}})).unwrap();
    h.frames(2);
}

/// Is there a point of the active sketch at `at`?
fn has_point(h: &crate::scenario::Harness, at: Vec2) -> bool {
    let st = h.app.session.world_state();
    let Some(ss) = h.app.session.active_sketch.and_then(|sid| st.sketch(sid)) else { return false };
    ss.sketch.points.iter().any(|p| p.pos.dist(at) < 1e-6)
}

#[test]
fn corner_window_takes_the_corner_then_applies_the_typed_size() {
    for command in ["sketch.fillet", "sketch.chamfer.equal_distance", "sketch.chamfer.distance_angle", "sketch.chamfer.two_distance"] {
        let mut h = crate::scenario::Harness::new();
        h.app.ui.show_origin = true;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[20,30]})).unwrap();
        h.frames(2);
        let sid = h.app.session.active_sketch.unwrap();
        let proj = crate::viewport::projection(&h.app, h.app.viewport.rect.unwrap());
        let cursor = proj.to_screen(Vec3::ZERO).unwrap();
        // The ordinary snap still finds the fixed origin; the window must instead reach the
        // rectangle's overlapping endpoint.
        assert_eq!(sketch_point_at(&h.app, &proj, cursor).unwrap().1.as_deref(), Some("origin"));
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len());
        h.step(&json!({"start":command})).unwrap();
        h.frames(2);
        assert!(h.app.tool.is_none(), "{command} must open a window, not a click tool");
        assert!(h.app.dialog.is_some(), "{command} must open a window");
        h.step(&json!({"move":{"world":[0,0,0]}})).unwrap();
        let hover = h.app.viewport.hover.clone();
        let Some(crate::viewport::Hit::SketchPoint { id, .. }) = hover else {
            panic!("{command}: no corner under the cursor: {hover:?}");
        };
        assert_ne!(id, "origin", "the actual shared endpoint must be highlighted");
        h.step(&json!({"click":{"world":[0,0,0]}})).unwrap();
        h.frames(2);
        assert_eq!(h.app.dialog.as_ref().map(|d| d.inputs[0].items.len()), Some(1), "{command}: the corner belongs in the input");
        assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len()), before, "{command} must change nothing before OK");
        // 5 mm: the size the click flow used to guess and commit before asking.
        set_size(&mut h.app, "5");
        apply(&mut h);
        assert!(h.app.dialog.is_none(), "{command}: {:?}", h.app.status);
        assert_eq!(h.app.session.undo.len(), before.1 + 1, "{command} must be one undo entry");
        let st = h.app.session.world_state();
        let ss = st.sketch(sid).unwrap();
        assert_eq!(ss.sketch.point(0), Some(Vec2::ZERO));
        assert!(ss.sketch.points[0].fixed);
        assert_eq!(ss.sketch.curves.len(), 5, "{command} must actually modify the geometry; status: {:?}", h.app.status);
        if command == "sketch.fillet" {
            assert!(ss.sketch.curves.iter().any(|c| matches!(c.kind, CurveKind::Arc { .. })));
        } else {
            assert!(ss.sketch.curves.iter().all(|c| matches!(c.kind, CurveKind::Line { .. })));
        }
        let expected = if command == "sketch.fillet" { 600.0 - 25.0 * (1.0 - std::f64::consts::PI / 4.0) } else { 587.5 };
        assert_eq!(ss.profiles.len(), 1);
        assert!((ss.profiles[0].area - expected).abs() < 1e-6, "{command}: {}", ss.profiles[0].area);
        drop(st);
        h.app.run("edit.undo", json!({})).unwrap();
        let st = h.app.session.world_state();
        let ss = st.sketch(sid).unwrap();
        assert_eq!(ss.sketch.curves.len(), 4);
        assert!((ss.profiles[0].area - 600.0).abs() < 1e-6);
    }
}

/// The window's reason to exist: a chamfer angle other than the 45 degrees the command used to
/// receive hard-coded. 10 mm along the first line at 30 degrees puts the second cut at
/// 10 * sin 30 / sin 60 mm, where 45 degrees would put it at 10 mm.
#[test]
fn typed_chamfer_angle_other_than_45_degrees_reaches_the_command() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[40,30]})).unwrap();
    h.frames(2);
    h.step(&json!({"start":"sketch.chamfer.distance_angle"})).unwrap();
    h.frames(2);
    h.step(&json!({"click":{"world":[40,0,0]}})).unwrap();
    h.frames(2);
    retype(&mut h, "dialog:Distance 1", "10");
    retype(&mut h, "dialog:Angle", "30");
    apply(&mut h);
    assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
    let second = 10.0 / 3f64.sqrt();
    assert!(has_point(&h, Vec2::new(30.0, 0.0)), "10 mm along the first line");
    assert!(has_point(&h, Vec2::new(40.0, second)), "{second} mm along the second line, not 10 mm");
    assert!(!has_point(&h, Vec2::new(40.0, 10.0)), "45 degrees must no longer be forced");
    let st = h.app.session.world_state();
    let ss = st.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    assert!((ss.profiles[0].area - (1200.0 - 0.5 * 10.0 * second)).abs() < 1e-6, "{}", ss.profiles[0].area);
}

/// One window serves all four commands: the Chamfer Type row of icon buttons switches which
/// chamfer command OK runs, with no drop-down to open.
#[test]
fn the_chamfer_type_icons_switch_which_command_the_window_runs() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[40,30]})).unwrap();
    h.frames(2);
    h.step(&json!({"start":"sketch.chamfer.equal_distance"})).unwrap();
    h.frames(2);
    h.step(&json!({"click":{"world":[40,0,0]}})).unwrap();
    h.frames(2);
    // Every mode is a button on screen, not an item behind a drop-down.
    for label in ["Equal Distance", "Two Distances", "Distance and Angle"] {
        assert!(
            crate::scenario::handle_at(&format!("dialog:Chamfer Type:{label}")).is_some(),
            "{label} must be its own button on the Chamfer Type row"
        );
    }
    h.step(&json!({"click":{"handle":"dialog:Chamfer Type:Distance and Angle"}})).unwrap();
    h.frames(2);
    retype(&mut h, "dialog:Distance 1", "10");
    retype(&mut h, "dialog:Angle", "30");
    apply(&mut h);
    assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
    // The Distance and Angle command ran, not the Equal Distance one the window opened on.
    let second = 10.0 / 3f64.sqrt();
    assert!(has_point(&h, Vec2::new(30.0, 0.0)));
    assert!(has_point(&h, Vec2::new(40.0, second)));
    assert!(!has_point(&h, Vec2::new(40.0, 10.0)), "Equal Distance would have cut 10 mm off both legs");
}

/// Two Distances forced its second distance equal to the first. Both are typed now, and Flip
/// names the two lines in the other order so the first distance lands on the other leg.
#[test]
fn two_distance_chamfer_takes_both_distances_and_flip_swaps_the_legs() {
    for (flip, first, second) in [(false, Vec2::new(28.0, 0.0), Vec2::new(40.0, 4.0)), (true, Vec2::new(40.0, 12.0), Vec2::new(36.0, 0.0))] {
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[40,30]})).unwrap();
        h.frames(2);
        h.step(&json!({"start":"sketch.chamfer.two_distance"})).unwrap();
        h.frames(2);
        h.step(&json!({"click":{"world":[40,0,0]}})).unwrap();
        h.frames(2);
        retype(&mut h, "dialog:Distance 1", "12");
        retype(&mut h, "dialog:Distance 2", "4");
        if flip {
            h.step(&json!({"click":{"handle":"dialog:Flip"}})).unwrap();
            h.frames(2);
        }
        apply(&mut h);
        assert!(h.app.dialog.is_none(), "flip {flip}: {:?}", h.app.status);
        assert!(has_point(&h, first), "flip {flip}: the first distance belongs at {first:?}");
        assert!(has_point(&h, second), "flip {flip}: the second distance belongs at {second:?}");
    }
}

#[test]
fn escape_leaves_the_sketch_and_the_history_untouched() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[20,30]})).unwrap();
    h.frames(2);
    let before = (h.app.session.doc.to_json(), h.app.session.undo.len());
    h.step(&json!({"start":"sketch.fillet"})).unwrap();
    h.frames(2);
    h.step(&json!({"click":{"world":[0,0,0]}})).unwrap();
    h.frames(2);
    set_size(&mut h.app, "5");
    h.frames(2);
    h.step(&json!({"key":"Escape"})).unwrap();
    h.frames(2);
    assert!(h.app.dialog.is_none());
    assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len()), before);
}

/// A size the corner cannot take is refused with a reason, and the window stays open on it.
#[test]
fn an_impossible_size_keeps_the_window_open_with_its_reason() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[20,30]})).unwrap();
    h.frames(2);
    let before = (h.app.session.doc.to_json(), h.app.session.undo.len());
    h.step(&json!({"start":"sketch.chamfer.equal_distance"})).unwrap();
    h.frames(2);
    h.step(&json!({"click":{"world":[0,0,0]}})).unwrap();
    h.frames(2);
    for size in ["500", "0", "-3", "not a number"] {
        set_size(&mut h.app, size);
        apply(&mut h);
        assert!(h.app.dialog.as_ref().is_some_and(|d| d.error.is_some()), "{size} must be refused with a reason");
        assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len()), before, "{size} must change nothing");
    }
    set_size(&mut h.app, "4");
    apply(&mut h);
    assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
    assert_eq!(h.app.session.undo.len(), before.1 + 1);
}

/// A fillet rounds every picked corner to one radius, as one undo entry.
#[test]
fn a_fillet_rounds_every_picked_corner_to_one_radius() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[40,30]})).unwrap();
    h.frames(2);
    let undo = h.app.session.undo.len();
    h.step(&json!({"start":"sketch.fillet"})).unwrap();
    h.frames(2);
    for corner in [[40, 0, 0], [40, 30, 0], [0, 30, 0]] {
        h.step(&json!({"click":{"world":corner}})).unwrap();
        h.frames(2);
    }
    assert_eq!(h.app.dialog.as_ref().map(|d| d.inputs[0].items.len()), Some(3));
    set_size(&mut h.app, "5");
    apply(&mut h);
    assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
    assert_eq!(h.app.session.undo.len(), undo + 1, "one radius, one undo entry");
    let st = h.app.session.world_state();
    let ss = st.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    assert_eq!(ss.sketch.curves.iter().filter(|c| matches!(c.kind, CurveKind::Arc { .. })).count(), 3);
    assert!((ss.profiles[0].area - (1200.0 - 3.0 * 25.0 * (1.0 - std::f64::consts::PI / 4.0))).abs() < 1e-6, "{}", ss.profiles[0].area);
}

#[test]
fn fully_constrained_center_rectangle_corners_can_be_picked_and_filleted() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.rectangle.center", json!({"center":[0,0],"corner":[20,15]})).unwrap();
    h.app.run("sketch.constraint.coincident", json!({"a":"p5","b":"origin"})).unwrap();
    h.app.run("sketch.dimension", json!({"entities":["l1"],"value":40})).unwrap();
    h.app.run("sketch.dimension", json!({"entities":["l2"],"value":30})).unwrap();
    h.frames(2);
    h.step(&json!({"start":"sketch.fillet"})).unwrap();
    h.frames(2);
    for corner in [[-20, -15, 0], [20, -15, 0], [20, 15, 0], [-20, 15, 0]] {
        h.step(&json!({"click":{"world":corner}})).unwrap();
        h.frames(2);
    }
    assert_eq!(h.app.dialog.as_ref().map(|d| d.inputs[0].items.len()), Some(4));
    set_size(&mut h.app, "3");
    apply(&mut h);
    assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
    let info = h.app.run("sketch.inspect", json!({})).unwrap();
    assert_eq!(info["dof"], 0, "{info}");
    assert_eq!(info["status"], "solved", "{info}");
    let st = h.app.session.world_state();
    let ss = st.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    assert_eq!(ss.sketch.curves.iter().filter(|c| matches!(c.kind, CurveKind::Arc { .. })).count(), 4);
    assert!((ss.profiles[0].area - (1200.0 - 36.0 * (1.0 - std::f64::consts::PI / 4.0))).abs() < 1e-6);
}

#[test]
fn ordinary_drawing_keeps_origin_snap_and_real_origin_corners_work() {
    let mut h = crate::scenario::Harness::new();
    h.app.ui.show_origin = true;
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.step(&json!({"start":"sketch.line"})).unwrap();
    h.step(&json!({"move":{"world":[0,0,0]}})).unwrap();
    assert_eq!(h.app.tool.as_ref().unwrap().hover.as_ref().unwrap().1.as_deref(), Some("origin"));
    h.step(&json!({"click":{"world":[0,0,0]}})).unwrap();
    assert_eq!(h.app.tool.as_ref().unwrap().pts[0].1.as_deref(), Some("origin"));
    h.app.tool = None;
    h.app.run("sketch.line", json!({"points":["origin",[20,0],[20,20],[0,20],"origin"]})).unwrap();
    h.step(&json!({"start":"sketch.fillet"})).unwrap();
    h.frames(2);
    h.step(&json!({"click":{"world":[0,0,0]}})).unwrap();
    h.frames(2);
    set_size(&mut h.app, "5");
    apply(&mut h);
    assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
    let st = h.app.session.world_state();
    let ss = st.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    assert_eq!(ss.sketch.point(0), Some(Vec2::ZERO));
    assert!(ss.sketch.curves.iter().any(|c| matches!(c.kind, CurveKind::Arc { .. })));
    assert_eq!(ss.profiles.len(), 1);
}

/// #1824: where a third line ends at a corner's point, the point names no single corner; the
/// Fillet window takes the corner's two lines instead and rounds the corner between them.
#[test]
fn fillet_window_rounds_the_corner_between_two_picked_lines() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,0],[10,0]],"ids":["left"]})).unwrap();
    h.app.run("sketch.line", json!({"points":["left.end",[20,0]],"ids":["right"]})).unwrap();
    h.app.run("sketch.line", json!({"points":["left.end",[18,8]],"ids":["diag"]})).unwrap();
    h.settle();
    let sid = h.app.session.active_sketch.unwrap();
    h.step(&json!({"start":"sketch.fillet"})).unwrap();
    // The shared point ends three lines, so it names no single corner: pick the two lines.
    h.step(&json!({"click":{"world":[16,0,0]}})).unwrap();
    h.step(&json!({"click":{"world":[13,3,0]}})).unwrap();
    assert_eq!(dialog_curves(&h), vec!["right".to_string(), "diag".to_string()], "{:?}", h.app.status);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
    let sk = h.app.session.model.state().sketch(sid).unwrap().sketch.clone();
    assert_eq!(sk.curves.iter().filter(|c| matches!(c.kind, CurveKind::Arc { .. })).count(), 1, "{:?}", h.app.status);
}

/// #1824 (review): the Chamfer window takes a corner's two lines the same way.
#[test]
fn chamfer_window_cuts_the_corner_between_two_picked_lines() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,0],[10,0]],"ids":["left"]})).unwrap();
    h.app.run("sketch.line", json!({"points":["left.end",[20,0]],"ids":["right"]})).unwrap();
    h.app.run("sketch.line", json!({"points":["left.end",[18,8]],"ids":["diag"]})).unwrap();
    h.settle();
    let sid = h.app.session.active_sketch.unwrap();
    let before = h.app.session.model.state().sketch(sid).unwrap().sketch.curves.len();
    h.step(&json!({"start":"sketch.chamfer.equal_distance"})).unwrap();
    h.step(&json!({"click":{"world":[16,0,0]}})).unwrap();
    h.step(&json!({"click":{"world":[13,3,0]}})).unwrap();
    assert_eq!(dialog_curves(&h), vec!["right".to_string(), "diag".to_string()], "{:?}", h.app.status);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.status);
    let sk = h.app.session.model.state().sketch(sid).unwrap().sketch.clone();
    assert_eq!(sk.curves.len(), before + 1, "a chamfer line: {:?}", h.app.status);
    assert!(sk.curve_index("left").is_some(), "the third line stays");
}
