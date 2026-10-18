//! Joint origins found by their face's name: a part jointed to a hole stays on it, on the same
//! side and turned the same way, as the design's parameters change.

use serde_json::{Value, json};
use solidcraft_doc::joints::{Snap, XRef};
use solidcraft_geom::Vec3;

use super::tests::{occ, place, run};
use crate::Session;

fn near(a: Vec3, b: Vec3) -> bool {
    a.dist(b) < 1e-6
}

/// Widths swept: small steps (a hole's wall in a 1 mm plate is a few long facets, and a pick
/// stored at the old width lies beside the moved hole) and large ones.
const WIDTHS: [f64; 9] = [302.0, 304.0, 310.0, 330.0, 400.0, 450.0, 510.0, 301.0, 300.0];

/// A tray: a 1 mm plate `W` wide (a parameter, 300 mm to start) and 100 deep, with a 6 mm hole
/// 20 mm in from its right edge (an expression); grounded. A bracket: a 20 x 20 x 2 foot with a
/// 6 mm hole in its middle and a 15 mm upright along its +X edge, so a turn shows.
fn tray() -> Session {
    let mut s = Session::default();
    run(&mut s, "parameters.change", json!({"name": "W", "expression": "300 mm"}));
    run(&mut s, "component.create", json!({"name": "Tray"}));
    run(&mut s, "solid.box", json!({"length": "W", "width": 100, "height": 1, "corner": [0, 0, 0]}));
    run(&mut s, "solid.hole", json!({"position": ["W - 20 mm", 50, 1], "diameter": 6}));
    let tray = occ(&s, "Tray");
    run(&mut s, "occurrence.ground", json!({"occurrence": tray, "grounded": true}));
    run(&mut s, "component.activate", json!({"component": "root"}));
    run(&mut s, "component.create", json!({"name": "Bracket"}));
    run(&mut s, "solid.box", json!({"length": 20, "width": 20, "height": 2, "corner": [100, 200, 0]}));
    run(&mut s, "solid.box", json!({"length": 2, "width": 20, "height": 15, "corner": [118, 200, 0], "operation": "join"}));
    run(&mut s, "solid.hole", json!({"position": [110, 210, 2], "diameter": 6}));
    s
}

fn width(s: &mut Session, w: f64) {
    run(s, "parameters.change", json!({"name": "W", "expression": format!("{w} mm")}));
    let r = run(s, "joint.solve", json!({}));
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "W = {w}: {r}");
}

/// The bracket sits on the tray's top, its hole on the tray's hole and its upright toward +X.
fn on_the_hole(s: &Session, w: f64) {
    let hole = place(s, "Bracket", Vec3::new(110.0, 210.0, 0.0));
    assert!(near(hole, Vec3::new(w - 20.0, 50.0, 1.0)), "W = {w}: the bracket's hole is at {hole:?}");
    let upright = place(s, "Bracket", Vec3::new(119.0, 210.0, 15.0));
    assert!(near(upright, Vec3::new(w - 11.0, 50.0, 16.0)), "W = {w}: the upright's top is at {upright:?}");
}

fn origin(s: &Session, joint: &str, a: bool) -> Snap {
    let j = s.doc.assembly.joints.iter().find(|j| j.name == joint).map(|j| if a { &j.a } else { &j.b });
    j.map(|o| o.snap.clone()).unwrap_or(Snap::Point { point: Vec3::ZERO, z: None })
}

#[test]
fn a_bracket_on_a_hole_in_thin_plate_stays_on_it_as_the_plate_widens() {
    let mut s = tray();
    // Picks inside the 1 mm wall of the tray's hole (nearer its top) and the 2 mm wall of the
    // bracket's (nearer its bottom); both x along +X.
    let r = run(
        &mut s,
        "joint.create",
        json!({"type": "rigid", "name": "Mount",
               "a": {"occurrence": "Tray", "circle": [283, 50, 0.8], "x": [1, 0, 0]},
               "b": {"occurrence": "Bracket", "circle": [113, 210, 0.4], "x": [1, 0, 0]}}),
    );
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    on_the_hole(&s, 300.0);
    // Both origins are kept by their wall's name and their end's (the tray's top face, the
    // bracket's bottom one).
    for a in [true, false] {
        let snap = origin(&s, "Mount", a);
        assert!(matches!(&snap, Snap::CircleCenter { face: Some(_), end: Some(_), x: Some(XRef::Dir(_)), .. }), "{snap:?}");
    }
    for w in WIDTHS {
        width(&mut s, w);
        on_the_hole(&s, w);
    }
    // Saved and opened again, it still follows.
    let dir = std::env::temp_dir().join(format!("solidcraft-joint-faces-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("tray.json").to_string_lossy().to_string();
    run(&mut s, "file.save", json!({"path": path}));
    run(&mut s, "file.new", json!({}));
    run(&mut s, "doc.open", json!({"path": path}));
    for w in [330.0, 510.0] {
        width(&mut s, w);
        on_the_hole(&s, w);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_circle_given_by_its_centre_follows_its_hole() {
    let mut s = tray();
    // The centres of the tray hole's top rim and the bracket hole's bottom rim.
    let r = run(
        &mut s,
        "joint.create",
        json!({"type": "rigid", "name": "Mount",
               "a": {"occurrence": "Tray", "circle": [280, 50, 1], "x": [1, 0, 0]},
               "b": {"occurrence": "Bracket", "circle": [110, 210, 0], "x": [1, 0, 0]}}),
    );
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    let snap = origin(&s, "Mount", true);
    assert!(matches!(&snap, Snap::CircleCenter { face: Some(_), end: Some(_), .. }), "a centre names its face and end: {snap:?}");
    on_the_hole(&s, 300.0);
    for w in WIDTHS {
        width(&mut s, w);
        on_the_hole(&s, w);
    }
}

/// The name `model.faces` gives the face of `component`'s body holding `p` (component coordinates).
fn face_name(s: &mut Session, component: &str, p: Vec3) -> String {
    let st = s.model.state();
    let c = s.doc.find_component(component).unwrap_or(0);
    solidcraft_doc::joints::face_name_at(&s.doc, &st, c, p, false).unwrap_or_default()
}

#[test]
fn origins_take_faces_by_name_and_an_end_and_turn_by_name() {
    let mut s = tray();
    let wall = face_name(&mut s, "Tray", Vec3::new(283.0, 50.0, 0.5));
    let top = face_name(&mut s, "Tray", Vec3::new(150.0, 50.0, 1.0));
    let bottom = face_name(&mut s, "Tray", Vec3::new(150.0, 50.0, 0.0));
    let right = face_name(&mut s, "Tray", Vec3::new(300.0, 50.0, 0.5));
    let foot = face_name(&mut s, "Bracket", Vec3::new(113.0, 210.0, 1.0));
    let under = face_name(&mut s, "Bracket", Vec3::new(105.0, 205.0, 0.0));
    assert!(![&wall, &top, &bottom, &right, &foot, &under].iter().any(|n| n.is_empty()));
    // A round end by the wall's name and the face it opens onto; x along the tray's right end
    // face's normal (+X).
    run(&mut s, "joint.origin", json!({"name": "Hole", "circle": wall, "end": top, "x": right}));
    let r = run(&mut s, "joint.create", json!({"type": "rigid", "name": "Mount", "a": "Hole", "b": {"circle": foot, "end": under, "x": [1, 0, 0]}}));
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    on_the_hole(&s, 300.0);
    for w in [304.0, 400.0, 510.0] {
        width(&mut s, w);
        on_the_hole(&s, w);
    }
    // The other end: the bracket hangs under the plate (its foot's underside on the plate's).
    run(&mut s, "joint.origin", json!({"name": "Under", "circle": wall, "end": bottom, "x": right}));
    run(&mut s, "joint.delete", json!({"joint": "Mount"}));
    run(&mut s, "joint.create", json!({"type": "rigid", "name": "Hung", "a": "Under", "b": {"circle": foot, "end": under, "x": [1, 0, 0]}}));
    let hole = place(&s, "Bracket", Vec3::new(110.0, 210.0, 0.0));
    assert!(near(hole, Vec3::new(490.0, 50.0, 0.0)), "{hole:?}");
    assert!(place(&s, "Bracket", Vec3::new(119.0, 210.0, 15.0)).z < -10.0);
    // An end the hole doesn't have, a missing face and a bad x are refused.
    for bad in [
        json!({"circle": wall, "end": right}),
        json!({"circle": "F999:nothing"}),
        json!({"circle": wall, "x": "F999:nothing"}),
        json!({"circle": wall, "x": [0, 0, 0]}),
        json!({"face": [10, 10, 1], "end": top}),
    ] {
        assert!(s.execute("joint.origin", &bad).is_err(), "{bad}");
    }
}

#[test]
fn a_face_centre_by_name_follows_its_face() {
    let mut s = tray();
    let right = face_name(&mut s, "Tray", Vec3::new(300.0, 50.0, 0.5));
    run(&mut s, "joint.origin", json!({"name": "End", "face": right, "x": [0, 1, 0]}));
    let frame = |s: &Session| {
        let o = s.doc.assembly.origins.iter().find(|o| o.name == "End").map(|o| o.origin.clone());
        o.and_then(|o| solidcraft_doc::joints::resolve_origin(&s.doc, &s.model.state(), &o).ok())
    };
    for w in [300.0, 330.0, 510.0] {
        width(&mut s, w);
        let m = frame(&s).unwrap_or(solidcraft_doc::IDENTITY);
        let (o, x, z) = (Vec3::new(m[3][0], m[3][1], m[3][2]), Vec3::new(m[0][0], m[0][1], m[0][2]), Vec3::new(m[2][0], m[2][1], m[2][2]));
        assert!(near(o, Vec3::new(w, 50.0, 0.5)) && near(z, Vec3::X) && near(x, Vec3::Y), "W = {w}: {o:?} {x:?} {z:?}");
    }
}

#[test]
fn an_origin_without_an_end_or_turn_opens_as_before() {
    // A design saved before origins named their end and turn: the snap reads without them.
    let snap: Snap = serde_json::from_value(json!({"type": "circle_center", "pick": [1, 2, 3], "face": "F1:side"}))
        .unwrap_or(Snap::Point { point: Vec3::ZERO, z: None });
    assert_eq!(snap, Snap::CircleCenter { pick: Vec3::new(1.0, 2.0, 3.0), face: Some("F1:side".into()), end: None, x: None });
    let back: Value = serde_json::to_value(&snap).unwrap_or_default();
    assert_eq!(back, json!({"type": "circle_center", "pick": [1.0, 2.0, 3.0], "face": "F1:side"}));
    let turned = Snap::FaceCenter { pick: Vec3::ZERO, face: None, x: Some(XRef::Face { face: "F2:+x".into(), pick: None }) };
    let v = serde_json::to_value(&turned).unwrap_or_default();
    assert_eq!(v["x"], json!({"face": "F2:+x"}));
    assert_eq!(serde_json::from_value::<Snap>(v).ok(), Some(turned));
}
