use serde_json::{Value, json};
use solidcraft_geom::Vec2;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn new_sketch() -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    s
}

fn sketch(s: &Session) -> solidcraft_sketch::Sketch {
    s.model.state().sketch(s.active_sketch.unwrap()).unwrap().sketch.clone()
}

#[test]
fn redundant_dimensions_are_refused_driven_ones_measure() {
    let mut s = new_sketch();
    let l = ids(&run(&mut s, "sketch.line", json!({"points": [[0, 0], [10, 0]], "infer": true}))["curves"])[0].clone();
    // Already horizontal (inferred): kept as redundant, unless asked to refuse it.
    let e = s.execute("sketch.constraint.horizontal_vertical", &json!({"line": l, "if_redundant": "refuse"})).unwrap_err().to_string();
    assert!(e.contains("over-constrain") && e.contains("already determined by"), "{e}");
    run(&mut s, "sketch.constraint.coincident", json!({"a": format!("{l}.start"), "b": "origin"}));
    run(&mut s, "sketch.dimension", json!({"entities": [l], "value": 10}));
    let si = run(&mut s, "sketch.inspect", json!({}));
    assert_eq!(si["dof"], 0, "{si}");
    // A second length on it is already determined: refused, unless driven.
    assert!(s.execute("sketch.dimension", &json!({"entities": [format!("{l}.start"), format!("{l}.end")]})).is_err());
    let d = run(&mut s, "sketch.dimension", json!({"entities": [format!("{l}.start"), format!("{l}.end")], "driven": true}));
    assert_eq!(d["driven"], true);
    // The driven dimension follows the driving one.
    let pname = si["constraints"].as_array().unwrap().iter().find_map(|c| c["param"].as_str().map(str::to_string)).unwrap();
    run(&mut s, "parameters.change", json!({"name": pname, "expression": "25 mm"}));
    let g = run(&mut s, "sketch.glyphs", json!({}));
    let driven = g["glyphs"].as_array().unwrap().iter().find(|x| x["driven"] == true).unwrap().clone();
    assert!((driven["value"].as_f64().unwrap() - 25.0).abs() < 1e-7, "{driven}");
    assert_eq!(g["fully_constrained"], true);
}

/// `measured` is the dimension on the solved sketch: a trough drawn 99.84 long and dimensioned
/// 100 reports 100, not the length it had before solving.
#[test]
fn a_dimension_reports_the_solved_value() {
    let mut s = new_sketch();
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [99.84, 40.2]}));
    run(&mut s, "sketch.constraint.coincident", json!({"a": "p1", "b": "origin"}));
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "value": 100}));
    assert!((d["measured"].as_f64().unwrap() - 100.0).abs() < 1e-6, "{d}");
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["l2"], "value": "40 mm"}));
    assert!((d["measured"].as_f64().unwrap() - 40.0).abs() < 1e-6, "{d}");
    // A driven dimension measures the solved geometry too.
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["p1", "p3"], "driven": true}));
    assert!((d["measured"].as_f64().unwrap() - 100.0_f64.hypot(40.0)).abs() < 1e-6, "{d}");
    let c = run(&mut s, "sketch.circle.center", json!({"center": [50, 20], "radius": 7.3}));
    let c = ids(&c["curves"])[0].clone();
    let d = run(&mut s, "sketch.dimension", json!({"entities": [c], "value": 15}));
    assert!((d["measured"].as_f64().unwrap() - 15.0).abs() < 1e-6, "{d}");
}

#[test]
fn curvature_polygon_arc_length_linear_diameter() {
    let mut s = new_sketch();
    // A horizontal line ending where a control-point spline starts: G2 keeps the spline's
    // start straight (zero curvature) and tangent.
    let l = ids(&run(&mut s, "sketch.line", json!({"points": [[-10, 0], [0, 0]], "infer": true}))["curves"])[0].clone();
    let sp =
        ids(&run(&mut s, "sketch.spline.control_point", json!({"points": [format!("{l}.end"), [5, 2], [10, 6], [15, 15]]}))["curves"])[0].clone();
    run(&mut s, "sketch.constraint.curvature", json!({"a": l, "b": sp}));
    let sk = sketch(&s);
    let pts: Vec<Vec2> = sk.curves[sk.curve_index(&sp).unwrap()].kind.point_ids().iter().map(|i| sk.point(*i).unwrap()).collect();
    // Tangent: first leg along +x; zero curvature: the second leg too (collinear first three).
    assert!((pts[1] - pts[0]).y.abs() < 1e-5, "{pts:?}");
    assert!((pts[2] - pts[1]).cross(pts[1] - pts[0]).abs() < 1e-3, "{pts:?}");

    // Polygon constraint on a rough quadrilateral: becomes a square-ish regular polygon.
    let q = run(&mut s, "sketch.line", json!({"points": [[100, 0], [111, 1], [110, 9], [99, 10]], "closed": true}));
    let ql = ids(&q["curves"]);
    run(&mut s, "sketch.constraint.polygon", json!({"lines": ql}));
    let sk = sketch(&s);
    let lens: Vec<f64> = ql.iter().map(|i| sk.polyline(sk.curve_index(i).unwrap())).map(|p| p[0].dist(p[p.len() - 1])).collect();
    assert!(lens.iter().all(|x| (x - lens[0]).abs() < 1e-6), "{lens:?}");

    // Arc length.
    let a = ids(&run(&mut s, "sketch.arc.center_point", json!({"center": [0, 50], "start": [10, 50], "end": [0, 60]}))["curves"])[0].clone();
    run(&mut s, "sketch.dimension", json!({"entities": [a], "type": "arc_length", "value": 20}));
    let sk = sketch(&s);
    let ai = sk.curve_index(&a).unwrap();
    let len = match sk.segs(ai)[0] {
        solidcraft_geom::Seg2::Arc { radius, sweep, .. } => radius * sweep,
        _ => 0.0,
    };
    assert!((len - 20.0).abs() < 1e-6, "{len}");

    // Linear diameter about a centerline.
    let cl = ids(&run(&mut s, "sketch.line", json!({"points": [[200, 0], [200, 50]]}))["curves"])[0].clone();
    run(&mut s, "sketch.centerline", json!({"curves": [cl]}));
    run(&mut s, "sketch.point", json!({"point": [207, 10], "id": "rim"}));
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["rim", cl], "value": 30}));
    assert_eq!(d["type"], "LinearDiameterDimension", "{d}");
    let sk = sketch(&s);
    let rim = sk.point(sk.point_index("rim").unwrap()).unwrap();
    let line = sk.polyline(sk.curve_index(&cl).unwrap());
    let (a, b) = (line[0], line[line.len() - 1]);
    let dist = (b - a).normalized().unwrap().cross(rim - a).abs();
    assert!((dist - 15.0).abs() < 1e-6, "{rim:?} {dist}");
}

#[test]
fn glyphs_and_snaps() {
    let mut s = new_sketch();
    let r = run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [20, 10], "connect": false}));
    let l = ids(&r["curves"]);
    let g = run(&mut s, "sketch.glyphs", json!({}));
    assert_eq!(g["glyphs"].as_array().unwrap().len(), 4);
    assert!(g["glyphs"][0]["at"].is_array());
    let sn = run(&mut s, "sketch.snap", json!({"at": [20.3, 0.2], "radius": 1}));
    assert_eq!(sn["snaps"][0]["type"], "coincident", "{sn}");
    let sn = run(&mut s, "sketch.snap", json!({"at": [10.2, 0.3], "radius": 1}));
    assert_eq!(sn["snaps"][0]["type"], "midpoint", "{sn}");
    assert_eq!(sn["snaps"][0]["curve"], json!(l[0]));
    let sn = run(&mut s, "sketch.snap", json!({"at": [40, 10.4], "from": [30, 10], "radius": 1}));
    assert!(sn["snaps"].as_array().unwrap().iter().any(|x| x["type"] == "horizontal"), "{sn}");
}

#[test]
fn auto_constrain_fully_constrains_a_rough_sketch() {
    let mut s = new_sketch();
    // A slightly crooked closed rectangle and a circle inside.
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [40, 0.004], [40.003, 30], [0, 30.002]], "closed": true}));
    run(&mut s, "sketch.circle.center", json!({"center": [20, 15], "radius": 5}));
    let r = run(&mut s, "sketch.auto_constrain", json!({}));
    assert_eq!(r["dof"], 0, "{r}");
    let si = run(&mut s, "sketch.inspect", json!({}));
    assert_eq!(si["fully_constrained"], true, "{si}");
    let names: Vec<String> = si["constraints"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect();
    assert!(names.iter().filter(|n| *n == "Horizontal").count() >= 2, "{names:?}");
    assert!(names.iter().any(|n| n == "DiameterDimension"), "{names:?}");
    // From a datum, and finishing.
    let mut s = new_sketch();
    run(&mut s, "sketch.point", json!({"point": [5, 5], "id": "d"}));
    run(&mut s, "sketch.line", json!({"points": [[10, 10], [20, 10]]}));
    let r = run(&mut s, "sketch.finish_auto_constrain", json!({"datum": "d"}));
    assert!(s.active_sketch.is_none());
    assert!(r["added"].as_array().unwrap().len() >= 3, "{r}");
}

#[cfg(feature = "solver-profiling")]
#[test]
fn internal_freedom_queries_skip_redundancy_and_keep_constraint_acceptance() {
    use solidcraft_sketch::{ConstraintKind, Sketch, profiling::capture};

    let mut sk = Sketch::new();
    let q = sk.add_point(Vec2::new(10.0, 2.0), None).unwrap();
    let k = ConstraintKind::Distance { p: 0, q, value: 10.0 };
    sk.add_constraint(k.clone(), None).unwrap();
    let redundant = sk.add_constraint(k.clone(), None).unwrap();
    let (dof, profile) = capture(|| super::dof_now(&sk));
    assert_eq!(dof, 1);
    assert_eq!(profile.counters.redundancy_checks, 0);

    let mut doc = solidcraft_doc::Document::default();
    let before = sk.clone();
    let (rejected, profile) = capture(|| super::try_add(&mut sk, &mut doc, k, dof, false));
    assert_eq!(rejected, None, "a duplicate must not be accepted as a DOF reduction");
    assert_eq!(sk, before);
    assert_eq!(profile.counters.redundancy_checks, 0);

    let (accepted, profile) = capture(|| super::try_add(&mut sk, &mut doc, ConstraintKind::DistanceX { p: 0, q, value: 6.0 }, dof, false));
    assert_eq!(accepted, Some(0));
    assert_eq!(profile.counters.redundancy_checks, 0);
    let full = solidcraft_sketch::solve(&mut sk);
    assert!(full.ok());
    assert_eq!(full.dof, 0);
    assert!(full.redundant.contains(&redundant), "the final full report must retain diagnostics");
}

#[test]
fn drawing_infers_tangent_and_perpendicular() {
    let mut s = new_sketch();
    let l1 = ids(&run(&mut s, "sketch.line", json!({"points": [[0, 0], [10, 10]], "infer": true}))["curves"])[0].clone();
    let r = run(&mut s, "sketch.line", json!({"points": [format!("{l1}.end"), [20, 0]], "infer": true}));
    assert_eq!(r["constraints"].as_array().unwrap().len(), 1, "{r}");
    let a = ids(&run(&mut s, "sketch.arc.center_point", json!({"center": [50, 0], "start": [60, 0], "end": [50, 10]}))["curves"])[0].clone();
    let r = run(&mut s, "sketch.line", json!({"points": [format!("{a}.end"), [30, 10]], "infer": true}));
    let si = run(&mut s, "sketch.inspect", json!({}));
    let names: Vec<&str> = si["constraints"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Perpendicular") && names.contains(&"Tangent"), "{names:?} {r}");
}

#[test]
fn smart_constrain_and_driven_toggle() {
    let mut s = new_sketch();
    let a = ids(&run(&mut s, "sketch.line", json!({"points": [[0, 0], [10, 0.5]]}))["curves"])[0].clone();
    let b = ids(&run(&mut s, "sketch.line", json!({"points": [[0, 5], [10, 6]]}))["curves"])[0].clone();
    let r = run(&mut s, "sketch.constrainer", json!({"entities": [a]}));
    assert_eq!(r["type"], "Horizontal");
    let r = run(&mut s, "sketch.constrainer", json!({"entities": [a, b]}));
    assert_eq!(r["type"], "Parallel");
    let c1 = ids(&run(&mut s, "sketch.circle.center", json!({"center": [50, 0], "radius": 5}))["curves"])[0].clone();
    let c2 = ids(&run(&mut s, "sketch.circle.center", json!({"center": [50.3, 0.2], "radius": 3}))["curves"])[0].clone();
    let r = run(&mut s, "sketch.constrainer", json!({"entities": [c1, c2]}));
    assert_eq!(r["type"], "Concentric");
    // Driven toggle.
    let d = run(&mut s, "sketch.dimension", json!({"entities": [c1], "value": 10}));
    let g = run(&mut s, "sketch.glyphs", json!({}));
    let did = g["glyphs"].as_array().unwrap().iter().find(|x| x["param"] == d["param"]).unwrap()["id"].as_str().unwrap().to_string();
    let r = run(&mut s, "sketch.toggle_driven", json!({"constraint": did}));
    assert_eq!(r["driven"], true);
    assert!(s.doc.param(d["param"].as_str().unwrap()).is_none(), "the parameter goes");
    let r = run(&mut s, "sketch.toggle_driven", json!({"constraint": did}));
    assert_eq!(r["driven"], false);
}

#[test]
fn drawing_from_a_line_midpoint_keeps_it_there() {
    let mut s = new_sketch();
    let l = ids(&run(&mut s, "sketch.line", json!({"points": [[0, 0], [20, 0]]}))["curves"])[0].clone();
    let r = run(&mut s, "sketch.line", json!({"points": [format!("mid:{l}"), [10, 15]]}));
    let si = run(&mut s, "sketch.inspect", json!({}));
    assert!(si["constraints"].as_array().unwrap().iter().any(|c| c["name"] == "MidPoint"), "{si}");
    // Lengthen the first line: the second one's start follows the middle.
    run(&mut s, "sketch.move_point", json!({"point": format!("{l}.end"), "to": [40, 0]}));
    let sk = sketch(&s);
    let n = ids(&r["curves"])[0].clone();
    let st = sk.resolve_point(&format!("{n}.start")).and_then(|i| sk.point(i)).unwrap();
    let a = sk.resolve_point(&format!("{l}.start")).and_then(|i| sk.point(i)).unwrap();
    let b = sk.resolve_point(&format!("{l}.end")).and_then(|i| sk.point(i)).unwrap();
    assert!(st.dist((a + b) * 0.5) < 1e-7, "{st:?}");
}

#[test]
fn drawing_onto_a_curve_keeps_the_point_on_it() {
    let mut s = new_sketch();
    let c = ids(&run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 10}))["curves"])[0].clone();
    let r = run(&mut s, "sketch.line", json!({"points": [format!("on:{c}:7.2,7.1"), [30, 30]]}));
    let n = ids(&r["curves"])[0].clone();
    run(&mut s, "sketch.dimension", json!({"entities": [c], "value": 16}));
    let sk = sketch(&s);
    let st = sk.resolve_point(&format!("{n}.start")).and_then(|i| sk.point(i)).unwrap();
    let cc = sk.center(sk.curve_index(&c).unwrap()).unwrap();
    assert!((st.dist(cc) - 8.0).abs() < 1e-7, "{st:?} {cc:?}");
}

/// A faucet spout path: a vertical riser, a horizontal top and an outlet sloping down, joined
/// by fillets. The riser points up and the outlet down-left, so their directions are 150° apart
/// while the lines meet at 30°.
fn spout() -> Session {
    let mut s = new_sketch();
    run(&mut s, "sketch.line", json!({"points": [[795, 955], [795, 1068], [689, 1068], [661, 1020]], "ids": ["l1", "l2", "l3"]}));
    run(&mut s, "sketch.fillet", json!({"point": "l1.end", "radius": 50}));
    run(&mut s, "sketch.fillet", json!({"point": "l2.end", "radius": 35}));
    run(&mut s, "sketch.constraint.horizontal_vertical", json!({"line": "l1", "mode": "vertical"}));
    run(&mut s, "sketch.constraint.horizontal_vertical", json!({"line": "l2", "mode": "horizontal"}));
    run(&mut s, "sketch.dimension", json!({"entities": ["origin", "l1.start"], "type": "horizontal", "value": 795}));
    run(&mut s, "sketch.dimension", json!({"entities": ["origin", "l1.start"], "type": "vertical", "value": 955}));
    run(&mut s, "sketch.dimension", json!({"entities": ["origin", "l2.start"], "type": "vertical", "value": 1068}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l3.end", "l1.start"], "type": "horizontal", "value": 134}));
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["origin", "l3.end"], "type": "vertical", "value": 1020}));
    assert_eq!(d["sketch"]["dof"], 1, "{d}");
    s
}

/// The angle (degrees, 0..=90) between two lines of the active sketch.
fn line_angle(s: &Session, a: &str, b: &str) -> f64 {
    let sk = sketch(s);
    let dir = |l: &str| {
        let c = sk.curve_index(l).unwrap();
        let (p, q) = crate::cmd::sketch::line_pts(&sk, c).unwrap();
        q - p
    };
    let (u, v) = (dir(a), dir(b));
    let t = u.cross(v).abs().atan2(u.dot(v)).to_degrees();
    t.min(180.0 - t)
}

#[test]
fn an_angle_between_lines_takes_the_reading_nearest_its_value_and_reports_degrees() {
    // The riser and the outlet are not adjacent and meet at 30°; their directions are 150°
    // apart. Asking for 30° used to swing the outlet round and was refused as over-constraining.
    for ents in [["l1", "l3"], ["l3", "l1"]] {
        let mut s = spout();
        let d = run(&mut s, "sketch.dimension", json!({"entities": ents, "type": "angle", "value": "30 deg"}));
        assert_eq!(d["sketch"]["dof"], 0, "{d}");
        assert_eq!(d["unit"], "deg", "{d}");
        assert!((d["measured"].as_f64().unwrap() - 30.0).abs() < 1e-6, "{d}");
        assert!((line_angle(&s, "l1", "l3") - 30.0).abs() < 1e-6);
    }
    // The other reading still works, and the adjacent top line measures 60° the same way.
    let mut s = spout();
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["l1", "l3"], "value": "150 deg"}));
    assert_eq!(d["sketch"]["dof"], 0, "{d}");
    assert!((line_angle(&s, "l1", "l3") - 30.0).abs() < 1e-6);
    let mut s = spout();
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["l2", "l3"], "value": "60 deg"}));
    assert!((d["measured"].as_f64().unwrap() - 60.0).abs() < 1e-6, "{d}");
    // Without a value the acute angle is kept as it is.
    let mut s = spout();
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["l1", "l3"]}));
    let e = d["expression"].as_str().unwrap();
    assert!(e.ends_with(" deg") && (e.trim_end_matches(" deg").parse::<f64>().unwrap() - 30.3).abs() < 0.1, "{d}");
    // Lengths report millimetres.
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["l2"], "driven": true}));
    assert_eq!(d["unit"], "mm", "{d}");
}

#[test]
fn symmetry_moves_a_free_line_between_two_fixed_lines() {
    // #1796: with both mirrored lines fixed, the symmetry line between them (a free construction
    // line) is the one to move; it was refused as "neither entity can move".
    let mut s = new_sketch();
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [0, 20]], "ids": ["left"]}));
    run(&mut s, "sketch.line", json!({"points": [[10, 0], [10, 20]], "ids": ["right"]}));
    run(&mut s, "sketch.line", json!({"points": [[3, 0], [3, 20]], "construction": true, "ids": ["mid"]}));
    run(&mut s, "sketch.constraint.fix", json!({"entities": ["left", "right"]}));
    run(&mut s, "sketch.constraint.symmetry", json!({"a": "left", "b": "right", "line": "mid"}));
    let sk = sketch(&s);
    let at = |r: &str| sk.point(sk.resolve_point(r).unwrap()).unwrap();
    for end in ["mid.start", "mid.end"] {
        assert!((at(end).x - 5.0).abs() < 1e-7, "{end} at {:?}", at(end));
    }
    for (end, x) in [("left.start", 0.0), ("left.end", 0.0), ("right.start", 10.0), ("right.end", 10.0)] {
        assert!((at(end).x - x).abs() < 1e-9, "{end} moved to {:?}", at(end));
    }
}
