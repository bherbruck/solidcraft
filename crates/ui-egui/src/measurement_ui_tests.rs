//! World-axis component dimensions use the same endpoints as the displayed measurement.
use crate::dialogs::Kind;
use crate::scenario::Harness;
use serde_json::{Value, json};
use solidcraft_engine::Sel;

fn result(h: &Harness) -> &Value {
    match &h.app.dialog.as_ref().unwrap().kind {
        Kind::Measure { result: Some(r), .. } => r,
        _ => panic!("expected a measurement result"),
    }
}

fn vertex_measurement(a: [f64; 3], b: [f64; 3]) -> Harness {
    let mut h = Harness::new();
    h.app.home.open = false;
    h.app.set_origin_shown(false);
    h.app.ui.show_browser = false;
    h.app.run("solid.box", json!({"length":3,"width":4,"height":12,"corner":[-3,0,-12],"body_name":"Box"})).unwrap();
    h.app.session.selection = [a, b].into_iter().map(|p| Sel::Vertex { body: "Box".into(), point: p.into() }).collect();
    h.app.start("inspect.measure");
    h.call("ui.view", json!({"view":"fit"}));
    h
}

/// Inspect the actual painted text, including the viewport labels (which are not widgets).
fn painted_text(h: &mut Harness) -> Vec<String> {
    // Continue the harness clock (a hair on) so egui's input history never sees time go backwards.
    let time = h.ctx.input(|i| i.time) + 1e-4;
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
        time: Some(time),
        ..Default::default()
    };
    let mut output = h.ctx.run_ui(raw, |ui| h.app.ui(ui));
    output.textures_delta.clear();
    output.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.text().to_string()) } else { None }).collect()
}

fn component_rects() -> Vec<egui::Rect> {
    ["measure:label", "measure:label:x", "measure:label:y", "measure:label:z"].into_iter().map(|key| crate::scenario::rect_of(key).unwrap()).collect()
}

#[test]
fn measurement_axis_distances_follow_the_upstream_centre_min_and_max_choices() {
    let mut h = Harness::new();
    h.app.home.open = false;
    h.app.set_origin_shown(false);
    h.app.ui.show_browser = false;
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    let mut items = Vec::new();
    for (center, radius) in [([0, 0], 3), ([30, 40], 5)] {
        let made = h.app.run("sketch.circle.center", json!({"center":center,"radius":radius})).unwrap();
        items.push(Sel::SketchCurve { id: made["curves"][0].as_str().unwrap().into() });
    }
    h.app.session.selection = items;
    h.app.start("inspect.measure");
    h.call("ui.view", json!({"view":"fit"}));
    let measured = result(&h).clone();
    assert!((measured["center_distance"].as_f64().unwrap() - 50.).abs() < 1e-8);
    assert!(measured["distance_mm"].as_f64().unwrap() < 50. && measured["max"].as_f64().unwrap() > 50.);

    for (label, distance, from, to) in [
        ("Centre to centre", "center_distance", "center_from", "center_to"),
        ("Min", "distance_mm", "from", "to"),
        ("Max", "max", "max_from", "max_to"),
    ] {
        let [x, y] = h.widget(label).unwrap_or_else(|| panic!("missing upstream distance choice: {label}"));
        h.call("ui.click", json!({"x":x,"y":y}));
        let texts = painted_text(&mut h);
        for (axis, index) in [("X", 0), ("Y", 1), ("Z", 2)] {
            let delta = measured[to][index].as_f64().unwrap() - measured[from][index].as_f64().unwrap();
            let expected = format!("{axis}: {}", crate::prefs::show_mm(&h.app, delta.abs(), 1));
            assert!(texts.contains(&expected), "{label} must use its own endpoints: {texts:?}");
        }
        assert!(texts.contains(&crate::prefs::show_mm(&h.app, measured[distance].as_f64().unwrap(), 1)));
        assert_eq!(*result(&h), measured, "changing the displayed distance must retain upstream measurement results");
    }
}

#[test]
fn measurement_axis_distances_show_absolute_world_components_in_every_view() {
    for reverse in [false, true] {
        let (a, b) = ([0., 0., 0.], [-3., 4., -12.]);
        let mut h = if reverse { vertex_measurement(b, a) } else { vertex_measurement(a, b) };
        assert!((result(&h)["distance_mm"].as_f64().unwrap() - 13.).abs() < 1e-8);
        for view in ["iso", "top", "front", "right"] {
            h.call("ui.view", json!({"view":view}));
            let texts = painted_text(&mut h);
            for text in ["13.00 mm", "X: 3.00 mm", "Y: 4.00 mm", "Z: 12.00 mm"] {
                assert!(texts.iter().any(|t| t == text), "{view}, reverse={reverse}: {texts:?}");
            }
            let rects = component_rects();
            for (i, a) in rects.iter().enumerate() {
                assert!(rects.iter().skip(i + 1).all(|b| !a.intersects(*b)), "labels overlap in {view}: {rects:?}");
            }
        }
        h.call("ui.set", json!({"perspective":true}));
        h.call("ui.prefs", json!({"up_axis":"y"}));
        let texts = painted_text(&mut h);
        assert!(texts.contains(&"Z: 12.00 mm".to_string()), "view orientation does not change world-axis distances");
        h.call("ui.key", json!({"key":"Escape"}));
        assert!(crate::scenario::handles("measure:label").is_empty(), "all temporary dimensions disappear on close");
    }
}

#[test]
fn measurement_axis_distances_keep_zero_components_readable_and_follow_units() {
    let mut h = vertex_measurement([0., 0., 0.], [-3., 0., 0.]);
    h.call("ui.view", json!({"view":"top"}));
    let texts = painted_text(&mut h);
    assert!(texts.contains(&"Y: 0.00 mm".into()) && texts.contains(&"Z: 0.00 mm".into()));
    let rects = component_rects();
    for (i, a) in rects.iter().enumerate() {
        assert!(rects.iter().skip(i + 1).all(|b| !a.intersects(*b)), "zero-component labels must remain readable: {rects:?}");
    }
    h.app.run("document.units", json!({"units":"in"})).unwrap();
    h.call("ui.prefs", json!({"length_decimals":3}));
    let texts = painted_text(&mut h);
    assert!(texts.contains(&"X: 0.1181 in".into()), "inch dimensions retain a meaningful fourth digit: {texts:?}");
    assert!(texts.contains(&"Y: 0.000 in".into()) && texts.contains(&"Z: 0.000 in".into()));
}

#[test]
fn measurement_axis_distances_can_be_disabled_saved_and_restored_in_preferences() {
    assert!(serde_json::from_value::<crate::prefs::Prefs>(json!({})).unwrap().measure_axis_distances, "existing settings get the default-on option");
    let mut h = vertex_measurement([0., 0., 0.], [-3., 4., -12.]);
    let measurement = result(&h).clone();
    h.call("ui.prefs", json!({"open":"Graphics"}));
    let checkbox = crate::scenario::rect_of("prefs:measure_axis_distances").unwrap().center();
    h.call("ui.click", json!({"x":checkbox.x,"y":checkbox.y}));
    assert!(!h.app.preferences.measure_axis_distances);
    assert!(crate::scenario::handle_at("measure:label").is_some(), "total stays visible");
    assert!(crate::scenario::handle_at("measure:label:x").is_none());
    assert_eq!(*result(&h), measurement, "hiding the extra overlay does not change the measurement");
    h.call("ui.prefs", json!({"button":"ok"}));
    let texts = painted_text(&mut h);
    assert!(!texts.iter().any(|text| text.starts_with("X: ") || text.starts_with("Y: ") || text.starts_with("Z: ")));
    let mut restored = crate::SolveApp::new(Default::default(), Default::default());
    restored.load_prefs(&h.app.prefs());
    assert!(!restored.preferences.measure_axis_distances, "disabled preference survives restart");
    h.call("ui.prefs", json!({"open":"Graphics","measure_axis_distances":true}));
    assert!(crate::scenario::handle_at("measure:label:x").is_some());
    h.call("ui.prefs", json!({"button":"cancel"}));
    assert!(!h.app.preferences.measure_axis_distances, "Cancel restores the saved choice");
    h.call("ui.prefs", json!({"open":"Graphics"}));
    h.call("ui.prefs", json!({"button":"defaults"}));
    assert!(h.app.preferences.measure_axis_distances);
    h.call("ui.prefs", json!({"button":"apply"}));
    h.call("ui.prefs", json!({"measure_axis_distances":false}));
    h.call("ui.prefs", json!({"button":"cancel"}));
    assert!(h.app.preferences.measure_axis_distances, "Cancel restores the last Apply");
}
