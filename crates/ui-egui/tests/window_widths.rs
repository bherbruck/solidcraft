//! Every command dialog and every window stays within the width limits of `frame`: started
//! through the control channel (the toolbar's way) and measured as drawn.

use serde_json::json;
use solidcraft_ui_egui::frame::{DIALOG_MAX_W, DIALOG_MIN_W, DIALOG_WIDE_MAX_W, WINDOW_WIDE_MAX_W};
use solidcraft_ui_egui::scenario::Harness;

/// Widths of the windows on screen (their areas in the middle layer).
fn window_widths(h: &Harness) -> Vec<(String, f32)> {
    h.ctx.memory(|m| {
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter(|l| l.order == egui::Order::Middle)
            .filter_map(|l| m.area_rect(l.id).map(|r| (format!("{:?}", l.id), r.width())))
            .collect()
    })
}

fn fresh() -> Harness {
    let mut h = Harness::new();
    h.call("engine.execute", json!({"command": "solid.box", "params": {"length": 40, "width": 30, "height": 20}}));
    h.frames(3);
    h
}

#[test]
fn command_dialogs_shrink_to_their_content_within_limits() {
    let mut h = fresh();
    let mut bad = Vec::new();
    // The frame border and shadow add a little to the content limits.
    let slack = 4.0;
    for spec in solidcraft_engine::command_specs() {
        h.call("ui.start", json!({"command": spec.id}));
        h.frames(4);
        let wide = h.app.dialog.as_ref().is_some_and(|d| d.is_wide());
        let max = if wide { DIALOG_WIDE_MAX_W } else { DIALOG_MAX_W };
        for (layer, w) in window_widths(&h) {
            // Floating windows a command opened (Parameters…) have their own limit.
            let limit = if h.app.dialog.is_some() { max } else { WINDOW_WIDE_MAX_W };
            if w > limit + slack {
                bad.push(format!("{} ({layer}): {w} px", spec.id));
            }
        }
        // The docked dialog shrinks to its content: most dialogs stay well under the maximum.
        if h.app.dialog.is_some() && !wide {
            for (_, w) in window_widths(&h) {
                if w < DIALOG_MIN_W - slack {
                    bad.push(format!("{}: {w} px is narrower than {DIALOG_MIN_W}", spec.id));
                }
            }
        }
        h.app.dialog = None;
        h.app.params = None;
        h.app.design_properties = None;
        h.app.bom = None;
        h.app.tool = None;
        if h.app.session.active_sketch.is_some() {
            h.app.finish_sketch();
        }
        h.frames(2);
    }
    assert!(bad.is_empty(), "windows too wide or too narrow:\n{}", bad.join("\n"));
}

#[test]
fn common_dialogs_are_not_stretched() {
    // Simple dialogs have room to spare: they must not be pushed out to the maximum.
    let mut h = fresh();
    for id in ["solid.extrude", "solid.fillet", "solid.shell", "solid.move", "solid.hole"] {
        h.call("ui.start", json!({"command": id}));
        h.frames(6);
        let w: f32 = window_widths(&h).iter().map(|x| x.1).fold(0.0, f32::max);
        assert!(w < DIALOG_MAX_W - 20.0, "{id} is stretched to {w} px");
        h.app.dialog = None;
        h.frames(2);
    }
}

#[test]
fn floating_windows_stay_within_limits() {
    let mut h = fresh();
    h.app.prefs_window.open = true;
    h.app.keymap.open = true;
    h.app.help.about = true;
    h.app.ui.palette_open = true;
    solidcraft_ui_egui::params_dialog::open(&mut h.app);
    solidcraft_ui_egui::design_properties::open(&mut h.app);
    h.frames(6);
    let ws = window_widths(&h);
    // Keyboard Shortcuts is a page of Preferences (one window, #36).
    assert!(ws.len() >= 3, "windows on screen: {ws:?}");
    for (layer, w) in ws {
        assert!(w <= WINDOW_WIDE_MAX_W + 4.0, "{layer} is {w} px wide");
    }
}
