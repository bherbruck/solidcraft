// Exact occurrence workflows through the same commands used by the Browser.
use crate::Session;
use serde_json::{Value, json};

fn run(s: &mut Session, command: &str, p: Value) -> Value {
    s.execute(command, &p).unwrap()
}
fn created(s: &mut Session, name: &str) -> (u64, u64) {
    let c = run(s, "component.create", json!({"name": name}));
    (c["component"].as_u64().unwrap(), c["occurrence"].as_u64().unwrap())
}
fn near(value: &Value, expected: f64) {
    assert!((value.as_f64().unwrap() - expected).abs() < 1e-6, "{value} != {expected}");
}

#[test]
fn instance_copy_uses_selected_source_transform() {
    let mut s = Session::default();
    let (component, first) = created(&mut s, "Block");
    let second = run(&mut s, "occurrence.copy", json!({"component": component, "translate": [20,0,0]}))["occurrence"].as_u64().unwrap();
    run(&mut s, "occurrence.move", json!({"occurrence": second, "translate": [30,0,0]}));
    let third = run(&mut s, "occurrence.copy", json!({"occurrence": second, "translate": [5,0,0]}))["occurrence"].as_u64().unwrap();
    let placement = s.doc.occurrences.iter().find(|o| o.id == third).unwrap();
    assert!((placement.transform[3][0] - 55.0).abs() < 1e-6);
    assert_eq!(s.doc.occurrences.iter().find(|o| o.id == first).unwrap().transform[3][0], 0.0);
}

#[test]
fn instance_world_move_under_rotated_parent_uses_selected_path() {
    let mut s = Session::default();
    let (_, parent) = created(&mut s, "Parent");
    run(&mut s, "occurrence.move", json!({"occurrence": parent, "axis": [0,0,1], "angle": "90 deg"}));
    let (_, child) = created(&mut s, "Child");
    run(&mut s, "solid.box", json!({"length": 1, "width": 1, "height": 1}));
    run(&mut s, "occurrence.move", json!({"occurrence": child, "path": [parent, child], "space": "world", "translate": [10,0,0]}));
    let result = run(&mut s, "occurrence.properties", json!({"occurrence": child, "path": [parent,child]}));
    near(&result["total"]["center_of_mass"][0], 9.5);
    near(&result["total"]["center_of_mass"][1], 0.5);
}

#[test]
fn instance_authoring_at_second_placement_keeps_definition_shared() {
    let mut s = Session::default();
    let (component, first) = created(&mut s, "Block");
    let second = run(&mut s, "occurrence.copy", json!({"component": component, "translate": [20,0,0]}))["occurrence"].as_u64().unwrap();
    run(&mut s, "component.activate", json!({"component": component, "occurrence": second, "path": [second]}));
    run(&mut s, "solid.box", json!({"length": 2, "width": 2, "height": 2, "corner": [20,0,0], "operation": "new"}));
    for (occurrence, x) in [(first, 1.0), (second, 21.0)] {
        let result = run(&mut s, "occurrence.properties", json!({"occurrence": occurrence, "path": [occurrence]}));
        near(&result["total"]["center_of_mass"][0], x);
    }
}

#[test]
fn instance_selection_survives_rename_move_capture_and_serde() {
    let mut s = Session::default();
    let (component, _) = created(&mut s, "Block");
    let second = run(&mut s, "occurrence.copy", json!({"component": component, "translate": [20,0,0]}))["occurrence"].as_u64().unwrap();
    let selected = json!({"type": "occurrence", "path": [second]});
    run(&mut s, "select.set", json!({"items": [selected]}));
    run(&mut s, "component.rename", json!({"component": component, "name": "Renamed"}));
    run(&mut s, "occurrence.move", json!({"occurrence": second, "translate": [5,0,0], "capture": false}));
    run(&mut s, "component.capture_position", json!({}));
    assert_eq!(serde_json::to_value(&s.selection).unwrap(), json!([selected]));
    let roundtrip: Vec<crate::Sel> = serde_json::from_value(json!([selected])).unwrap();
    assert_eq!(roundtrip, s.selection);
}

#[test]
fn component_rename_carries_into_placement_names() {
    // #1665: the browser row shows the placement's name, so a rename has to reach it.
    let mut s = Session::default();
    let (component, first) = created(&mut s, "Block");
    let second = run(&mut s, "occurrence.copy", json!({"component": component, "translate": [20,0,0]}))["occurrence"].as_u64().unwrap();
    let third = run(&mut s, "occurrence.copy", json!({"component": component, "translate": [40,0,0]}))["occurrence"].as_u64().unwrap();
    let name = |s: &Session, id: u64| s.doc.occurrences.iter().find(|o| o.id == id).unwrap().name.clone();
    assert_eq!(name(&s, second), "Block:2");
    // A placement named otherwise (an import's own name) keeps it.
    s.doc_mut().occurrences.iter_mut().find(|o| o.id == third).unwrap().name = "Blockade:3".into();
    run(&mut s, "component.rename", json!({"component": component, "name": "box"}));
    assert_eq!([name(&s, first), name(&s, second), name(&s, third)], ["box:1", "box:2", "Blockade:3"]);
    assert_eq!(run(&mut s, "component.list", json!({}))["components"][0]["children"][0]["name"], "box:1");
}

#[test]
fn instance_selection_delete_removes_only_selected_instance() {
    let mut s = Session::default();
    let (component, first) = created(&mut s, "Block");
    run(&mut s, "solid.box", json!({"length": 2, "width": 2, "height": 2}));
    let second = run(&mut s, "occurrence.copy", json!({"component": component, "translate": [20,0,0]}))["occurrence"].as_u64().unwrap();
    run(&mut s, "select.set", json!({"items": [{"type": "occurrence", "path": [second]}]}));
    let items = s.selection.clone();
    run(&mut s, "selection.delete", json!({"items": items}));
    assert!(s.doc.occurrences.iter().any(|o| o.id == first));
    assert!(!s.doc.occurrences.iter().any(|o| o.id == second));
    assert_eq!(s.world_state().bodies.len(), 1);
    assert!(s.doc.components.iter().any(|c| c.id == component));
}

#[test]
fn world_move_includes_pending_parent_and_leaf_and_shared_scope() {
    let mut s = Session::default();
    let (parent_component, parent) = created(&mut s, "Parent");
    let (_, child) = created(&mut s, "Child");
    run(&mut s, "occurrence.move", json!({"occurrence":parent,"axis":[0,0,1],"angle":"90 deg","translate":[3,0,0],"capture":false}));
    run(&mut s, "occurrence.move", json!({"occurrence":child,"translate":[2,0,0],"capture":false}));
    let before = s.occurrence_placement(&[parent, child]).unwrap().world;
    run(&mut s, "occurrence.move", json!({"occurrence":child,"path":[parent,child],"space":"world","translate":[10,0,0],"capture":false}));
    let after = s.occurrence_placement(&[parent, child]).unwrap().world;
    assert!((after[3][0] - before[3][0] - 10.0).abs() < 1e-8);
    assert!((after[3][1] - before[3][1]).abs() < 1e-8);
    run(
        &mut s,
        "occurrence.move",
        json!({"occurrence":child,"path":[parent,child],"space":"world","axis":[0,0,1],"angle":"90 deg","origin":[0,0,0],"capture":false}),
    );
    let rotated = s.occurrence_placement(&[parent, child]).unwrap().world;
    assert!((rotated[3][0] + after[3][1]).abs() < 1e-8);
    assert!((rotated[3][1] - after[3][0]).abs() < 1e-8);
    run(&mut s, "component.activate", json!({"component":parent_component}));
    let second_parent = run(&mut s, "occurrence.copy", json!({"occurrence":parent,"translate":[30,0,0]}))["occurrence"].as_u64().unwrap();
    assert_eq!(s.occurrence_scope_count(&[second_parent, child]).unwrap(), 2);
    assert!(s.execute("occurrence.move", &json!({"occurrence":child,"space":"world","translate":[1,0,0]})).is_err());
}

#[test]
fn invalid_paths_are_atomic_for_session_state() {
    let mut s = Session::default();
    let (component, first) = created(&mut s, "Part");
    let second = run(&mut s, "occurrence.copy", json!({"occurrence":first}))["occurrence"].as_u64().unwrap();
    run(&mut s, "component.activate", json!({"component":component,"occurrence":second,"path":[second]}));
    run(&mut s, "occurrence.move", json!({"occurrence":second,"translate":[5,0,0],"capture":false}));
    run(&mut s, "browser.visibility", json!({"visible":false,"paths":[[second]]}));
    let pending = s.pending_moves.clone();
    let active = s.active_occurrence_path.clone();
    let vis = s.visibility.clone();
    let doc = s.doc.clone();
    for (cmd, p) in [
        ("occurrence.move", json!({"occurrence":second,"path":[first],"space":"world","translate":[10,0,0]})),
        ("occurrence.move", json!({"occurrence":second,"space":17,"translate":[10,0,0]})),
        ("component.activate", json!({"component":component,"occurrence":second,"path":[first,second]})),
        ("browser.visibility", json!({"visible":true,"paths":[[second],[9999]]})),
        ("select.set", json!({"items":[{"type":"occurrence","path":[first,second]}]})),
        ("occurrence.copy", json!({"occurrence":second,"path":[first]})),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd}");
        assert_eq!(s.pending_moves, pending);
        assert_eq!(s.active_occurrence_path, active);
        assert_eq!(s.visibility, vis);
        assert_eq!(s.doc, doc);
    }
}

#[test]
fn exact_visibility_isolate_rename_and_async_accept() {
    let mut s = Session::default();
    let (component, first) = created(&mut s, "Part");
    run(&mut s, "solid.box", json!({"length":2,"width":2,"height":2}));
    let second = run(&mut s, "occurrence.copy", json!({"occurrence":first,"translate":[20,0,0]}))["occurrence"].as_u64().unwrap();
    let first_body = s.occurrence_bodies(&[first], true).unwrap()[0].clone();
    let second_body = s.occurrence_bodies(&[second], true).unwrap()[0].clone();
    run(&mut s, "browser.visibility", json!({"visible":false,"paths":[[second]]}));
    assert!(s.body_visible(&first_body));
    assert!(!s.body_visible(&second_body));
    let mut scratch = s.scratch();
    assert_eq!(scratch.visibility, s.visibility);
    run(&mut scratch, "component.rename", json!({"component":component,"name":"Changed"}));
    s.accept_computed(scratch);
    assert!(!s.occurrence_visible(&[second]));
    run(&mut s, "browser.visibility", json!({"visible":true,"paths":[[second]],"isolate":true}));
    assert!(!s.body_visible(&first_body));
    assert!(s.body_visible(&second_body));
    run(&mut s, "browser.visibility", json!({"visible":true,"folder":"bodies"}));
    assert!(s.body_visible(&first_body));
    assert!(s.body_visible(&second_body));
}

#[test]
fn synthesized_name_collision_keeps_source_and_instance_separate() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length":1,"width":1,"height":1}));
    let root = s.model.state().bodies[0].name.clone();
    let (_, first) = created(&mut s, "Part");
    run(&mut s, "solid.box", json!({"length":2,"width":2,"height":2}));
    let second = run(&mut s, "occurrence.copy", json!({"occurrence":first,"translate":[20,0,0]}))["occurrence"].as_u64().unwrap();
    let candidate = s.occurrence_bodies(&[second], false).unwrap()[0].clone();
    run(&mut s, "body.rename", json!({"body":root,"name":candidate}));
    let instance = s.occurrence_bodies(&[second], false).unwrap()[0].clone();
    assert_ne!(instance, candidate);
    let frames = s.doc.body_frames(&s.model.state());
    let world = s.world_state();
    assert!(frames.iter().all(|(name, _, _, _)| world.body(name).is_some()));
    assert_eq!(world.bodies.len(), world.bodies.iter().map(|b| &b.name).collect::<std::collections::BTreeSet<_>>().len());
    run(&mut s, "browser.visibility", json!({"visible":false,"paths":[[second]]}));
    assert!(s.body_visible(&candidate));
    assert!(!s.body_visible(&instance));
    let source = s.occurrence_placement(&[second]).unwrap().bodies[0].source.clone();
    run(&mut s, "material.assign", json!({"bodies":[instance],"material":"Brass"}));
    assert_eq!(s.doc.materials.get(&source).map(String::as_str), Some("Brass"));
    assert!(!s.doc.materials.contains_key(&candidate));
}

#[test]
fn duplicate_nested_labels_use_full_path_and_reopen_undo_preserve_placements() {
    let mut s = Session::default();
    let (parent_component, parent) = created(&mut s, "Parent");
    let (_, child) = created(&mut s, "Child");
    run(&mut s, "solid.box", json!({"length":1,"width":1,"height":1}));
    let parent2 = s.doc_mut().add_occurrence(parent_component, 0, solidcraft_doc::IDENTITY).unwrap();
    let parent3 = s.doc_mut().add_occurrence(parent_component, 0, solidcraft_doc::IDENTITY).unwrap();
    for o in s.doc_mut().occurrences.iter_mut().filter(|o| o.id == parent2 || o.id == parent3) {
        o.name = "Same".into();
    }
    s.refresh();
    let a = s.occurrence_bodies(&[parent2, child], false).unwrap()[0].clone();
    let b = s.occurrence_bodies(&[parent3, child], false).unwrap()[0].clone();
    assert_ne!(a, b);
    let mut reopened = Session::new(solidcraft_doc::format::read(&s.doc.to_json()).unwrap());
    assert_eq!(reopened.occurrence_bodies(&[parent2, child], false).unwrap()[0], a);
    assert_eq!(reopened.occurrence_bodies(&[parent3, child], false).unwrap()[0], b);
    run(&mut reopened, "selection.delete", json!({"occurrences":[parent3]}));
    assert!(reopened.occurrence_placement(&[parent3, child]).is_err());
    run(&mut reopened, "edit.undo", json!({}));
    assert!(reopened.occurrence_placement(&[parent3, child]).is_ok());
    assert!(reopened.occurrence_placement(&[parent, child]).is_ok());
}

#[test]
fn compact_duplicated_assembly_exact_resolution_and_expansion_limit() {
    let mut s = Session::default();
    let mut parent = 0;
    let mut path = Vec::new();
    for i in 0..18 {
        let component = s.doc_mut().add_component(Some(&format!("Level{i}")), parent).unwrap();
        let occurrence = s.doc.occurrence_of(component).unwrap().id;
        path.push(occurrence);
        s.doc_mut().add_occurrence(component, parent, solidcraft_doc::IDENTITY).unwrap();
        parent = component;
    }
    assert!(s.occurrence_placement(&path).is_ok());
    let top = [path[0]];
    assert!(s.occurrence_bodies(&top, true).is_err());
    assert!(s.execute("select.set", &json!({"items":[{"type":"occurrence","path":top}]})).is_err());
    assert!(s.selection.is_empty());
}

#[test]
fn identity_instances_map_world_keys_to_shared_sources() {
    let mut s = Session::default();
    let (_, first) = created(&mut s, "Part");
    run(&mut s, "solid.box", json!({"length":2,"width":2,"height":2}));
    let second = run(&mut s, "occurrence.copy", json!({"occurrence":first}))["occurrence"].as_u64().unwrap();
    let placement = s.occurrence_placement(&[second]).unwrap();
    let body = &placement.bodies[0];
    assert_ne!(body.source, body.world);
    run(&mut s, "material.assign", json!({"bodies":[body.world],"material":"Brass"}));
    assert_eq!(s.doc.materials.get(&body.source).map(String::as_str), Some("Brass"));
}

#[test]
fn dialog_baseline_move_preserves_other_pending_positions_without_drift() {
    let mut s = Session::default();
    let (_, first) = created(&mut s, "Part");
    let second = run(&mut s, "occurrence.copy", json!({"occurrence":first,"translate":[20,0,0]}))["occurrence"].as_u64().unwrap();
    run(&mut s, "occurrence.move", json!({"occurrence":first,"translate":[5,0,0],"capture":false}));
    run(&mut s, "occurrence.move", json!({"occurrence":second,"translate":[3,0,0],"capture":false}));
    let baseline = s.occurrence_placement(&[second]).unwrap().local;
    for offset in [10.0, 15.0, 0.0] {
        run(
            &mut s,
            "occurrence.move",
            json!({"occurrence":second,"path":[second],"space":"world","base_transform":baseline,"translate":[offset,0,0],"capture":false}),
        );
        assert_eq!(s.pending_moves.get(&first).unwrap()[3][0], 5.0);
        assert_eq!(s.pending_moves.get(&second).unwrap()[3][0], 23.0 + offset);
    }
    let pending = s.pending_moves.clone();
    assert!(s.execute("occurrence.move", &json!({"occurrence":second,"base_transform":[[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,1]]})).is_err());
    assert_eq!(s.pending_moves, pending);
    for base in [
        [[2.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]],
        [[-1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]],
    ] {
        assert!(s.execute("occurrence.move", &json!({"occurrence":second,"base_transform":base})).is_err());
        assert_eq!(s.pending_moves, pending);
    }
    assert!(s.execute("component.revert_position", &json!({"occurrences":[second,9999]})).is_err());
    assert_eq!(s.pending_moves, pending);
    run(&mut s, "component.revert_position", json!({"occurrences":[second]}));
    assert!(!s.pending_moves.contains_key(&second));
    assert_eq!(s.pending_moves.get(&first).unwrap()[3][0], 5.0);
}

#[test]
fn hostile_moves_copies_and_reused_ids_do_not_leak_state() {
    let mut s = Session::default();
    let (_, first) = created(&mut s, "Part");
    run(&mut s, "occurrence.move", json!({"occurrence":first,"translate":[10,0,0],"capture":false}));
    let pending = s.pending_moves.clone();
    let doc = s.doc.clone();
    for (cmd, p) in [
        ("occurrence.move", json!({"occurrence":first,"origin":[1e308,0,0],"angle":"180 deg"})),
        ("occurrence.move", json!({"occurrence":first,"translate":[1e9,0,0],"capture":false})),
        ("occurrence.move", json!({"occurrence":first,"translate":[1,2]})),
        ("occurrence.move", json!({"occurrence":first,"angle":false})),
        ("occurrence.move", json!({"occurrence":first,"axis":[1e308,0,0],"angle":"90 deg"})),
        ("occurrence.move", json!({"occurrence":first,"axis":[0,0,0],"angle":"90 deg"})),
        ("occurrence.copy", json!({"occurrence":first,"translate":[1e9,0,0]})),
        ("occurrence.copy", json!({"occurrence":first,"translate":"oops"})),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd}");
        assert_eq!(s.pending_moves, pending);
        assert_eq!(s.doc, doc);
    }
    run(&mut s, "edit.undo", json!({}));
    assert!(s.pending_moves.is_empty());
    let (_, other) = created(&mut s, "Other");
    assert_eq!(first, other);
    assert_eq!(s.occurrence_placement(&[other]).unwrap().world[3][0], 0.0);
    run(&mut s, "occurrence.move", json!({"occurrence":other,"translate":[20,0,0],"capture":false}));
    run(&mut s, "component.capture_position", json!({}));
    assert!(s.pending_moves.is_empty());
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.pending_moves.get(&other).unwrap()[3][0], 20.0);
    assert_eq!(s.doc.occurrences.iter().find(|occurrence| occurrence.id == other).unwrap().transform[3][0], 0.0);
    assert_eq!(s.occurrence_placement(&[other]).unwrap().world[3][0], 20.0);
    run(&mut s, "edit.redo", json!({}));
    assert!(s.pending_moves.is_empty());
    assert_eq!(s.doc.occurrences.iter().find(|occurrence| occurrence.id == other).unwrap().transform[3][0], 20.0);
}

#[test]
fn isolate_nested_child_hides_ancestor_material_and_sibling() {
    let mut s = Session::default();
    let (_, parent) = created(&mut s, "Parent");
    run(&mut s, "solid.box", json!({"length":10,"width":10,"height":10}));
    let (child_component, child) = created(&mut s, "Child");
    run(&mut s, "solid.box", json!({"length":1,"width":1,"height":1}));
    let parent_component = s.doc.occurrences.iter().find(|o| o.id == child).unwrap().parent;
    let sibling = s
        .doc_mut()
        .add_occurrence(
            child_component,
            parent_component,
            solidcraft_doc::rigid(crate::geom::Vec3::new(20.0, 0.0, 0.0), crate::geom::Vec3::ZERO, crate::geom::Vec3::Z, 0.0),
        )
        .unwrap();
    s.refresh();
    run(&mut s, "browser.visibility", json!({"visible":true,"paths":[[parent,child]],"isolate":true}));
    for (path, expected) in [(vec![parent], false), (vec![parent, child], true), (vec![parent, sibling], false)] {
        let body = s.occurrence_bodies(&path, false).unwrap()[0].clone();
        assert_eq!(s.body_visible(&body), expected);
    }
}

#[test]
fn context_new_child_uses_inactive_parent_placement() {
    let mut s = Session::default();
    let (parent_component, parent) = created(&mut s, "Parent");
    let second = run(&mut s, "occurrence.copy", json!({"occurrence":parent,"translate":[20,0,0]}))["occurrence"].as_u64().unwrap();
    run(&mut s, "component.activate", json!({"component":0}));
    let child =
        run(&mut s, "component.create", json!({"parent":parent_component,"parent_path":[second],"name":"Child"}))["occurrence"].as_u64().unwrap();
    assert_eq!(s.active_occurrence_path, Some(vec![second, child]));
    assert_eq!(s.active_component_frame()[3][0], 20.0);
    let doc = s.doc.clone();
    let active = s.active_occurrence_path.clone();
    assert!(s.execute("component.create", &json!({"parent":parent_component,"parent_path":[999],"name":"Invalid"})).is_err());
    assert_eq!(s.doc, doc);
    assert_eq!(s.active_occurrence_path, active);
}

#[test]
fn paste_new_uses_selected_pending_placement() {
    let mut s = Session::default();
    let (component, first) = created(&mut s, "Part");
    run(&mut s, "solid.box", json!({"length":1,"width":1,"height":1}));
    let second = run(&mut s, "occurrence.copy", json!({"occurrence":first,"translate":[20,0,0]}))["occurrence"].as_u64().unwrap();
    run(&mut s, "occurrence.move", json!({"occurrence":second,"translate":[30,0,0],"capture":false}));
    let copied =
        run(&mut s, "component.paste_new", json!({"component":component,"occurrence":second,"path":[second],"translate":[5,0,0]}))["component"]
            .as_u64()
            .unwrap();
    assert_eq!(s.doc.occurrence_of(copied).unwrap().transform[3][0], 55.0);
    let result = run(&mut s, "component.properties", json!({"component":copied}));
    near(&result["total"]["center_of_mass"][0], 55.5);
    let result = run(&mut s, "occurrence.properties", json!({"occurrence":first,"path":[first]}));
    near(&result["total"]["center_of_mass"][0], 0.5);
}

#[test]
fn instance_styles_follow_source_identity_after_collision_rename_and_pending_move() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length":1,"width":1,"height":1}));
    let root = s.model.state().bodies[0].name.clone();
    let (component, first) = created(&mut s, "Part");
    run(&mut s, "solid.box", json!({"length":2,"width":2,"height":2}));
    let second = run(&mut s, "occurrence.copy", json!({"occurrence":first,"translate":[20,0,0]}))["occurrence"].as_u64().unwrap();
    let candidate = s.occurrence_bodies(&[second], false).unwrap()[0].clone();
    run(&mut s, "body.rename", json!({"body":root,"name":candidate}));
    let instance = s.occurrence_bodies(&[second], false).unwrap()[0].clone();
    let source = s.occurrence_placement(&[second]).unwrap().bodies[0].source.clone();
    run(&mut s, "material.assign", json!({"bodies":[instance],"material":"Brass"}));
    let world = s.world_state();
    assert_eq!(s.doc.body_color(world.body(&instance).unwrap()), s.doc.body_color(world.body(&source).unwrap()));
    run(&mut s, "appearance.assign", json!({"components":[component.to_string()],"color":"#0000ff","opacity":0.7}));
    assert_eq!(s.doc.body_color(s.world_state().body(&instance).unwrap()), Some([0.0, 0.0, 1.0]));
    run(&mut s, "appearance.assign", json!({"bodies":[instance],"color":"#ff0000","opacity":0.5}));
    run(&mut s, "appearance.assign", json!({"faces":[{"body":instance,"point":[21,1,2]}],"color":"#00ff00","opacity":0.4}));
    // Exercise the point-based face fallback as well as persistent face-name matching.
    for face in &mut s.doc_mut().appearances.faces {
        face.name = None;
    }
    run(&mut s, "body.rename", json!({"body":source,"name":"Shared Renamed"}));
    run(&mut s, "occurrence.move", json!({"occurrence":second,"translate":[5,0,0],"capture":false}));
    let world = s.world_state();
    for id in [first, second] {
        let bodyname = s.occurrence_bodies(&[id], false).unwrap()[0].clone();
        let body = world.body(&bodyname).unwrap();
        assert_eq!(body.definition_name(), "Shared Renamed");
        assert_eq!(body.component, component);
        let look = s.doc.model_body_look(body).unwrap();
        assert_eq!(look.color, [255, 0, 0]);
        assert_eq!(look.opacity, 0.5);
        assert!(s.doc.face_colors(body).iter().any(|(_, l)| l.color == [0, 255, 0] && l.opacity == 0.4));
        let measured = run(&mut s, "inspect.measure", json!({"bodies":[bodyname]}));
        assert_eq!(measured["bodies"][0]["material"], "Brass");
        assert!((measured["bodies"][0]["mass_g"].as_f64().unwrap() - 0.068).abs() < 1e-8);
    }
    let definition = s.model.state();
    assert_eq!(solidcraft_kernel::measure(&definition.body("Shared Renamed").unwrap().body).unwrap().bbox.min, crate::geom::Vec3::ZERO);
    let painted = s.doc.painted(&world);
    let secondname = s.occurrence_bodies(&[second], false).unwrap()[0].clone();
    let body = painted.body(&secondname).unwrap();
    assert_eq!(body.body.color(), Some([1.0, 0.0, 0.0]));
    assert!(body.body.paint().unwrap().faces.iter().any(|f| f.color == [0.0, 1.0, 0.0]));
    let bytes = solidcraft_io::export(&painted, &[secondname], solidcraft_io::Format::Step, "InstanceStyles").unwrap();
    let imported = solidcraft_kernel::step_import(&String::from_utf8(bytes).unwrap()).unwrap();
    assert_eq!(imported.bodies.len(), 1);
    assert_eq!(imported.bodies[0].body.color(), Some([1.0, 0.0, 0.0]));
    assert!(imported.bodies[0].body.paint().unwrap().faces.iter().any(|f| f.color == [0.0, 1.0, 0.0]));
}

/// A design file path in a fresh temporary folder (removed by the caller).
fn temp_design(tag: &str, ext: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("solidcraft-visibility-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("design.{ext}")).to_string_lossy().to_string();
    (dir, path)
}

/// #531: hiding a placement and a body is saved with the design and comes back when it opens.
/// It is not an undo step (undoing a later feature leaves it hidden), but the design then has
/// unsaved changes, and showing it again is saved the same way.
#[test]
fn hidden_placements_and_bodies_are_saved_with_the_design_but_not_undone() {
    let (dir, path) = temp_design("saved", "solidcraft");
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length":4,"width":4,"height":4,"body_name":"Base"}));
    let (component, first) = created(&mut s, "Part");
    run(&mut s, "solid.box", json!({"length":2,"width":2,"height":2}));
    let second = run(&mut s, "occurrence.copy", json!({"occurrence":first,"translate":[20,0,0]}))["occurrence"].as_u64().unwrap();
    run(&mut s, "component.activate", json!({"component":0}));
    run(&mut s, "file.save", json!({"path": path}));
    assert!(!s.is_dirty());

    let undo = s.undo.len();
    run(&mut s, "browser.visibility", json!({"visible":false,"paths":[[second]]}));
    run(&mut s, "browser.visibility", json!({"visible":false,"items":["Base"]}));
    assert_eq!(s.undo.len(), undo, "hiding is not an undo step");
    assert!(s.is_dirty(), "hiding leaves the design with unsaved changes");
    run(&mut s, "solid.box", json!({"length":1,"width":1,"height":1,"corner":[50,0,0],"operation":"new","body_name":"Later"}));
    run(&mut s, "edit.undo", json!({}));
    assert!(!s.body_visible("Base") && !s.occurrence_visible(&[second]), "undo keeps the hiding");

    run(&mut s, "file.save", json!({}));
    assert!(!s.is_dirty());
    run(&mut s, "file.new", json!({}));
    run(&mut s, "doc.open", json!({"path": path}));
    assert!(!s.body_visible("Base"), "the hidden body stays hidden");
    assert!(!s.occurrence_visible(&[second]), "the hidden placement stays hidden");
    assert!(s.occurrence_visible(&[first]));
    assert!(s.doc.components.iter().any(|c| c.id == component));
    assert!(!s.is_dirty(), "an opened design has no unsaved changes");
    assert!(s.undo.is_empty());

    run(&mut s, "browser.visibility", json!({"visible":true,"folder":"bodies"}));
    assert!(s.is_dirty());
    run(&mut s, "file.save", json!({}));
    run(&mut s, "file.new", json!({}));
    run(&mut s, "doc.open", json!({"path": path}));
    assert!(s.body_visible("Base") && s.occurrence_visible(&[second]), "showing again is saved too");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #531: designs saved before visibility was kept (no `visibility` in the file) open with
/// everything shown, and a design with nothing hidden saves exactly as before.
#[test]
fn designs_without_saved_visibility_open_unchanged() {
    let (dir, path) = temp_design("legacy", "json");
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length":4,"width":4,"height":4,"body_name":"Base"}));
    let mut legacy: Value = serde_json::from_slice(&solidcraft_io::write_design(&s.doc)).unwrap();
    assert!(legacy.get("visibility").is_none(), "nothing hidden: no visibility in the file");
    legacy["format"] = json!("solidcraft/15");
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

    run(&mut s, "file.new", json!({}));
    run(&mut s, "doc.open", json!({"path": path}));
    assert!(s.body_visible("Base"));
    assert_eq!(s.visibility, crate::Visibility::default());
    assert!(!s.is_dirty());
    run(&mut s, "file.save", json!({}));
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(saved.get("visibility").is_none());

    run(&mut s, "browser.visibility", json!({"visible":false,"items":["Base"]}));
    run(&mut s, "file.save", json!({}));
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["visibility"]["hidden_bodies"], json!(["Base"]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Command names for linking another design are a proposal (#533); adjust them to the real API.
const INSERT_DESIGN: &str = "component.insert_design";
const UPDATE_LINKS: &str = "component.update_links";

fn saved_box_design(path: &std::path::Path, height: f64) {
    let mut part = Session::default();
    run(&mut part, "solid.box", json!({"length": 10, "width": 20, "height": height, "body_name": "Plate"}));
    run(&mut part, "file.save", json!({"path": path}));
}

#[test]
fn a_linked_design_brings_its_bodies_in_and_follows_edits_to_the_part_file() {
    let dir = std::env::temp_dir().join(format!("solidcraft-linked-design-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let part = dir.join("plate.solidcraft");
    // Given a part saved in its own design: a 10 x 20 x 30 plate.
    saved_box_design(&part, 30.0);
    // When an assembly inserts it as a linked component, the plate's body is in the assembly.
    let mut assembly = Session::default();
    let inserted = assembly.execute(INSERT_DESIGN, &json!({"path": part, "translate": [50, 0, 0]}));
    let inserted = inserted.unwrap_or_else(|e| panic!("{INSERT_DESIGN}: {e}"));
    let occurrence = inserted["occurrence"].as_u64().unwrap_or_else(|| panic!("no occurrence in {inserted}"));
    let measured = run(&mut assembly, "occurrence.properties", json!({"occurrence": occurrence, "path": [occurrence]}));
    near(&measured["total"]["volume_mm3"], 6000.0);
    near(&measured["total"]["center_of_mass"][0], 55.0);
    // When the part is edited (30 -> 45 tall) and saved again, updating the links follows it.
    saved_box_design(&part, 45.0);
    run(&mut assembly, UPDATE_LINKS, json!({}));
    let measured = run(&mut assembly, "occurrence.properties", json!({"occurrence": occurrence, "path": [occurrence]}));
    near(&measured["total"]["volume_mm3"], 9000.0);
    near(&measured["total"]["center_of_mass"][0], 55.0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The compact list counts what a component holds and leaves out identity transforms; the full
/// list stays as it was.
#[test]
fn compact_component_list_counts_and_skips_identity_transforms() {
    let mut s = Session::default();
    let (component, first) = created(&mut s, "Block");
    run(&mut s, "solid.box", json!({"length": 1, "width": 1, "height": 1, "body_name": "Cube"}));
    run(&mut s, "component.activate", json!({"component": "root"}));
    let second = run(&mut s, "occurrence.copy", json!({"component": component, "translate": [20, 0, 0]}))["occurrence"].as_u64().unwrap();
    let full = run(&mut s, "component.list", json!({}));
    let block = |l: &Value| l["components"].as_array().unwrap().iter().find(|c| c["id"] == component).unwrap().clone();
    assert_eq!(block(&full)["bodies"], json!(["Cube"]));
    assert!(block(&full)["occurrences"].as_array().unwrap().iter().all(|o| o["transform"].is_array()));
    let compact = run(&mut s, "component.list", json!({"compact": true}));
    let b = block(&compact);
    assert_eq!((&b["bodies"], &b["features"], &b["sketches"]), (&json!(1), &json!(1), &json!(0)), "{b}");
    let placed = |id: u64| b["occurrences"].as_array().unwrap().iter().find(|o| o["id"] == id).unwrap().clone();
    assert!(placed(first).get("transform").is_none(), "{b}");
    assert_eq!(placed(second)["transform"][3][0], 20.0, "{b}");
    let root = compact["components"].as_array().unwrap().iter().find(|c| c["id"] == 0).unwrap().clone();
    assert_eq!(root["children"], json!([first, second]), "{root}");
    // Body names on request.
    assert_eq!(block(&run(&mut s, "component.list", json!({"compact": true, "bodies": true})))["bodies"], json!(["Cube"]));
}

/// A script's 3x4 placement matrix (rotation rows, then the translation column) moves an
/// occurrence exactly as the same turn and move given as an axis, an angle and a translation.
#[test]
fn matrix_move_turns_a_quarter_and_moves() {
    let mut s = Session::default();
    let (_, occ) = created(&mut s, "Block");
    run(&mut s, "solid.box", json!({"length": 2, "width": 1, "height": 1}));
    // 90 degrees about Z, then (10, 20, 30): (x, y, z) goes to (10 - y, 20 + x, 30 + z).
    let r = run(&mut s, "occurrence.move", json!({"occurrence": occ, "matrix": [0, -1, 0, 10, 1, 0, 0, 20, 0, 0, 1, 30]}));
    let t = &r["transform"];
    for (col, expected) in [[0.0, 1.0, 0.0, 0.0], [-1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [10.0, 20.0, 30.0, 1.0]].iter().enumerate() {
        for (row, x) in expected.iter().enumerate() {
            near(&t[col][row], *x);
        }
    }
    // The box's centre (1, 0.5, 0.5) lands at (9.5, 21, 30.5).
    let props = run(&mut s, "occurrence.properties", json!({"occurrence": occ}));
    for (i, x) in [9.5, 21.0, 30.5].into_iter().enumerate() {
        near(&props["total"]["center_of_mass"][i], x);
    }
    // The same as axis/angle/translate, and the 4x4 form as rows says the same.
    let mut other = Session::default();
    let (_, o2) = created(&mut other, "Block");
    let r2 = run(&mut other, "occurrence.move", json!({"occurrence": o2, "axis": [0, 0, 1], "angle": "90 deg", "translate": [10, 20, 30]}));
    let mut rows = Session::default();
    let (_, o3) = created(&mut rows, "Block");
    let r3 = run(&mut rows, "occurrence.move", json!({"occurrence": o3, "matrix": [[0, -1, 0, 10], [1, 0, 0, 20], [0, 0, 1, 30], [0, 0, 0, 1]]}));
    for col in 0..4 {
        for row in 0..4 {
            near(&r2["transform"][col][row], t[col][row].as_f64().unwrap());
            near(&r3["transform"][col][row], t[col][row].as_f64().unwrap());
        }
    }
}

/// A rotation that is one only up to rounding is orthonormalised; a scale, a shear or a mirror
/// is refused with the reason, and the occurrence stays where it was.
#[test]
fn matrix_move_orthonormalises_rounding_and_refuses_what_is_not_rigid() {
    let mut s = Session::default();
    let (_, occ) = created(&mut s, "Block");
    let (sin, cos) = 30f64.to_radians().sin_cos();
    let e = 3e-10;
    let nearly = json!([cos + e, -sin, 0, 5, sin, cos - e, e, 0, 0, -e, 1.0 + e, 0]);
    let r = run(&mut s, "occurrence.move", json!({"occurrence": occ, "matrix": nearly, "space": "world"}));
    let m: solidcraft_doc::Mat = serde_json::from_value(r["transform"].clone()).unwrap();
    for i in 0..3 {
        for j in 0..3 {
            let dot: f64 = (0..3).map(|k| m[i][k] * m[j][k]).sum();
            assert!((dot - f64::from(u8::from(i == j))).abs() < 1e-15, "column {i}·{j} = {dot}");
        }
    }
    assert!((m[0][0] - cos).abs() < 1e-9 && (m[0][1] - sin).abs() < 1e-9 && (m[3][0] - 5.0).abs() < 1e-12, "{m:?}");

    let before = s.doc.occurrences.iter().find(|o| o.id == occ).unwrap().transform;
    let refused = |s: &mut Session, p: Value| s.execute("occurrence.move", &p).unwrap_err().to_string();
    let scaled = refused(&mut s, json!({"occurrence": occ, "matrix": [1.001, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0]}));
    assert!(scaled.contains("not rigid") && scaled.contains("2.00"), "{scaled}");
    let sheared = refused(&mut s, json!({"occurrence": occ, "matrix": [1, 0.01, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0]}));
    assert!(sheared.contains("not rigid"), "{sheared}");
    let mirrored = refused(&mut s, json!({"occurrence": occ, "matrix": [-1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0]}));
    assert!(mirrored.contains("solid.mirror"), "{mirrored}");
    let projective = refused(&mut s, json!({"occurrence": occ, "matrix": [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0.5, 0, 0, 1]}));
    assert!(projective.contains("fourth row"), "{projective}");
    // A looser tolerance is the caller's to ask for, up to 1e-3.
    let rounded = json!([0.866, -0.5, 0, 0, 0.5, 0.866, 0, 0, 0, 0, 1, 0]);
    assert!(refused(&mut s, json!({"occurrence": occ, "matrix": rounded})).contains("not rigid"));
    for p in [
        json!({"occurrence": occ, "matrix": rounded, "tolerance": 1}),
        json!({"occurrence": occ, "matrix": [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1], "translate": [1, 0, 0]}),
        json!({"occurrence": occ, "matrix": [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0], "translate": [1, 0, 0]}),
        json!({"occurrence": occ, "matrix": [1, 0, 0, 1e12, 0, 1, 0, 0, 0, 0, 1, 0]}),
        json!({"occurrence": occ, "matrix": [[1, 0, 0, 0], [0, 1, 0, 0]]}),
        json!({"occurrence": occ, "matrix": "identity"}),
        json!({"occurrence": occ, "matrix": [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]}),
    ] {
        assert!(s.execute("occurrence.move", &p).is_err(), "{p}");
    }
    assert_eq!(s.doc.occurrences.iter().find(|o| o.id == occ).unwrap().transform, before);
    run(&mut s, "occurrence.move", json!({"occurrence": occ, "matrix": rounded, "tolerance": 1e-3}));
}
