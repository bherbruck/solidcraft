use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn len(v: &Value) -> usize {
    v.as_array().map_or(0, Vec::len)
}

/// A 40 x 30 x 5 plate (its own component) with a 6 mm hole at x = `hx` (a parameter, 20 mm),
/// and a pin (another component) held on the hole by a rigid joint.
fn pinned(s: &mut Session) {
    run(s, "parameters.add", json!({"name": "hx", "expression": "20 mm"}));
    run(s, "component.create", json!({"name": "Plate"}));
    run(s, "solid.box", json!({"length": 40, "width": 30, "height": 5, "corner": [0, 0, 0]}));
    run(s, "solid.hole", json!({"position": ["hx", 15, 5], "diameter": 6}));
    run(s, "component.activate", json!({"component": "root"}));
    run(s, "component.create", json!({"name": "Pin"}));
    run(s, "solid.cylinder", json!({"base": [100, 0, 0], "axis": [0, 0, 1], "radius": 3, "height": 12, "operation": "new"}));
    run(s, "component.activate", json!({"component": "root"}));
    run(
        s,
        "joint.create",
        json!({"type": "rigid", "a": {"occurrence": "Plate", "circle": [23, 15, 4.5]}, "b": {"occurrence": "Pin", "circle": [103, 0, 1]}, "name": "Seat"}),
    );
}

#[test]
fn a_sweep_reports_where_a_hole_and_its_joint_are_lost_and_leaves_the_design_alone() {
    let mut s = Session::default();
    pinned(&mut s);
    let r = run(&mut s, "inspect.parameter_sweep", json!({"parameter": "hx", "values": [10, "30 mm", 80]}));
    let results = r["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(results.len(), 3, "{r}");
    // Inside the plate the hole and the pin's joint hold.
    for v in &results[..2] {
        assert_eq!(v["ok"], json!(true), "{v}");
        assert!(v["ms"].as_f64().is_some_and(|ms| ms >= 0.0), "{v}");
    }
    // Off the plate the joint has no hole to seat on.
    let off = &results[2];
    assert_eq!(off["ok"], json!(false), "{off}");
    assert!(len(&off["joints_lost"]) + len(&off["failed"]) > 0, "{off}");
    assert_eq!((r["summary"]["values"].as_u64(), r["summary"]["ok"].as_u64()), (Some(3), Some(2)), "{r}");
    // The design is as it was.
    assert_eq!(s.doc.params.iter().find(|p| p.name == "hx").map(|p| p.expr.as_str()), Some("20 mm"));
    assert!(s.model.results.iter().all(|x| x.error.is_none()));
}

#[test]
fn a_sweep_reports_a_sketch_plane_that_turns_over() {
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "tilt", "expression": "30 deg"}));
    run(&mut s, "construct.plane.at_angle", json!({"base": "XY", "axis": "X", "angle": "tilt", "name": "Tilted"}));
    run(&mut s, "sketch.create", json!({"plane": "Tilted", "name": "OnTilted"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 5}));
    run(&mut s, "sketch.finish", json!({}));
    let r = run(&mut s, "inspect.parameter_sweep", json!({"parameter": "tilt", "from": 30, "to": 150, "steps": 2}));
    let results = r["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(results.len(), 3, "{r}");
    assert_eq!(results[1]["ok"], json!(true), "60° from where it is: {r}");
    let turned = &results[2]["sketches_flipped"];
    assert!(len(turned) == 1 && turned[0]["name"] == json!("OnTilted") && turned[0]["degrees"].as_f64().is_some_and(|d| d > 119.0), "{r}");
    assert_eq!(r["summary"]["sketches_flipped"].as_u64(), Some(1), "{r}");
}

#[test]
fn a_sweep_refuses_what_it_cannot_sweep() {
    let mut s = Session::default();
    pinned(&mut s);
    for bad in [
        json!({"parameter": "nope", "values": [1]}),
        json!({"parameter": "hx"}),
        json!({"parameter": "hx", "values": []}),
        json!({"parameter": "hx", "values": [null]}),
        json!({"parameter": "hx", "from": 1, "to": 2, "steps": 0}),
        json!({"parameter": "hx", "from": 1, "to": 2, "steps": 500}),
        json!({"parameter": "hx", "from": "a", "to": 2}),
    ] {
        assert!(s.execute("inspect.parameter_sweep", &bad).is_err(), "{bad}");
    }
}
