//! A cut takes exactly the prism its sketch and extent define, and a join adds exactly that,
//! whatever stands beside the tool or beyond its ends. The tool is still grown past a face it
//! lies on (to keep coincident faces out of the boolean), but only where what it gains is empty
//! (a cut) or material (a join) on the bodies' faces, not just at a few sample points.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn volume(s: &Session, body: &str) -> f64 {
    s.model.state().body(body).map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).unwrap_or(f64::NAN)
}

fn has(s: &Session, body: &str, at: [f64; 3]) -> bool {
    s.model.state().body(body).is_some_and(|b| b.body.contains(Vec3::new(at[0], at[1], at[2])))
}

fn bodies(s: &Session) -> Vec<String> {
    s.model.state().bodies.iter().map(|b| b.name.clone()).collect()
}

fn same(got: f64, want: f64) -> bool {
    (got - want).abs() <= 1e-9 * want.abs().max(1.0)
}

/// A rectangle sketched on `plane`, extruded by `distance` along its normal (the other way
/// when negative).
fn rectangle(s: &mut Session, plane: Value, p0: [f64; 2], p1: [f64; 2], distance: f64, operation: &str, targets: Value) {
    run(s, "sketch.create", json!({"plane": plane, "name": "Tool"}));
    run(s, "sketch.rectangle.two_point", json!({"p0": p0, "p1": p1}));
    run(s, "sketch.finish", json!({}));
    let direction = if distance < 0.0 { "negative" } else { "positive" };
    let mut p = json!({"sketch": "Tool", "distance": distance.abs(), "direction": direction, "operation": operation});
    if !targets.is_null() {
        p["targets"] = targets;
    }
    run(s, "solid.extrude", p);
}

/// The plane z = `z`.
fn at_height(z: f64) -> Value {
    json!({"origin": [0, 0, z], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]})
}

/// An open tray, `l` × `w` × `h` outside, with a 2 mm floor and 2 mm walls.
fn tray(l: f64, w: f64, h: f64) -> Session {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": l, "width": w, "height": h, "body_name": "Tray"}));
    run(&mut s, "solid.box", json!({"corner": [2, 2, 2], "length": l - 4.0, "width": w - 4.0, "height": h, "operation": "cut", "targets": ["Tray"]}));
    assert!(same(volume(&s, "Tray"), l * w * h - (l - 4.0) * (w - 4.0) * (h - 2.0)));
    s
}

/// A cut from the tray's floor up, over x −1 … X and all of y, takes the end wall x = 0 … 2
/// and, past x = 2, the start of the two long walls. At X = 2 its side lies in the end wall's
/// inner face, at 1.9 it leaves 0.1 mm of that wall, at 2.1 it reaches 0.1 mm past it. The
/// tray then starts at x = X and has lost min(X, 2) · W · (H − 2) + max(0, X − 2) · 2 · 2 ·
/// (H − 2), whatever its size. (The tool's side used to be grown by 0.5 % of the tray's size
/// where three points along it found room: 0.6 mm of the long walls went with it on a 100 mm
/// tray.)
#[test]
fn a_cut_beside_a_wall_takes_only_its_sketch() {
    for (l, w, h, x) in [(100.0, 60.0, 30.0, 1.9), (100.0, 60.0, 30.0, 2.0), (100.0, 60.0, 30.0, 2.1), (1000.0, 600.0, 300.0, 2.0)] {
        let mut s = tray(l, w, h);
        let before = volume(&s, "Tray");
        rectangle(&mut s, at_height(2.0), [-1.0, -1.0], [x, w + 10.0], h + 10.0, "cut", json!(["Tray"]));
        let removed = x.min(2.0) * w * (h - 2.0) + (x - 2.0).max(0.0) * 2.0 * 2.0 * (h - 2.0);
        let took = before - volume(&s, "Tray");
        assert!(same(took, removed), "{l} × {w} × {h} cut to x = {x}: took {took} mm³, its sketch {removed} mm³");
        // The long wall starts at the cut; what is left of the end wall and the floor stay.
        assert!(has(&s, "Tray", [x + 0.05, 1.0, h / 2.0]) && !has(&s, "Tray", [x - 0.05, 1.0, h / 2.0]), "{l} × {w} × {h} cut to x = {x}");
        assert_eq!(has(&s, "Tray", [(x + 2.0) / 2.0, w / 2.0, h / 2.0]), x < 2.0, "{l} × {w} × {h} cut to x = {x}");
        assert!(has(&s, "Tray", [x / 2.0, w / 2.0, 1.0]), "{l} × {w} × {h} cut to x = {x}: the floor under the cut is gone");
    }
}

/// What stands beside a cut whose side lies in a face stays: a 0.2 mm web between a slot and
/// the plate's edge, and a 1 mm plate standing 0.8 mm from the face (which the grown tool used
/// to cut in two).
#[test]
fn a_cut_leaves_what_stands_beside_it() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 200, "width": 100, "height": 10, "body_name": "Plate"}));
    rectangle(&mut s, at_height(0.0), [0.2, 20.0], [10.0, 80.0], 10.0, "cut", Value::Null);
    assert!(same(volume(&s, "Plate"), 200.0 * 100.0 * 10.0 - 9.8 * 60.0 * 10.0), "{}", volume(&s, "Plate"));
    assert!(has(&s, "Plate", [0.1, 50.0, 5.0]) && !has(&s, "Plate", [0.3, 50.0, 5.0]));

    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 1000, "width": 600, "height": 200, "body_name": "A"}));
    run(&mut s, "solid.box", json!({"corner": [1000.8, 0, 0], "length": 1, "width": 600, "height": 200, "operation": "new_body", "body_name": "B"}));
    rectangle(&mut s, at_height(0.0), [900.0, 100.0], [1000.0, 500.0], 200.0, "cut", Value::Null);
    assert_eq!(bodies(&s), ["A", "B"], "the plate must not be cut in two");
    assert!(same(volume(&s, "A"), 1000.0 * 600.0 * 200.0 - 100.0 * 400.0 * 200.0), "{}", volume(&s, "A"));
    assert!(same(volume(&s, "B"), 600.0 * 200.0), "{}", volume(&s, "B"));
}

/// A join adds only its sketch: a block against a wall narrower than the block adds nothing
/// beside the wall, a block 0.1 mm from a wall leaves the gap, and a pad flush with a block's
/// side does not reach a neighbour 0.8 mm from that side.
#[test]
fn a_join_adds_only_its_sketch() {
    let wall = || {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"corner": [0, 10, 0], "length": 2, "width": 40, "height": 30, "body_name": "Wall"}));
        s
    };
    let mut s = wall();
    rectangle(&mut s, at_height(0.0), [2.0, 0.0], [20.0, 60.0], 30.0, "join", Value::Null);
    assert!(same(volume(&s, "Wall"), 2.0 * 40.0 * 30.0 + 18.0 * 60.0 * 30.0), "{}", volume(&s, "Wall"));
    assert!(!has(&s, "Wall", [1.9, 5.0, 15.0]) && !has(&s, "Wall", [1.9, 55.0, 15.0]) && has(&s, "Wall", [1.9, 30.0, 15.0]));

    let mut s = wall();
    rectangle(&mut s, at_height(0.0), [2.1, 10.0], [20.0, 50.0], 30.0, "join", Value::Null);
    assert!(same(volume(&s, "Wall"), 2.0 * 40.0 * 30.0 + 17.9 * 40.0 * 30.0), "{}", volume(&s, "Wall"));
    assert!(!has(&s, "Wall", [2.05, 30.0, 15.0]), "the 0.1 mm gap is filled");

    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 300, "width": 200, "height": 100, "body_name": "A"}));
    run(
        &mut s,
        "solid.box",
        json!({"corner": [300.8, 0, 0], "length": 99.2, "width": 200, "height": 150, "operation": "new_body", "body_name": "B"}),
    );
    rectangle(&mut s, at_height(100.0), [280.0, 50.0], [300.0, 150.0], 20.0, "join", Value::Null);
    assert_eq!(bodies(&s), ["A", "B"], "the pad must not bridge the gap");
    assert!(same(volume(&s, "A"), 300.0 * 200.0 * 100.0 + 20.0 * 100.0 * 20.0), "{}", volume(&s, "A"));
}

/// A window cut down through a plate ends on its underside, where a 4 mm rib hangs below
/// between the points the end used to be checked at: the rib stays whole.
#[test]
fn a_cut_ending_on_a_face_leaves_the_rib_beyond_it() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 0], "x_dir": [1, 0, 0], "y_dir": [0, 0, 1]}, "name": "Section"}));
    let tee = [[0, 20], [88, 20], [88, 0], [92, 0], [92, 20], [200, 20], [200, 30], [0, 30]];
    run(&mut s, "sketch.line", json!({"points": tee, "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Section", "distance": 200, "body_name": "Tee"}));
    let before = volume(&s, "Tee");
    assert!(same(before, (200.0 * 10.0 + 4.0 * 20.0) * 200.0), "{before}");
    rectangle(&mut s, at_height(30.0), [50.0, -150.0], [150.0, -50.0], -10.0, "cut", Value::Null);
    assert!(same(before - volume(&s, "Tee"), 100.0 * 100.0 * 10.0), "{}", before - volume(&s, "Tee"));
    assert!(has(&s, "Tee", [90.0, -100.0, 19.5]), "the rib under the window is cut");
}

/// A hole cut up through a ring from its flat underside (the reference case `60-pipe-flange`).
/// The ring is revolved, so that face is a line swept round the axis, not a plane, but flat
/// all the same: the tool still starts past it and the hole opens it, leaving no film of zero
/// thickness over the hole (two discs of its area more surface).
#[test]
fn a_hole_from_a_revolved_rings_flat_side_opens_it() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "Profile"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [25, 0], "p1": [75, -16]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.revolve", json!({"sketch": "Profile", "axis": {"origin": [0, 0, 0], "dir": [0, 0, 1]}, "angle": 360, "body_names": ["Ring"]}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Hole"}));
    run(&mut s, "sketch.circle.center", json!({"center": [60, 0], "radius": 7}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Hole", "through_all": true, "direction": "positive", "operation": "cut"}));
    let pi = std::f64::consts::PI;
    let ring = s.model.state().body("Ring").map(|b| solidcraft_kernel::measure(&b.body).unwrap()).unwrap();
    let want = 2.0 * pi * (75.0 * 75.0 - 25.0 * 25.0 - 7.0 * 7.0) + 2.0 * pi * (75.0 + 25.0 + 7.0) * 16.0;
    assert!((ring.area - want).abs() < 1e-3 * want, "area {} instead of {want}", ring.area);
    let volume = pi * (75.0 * 75.0 - 25.0 * 25.0 - 7.0 * 7.0) * 16.0;
    assert!((ring.volume / volume - 1.0).abs() < 1e-3, "volume {} instead of {volume}", ring.volume);
    assert!(!has(&s, "Ring", [60.0, 0.0, 8.0]), "the hole is not cut");
}
