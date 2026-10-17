//! Guide rails through the commands (issue #771): solid.sweep and surface.sweep with one rail
//! or two, scaling or stretching, checked against analytic values; editing, naming, saving and
//! hostile values.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::PI;

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// A circle `C` (radius 10) on XY; a path `P` on XZ, a line 40 up the Z axis (`up`), with a rail
/// beside it (`in`) from the circle's edge at x = 10 in to x = 5 at the top; a line `L` across
/// XY from x = −10 to 10 (`l`); and in `R` on XZ two arcs from x = ±10 at the bottom to x = ±10
/// at the top, bulging out to x = ±20 halfway (radius 25 about (∓5, 0, 20)).
fn setup() -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "C"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 10, "id": "c"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "P"}));
    // The XZ sketch's y runs down world Z.
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [0, -40]], "ids": ["up"]}));
    run(&mut s, "sketch.line", json!({"points": [[10, 0], [5, -40]], "ids": ["in"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "L"}));
    run(&mut s, "sketch.line", json!({"points": [[-10, 0], [10, 0]], "ids": ["l"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "R"}));
    run(&mut s, "sketch.arc.three_point", json!({"start": [10, 0], "end": [10, -40], "through": [20, -20], "id": "right"}));
    run(&mut s, "sketch.arc.three_point", json!({"start": [-10, 0], "end": [-10, -40], "through": [-20, -20], "id": "left"}));
    run(&mut s, "sketch.finish", json!({}));
    s
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.world_state().body(body).unwrap().body).unwrap()
}

fn sweep(s: &mut Session, body: &str, extra: Value) -> Result<Value, String> {
    let mut p = json!({"sketch": "C", "path_sketch": "P", "path": ["up"], "operation": "new", "body_name": body});
    p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    s.execute("solid.sweep", &p).map_err(|e| e.to_string())
}

fn face_names(s: &mut Session, body: &str) -> Vec<String> {
    let f = run(s, "model.faces", json!({ "body": body }));
    let mut n: Vec<String> = f["faces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x["name"].as_str())
        .map(|x| x.split_once(':').map_or(x, |(_, rest)| rest).to_string())
        .collect();
    n.sort();
    n
}

/// A rail closing in from radius 10 to 5 sweeps the circle into a frustum (and its curve into
/// the frustum's side); the faces keep the plain sweep's names; switching the fit to stretch is
/// an edit (the circle then widens only toward the rail: an elliptic cone, sections πr²k); the
/// rails are saved with the feature.
#[test]
fn a_rail_sweeps_a_frustum_and_stretching_is_an_edit() {
    let mut s = setup();
    let volume = PI * 40.0 / 3.0 * (100.0 + 50.0 + 25.0);
    let lateral = PI * 15.0 * (40.0f64 * 40.0 + 25.0).sqrt();
    sweep(&mut s, "Plain", json!({})).unwrap();
    sweep(&mut s, "Frustum", json!({"rails": [{"curves": ["in"]}], "name": "Railed"})).unwrap();
    let m = measure(&s, "Frustum");
    assert!((m.volume - volume).abs() < 2e-3 * volume, "volume {} vs {volume}", m.volume);
    assert_eq!(face_names(&mut s, "Plain"), face_names(&mut s, "Frustum"));
    run(
        &mut s,
        "surface.sweep",
        json!({"sketch": "C", "curves": ["c"], "path_sketch": "P", "path": ["up"], "rails": [{"sketch": "P", "curves": ["in"]}], "body_name": "Side"}),
    );
    let a = measure(&s, "Side").area;
    assert!((a - lateral).abs() < 2e-3 * lateral, "side {a} vs {lateral}");

    let doc = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap();
    let reopened = Session::new(doc);
    assert!((measure(&reopened, "Frustum").volume - m.volume).abs() < 1e-9 * volume);

    // Stretched: k = 1 − z/80 across (x) only, so sections are π · 100 · k.
    run(&mut s, "timeline.edit", json!({"feature": "Railed", "set": {"rail_fit": "stretch"}}));
    let stretched = measure(&s, "Frustum");
    let elliptic = PI * 100.0 * 40.0 * 0.75;
    assert!((stretched.volume - elliptic).abs() < 2e-3 * elliptic, "stretched {} vs {elliptic}", stretched.volume);
    assert!((stretched.bbox.max.y - 10.0).abs() < 1e-3 && (stretched.bbox.min.y + 10.0).abs() < 1e-3, "{:?}", stretched.bbox);
}

/// Two arcs bulging apart carry a line between them: the sheet fills the region between the
/// arcs in the XZ plane, area 2(a√(R² − a²) + R² asin(a/R)) − 400 with R = 25, a = 20.
#[test]
fn two_rails_carry_a_line_between_arcs() {
    let mut s = setup();
    run(
        &mut s,
        "surface.sweep",
        json!({"sketch": "L", "curves": ["l"], "path_sketch": "P", "path": ["up"], "rails": [{"sketch": "R", "curves": ["left"]}, {"sketch": "R", "curves": ["right"]}], "body_name": "Between"}),
    );
    let m = measure(&s, "Between");
    let area = 2.0 * (20.0 * 15.0 + 625.0 * 0.8f64.asin()) - 400.0;
    assert!((m.area - area).abs() < 1e-3 * area, "area {} vs {area}", m.area);
    assert!((m.bbox.max.x - 20.0).abs() < 1e-2 && (m.bbox.min.x + 20.0).abs() < 1e-2, "{:?}", m.bbox);
}

/// Hostile rails are refused with an error and leave the design as it was.
#[test]
fn hostile_rails_are_refused() {
    let mut s = setup();
    let before = s.doc.features.len();
    let rail = json!([{"curves": ["in"]}]);
    for bad in [
        json!({"rails": "in"}),
        json!({"rails": []}),
        json!({"rails": [{"curves": ["in"]}, {"curves": ["in"]}, {"curves": ["in"]}]}),
        json!({"rails": [{"curves": []}]}),
        json!({"rails": [{}]}),
        json!({"rails": [{"curves": ["nope"]}]}),
        json!({"rails": [{"sketch": "Nowhere", "curves": ["in"]}]}),
        json!({"rails": rail, "rail_fit": "wobble"}),
        json!({"rails": rail, "twist": "10 deg"}),
        json!({"rails": rail, "scale": 2}),
        json!({"rails": rail, "orientation": "parallel"}),
        // A rail along the path itself: nothing to scale against.
        json!({"rails": [{"curves": ["up"]}]}),
        // Two rails starting at one point.
        json!({"rails": [{"curves": ["in"]}, {"curves": ["in"]}]}),
    ] {
        assert!(sweep(&mut s, "Bad", bad.clone()).is_err(), "{bad}");
        let mut p = json!({"sketch": "C", "curves": ["c"], "path_sketch": "P", "path": ["up"]});
        p.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        assert!(s.execute("surface.sweep", &p).is_err(), "surface {bad}");
        assert_eq!(s.doc.features.len(), before, "{bad}");
    }
}
