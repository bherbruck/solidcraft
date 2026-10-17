//! A failed feature is marked in the bottom timeline and the Feature History by a shape, not only
//! its colour (#972): the error mark where a warning's mark goes (now an icon image, so these
//! tests look for the 10-point image at the mark's place).

use egui::{Pos2, Rect, Shape, vec2};
use serde_json::json;

use crate::SolveApp;

/// Every shape in `s`, the parts of a list of shapes too.
fn each(s: &Shape, f: &mut impl FnMut(&Shape)) {
    match s {
        Shape::Vec(v) => v.iter().for_each(|s| each(s, f)),
        s => f(s),
    }
}

/// The sample with Fillet1 failing (the plate made narrower than its fillets).
fn failing_sample() -> SolveApp {
    let mut s = solidcraft_engine::Session::default();
    s.run_script(&solidcraft_engine::sample::script()).unwrap();
    s.execute("parameters.change", &json!({"name": "width", "expression": "5 mm"})).unwrap();
    SolveApp::new(s, crate::Services::default())
}

/// Each feature's name and whether it failed.
fn features(app: &SolveApp) -> Vec<(String, bool)> {
    app.session.doc.features.iter().map(|f| (f.name.clone(), app.session.model.result(f.id).is_some_and(|r| r.error.is_some()))).collect()
}

/// The centres of the 10 to 12-point icon images one frame of `draw` paints: the failed-feature mark
/// (the sample has no warnings, so every such image is an error mark).
fn error_discs(app: &mut SolveApp, draw: fn(&mut SolveApp, &mut egui::Ui)) -> Vec<Pos2> {
    crate::scenario::clear_handles();
    let ctx = egui::Context::default();
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0));
    let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| draw(app, ui));
    // Icons upload textures; dropping their deltas unapplied would panic.
    out.textures_delta.clear();
    let mut found = Vec::new();
    for clipped in &out.shapes {
        each(&clipped.shape, &mut |s| {
            if let Shape::Mesh(m) = s {
                let b = m.calc_bounds();
                if matches!(m.texture_id, egui::TextureId::Managed(_)) && (b.width() - 11.0).abs() < 1.5 && (b.height() - 11.0).abs() < 1.5 {
                    found.push(b.center());
                }
            }
        });
    }
    found
}

/// The features that have a disc where their mark goes (`at`: the mark's centre from the item's).
fn marked(app: &mut SolveApp, draw: fn(&mut SolveApp, &mut egui::Ui), at: impl Fn(Pos2, Pos2) -> bool) -> Vec<(String, bool)> {
    let discs = error_discs(app, draw);
    let handles = crate::scenario::handles("timeline:");
    features(app)
        .into_iter()
        .map(|(name, _)| {
            let item = handles.iter().find(|(h, _)| *h == format!("timeline:{name}")).map(|(_, p)| *p);
            let item = item.unwrap_or_else(|| panic!("{name} is not drawn"));
            let has = discs.iter().any(|d| at(item, *d));
            (name, has)
        })
        .collect()
}

#[test]
fn a_failed_feature_is_marked_in_the_bottom_timeline() {
    let mut app = failing_sample();
    let want = features(&app);
    assert!(want.iter().any(|(n, failed)| *failed && n == "Fillet1"), "the sample's Fillet1 fails: {want:?}");
    assert!(want.iter().filter(|(_, failed)| !failed).count() >= 3, "and others don't");
    // The mark sits in the item's top-right corner: 10 points square, inside the 28-point item.
    let got = marked(&mut app, crate::timeline::timeline, |item, d| (d.x - (item.x + 9.0)).abs() < 1.5 && (d.y - (item.y - 9.0)).abs() < 1.5);
    assert_eq!(got, want, "an error mark on each failed feature and on no other");
}

#[test]
fn a_failed_feature_is_marked_in_the_feature_history() {
    let mut app = failing_sample();
    let want = features(&app);
    // The mark sits at the row's right end, on its centre line.
    let got = marked(&mut app, crate::timeline_left::left_timeline, |item, d| (d.y - item.y).abs() < 1.5 && d.x > item.x);
    assert_eq!(got, want, "an error mark on each failed feature and on no other");
}
