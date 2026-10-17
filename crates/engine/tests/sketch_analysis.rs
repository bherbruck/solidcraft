//! Command-level diagnostic work, including the report ultimately published to the model.
#![cfg(feature = "solver-profiling")]

use std::sync::Arc;

use serde_json::json;
use solidcraft_engine::Session;
use solidcraft_geom::Vec2;
use solidcraft_sketch::{Analysis, ConstraintKind, Sketch, profiling::capture};

fn session(horizontal: bool) -> (Session, u64, String) {
    let mut s = Session::default();
    s.execute("sketch.create", &json!({"plane": "XY"})).unwrap();
    let id = s.active_sketch.unwrap();
    let mut sk = Sketch::new();
    let q = sk.add_point(Vec2::new(10.0, 0.0), None).unwrap();
    let l = sk.add_line_pts(0, q, None).unwrap();
    let k = ConstraintKind::Distance { p: 0, q, value: 10.0 };
    sk.add_constraint(k.clone(), None).unwrap();
    let redundant = sk.add_constraint(k, None).unwrap();
    if horizontal {
        sk.add_constraint(ConstraintKind::Horizontal { l }, None).unwrap();
    }
    *Arc::make_mut(&mut s.doc).sketch_mut(id).unwrap() = sk;
    (s, id, redundant)
}

#[test]
fn auto_constrain_computes_redundancy_once_and_publishes_full_diagnostics() {
    let (mut s, id, redundant) = session(true);
    let before = s.doc.sketch(id).unwrap().clone();
    let (response, profile) = capture(|| s.execute("sketch.auto_constrain", &json!({"dimensions": false})));
    let response = response.unwrap();
    assert_eq!(response["added"], json!([]));
    assert_eq!(response["sketch"], json!({"solved": true, "dof": 0}));
    assert_eq!(*s.doc.sketch(id).unwrap(), before);
    let state = s.model.state();
    let shown = state.sketch(id).unwrap();
    assert!(shown.report.ok());
    assert_eq!(shown.report.dof, 0);
    assert!(shown.report.redundant.contains(&redundant));
    assert!(shown.report.point_determined.iter().all(|d| *d));
    assert_eq!(profile.counters.redundancy_checks, 1, "only the published model report needs redundant IDs");
    assert_eq!(profile.counters.solve_calls, 3, "reuse the preparation solve's DOF");
}

#[test]
fn auto_constrain_reuses_real_freedom_and_keeps_accepted_trial_diagnostics() {
    let (mut s, id, redundant) = session(false);
    let (response, profile) = capture(|| s.execute("sketch.auto_constrain", &json!({"dimensions": false})));
    let response = response.unwrap();
    assert_eq!(response["added"].as_array().unwrap().len(), 1, "one angular degree of freedom must be removed");
    assert_eq!(response["dof"], 0);
    assert_eq!(response["sketch"], json!({"solved": true, "dof": 0}));
    let state = s.model.state();
    let shown = state.sketch(id).unwrap();
    assert!(shown.report.redundant.contains(&redundant));
    assert!(shown.report.ok());
    assert_eq!(profile.counters.redundancy_checks, 1);
}

#[test]
fn a_live_drag_keeps_full_diagnostics_when_it_publishes_without_evaluation() {
    for horizontal in [false, true] {
        let (mut s, id, redundant) = session(horizontal);
        // The fixture edits the document directly. Establish a valid full report before
        // testing its reuse; a no-op solve is not required to rebuild an unchanged model.
        s.model.evaluate(&s.doc);
        s.execute("sketch.solve", &json!({})).unwrap();
        assert!(
            s.model.state().sketch(id).unwrap().report.redundant.contains(&redundant),
            "the pre-drag model must contain the fixture's full diagnostics"
        );
        let endpoint = s.doc.sketch(id).unwrap().points[1].id.clone();
        s.execute("sketch.drag", &json!({"entity": endpoint, "from": [10, 0], "to": [12, 8], "live": true})).unwrap();
        let state = s.model.state();
        let shown = state.sketch(id).unwrap();
        assert!(shown.report.redundant.contains(&redundant));
        assert_eq!(shown.report.dof, usize::from(!horizontal));
        if !horizontal {
            assert!(s.doc.sketch(id).unwrap().points[1].pos.dist(Vec2::new(10.0, 0.0)) > 1.0, "exercise publication of changed geometry");
        }
    }
}

#[test]
fn a_live_text_edit_keeps_full_diagnostics_when_it_publishes_without_evaluation() {
    let (mut s, id, redundant) = session(false);
    let text = s.execute("sketch.text", &json!({"text": "T", "at": [30, 20], "height": 5})).unwrap();
    assert!(s.model.state().sketch(id).unwrap().report.redundant.contains(&redundant));
    s.execute("sketch.edit_text", &json!({"link": text["link"], "height": 8, "live": true})).unwrap();
    let state = s.model.state();
    let shown = state.sketch(id).unwrap();
    assert!(shown.report.ok());
    assert!(shown.report.redundant.contains(&redundant), "live publication must retain redundant IDs for unrelated geometry");
}

fn compare_document_levels(doc: &solidcraft_doc::Document, id: u64, expected_ok: bool) {
    let input = doc.sketch(id).unwrap();
    let stored = input.driving_values();
    let values = doc.param_values().0;
    let mut full_geometry = input.clone();
    let (full, profile) = capture(|| doc.solve_sketch(&values, &mut full_geometry, &stored, Some(input)).unwrap());
    assert_eq!(full.ok(), expected_ok);
    assert!(profile.counters.redundancy_checks > 0, "the default document API retains full diagnostics");
    if expected_ok {
        let (_, last_only) = capture(|| solidcraft_sketch::solve(&mut full_geometry.clone()));
        assert_eq!(profile.counters.redundancy_checks, last_only.counters.redundancy_checks, "only the last offset solve needs redundant IDs");
    }
    for analysis in [Analysis::Freedom, Analysis::StatusOnly] {
        let mut geometry = input.clone();
        let (report, profile) = capture(|| doc.solve_sketch_with_analysis(&values, &mut geometry, &stored, Some(input), analysis).unwrap());
        assert_eq!(geometry, full_geometry, "parameter application, offset refresh and rollback must match");
        assert_eq!(profile.counters.redundancy_checks, 0, "offset retries must retain the requested level");
        let mut expected = full.clone();
        expected.redundant.clear();
        if analysis == Analysis::StatusOnly {
            expected.dof = 0;
            expected.point_determined.clear();
            expected.curve_determined.clear();
        }
        assert_eq!(serde_json::to_value(report).unwrap(), serde_json::to_value(expected).unwrap());
    }
}

#[test]
fn document_analysis_levels_survive_parameter_application_and_offset_retries() {
    for freeform in [false, true] {
        let (mut s, id, _) = session(false);
        let curve = if freeform {
            s.execute("sketch.ellipse", &json!({"center": [30, 30], "major": [40, 30], "minor_radius": 5})).unwrap()["curves"][0]
                .as_str()
                .unwrap()
                .to_owned()
        } else {
            s.doc.sketch(id).unwrap().curves[0].id.clone()
        };
        s.execute("sketch.offset", &json!({"curves": [curve], "distance": -1, "chain": false})).unwrap();
        let sk = s.doc.sketch(id).unwrap();
        assert!(if freeform { !sk.freeform_offsets.is_empty() } else { !sk.offsets.is_empty() });
        compare_document_levels(&s.doc, id, true);

        // Both original distances must follow the edited parameter before analyzing freedom.
        let doc = Arc::make_mut(&mut s.doc);
        let param = doc.new_model_param("20 mm", "mm");
        for c in &mut doc.sketch_mut(id).unwrap().constraints {
            if matches!(c.kind, ConstraintKind::Distance { .. }) {
                c.param = Some(param.clone());
            }
        }
        compare_document_levels(doc, id, true);
        // One conflicting dimension makes the first solve fail, exercising the offset retry.
        let conflict = doc.new_model_param("11 mm", "mm");
        doc.sketch_mut(id).unwrap().constraints.iter_mut().find(|c| matches!(c.kind, ConstraintKind::Distance { .. })).unwrap().param =
            Some(conflict);
        compare_document_levels(doc, id, false);
    }
}
