//! A part ring cut by `solid.combine` (two prisms through its ends, a notch through its chamfer)
//! measures what it encloses, wherever it is moved. The reference mesher left a crack where the
//! notch meets the chamfer's cone; the volume beside it was left out (0.2 %), and integrated
//! about the world origin the loss grew with the distance (7 % moved 1000 mm away).
use serde_json::{Value, json};

use crate::*;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn volume(s: &mut Session, body: &str) -> f64 {
    run(s, "inspect.measure", json!({"bodies": [body]}))["total"]["volume_mm3"].as_f64().unwrap()
}

fn prism(s: &mut Session, offset: f64, points: Value, distance: f64, name: &str) {
    run(s, "sketch.create", json!({"plane": "XY", "offset": offset}));
    run(s, "sketch.line", json!({"points": points, "closed": true}));
    run(s, "sketch.finish", json!({}));
    run(s, "solid.extrude", json!({"distance": distance, "operation": "new", "body_name": name}));
}

/// A part ring (210° of a chamfered section) less two oblique prisms and a notch.
fn cut_ring(s: &mut Session) {
    let (c, si) = (15f64.to_radians().cos(), 15f64.to_radians().sin());
    run(s, "sketch.create", json!({"plane": {"origin": [0, 0, 0], "x_dir": [si, c, 0], "y_dir": [0, 0, -1]}}));
    let section = json!([[16.6, 0], [17.5, 0], [18.5, -1.0], [18.5, -7.0], [17.5, -8.0], [16.6, -8.0], [16.4, -7.8], [16.4, -0.2]]);
    run(s, "sketch.line", json!({"points": section, "closed": true}));
    run(s, "sketch.finish", json!({}));
    run(s, "solid.revolve", json!({"axis": "Z", "angle": "210 deg", "body_name": "Ring"}));
    let quad = |sy: f64| json!([[4.644832, 15.101839 * sy], [5.17399, 16.822301 * sy], [4.555215, 17.000295 * sy], [2.187187, 15.574386 * sy]]);
    prism(s, -1.0, quad(1.0), 10.0, "T0");
    prism(s, -1.0, quad(-1.0), 10.0, "T1");
    prism(s, 7.1, json!([[-17.6, 0.45], [-19.5, 0.45], [-19.5, -0.45], [-17.6, -0.45]]), 1.9, "N0");
    run(s, "solid.combine", json!({"target": "Ring", "tools": ["T0", "T1", "N0"], "operation": "cut"}));
}

#[test]
fn combine_cut_volume_does_not_depend_on_placement() {
    let mut s = Session::default();
    cut_ring(&mut s);
    let here = volume(&mut s, "Ring");
    // The 210° ring by Pappus less what the prisms and the notch take (a fine grid quadrature
    // of each, independent of the kernel's meshes): 1005.489 − 2 × 5.639 − 0.287.
    let want = 993.924;
    assert!((here - want).abs() <= 2e-4 * want, "at the origin: {here} vs {want}");
    let mut moved = Vec::new();
    for by in [[100.0, 0.0, 0.0], [900.0, 0.0, 0.0], [-1000.0, 100.0, 0.0], [0.0, 0.0, 10000.0], [0.0, -10000.0, 0.0]] {
        run(&mut s, "solid.move", json!({"bodies": ["Ring"], "translate": by}));
        moved.push((by, volume(&mut s, "Ring")));
    }
    for (by, v) in moved {
        assert!((v - here).abs() <= 1e-6 * here, "after a move by {by:?}: {v} vs {here} at the origin");
    }
}
