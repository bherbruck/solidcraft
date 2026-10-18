//! User scenarios in the real UI, headless (see `solidcraft_ui_egui::scenario`). Each JSON file
//! under `tests/scenarios/` is one workflow done with clicks, keys and drags, and one test here:
//! `build.rs` makes a test per file, named after it. A scenario waiting for a fix starts with a
//! `{"pending": "why"}` step (its test is ignored with that reason). Adding a scenario is adding
//! its file.

use solidcraft_ui_egui::scenario::Harness;

fn run(name: &str) {
    let path = format!("{}/tests/scenarios/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let steps: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut h = Harness::new();
    if let Err(e) = h.run(&steps) {
        panic!("{name}: {e}");
    }
}

include!(concat!(env!("OUT_DIR"), "/scenario_tests.rs"));

/// A scenario file named by `SOLIDCRAFT_SCENARIO_FILE` (for trying one out):
/// `SOLIDCRAFT_SCENARIO_FILE=x.json cargo test --test scenarios adhoc -- --ignored --nocapture`.
#[test]
#[ignore]
fn adhoc() {
    let Ok(path) = solidcraft_engine::geom::env::var("SCENARIO_FILE").ok_or(()) else { return };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let steps: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut h = Harness::new();
    if let Err(e) = h.run(&steps) {
        panic!("{path}: {e}");
    }
}

#[test]
fn angle_dimension_both_interactive_workflows() {
    use serde_json::json;
    for preselect in [false, true] {
        let mut h = Harness::new();
        h.run(&json!([
            {"call":"engine.execute","params":{"command":"sketch.create","params":{"plane":"XY"}}},
            {"call":"engine.execute","params":{"command":"sketch.line","params":{"points":[[0,0],[40,0]]}}},
            {"call":"engine.execute","params":{"command":"sketch.line","params":{"points":[[0,0],[30,30]]}}},
            {"call":"ui.view","params":{"view":"top"}}
        ]))
        .unwrap();
        let picks = json!([
            {"click":{"sketch":[20,0]}},
            {"click":{"sketch":[15,15]},"shift":true}
        ]);
        if preselect {
            h.run(&picks).unwrap();
            h.run(&json!([{ "key":"D" }])).unwrap();
        } else {
            h.run(&json!([{ "key":"D" }])).unwrap();
            h.run(&picks).unwrap();
        }
        h.run(&json!([
            {"click":{"sketch":[20,8]}},
            {"text":"45"}, {"key":"Enter"},
            {"expect":{"errors":0,"param":{"d1":std::f64::consts::FRAC_PI_4}}}
        ]))
        .unwrap();
        let sk = &h.app.session.model.state().sketches[0].sketch;
        assert!(sk.constraints.iter().any(|c| matches!(c.kind, solidcraft_engine::sketch::ConstraintKind::Angle { .. })));
    }
}

/// The conic tool still places a curve in three clicks (start, end, apex), the way it always
/// has, defaulting to a parabola (`rho` 0.5). A further drag on the curve itself (not on one of
/// its three points) reads `rho` off where the drag lands between the chord's midpoint and the
/// apex, so an ellipse or a hyperbola is a drag away rather than a fourth click away. The apex
/// stays an ordinary, separately draggable point: dragging it slides the curve's tangent point
/// and leaves `rho` alone. Both drags are tried after leaving the tool with Escape, the way a
/// person reshapes anything else they have just drawn (the tool is sticky, placing more conics
/// on further clicks, the way the ellipse and slot tools already are too).
#[test]
fn dragging_a_conic_after_the_tool_sets_rho_and_its_apex_stays_separately_draggable() {
    use serde_json::json;
    let mut h = Harness::new();
    h.run(&json!([
        {"call": "engine.execute", "params": {"command": "sketch.create", "params": {"plane": "XY"}}},
        {"call": "ui.view", "params": {"view": "top"}},
        {"start": "sketch.conic"},
        {"click": {"sketch": [0, 0]}},
        {"click": {"sketch": [40, 0]}},
        {"click": {"sketch": [20, 20]}},
        {"key": "Escape"},
        {"expect": {"errors": 0}}
    ]))
    .unwrap();

    let sk = h.app.session.model.state().sketches[0].sketch.clone();
    let (ci, apex_idx, rho) = sk
        .curves
        .iter()
        .enumerate()
        .find_map(|(i, c)| match c.kind {
            solidcraft_engine::sketch::CurveKind::Conic { apex, rho, .. } => Some((i, apex, rho)),
            _ => None,
        })
        .expect("the three clicks created a conic");
    assert!((rho - 0.5).abs() < 1e-6, "default rho {rho}");
    let apex_before = sk.point(apex_idx).expect("apex point resolves");
    assert!(apex_before.dist(solidcraft_engine::geom::Vec2::new(20.0, 20.0)) < 1e-6);

    // Grabbed on the curve itself, at its default-rho midpoint (20, 10): the chord's midpoint
    // is (20, 0), the apex (20, 20), so (20, 14) is 0.7 of the way from one to the other.
    h.run(&json!([
        {"drag": [{"sketch": [20, 10]}, {"sketch": [20, 14]}], "steps": 4},
        {"expect": {"errors": 0}}
    ]))
    .unwrap();
    let sk_after_rho_drag = h.app.session.model.state().sketches[0].sketch.clone();
    let rho_after = match sk_after_rho_drag.curves[ci].kind {
        solidcraft_engine::sketch::CurveKind::Conic { rho, .. } => rho,
        ref k => panic!("not a conic: {k:?}"),
    };
    assert!((rho_after - 0.7).abs() < 0.02, "rho after drag {rho_after}");
    let apex_after_rho_drag = sk_after_rho_drag.point(apex_idx).expect("apex point resolves");
    assert!(apex_before.dist(apex_after_rho_drag) < 1e-6, "the apex moved on a curve-body drag");

    // Grabbed on the apex point itself: it slides, and rho is left as the previous drag set it.
    h.run(&json!([
        {"drag": [{"sketch": [20, 20]}, {"sketch": [10, 30]}], "steps": 4},
        {"expect": {"errors": 0}}
    ]))
    .unwrap();
    let sk_after_apex_drag = h.app.session.model.state().sketches[0].sketch.clone();
    let apex_after = sk_after_apex_drag.point(apex_idx).expect("apex point still resolves");
    assert!(apex_before.dist(apex_after) > 1.0, "apex did not move: still at {apex_after:?}");
    let rho_final = match sk_after_apex_drag.curves[ci].kind {
        solidcraft_engine::sketch::CurveKind::Conic { rho, .. } => rho,
        ref k => panic!("not a conic: {k:?}"),
    };
    assert!((rho_final - 0.7).abs() < 0.02, "rho changed on an apex drag: {rho_final}");
    // The same curve, reshaped in place (not replaced): the id survives both drags.
    assert_eq!(sk_after_apex_drag.curves.get(ci).map(|c| &c.id), sk.curves.get(ci).map(|c| &c.id));
}

#[test]
fn home_tab_and_file_recent_open_designs() {
    use serde_json::json;
    let mut h = Harness::new();
    let path = h.dir.join("recent.solidcraft").to_string_lossy().into_owned();
    h.run(&json!([
        {"call":"engine.execute","params":{"command":"solid.box","params":{"length":10,"width":10,"height":10}}},
        {"call":"engine.execute","params":{"command":"file.save_as","params":{"path":path}}},
        {"click":{"handle":"tab:home"}},
        {"expect":{"home":true,"documents":1}},
        {"click":{"handle":"tab:design:0"}},
        {"expect":{"home":false,"bodies":1}},
        {"click":{"handle":"tab:home"}},
        {"click":{"handle":"home:recent:0"}},
        {"until":{"file_busy":false}},
        {"expect":{"home":false,"bodies":1,"documents":2}},
        {"click":{"handle":"appbar:folder"}},
        {"widget":"Open Recent"}
    ]))
    .unwrap();
    h.run(&json!([
        {"widget":path},
        {"until":{"file_busy":false}},
        {"expect":{"home":false,"bodies":1,"documents":3}}
    ]))
    .unwrap();
}

#[test]
fn sketch_toolbar_imports_use_file_picker() {
    use serde_json::json;
    let mut h = Harness::new();
    let path = h.dir.join("drawing.svg");
    std::fs::write(
        &path,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="20mm" viewBox="0 0 40 20"><path d="M 0 0 L 40 0 L 40 20 L 0 20 Z"/></svg>"#,
    )
    .unwrap();
    h.run(&json!([
        {"call":"engine.execute","params":{"command":"sketch.create","params":{"plane":"XY"}}},
        {"choose_file":path},
        {"start":"sketch.insert_svg"},
        {"expect":{"errors":0,"sketch_curves":{"own":4}}}
    ]))
    .unwrap();
}

#[test]
fn narrow_toolbar_reaches_collapsed_commands() {
    use serde_json::json;
    let mut h = Harness::new();
    h.run(&json!([
        // Too narrow for every button: the last panels collapse into their drop-down.
        {"call":"ui.resize","params":{"width":640,"height":800}},
        {"expect":{"shown_handles":["collapsed:MODEL:CONSTRUCT"],"hidden_handles":["button:construct.plane.offset"]}},
        {"click":{"handle":"collapsed:MODEL:CONSTRUCT"}},
        {"expect":{"shown_handles":["dropdown:construct.plane.offset"]}},
        {"key":"Escape"},
        // Narrower: they go into the » menu.
        {"call":"ui.resize","params":{"width":480,"height":800}},
        {"expect":{"shown_handles":["toolbar:overflow"],"hidden_handles":["collapsed:MODEL:CONSTRUCT"]}},
        {"click":{"handle":"toolbar:overflow"}},
        {"widget":"CONSTRUCT"},
        {"expect":{"shown_handles":["toolbar:construct.plane.offset"]}},
        {"click":{"handle":"toolbar:construct.plane.offset"}},
        {"expect":{"dialog":"OffsetPlane"}}
    ]))
    .unwrap();
}
