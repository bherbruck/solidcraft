use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// A 40 x 30 plate from z0, `t` thick, as its own body, with a through hole of `d` at (x, y).
fn plate(s: &mut Session, name: &str, z0: f64, t: f64, hole: Option<(f64, f64, f64)>) {
    run(s, "solid.box", json!({"length": 40, "width": 30, "height": t, "corner": [0, 0, z0], "operation": "new", "body_name": name}));
    if let Some((x, y, d)) = hole {
        run(s, "solid.hole", json!({"position": [x, y, z0 + t], "diameter": d, "depth": t}));
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn three_plates_with_coaxial_holes_are_one_stack() {
    let mut s = Session::default();
    plate(&mut s, "Bottom", 0.0, 2.0, Some((20.0, 15.0, 5.2)));
    plate(&mut s, "Middle", 2.0, 3.0, Some((20.0, 15.0, 5.2)));
    plate(&mut s, "Top", 5.0, 2.0, Some((20.0, 15.0, 5.2)));
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    let stacks = r["stacks"].as_array().cloned().unwrap_or_default();
    assert_eq!(stacks.len(), 1, "{r}");
    let st = &stacks[0];
    assert!(close(st["grip_mm"].as_f64().unwrap_or(0.0), 7.0), "{st}");
    // From the top down: −z, starting on the top plate's top face at the hole's centre.
    assert_eq!(st["axis"], json!([0.0, 0.0, -1.0]), "{st}");
    let start: Vec<f64> = st["start"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    assert!(close(start[0], 20.0) && close(start[1], 15.0) && close(start[2], 7.0), "{st}");
    let layers = st["layers"].as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = layers.iter().filter_map(|l| l["body"].as_str()).collect();
    assert_eq!(names, ["Top", "Middle", "Bottom"], "{st}");
    let t: Vec<(f64, f64)> = layers.iter().map(|l| (l["t"][0].as_f64().unwrap_or(-1.0), l["t"][1].as_f64().unwrap_or(-1.0))).collect();
    assert!(close(t[0].0, 0.0) && close(t[0].1, 2.0) && close(t[1].1, 5.0) && close(t[2].1, 7.0), "{t:?}");
    assert!(layers.iter().all(|l| close(l["d_mm"].as_f64().unwrap_or(0.0), 5.2)), "{st}");
    assert_eq!(r["lone"], json!([]), "{r}");
    // A diameter range that leaves the holes out finds nothing; one that takes them in finds them.
    let none = run(&mut s, "inspect.hole_stacks", json!({"diameters": [[6, 7]]}));
    assert_eq!((none["stacks"].as_array().map(Vec::len), none["lone"].as_array().map(Vec::len)), (Some(0), Some(0)), "{none}");
    let some = run(&mut s, "inspect.hole_stacks", json!({"diameters": [[5, 5.5]], "bodies": ["Top", "Middle"]}));
    let grip = some["stacks"][0]["grip_mm"].as_f64().unwrap_or(0.0);
    assert!(close(grip, 5.0), "{some}");
}

#[test]
fn a_hole_offset_by_two_tenths_is_no_stack() {
    let mut s = Session::default();
    plate(&mut s, "Lower", 0.0, 2.0, Some((20.0, 15.0, 5.0)));
    plate(&mut s, "Upper", 2.0, 2.0, Some((20.2, 15.0, 5.0)));
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    assert_eq!(r["stacks"], json!([]), "{r}");
    assert_eq!(r["lone"].as_array().map(Vec::len), Some(2), "{r}");
    // A looser tolerance lines them up.
    let loose = run(&mut s, "inspect.hole_stacks", json!({"axis_tolerance": 0.3}));
    assert_eq!(loose["stacks"].as_array().map(Vec::len), Some(1), "{loose}");
}

#[test]
fn a_round_hole_over_a_slot_is_a_stack() {
    let mut s = Session::default();
    plate(&mut s, "Slotted", 0.0, 3.0, None);
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 3}));
    run(&mut s, "sketch.slot.overall", json!({"p0": [14, 15], "p1": [26, 15], "width": 6}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 3, "direction": "negative", "operation": "cut"}));
    // The round hole sits off the slot's centre, over one part of its length.
    plate(&mut s, "Plate", 3.0, 2.0, Some((22.0, 15.0, 6.0)));
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    let layers = r["stacks"][0]["layers"].as_array().cloned().unwrap_or_default();
    assert_eq!(layers.len(), 2, "{r}");
    assert!(close(r["stacks"][0]["grip_mm"].as_f64().unwrap_or(0.0), 5.0), "{r}");
    let slot = &layers[1];
    assert_eq!(slot["body"], "Slotted", "{r}");
    assert!(close(slot["d_mm"].as_f64().unwrap_or(0.0), 6.0) && close(slot["slot"]["length_mm"].as_f64().unwrap_or(0.0), 12.0), "{slot}");
    assert!(close(slot["slot"]["along"][0].as_f64().unwrap_or(0.0).abs(), 1.0), "{slot}");
    // Without slots the round hole is alone.
    let r = run(&mut s, "inspect.hole_stacks", json!({"slots": false}));
    assert_eq!((r["stacks"].as_array().map(Vec::len), r["lone"].as_array().map(Vec::len)), (Some(0), Some(1)), "{r}");
}

#[test]
fn a_hole_in_one_plate_is_lone() {
    let mut s = Session::default();
    plate(&mut s, "Plate", 0.0, 4.0, Some((10.0, 10.0, 4.2)));
    // A plate on top without a hole: no stack either.
    plate(&mut s, "Cover", 4.0, 2.0, None);
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    assert_eq!(r["stacks"], json!([]), "{r}");
    let lone = r["lone"].as_array().cloned().unwrap_or_default();
    assert_eq!(lone.len(), 1, "{r}");
    assert_eq!(lone[0]["body"], "Plate");
    assert!(close(lone[0]["d_mm"].as_f64().unwrap_or(0.0), 4.2) && close(lone[0]["depth_mm"].as_f64().unwrap_or(0.0), 4.0), "{r}");
    let at: Vec<f64> = lone[0]["at"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    assert!(close(at[0], 10.0) && close(at[1], 10.0) && close(at[2], 4.0), "{r}");
}

#[test]
fn plates_apart_are_no_stack_and_bad_params_are_refused() {
    let mut s = Session::default();
    plate(&mut s, "A", 0.0, 2.0, Some((20.0, 15.0, 5.0)));
    plate(&mut s, "B", 2.5, 2.0, Some((20.0, 15.0, 5.0)));
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    assert_eq!(r["stacks"], json!([]), "{r}");
    let r = run(&mut s, "inspect.hole_stacks", json!({"touching": 0.6}));
    assert!(close(r["stacks"][0]["grip_mm"].as_f64().unwrap_or(0.0), 4.5), "{r}");
    for bad in
        [json!({"touching": -1}), json!({"diameters": [[5, 2]]}), json!({"diameters": 3}), json!({"slots": "yes"}), json!({"bodies": ["Nope"]})]
    {
        assert!(s.execute("inspect.hole_stacks", &bad).is_err(), "{bad}");
    }
}

/// A hole whose wall is a B-spline (cut by a loft between two equal circles, as the walls of
/// holes in refolded or imported parts can be) counts like an exact cylinder.
#[test]
fn a_b_spline_hole_wall_counts() {
    let mut s = Session::default();
    plate(&mut s, "Lower", 0.0, 3.0, Some((20.0, 15.0, 5.0)));
    plate(&mut s, "Upper", 3.0, 4.0, None);
    for (name, z) in [("S0", 3.0), ("S1", 7.0)] {
        run(&mut s, "sketch.create", json!({"plane": "XY", "offset": z, "name": name}));
        run(&mut s, "sketch.circle.center", json!({"center": [20, 15], "radius": 2.5}));
        run(&mut s, "sketch.finish", json!({}));
    }
    run(&mut s, "solid.loft", json!({"sections": [{"sketch": "S0"}, {"sketch": "S1"}], "operation": "cut", "targets": ["Upper"]}));
    let st = s.world_state();
    let upper = st.body("Upper").map(|b| b.body.clone()).unwrap_or_else(|| panic!("no Upper"));
    let analytic = (0..upper.face_count()).filter(|i| solidcraft_kernel::face_surface(&upper, *i).is_some()).count();
    assert!(analytic < upper.face_count(), "the loft's wall should be a B-spline");
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    let st = &r["stacks"][0];
    assert!((st["grip_mm"].as_f64().unwrap_or(0.0) - 7.0).abs() < 1e-3, "{r}");
    assert_eq!(st["layers"][0]["body"], "Upper", "{r}");
    assert!((st["layers"][0]["d_mm"].as_f64().unwrap_or(0.0) - 5.0).abs() < 1e-3, "{r}");
}

#[test]
fn a_slot_over_the_same_slot_is_a_stack() {
    let mut s = Session::default();
    for (name, z0) in [("Lower", 0.0), ("Upper", 2.0)] {
        plate(&mut s, name, z0, 2.0, None);
        run(&mut s, "sketch.create", json!({"plane": "XY", "offset": z0 + 2.0}));
        run(&mut s, "sketch.slot.overall", json!({"p0": [16, 15], "p1": [24, 15], "width": 5}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance": 2, "direction": "negative", "operation": "cut", "targets": [name]}));
    }
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    let st = &r["stacks"][0];
    assert_eq!((r["stacks"].as_array().map(Vec::len), r["lone"].clone()), (Some(1), json!([])), "{r}");
    assert!(close(st["grip_mm"].as_f64().unwrap_or(0.0), 4.0), "{st}");
    // The axis through the slots' middle, each layer with its slot.
    assert!(close(st["start"][0].as_f64().unwrap_or(0.0), 20.0) && close(st["start"][1].as_f64().unwrap_or(0.0), 15.0), "{st}");
    assert!(st["layers"].as_array().into_iter().flatten().all(|l| close(l["slot"]["length_mm"].as_f64().unwrap_or(0.0), 8.0)), "{st}");
}

#[test]
fn holes_on_a_nearly_diagonal_axis_are_one_stack() {
    // Two rings on a 45° axis, each drawn along its own axis a hair to either side of 45°, so
    // the two hole axes come out of the kernel pointing opposite ways.
    let mut s = Session::default();
    let k = std::f64::consts::FRAC_1_SQRT_2;
    for (name, axis, at) in [("A", [1.0, 0.0, -1.00001], 0.0), ("B", [1.00001, 0.0, -1.0], 1.5)] {
        let base = [at * k, 0.0, -at * k];
        run(&mut s, "solid.cylinder", json!({"radius": 6, "height": 1.5, "base": base, "axis": axis, "body_name": name}));
        let tool = format!("{name}_hole");
        let hb = [base[0] - k, 0.0, base[2] + k];
        run(&mut s, "solid.cylinder", json!({"radius": 1.7, "height": 3.5, "base": hb, "axis": axis, "body_name": tool}));
        run(&mut s, "solid.combine", json!({"target": name, "tools": [tool], "operation": "cut"}));
    }
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    assert_eq!((r["stacks"].as_array().map(Vec::len), r["lone"].clone()), (Some(1), json!([])), "{r}");
    assert!(close(r["stacks"][0]["grip_mm"].as_f64().unwrap_or(0.0), 3.0), "{r}");
}

#[test]
fn placed_standard_parts_are_no_layers() {
    let mut s = Session::default();
    plate(&mut s, "Lower", 0.0, 2.0, Some((20.0, 15.0, 6.4)));
    plate(&mut s, "Upper", 2.0, 2.0, Some((20.0, 15.0, 6.4)));
    // A washer lying on the top plate over the hole: its hole lines up but it is a part.
    run(&mut s, "parts.insert", json!({"family": "washer", "size": "M6", "point": [20, 15, 4]}));
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    assert_eq!(r["stacks"][0]["layers"].as_array().map(Vec::len), Some(2), "{r}");
    assert_eq!(r["lone"], json!([]), "{r}");
}

#[test]
fn a_folded_part_drilled_through_both_flanges_makes_a_stack_at_each() {
    // A channel whose two upright flanges are drilled together, one hole through both, with a
    // plate outside each flange: two stacks, one at each flange, none across the channel.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 60, "width": 30, "height": 25, "corner": [0, 0, 0], "operation": "new", "body_name": "Channel"}));
    run(&mut s, "solid.box", json!({"length": 57, "width": 30, "height": 23.5, "corner": [1.5, 0, 1.5], "operation": "cut", "targets": ["Channel"]}));
    run(&mut s, "solid.box", json!({"length": 1.5, "width": 30, "height": 25, "corner": [-1.5, 0, 0], "operation": "new", "body_name": "Left"}));
    run(&mut s, "solid.box", json!({"length": 1.5, "width": 30, "height": 25, "corner": [60, 0, 0], "operation": "new", "body_name": "Right"}));
    run(
        &mut s,
        "solid.cylinder",
        json!({"radius": 2.1, "height": 80, "base": [-10, 15, 15], "axis": [1, 0, 0], "operation": "cut", "targets": ["Channel", "Left", "Right"]}),
    );
    let r = run(&mut s, "inspect.hole_stacks", json!({}));
    let mut stacks: Vec<(f64, Vec<String>)> = r["stacks"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|st| {
                    let names = st["layers"]
                        .as_array()
                        .map(|l| l.iter().filter_map(|x| x["body"].as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    (st["grip_mm"].as_f64().unwrap_or(0.0), names)
                })
                .collect()
        })
        .unwrap_or_default();
    for st in &mut stacks {
        st.1.sort();
    }
    stacks.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(stacks, [(3.0, vec!["Channel".to_string(), "Left".to_string()]), (3.0, vec!["Channel".to_string(), "Right".to_string()])], "{r}");
}
