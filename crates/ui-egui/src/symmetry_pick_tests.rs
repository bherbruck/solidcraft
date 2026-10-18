//! The Symmetry tool's third pick may be the origin point or an origin axis (#1075).

use serde_json::json;
use solidcraft_engine::geom::Vec2;

use crate::scenario::Harness;

/// An XY sketch with points p (10, 5) and q (-12, -3), the Symmetry tool started and both picked.
fn two_points_picked() -> Harness {
    let mut h = Harness::new();
    h.app.set_origin_shown(true);
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.point", json!({"point":[10,5],"id":"p"})).unwrap();
    h.app.run("sketch.point", json!({"point":[-12,-3],"id":"q"})).unwrap();
    h.app.cam.distance = 120.0;
    h.settle();
    h.app.start("sketch.constraint.symmetry");
    for xy in [[10, 5], [-12, -3]] {
        let at = h.call("ui.at", json!({ "sketch": xy }))["result"].clone();
        h.step(&json!({ "click": at })).unwrap();
    }
    h
}

fn point(h: &Harness, id: &str) -> Vec2 {
    let sk = h.app.session.doc.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    sk.point(sk.resolve_point(id).unwrap()).unwrap()
}

#[test]
fn symmetry_about_the_origin_point() {
    let mut h = two_points_picked();
    let at = h.call("ui.at", json!({"sketch":[0,0]}))["result"].clone();
    h.step(&json!({ "click": at })).unwrap();
    assert!((point(&h, "p") + point(&h, "q")).len() < 1e-6, "{:?} {:?}", point(&h, "p"), point(&h, "q"));
}

#[test]
fn symmetry_about_an_origin_axis() {
    let mut h = two_points_picked();
    // On the Y axis, away from the origin point and from p and q.
    let at = h.call("ui.at", json!({"sketch":[0,6]}))["result"].clone();
    h.step(&json!({ "click": at })).unwrap();
    let (p, q) = (point(&h, "p"), point(&h, "q"));
    assert!((p.x + q.x).abs() < 1e-6 && (p.y - q.y).abs() < 1e-6, "mirrored in the Y axis: {p:?} {q:?}");
}
