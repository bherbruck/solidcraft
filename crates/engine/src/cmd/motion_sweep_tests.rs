//! `joint.sweep` against stops worked out by hand: a door swinging into a block, the four-bar's
//! rocker meeting a stop and a Watt six-bar's output link meeting one, each checked against the
//! linkage solved with circle intersections and a separating-axis test of the parts' outlines.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use super::{four_bar, meet, occ, part, pin, run};
use crate::Session;

/// A part made of one box (`size` from `corner`) in a new component.
fn block(s: &mut Session, name: &str, corner: [f64; 3], size: [f64; 3]) {
    run(s, "component.activate", json!({"component": "root"}));
    run(s, "component.create", json!({"name": name}));
    run(s, "solid.box", json!({"length": size[0], "width": size[1], "height": size[2], "corner": corner}));
}

/// A 100 × 4 × 50 door hinged about Z at the origin, and a 20 × 20 block from (60, 30) in its
/// way.
fn door() -> Session {
    let mut s = Session::default();
    block(&mut s, "Door", [0.0, 0.0, 0.0], [100.0, 4.0, 50.0]);
    block(&mut s, "Block", [60.0, 30.0, 0.0], [20.0, 20.0, 50.0]);
    let r = run(
        &mut s,
        "joint.create",
        json!({"type": "revolute", "a": {"occurrence": 0, "point": [0, 0, 0]}, "b": {"occurrence": "Door", "point": [0, 0, 0]}, "flip": true, "name": "Hinge"}),
    );
    assert_eq!(r["conflicts"].as_array().map(Vec::len), Some(0), "{r}");
    s
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

fn hinge(s: &Session) -> f64 {
    s.doc.assembly.joints.iter().find(|j| j.name == "Hinge").and_then(|j| j.values.first().copied()).unwrap_or(f64::NAN)
}

/// The door's face 4 mm off its hinge line meets a corner of the block at polar (r, φ) when
/// r·sin(φ − θ) = 4, so at θ = φ − asin(4 / r); the corner (80, 30) is the first.
#[test]
fn a_door_swings_until_it_meets_the_block() {
    let mut s = door();
    let corner = |x: f64, y: f64| y.atan2(x) - (4.0 / x.hypot(y)).asin();
    let expected =
        [(60.0, 30.0), (80.0, 30.0), (60.0, 50.0), (80.0, 50.0)].iter().map(|(x, y)| corner(*x, *y)).fold(f64::INFINITY, f64::min).to_degrees();
    let before = s.doc.clone();
    let r = run(&mut s, "joint.sweep", json!({"joint": "Hinge", "from": 0, "to": 90, "report_gaps": true}));
    assert_eq!(*s.doc, *before, "a sweep leaves the design as it was");
    assert_eq!(r["applied"], false);
    let at = num(&r["stopped_at"]);
    assert!((at - expected).abs() < 0.01 && at <= expected + 1e-9, "stopped at {at}°, the block is met at {expected}°: {r}");
    let by = &r["stopped_by"];
    let names = [by["a"].as_str(), by["b"].as_str()];
    assert!(names.contains(&Some("Door:1")) && names.contains(&Some("Block:1")), "{by}");
    let touch = Vec3::new(num(&by["at"][0]), num(&by["at"][1]), num(&by["at"][2]));
    assert!((touch.x - 80.0).abs() < 0.05 && (touch.y - 30.0).abs() < 0.05 && (0.0..=50.0).contains(&touch.z), "{touch:?}");
    // Flat faces: the gaps are the boxes' own.
    assert_eq!((r["gap_method"].as_str(), num(&r["gap_tolerance_mm"])), (Some("mesh"), 0.0));
    let samples = r["samples"].as_array().cloned().unwrap_or_default();
    assert!(samples.len() > 2, "{r}");
    assert!((num(&samples[0]["min_gap_mm"]) - 26.0).abs() < 1e-9, "the door starts 26 mm below the block: {}", samples[0]);
    for w in samples.windows(2) {
        assert!(num(&w[1]["min_gap_mm"]) < num(&w[0]["min_gap_mm"]) && num(&w[1]["value"]) > num(&w[0]["value"]), "{w:?}");
    }
    let nearest = &r["min_gap"]["Door:1"];
    assert!(nearest["to"] == "Block:1" && num(&nearest["mm"]) < 2.0 && num(&nearest["at_value"]) < expected, "{}", r["min_gap"]);
    // Kept where it stopped on request.
    let r = run(&mut s, "joint.sweep", json!({"joint": "Hinge", "from": 0, "to": 90, "refine": 0.001, "apply": true}));
    assert_eq!(r["applied"], true);
    assert!((hinge(&s).to_degrees() - expected).abs() < 0.002, "{}° wants {expected}°", hinge(&s).to_degrees());
    run(&mut s, "contact.create", json!({"occurrences": ["Door", "Block"]}));
    let r = run(&mut s, "contact.check", json!({}));
    assert!(r["collisions"].as_array().is_some_and(Vec::is_empty), "kept clear of the block: {r}");
}

/// Swung the other way, nothing is in the way; a limit stops it, and a sweep starting there
/// does not move.
#[test]
fn a_door_swings_free_until_its_limit() {
    let mut s = door();
    let r = run(&mut s, "joint.sweep", json!({"joint": "Hinge", "from": 0, "to": -90, "step": 5}));
    assert!(r["stopped_at"].is_null() && r["stopped_by"].is_null(), "{r}");
    assert_eq!(r["samples"].as_array().map(Vec::len), Some(19), "{r}");
    run(&mut s, "joint.limits", json!({"joint": "Hinge", "min": -30, "max": 10}));
    let r = run(&mut s, "joint.sweep", json!({"joint": "Hinge", "from": 0, "to": -90}));
    assert_eq!((r["stopped_by"].as_str(), num(&r["stopped_at"]).round()), (Some("limit"), -30.0), "{r}");
    assert!(s.execute("joint.sweep", &json!({"joint": "Hinge", "from": 20, "to": 0})).is_err(), "`from` is past the limit");
}

/// Parts already touching where the sweep starts (a door on its stop) do not stop it; listing
/// only the parts that must stay clear checks those.
#[test]
fn parts_touching_at_the_start_are_left_out() {
    let mut s = door();
    block(&mut s, "Frame", [0.0, -10.0, 0.0], [100.0, 10.0, 50.0]);
    let r = run(&mut s, "joint.sweep", json!({"joint": "Hinge", "from": 0, "to": 90}));
    let touching = r["touching_at_start"].as_array().cloned().unwrap_or_default();
    assert_eq!(touching.len(), 1, "{r}");
    assert!(r["stopped_by"].is_object(), "the block still stops it: {r}");
    let only = json!({"joint": "Hinge", "from": 0, "to": 90, "occurrences": ["Door"], "against": ["Frame"]});
    assert!(s.execute("joint.sweep", &only).is_err(), "the only pair touches at the start");
}

#[test]
fn a_sweep_refuses_what_it_cannot_do() {
    let mut s = door();
    for p in [
        json!({"joint": "Hinge", "to": 90}),
        json!({"joint": "Hinge", "from": 0}),
        json!({"joint": "Nope", "from": 0, "to": 90}),
        json!({"joint": "Hinge", "from": 0, "to": 90, "index": 3}),
        json!({"joint": "Hinge", "from": 0, "to": 90, "refine": 0}),
        json!({"joint": "Hinge", "from": 0, "to": 90, "refine": -1}),
        json!({"joint": "Hinge", "from": 0, "to": 90, "step": 0}),
        json!({"joint": "Hinge", "from": 0, "to": 1e8, "step": 1e-6}),
        json!({"joint": "Hinge", "from": "nan", "to": 90}),
        json!({"joint": "Hinge", "from": 0, "to": 90, "occurrences": ["Door"]}),
        json!({"joint": "Hinge", "from": 0, "to": 90, "against": ["Block"]}),
    ] {
        assert!(s.execute("joint.sweep", &p).is_err(), "{p}");
    }
    run(&mut s, "joint.as_built", json!({"type": "rigid", "a": "Block", "b": "Door", "name": "Lock"}));
    assert!(s.execute("joint.sweep", &json!({"joint": "Lock", "from": 0, "to": 1})).is_err(), "a rigid joint does not move");
}

/// A point turned about `c` by `a` radians in the XY plane.
fn turn(p: Vec3, c: Vec3, a: f64) -> Vec3 {
    let (sin, cos) = a.sin_cos();
    let d = p - c;
    Vec3::new(c.x + d.x * cos - d.y * sin, c.y + d.x * sin + d.y * cos, p.z)
}

/// The XY outline of a 2 × 2 part box from `corner`, turned about `c` by `a`.
fn outline(corner: Vec3, size: f64, c: Vec3, a: f64) -> [Vec3; 4] {
    [(0.0, 0.0), (size, 0.0), (size, size), (0.0, size)].map(|(x, y)| turn(corner + Vec3::new(x, y, 0.0), c, a))
}

/// Whether two convex outlines overlap (no edge normal of either separates them).
fn overlap(a: &[Vec3; 4], b: &[Vec3; 4]) -> bool {
    for poly in [a, b] {
        for i in 0..4 {
            let e = poly[(i + 1) % 4] - poly[i];
            let n = Vec3::new(-e.y, e.x, 0.0);
            let span = |p: &[Vec3; 4]| p.iter().map(|q| q.dot(n)).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)));
            let ((a0, a1), (b0, b1)) = (span(a), span(b));
            if a1 <= b0 || b1 <= a0 {
                return false;
            }
        }
    }
    true
}

/// The crank value (degrees, 0…`hi`) where `clear` first fails, bisected to 1e-9°.
fn first_contact(hi: f64, clear: impl Fn(f64) -> bool) -> f64 {
    let (mut lo, mut hi) = (0.0, hi);
    assert!(clear(lo) && !clear(hi), "the stop must lie between 0° and {hi}°");
    while hi - lo > 1e-9 {
        let mid = (lo + hi) / 2.0;
        if clear(mid) { lo = mid } else { hi = mid }
    }
    lo
}

/// A part box 3 mm square about `c` (in XY), deep enough for the linkage's boxes (z −5…−3).
fn stop_at(s: &mut Session, c: Vec3) {
    block(s, "Stop", [c.x - 1.5, c.y - 1.5, -6.0], [3.0, 3.0, 4.0]);
    let id = occ(s, "Stop");
    run(s, "occurrence.ground", json!({"occurrence": id, "grounded": true}));
}

const O4: Vec3 = Vec3::new(45.0, 0.0, 0.0);

/// The four-bar's crank driven to `v`°, followed a quarter degree at a time from where it was
/// assembled: the crank pin and the rocker pin.
fn four_bar_at(v: f64, q0: Vec3) -> (Vec3, Vec3) {
    let n = ((v.abs() * 4.0).ceil() as usize).max(1);
    let (mut p, mut q) = (Vec3::new(0.0, 20.0, 0.0), q0);
    for k in 1..=n {
        let th = (90.0 + v * k as f64 / n as f64).to_radians();
        p = Vec3::new(20.0 * th.cos(), 20.0 * th.sin(), 0.0);
        let [m1, m2] = meet(p, 50.0, O4, 40.0);
        q = if m1.dist(q) <= m2.dist(q) { m1 } else { m2 };
    }
    (p, q)
}

fn angle(d: Vec3) -> f64 {
    d.y.atan2(d.x)
}

/// The four-bar's rocker swings into a stop: the crank stops where the rocker's box first
/// overlaps it.
#[test]
fn a_four_bar_rocker_meets_a_stop() {
    let (mut s, _, q0) = four_bar();
    let rocker = |v: f64| angle(four_bar_at(v, q0).1 - O4) - angle(q0 - O4);
    let corner = Vec3::new(q0.x, q0.y, 0.0);
    let centre = turn(corner + Vec3::new(1.0, 1.0, 0.0), O4, rocker(60.0));
    stop_at(&mut s, centre);
    let stop = [(-1.5, -1.5), (1.5, -1.5), (1.5, 1.5), (-1.5, 1.5)].map(|(x, y)| centre + Vec3::new(x, y, 0.0));
    let expected = first_contact(60.0, |v| !overlap(&outline(corner, 2.0, O4, rocker(v)), &stop));
    let r =
        run(&mut s, "joint.sweep", json!({"joint": "Crank", "from": 0, "to": 90, "refine": 0.001, "occurrences": ["Rocker"], "against": ["Stop"]}));
    let at = num(&r["stopped_at"]);
    assert!((at - expected).abs() < 0.01, "stopped at {at}°, the rocker meets the stop at {expected}°: {r}");
    let names = [r["stopped_by"]["a"].as_str(), r["stopped_by"]["b"].as_str()];
    assert!(names.contains(&Some("Rocker:1")) && names.contains(&Some("Stop:1")), "{r}");
    assert_eq!(r["pairs"], 1);
}

/// Driving the four-bar's rocker past its toggle stops where the loop can no longer close
/// (|Q| = 30: about 139.2° from the ground line).
#[test]
fn a_four_bar_sweep_stops_where_its_loop_cannot_close() {
    let (mut s, _, q0) = four_bar();
    let limit = ((900.0f64 - 3625.0) / 3600.0).acos().to_degrees() - q0.y.atan2(q0.x - 45.0).to_degrees();
    let r = run(&mut s, "joint.sweep", json!({"joint": "Rocker", "from": 0, "to": 100, "occurrences": ["Rocker"], "against": ["Ground"]}));
    assert_eq!(r["stopped_by"].as_str(), Some("loop"), "{r}");
    let at = num(&r["stopped_at"]);
    assert!(at > limit - 1.0 && at <= limit + 1e-6, "stopped at {at}°, the loop closes up to {limit}°");
}

/// A Watt six-bar: the four-bar above, its rocker a ternary link carrying a pin R0 = Q0 + 15 x,
/// and a second loop R–S–O6 (link 40, output 35, O6 = (90, 0)). The output link's box swings
/// into a stop.
#[test]
fn a_watt_six_bar_output_meets_a_stop() {
    let (mut s, _, q0) = four_bar();
    let o6 = Vec3::new(90.0, 0.0, 0.0);
    let r0 = q0 + Vec3::new(15.0, 0.0, 0.0);
    let [s0, _] = meet(r0, 40.0, o6, 35.0);
    for (name, c) in [("Link", (r0 + s0) * 0.5), ("Output", s0)] {
        part(&mut s, name, [c.x, c.y, -5.0]);
    }
    pin(&mut s, "revolute", "Rocker arm", "Rocker", "Link", r0, Vec3::Z);
    pin(&mut s, "revolute", "Output pin", "Link", "Output", s0, Vec3::Z);
    pin(&mut s, "revolute", "Output", "Ground", "Output", o6, Vec3::Z);
    // The output's turn with the crank at `v`°, both loops followed a quarter degree at a time.
    let output = |v: f64| {
        let n = ((v.abs() * 4.0).ceil() as usize).max(1);
        let mut sp = s0;
        for k in 1..=n {
            let (_, q) = four_bar_at(v * k as f64 / n as f64, q0);
            let r = turn(r0, O4, angle(q - O4) - angle(q0 - O4));
            let [m1, m2] = meet(r, 40.0, o6, 35.0);
            sp = if m1.dist(sp) <= m2.dist(sp) { m1 } else { m2 };
        }
        angle(sp - o6) - angle(s0 - o6)
    };
    // Where the solver puts the output at 30° is where the circles say.
    let check = run(&mut s.scratch(), "joint.drive", json!({"joint": "Crank", "value": 30}));
    assert!(check["stopped_by_loop"].is_null(), "{check}");
    let corner = Vec3::new(s0.x, s0.y, 0.0);
    let centre = turn(corner + Vec3::new(1.0, 1.0, 0.0), o6, output(60.0));
    stop_at(&mut s, centre);
    let stop = [(-1.5, -1.5), (1.5, -1.5), (1.5, 1.5), (-1.5, 1.5)].map(|(x, y)| centre + Vec3::new(x, y, 0.0));
    let expected = first_contact(60.0, |v| !overlap(&outline(corner, 2.0, o6, output(v)), &stop));
    let r =
        run(&mut s, "joint.sweep", json!({"joint": "Crank", "from": 0, "to": 90, "refine": 0.001, "occurrences": ["Output"], "against": ["Stop"]}));
    let at = num(&r["stopped_at"]);
    assert!((at - expected).abs() < 0.01, "stopped at {at}°, the output meets the stop at {expected}°: {r}");
    let names = [r["stopped_by"]["a"].as_str(), r["stopped_by"]["b"].as_str()];
    assert!(names.contains(&Some("Output:1")) && names.contains(&Some("Stop:1")), "{r}");
}
