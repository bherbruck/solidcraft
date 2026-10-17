//! Exercise the actual feature editor, including frames rendered before OK.
use crate::scenario::Harness;
use serde_json::{Value, json};
use solidcraft_engine::doc::{Extent, FeatureKind};

fn make(params: Value) -> Harness {
    make_in(Harness::new(), params)
}
fn make_in(mut h: Harness, params: Value) -> Harness {
    h.app.run("sketch.create", json!({"plane":"XY","name":"Base"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[10,10]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    let mut p = json!({"sketch":"Base","distance":"10 mm","operation":"new","name":"Depth","body_name":"Main"});
    p.as_object_mut().unwrap().extend(params.as_object().unwrap().clone());
    h.app.run("solid.extrude", p).unwrap();
    h.frames(4);
    h
}
fn extent(h: &Harness) -> Extent {
    let FeatureKind::Extrude { extent, .. } = &h.app.session.doc.find_feature("Depth").unwrap().kind else { panic!("extrude") };
    extent.clone()
}
fn open(h: &mut Harness) {
    h.step(&json!({"call":"ui.editFeature","params":{"feature":"Depth"}})).unwrap();
    h.frames(8);
}

#[test]
fn unchanged_all_profiles_includes_profiles_added_after_edit() {
    let mut h = make(json!({}));
    open(&mut h);
    h.step(&json!({"widget":"OK"})).unwrap();
    let FeatureKind::Extrude { profiles, .. } = &h.app.session.doc.find_feature("Depth").unwrap().kind else { panic!("extrude") };
    assert!(matches!(profiles, solidcraft_engine::doc::ProfileSel::All));
    h.app.run("sketch.edit", json!({"sketch":"Base"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[20,0],"p1":[25,5]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    let state = h.app.session.world_state();
    let volume: f64 = state.bodies.iter().map(|b| solidcraft_engine::doc::kernel::measure(&b.body).unwrap().volume).sum();
    assert!((volume - 1250.0).abs() < 1e-7, "{volume}");
}

#[test]
fn unchanged_actual_editor_preserves_all_extents_and_parameter_identities() {
    for p in [
        json!({"direction":"negative","distance":"10"}),
        json!({"direction":"negative","distance":"-10 mm"}),
        json!({"direction":"symmetric","distance":"5 mm"}),
        json!({"direction":"symmetric","distance":"5 mm","convention":"legacy_per_side"}),
        json!({"distance":"10 mm","distance2":"3 mm"}),
        json!({"from":[0,0,5],"start_offset":"0 mm"}),
        json!({"to":[0,0,10],"to_offset":"3 mm"}),
        json!({"to":[0,0,-10],"to_offset":"-2 mm"}),
        json!({"taper":"0 deg"}),
    ] {
        let mut h = make(p.clone());
        let original = extent(&h);
        let names = h.app.session.doc.find_feature("Depth").unwrap().param_names.clone();
        let before = h.app.session.world_state().body("Main").unwrap().mesh().bounds();
        open(&mut h);
        h.step(&json!({"widget":"OK"})).unwrap();
        assert!(h.app.dialog.is_none(), "{p}: {:?}", h.app.session.log);
        assert_eq!(extent(&h), original, "{p}");
        assert_eq!(h.app.session.doc.find_feature("Depth").unwrap().param_names, names, "{p}");
        assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds(), before, "{p}");
    }
}

#[test]
fn negative_edit_keeps_downstream_distance_reference_positive() {
    let mut h = make(json!({"direction":"negative","distance":"10"}));
    h.app.run("solid.box", json!({"length":10,"width":10,"height":"d1","corner":[30,0,0],"body_name":"Dependent"})).unwrap();
    open(&mut h);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(h.app.session.doc.eval("d1", solidcraft_engine::doc::expr::Kind::Length).unwrap(), 10.0);
    assert_eq!(h.app.session.world_state().body("Dependent").unwrap().mesh().bounds().max.z, 10.0);
}

#[test]
fn cancel_after_canvas_edit_preserves_document_and_undo() {
    let mut h = make(json!({"from":[0,0,5],"to":[0,0,15],"to_offset":"2 mm"}));
    let original = extent(&h);
    let undo = h.app.session.undo.len();
    open(&mut h);
    assert_eq!(crate::canvas::anchor(&h.app, h.app.dialog.as_ref().unwrap()).unwrap().0.z, 5.0);
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":"4 mm"})).unwrap();
    h.frames(8);
    h.step(&json!({"widget":"Cancel"})).unwrap();
    assert_eq!(extent(&h), original);
    assert_eq!(h.app.session.undo.len(), undo);
}

#[test]
fn to_offset_canvas_edit_changes_offset_and_one_undo_restores_extent() {
    let mut h = make(json!({"from":[0,0,5],"to":[0,0,15],"to_offset":"2 mm"}));
    let original = extent(&h);
    open(&mut h);
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":"4 mm"})).unwrap();
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(extent(&h).to_offset.as_deref(), Some("4 mm"));
    assert_eq!(extent(&h).distance, original.distance);
    assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds().max.z, 19.0);
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(extent(&h), original);
}

#[test]
fn moved_owner_references_survive_actual_edit_with_another_component_active() {
    let mut h = Harness::new();
    let owner = h.app.run("component.create", json!({"name":"Owner"})).unwrap();
    let mut h = make_in(h, json!({"from":[0,0,5],"to":[0,0,15],"to_offset":"2 mm"}));
    h.app.run("occurrence.move", json!({"occurrence":owner["occurrence"],"translate":[100,0,20]})).unwrap();
    let other = h.app.run("component.create", json!({"name":"Other","parent":0})).unwrap();
    h.app.run("occurrence.move", json!({"occurrence":other["occurrence"],"translate":[200,0,40]})).unwrap();
    let original = extent(&h);
    let active = h.app.session.active_component;
    open(&mut h);
    let base = crate::canvas::anchor(&h.app, h.app.dialog.as_ref().unwrap()).unwrap().0;
    assert_eq!(base.z, 25.0);
    assert!(base.x >= 100.0 && base.x <= 110.0);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(extent(&h), original);
    assert_eq!(h.app.session.active_component, active);
    assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds().max.z, 37.0);
}

#[test]
fn dragged_to_object_handle_edits_offset_not_dormant_distance() {
    let mut h = make(json!({"from":[0,0,5],"to":[0,0,15],"to_offset":"2 mm"}));
    open(&mut h);
    let original = extent(&h);
    h.step(&json!({"drag":[{"handle":"arrow"},{"world":[5,5,21]}],"ctrl":true})).unwrap();
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.session.log);
    assert_eq!(extent(&h).distance, original.distance);
    let state = h.app.session.world_state();
    let z = state.body("Main").unwrap().mesh().bounds().max.z;
    assert!((z - 21.0).abs() < 0.1, "dragged end {z}");
}

#[test]
fn referenced_offset_recomputes_after_edit_and_save_reopen() {
    let mut h = Harness::new();
    h.app.run("parameters.add", json!({"name":"clearance","expression":"2 mm"})).unwrap();
    let mut h = make_in(h, json!({"to":[0,0,15],"to_offset":"clearance"}));
    open(&mut h);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(extent(&h).to_offset.as_deref(), Some("clearance"));
    h.app.run("parameters.change", json!({"name":"clearance","expression":"6 mm"})).unwrap();
    assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds().max.z, 21.0);
    let path = h.dir.join("extent.solidcraft");
    h.app.run("file.save", json!({"path":path})).unwrap();
    h.app.run("doc.open", json!({"path":path})).unwrap();
    assert_eq!(extent(&h).to_offset.as_deref(), Some("clearance"));
    assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds().max.z, 21.0);
}

#[test]
fn from_object_start_can_be_explicitly_changed_to_profile_plane() {
    let mut h = make(json!({"from":[0,0,5]}));
    open(&mut h);
    h.step(&json!({"click":{"handle":"dialog:Start"}})).unwrap();
    h.step(&json!({"click":{"handle":"dialog:Start:Profile Plane"}})).unwrap();
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(extent(&h).from.is_none());
    assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds().min.z, 0.0);
}

#[test]
fn explicit_reverse_keeps_original_expression_and_parameter_sign() {
    let mut h = make(json!({"distance":"10"}));
    open(&mut h);
    h.step(&json!({"widget":"Reverse Direction"})).unwrap();
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(extent(&h).distance, "10");
    assert_eq!(extent(&h).direction, solidcraft_engine::doc::Direction::Negative);
    assert_eq!(h.app.session.doc.eval("d1", solidcraft_engine::doc::expr::Kind::Length).unwrap(), 10.0);
    assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds().min.z, -10.0);
}

#[test]
fn failed_edit_and_cancel_keep_full_history_and_existing_redo() {
    let mut h = make(json!({}));
    for i in 0..205 {
        h.app.run("timeline.rename", json!({"feature":1,"name":format!("Base{i}")})).unwrap();
    }
    h.app.run("solid.box", json!({"length":2,"width":2,"height":2,"corner":[30,0,0],"body_name":"RedoBody"})).unwrap();
    h.app.run("edit.undo", json!({})).unwrap();
    let undo = h.app.session.undo.len();
    let redo = h.app.session.redo.len();
    assert_eq!(undo, 199);
    assert_eq!(redo, 1);
    h.app.session.selection = vec![solidcraft_engine::Sel::Body { name: "Main".into() }];
    let selection = h.app.session.selection.clone();
    let original = serde_json::to_value(&*h.app.session.doc).unwrap();
    open(&mut h);
    assert_eq!(h.app.session.undo.len(), undo);
    assert_eq!(h.app.session.redo.len(), redo);
    assert_eq!(h.app.session.selection, selection);
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":"missing_extent"})).unwrap();
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(h.app.dialog.is_some(), "failed edit must remain editable");
    h.step(&json!({"widget":"Cancel"})).unwrap();
    assert_eq!(serde_json::to_value(&*h.app.session.doc).unwrap(), original);
    assert_eq!(h.app.session.undo.len(), undo);
    assert_eq!(h.app.session.redo.len(), redo);
    assert_eq!(h.app.session.selection, selection);
    h.app.run("edit.redo", json!({})).unwrap();
    assert!(h.app.session.world_state().body("RedoBody").is_some());
}

#[test]
fn tapered_from_and_signed_to_have_correct_start_volume_and_preview_parity() {
    for (params, lo, hi) in [
        (json!({"from":[0,0,5],"distance":"10 mm","taper":"atan(0.1)"}), 5.0, 15.0),
        (json!({"from":[0,0,5],"to":[0,0,15],"direction":"negative","taper":"atan(0.1)"}), 5.0, 15.0),
        (json!({"from":[0,0,5],"to":[0,0,-5],"direction":"negative","taper":"atan(0.1)"}), -5.0, 5.0),
        (json!({"from":[0,0,5],"to":[0,0,-5],"direction":"positive","taper":"atan(0.1)"}), -5.0, 5.0),
    ] {
        let mut h = make(params.clone());
        let state = h.app.session.world_state();
        let body = state.body("Main").unwrap();
        let bounds = body.mesh().bounds();
        assert!((bounds.min.z - lo).abs() < 1e-8 && (bounds.max.z - hi).abs() < 1e-8, "{params}: {bounds:?}");
        let expected = 10.0 * (100.0 + 120.0 + 144.0) / 3.0;
        let volume = solidcraft_engine::doc::kernel::measure(&body.body).unwrap().volume;
        assert!((volume - expected).abs() < 1e-5, "{params}: volume{volume} expected{expected}");
        let original = extent(&h);
        open(&mut h);
        let commands = crate::dialogs::apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
        let preview = h.app.session.preview(&commands).unwrap();
        assert_eq!(preview.after.body("Main").unwrap().mesh().bounds(), bounds);
        h.step(&json!({"widget":"OK"})).unwrap();
        assert_eq!(extent(&h), original);
        assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds(), bounds);
    }
}

#[test]
fn adjacent_profiles_from_slanted_face_use_same_merged_centre_as_geometry() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length":30,"width":30,"height":2,"corner":[-10,-10,10],"body_name":"Reference"})).unwrap();
    h.app.run("solid.move", json!({"bodies":["Reference"],"axis":[0,1,0],"angle":"atan(0.1)","origin":[0,0,10]})).unwrap();
    h.app.run("sketch.create", json!({"plane":"XY","name":"Adjacent"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[2,10]})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[2,0],"p1":[10,10]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    let angle = 0.1f64.atan();
    let point = [2.0 * angle.sin(), 0.0, 10.0 + 2.0 * angle.cos()];
    h.app.run("solid.extrude", json!({"sketch":"Adjacent","distance":10,"from":point,"name":"Depth","body_name":"Main","operation":"new"})).unwrap();
    let min_z = h.app.session.world_state().body("Main").unwrap().mesh().bounds().min.z;
    open(&mut h);
    let base = crate::canvas::anchor(&h.app, h.app.dialog.as_ref().unwrap()).unwrap().0;
    assert!((base.z - min_z).abs() < 1e-7, "displayed start{} geometry start{min_z}", base.z);
    // Averaging the two unmerged rectangle loops gives x=3.5; their internal
    // seam must not bias the reference ray used by the extent handle.
    let unmerged_height = 10.0 + 2.0 / angle.cos() - 3.5 * angle.tan();
    assert!((base.z - unmerged_height).abs() > 0.01, "fixture must distinguish the raw-profile centre");
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!((h.app.session.world_state().body("Main").unwrap().mesh().bounds().min.z - min_z).abs() < 1e-7);
}

#[test]
fn rotated_second_occurrence_keeps_saved_from_and_to_points_exact() {
    let mut h = Harness::new();
    let owner = h.app.run("component.create", json!({"name":"Owner"})).unwrap();
    let mut h = make_in(h, json!({"from":[0,0,5],"to":[0,0,15],"to_offset":"2 mm"}));
    let second = h.app.run("occurrence.copy", json!({"component":owner["component"],"translate":[100,0,20]})).unwrap()["occurrence"].clone();
    h.app.run("occurrence.move", json!({"occurrence":second,"axis":[1,0,0],"angle":"17 deg"})).unwrap();
    h.app.run("component.activate", json!({"component":owner["component"],"occurrence":second,"path":[second]})).unwrap();
    let original = extent(&h);
    let before: Vec<_> = h.app.session.world_state().bodies.iter().map(|b| (b.name.clone(), b.mesh().bounds())).collect();
    open(&mut h);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(extent(&h), original);
    let after: Vec<_> = h.app.session.world_state().bodies.iter().map(|b| (b.name.clone(), b.mesh().bounds())).collect();
    assert_eq!(after, before);
}

#[test]
fn dragging_to_update_value_reselects_whole_value_box_text() {
    let mut h = make(json!({"distance":"4 mm"}));
    open(&mut h);
    let id = egui::Id::new("sc_canvas_value");
    let selection = |h: &Harness| {
        let range = egui::text_edit::TextEditState::load(&h.ctx, id).unwrap().cursor.char_range().unwrap();
        let (a, b) = (range.primary.index, range.secondary.index);
        (a.min(b).0, a.max(b).0)
    };
    assert_eq!(selection(&h), (0, 4), "the pick selects all of \"4 mm\"");
    h.step(&json!({"drag":[{"handle":"arrow"},{"world":[5,5,15]}]})).unwrap();
    let Some(crate::dialogs::Dialog { kind: crate::dialogs::Kind::Extrude { distance, .. }, .. }) = &h.app.dialog else { panic!("extrude dialog") };
    assert_eq!(distance, "15 mm");
    assert_eq!(selection(&h), (0, 5), "the drag grew the text length; the selection must cover all of it");
}

#[test]
fn symmetric_total_distance_preview_handle_drag_apply_and_reopen_agree() {
    for (convention, cap, total_after_drag) in [("total_distance", 50.0, 120.0), ("legacy_per_side", 100.0, 60.0)] {
        let mut h = make(json!({"direction":"symmetric","distance":"100 mm","convention":convention}));
        h.call("ui.view", json!({"view":"fit"}));
        h.frames(4);
        // Leave room above the fitted solid to grow the arrow without entering the ribbon.
        h.app.cam.distance *= 1.5;
        open(&mut h);
        let expected_tip = h.at(&json!({"world":[5,5,cap]})).unwrap();
        let handle = h.at(&json!({"handle":"arrow"})).unwrap();
        assert!((handle[0] - expected_tip[0]).hypot(handle[1] - expected_tip[1]) < 1.0, "{convention}: {handle:?} expected {expected_tip:?}");
        h.step(&json!({"drag":[{"handle":"arrow"},{"world":[5,5,60]}]})).unwrap();
        let d = h.app.dialog.as_ref().unwrap();
        let crate::dialogs::Kind::Extrude { distance, .. } = &d.kind else { panic!("extrude") };
        assert_eq!(h.app.session.doc.eval(distance, solidcraft_engine::doc::expr::Kind::Length).unwrap(), total_after_drag);
        let commands = crate::dialogs::apply_commands(&h.app, d).unwrap();
        let preview = h.app.session.preview(&commands).unwrap();
        let preview_bounds = preview.after.body("Main").unwrap().mesh().bounds();
        assert!((preview_bounds.min.z + 60.0).abs() < 1e-7 && (preview_bounds.max.z - 60.0).abs() < 1e-7);
        h.step(&json!({"widget":"OK"})).unwrap();
        assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds(), preview_bounds);
        let path = h.dir.join("symmetric.solidcraft");
        h.app.run("file.save_as", json!({"path":path})).unwrap();
        h.app.run("doc.open", json!({"path":path})).unwrap();
        assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds(), preview_bounds);
        open(&mut h);
        h.step(&json!({"widget":"OK"})).unwrap();
        assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds(), preview_bounds);
    }
}

#[test]
fn switching_one_side_to_symmetric_in_actual_editor_keeps_total_thickness() {
    for convention in ["total_distance", "legacy_per_side"] {
        let mut h = make(json!({"convention":convention}));
        open(&mut h);
        h.step(&json!({"widget":"One Side"})).unwrap();
        h.step(&json!({"widget":"Symmetric"})).unwrap();
        let commands = crate::dialogs::apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
        let preview = h.app.session.preview(&commands).unwrap();
        let bounds = preview.after.body("Main").unwrap().mesh().bounds();
        assert!((bounds.min.z + 5.0).abs() < 1e-7 && (bounds.max.z - 5.0).abs() < 1e-7);
        h.step(&json!({"widget":"OK"})).unwrap();
        assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds(), bounds);
        assert_eq!(extent(&h).convention, solidcraft_engine::doc::ExtrudeConvention::TotalDistance);
    }
}

#[test]
fn symmetric_through_all_handle_drag_switches_to_total_distance() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":100,"corner":[0,0,-50],"body_name":"Reference"})).unwrap();
    let mut h = make_in(h, json!({"direction":"symmetric","through_all":true}));
    h.call("ui.view", json!({"view":"fit"}));
    h.frames(4);
    open(&mut h);
    h.step(&json!({"drag":[{"handle":"arrow"},{"world":[5,5,60]}]})).unwrap();
    let d = h.app.dialog.as_ref().unwrap();
    let crate::dialogs::Kind::Extrude { distance, all, .. } = &d.kind else { panic!("extrude") };
    assert!(!all);
    assert_eq!(h.app.session.doc.eval(distance, solidcraft_engine::doc::expr::Kind::Length).unwrap(), 120.0);
    h.step(&json!({"widget":"OK"})).unwrap();
    let bounds = h.app.session.world_state().body("Main").unwrap().mesh().bounds();
    assert!((bounds.min.z + 60.0).abs() < 1e-7 && (bounds.max.z - 60.0).abs() < 1e-7);
}

/// Inspect each held frame, not just the settled end of the drag: the displayed arrow must
/// be at the distance the pointer just wrote, rather than the previous frame's distance.
#[test]
fn extrusion_arrow_tracks_the_current_drag_frame() {
    let mut h = make(json!({"distance":10}));
    open(&mut h);
    let at = h.at(&json!({"handle":"arrow"})).unwrap();
    let at = egui::pos2(at[0] as f32, at[1] as f32);
    h.app.synthetic_mods.ctrl = true;
    h.app.synthetic.push(egui::Event::PointerMoved(at));
    h.frame();
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: h.app.synthetic_mods,
    });
    h.frame();
    for dy in [-20.0, -40.0, -60.0] {
        h.app.synthetic.push(egui::Event::PointerMoved(at + egui::vec2(0.0, dy)));
        h.frame();
        let d = h.app.dialog.as_ref().unwrap();
        let crate::dialogs::Kind::Extrude { distance, .. } = &d.kind else { panic!("extrude") };
        let distance = h.app.session.doc.eval(distance, solidcraft_engine::doc::expr::Kind::Length).unwrap();
        assert!(distance > 10.0);
        let (base, normal) = crate::canvas::anchor(&h.app, d).unwrap();
        let world = base + normal.unwrap() * distance;
        let expected = h.at(&json!({"world":[world.x, world.y, world.z]})).unwrap();
        let drawn = h.at(&json!({"handle":"arrow"})).unwrap();
        assert!((drawn[0] - expected[0]).hypot(drawn[1] - expected[1]) < 0.1, "arrow {drawn:?} trails current value {expected:?}");
    }
}

/// The world point `at` mm along the open extrude's arrow.
fn on_arrow(h: &Harness, at: f64) -> Value {
    let d = h.app.dialog.as_ref().unwrap();
    let (base, normal) = crate::canvas::anchor(&h.app, d).unwrap();
    let p = base + normal.unwrap() * at;
    json!({"world":[p.x, p.y, p.z]})
}

fn dialog_distance(h: &Harness) -> f64 {
    let d = h.app.dialog.as_ref().unwrap();
    let crate::dialogs::Kind::Extrude { distance, .. } = &d.kind else { panic!("extrude") };
    h.app.session.doc.eval(distance, solidcraft_engine::doc::expr::Kind::Length).unwrap()
}

/// The arrow's stem takes the drag as its tip does (#1740): pressed 4 mm along a 10 mm arrow and
/// moved to 9 mm, it grows by the 5 mm the pointer moved, not to where the pointer is.
#[test]
fn the_extrude_arrow_drags_from_its_stem() {
    let mut h = make(json!({"distance":10}));
    open(&mut h);
    let (from, to) = (on_arrow(&h, 4.0), on_arrow(&h, 9.0));
    h.step(&json!({"drag":[from, to]})).unwrap();
    assert!((dialog_distance(&h) - 15.0).abs() < 1e-9, "{}", dialog_distance(&h));
    h.step(&json!({"widget":"OK"})).unwrap();
    let bounds = h.app.session.world_state().body("Main").unwrap().mesh().bounds();
    assert!((bounds.max.z - 15.0).abs() < 1e-7, "{bounds:?}");
}

/// A click on the arrow's stem is the arrow's (#1740): a new extrude of a square sketched on a
/// block keeps its profile picked and its distance. Before, the click went through to the square
/// under the stem and unpicked it.
#[test]
fn a_click_on_the_extrude_arrow_stem_keeps_the_profile() {
    let mut h = Harness::new();
    h.app.home.open = false;
    for (id, p) in [
        ("sketch.create", json!({"plane": "XY"})),
        ("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [60, 20]})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance": 10})),
        ("sketch.create", json!({"plane": {"origin": [0, 0, 10], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}})),
        ("sketch.rectangle.two_point", json!({"p0": [5, 5], "p1": [15, 15]})),
    ] {
        h.app.run(id, p).unwrap();
    }
    h.app.finish_sketch();
    h.app.start("solid.extrude");
    h.frames(4);
    let picked = |h: &Harness| h.app.dialog.as_ref().unwrap().inputs.iter().map(|i| i.items.len()).collect::<Vec<_>>();
    let before = picked(&h);
    assert!(before.iter().any(|n| *n > 0), "{before:?}");
    let distance = dialog_distance(&h);
    let at = on_arrow(&h, distance * 0.3);
    h.step(&json!({"click": at})).unwrap();
    h.frames(2);
    assert_eq!(picked(&h), before);
    assert!((dialog_distance(&h) - distance).abs() < 1e-9, "{} for {distance}", dialog_distance(&h));
}
