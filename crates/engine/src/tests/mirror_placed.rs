//! `solid.mirror` of bodies in a placed component (a moved occurrence, a further instance, an
//! inserted design) mirrors each body where it is shown. Before, it mirrored the component's own
//! geometry and put the copy in the active component, so the copy lost the placement.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use super::run;
use crate::*;

fn world_box(s: &Session, body: &str) -> (Vec3, Vec3) {
    let b = s.world_state().body(body).unwrap_or_else(|| panic!("no body `{body}`")).mesh().bounds();
    (b.min, b.max)
}

/// The box of `body` mirrored in the world YZ plane.
fn mirrored_yz(s: &Session, body: &str) -> (Vec3, Vec3) {
    let (lo, hi) = world_box(s, body);
    (Vec3::new(-hi.x, lo.y, lo.z), Vec3::new(-lo.x, hi.y, hi.z))
}

fn assert_box(got: (Vec3, Vec3), want: (Vec3, Vec3)) {
    assert!(got.0.dist(want.0) < 1e-6 && got.1.dist(want.1) < 1e-6, "got {got:?}, want {want:?}");
}

/// A component "Arm" holding "ArmBody", its occurrence turned and moved; the root is active.
fn moved_arm(s: &mut Session) -> (u64, u64) {
    let c = run(s, "component.create", json!({"name": "Arm"}));
    let (component, occurrence) = (c["component"].as_u64().unwrap(), c["occurrence"].as_u64().unwrap());
    run(s, "solid.box", json!({"corner": [10, 0, 0], "length": 20, "width": 4, "height": 6, "body_name": "ArmBody"}));
    run(s, "component.activate", json!({"component": 0}));
    run(s, "occurrence.move", json!({"occurrence": occurrence, "axis": [1, 0, 0], "angle": "90 deg", "translate": [30, 50, 0]}));
    (component, occurrence)
}

fn created(r: &Value) -> String {
    r["created_bodies"][0].as_str().unwrap().to_string()
}

#[test]
fn a_moved_components_body_is_mirrored_where_it_is_shown() {
    let mut s = Session::default();
    let (_, occurrence) = moved_arm(&mut s);
    let copy = created(&run(&mut s, "solid.mirror", json!({"bodies": ["ArmBody"], "plane": "YZ"})));
    assert_box(world_box(&s, &copy), mirrored_yz(&s, "ArmBody"));
    // The copy follows its source when the occurrence moves again.
    run(&mut s, "occurrence.move", json!({"occurrence": occurrence, "space": "world", "translate": [0, 200, 0]}));
    assert_box(world_box(&s, &copy), mirrored_yz(&s, "ArmBody"));
    // It survives a save and reopen.
    let t = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert_box(world_box(&t, &copy), mirrored_yz(&t, "ArmBody"));
}

#[test]
fn a_further_instance_is_mirrored_at_its_own_placement() {
    let mut s = Session::default();
    let (component, _) = moved_arm(&mut s);
    run(&mut s, "occurrence.copy", json!({"component": component, "translate": [0, 0, 40]}));
    let names: Vec<String> = s.world_state().bodies.iter().map(|b| b.name.clone()).collect();
    let instance = names.iter().find(|n| n.starts_with("ArmBody (")).cloned().unwrap();
    let r = run(&mut s, "solid.mirror", json!({"bodies": ["ArmBody", instance], "plane": "YZ"}));
    let copies: Vec<String> = r["created_bodies"].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_string()).collect();
    assert_eq!(copies.len(), 2, "{r}");
    assert_box(world_box(&s, &copies[0]), mirrored_yz(&s, "ArmBody"));
    assert_box(world_box(&s, &copies[1]), mirrored_yz(&s, &instance));
    // A component placed twice would show a copy at both placements: the copies join the
    // innermost component around it that is placed once, here the design itself.
    let st = s.model.state();
    for copy in &copies {
        assert_eq!(s.doc.model_body_component(st.body(copy).unwrap()), 0, "{copy}");
    }
}

#[test]
fn a_plane_picked_on_a_moved_body_is_a_plane_of_the_active_component() {
    let mut s = Session::default();
    moved_arm(&mut s);
    // A world plane x = 30 through the moved body: the copy is mirrored in it, as shown.
    let copy = created(&run(&mut s, "solid.mirror", json!({"bodies": ["ArmBody"], "plane": {"origin": [30, 50, 0], "normal": [1, 0, 0]}})));
    let (lo, hi) = world_box(&s, "ArmBody");
    assert_box(world_box(&s, &copy), (Vec3::new(60.0 - hi.x, lo.y, lo.z), Vec3::new(60.0 - lo.x, hi.y, hi.z)));
}

#[test]
fn a_joined_mirror_of_a_moved_body_stays_in_its_component() {
    let mut s = Session::default();
    moved_arm(&mut s);
    // A world plane through the body's far end: the joined body doubles along x where it is.
    let (lo, hi) = world_box(&s, "ArmBody");
    run(&mut s, "solid.mirror", json!({"bodies": ["ArmBody"], "plane": {"origin": [hi.x, 0, 0], "normal": [1, 0, 0]}, "combine": true}));
    let len = hi.x - lo.x;
    assert_box(world_box(&s, "ArmBody"), (lo, Vec3::new(hi.x + len, hi.y, hi.z)));
}

#[test]
fn bodies_in_place_keep_no_placement() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Base"}));
    run(&mut s, "solid.mirror", json!({"bodies": ["Base"], "plane": "YZ"}));
    let f = s.doc.features.last().unwrap();
    assert!(matches!(&f.kind, solidcraft_doc::FeatureKind::Mirror { placements, .. } if placements.is_empty()), "{:?}", f.kind);
}

#[test]
fn a_position_not_captured_yet_is_refused() {
    let mut s = Session::default();
    let (_, occurrence) = moved_arm(&mut s);
    run(&mut s, "occurrence.move", json!({"occurrence": occurrence, "translate": [5, 0, 0], "capture": false}));
    let e = s.execute("solid.mirror", &json!({"bodies": ["ArmBody"], "plane": "YZ"})).unwrap_err().to_string();
    assert!(e.contains("Capture Position"), "{e}");
}

#[test]
fn a_copy_goes_beside_its_source_in_that_component() {
    // Moved or in place, a component's mirrored body is a body of that component.
    for moved in [true, false] {
        let mut s = Session::default();
        let (component, occurrence) = moved_arm(&mut s);
        if !moved {
            run(&mut s, "occurrence.move", json!({"occurrence": occurrence, "base_transform": solidcraft_doc::IDENTITY}));
        }
        let copy = created(&run(&mut s, "solid.mirror", json!({"bodies": ["ArmBody"], "plane": "YZ"})));
        let st = s.model.state();
        assert_eq!(s.doc.model_body_component(st.body(&copy).unwrap()), component, "moved: {moved}");
        assert_box(world_box(&s, &copy), mirrored_yz(&s, "ArmBody"));
        // It stays there through a later feature on it.
        run(&mut s, "solid.move", json!({"bodies": [copy.clone()], "translate": [0, 0, 5]}));
        assert_eq!(s.doc.model_body_component(s.model.state().body(&copy).unwrap()), component, "moved: {moved}");
    }
}

/// Inserting the same file again reuses its body names; a mirror of the first insert's bodies
/// keeps mirroring them (their copies vanished: the names led to the second insert's components).
#[test]
fn inserting_the_same_file_again_keeps_the_mirrored_copies() {
    let dir = std::env::temp_dir().join(format!("solidcraft-mirror-placed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("nested.step").to_string_lossy().replace('\\', "/");
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Outer"}));
    run(&mut s, "solid.box", json!({"length": 30, "width": 30, "height": 5, "body_name": "Plate"}));
    let inner = run(&mut s, "component.create", json!({"name": "Inner"}))["occurrence"].as_u64().unwrap();
    run(&mut s, "solid.box", json!({"length": 40, "width": 20, "height": 10, "body_name": "Block"}));
    run(&mut s, "component.activate", json!({"component": 0}));
    run(&mut s, "occurrence.move", json!({"occurrence": inner, "axis": [0, 0, 1], "angle": "90 deg", "translate": [100, 0, 0]}));
    run(&mut s, "file.export", json!({"path": file}));

    let mut s = Session::default();
    run(&mut s, "file.insert_step", json!({"path": file, "name": "A"}));
    let wanted: Vec<(Vec3, Vec3)> = ["Plate", "Block"].iter().map(|b| mirrored_yz(&s, b)).collect();
    let r = run(&mut s, "solid.mirror", json!({"bodies": ["Plate", "Block"], "plane": "YZ"}));
    let copies: Vec<String> = r["created_bodies"].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_string()).collect();
    run(&mut s, "solid.remove", json!({"bodies": ["Plate", "Block"]}));
    run(&mut s, "component.from_bodies", json!({"bodies": copies}));
    run(&mut s, "file.insert_step", json!({"path": file, "name": "B"}));
    let _ = std::fs::remove_dir_all(&dir);
    for r in &s.model.results {
        assert!(r.error.is_none(), "{}: {:?}", r.name, r.error);
    }
    for (copy, want) in copies.iter().zip(wanted) {
        assert_box(world_box(&s, copy), want);
    }
}

/// Moving an occurrence the mirror does not depend on leaves the mirror alone: before, any
/// occurrence move evaluated it again, and every later command paid for one more mirror.
#[test]
fn moving_an_unrelated_occurrence_does_not_evaluate_the_mirror_again() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 20, "width": 10, "height": 5, "corner": [10, 0, 0], "body_name": "A"}));
    run(&mut s, "solid.box", json!({"length": 8, "width": 8, "height": 8, "corner": [30, 0, 0], "body_name": "B"}));
    run(&mut s, "component.from_bodies", json!({"bodies": ["A", "B"]}));
    let occurrences: Vec<u64> = s.doc.occurrences.iter().map(|o| o.id).collect();
    assert_eq!(occurrences.len(), 2, "{:?}", s.doc.occurrences);
    let a = s.doc.body_frames(&s.model.state()).into_iter().find(|(name, ..)| name == "A").map(|(_, _, path, _)| path).unwrap();
    let (mine, other) = if a.contains(&occurrences[0]) { (occurrences[0], occurrences[1]) } else { (occurrences[1], occurrences[0]) };
    run(&mut s, "occurrence.move", json!({"occurrence": mine, "translate": [0, 5, 0]}));
    let copy = created(&run(&mut s, "solid.mirror", json!({"bodies": ["A"], "plane": "YZ"})));
    for _ in 0..3 {
        run(&mut s, "occurrence.move", json!({"occurrence": other, "translate": [0, 0.1, 0]}));
        assert_eq!(s.model.last_recomputed, 0, "an unrelated move evaluated features again");
    }
    // Moving the mirrored body's own occurrence does evaluate it, and the copy follows.
    run(&mut s, "occurrence.move", json!({"occurrence": mine, "translate": [0, 1, 0]}));
    assert!(s.model.last_recomputed > 0);
    assert_box(world_box(&s, &copy), mirrored_yz(&s, "A"));
}

/// Two mirrored copies of overlapping bodies of a placed component intersect as their sources do.
#[test]
fn mirrored_copies_of_a_placed_component_intersect() {
    let mut s = Session::default();
    let c = run(&mut s, "component.create", json!({"name": "Pair"}));
    let occurrence = c["occurrence"].as_u64().unwrap();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [10, 0, 0], "body_name": "P"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [15, 5, 5], "body_name": "Q"}));
    run(&mut s, "component.activate", json!({"component": 0}));
    run(&mut s, "occurrence.move", json!({"occurrence": occurrence, "axis": [0, 0, 1], "angle": "30 deg", "translate": [5, 20, 0]}));
    let r = run(&mut s, "solid.mirror", json!({"bodies": ["P", "Q"], "plane": "YZ"}));
    let copies: Vec<String> = r["created_bodies"].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_string()).collect();
    let i = run(&mut s, "inspect.interference", json!({}));
    let pair = i["interferences"].as_array().unwrap().iter().find(|x| x["a"] == copies[0].as_str() || x["b"] == copies[0].as_str()).cloned();
    let pair = pair.unwrap_or_else(|| panic!("{i}"));
    assert!((pair["volume_mm3"].as_f64().unwrap() - 125.0).abs() < 1e-6, "{i}");
    run(&mut s, "solid.combine", json!({"target": copies[0], "tools": [copies[1]], "operation": "intersect", "keep_tools": true}));
    let v = run(&mut s, "inspect.measure", json!({"bodies": [copies[0]]}))["total"]["volume_mm3"].as_f64().unwrap();
    assert!((v - 125.0).abs() < 1e-6, "{v}");
}
