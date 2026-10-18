use serde_json::json;
use solidcraft_engine::Session;

use super::*;
use crate::Services;
use crate::dialogs::{apply_commands, face_sel, for_feature as edit_dialog};

fn app_with(cmds: &[(&str, Value)]) -> SolveApp {
    let mut s = Session::default();
    for (id, p) in cmds {
        s.execute(id, p).unwrap();
    }
    SolveApp::new(s, Services::default())
}

fn run_all(app: &mut SolveApp, c: Vec<(String, Value)>) {
    for (id, p) in c {
        app.run(&id, p).unwrap();
    }
}

/// A face pick places a boss; optional values left empty take the rule; ribs only with a count;
/// Edit Feature keeps the direction and refills the values.
#[test]
fn boss_from_a_face_pick() {
    let mut app = app_with(&[("solid.box", json!({"length": 60, "width": 60, "height": 3}))]);
    app.session.selection = face_sel(&app.session, Vec3::new(30.0, 30.0, 3.0)).into_iter().collect();
    app.start("plastic.boss");
    let mut d = app.dialog.clone().unwrap();
    assert_eq!(d.primary().map(|p| p.0), Some("Height"));
    let c = apply_commands(&app, &d).unwrap();
    let p = &c[0].1;
    assert_eq!(p["position"], json!([30.0, 30.0, 3.0]));
    assert_eq!(p["hole_diameter"], "3 mm");
    assert!(p.get("draft").is_none() && p.get("ribs").is_none(), "{p}");
    if let Kind::Plastic(Pl::Boss { ribs, .. }) = &mut d.kind {
        *ribs = "4".into();
    }
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].1["ribs"], "4");
    run_all(&mut app, c);
    let f = app.session.doc.features.last().unwrap().clone();
    let e = edit_dialog(&app, f.id, None).unwrap();
    assert!(matches!(&e.kind, Kind::Plastic(Pl::Boss { ribs, .. }) if ribs == "4"));
    let c = apply_commands(&app, &e).unwrap();
    assert_eq!(c[0].0, "timeline.redefine");
    assert_eq!(c[0].1["params"]["direction"], json!([0.0, 0.0, 1.0]));
}

/// A snap fit's catch points across the face; a groove takes its clearance; rests can be round
/// or rectangular.
#[test]
fn snap_lip_rest() {
    for n in [Vec3::Z, Vec3::X, Vec3::new(0.0, -1.0, 0.0)] {
        let dirs = hook_dirs(n);
        assert_eq!(dirs.len(), 4);
        assert!(dirs.iter().all(|(_, d)| d.dot(n).abs() < 1e-12 && (d.len() - 1.0).abs() < 1e-12));
    }
    let mut app = app_with(&[
        ("solid.box", json!({"length": 80, "width": 50, "height": 30})),
        ("solid.shell", json!({"faces": [[40, 25, 30]], "thickness": 2})),
    ]);
    app.session.selection = face_sel(&app.session, Vec3::new(1.0, 25.0, 30.0)).into_iter().collect();
    app.start("plastic.lip");
    let mut d = app.dialog.clone().unwrap();
    if let Kind::Plastic(Pl::Lip { groove, gap, .. }) = &mut d.kind {
        *groove = true;
        *gap = "0.2 mm".into();
    }
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].1["type"], "groove");
    assert_eq!(c[0].1["gap"], "0.2 mm");
    run_all(&mut app, c);
    let mut app = app_with(&[("solid.box", json!({"length": 40, "width": 40, "height": 4}))]);
    app.session.selection = face_sel(&app.session, Vec3::new(20.0, 20.0, 4.0)).into_iter().collect();
    app.start("plastic.snap_fit");
    let c = apply_commands(&app, app.dialog.as_ref().unwrap()).unwrap();
    assert_eq!(c[0].1["hook"], json!([1.0, 0.0, 0.0]));
    run_all(&mut app, c);
    app.dialog = None;
    app.session.selection = face_sel(&app.session, Vec3::new(5.0, 5.0, 4.0)).into_iter().collect();
    app.start("plastic.rest");
    let mut d = app.dialog.clone().unwrap();
    assert!(apply_commands(&app, &d).unwrap()[0].1.get("length").is_none(), "round");
    if let Kind::Plastic(Pl::Rest { round, .. }) = &mut d.kind {
        *round = false;
    }
    assert_eq!(apply_commands(&app, &d).unwrap()[0].1["length"], "15 mm");
}

/// The rule manager edits a library rule into the design; Assign gives bodies a rule.
#[test]
fn plastic_rules_manage_and_assign() {
    let mut app = app_with(&[("solid.box", json!({"length": 10, "width": 10, "height": 10}))]);
    app.start("plastic.manage_rules");
    let d = app.dialog.clone().unwrap();
    assert!(apply_commands(&app, &d).is_ok());
    app.session.selection = vec![Sel::Body { name: "Body1".into() }];
    app.start("plastic.assign_rule");
    let c = apply_commands(&app, app.dialog.as_ref().unwrap()).unwrap();
    assert_eq!(c[0].1["bodies"], json!(["Body1"]));
    assert_eq!(c[0].1["rule"], "ABS (1.5mm)");
    run_all(&mut app, c);
}

/// Features stored with bare numbers (as the old Plastic Enclosure sample did) edit with their
/// units written out, and re-applying keeps the values (#1936).
#[test]
fn bare_numbers_show_their_units() {
    let mut app = app_with(&[
        ("solid.box", json!({"length": 80, "width": 50, "height": 30})),
        ("solid.shell", json!({"faces": [[40, 25, 30]], "thickness": 2})),
        ("plastic.lip", json!({"face": [1, 25, 30], "width": 1, "height": 2})),
        (
            "plastic.snap_fit",
            json!({"position": [40, 44, 2], "hook": [0, -1, 0], "length": 14, "thickness": 1.5, "width": 6, "catch_depth": 1, "catch_length": 2}),
        ),
        (
            "plastic.boss",
            json!({"position": [10, 10, 2], "diameter": 7, "height": 12, "hole_diameter": 2.5, "draft": 1, "ribs": 4, "rib_thickness": 1, "rib_length": 3, "rib_offset": 1}),
        ),
    ]);
    let ids: Vec<_> = app.session.doc.features.iter().rev().take(3).map(|f| f.id).collect();
    let (boss, snap, lip) = (ids[0], ids[1], ids[2]);
    let d = edit_dialog(&app, lip, None).unwrap();
    let before = apply_commands(&app, &d).unwrap();
    assert!(matches!(&d.kind, Kind::Plastic(Pl::Lip { width, height, .. }) if width == "1 mm" && height == "2 mm"));
    let d = edit_dialog(&app, snap, None).unwrap();
    assert!(matches!(&d.kind, Kind::Plastic(Pl::SnapFit { length, thickness, width, catch_depth, catch_length, .. })
        if length == "14 mm" && thickness == "1.5 mm" && width == "6 mm" && catch_depth == "1 mm" && catch_length == "2 mm"));
    let d = edit_dialog(&app, boss, None).unwrap();
    assert!(matches!(&d.kind, Kind::Plastic(Pl::Boss { diameter, height, hole_diameter, draft, ribs, rib_thickness, rib_length, rib_offset, .. })
        if diameter == "7 mm" && height == "12 mm" && hole_diameter == "2.5 mm" && draft == "1 deg" && ribs == "4"
            && rib_thickness == "1 mm" && rib_length == "3 mm" && rib_offset == "1 mm"));
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].1["params"]["diameter"], "7 mm");
    assert_eq!(c[0].1["params"]["draft"], "1 deg");
    assert_eq!(c[0].1["params"]["ribs"], "4");
    // Re-applying the dialog redefines the features in place (no new ones, no errors).
    let count = |app: &SolveApp| app.session.doc.features.len();
    let n = count(&app);
    run_all(&mut app, c);
    run_all(&mut app, before);
    assert_eq!(count(&app), n);
}

/// `shown_expr` adds the unit only to a bare number of a length or an angle.
#[test]
fn shown_expr_adds_units_to_bare_numbers() {
    use crate::dialogs::shown_expr;
    assert_eq!(shown_expr("2", ValueKind::Length), "2 mm");
    assert_eq!(shown_expr("-1.5", ValueKind::Length), "-1.5 mm");
    assert_eq!(shown_expr("5", ValueKind::Angle), "5 deg");
    assert_eq!(shown_expr("4", ValueKind::Unitless), "4");
    assert_eq!(shown_expr("2 mm", ValueKind::Length), "2 mm");
    assert_eq!(shown_expr("w * 2", ValueKind::Length), "w * 2");
    assert_eq!(shown_expr("", ValueKind::Length), "");
}
