//! Cases found by the robustness runs (`solidcraft-cli fuzz`, docs/robustness.md), cut down to
//! the steps that matter, and a fixed set of seeds that must keep running clean.

use std::f64::consts::PI;

use serde_json::{Value, json};

use super::{run, volume};
use crate::Session;
use crate::fuzz;

/// Run a script; every command must succeed and every body be a sound solid.
fn script(steps: Value) -> Session {
    let mut s = Session::default();
    for st in steps.as_array().into_iter().flatten() {
        let (c, p) = (st["command"].as_str().unwrap_or(""), &st["params"]);
        if let Err(e) = s.execute(c, p) {
            panic!("{c} {p}: {e}");
        }
    }
    for b in &s.model.state().bodies {
        let bad = b.body.validity();
        assert!(bad.is_empty(), "{}: {bad:?}", b.name);
    }
    s
}

/// Shell of a notched plate closed by a shallow arc, opening a notch wall: the arc's cylinder
/// meets the plates at an angle, so its corners need more than one linear step to move.
#[test]
fn shell_with_a_shallow_arc_side() {
    script(json!([
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.line", "params": {"points": [[0, 0], [37.5, 0], [37.5, 37.5], [22.5, 37.5], [22.5, 20.0], [12.5, 20.0], [12.5, 37.5], [0, 37.5]]}},
        {"command": "sketch.arc.three_point", "params": {"end": [0, 0], "start": [0, 37.5], "through": [-5.0, 18.75]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": 7.5}},
        {"command": "solid.shell", "params": {"faces": [[12.5, 28.75, 3.75]], "thickness": 2.0}}
    ]));
}

/// Shell of a plate with a hole whose wall meets the plate's arc side: the edge where the two
/// cylinders meet is neither a line nor an arc, and is rebuilt through its moved points.
#[test]
fn shell_with_a_hole_through_a_curved_side() {
    script(json!([
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.line", "params": {"points": [[0, 0], [42.5, 0], [42.5, 20.0], [0, 20.0]]}},
        {"command": "sketch.arc.three_point", "params": {"end": [0, 0], "start": [0, 20.0], "through": [-10.0, 10.0]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": 27.5}},
        {"command": "solid.cylinder", "params": {"axis": [1.0, 0.0, 0.0], "base": [-10.000000000000002, 12.5, 12.5], "height": 17.5, "operation": "cut", "radius": 5.0}},
        {"command": "solid.shell", "params": {"faces": [[42.5, 10.0, 13.75]], "thickness": 2.0}}
    ]));
}

/// A cut whose end only touches the plate's arc side (#897, robustness seed 14): the end lies
/// tangent to the arc at its apex, the direct boolean fails, and the last-resort nudge used to
/// push the end into the arc, leaving a sliver of it a tenth of a millimetre wide that a shell
/// then couldn't offset. The nudge the other way parts them: the same faces as a cut clear of
/// the apex, and seed 14 shells and fillets again.
#[test]
fn cut_whose_end_touches_an_arc_side_leaves_no_sliver() {
    let plate = |x: f64| {
        script(json!([
            {"command": "sketch.create", "params": {"plane": "XY"}},
            {"command": "sketch.line", "params": {"points": [[0, 0], [42.5, 0], [42.5, 20.0], [0, 20.0]]}},
            {"command": "sketch.arc.three_point", "params": {"end": [0, 0], "start": [0, 20.0], "through": [-10.0, 10.0]}},
            {"command": "sketch.finish", "params": {}},
            {"command": "solid.extrude", "params": {"distance": 27.5}},
            {"command": "solid.cylinder", "params": {"axis": [1.0, 0.0, 0.0], "base": [x, 12.5, 12.5], "height": 17.5, "operation": "cut", "radius": 5.0}}
        ]))
    };
    let (touching, clear) = (plate(-10.0), plate(-10.1));
    let faces = |s: &Session| s.model.state().bodies.iter().map(|b| b.body.face_count()).sum::<usize>();
    assert_eq!(faces(&touching), faces(&clear), "a sliver face was left where the cut's end touches the arc");
    let r = fuzz::run(14, 8);
    let kinds: Vec<&str> = r.steps.iter().map(|s| s.kind).collect();
    assert!(r.outcome.kind == "ok" && kinds.contains(&"shell"), "seed 14: {:?} {kinds:?}\n{}", r.outcome, r.script());
}

/// #827: a hinge barrel joined to a laptop lid through its mounting leaf. The barrel's top lies
/// tangent to the lid's underside where the leaf ends, so the direct boolean fails, and the
/// last-resort nudge pushed the barrel into the lid and failed too; the nudge the other way
/// parts them and the join builds. The barrel adds three quarters of a cylinder (the quarter
/// inside the leaf was there already).
#[test]
fn hinge_barrel_joins_a_filleted_lid() {
    let all = json!([
        {"command": "solid.box", "params": {"length": 320, "width": 220, "height": 6, "corner": [-160, 4, 21], "body_name": "Lid Housing"}},
        {"command": "solid.fillet", "params": {"body": "Lid Housing", "edges": [[-160, 4, 24], [160, 4, 24], [-160, 224, 24], [160, 224, 24]], "radius": 5}},
        {"command": "solid.fillet", "params": {"body": "Lid Housing", "edges": [[0, 4, 27], [0, 224, 27], [-160, 114, 27], [160, 114, 27]], "radius": 1}},
        {"command": "solid.box", "params": {"length": 296, "width": 190, "height": 1.1, "corner": [-148, 17, 20.9], "operation": "cut", "targets": ["Lid Housing"]}},
        {"command": "solid.box", "params": {"length": 14, "width": 10, "height": 6, "corner": [-123.5, 214, 16], "operation": "join", "targets": ["Lid Housing"]}},
        {"command": "solid.cylinder", "params": {"radius": 5, "height": 14, "axis": "X", "base": [-123.5, 224, 16], "operation": "join", "targets": ["Lid Housing"]}}
    ]);
    let steps = all.as_array().unwrap();
    let before = total_volume(&script(Value::Array(steps[..5].to_vec())));
    let s = script(all.clone());
    assert_eq!(s.model.state().bodies.len(), 1);
    let added = total_volume(&s) - before;
    let want = 0.75 * PI * 5.0 * 5.0 * 14.0;
    assert!((added - want).abs() < want * 1e-3, "the barrel added {added} mm³, not {want}");
}

/// #871: the fillet from the report (its feature as saved) on a plate with a block on top and
/// a window through both. Radius 1 on the window's edges and on the block's sides overlaps in
/// the 1.26 mm wall between them; the result had openings in its end faces and was accepted.
/// It is refused now, and the part is left as it was.
#[test]
fn fillet_that_would_open_the_surface_is_refused() {
    let (a, b) = (12.74437365538526, 22.69275963366936);
    let mut s = script(json!([
        {"command": "solid.box", "params": {"length": 14, "width": 21.5, "height": 10, "corner": [9.5, 8, 0]}},
        {"command": "solid.box", "params": {"length": 14, "width": b - a, "height": 10, "corner": [9.5, a, 10], "operation": "join", "targets": ["Body1"]}},
        {"command": "solid.box", "params": {"length": 14, "width": 5.2, "height": 7.1, "corner": [9.5, 14, 7], "operation": "cut", "targets": ["Body1"]}}
    ]));
    let feature = json!({
        "type": "fillet",
        "edges": [
            [23.5, 12.74437365538526, 15.0], [23.5, 10.37218682769263, 10.0], [23.5, 8.0, 5.0], [23.5, 18.75, 0.0],
            [23.5, 29.5, 5.0], [23.5, 26.09637981683468, 10.0], [23.5, 22.69275963366936, 15.0], [23.5, 17.71856664452731, 20.0],
            [23.5, 16.6, 7.0], [23.5, 14.0, 10.55], [23.5, 16.6, 14.100000000000001], [23.5, 19.200000000000003, 10.55],
            [9.5, 17.71856664452731, 20.0], [9.5, 22.69275963366936, 15.0], [9.5, 26.09637981683468, 10.0], [9.5, 29.5, 5.0],
            [9.5, 18.75, 0.0], [9.5, 8.0, 5.0], [9.5, 10.37218682769263, 10.0], [9.5, 12.74437365538526, 15.0],
            [9.5, 19.200000000000003, 10.55], [9.5, 16.6, 14.100000000000001], [9.5, 14.0, 10.55], [9.5, 16.6, 7.0]
        ],
        "radius": "1"
    });
    let before = total_volume(&s);
    let result = s.execute("solid.fillet", &json!({"edges": feature["edges"], "radius": feature["radius"]}));
    assert!(result.is_err(), "a fillet with openings in the model was accepted");
    assert!((total_volume(&s) - before).abs() < 1e-9);
    for body in &s.model.state().bodies {
        assert!(body.body.validity().is_empty() && body.body.crossed_face_loops().unwrap() == 0);
    }
    // At half the radius the rounds fit.
    run(&mut s, "solid.fillet", json!({"edges": feature["edges"], "radius": "0.5"}));
    for body in &s.model.state().bodies {
        assert!(body.body.validity().is_empty() && body.body.crossed_face_loops().unwrap() == 0);
    }
}

/// Shell of a body in two pieces (a cylinder joined clear of a ring): each piece is shelled.
#[test]
fn shell_of_a_body_in_two_pieces() {
    script(json!([
        {"command": "sketch.create", "params": {"plane": "XZ"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [2.5, 0], "p1": [22.5, 17.5]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.revolve", "params": {"angle": "360 deg", "axis": "Z"}},
        {"command": "solid.cylinder", "params": {"axis": [0.0, 1.0, 0.0], "base": [-22.5, 22.5, 0.0], "height": 30.0, "operation": "join", "radius": 10.0}},
        {"command": "solid.shell", "params": {"faces": [[-22.5, 22.5, 0.0]], "thickness": 2.0}}
    ]));
}

/// A shell thicker than a step under a notch would turn the cavity inside out: a clear "not
/// supported yet" error instead of a failed boolean.
#[test]
fn shell_thicker_than_a_step_says_so() {
    let mut s = Session::default();
    for (c, p) in [
        ("solid.box", json!({"height": 7.5, "length": 30.0, "width": 35.0})),
        ("solid.box", json!({"corner": [0.0, 30.0, 2.5], "height": 22.5, "length": 22.5, "operation": "cut", "width": 20.0})),
    ] {
        s.execute(c, &p).unwrap();
    }
    let e = s.execute("solid.shell", &json!({"faces": [[18.75, 35.0, 2.5]], "thickness": 1.5})).unwrap_err();
    assert!(e.to_string().contains("not supported yet"), "{e}");
}

/// Panics inside the kernel are caught natively, but abort the web build (wasm can't unwind).
/// Building the samples must not panic at all (the Plastic Enclosure's bosses used to, 8 times).
#[test]
fn samples_build_without_panics() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let me = std::thread::current().id();
    let count = Arc::new(AtomicUsize::new(0));
    let c2 = count.clone();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == me {
            c2.fetch_add(1, Ordering::SeqCst);
        }
        let _ = info;
    }));
    let mut failed = Vec::new();
    for sample in crate::sample::samples() {
        let mut s = Session::default();
        for c in (sample.script)()["commands"].as_array().into_iter().flatten() {
            let _ = s.execute(c["command"].as_str().unwrap_or(""), &c["params"]);
        }
        let n = count.swap(0, Ordering::SeqCst);
        if n > 0 {
            failed.push(format!("{}: {n} panic(s)", sample.name));
        }
    }
    std::panic::set_hook(prev);
    assert!(failed.is_empty(), "{failed:?}");
}

fn total_volume(s: &Session) -> f64 {
    s.model.state().bodies.iter().map(|b| solidcraft_kernel::measure(&b.body).map(|m| m.volume).unwrap_or(0.0)).sum()
}

/// Volume of a box L×W×H with its vertical edges rounded (R) and its bottom loop filleted
/// (ρ ≤ R): the straight runs lose (1 − π/4)ρ² each, the corners a quarter turn of that
/// (Pappus, about the corner's axis).
fn rounded_box(l: f64, w: f64, h: f64, r: f64, rho: f64) -> f64 {
    use std::f64::consts::PI;
    let a = (1.0 - PI / 4.0) * rho * rho;
    let xbar = rho * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI));
    l * w * h - (4.0 - PI) * r * r * h - a * (2.0 * (l - 2.0 * r) + 2.0 * (w - 2.0 * r)) - 2.0 * PI * (r - xbar) * a
}

/// Box, fillets, shell: a 60×40×30 box with its vertical edges rounded r5 and its bottom loop
/// r3, shelled 2 mm with the top open. The bottom round runs round the corners as spindle tori
/// (the tube wider than its circle), which the offset now takes.
#[test]
fn shell_of_a_filleted_box() {
    let s = script(json!([
        {"command": "solid.box", "params": {"length": 60, "width": 40, "height": 30}},
        {"command": "solid.fillet", "params": {"edges": [[0, 0, 15], [60, 0, 15], [60, 40, 15], [0, 40, 15]], "radius": 5}},
        {"command": "solid.fillet", "params": {"edges": [[30, 0, 0], [60, 20, 0], [30, 40, 0], [0, 20, 0]], "radius": 3}},
        {"command": "solid.shell", "params": {"faces": [[30, 20, 30]], "thickness": 2}}
    ]));
    let want = rounded_box(60.0, 40.0, 30.0, 5.0, 3.0) - rounded_box(56.0, 36.0, 28.0, 3.0, 1.0);
    let v = total_volume(&s);
    assert!((v - want).abs() < 1e-4 * want, "{v} vs {want}");
}

/// The first tutorial part: a 60×40×15 plate with its top front edge and front right edge
/// rounded r2, shelled with the top open. At 2 mm the rounds' inner sides collapse to sharp
/// edges (as in Fusion); at 1.5 mm the top's round, tangent to the open top, keeps its
/// tangency (every face moves in, the opening is swept out through the top).
#[test]
fn shell_of_the_tutorial_plate() {
    let base = json!([
        {"command": "solid.box", "params": {"length": 60, "width": 40, "height": 15}},
        {"command": "solid.fillet", "params": {"edges": [[30, 0, 15], [60, 0, 7]], "radius": 2}}
    ]);
    let s0 = script(base.clone());
    let body = total_volume(&s0);
    // 2 mm: the cavity is a sharp 56 × 36 box from z = 2 up through the top.
    let mut steps = base.as_array().cloned().unwrap_or_default();
    steps.push(json!({"command": "solid.shell", "params": {"faces": [[30, 20, 15]], "thickness": 2}}));
    let v = total_volume(&script(Value::Array(steps)));
    let want = body - 56.0 * 36.0 * 13.0;
    assert!((v - want).abs() < 1e-4 * want, "{v} vs {want}");
    // 1.5 mm: a 57 × 37 × 12 cavity with r0.5 rounds, opened by its top face (57 × 36.5)
    // swept up through the top (estimated: the rounds' mitre is left out, hence 1e-3).
    let mut steps = base.as_array().cloned().unwrap_or_default();
    steps.push(json!({"command": "solid.shell", "params": {"faces": [[30, 20, 15]], "thickness": 1.5}}));
    let v = total_volume(&script(Value::Array(steps)));
    let a = (1.0 - std::f64::consts::PI / 4.0) * 0.25;
    let want = body - (57.0 * 37.0 * 12.0 - a * (57.0 + 12.0)) - 57.0 * 36.5 * 1.5;
    assert!((v - want).abs() < 1e-3 * want, "{v} vs {want}");
}

/// Shell after a drilled hole (its tip a cone): the cone moves like the other faces.
#[test]
fn shell_after_a_drilled_hole() {
    for open in [[30.0, 0.0, 7.5], [10.0, 10.0, 15.0]] {
        script(json!([
            {"command": "solid.box", "params": {"length": 60, "width": 40, "height": 15}},
            {"command": "solid.hole", "params": {"position": [30, 20, 15], "diameter": 5, "depth": 8, "type": "drilled"}},
            {"command": "solid.shell", "params": {"faces": [open], "thickness": 2}}
        ]));
    }
}

/// Rounding a box's edges one at a time: an edge running into an earlier round (of the same
/// radius or another) takes off about what each round alone would (the corner where they meet
/// is small).
#[test]
fn fillet_into_an_earlier_round() {
    let box_ = json!({"command": "solid.box", "params": {"length": 60, "width": 40, "height": 15}});
    for r2 in [2.0, 3.0] {
        let s = script(json!([
            box_,
            {"command": "solid.fillet", "params": {"edges": [[30, 0, 15]], "radius": 2}},
            {"command": "solid.fillet", "params": {"edges": [[60, 0, 7]], "radius": r2}}
        ]));
        let lost = |r: f64, len: f64| (1.0 - PI / 4.0) * r * r * len;
        let want = 36000.0 - lost(2.0, 60.0) - lost(r2, 15.0);
        let v = total_volume(&s);
        assert!((v - want).abs() < 2.0, "r2 {r2}: {v} vs {want}");
    }
}

/// Clean seeds (seed 16: a partial face contact, fused), and the original seed 5 edge contact,
/// now rejected without distortion.
#[test]
fn fixed_seeds_run_clean() {
    for seed in FIXED_SEEDS {
        if *seed == 5 {
            // Seed 5 originally joined these boxes along only an edge. Accepting
            // it required silently shifting the tool; that is now invalid input.
            let mut s = Session::default();
            run(&mut s, "solid.box", json!({"length": 25, "width": 15, "height": 10}));
            let error = s
                .execute(
                    "solid.box",
                    &json!({
                        "corner": [25, 2.5, 10], "length": 17.5, "width": 17.5,
                        "height": 20, "operation": "join"
                    }),
                )
                .unwrap_err();
            assert!(error.to_string().contains("zero-thickness edge contact"), "{error}");
            assert!((total_volume(&s) - 3750.0).abs() < 1e-6);
            assert_eq!(s.model.state().bodies.len(), 1);
            assert!(s.model.state().bodies.first().unwrap().body.validity().is_empty());
            continue;
        }
        if *seed == 16 {
            // Seed 16 patterns a short cylinder around a block: one copy's end lies on a side of
            // the block with half of it past the block's bottom edge (#674). The end and the side
            // are split along each other's outline and the half disc they share goes: the block,
            // half of the first cylinder (the other half is inside the block) and three whole ones.
            let mut s = Session::default();
            run(&mut s, "solid.box", json!({"length": 25.0, "width": 47.5, "height": 37.5}));
            run(
                &mut s,
                "solid.cylinder",
                json!({"axis": [1.0, 0.0, 0.0], "base": [0.0, 7.5, 0.0], "height": 2.5, "operation": "join", "radius": 5.0}),
            );
            run(&mut s, "solid.pattern.circular", json!({"axis": "Z", "count": 4, "features": ["Cylinder1"]}));
            // (Within the measured round sides' mesh error, about 0.03 mm³ a cylinder.)
            let want = 25.0 * 47.5 * 37.5 + 3.5 * PI * 25.0 * 2.5;
            assert!((total_volume(&s) - want).abs() < 0.2, "{} vs {want}", total_volume(&s));
            assert!(s.model.state().bodies.iter().all(|b| b.body.validity().is_empty()));
            let r = fuzz::run(*seed, 8);
            assert!(!r.outcome.is_bug(), "seed {seed}: {:?}\n{}", r.outcome, r.script());
            continue;
        }
        let r = fuzz::run(*seed, 8);
        assert!(!r.outcome.is_bug(), "seed {seed}: {:?}\n{}", r.outcome, r.script());
    }
}

const FIXED_SEEDS: &[u64] = &[3, 5, 11, 16, 21, 29, 33, 35];

/// A drilled hole as deep as the plate: the drill point breaks through the bottom.
#[test]
fn hole_as_deep_as_the_plate() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 50, "width": 25, "height": 10}));
    let v0 = volume(&mut s);
    run(&mut s, "solid.hole", json!({"position": [25, 12.5, 10], "diameter": 6, "depth": 10, "type": "drilled"}));
    let v = volume(&mut s);
    assert!(v < v0 - PI * 9.0 * 9.0 && v > v0 - PI * 9.0 * 10.0 - 1.0, "{v0} {v}");
    // Other sizes and places, the defaults, and a tapped hole.
    for (i, p) in [
        json!({"position": [10, 10, 10], "diameter": 5, "depth": 10, "type": "drilled"}),
        json!({"position": [40, 15, 10], "diameter": 5, "depth": 10}),
        json!({"position": [12, 6, 10], "diameter": 8.5, "depth": 10}),
        json!({"position": [30, 7, 10], "thread": "M6", "depth": 10}),
        json!({"position": [20, 18, 10], "diameter": 4, "depth": 10, "tip_angle": 90}),
    ]
    .into_iter()
    .enumerate()
    {
        let before = volume(&mut s);
        let r = s.execute("solid.hole", &p);
        assert!(r.is_ok(), "{i} {p}: {r:?}");
        assert!(volume(&mut s) < before - 1.0, "{i} {p}");
    }
}

/// An enclosure built in the usual order: shell, bosses, then the lip on the rim.
#[test]
fn lip_after_boss() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 80, "width": 50, "height": 30}));
    run(&mut s, "solid.shell", json!({"faces": [[40, 25, 30]], "thickness": 2}));
    run(&mut s, "plastic.boss", json!({"position": [10, 10, 2], "diameter": 7, "height": 12, "hole_diameter": 2.5}));
    run(&mut s, "plastic.boss", json!({"position": [70, 40, 2], "diameter": 7, "height": 12, "hole_diameter": 2.5}));
    let v0 = volume(&mut s);
    run(&mut s, "plastic.lip", json!({"face": [1, 25, 30], "width": 1, "height": 2}));
    let v = volume(&mut s);
    assert!(v > v0 + 1.0, "{v0} {v}");
    // Bosses up to the rim, with ribs, against a wall; lips, grooves and outside rims.
    let bosses = [
        json!({"position": [15, 15, 2], "diameter": 8, "height": 28, "hole_diameter": 3}),
        // Ribs out to the inner walls (15 − 4 − 9 = 2), and short of them.
        json!({"position": [15, 15, 2], "diameter": 8, "height": 12, "ribs": 4, "rib_thickness": 1.5, "rib_length": 9}),
        json!({"position": [15, 15, 2], "diameter": 8, "height": 12, "ribs": 4, "rib_thickness": 1.5, "rib_length": 6}),
        json!({"position": [5.5, 25, 2], "diameter": 7, "height": 12, "hole_diameter": 2.5}),
        json!({"position": [40, 25, 2], "diameter": 8, "height": 20, "draft": "1 deg", "fillet": 1}),
    ];
    let lips = [
        json!({"face": [1, 25, 30], "width": 1, "height": 2}),
        json!({"face": [1, 25, 30], "width": 1, "height": 2, "type": "groove"}),
        json!({"face": [1, 25, 30], "width": 1, "height": 2, "side": "outside"}),
    ];
    for (i, b) in bosses.iter().enumerate() {
        for (j, l) in lips.iter().enumerate() {
            let mut s = Session::default();
            run(&mut s, "solid.box", json!({"length": 80, "width": 50, "height": 30}));
            run(&mut s, "solid.shell", json!({"faces": [[40, 25, 30]], "thickness": 2}));
            let r = s.execute("plastic.boss", b);
            assert!(r.is_ok(), "boss {i}: {r:?}");
            let v0 = volume(&mut s);
            let r = s.execute("plastic.lip", l);
            assert!(r.is_ok(), "boss {i} lip {j}: {r:?}");
            assert!((volume(&mut s) - v0).abs() > 1.0, "boss {i} lip {j}");
        }
    }
}

/// A lip, a groove and an outside lip on the rim of a rounded, shelled enclosure (#29: the
/// lip's walls continue the shell's walls, which a boolean join couldn't do).
#[test]
fn lip_on_a_rounded_shelled_rim() {
    use std::f64::consts::PI;
    let base = json!([
        {"command": "solid.box", "params": {"corner": [-40, -30, 0], "length": 80, "width": 60, "height": 30}},
        {"command": "solid.fillet", "params": {"edges": [[40, 30, 15], [-40, 30, 15], [-40, -30, 15], [40, -30, 15]], "radius": 6}},
        {"command": "solid.shell", "params": {"faces": [[0, 0, 30]], "thickness": 2}}
    ]);
    let v0 = total_volume(&script(base.clone()));
    let ar = |w: f64, h: f64, r: f64| w * h - (4.0 - PI) * r * r;
    let inside = ar(78.0, 58.0, 5.0) - ar(76.0, 56.0, 4.0);
    for (extra, want) in [
        (json!({}), v0 + inside * 1.5),
        (json!({"side": "outside"}), v0 + (ar(80.0, 60.0, 6.0) - ar(78.0, 58.0, 5.0)) * 1.5),
        // (The groove's band comes from the rim's meshed outline: within its sag.)
        (json!({"type": "groove"}), v0 - inside * 1.5),
    ] {
        let mut p = json!({"face": [0, 29, 30], "width": 1, "height": 1.5});
        if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        let mut steps = base.as_array().cloned().unwrap_or_default();
        steps.push(json!({"command": "plastic.lip", "params": p}));
        let v = total_volume(&script(Value::Array(steps)));
        assert!((v - want).abs() < 1e-3 * want, "{extra}: {v} vs {want}");
    }
}

/// The #851 part: a wall of radius 21 meets the front face square and runs out tangent into the
/// back face, in a knife edge at (62⅙, 0). Its two blends below are checked against the volumes
/// of the swept corner (the wall's angle integrated over the section), through the display plan.
fn knife_edge_part() -> Session {
    script(json!([
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [41.5, -21]}},
        {"command": "sketch.dimension", "params": {"entities": ["l2"], "text_at": [36.5, -8.5], "value": "21 mm"}},
        {"command": "sketch.line", "params": {"infer": true, "points": ["p2", [61.5, -21]]}},
        {"command": "sketch.dimension", "params": {"entities": ["l5"], "text_at": [54.5, -26.5], "value": "21"}},
        {"command": "sketch.circle.center", "params": {"center": "l5.end", "radius": 21}},
        {"command": "sketch.line", "params": {"infer": true, "points": ["p3", [62.166666666666664, 0]]}},
        {"command": "sketch.constraint.tangent", "params": {"a": "c1", "b": "l6"}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"direction": "positive", "distance": "10 mm", "operation": "new", "profiles": [0, 1], "sketch": 1}}
    ]))
}

/// The volume of the session's bodies through the display plan.
fn planned_volume(s: &Session) -> f64 {
    s.model.state().bodies.iter().map(|b| b.body.display_plan(2e-4).unwrap().evaluate().measure().volume).sum()
}

/// #851: the knife edge where the wall runs out has opposite normals on its two faces. It rounds
/// to the ball that fits there and chamfers with the plane through that ball's contacts (the cusp
/// area less the wall's sector and the ball's, or the chord triangle less the wall's segment,
/// times the 10 mm height).
#[test]
fn blend_a_knife_edge_where_a_curved_wall_runs_out() {
    let knife = json!([[62.1667, 0, 5]]);
    for (command, params, removed) in
        [("solid.fillet", json!({"edges": knife, "radius": 2}), 119.648_812), ("solid.chamfer", json!({"edges": knife, "distance": 2}), 159.236_451)]
    {
        let mut s = knife_edge_part();
        let before = planned_volume(&s);
        s.execute(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
        let after = planned_volume(&s);
        assert!((before - after - removed).abs() < 0.5, "{command}: removed {}, expected {removed}", before - after);
        for b in &s.model.state().bodies {
            assert!(b.body.validity().is_empty(), "{command}: {:?}", b.body.validity());
        }
    }
}

/// #851: the arc's top edge runs out tangent to the back face at the knife edge's top. The blend
/// is cut by that face along a curve that touches the wall's contact at the tip.
#[test]
fn blend_an_arc_edge_that_runs_out_tangent_to_a_face() {
    let arc = json!([[47.3174, -6.1508, 10]]);
    for (command, params, removed) in
        [("solid.fillet", json!({"edges": arc, "radius": 2}), 25.506), ("solid.chamfer", json!({"edges": arc, "distance": 2}), 58.077)]
    {
        let mut s = knife_edge_part();
        let before = planned_volume(&s);
        s.execute(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
        let after = planned_volume(&s);
        assert!((before - after - removed).abs() < 0.05, "{command}: removed {}, expected {removed}", before - after);
        for b in &s.model.state().bodies {
            assert!(b.body.validity().is_empty(), "{command}: {:?}", b.body.validity());
        }
    }
}

/// #851: every selection of the issue's table works, picked together or one after the other as
/// the issue lists them, on the part with the wall that meets the front face square and runs out
/// tangent into the back face (the top front edge, the arc's top edge, the front upright at the
/// arc, the front-left upright and the knife edge at the tip). Each makes a sound solid the
/// display plan can mesh, and takes less than its edges would one by one where they meet.
#[test]
fn blend_corners_where_edges_meet_on_a_curved_wall() {
    let (tf, at, fu, fl, kn) =
        (json!([20, -21, 10]), json!([47.3174, -6.1508, 10]), json!([41.1667, -21, 5]), json!([0, -21, 5]), json!([62.1667, 0, 5]));
    let fillet = |edges: Value| json!({"command": "solid.fillet", "params": {"edges": edges, "radius": 2}});
    let chamfer = |edges: Value| json!({"command": "solid.chamfer", "params": {"edges": edges, "distance": 2}});
    let base = planned_volume(&knife_edge_part());
    let rows: Vec<(&str, Vec<Value>)> = vec![
        ("three at the corner", vec![fillet(json!([tf, at, fu]))]),
        ("top front and arc", vec![fillet(json!([tf, at]))]),
        ("arc and upright", vec![fillet(json!([at, fu]))]),
        ("top front and upright", vec![fillet(json!([tf, fu]))]),
        ("arc alone", vec![fillet(json!([at]))]),
        ("top front, then the front-left upright", vec![fillet(json!([tf])), fillet(json!([fl]))]),
        ("top front and front-left together", vec![fillet(json!([tf, fl]))]),
        ("the knife edge", vec![fillet(json!([kn]))]),
        ("chamfer: the knife edge", vec![chamfer(json!([kn]))]),
        ("chamfer: the arc", vec![chamfer(json!([at]))]),
        ("chamfer: the arc and the top front", vec![chamfer(json!([at, tf]))]),
        ("arc, then top front", vec![fillet(json!([at])), fillet(json!([tf]))]),
        ("top front, then upright", vec![fillet(json!([tf])), fillet(json!([fu]))]),
    ];
    for (name, steps) in rows {
        let mut s = knife_edge_part();
        for step in &steps {
            let (c, p) = (step["command"].as_str().unwrap_or(""), &step["params"]);
            s.execute(c, p).unwrap_or_else(|e| panic!("{name}: {c} {p}: {e}"));
        }
        for b in &s.model.state().bodies {
            assert!(b.body.validity().is_empty(), "{name}: {:?}", b.body.validity());
            assert!(b.body.display_plan(2e-4).is_ok(), "{name}: the display plan refuses the body");
        }
        let removed = base - planned_volume(&s);
        assert!(removed > 0.0 && removed < 250.0, "{name}: removed {removed}");
    }
    // The three edges end in a sphere: the volume of the rounds and the ball's corner.
    let mut s = knife_edge_part();
    s.execute("solid.fillet", &fillet(json!([tf, at, fu]))["params"]).unwrap_or_else(|e| panic!("three at the corner: {e}"));
    let removed = base - planned_volume(&s);
    assert!((removed - 67.05).abs() < 0.15, "three at the corner: removed {removed}, expected 67.05");
}

/// Seed 20 cuts a patterned cylinder's holes through a filleted plate. The lenient retry of a
/// face's parameter boundary (#970) searched the whole cylinder for a point and found it across
/// the seam, half a turn away; the face was divided wrongly and the cut failed. Such a boundary
/// is refused now, and the cut builds as before #970.
#[test]
fn a_boundary_found_across_a_seam_is_not_used() {
    let r = fuzz::run(20, 8);
    assert_eq!(r.outcome.kind, "ok", "seed 20: {:?}\n{}", r.outcome, r.script());
    assert!(r.steps.iter().any(|s| s.kind == "circular pattern"), "the pattern must build, not be skipped as unsupported: {:?}", r.unsupported);
}

/// Seed 33 ends in a fillet on its second body (#854's corner blends build it differently from
/// before). Its display plan and its measured volume agree for every body (on main before #854,
/// that body read 4439.42 through the display plan and 4443.51 measured).
#[test]
fn seed_33_display_plan_and_measure_agree() {
    let r = fuzz::run(33, 8);
    assert_eq!(r.outcome.kind, "ok", "seed 33: {:?}\n{}", r.outcome, r.script());
    let s = script(r.script());
    for b in &s.model.state().bodies {
        let planned = b.body.display_plan(2e-4).unwrap().evaluate().measure().volume;
        let measured = solidcraft_kernel::measure(&b.body).unwrap().volume;
        assert!((planned - measured).abs() < 2e-5 * measured, "{}: planned {planned}, measured {measured}", b.name);
    }
}
