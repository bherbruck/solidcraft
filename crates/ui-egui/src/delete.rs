//! Delete (the Delete and Backspace keys, and every menu's Delete item): whatever is selected in
//! the viewport or the browser goes in one undoable step through `selection.delete`. When it
//! takes other features with it or makes some fail, a confirmation lists them first.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::SolveApp;
use crate::theme::Tokens;

/// A delete waiting for confirmation: its parameters and the dry run's report.
#[derive(Clone, Debug)]
pub struct Pending {
    pub params: Value,
    pub report: Value,
}

/// What Delete acts on now: the selection (bodies not locked), plus the components picked in
/// the browser (their occurrences).
pub fn selection_params(app: &SolveApp) -> Value {
    let items: Vec<&solidcraft_engine::Sel> = app
        .session
        .selection
        .iter()
        .filter(|s| !matches!(s, solidcraft_engine::Sel::Body { name } if app.session.visibility.locked_bodies.contains(name)))
        .collect();
    let occurrences: Vec<u64> = app.tree.picked_components.iter().filter_map(|c| app.session.doc.occurrence_of(*c).map(|o| o.id)).collect();
    json!({ "items": items, "occurrences": occurrences, "canvases": app.tree.picked_canvases })
}

fn is_empty(p: &Value) -> bool {
    ["items", "occurrences", "components", "canvases"].iter().all(|k| p.get(*k).and_then(Value::as_array).is_none_or(|a| a.is_empty()))
}

fn shared_occurrences(app: &SolveApp, params: &Value) -> bool {
    let explicit = params.get("occurrences").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64);
    let selected = params
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "occurrence")
        .filter_map(|item| item["path"].as_array()?.last()?.as_u64());
    explicit.chain(selected).any(|id| app.session.occurrence_id_scope_count(id).is_ok_and(|n| n > 1))
}

/// Delete what is selected (the Delete key). A selected sketch dimension or constraint glyph
/// goes first, as before.
pub fn delete_selection(app: &mut SolveApp) {
    if crate::sketch3d::delete_selected(app) || crate::sketch_tools::delete_glyph(app) {
        return;
    }
    request(app, selection_params(app));
}

/// Delete `params` (as `selection.delete` takes them): at once, or after a confirmation when
/// other features go with it or would fail.
pub fn request(app: &mut SolveApp, params: Value) {
    if is_empty(&params) {
        return;
    }
    let mut dry = params.clone();
    dry["dry_run"] = json!(true);
    let report = match app.session.execute("selection.delete", &dry) {
        Ok(r) => r,
        Err(e) => {
            app.set_status(e.to_string().replace("selection.delete: ", ""), true);
            return;
        }
    };
    let asked =
        params.get("items").and_then(Value::as_array).map_or(0, |a| a.iter().filter(|i| i["type"] == "feature" || i["type"] == "plane").count());
    let extra = report["deleted"].as_array().map_or(0, Vec::len) > asked;
    let fails = report["would_fail"].as_array().is_some_and(|a| !a.is_empty());
    let shared = shared_occurrences(app, &params);
    if extra || fails || shared {
        app.menu.confirm = Some(Pending { params, report });
    } else {
        run(app, &params);
    }
}

fn run(app: &mut SolveApp, params: &Value) {
    match app.run("selection.delete", params.clone()) {
        Ok(r) => {
            app.tree.picked_components.clear();
            app.tree.picked_canvases.clear();
            if let Some(s) = r["skipped"].as_array().and_then(|a| a.first()).and_then(Value::as_str) {
                app.set_status(format!("deleted; {s}"), false);
            }
        }
        Err(e) => app.set_status(e.replace("selection.delete: ", ""), true),
    }
}

/// Answer the confirmation (also `ui.confirm`): delete, or keep everything.
pub fn confirm(app: &mut SolveApp, accept: bool) -> bool {
    let Some(p) = app.menu.confirm.take() else { return false };
    if accept {
        run(app, &p.params);
    }
    true
}

/// The confirmation window.
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(p) = app.menu.confirm.clone() else { return };
    let t = Tokens::get();
    let list = |k: &str| -> Vec<String> {
        p.report[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
    };
    let (deleted, fails) = (list("deleted"), list("would_fail"));
    let mut answer: Option<bool> = None;
    crate::frame::window(ctx, crate::i18n::tr_label("Delete"), crate::frame::Width::Normal)
        .id(egui::Id::new("sc_delete_confirm"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -60.0))
        .show(ctx, |ui| {
            ui.set_max_width(360.0);
            if shared_occurrences(app, &p.params) {
                ui.label(crate::i18n::tr_label("This child is shared. Deleting it removes it from every copy of its parent."));
            }
            if !deleted.is_empty() {
                ui.label(crate::i18n::tr_label("These features will be deleted:"));
                for d in deleted.iter().take(20) {
                    ui.label(RichText::new(format!("  • {d}")).color(t.text));
                }
                if deleted.len() > 20 {
                    ui.label(format!("  … and {} more", deleted.len() - 20));
                }
            }
            if !fails.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(crate::i18n::tr_label("These features will fail:")).color(t.warning));
                for f in fails.iter().take(20) {
                    ui.label(RichText::new(format!("  • {f}")).color(t.warning));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(false);
            }
        });
    if let Some(a) = answer {
        confirm(app, a);
    }
}
