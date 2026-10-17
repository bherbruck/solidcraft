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
