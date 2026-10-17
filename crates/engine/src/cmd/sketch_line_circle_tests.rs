//! Sketch Dimension from a line to the edge of a circle or arc (#1803): `sketch.dimension` with
//! a line and a circle or arc measures the gap to the edge (or how far the edge reaches past a
//! line that cuts it), drives it, follows its parameter, and survives undo and reopening.

use serde_json::{Value, json};
use solidcraft_geom::Vec2;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn sketch(s: &Session) -> solidcraft_sketch::Sketch {
    s.model.state().sketch(s.active_sketch.unwrap()).unwrap().sketch.clone()
}

fn point(s: &Session, id: &str) -> Vec2 {
    let sk = sketch(s);
    sk.resolve_point(id).and_then(|p| sk.point(p)).unwrap()
}

/// A fixed line along x from (0, 0) to (40, 0) (`l1`) and a circle of diameter 10 at `centre`
/// (`c1`).
fn line_and_circle(centre: [f64; 2]) -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [40, 0]]}));
    run(&mut s, "sketch.constraint.fix", json!({"entities": ["l1"]}));
    run(&mut s, "sketch.circle.center", json!({"center": centre, "radius": 5}));
    run(&mut s, "sketch.dimension", json!({"entities": ["c1"], "type": "diameter", "value": 10}));
    s
}

#[test]
fn a_line_and_a_circle_dimension_to_its_edge() {
    let mut s = line_and_circle([20.0, 15.0]);
    // Centre 15 above the line, radius 5: the gap to the edge is 10.
    let r = run(&mut s, "sketch.dimension", json!({"entities": ["l1", "c1"]}));
    assert_eq!(r["type"], "LinearDimension", "{r}");
    assert_eq!(r["measured"], 10.0, "{r}");
    let param = r["param"].as_str().unwrap().to_string();
    assert_eq!(r["expression"], "10 mm", "{r}");
    // The parameter drives the gap: 7 puts the centre 7 + 5 = 12 above the line.
    run(&mut s, "parameters.change", json!({"name": param, "expression": "7 mm"}));
    assert!(point(&s, "c1.center").dist(Vec2::new(20.0, 12.0)) < 1e-7, "{:?}", point(&s, "c1.center"));
    // The circle and the line in the other order, with a value.
    let mut s = line_and_circle([20.0, 15.0]);
    let r = run(&mut s, "sketch.dimension", json!({"entities": ["c1", "l1"], "value": 3}));
    assert_eq!(r["measured"], 3.0, "{r}");
    assert!(point(&s, "c1.center").dist(Vec2::new(20.0, 8.0)) < 1e-7, "{:?}", point(&s, "c1.center"));
}

#[test]
fn a_line_that_cuts_the_circle_dimensions_how_far_the_edge_reaches_past_it() {
    // Centre 3 above the line, radius 5: the edge reaches 2 below the line.
    let mut s = line_and_circle([20.0, 3.0]);
    let r = run(&mut s, "sketch.dimension", json!({"entities": ["l1", "c1"]}));
    assert_eq!(r["measured"], 2.0, "{r}");
    let param = r["param"].as_str().unwrap().to_string();
    run(&mut s, "parameters.change", json!({"name": param, "expression": "4"}));
    // Reaching 4 past the line leaves the centre 1 above it; it never crosses the line.
    assert!(point(&s, "c1.center").dist(Vec2::new(20.0, 1.0)) < 1e-7, "{:?}", point(&s, "c1.center"));
}

#[test]
fn an_arc_dimensions_to_its_circle() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [40, 0]]}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [20, 20], "start": [26, 20], "end": [14, 20]}));
    let r = run(&mut s, "sketch.dimension", json!({"entities": ["a1", "l1"]}));
    assert_eq!(r["measured"], 14.0, "{r}");
}

#[test]
fn the_dimension_undoes_and_reopens() {
    let mut s = line_and_circle([20.0, 15.0]);
    let before = sketch(&s).constraints.len();
    run(&mut s, "sketch.dimension", json!({"entities": ["l1", "c1"], "value": 6}));
    let saved = s.doc.to_json();
    assert!(saved.contains("\"line_circle_distance\""), "not saved");
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&saved).unwrap());
    run(&mut reopened, "sketch.edit", json!({"sketch": "Sketch1"}));
    assert!(point(&reopened, "c1.center").dist(Vec2::new(20.0, 11.0)) < 1e-7, "{:?}", point(&reopened, "c1.center"));
    let kinds = |s: &Session| sketch(s).constraints.iter().map(|c| c.kind.clone()).collect::<Vec<_>>();
    assert_eq!(kinds(&reopened), kinds(&s));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(sketch(&s).constraints.len(), before);
}

#[test]
fn a_gap_the_sketch_already_has_is_refused_or_driven() {
    // A tangent circle already has a gap of zero.
    let mut s = line_and_circle([20.0, 5.0]);
    run(&mut s, "sketch.constraint.tangent", json!({"a": "l1", "b": "c1"}));
    let err = s.execute("sketch.dimension", &json!({"entities": ["l1", "c1"]})).unwrap_err().to_string();
    assert!(err.contains("already determined"), "{err}");
    let r = run(&mut s, "sketch.dimension", json!({"entities": ["l1", "c1"], "if_redundant": "driven"}));
    assert_eq!(r["driven"], true, "{r}");
    assert!(r["measured"].as_f64().unwrap().abs() < 1e-7, "{r}");
}

#[test]
fn a_line_and_a_curve_that_is_not_round_is_still_refused() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [40, 0]]}));
    run(&mut s, "sketch.ellipse", json!({"center": [20, 20], "major": [30, 20], "minor_radius": 4}));
    let err = s.execute("sketch.dimension", &json!({"entities": ["l1", "e1"]})).unwrap_err().to_string();
    assert!(err.contains("cannot dimension that combination"), "{err}");
}
