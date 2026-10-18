use serde_json::json;
use solidcraft_engine::Session;

use super::*;
use crate::Services;
use crate::dialogs::{apply_commands, for_feature as edit_dialog};

/// A 100 x 60 base flange on XY (2.5 mm steel).
fn plate() -> SolveApp {
    let mut s = Session::default();
    s.execute("sketch.create", &json!({"plane": "XY", "name": "Base"})).unwrap();
    s.execute("sketch.rectangle.two_point", &json!({"p0": [0, 0], "p1": [100, 60]})).unwrap();
    s.execute("sketch.finish", &json!({})).unwrap();
    SolveApp::new(s, Services::default())
}

fn run_all(app: &mut SolveApp, c: Vec<(String, Value)>) {
    for (id, p) in c {
        app.run(&id, p).unwrap();
    }
}

/// The picks decide the flange: a profile makes the base, edges then edge flanges with the
/// dialog's height, angle and bend position; Edit Feature brings the edges back.
#[test]
fn flange_follows_the_picks() {
    let mut app = plate();
    let sk = app.session.doc.features[0].id;
    app.session.selection = vec![Sel::Profile { sketch: sk, index: 0 }];
    app.start("sheet.flange");
    let d = app.dialog.clone().unwrap();
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].1["type"], "base");
    assert_eq!(c[0].1["profiles"], json!([0]));
    run_all(&mut app, c);
    assert_eq!(app.session.model.state().sheets.len(), 1);
    // Four edge flanges at once, 20 high, outside.
    let edges: Vec<Sel> = [[50.0, 0.0, 2.5], [50.0, 60.0, 2.5], [0.0, 30.0, 2.5], [100.0, 30.0, 2.5]]
        .iter()
        .filter_map(|p| edge_sel(&app.session, Vec3::new(p[0], p[1], p[2])))
        .collect();
    assert_eq!(edges.len(), 4);
    app.session.selection = edges;
    app.start("sheet.flange");
    let mut d = app.dialog.clone().unwrap();
    if let Kind::Sheet(Sm::Flange { height, position, .. }) = &mut d.kind {
        *height = "20 mm".into();
        *position = 1;
    }
    assert_eq!(d.primary().map(|p| p.0), Some("Height"), "the arrow drives the height");
    assert!(arrow(&app, &d).is_some_and(|n| n.dist(Vec3::Z) < 1e-9), "edge flanges rise out of the sheet");
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].1["type"], "edge");
    assert_eq!(c[0].1["edges"].as_array().map(Vec::len), Some(4));
    assert_eq!(c[0].1["position"], "outside");
    run_all(&mut app, c);
    let f = app.session.doc.features.last().unwrap().clone();
    let e = edit_dialog(&app, f.id, None).unwrap();
    assert_eq!(e.inputs[0].items.len(), 4);
    assert!(matches!(&e.kind, Kind::Sheet(Sm::Flange { ty: 1, position: 1, .. })));
    // Rebuilt in place.
    let c = apply_commands(&app, &e).unwrap();
    assert_eq!(c[0].0, "timeline.redefine");
    assert_eq!(c[0].1["params"]["height"], "20 mm");
    // Flat pattern: four bends; Unfold previews.
    let flat = app.session.execute("sheet.flat_pattern", &json!({})).unwrap();
    assert_eq!(flat["bends"].as_array().map(Vec::len), Some(4), "{flat}");
    app.dialog = None;
    app.start("sheet.unfold");
    assert_eq!(apply_commands(&app, app.dialog.as_ref().unwrap()).unwrap()[0].0, "sheet.unfold");
}

/// Hem takes edges and an optional gap; rules make and pick a rule with its expressions.
#[test]
fn hem_and_rules() {
    let mut app = plate();
    app.run("sheet.flange", json!({"sketch": "Base"})).unwrap();
    app.session.selection = edge_sel(&app.session, Vec3::new(50.0, 0.0, 2.5)).into_iter().collect();
    app.start("sheet.hem");
    let d = app.dialog.clone().unwrap();
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].1["length"], "5 mm");
    assert!(c[0].1.get("gap").is_none(), "the rule's gap");
    run_all(&mut app, c);
    app.dialog = None;
    app.start("sheet.manage_rules");
    let mut d = app.dialog.clone().unwrap();
    if let Kind::Sheet(Sm::Rules { pick, name, values, loaded, .. }) = &mut d.kind {
        *pick = 1;
        *loaded = Some(1);
        *name = "Alu".into();
        *values = vec![
            "1.5 mm".into(),
            "0.4".into(),
            "Thickness".into(),
            "Thickness".into(),
            "Thickness".into(),
            "Thickness".into(),
            "Thickness".into(),
            "Thickness".into(),
        ];
    }
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].1["name"], "Alu");
    assert_eq!(c[0].1["thickness"], "1.5 mm");
    run_all(&mut app, c);
    assert!(app.session.doc.sheet.rules.iter().any(|r| r.name == "Alu"));
}

#[test]
fn flat_exports_use_the_save_picker() {
    use std::sync::{Arc, Mutex};
    let mut app = plate();
    app.run("sheet.flange", json!({"sketch": "Base"})).unwrap();
    let picks = Arc::new(Mutex::new(Vec::new()));
    let captured = picks.clone();
    app.services.pick_save = Some(Box::new(move |name, extensions| {
        captured.lock().unwrap().push((name.to_string(), extensions.iter().map(|x| x.to_string()).collect::<Vec<_>>()));
        Box::pin(std::future::ready(None))
    }));
    assert_eq!(export_flat(&mut app, "Base", "csv"), "Choose an export file…");
    crate::file_picker::tick(&mut app, &egui::Context::default());
    assert_eq!(export_flat(&mut app, "Base", "dxf"), "Choose an export file…");
    crate::file_picker::tick(&mut app, &egui::Context::default());
    let picks = picks.lock().unwrap();
    assert!(picks[0].0.ends_with("-Base.csv"));
    assert_eq!(picks[0].1, vec!["csv"]);
    assert!(picks[1].0.ends_with("-Base.dxf"));
    assert_eq!(picks[1].1, vec!["dxf"]);
}

#[test]
fn shallow_pocket_overlay_labels_depth_without_drawing_a_through_void() {
    let mut app = plate();
    app.run("sheet.flange", json!({"sketch": "Base"})).unwrap();
    app.run("sketch.create", json!({"plane": "XY", "name": "Pocket"})).unwrap();
    app.run("sketch.rectangle.two_point", json!({"p0": [20, 20], "p1": [30, 30]})).unwrap();
    app.run("sketch.finish", json!({})).unwrap();
    app.run("solid.extrude", json!({"sketch": "Pocket", "distance": 1, "operation": "cut"})).unwrap();
    let flat = app.session.execute("sheet.flat_pattern", &json!({})).unwrap();
    assert_eq!(flat["cutouts"], json!([]));
    assert_eq!(flat["pockets"][0]["depth_mm"], 1.0);
    app.start("sheet.flat_pattern");
    let mut dialog = app.dialog.clone().unwrap();
    let Kind::Sheet(Sm::FlatPattern { data, .. }) = &mut dialog.kind else { panic!("flat dialog") };
    *data = Some(Ok(flat));
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("flat-pocket-test")));
        assert!(overlay(&mut app, &painter, Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0)), &dialog));
    });
    output.textures_delta.clear();
    let texts: Vec<&str> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect();
    assert!(texts.contains(&"Bottom pocket: 1.00 mm deep"));
    assert!(texts.contains(&"Pocket boundary (material remains)"));
    let paths: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Path(path) => Some(path),
            _ => None,
        })
        .collect();
    assert!(!paths.is_empty(), "the retained sheet outline is drawn");
    assert!(paths.iter().all(|p| p.fill == Color32::TRANSPARENT), "pockets must not be rendered as filled voids");
    let lines = output.shapes.iter().filter(|s| matches!(s.shape, egui::Shape::LineSegment { .. })).count();
    assert!(lines > 6, "pocket contours and legend use dashed lines, not through-cut strokes");
}

#[test]
fn flat_pattern_islands_fit_before_the_actual_command_panel() {
    let mut app = plate();
    app.run("sheet.flange", json!({"sketch": "Base"})).unwrap();
    app.run("sketch.create", json!({"plane": "XY", "name": "Retain"})).unwrap();
    for x in [20, 60] {
        app.run("sketch.rectangle.two_point", json!({"p0": [x, 20], "p1": [x + 10, 30]})).unwrap();
    }
    app.run("sketch.finish", json!({})).unwrap();
    app.run("solid.extrude", json!({"sketch": "Retain", "profiles": "all", "through_all": true, "direction": "symmetric", "operation": "intersect"}))
        .unwrap();
    let flat = app.session.execute("sheet.flat_pattern", &json!({})).unwrap();
    assert_eq!(flat["outline"].as_array().unwrap().len(), 2);
    app.start("sheet.flat_pattern");
    let mut dialog = app.dialog.clone().unwrap();
    let Kind::Sheet(Sm::FlatPattern { data, .. }) = &mut dialog.kind else { panic!("flat dialog") };
    *data = Some(Ok(flat));
    let ctx = egui::Context::default();
    let id = egui::Id::new(("sc_dialog", "FLAT PATTERN"));
    let mut output = egui::FullOutput::default();
    for _ in 0..3 {
        output = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), ..Default::default() }, |ui| {
            crate::frame::docked("FLAT PATTERN", id, pos2(1400.0, 294.0), 540.0, crate::frame::Width::Normal).min_width(360.0).show(ui.ctx(), |ui| {
                ui.label("Flat size: 50 × 10 × 2.5 mm");
            });
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("island-fit")));
            assert!(overlay(
                &mut app,
                &painter,
                Rect::from_min_max(pos2(250.0, crate::toolbar::APP_BAR_HEIGHT + crate::toolbar::RIBBON_HEIGHT), pos2(1400.0, 832.0)),
                &dialog
            ));
        });
        output.textures_delta.clear();
    }
    let panel = ctx.memory(|m| m.area_rect(id)).unwrap();
    let outlines: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Path(p) if p.closed && p.fill == Color32::TRANSPARENT => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(outlines.len(), 2, "both retained islands are drawn");
    assert!(
        outlines.iter().flat_map(|p| &p.points).all(|p| p.x <= panel.left() - 19.0),
        "both complete islands must remain exposed before the command panel: {panel:?}"
    );
}

#[test]
fn coincident_top_and_bottom_pocket_depths_share_a_readable_annotation() {
    let mut app = plate();
    app.run("sheet.flange", json!({"sketch":"Base"})).unwrap();
    app.run("sketch.create", json!({"plane":"XY","name":"Retain"})).unwrap();
    app.run("sketch.rectangle.two_point", json!({"p0":[20,20],"p1":[30,30]})).unwrap();
    app.run("sketch.finish", json!({})).unwrap();
    app.run("solid.extrude", json!({"sketch":"Retain","distance":1,"start_offset":0.5,"operation":"intersect"})).unwrap();
    let flat = app.session.execute("sheet.flat_pattern", &json!({})).unwrap();
    assert_eq!(flat["cutouts"], json!([]));
    assert_eq!(flat["pockets"].as_array().unwrap().len(), 2);
    let doc = app.session.doc.to_json();
    app.start("sheet.flat_pattern");
    let mut dialog = app.dialog.clone().unwrap();
    let Kind::Sheet(Sm::FlatPattern { data, .. }) = &mut dialog.kind else { panic!("flat dialog") };
    *data = Some(Ok(flat));
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("coincident-pocket-test")));
        assert!(overlay(&mut app, &painter, Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0)), &dialog));
    });
    output.textures_delta.clear();
    let labels: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().contains("pocket:") => Some(text.galley.text()),
            _ => None,
        })
        .collect();
    assert_eq!(labels.len(), 1, "coincident depths must not overwrite one another");
    assert!(labels[0].contains("Top pocket: 1.00 mm deep"));
    assert!(labels[0].contains("Bottom pocket: 0.50 mm deep"));
    assert!(labels[0].contains('\n'));
    assert_eq!(app.session.doc.to_json(), doc);
}

#[test]
fn pocket_export_button_uses_the_picker_and_preserves_depth_layers() {
    let mut h = crate::scenario::Harness::new();
    for (command, params) in [
        ("sketch.create", json!({"plane":"XY","name":"Base"})),
        ("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[100,60]})),
        ("sketch.finish", json!({})),
        ("sheet.flange", json!({"sketch":"Base"})),
        ("sketch.create", json!({"plane":"XY","name":"Pocket"})),
        ("sketch.rectangle.two_point", json!({"p0":[20,20],"p1":[30,30]})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"sketch":"Pocket","distance":1,"operation":"cut"})),
    ] {
        h.app.run(command, params).unwrap();
    }
    let path = h.dir.join("picked-pocket.dxf");
    let chosen = path.to_string_lossy().into_owned();
    h.app.services.pick_save = Some(Box::new(move |name, extensions| {
        assert!(name.ends_with("-Body1.dxf"));
        assert_eq!(extensions, &["dxf"]);
        Box::pin(std::future::ready(Some(chosen.clone())))
    }));
    h.app.start("sheet.flat_pattern");
    h.settle();
    let undo = h.app.session.undo.len();
    h.step(&json!({"widget":"Export DXF…"})).unwrap();
    let exported = std::fs::read_to_string(&path).expect("actual Export DXF button writes the picker destination");
    assert!(exported.contains("POCKET_BOTTOM_Z0.000000_TO_1.000000_MM"));
    assert!(exported.contains("OUTLINE"));
    assert_eq!(h.app.session.undo.len(), undo, "export does not alter the part");
    assert!(h.app.dialog.is_some(), "export keeps the flat viewer open");
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn bend_table_picker_writes_the_current_body() {
    let mut app = plate();
    app.run("sheet.flange", json!({"sketch": "Base"})).unwrap();
    app.run("sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 20})).unwrap();
    let body = app.session.model.state().sheets[0].body.clone();
    let path = std::env::temp_dir().join(format!("solidcraft-bend-table-{}.csv", std::process::id()));
    let output = path.to_string_lossy().into_owned();
    let chosen = output.clone();
    app.services.pick_save = Some(Box::new(move |_, extensions| {
        assert_eq!(extensions, &["csv"]);
        Box::pin(std::future::ready(Some(chosen.clone())))
    }));
    let undo = app.session.undo.len();
    assert_eq!(export_flat(&mut app, &body, "csv"), "Choose an export file…");
    crate::file_picker::tick(&mut app, &egui::Context::default());
    assert_eq!(app.status.as_ref().map(|s| &s.0), Some(&format!("Exported {output}")));
    let contents = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(contents.lines().count(), 2);
    assert!(contents.contains(",90.000000000,up,2.500000000,"), "{contents}");
    assert_eq!(app.session.undo.len(), undo);
}

/// The rule manager's shape selector: picked in the dialog, the shape reaches the rule and the part
/// made with it, and the dialog shows it the next time.
#[test]
fn rules_dialog_picks_the_corner_relief_shape() {
    let mut h = crate::scenario::Harness::new();
    h.app
        .run(
            "sheet.manage_rules",
            json!({"name": "AL2", "thickness": "2 mm", "k_factor": 0.5, "bend_radius": "1 mm", "gap": "0.5 mm", "active": true}),
        )
        .unwrap();
    h.app.run("sketch.create", json!({"plane": "XY", "name": "Base"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [120, 90]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("sheet.flange", json!({"sketch": "Base"})).unwrap();
    h.app.run("sheet.flange", json!({"edges": [[60, 0, 2], [120, 45, 2], [60, 90, 2], [0, 45, 2]], "height": 20, "angle": "72 deg"})).unwrap();
    // The rounds in the base of the tray, and the shape the open dialog shows.
    let rounds = |app: &SolveApp| {
        let state = app.session.model.state();
        state.sheets.last().map(|s| s.base.outer.segs.iter().filter(|g| matches!(g, solidcraft_engine::geom::Seg2::Arc { .. })).count())
    };
    let shown = |app: &SolveApp| match app.dialog.as_ref().map(|d| &d.kind) {
        Some(Kind::Sheet(Sm::Rules { relief, .. })) => Some(*relief),
        _ => None,
    };
    assert_eq!(rounds(&h.app), Some(0));
    h.step(&json!({"start": "sheet.manage_rules"})).unwrap();
    h.frames(3);
    assert_eq!(shown(&h.app), Some(0), "a rule starts with the notch");
    h.step(&json!({"click": {"handle": "dialog:Corner Relief Shape"}})).unwrap();
    h.step(&json!({"click": {"handle": "dialog:Corner Relief Shape:Round"}})).unwrap();
    assert_eq!(shown(&h.app), Some(1));
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
    assert_eq!(h.app.session.doc.sheet_rule(None).corner_relief_shape, "round");
    assert_eq!(rounds(&h.app), Some(4), "one round at each corner of the tray");
    h.step(&json!({"start": "sheet.manage_rules"})).unwrap();
    h.frames(3);
    assert_eq!(shown(&h.app), Some(1), "the dialog shows the rule's shape");
    // The tear: no relief in the base, both ends of all four bends torn.
    h.step(&json!({"click": {"handle": "dialog:Corner Relief Shape"}})).unwrap();
    h.step(&json!({"click": {"handle": "dialog:Corner Relief Shape:Tear"}})).unwrap();
    assert_eq!(shown(&h.app), Some(3));
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
    assert_eq!(h.app.session.doc.sheet_rule(None).corner_relief_shape, "tear");
    let torn = h.app.session.model.state().sheets.last().map(|s| s.flanges.iter().filter(|f| f.tear == (true, true)).count());
    assert_eq!((rounds(&h.app), torn), (Some(0), Some(4)));
}

/// The Flange dialog's relief rows: Rule (the default) and an empty size leave the relief to the rule,
/// a shape and a size reach the feature, Edit Feature shows them again, and Rule takes them back.
#[test]
fn flange_dialog_picks_a_corner_relief_of_its_own() {
    let mut h = crate::scenario::Harness::new();
    h.app
        .run(
            "sheet.manage_rules",
            json!({"name": "AL2", "thickness": "2 mm", "k_factor": 0.5, "bend_radius": "1 mm", "gap": "0.5 mm", "active": true}),
        )
        .unwrap();
    h.app.run("sketch.create", json!({"plane": "XY", "name": "Base"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [120, 90]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("sheet.flange", json!({"sketch": "Base"})).unwrap();
    let edges: Vec<Sel> = [[60.0, 0.0, 2.0], [0.0, 45.0, 2.0]].iter().filter_map(|p| edge_sel(&h.app.session, Vec3::new(p[0], p[1], p[2]))).collect();
    assert_eq!(edges.len(), 2);
    h.app.session.selection = edges;
    h.app.start("sheet.flange");
    h.frames(3);
    let shown = |app: &SolveApp| match app.dialog.as_ref().map(|d| &d.kind) {
        Some(Kind::Sheet(Sm::Flange { relief, relief_size, .. })) => Some((*relief, relief_size.clone())),
        _ => None,
    };
    let relief = |app: &SolveApp| match &app.session.doc.features.last().map(|f| &f.kind) {
        Some(FeatureKind::SheetFlange { corner_relief_shape, corner_relief, .. }) => Some((corner_relief_shape.clone(), corner_relief.clone())),
        _ => None,
    };
    // Untouched, the dialog asks for nothing: the rule's relief.
    assert_eq!(shown(&h.app), Some((0, String::new())));
    let d = h.app.dialog.clone().unwrap();
    let c = apply_commands(&h.app, &d).unwrap();
    assert!(c[0].1.get("corner_relief_shape").is_none() && c[0].1.get("corner_relief").is_none(), "{}", c[0].1);
    // Round from the selector, 6 mm in the size row.
    h.step(&json!({"click": {"handle": "dialog:Corner Relief Shape"}})).unwrap();
    h.step(&json!({"click": {"handle": "dialog:Corner Relief Shape:Round"}})).unwrap();
    assert_eq!(shown(&h.app), Some((2, String::new())));
    if let Some(Kind::Sheet(Sm::Flange { relief_size, .. })) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
        *relief_size = "6 mm".into();
    }
    let d = h.app.dialog.clone().unwrap();
    let c = apply_commands(&h.app, &d).unwrap();
    assert_eq!((&c[0].1["corner_relief_shape"], &c[0].1["corner_relief"]), (&json!("round"), &json!("6 mm")));
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
    assert_eq!(relief(&h.app), Some((Some("round".into()), Some("6 mm".into()))));
    assert!(h.app.session.model.results.iter().all(|r| r.error.is_none() && r.warning.is_none()), "{:?}", h.app.session.model.results);
    let arcs = |app: &SolveApp| {
        app.session
            .model
            .state()
            .sheets
            .last()
            .map(|s| s.base.outer.segs.iter().filter(|g| matches!(g, solidcraft_engine::geom::Seg2::Arc { .. })).count())
    };
    assert_eq!(arcs(&h.app), Some(1), "the round is cut at the one corner the two flanges make");
    // Edit Feature shows both again; Rule and an empty size take the feature back to the rule's relief.
    let id = h.app.session.doc.features.last().map(|f| f.id).unwrap();
    let mut e = edit_dialog(&h.app, id, None).unwrap();
    assert!(matches!(&e.kind, Kind::Sheet(Sm::Flange { relief: 2, relief_size, .. }) if relief_size == "6 mm"));
    let c = apply_commands(&h.app, &e).unwrap();
    assert_eq!((&c[0].1["params"]["corner_relief_shape"], &c[0].1["params"]["corner_relief"]), (&json!("round"), &json!("6 mm")));
    if let Kind::Sheet(Sm::Flange { relief, relief_size, .. }) = &mut e.kind {
        *relief = 0;
        relief_size.clear();
    }
    let commands = apply_commands(&h.app, &e).unwrap();
    run_all(&mut h.app, commands);
    assert_eq!(relief(&h.app), Some((None, None)));
    assert_eq!(arcs(&h.app), Some(0));
}

#[test]
fn pocket_depth_labels_follow_inches_without_changing_flat_data() {
    let mut app = plate();
    app.run("document.units", json!({"units":"in"})).unwrap();
    assert_eq!(super::pocket_label(&app, &json!({"side":"top","depth_mm":25.4*0.1234})).as_deref(), Some("Top pocket: 0.1234 in deep"));
    assert!(super::pocket_label(&app, &json!({"side":"top","depth_mm":-1})).is_none());
}

/// Edges with an angle and/or a height of their own: the texts of the dialog's rows per edge
/// become `{point, angle?, height?}` edges while the others stay plain points; Edit Feature
/// brings them back and rebuilds the same flanges.
#[test]
fn edge_flanges_can_have_an_angle_and_a_height_of_their_own() {
    let mut app = plate();
    app.run("sheet.flange", json!({"sketch": "Base"})).unwrap();
    let edges: Vec<Sel> = [[50.0, 0.0, 2.5], [50.0, 60.0, 2.5], [0.0, 30.0, 2.5], [100.0, 30.0, 2.5]]
        .iter()
        .filter_map(|p| edge_sel(&app.session, Vec3::new(p[0], p[1], p[2])))
        .collect();
    assert_eq!(edges.len(), 4);
    let point = |i: usize| if let Sel::Edge { point, .. } = &edges[i] { *point } else { Vec3::ZERO };
    app.session.selection = edges.clone();
    app.start("sheet.flange");
    let mut d = app.dialog.clone().unwrap();
    if let Kind::Sheet(Sm::Flange { height, angle, own, .. }) = &mut d.kind {
        *height = "20 mm".into();
        *angle = "75 deg".into();
        // The second edge: its own angle; the fourth: both; the third: texts left blank.
        own.push((point(1), "100 deg".into(), String::new()));
        own.push((point(3), "90 deg".into(), "15 mm".into()));
        own.push((point(2), "  ".into(), String::new()));
    }
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(
        c[0].1["edges"],
        json!([pt(point(0)), {"point": pt(point(1)), "angle": "100 deg"}, pt(point(2)), {"point": pt(point(3)), "angle": "90 deg", "height": "15 mm"}]),
        "no text or a blank one: a plain point"
    );
    run_all(&mut app, c);
    let bends = app.session.execute("sheet.flat_pattern", &json!({})).unwrap()["bends"].clone();
    let angles: Vec<f64> = bends.as_array().unwrap().iter().map(|b| b["angle_deg"].as_f64().unwrap().round()).collect();
    assert_eq!(angles, [75.0, 100.0, 75.0, 90.0], "{bends}");
    assert!(app.session.model.results.iter().all(|r| r.error.is_none()), "{:?}", app.session.model.results);
    // Edit Feature: the edges and their own values come back, and OK rebuilds the same part.
    let f = app.session.doc.features.last().unwrap().clone();
    let mut e = edit_dialog(&app, f.id, None).unwrap();
    assert_eq!(e.inputs[0].items.len(), 4);
    let Kind::Sheet(Sm::Flange { own, .. }) = &e.kind else { panic!("not a flange dialog") };
    assert_eq!(own.len(), 2, "{own:?}");
    assert!(own.iter().any(|o| o.0.dist(point(1)) < 1e-9 && o.1 == "100 deg" && o.2.is_empty()), "{own:?}");
    assert!(own.iter().any(|o| o.0.dist(point(3)) < 1e-9 && o.1 == "90 deg" && o.2 == "15 mm"), "{own:?}");
    let volume = |app: &mut SolveApp| app.session.execute("inspect.measure", &json!({})).unwrap()["total"]["volume_mm3"].as_f64().unwrap();
    let before = volume(&mut app);
    let c = apply_commands(&app, &e).unwrap();
    assert_eq!(c[0].0, "timeline.redefine");
    assert_eq!(c[0].1["params"]["edges"][1], json!({"point": pt(point(1)), "angle": "100 deg"}));
    assert_eq!(c[0].1["params"]["edges"][3]["height"], "15 mm");
    run_all(&mut app, c);
    let after = volume(&mut app);
    assert!((after - before).abs() < 1e-6 * before, "{after} vs {before}");
    // Taking edges out of the selection leaves each value with its edge.
    e.inputs[0].items.remove(2);
    e.inputs[0].items.remove(0);
    let c = apply_commands(&app, &e).unwrap();
    assert_eq!(
        c[0].1["params"]["edges"],
        json!([{"point": pt(point(1)), "angle": "100 deg"}, {"point": pt(point(3)), "angle": "90 deg", "height": "15 mm"}])
    );
}

/// Convert takes a fixed face and, for a solid or shelled body, its bend edges; Edit Feature
/// brings both back.
#[test]
fn convert_takes_the_fixed_face_and_the_bend_edges() {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 100, "width": 60, "height": 30, "body_name": "Tray"})).unwrap();
    s.execute("solid.shell", &json!({"faces": [[50, 30, 30]], "thickness": 2})).unwrap();
    let mut app = SolveApp::new(s, Services::default());
    app.start("sheet.convert");
    let mut d = app.dialog.clone().unwrap();
    assert_eq!(d.inputs.iter().map(|i| i.label).collect::<Vec<_>>(), ["Fixed Face", "Bend Edges"]);
    assert!(apply_commands(&app, &d).is_err(), "nothing to convert without a face");
    // The face alone is the plate conversion: no `bends`.
    d.inputs[0].items = face_sel(&app.session, Vec3::new(50.0, 30.0, 0.0)).into_iter().collect();
    assert!(apply_commands(&app, &d).unwrap()[0].1.get("bends").is_none());
    let edges: Vec<Sel> = [[50.0, 0.0, 0.0], [100.0, 30.0, 0.0], [50.0, 60.0, 0.0], [0.0, 30.0, 0.0]]
        .iter()
        .filter_map(|p| edge_sel(&app.session, Vec3::new(p[0], p[1], p[2])))
        .collect();
    d.inputs[1].items = edges;
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].0, "sheet.convert");
    assert_eq!(c[0].1["bends"], json!([[50.0, 0.0, 0.0], [100.0, 30.0, 0.0], [50.0, 60.0, 0.0], [0.0, 30.0, 0.0]]));
    run_all(&mut app, c);
    let bends = |app: &mut SolveApp| app.session.execute("sheet.flat_pattern", &json!({})).unwrap()["bends"].as_array().map(Vec::len);
    assert_eq!(bends(&mut app), Some(4));
    // Edit Feature: the face and the four edges are back in their rows, and OK rebuilds the sheet.
    let f = app.session.doc.features.last().unwrap().clone();
    let e = edit_dialog(&app, f.id, None).unwrap();
    assert_eq!((e.inputs[0].items.len(), e.inputs[1].items.len()), (1, 4));
    let c = apply_commands(&app, &e).unwrap();
    assert_eq!(c[0].0, "timeline.redefine");
    assert_eq!(c[0].1["params"]["bends"].as_array().map(Vec::len), Some(4));
    run_all(&mut app, c);
    assert_eq!(bends(&mut app), Some(4));
    assert!(app.session.model.results.iter().all(|r| r.error.is_none()), "{:?}", app.session.model.results);
}

// ---- flat pattern navigation ----

fn flat_app(preset: &str) -> SolveApp {
    let mut app = plate();
    app.run("sheet.flange", json!({"sketch": "Base"})).unwrap();
    let flat = app.session.execute("sheet.flat_pattern", &json!({})).unwrap();
    app.start("sheet.flat_pattern");
    let Some(Kind::Sheet(Sm::FlatPattern { data, .. })) = app.dialog.as_mut().map(|d| &mut d.kind) else { panic!("flat dialog") };
    *data = Some(Ok(flat));
    app.preferences.nav = preset.into();
    app.preferences.zoom_to_cursor = true;
    app
}

/// Same view in the flat plane: the Design viewport may anchor a zoom on the model's surface
/// (a little out of the plane), the flat view anchors on its plane.
fn same_view(a: &solidcraft_engine::render::Camera, b: &solidcraft_engine::render::Camera) -> bool {
    (a.target.x - b.target.x).abs() < 1e-3
        && (a.target.y - b.target.y).abs() < 1e-3
        && (a.distance - b.distance).abs() < 1e-9
        && a.yaw == b.yaw
        && a.pitch == b.pitch
}

fn frame(app: &mut SolveApp, ctx: &egui::Context, time: &mut f64, mods: egui::Modifiers, events: Vec<egui::Event>) {
    *time += 1.0 / 60.0;
    let events = std::iter::once(egui::Event::ModifiersChanged(mods)).chain(events).collect();
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
        time: Some(*time),
        focused: true,
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| crate::viewport::show(app, ui));
    out.textures_delta.clear();
}

/// The same pan and zoom input moves the flat pattern's view exactly as it moves the Design
/// viewport's camera, under every navigation preset, with and without reverse zoom and zoom to the
/// cursor. The flat view never orbits.
#[test]
fn flat_pattern_pans_and_zooms_like_the_design_viewport() {
    use egui::{Event, Modifiers, PointerButton};
    let none = Modifiers::NONE;
    let chords = [none, Modifiers::SHIFT, Modifiers::CTRL, Modifiers::ALT, Modifiers::SHIFT | Modifiers::ALT];
    for preset in crate::prefs::NAV_PRESETS.iter().map(|p| p.id) {
        for (reverse, to_cursor) in [(false, true), (true, true), (false, false)] {
            let (flat_ctx, design_ctx) = (egui::Context::default(), egui::Context::default());
            let mut flat = flat_app(preset);
            let mut design = flat_app(preset);
            design.dialog = None;
            for app in [&mut flat, &mut design] {
                app.preferences.zoom_reverse = reverse;
                app.preferences.zoom_to_cursor = to_cursor;
            }
            let (mut ft, mut dt) = (0.0, 0.0);
            frame(&mut flat, &flat_ctx, &mut ft, none, vec![]);
            frame(&mut flat, &flat_ctx, &mut ft, none, vec![]);
            assert!(flat.flat_view.fitted);
            // The Design viewport starts from the flat view's camera: top-down, orthographic.
            design.cam = flat.flat_view.cam;
            design.ui.perspective = false;
            let design_before = flat.cam;
            let (mut panned, mut zoomed) = (0, 0);
            let at = pos2(300.0, 250.0);
            for button in [PointerButton::Primary, PointerButton::Middle, PointerButton::Secondary] {
                for mods in chords {
                    let mode = crate::prefs::nav_mode(&flat, button.into_nav(), mods);
                    if !matches!(mode, Some(crate::viewport::NavMode::Pan | crate::viewport::NavMode::Zoom)) {
                        continue;
                    }
                    let before = flat.flat_view.cam;
                    let mut both = |events: Vec<Event>, mods: Modifiers, flat: &mut SolveApp, design: &mut SolveApp| {
                        frame(flat, &flat_ctx, &mut ft, mods, events.clone());
                        frame(design, &design_ctx, &mut dt, mods, events);
                    };
                    both(vec![Event::PointerMoved(at)], mods, &mut flat, &mut design);
                    both(vec![Event::PointerButton { pos: at, button, pressed: true, modifiers: mods }], mods, &mut flat, &mut design);
                    let mut p = at;
                    for _ in 0..5 {
                        p += vec2(14.0, -9.0);
                        both(vec![Event::PointerMoved(p)], mods, &mut flat, &mut design);
                    }
                    both(vec![Event::PointerButton { pos: p, button, pressed: false, modifiers: mods }], mods, &mut flat, &mut design);
                    for _ in 0..10 {
                        both(vec![], none, &mut flat, &mut design);
                    }
                    assert!(same_view(&flat.flat_view.cam, &design.cam), "{preset}: {button:?} drag with {mods:?} ({mode:?})");
                    assert_ne!(flat.flat_view.cam, before, "{preset}: {button:?} drag with {mods:?} moves the flat view");
                    if mode == Some(crate::viewport::NavMode::Pan) {
                        panned += 1
                    } else {
                        zoomed += 1
                    }
                    flat.flat_view.cam = before;
                    design.cam = before;
                }
            }
            assert!(panned + zoomed >= 1, "{preset}: the preset has a pan or zoom drag");
            // The wheel zooms (to the cursor when asked), identically.
            let before = flat.flat_view.cam;
            let wheel = Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, 3.0), modifiers: none, phase: egui::TouchPhase::Move };
            for (app, ctx, t) in [(&mut flat, &flat_ctx, &mut ft), (&mut design, &design_ctx, &mut dt)] {
                frame(app, ctx, t, none, vec![Event::PointerMoved(at)]);
                frame(app, ctx, t, none, vec![wheel.clone()]);
                for _ in 0..30 {
                    frame(app, ctx, t, none, vec![]);
                }
            }
            assert_ne!(flat.flat_view.cam.distance, before.distance, "{preset}: the wheel zooms the flat view");
            assert!(same_view(&flat.flat_view.cam, &design.cam), "{preset} reverse {reverse} cursor {to_cursor}: wheel zoom");
            assert_eq!(flat.flat_view.cam.distance < before.distance, !reverse, "{preset}: reverse zoom flips the wheel");
            if to_cursor {
                assert!(flat.flat_view.cam.target.dist(before.target) > 1e-6, "{preset}: zoom to the cursor moves the target");
            }
            assert_eq!(flat.flat_view.cam.pitch, before.pitch);
            assert_eq!(flat.flat_view.cam.yaw, before.yaw);
            assert_eq!(flat.cam, design_before, "{preset}: the Design camera is untouched");
        }
    }
}

trait IntoNav {
    fn into_nav(self) -> crate::prefs::NavButton;
}
impl IntoNav for egui::PointerButton {
    fn into_nav(self) -> crate::prefs::NavButton {
        match self {
            egui::PointerButton::Middle => crate::prefs::NavButton::Middle,
            egui::PointerButton::Secondary => crate::prefs::NavButton::Right,
            _ => crate::prefs::NavButton::Left,
        }
    }
}

/// Input over the flat pattern, including orbit bindings, wheel and pinch, leaves the Design
/// camera and selection alone.
#[test]
fn design_camera_is_unchanged_while_the_flat_pattern_has_focus() {
    use egui::{Event, Modifiers, PointerButton};
    let none = Modifiers::NONE;
    for preset in crate::prefs::NAV_PRESETS.iter().map(|p| p.id) {
        let ctx = egui::Context::default();
        let mut app = flat_app(preset);
        let mut t = 0.0;
        frame(&mut app, &ctx, &mut t, none, vec![]);
        let cam = app.cam;
        let at = pos2(300.0, 250.0);
        for button in [PointerButton::Primary, PointerButton::Middle, PointerButton::Secondary] {
            for mods in [none, Modifiers::SHIFT, Modifiers::CTRL, Modifiers::ALT, Modifiers::SHIFT | Modifiers::ALT] {
                frame(&mut app, &ctx, &mut t, mods, vec![Event::PointerMoved(at)]);
                frame(&mut app, &ctx, &mut t, mods, vec![Event::PointerButton { pos: at, button, pressed: true, modifiers: mods }]);
                for i in 1..6 {
                    frame(&mut app, &ctx, &mut t, mods, vec![Event::PointerMoved(at + vec2(14.0, -9.0) * i as f32)]);
                }
                frame(&mut app, &ctx, &mut t, mods, vec![Event::PointerButton { pos: at, button, pressed: false, modifiers: mods }]);
            }
        }
        let wheel = Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, 3.0), modifiers: none, phase: egui::TouchPhase::Move };
        frame(&mut app, &ctx, &mut t, none, vec![wheel]);
        frame(&mut app, &ctx, &mut t, none, vec![Event::Zoom(1.3)]);
        for _ in 0..30 {
            frame(&mut app, &ctx, &mut t, none, vec![]);
        }
        assert_eq!(app.cam, cam, "{preset}: the Design camera stays put under the flat pattern");
        assert!(app.session.selection.is_empty(), "{preset}: nothing underneath is selected");
        assert_eq!(app.flat_view.cam.pitch, app.flat_view.cam.pitch.clamp(1.5, 2.0), "{preset}: still looking straight down");
    }
}

/// Fit and Home frame the flat pattern (not the model), and leave the Design camera alone.
#[test]
fn fit_and_home_frame_the_flat_pattern() {
    let ctx = egui::Context::default();
    let mut app = flat_app("fusion");
    let mut t = 0.0;
    let none = egui::Modifiers::NONE;
    frame(&mut app, &ctx, &mut t, none, vec![]);
    let fitted = app.flat_view.cam;
    // The 100 x 60 pattern sits inside the view with room for the command panel.
    let scale = 600.0 / (2.0 * fitted.half_height());
    assert!(100.0 * scale < 800.0 && 60.0 * scale < 600.0 && 100.0 * scale > 200.0, "fits: {scale}");
    let design = app.cam;
    for view in ["fit", "home"] {
        app.flat_view.cam.pan(120.0, -80.0, 600.0);
        app.flat_view.cam.zoom_at(0.3, None);
        assert_ne!(app.flat_view.cam, fitted);
        app.animate_view(view);
        frame(&mut app, &ctx, &mut t, none, vec![]);
        assert!(app.flat_view.cam.target.dist(fitted.target) < 1e-9 && (app.flat_view.cam.distance - fitted.distance).abs() < 1e-9, "{view} refits");
        assert_eq!(app.cam, design, "{view} leaves the Design camera alone");
    }
    app.flat_view.cam.pan(50.0, 50.0, 600.0);
    app.fit_view();
    frame(&mut app, &ctx, &mut t, none, vec![]);
    assert!(app.flat_view.cam.target.dist(fitted.target) < 1e-9, "fit_view refits");
    // Closing the dialog returns the viewport to the Design camera, and reopening refits.
    app.flat_view.cam.pan(50.0, 50.0, 600.0);
    app.dialog = None;
    frame(&mut app, &ctx, &mut t, none, vec![]);
    assert!(!app.flat_view.fitted);
}
