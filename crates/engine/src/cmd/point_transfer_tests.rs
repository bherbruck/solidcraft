use serde_json::{Value, json};
use solidcraft_geom::{Vec2, Vec3};

use crate::Session;

fn run(s: &mut Session, command: &str, params: Value) -> Value {
    s.execute(command, &params).unwrap()
}

#[test]
fn sketch_move_and_copy_use_the_picked_source_anchor_and_one_undo() {
    for copy in [false, true] {
        let mut s = Session::default();
        let sketch = run(&mut s, "sketch.create", json!({"plane": "YZ"}))["sketch"].as_u64().unwrap();
        run(&mut s, "sketch.line", json!({"points": [[5,7],[15,7]], "ids": ["l1"]}));
        let before = s.doc.to_json();
        let undo = s.undo.len();
        let out = run(&mut s, "sketch.move", json!({"entities": ["l1"], "from": [5,7], "to": [25,37], "copy": copy}));
        let sk = s.doc.sketch(sketch).unwrap();
        let moved = if copy { out["curves"][0].as_str().unwrap() } else { "l1" };
        let curve = &sk.curves[sk.curve_index(moved).unwrap()];
        assert!(sk.point(curve.kind.point_ids()[0]).unwrap().dist(Vec2::new(25., 37.)) < 1e-7);
        assert_eq!(sk.curves.len(), if copy { 2 } else { 1 });
        assert_eq!(s.undo.len(), undo + 1);
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.to_json(), before);
    }
}

#[test]
fn anchored_sketch_paste_keeps_dimensions_and_the_original_in_one_undo() {
    let mut s = Session::default();
    let sketch = run(&mut s, "sketch.create", json!({"plane": "XY"}))["sketch"].as_u64().unwrap();
    run(&mut s, "sketch.line", json!({"points": [[5,7],[15,7]], "ids": ["l1"]}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "value": 10}));
    run(&mut s, "sketch.copy", json!({"entities": ["l1"]}));
    let before = s.doc.to_json();
    let undo = s.undo.len();
    let out = run(&mut s, "edit.paste.point_to_point", json!({"from": [5,7], "to": [45,27]}));
    let sk = s.doc.sketch(sketch).unwrap();
    let curve = &sk.curves[sk.curve_index(out["curves"][0].as_str().unwrap()).unwrap()];
    assert!(sk.point(curve.kind.point_ids()[0]).unwrap().dist(Vec2::new(45., 27.)) < 1e-6);
    let params: Vec<_> = sk.constraints.iter().filter_map(|c| c.param.as_ref()).collect();
    assert_eq!(params.len(), 2);
    assert_ne!(params[0], params[1]);
    assert_eq!(s.undo.len(), undo + 1);
    assert_eq!(s.sketch_clipboard.as_ref().unwrap().pastes, 0);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.to_json(), before);
}

#[test]
fn body_paste_and_move_keep_exact_world_delta_in_rotated_component() {
    for copy in [false, true] {
        let mut s = Session::default();
        let c = run(&mut s, "component.create", json!({"name": "Block"}));
        let occurrence = c["occurrence"].as_u64().unwrap();
        run(&mut s, "occurrence.move", json!({"occurrence": occurrence, "axis": [0,0,1], "angle": "90 deg", "translate": [20,30,0]}));
        run(&mut s, "solid.box", json!({"length": 2, "width": 2, "height": 2, "corner": [20,30,0], "operation": "new"}));
        let state = s.world_state();
        let body = state.bodies.first().unwrap();
        let center = (body.mesh().bounds().min + body.mesh().bounds().max) * 0.5;
        let objects = json!([{"type": "body", "name": body.name}]);
        let before = s.doc.to_json();
        let undo = s.undo.len();
        let p = json!({"objects": objects, "from": [20,30,0], "to": [27,41,13]});
        if copy {
            run(&mut s, "objects.copy", json!({"objects": objects}));
            run(&mut s, "edit.paste.point_to_point", p);
        } else {
            run(&mut s, "objects.move.point_to_point", p);
        }
        let world = s.world_state();
        assert_eq!(world.bodies.len(), if copy { 2 } else { 1 });
        let expected = center + Vec3::new(7., 11., 13.);
        assert!(
            world.bodies.iter().any(|b| {
                let bb = b.mesh().bounds();
                ((bb.min + bb.max) * 0.5).dist(expected) < 1e-6
            }),
            "{:?}",
            world.bodies.iter().map(|b| b.mesh().bounds()).collect::<Vec<_>>()
        );
        assert_eq!(s.undo.len(), undo + 1);
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.to_json(), before);
    }
}

#[test]
fn nested_component_paste_uses_exact_path_and_pending_transform_in_world_space() {
    let mut s = Session::default();
    let parent = run(&mut s, "component.create", json!({"name": "Parent"}))["occurrence"].as_u64().unwrap();
    run(&mut s, "occurrence.move", json!({"occurrence": parent, "axis": [0,0,1], "angle": "90 deg"}));
    let child = run(&mut s, "component.create", json!({"name": "Child"}))["occurrence"].as_u64().unwrap();
    run(&mut s, "occurrence.move", json!({"occurrence": child, "path": [parent,child], "space": "world", "translate": [10,0,0], "capture": false}));
    let source = s.occurrence_placement(&[parent, child]).unwrap();
    run(&mut s, "objects.copy", json!({"objects": [{"type": "occurrence", "path": [parent,child]}]}));
    let undo = s.undo.len();
    let out = run(&mut s, "edit.paste.point_to_point", json!({"from": [10,0,0], "to": [25,8,3]}));
    let pasted = out["occurrence"].as_u64().unwrap();
    let placement = s.occurrence_placement(&[parent, pasted]).unwrap();
    let delta = Vec3::new(15., 8., 3.);
    assert!(solidcraft_doc::apply_point(&placement.world, Vec3::ZERO).dist(solidcraft_doc::apply_point(&source.world, Vec3::ZERO) + delta) < 1e-7);
    assert_eq!(s.undo.len(), undo + 1);
    run(&mut s, "edit.undo", json!({}));
    assert!(s.occurrence_placement(&[parent, pasted]).is_err());
    assert_eq!(s.occurrence_placement(&[parent, child]).unwrap().world, source.world);
    run(
        &mut s,
        "objects.move.point_to_point",
        json!({"objects": [{"type": "occurrence", "path": [parent,child]}], "from": [10,0,0], "to": [25,8,3]}),
    );
    let moved = s.occurrence_placement(&[parent, child]).unwrap();
    assert!(solidcraft_doc::apply_point(&moved.world, Vec3::ZERO).dist(solidcraft_doc::apply_point(&source.world, Vec3::ZERO) + delta) < 1e-7);
}

#[test]
fn malformed_anchors_are_refused_without_mutation_or_panic() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.line", json!({"points": [[0,0],[10,0]], "ids": ["l1"]}));
    run(&mut s, "sketch.copy", json!({"entities": ["l1"]}));
    for params in [
        json!({"from": [0,0]}),
        json!({"to": [0,0]}),
        json!({"from": [0,0,0], "to": [1,2]}),
        json!({"from": [0,0], "to": [1e12,0]}),
        json!({"from": [-9e8,0], "to": [9e8,0]}),
        json!({"from": [0,0], "to": [2,3], "translate": [1,2]}),
        json!([]),
    ] {
        let before = s.doc.to_json();
        assert!(s.execute("sketch.paste", &params).is_err(), "{params}");
        assert_eq!(s.doc.to_json(), before);
        assert!(s.execute("sketch.move", &params).is_err(), "{params}");
        assert_eq!(s.doc.to_json(), before);
    }
}

#[test]
fn body_operations_refuse_a_different_active_component_without_changing_design() {
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Block"}));
    run(&mut s, "solid.box", json!({"length": 2, "width": 2, "height": 2}));
    let name = s.world_state().bodies[0].name.clone();
    run(&mut s, "objects.copy", json!({"objects": [{"type":"body", "name":name}]}));
    run(&mut s, "component.activate", json!({"component": "root"}));
    let before = s.doc.to_json();
    let undo = s.undo.len();
    let error = s.execute("edit.paste.point_to_point", &json!({"from":[0,0,0],"to":[20,30,40]})).unwrap_err();
    assert!(error.to_string().contains("activate"));
    assert_eq!(s.doc.to_json(), before);
    assert_eq!(s.undo.len(), undo);
}

#[test]
fn body_copy_uses_the_selected_instances_rotation_even_when_another_is_active() {
    let mut s = Session::default();
    let component = run(&mut s, "component.create", json!({"name": "Block"}))["component"].as_u64().unwrap();
    run(&mut s, "solid.box", json!({"length": 2, "width": 2, "height": 2}));
    let second = run(&mut s, "occurrence.copy", json!({"component": component, "translate": [40,50,0]}))["occurrence"].as_u64().unwrap();
    run(&mut s, "occurrence.move", json!({"occurrence": second, "axis": [0,0,1], "angle": "90 deg"}));
    let placement = s.occurrence_placement(&[second]).unwrap();
    let name = &placement.bodies[0].world;
    let state = s.world_state();
    let bounds = state.body(name).unwrap().mesh().bounds();
    let center = (bounds.min + bounds.max) * 0.5;
    s.doc_mut().units = "in".into();
    run(&mut s, "objects.copy", json!({"objects": [{"type":"body", "name":name}]}));
    run(&mut s, "edit.paste.point_to_point", json!({"from":[0,0,0], "to":[7,11,13]}));
    let expected = center + Vec3::new(7., 11., 13.);
    assert!(s.world_state().bodies.iter().any(|b| {
        let bb = b.mesh().bounds();
        ((bb.min + bb.max) * 0.5).dist(expected) < 1e-6
    }));
}
