//! Layout checks over every toolbar command, headless (see `solidcraft_ui_egui::scenario`):
//! each command's dialog opens docked at the viewport's right within its width limits, and
//! every window it opens fits on screen.

use serde_json::{Value, json};
use solidcraft_ui_egui::scenario::{Harness, check, rect_of};

/// A box body (its sketch finished) to give feature dialogs something to work on.
fn harness() -> Harness {
    let mut h = Harness::new();
    for (id, p) in [
        ("sketch.create", json!({"plane": "XY"})),
        ("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance": 10})),
    ] {
        h.call("engine.execute", json!({"command": id, "params": p}));
    }
    h.frames(3);
    h
}

#[test]
fn every_dialog_opens_docked_within_its_width_limits() {
    let mut bad = Vec::new();
    let mut seen = 0;
    for spec in solidcraft_engine::command_specs() {
        if spec.tab.is_empty() || spec.tab == "SKETCH" {
            continue;
        }
        let mut h = harness();
        h.call("ui.start", json!({"command": spec.id}));
        h.frames(4);
        if h.app.dialog.is_some() {
            seen += 1;
            // Command dialogs are 260–380 px (plus their 1 px border), the few with tables
            // (motion study, configurations) 480–560.
            let w = rect_of("dialog:outer").map_or(0.0, |r| r.width());
            let limits = if w > 400.0 { json!({"min": 480, "max": 562}) } else { json!({"min": 260, "max": 382}) };
            if let Err(e) = check(&mut h, &json!({ "dialog_docked": limits })) {
                bad.push(format!("{}: {e}", spec.id));
            }
        }
        if let Err(e) = check(&mut h, &json!({"windows_fit": {"max_width": 900}})) {
            bad.push(format!("{}: {e}", spec.id));
        }
    }
    assert!(seen > 20, "only {seen} commands opened a dialog");
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.join("\n"));
}

/// Windows that are not command dialogs: each fits on screen.
#[test]
fn other_windows_fit_on_screen() {
    let mut bad = Vec::new();
    let opens: [(&str, Value); 3] = [("ui.help", json!({"item": "about"})), ("ui.key", json!({"key": "S"})), ("ui.prefs", json!({"open": true}))];
    for (m, p) in opens {
        let mut h = harness();
        h.call(m, p.clone());
        h.frames(4);
        if let Err(e) = check(&mut h, &json!({"windows_fit": {"max_width": 900}})) {
            bad.push(format!("{m} {p}: {e}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The docked ribbon reserves the same space in both themes and at narrow widths. The
/// existing panel menus and S box remain usable through the new painted hit targets.
#[test]
fn docked_ribbon_geometry_and_search() {
    use solidcraft_ui_egui::{scenario::handle_at, toolbar};
    for theme in ["light", "dark"] {
        let mut h = harness();
        h.app.preferences.theme = theme.into();
        for width in [1600.0, 640.0, 320.0] {
            h.call("ui.resize", json!({"width":width,"height":1000}));
            h.frames(4);
            let ribbon = rect_of("toolbar").unwrap();
            let workspace = rect_of("ribbon:workspace").unwrap();
            let search = rect_of("ribbon:search").unwrap();
            assert_eq!(ribbon.top(), toolbar::APP_BAR_HEIGHT);
            assert_eq!(ribbon.height(), 117.0);
            assert_eq!(ribbon.width(), width);
            assert_eq!(workspace.width(), 88.0);
            assert_eq!(workspace.top(), ribbon.top());
            assert_eq!(workspace.bottom(), ribbon.bottom() - 1.0);
            assert_eq!(search.height(), 24.0);
            assert_eq!(search.width(), if width == 1600.0 { 188.0 } else { 24.0 });
            assert!(ribbon.contains_rect(search));
            assert!(search.left() > workspace.right());
            assert!(h.app.viewport.rect.unwrap().top() >= toolbar::APP_BAR_HEIGHT + toolbar::RIBBON_HEIGHT);
            h.step(&json!({"click":{"handle":"ribbon:search"}})).unwrap();
            assert!(h.app.ui.palette_open);
            let popup = rect_of("ribbon:search_popup").unwrap();
            assert!(popup.top() >= search.bottom());
            assert!(popup.left() <= search.left());
            h.step(&json!({"key":"Escape"})).unwrap();
            assert!(!h.app.ui.palette_open);
            h.step(&json!({"key":"S"})).unwrap();
            assert!(h.app.ui.palette_open);
            assert_eq!(rect_of("ribbon:search_popup").unwrap().min, popup.min);
            h.step(&json!({"key":"Escape"})).unwrap();
        }
        h.call("ui.resize", json!({"width":1600,"height":1000}));
        h.frames(4);
        let button = rect_of("button:solid.extrude").unwrap();
        assert_eq!(button.height(), 60.0);
        assert!(button.width() >= 54.0);
        let section = rect_of("ribbon:section:MODEL:CREATE").unwrap();
        assert_eq!(section.height(), 20.0);
        assert!(section.top() > button.bottom());
        assert_eq!(section.bottom(), rect_of("toolbar").unwrap().bottom() - 4.0);
        assert_eq!(handle_at("panel:MODEL:CREATE"), handle_at("ribbon:section:MODEL:CREATE"));
        h.step(&json!({"click":{"handle":"ribbon:section:MODEL:CREATE"}})).unwrap();
        assert!(rect_of("toolbar_dropdown").is_some());
        h.step(&json!({"key":"Escape"})).unwrap();
        h.step(&json!({"click":{"handle":"ribbon:workspace"}})).unwrap();
        assert!(h.app.viewport.context_menu.is_some());
    }
}

#[test]
fn finish_sketch_stays_at_the_right_when_groups_collapse() {
    use solidcraft_ui_egui::scenario::handle_at;
    let mut h = Harness::new();
    h.call("engine.execute", json!({"command":"sketch.create","params":{"plane":"XY"}}));
    for width in [1600.0, 640.0, 320.0] {
        h.call("ui.resize", json!({"width":width,"height":1000}));
        h.frames(5);
        let ribbon = rect_of("toolbar").unwrap();
        let exit = rect_of("ribbon:section:SKETCH:FINISH SKETCH").unwrap();
        let finish = rect_of("button:sketch.finish").unwrap();
        assert_eq!(exit.right(), ribbon.right() - 3.0);
        assert!(ribbon.contains_rect(finish));
        assert!(finish.bottom() < exit.top());
        assert!(handle_at("tab:SKETCH").is_some(), "the selected contextual tab scrolls into view");
    }
    h.step(&json!({"click":{"handle":"button:sketch.finish"}})).unwrap();
    assert!(h.app.session.active_sketch.is_none());
}
