//! #1669: a window cut through a battery case's wall and a lofted pleat joined into it, mirrored
//! to the case's far end, and the same features built directly there.

use serde_json::{Value, json};
use solidcraft_engine::Session;

fn replay(script: &str) -> Session {
    let mut s = Session::default();
    let script: Value = serde_json::from_str(script).unwrap();
    s.run_script(&script).unwrap();
    s
}

fn volume(s: &Session, body: &str) -> f64 {
    s.model.state().body(body).map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).unwrap_or(0.0)
}

/// #1669: an Extrude Cut whose profile stops 0.25 mm short of the far face of a 3.75 mm wall
/// leaves that skin. The profile's edge was taken for one lying on the face (the space half a
/// growth step past it was empty) and grown through the skin, until #1310 asked the faces the
/// growth sweeps instead. A pleat window built so at the far end of a case opened, where its
/// mirror rightly left the skin.
#[test]
fn cut_stopping_short_of_a_face_leaves_the_skin() {
    let wall = |width: f64| {
        let s = replay(
            &json!({"commands": [
                {"command": "solid.box", "params": {"corner": [-100, 0, 0], "length": 200, "width": width, "height": 95, "body_name": "Wall"}},
                {"command": "sketch.create", "params": {"plane": "XY", "offset": -1}},
                {"command": "sketch.rectangle.two_point", "params": {"p0": [-24, -0.5], "p1": [24, 3.5]}},
                {"command": "sketch.finish", "params": {}},
                {"command": "solid.extrude", "params": {"distance": 91, "operation": "cut", "targets": ["Wall"]}}
            ]})
            .to_string(),
        );
        volume(&s, "Wall")
    };
    // 48 wide, 3.5 deep (of the wall's 3.75), 90 high.
    let want = 200.0 * 3.75 * 95.0 - 48.0 * 3.5 * 90.0;
    let v = wall(3.75);
    assert!((v - want).abs() < 1e-6 * want, "{v} vs {want}");
    // A profile ending on the far face still cuts clean through, as before.
    let want = 200.0 * 3.5 * 95.0 - 48.0 * 3.5 * 90.0;
    let v = wall(3.5);
    assert!((v - want).abs() < 1e-6 * want, "{v} vs {want}");
}

/// The features of `s` that failed, with their errors.
fn feature_errors(s: &Session) -> Vec<(String, String)> {
    s.model.results.iter().filter_map(|r| r.error.clone().map(|e| (r.name.clone(), e))).collect()
}

/// #1669: a window and a lofted pleat mirrored to the far end of a case shelled 3.5 mm. The
/// pleat's twisted faces were each meshed as two flat triangles (#1666), the joins were checked
/// against those meshes, and the mirror failed ("boolean Union: no result").
#[test]
fn mirrored_pleat_joins_into_a_thick_case_wall() {
    let s = replay(include_str!("../../../examples/regressions/battery_case_pleats.json"));
    let errors = feature_errors(&s);
    assert!(errors.is_empty(), "{errors:?}");
}

/// #1669: at 3 mm, the same window and pleat built directly at the far end of the finished case
/// (a cut and a loft of their own, the mirror suppressed) failed to join as well: the end rounds
/// they meet had folded faces (#1667).
#[test]
fn pleat_built_at_the_far_end_joins_the_finished_case() {
    let mut s = replay(include_str!("../../../examples/regressions/battery_case_pleats.json"));
    s.execute("parameters.change", &json!({"name": "wall", "expression": "3 mm"})).unwrap();
    let errors = feature_errors(&s);
    assert!(errors.is_empty(), "{errors:?}");
    s.execute("timeline.suppress", &json!({"feature": "PleatFar", "suppressed": true})).unwrap();
    let before = volume(&s, "Case");
    // Mirrored about y = -99: y' = -198 - y.
    let far = |points: &[[f64; 2]]| points.iter().map(|[x, y]| json!([x, -198.0 - y])).collect::<Vec<_>>();
    let bottom = [
        [-26.0, 3.0],
        [-24.76384, 3.0],
        [-16.0568, 14.97217],
        [-8.0, 4.90117],
        [0.0, 14.90117],
        [8.0, 4.90117],
        [16.0568, 14.97217],
        [24.76384, 3.0],
        [26.0, 3.0],
        [26.0, 0.0],
        [23.23616, 0.0],
        [15.9432, 10.02783],
        [8.0, 0.09883],
        [0.0, 10.09883],
        [-8.0, 0.09883],
        [-15.9432, 10.02783],
        [-23.23616, 0.0],
        [-26.0, 0.0],
    ];
    let top = [
        [-26.0, 3.0],
        [-24.0, 3.0],
        [-16.0, 3.0],
        [-8.0, 3.0],
        [0.0, 3.0],
        [8.0, 3.0],
        [16.0, 3.0],
        [24.0, 3.0],
        [26.0, 3.0],
        [26.0, 0.0],
        [24.0, 0.0],
        [16.0, 0.0],
        [8.0, 0.0],
        [0.0, 0.0],
        [-8.0, 0.0],
        [-16.0, 0.0],
        [-24.0, 0.0],
        [-26.0, 0.0],
    ];
    let steps = json!([
        {"command": "sketch.create", "params": {"plane": "XY", "offset": "-clear", "name": "FarCutSketch"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [-24, -197.5], "p1": [24, -201.5]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"sketch": "FarCutSketch", "distance": "pleat_top + clear", "operation": "cut", "targets": ["Case"]}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "FarBottom"}},
        {"command": "sketch.line", "params": {"closed": true, "infer": false, "connect": false, "points": far(&bottom)}},
        {"command": "sketch.finish", "params": {}},
        {"command": "sketch.create", "params": {"plane": "XY", "offset": "pleat_top + 0.5 mm", "name": "FarTop"}},
        {"command": "sketch.line", "params": {"closed": true, "infer": false, "connect": false, "points": far(&top)}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.loft", "params": {"sections": [{"sketch": "FarBottom"}, {"sketch": "FarTop"}], "operation": "join", "targets": ["Case"]}}
    ]);
    for c in steps.as_array().into_iter().flatten() {
        let (id, p) = (c["command"].as_str().unwrap_or_default(), &c["params"]);
        if let Err(e) = s.execute(id, p) {
            panic!("{id}: {e}");
        }
    }
    let errors = feature_errors(&s);
    assert!(errors.is_empty(), "{errors:?}");
    // The far pleat is there: its window and the loft change the case.
    let after = volume(&s, "Case");
    assert!((after - before).abs() > 1000.0, "{after} after the far pleat, {before} before");
}
