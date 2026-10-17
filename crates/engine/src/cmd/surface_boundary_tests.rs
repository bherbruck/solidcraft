//! `surface.boundary` through loops of three or four sketch curves and through curve networks
//! (#763): analytic areas and shapes, G1 and G2 measured across the edge a surface shares with a
//! body (`inspect.continuity`), and refusals of hostile networks that never panic.

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn first(v: &Value) -> String {
    v.as_array().and_then(|a| a.first()).and_then(Value::as_str).map(str::to_string).unwrap_or_default()
}

/// A sketch line's id.
fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> String {
    first(&run(s, "sketch.line", json!({"points": [a, b]}))["created"]["curves"])
}

/// A 3D sketch line's id (the active sketch must have 3D Sketch on).
fn line3d(s: &mut Session, a: [f64; 3], b: [f64; 3]) -> String {
    first(&run(s, "sketch.line3d", json!({"points": [a, b]}))["curves"])
}

/// The area of the one face of `body`, and the area `inspect.measure` gives for the whole model.
fn areas(s: &mut Session, body: &str) -> (f64, f64) {
    let f = run(s, "model.faces", json!({"body": body}));
    let faces = f["faces"].as_array().cloned().unwrap_or_default();
    assert_eq!(faces.len(), 1, "one face: {f}");
    let face = faces[0]["area"].as_f64().unwrap_or(f64::NAN);
    let m = run(s, "inspect.measure", json!({}));
    (face, m["total"]["area_mm2"].as_f64().unwrap_or(f64::NAN))
}

/// Three and four sketch curves in loop order: the flat patch of the loop. A triangle and a square
/// of lines, and a square whose top is a half circle bulging out, each measure their own area.
#[test]
fn three_and_four_curve_loops_are_flat_patches() {
    use std::f64::consts::PI;
    let cases: Vec<(Vec<[[f64; 2]; 2]>, f64, f64)> = vec![
        (vec![[[0.0, 0.0], [20.0, 0.0]], [[20.0, 0.0], [10.0, 15.0]], [[10.0, 15.0], [0.0, 0.0]]], 150.0, 1e-6),
        (vec![[[0.0, 0.0], [10.0, 0.0]], [[10.0, 0.0], [10.0, 10.0]], [[10.0, 10.0], [0.0, 10.0]], [[0.0, 10.0], [0.0, 0.0]]], 100.0, 1e-6),
    ];
    for (lines, want, tol) in cases {
        for continuity in [json!("g0"), json!("g1"), json!("g3")] {
            let mut s = Session::default();
            run(&mut s, "sketch.create", json!({"plane": "XY"}));
            let curves: Vec<String> = lines.iter().map(|[a, b]| line(&mut s, *a, *b)).collect();
            let r = run(&mut s, "surface.boundary", json!({"curves": curves, "continuity": continuity}));
            let body = first(&r["created_bodies"]);
            let (face, total) = areas(&mut s, &body);
            assert!(
                (face - want).abs() < tol * want && (total - want).abs() < tol * want,
                "{} sides, {continuity}: {face} / {total} vs {want}",
                lines.len()
            );
        }
    }
    // A square with a half circle for its top, bulging out: 100 + 12.5π.
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let bottom = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let right = line(&mut s, [10.0, 0.0], [10.0, 10.0]);
    let arc = first(&run(&mut s, "sketch.arc.three_point", json!({"start": [10, 10], "end": [0, 10], "through": [5, 15]}))["created"]["curves"]);
    let left = line(&mut s, [0.0, 10.0], [0.0, 0.0]);
    let r = run(&mut s, "surface.boundary", json!({"curves": [bottom, right, arc, left]}));
    let (face, total) = areas(&mut s, &first(&r["created_bodies"]));
    let want = 100.0 + 12.5 * PI;
    assert!((face - want).abs() < 2e-3 * want && (total - want).abs() < 2e-3 * want, "{face} / {total} vs {want}");
}

/// Simpson's rule over [0, a] × [0, a] with n (even) intervals each way.
fn simpson2(f: impl Fn(f64, f64) -> f64, a: f64, n: usize) -> f64 {
    let h = a / n as f64;
    let w = |k: usize| {
        if k == 0 || k == n {
            1.0
        } else if k % 2 == 1 {
            4.0
        } else {
            2.0
        }
    };
    let mut sum = 0.0;
    for i in 0..=n {
        for j in 0..=n {
            sum += w(i) * w(j) * f(i as f64 * h, j as f64 * h);
        }
    }
    sum * h * h / 9.0
}

/// Four straight 3D sketch lines bounding the hyperbolic paraboloid z = x·y/10 over the 10 × 10
/// square: the network's Coons patch is that surface (every vertex of its mesh on it), and its
/// area is the paraboloid's, ∫∫ √(1 + (x/10)² + (y/10)²).
#[test]
fn a_network_of_3d_lines_is_a_hyperbolic_paraboloid() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.options", json!({"sketch_3d": true}));
    let a = line3d(&mut s, [0.0, 0.0, 0.0], [10.0, 0.0, 0.0]);
    let c = line3d(&mut s, [0.0, 10.0, 0.0], [10.0, 10.0, 10.0]);
    let d = line3d(&mut s, [0.0, 0.0, 0.0], [0.0, 10.0, 0.0]);
    let b = line3d(&mut s, [10.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
    let r = run(&mut s, "surface.boundary", json!({"u": [a, c], "v": [d, b]}));
    let body = first(&r["created_bodies"]);
    let want = simpson2(|x, y| (1.0 + (x / 10.0).powi(2) + (y / 10.0).powi(2)).sqrt(), 10.0, 200);
    let (face, total) = areas(&mut s, &body);
    // The face's own area comes from the coarse reference mesh (a chord of a fiftieth of a per
    // cent of the body); the measuring mesh is refined onto the surface.
    assert!((face - want).abs() < 1e-2 * want && (total - want).abs() < 1e-3 * want, "{face} / {total} vs {want}");
    let st = s.world_state();
    let mesh = st.body(&body).map(|b| b.body.tessellate(0.01)).and_then(Result::ok).unwrap_or_default();
    assert!(!mesh.positions.is_empty());
    for p in &mesh.positions {
        assert!((p.z - p.x * p.y / 10.0).abs() < 1e-6, "{p:?} is off the paraboloid");
    }
}

/// Curves of two planar sketches, one picked by a bare id in the sketch being edited: the loft
/// between a line on the ground and one 10 mm across and 5 mm up is the flat strip between them.
#[test]
fn a_network_takes_curves_from_several_sketches() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Low"}));
    let low = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 5, "name": "High"}));
    let high = line(&mut s, [0.0, 10.0], [10.0, 10.0]);
    let r = run(&mut s, "surface.boundary", json!({"u": [{"sketch": "Low", "curves": [low]}, high]}));
    let (face, _) = areas(&mut s, &first(&r["created_bodies"]));
    let want = 10.0 * 125f64.sqrt();
    assert!((face - want).abs() < 1e-6 * want, "{face} vs {want}");
}

/// A quarter cylinder sheet (R = 10, axis along x, its straight top edge at y = 10, z = 0 open)
/// and a line 3D-sketched above it; returns the sheet's name and the line's id.
fn cylinder_and_line(s: &mut Session) -> (String, String) {
    run(s, "sketch.create", json!({"plane": "YZ", "name": "Arc"}));
    let h = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
    let arc = first(&run(s, "sketch.arc.three_point", json!({"start": [0, -10], "end": [10, 0], "through": [h, -h]}))["created"]["curves"]);
    run(s, "sketch.finish", json!({}));
    let sheet = first(&run(s, "surface.extrude", json!({"sketch": "Arc", "curves": [arc], "distance": 10}))["created_bodies"]);
    run(s, "sketch.create", json!({"plane": "XY", "name": "Top"}));
    run(s, "sketch.options", json!({"sketch_3d": true}));
    let top = line3d(s, [0.0, 3.0, 12.0], [10.0, 3.0, 12.0]);
    run(s, "sketch.finish", json!({}));
    (sheet, top)
}

/// The edge a surface built from the cylinder's top edge shares with it, measured with
/// `inspect.continuity`: (normal angle in degrees, curvature jump per mm).
fn seam(s: &mut Session, body: &str) -> (f64, f64) {
    let v = run(s, "inspect.continuity", json!({"body": body, "edges": [[5.0, 10.0, 0.0]]}));
    let e = v["bodies"][0]["edges"].as_array().and_then(|a| a.first()).cloned().unwrap_or_default();
    (e["normal_angle_deg"]["value"].as_f64().unwrap_or(f64::NAN), e["curvature_jump_per_mm"]["value"].as_f64().unwrap_or(f64::NAN))
}

/// G1 and G2 against a body: a network from the cylinder's open top edge to a line above, joined
/// into the cylinder. Across the shared edge the normals agree within 0.1° for G1 and G2; the
/// curvature jumps by much of the cylinder's 1/R with G1 and by under 1% of it with G2.
#[test]
fn g1_and_g2_networks_meet_a_body_across_its_edge() {
    let mut jumps = Vec::new();
    for g in ["g1", "g2"] {
        let mut s = Session::default();
        let (sheet, top) = cylinder_and_line(&mut s);
        let r = run(
            &mut s,
            "surface.boundary",
            json!({"u": [{"body": sheet, "edges": [[5.0, 10.0, 0.0]], "continuity": g}, {"sketch": "Top", "curves": [top]}], "operation": "join"}),
        );
        assert!(r["created_bodies"].as_array().is_none_or(Vec::is_empty), "a join makes no new body: {r}");
        let f = run(&mut s, "model.faces", json!({"body": sheet}));
        assert_eq!(f["faces"].as_array().map(Vec::len), Some(2), "the surface is sewn into the sheet: {f}");
        let (angle, jump) = seam(&mut s, &sheet);
        assert!(angle < 0.1, "{g}: normals {angle}° apart across the edge");
        jumps.push(jump);
    }
    let r = 10.0;
    assert!(jumps[0] > 0.2 / r, "G1 leaves the curvature behind: jump {}", jumps[0]);
    assert!(jumps[1] < 0.01 / r, "G2 carries the cylinder's curvature across: jump {}", jumps[1]);
}

/// Networks that cannot be built are refused with a reason — by the command's own checks or by the
/// kernel — and leave nothing behind; none of them panics.
#[test]
fn hostile_networks_are_refused() {
    let mut s = Session::default();
    let (sheet, top) = cylinder_and_line(&mut s);
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [40, 40, 0]}));
    let boxed = s.world_state().bodies.last().map(|b| b.name.clone()).unwrap_or_default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Flat"}));
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let b = line(&mut s, [0.0, 5.0], [10.0, 5.0]);
    let far = line(&mut s, [0.0, 20.0], [10.0, 20.0]);
    let ring = first(&run(&mut s, "sketch.circle.center", json!({"center": [50, 50], "radius": 3}))["created"]["curves"]);
    let bodies = s.world_state().bodies.len();
    let edge = |g: &str| json!({"body": sheet, "edges": [[5.0, 10.0, 0.0]], "continuity": g});
    let many: Vec<Value> = (0..33).map(|_| json!(a)).collect();
    let cases: Vec<(Value, &str)> = vec![
        (json!({"u": "l1"}), "list"),
        (json!({"u": [42, a]}), "curve id"),
        (json!({"u": [{}, a]}), "one of"),
        (json!({"u": [{"curves": [], "sketch": "Flat"}, a]}), "sketch curves"),
        (json!({"u": [{"curves": ["nope"]}, a]}), "no sketch curve"),
        (json!({"u": [{"curves": [a], "sketch": "Nope"}, b]}), "no sketch"),
        (json!({"u": [{"body": "Nope", "edges": [[0, 0, 0]]}, a]}), "no body"),
        (json!({"u": [{"body": sheet, "edges": []}, a]}), "edges"),
        (json!({"u": [{"body": sheet, "edges": [["x", 0, 0]]}, a]}), "[x, y, z]"),
        (json!({"u": [{"body": sheet, "edges": [[1e300, 0, 0]]}, a]}), "[x, y, z]"),
        (json!({"u": [{"point": [0, 0]}, a]}), "point"),
        (json!({"u": [{"curves": [a], "body": sheet, "edges": [[5, 10, 0]]}, b]}), "one of"),
        (json!({"u": [{"curves": [a], "continuity": "g1"}, b]}), "body's edges"),
        (json!({"u": [edge("g3"), {"sketch": "Top", "curves": [top]}]}), "not supported yet"),
        (json!({"u": [edge("g9"), {"sketch": "Top", "curves": [top]}]}), "continuity"),
        (json!({"u": many}), "at most"),
        (json!({"u": [a, b], "curves": [a, b]}), "loop"),
        (json!({"u": [a, b], "crown": 1}), "loop"),
        (json!({"u": [a, b], "operation": "cut"}), "operation"),
        (json!({"u": [a, b], "operation": "join"}), "join"),
        (json!({"u": [a]}), "two curves"),
        (json!({"u": [], "v": [a, b]}), "`u`"),
        // The kernel's refusals: curves that miss, a closed curve, G1 on an edge between two faces.
        (json!({"u": [a, b], "v": [{"curves": [far]}, {"curves": [a]}]}), "don't meet"),
        (json!({"u": [{"curves": [ring]}, b]}), "closed"),
        (json!({"u": [{"body": boxed, "edges": [[45, 40, 0]], "continuity": "g1"}, b]}), "open edge"),
        (json!({"u": [a, {"point": [5, 2, 0]}, b]}), "point"),
    ];
    for (params, want) in cases {
        let e = match s.execute("surface.boundary", &params) {
            Ok(v) => panic!("{params} was built: {v}"),
            Err(e) => e,
        };
        assert!(!matches!(e, crate::EngineError::Internal(..)), "{params}: {e}");
        assert!(e.to_string().contains(want), "{params}: {e}");
    }
    assert_eq!(s.world_state().bodies.len(), bodies, "a refused network left a body behind");
}

/// An edit (`timeline.redefine`) may turn a loop of sketch curves into the network of the same
/// curves and back: the feature keeps its place and its name, and the surface its area.
#[test]
fn an_edit_turns_a_loop_into_a_network_and_back() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Square"}));
    let sides: Vec<String> = [[[0.0, 0.0], [10.0, 0.0]], [[10.0, 0.0], [10.0, 10.0]], [[10.0, 10.0], [0.0, 10.0]], [[0.0, 10.0], [0.0, 0.0]]]
        .iter()
        .map(|[a, b]| line(&mut s, *a, *b))
        .collect();
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "surface.boundary", json!({"sketch": "Square", "curves": sides, "name": "Patch"}));
    let network = json!({"sketch": "Square", "u": [sides[0], sides[2]], "v": [sides[1], sides[3]]});
    run(&mut s, "timeline.redefine", json!({"feature": "Patch", "command": "surface.boundary", "params": network}));
    let kind = |s: &Session| s.doc.find_feature("Patch").map(|f| f.kind.clone());
    assert!(matches!(kind(&s), Some(solidcraft_doc::FeatureKind::SurfaceNetwork { .. })), "{:?}", kind(&s));
    let body = s.world_state().bodies.first().map(|b| b.name.clone()).unwrap_or_default();
    assert!((areas(&mut s, &body).0 - 100.0).abs() < 1e-6);
    run(&mut s, "timeline.redefine", json!({"feature": "Patch", "command": "surface.boundary", "params": {"sketch": "Square", "curves": sides}}));
    assert!(matches!(kind(&s), Some(solidcraft_doc::FeatureKind::SurfaceBoundary { .. })), "{:?}", kind(&s));
    assert!((areas(&mut s, &body).0 - 100.0).abs() < 1e-6);
}
