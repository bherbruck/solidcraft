use crate::scenario::Harness;
use serde_json::{Value, json};
use solidcraft_engine::doc::{AxisRef, FeatureKind};

fn make(h: Harness, axis: Value) -> Harness {
    make_params(h, axis, json!({}))
}
fn make_params(mut h: Harness, axis: Value, extra: Value) -> Harness {
    h.app.run("sketch.create", json!({"plane":"XY","name":"Profile"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[10,0],"p1":[15,5]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    let mut params = json!({"sketch":"Profile","axis":axis,"angle":"90 deg","operation":"new","name":"Turn","body_name":"Main"});
    params.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    h.app.run("solid.revolve", params).unwrap();
    h.frames(4);
    h
}

#[test]
fn unchanged_all_profiles_retains_future_geometry_but_reselection_is_explicit() {
    for reselect in [false, true] {
        let mut h = make(Harness::new(), json!("Y"));
        open(&mut h);
        if reselect {
            let selection = h.app.dialog.as_ref().unwrap().inputs[0].items[0].clone();
            let mut dialog = h.app.dialog.take().unwrap();
            dialog.active = 0;
            dialog.pick(&h.app.session, selection.clone());
            dialog.pick(&h.app.session, selection);
            h.app.dialog = Some(dialog);
            h.frames(4);
        }
        h.step(&json!({"widget":"OK"})).unwrap();
        let FeatureKind::Revolve { profiles, .. } = &h.app.session.doc.find_feature("Turn").unwrap().kind else { panic!("revolve") };
        assert_eq!(matches!(profiles, solidcraft_engine::doc::ProfileSel::All), !reselect);
        h.app.run("sketch.edit", json!({"sketch":"Profile"})).unwrap();
        h.app.run("sketch.rectangle.two_point", json!({"p0":[20,0],"p1":[25,5]})).unwrap();
        h.app.run("sketch.finish", json!({})).unwrap();
        let state = h.app.session.world_state();
        let volume: f64 = state.bodies.iter().map(|b| solidcraft_engine::doc::kernel::measure(&b.body).unwrap().volume).sum();
        let expected = if reselect { 490.873852 } else { 1374.446786 };
        assert!((volume - expected).abs() < 0.3, "{volume} != {expected}");
    }
}

#[test]
fn to_object_noop_preserves_cut_targets_and_untargeted_body() {
    let mut h = Harness::new();
    for name in ["A", "B"] {
        h.app.run("solid.box", json!({"length":20,"width":5,"height":20,"corner":[0,0,0],"body_name":name})).unwrap();
    }
    let mut h = make_params(h, json!("Y"), json!({"to":[0,2,12],"angle":"45 deg","operation":"cut","targets":["A"]}));
    let state = h.app.session.world_state();
    // A quarter annulus separates the stock into inner and outer solids. Both are
    // results of cutting A; checking only the first named solid loses material.
    let cut_volume = |state: &solidcraft_engine::doc::ModelState| -> f64 {
        state.bodies.iter().filter(|b| b.name != "B").map(|b| solidcraft_engine::doc::kernel::measure(&b.body).unwrap().volume).sum()
    };
    let before_a = cut_volume(&state);
    let removed = 0.5 * (15.0f64.powi(2) - 10.0f64.powi(2)) * 5.0 * std::f64::consts::FRAC_PI_2;
    assert!((before_a - (2000.0 - removed)).abs() < 0.2, "cut results {:?}: {before_a}", state.bodies.iter().map(|b| &b.name).collect::<Vec<_>>());
    let in_cut_stock = |p| state.bodies.iter().filter(|b| b.name != "B").any(|b| b.body.contains(p));
    assert!(in_cut_stock(solidcraft_engine::geom::Vec3::new(2.0, 2.0, 2.0)));
    assert!(in_cut_stock(solidcraft_engine::geom::Vec3::new(18.0, 2.0, 18.0)));
    assert!(!in_cut_stock(solidcraft_engine::geom::Vec3::new(9.0, 2.0, 9.0)));
    assert!(state.body("B").unwrap().body.contains(solidcraft_engine::geom::Vec3::new(9.0, 2.0, 9.0)));
    let before_bounds: Vec<_> = state.bodies.iter().map(|b| (b.name.clone(), b.mesh().bounds())).collect();
    let undo = h.app.session.undo.len();
    open(&mut h);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(h.app.dialog.is_none());
    let state = h.app.session.world_state();
    assert!((solidcraft_engine::doc::kernel::measure(&state.body("B").unwrap().body).unwrap().volume - 2000.0).abs() < 1e-7);
    assert!((cut_volume(&state) - before_a).abs() < 1e-7);
    assert_eq!(state.bodies.iter().map(|b| (b.name.clone(), b.mesh().bounds())).collect::<Vec<_>>(), before_bounds);
    assert_eq!(h.app.session.undo.len(), undo + 1);
    let FeatureKind::Revolve { targets, .. } = &h.app.session.doc.find_feature("Turn").unwrap().kind else { panic!("revolve") };
    assert_eq!(targets, &["A"]);
}

#[test]
fn to_object_noop_retains_dormant_secondary_angle_and_downstream_dimension() {
    let mut h = make_params(Harness::new(), json!("Y"), json!({"to":[0,2,12],"angle":"45 deg","angle2":"30 deg","direction":"symmetric"}));
    h.app.run("solid.box", json!({"length":2,"width":2,"height":"(d2 / (1 rad)) * 1 mm","corner":[30,0,0],"body_name":"Dependent"})).unwrap();
    let original = serde_json::to_value(&h.app.session.doc.find_feature("Turn").unwrap().kind).unwrap();
    let names = h.app.session.doc.find_feature("Turn").unwrap().param_names.clone();
    open(&mut h);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(serde_json::to_value(&h.app.session.doc.find_feature("Turn").unwrap().kind).unwrap(), original);
    assert_eq!(h.app.session.doc.find_feature("Turn").unwrap().param_names, names);
    assert!((h.app.session.world_state().body("Dependent").unwrap().mesh().bounds().max.z - std::f64::consts::PI / 6.0).abs() < 1e-8);
}

#[test]
fn rotated_owner_to_point_remains_exact_on_noop_edit() {
    let mut h = Harness::new();
    let owner = h.app.run("component.create", json!({"name":"Owner"})).unwrap();
    let mut h = make_params(h, json!({"origin":[0,0,0],"dir":[0,2,0]}), json!({"to":[0,2,12],"angle":"45 deg"}));
    h.app.run("occurrence.move", json!({"occurrence":owner["occurrence"],"translate":[100,0,20],"axis":[0,0,1],"angle":"17 deg"})).unwrap();
    h.app.run("component.create", json!({"name":"Other","parent":0})).unwrap();
    let original = serde_json::to_value(&h.app.session.doc.find_feature("Turn").unwrap().kind).unwrap();
    let bounds = h.app.session.world_state().body("Main").unwrap().mesh().bounds();
    open(&mut h);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(serde_json::to_value(&h.app.session.doc.find_feature("Turn").unwrap().kind).unwrap(), original);
    assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds(), bounds);
}

#[test]
fn second_active_occurrence_noop_preserves_definition_axis_target_and_instances() {
    let mut h = Harness::new();
    let owner = h.app.run("component.create", json!({"name":"Owner"})).unwrap();
    let mut h = make_params(h, json!({"origin":[0,0,0],"dir":[0,2,0]}), json!({"to":[0,2,12],"angle":"45 deg"}));
    let second = h.app.run("occurrence.copy", json!({"component":owner["component"],"translate":[100,0,20]})).unwrap()["occurrence"].clone();
    h.app.run("occurrence.move", json!({"occurrence":second,"axis":[0,0,1],"angle":"17 deg"})).unwrap();
    h.app.run("component.activate", json!({"component":owner["component"],"occurrence":second,"path":[second]})).unwrap();
    let original = serde_json::to_value(&h.app.session.doc.find_feature("Turn").unwrap().kind).unwrap();
    let frame = h.app.session.active_component_frame();
    let expected_origin = solidcraft_engine::doc::apply_point(&frame, solidcraft_engine::geom::Vec3::ZERO);
    let before: Vec<_> = h.app.session.world_state().bodies.iter().map(|b| (b.name.clone(), b.mesh().bounds())).collect();
    open(&mut h);
    let (origin, _) = crate::canvas::revolve_axis(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    assert!((origin - expected_origin).len() < 1e-8);
    assert!(origin.len() > 50.0, "fixture must distinguish first and second placements");
    let commands = crate::dialogs::apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    let preview = h.app.session.preview_world(&commands).unwrap();
    let placed = preview.after.bodies.iter().find(|body| body.mesh().bounds().center().x > 50.0).unwrap();
    assert!(placed.mesh().triangles.len() > 10);
    assert_eq!(placed.mesh().bounds(), before.iter().find(|(_, bounds)| bounds.center().x > 50.0).unwrap().1);
    assert!(h.widget("↖ Custom Axis").is_some());
    // In the floating shell the read-only Sweep badge overlaps OK at this placement.
    // It must not intercept the dialog's click or leave the timeline rolled back.
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "OK must apply even underneath the Sweep badge");
    assert_eq!(serde_json::to_value(&h.app.session.doc.find_feature("Turn").unwrap().kind).unwrap(), original);
    let after: Vec<_> = h.app.session.world_state().bodies.iter().map(|b| (b.name.clone(), b.mesh().bounds())).collect();
    assert_eq!(after, before);
    assert_eq!(h.app.session.active_occurrence_path.as_ref(), Some(&vec![second.as_u64().unwrap()]));
}
fn axis(h: &Harness) -> Value {
    let FeatureKind::Revolve { axis, .. } = &h.app.session.doc.find_feature("Turn").unwrap().kind else { panic!("revolve") };
    serde_json::to_value(axis).unwrap()
}
fn open(h: &mut Harness) {
    h.step(&json!({"call":"ui.editFeature","params":{"feature":"Turn"}})).unwrap();
    h.frames(8);
}

#[test]
fn actual_noop_editor_retains_all_revolve_axis_kinds_and_analytic_volume() {
    for axis_value in [json!("Y"), json!("y"), json!("l4"), json!({"origin":[0,0,0],"dir":[0,2,0]})] {
        let mut h = make(Harness::new(), axis_value.clone());
        let original = axis(&h);
        let names = h.app.session.doc.find_feature("Turn").unwrap().param_names.clone();
        let state = h.app.session.world_state();
        let body = state.body("Main").unwrap();
        let bounds = body.mesh().bounds();
        let radii = if axis_value == json!("l4") { 25.0 } else { 225.0 - 100.0 };
        let expected = 0.5 * radii * 5.0 * std::f64::consts::FRAC_PI_2;
        let volume = solidcraft_engine::doc::kernel::measure(&body.body).unwrap().volume;
        assert!((volume - expected).abs() < 0.03, "{axis_value}: {volume}");
        open(&mut h);
        let commands = crate::dialogs::apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
        let preview = h.app.session.preview(&commands).unwrap();
        assert_eq!(preview.after.body("Main").unwrap().mesh().bounds(), bounds);
        h.step(&json!({"widget":"OK"})).unwrap();
        assert!(h.app.dialog.is_none(), "{axis_value}: {:?}", h.app.session.log);
        assert_eq!(axis(&h), original);
        assert_eq!(h.app.session.doc.find_feature("Turn").unwrap().param_names, names);
        assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds(), bounds);
    }
}

#[test]
fn revolve_angle_edit_preserves_axis_downstream_binding_and_one_undo() {
    let mut h = make(Harness::new(), json!({"origin":[0,0,0],"dir":[0,2,0]}));
    h.app.run("solid.box", json!({"length":2,"width":2,"height":"(d1 / (1 rad)) * 1 mm","corner":[30,0,0],"body_name":"Dependent"})).unwrap();
    let original = axis(&h);
    let undo = h.app.session.undo.len();
    open(&mut h);
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":"180 deg"})).unwrap();
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(axis(&h), original);
    assert_eq!(h.app.session.undo.len(), undo + 1);
    assert!((h.app.session.world_state().body("Dependent").unwrap().mesh().bounds().max.z - std::f64::consts::PI).abs() < 1e-8);
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(axis(&h), original);
    assert!((h.app.session.world_state().body("Dependent").unwrap().mesh().bounds().max.z - std::f64::consts::FRAC_PI_2).abs() < 1e-8);
}

#[test]
fn rotated_owner_explicit_axis_is_lossless_with_another_component_active() {
    let mut h = Harness::new();
    let owner = h.app.run("component.create", json!({"name":"Owner"})).unwrap();
    let mut h = make(h, json!({"origin":[0,0,0],"dir":[0,2,0]}));
    h.app.run("occurrence.move", json!({"occurrence":owner["occurrence"],"translate":[100,0,20],"axis":[0,0,1],"angle":"17 deg"})).unwrap();
    h.app.run("component.create", json!({"name":"Other","parent":0})).unwrap();
    let active = h.app.session.active_component;
    let original = axis(&h);
    let bounds = h.app.session.world_state().body("Main").unwrap().mesh().bounds();
    open(&mut h);
    let (origin, direction) = crate::canvas::revolve_axis(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    assert!((origin.x - 100.0).abs() < 1e-8 && (origin.z - 20.0).abs() < 1e-8);
    let angle = 17.0f64.to_radians();
    assert!((direction.x + angle.sin()).abs() < 1e-8 && (direction.y - angle.cos()).abs() < 1e-8);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert_eq!(axis(&h), original);
    assert_eq!(h.app.session.active_component, active);
    assert_eq!(h.app.session.world_state().body("Main").unwrap().mesh().bounds(), bounds);
}

#[test]
fn world_and_sketch_axis_handles_follow_authoring_occurrence() {
    for selected in [json!("Y"), json!("y")] {
        let mut h = Harness::new();
        let owner = h.app.run("component.create", json!({"name":"Owner"})).unwrap();
        let mut h = make(h, selected);
        h.app.run("occurrence.move", json!({"occurrence":owner["occurrence"],"translate":[100,0,20],"axis":[0,0,1],"angle":"17 deg"})).unwrap();
        let original = axis(&h);
        open(&mut h);
        let (origin, direction) = crate::canvas::revolve_axis(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
        let angle = 17.0f64.to_radians();
        assert!((origin.x - 100.0).abs() < 1e-8 && (origin.z - 20.0).abs() < 1e-8);
        assert!((direction.x + angle.sin()).abs() < 1e-8 && (direction.y - angle.cos()).abs() < 1e-8);
        h.step(&json!({"widget":"OK"})).unwrap();
        assert_eq!(axis(&h), original);
    }
}

#[test]
fn sketch_axis_cancel_restores_original_document_and_history() {
    let mut h = make(Harness::new(), json!("y"));
    let original = serde_json::to_value(&*h.app.session.doc).unwrap();
    let undo = h.app.session.undo.len();
    open(&mut h);
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":"180 deg"})).unwrap();
    h.step(&json!({"widget":"Cancel"})).unwrap();
    assert_eq!(serde_json::to_value(&*h.app.session.doc).unwrap(), original);
    assert_eq!(h.app.session.undo.len(), undo);
    assert!(matches!(&h.app.session.doc.find_feature("Turn").unwrap().kind,FeatureKind::Revolve{axis:AxisRef::SketchAxis{axis},..} if axis=="y"));
}
