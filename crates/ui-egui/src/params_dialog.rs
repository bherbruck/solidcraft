//! The Change Parameters dialog and parameter-name autocomplete for expression boxes.
//!
//! A window over the engine's `parameters.*` commands: favourites first, then user parameters
//! (name, unit, expression, value, comment; add, rename, delete) and model parameters grouped by
//! the sketch or feature they drive (editable). Expressions validate as you type
//! (`expr.evaluate`) with a live model preview; Enter or leaving the field applies them.
//! Import and export go through `parameters.import` / `parameters.export`.
//!
//! [`complete`] adds suggestions (parameters, functions, units) under any expression box: the
//! names come from `parameters.complete`, cached per document revision by [`publish`].

use std::sync::Arc;

use egui::RichText;
use serde_json::{Value, json};

use crate::SolveApp;
use crate::theme::Tokens;

const UNITS: [&str; 9] = ["mm", "cm", "m", "in", "ft", "deg", "rad", "", "um"];
/// The expression a new parameter row starts with.
const NEW_EXPR: &str = "10 mm";

/// Dialog state (what is being typed; the parameters themselves live in the document).
#[derive(Default)]
pub struct ParamsDialog {
    pub filter: String,
    /// Expressions being edited, by parameter name.
    pub editing: std::collections::BTreeMap<String, String>,
    /// Comments being edited, by parameter name.
    pub comments: std::collections::BTreeMap<String, String>,
    /// A parameter being renamed: (old name, new name).
    pub renaming: Option<(String, String)>,
    pub new_name: String,
    pub new_unit: usize,
    pub new_expr: String,
    pub new_comment: String,
    /// The last error from an action in the dialog.
    pub error: Option<String>,
    /// The expression edit to preview: (name, expression).
    pub preview: Option<(String, String)>,
}

impl ParamsDialog {
    pub fn new() -> Self {
        ParamsDialog { new_expr: NEW_EXPR.into(), ..Default::default() }
    }
    /// The command the live preview should run (a valid expression being edited).
    pub fn preview_commands(&self) -> Option<Vec<(String, Value)>> {
        let (n, e) = self.preview.as_ref()?;
        Some(vec![("parameters.change".to_string(), json!({"name": n, "expression": e}))])
    }
}

/// One row as the engine lists it.
#[derive(Clone, Debug, Default)]
struct Row {
    name: String,
    expression: String,
    unit: String,
    value: Option<f64>,
    error: Option<String>,
    comment: String,
    source: String,
    group: String,
    favorite: bool,
}

fn rows(app: &mut SolveApp) -> Vec<Row> {
    let list = app.session.execute("parameters.list", &json!({})).unwrap_or_default();
    // Sketch dimension parameters: the sketch that owns each.
    let mut owner: std::collections::BTreeMap<String, String> = Default::default();
    for f in &app.session.doc.features {
        if let solidcraft_engine::doc::FeatureKind::Sketch { sketch, .. } = &f.kind {
            for c in &sketch.constraints {
                if let Some(p) = &c.param {
                    owner.insert(p.clone(), f.name.clone());
                }
            }
        }
    }
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    list["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let name = s(r, "name");
            let source = s(r, "source");
            let group = match source.as_str() {
                "feature" => s(r, "feature"),
                "sketch" => owner.get(&name).cloned().unwrap_or_else(|| "Sketch".into()),
                _ => String::new(),
            };
            Row {
                expression: s(r, "expression"),
                unit: s(r, "unit"),
                value: r.get("value").and_then(Value::as_f64),
                error: r.get("error").and_then(Value::as_str).map(str::to_string),
                comment: s(r, "comment"),
                favorite: r.get("favorite").and_then(Value::as_bool).unwrap_or(false),
                source,
                group,
                name,
            }
        })
        .collect()
}

fn show_value(r: &Row) -> String {
    match r.value {
        Some(v) if r.unit.is_empty() => format!("{v:.4}"),
        Some(v) => format!("{v:.4} {}", r.unit),
        None => "—".into(),
    }
}

/// Open the dialog (Change Parameters on the toolbar).
pub fn open(app: &mut SolveApp) {
    app.params = Some(ParamsDialog::new());
}

/// Draw the dialog when it is open. Called once per frame.
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    // Completions are offered only under a focused text box; gathering them evaluates every
    // parameter, so a design changing command after command (an agent building over MCP)
    // does not pay for it on every change while nobody is typing.
    if app.params.is_some() || ctx.memory(|m| m.focused().is_some()) {
        publish(app, ctx);
    }
    let Some(mut d) = app.params.take() else { return };
    let t = Tokens::get();
    let mut open = true;
    let all = rows(app);
    crate::frame::window(ctx, RichText::new(crate::i18n::tr_label("PARAMETERS")).strong().size(13.0), crate::frame::Width::Wide)
        .id(egui::Id::new("sc_params_dialog"))
        .open(&mut open)
        .default_width(740.0)
        .default_height(520.0)
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.filter).hint_text(crate::i18n::tr_label("Filter")).desired_width(180.0));
                ui.separator();
                if ui.button("Import…").on_hover_text(crate::i18n::tr_label("CSV or JSON: updates existing parameters, adds new ones")).clicked() {
                    app.pick_open_command("parameters.import", json!({}), &[]);
                }
                for (label, ext) in [("Export CSV…", "csv"), ("Export JSON…", "json")] {
                    if ui.button(label).clicked() {
                        app.pick_save_command(&format!("parameters.{ext}"), &[ext], "parameters.export", json!({"format": ext}));
                    }
                }
            });
            if let Some(e) = &d.error {
                ui.label(RichText::new(e).color(t.error));
            }
            ui.separator();
            let f = d.filter.to_ascii_lowercase();
            let visible: Vec<&Row> = all
                .iter()
                .filter(|r| {
                    f.is_empty()
                        || r.name.to_ascii_lowercase().contains(&f)
                        || r.comment.to_ascii_lowercase().contains(&f)
                        || r.group.to_ascii_lowercase().contains(&f)
                })
                .collect();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let favs: Vec<&Row> = visible.iter().copied().filter(|r| r.favorite).collect();
                if !favs.is_empty() {
                    section(ui, &t, "Favorites");
                    table(app, ui, &mut d, &favs, "fav");
                }
                section(ui, &t, "User Parameters");
                let users: Vec<&Row> = visible.iter().copied().filter(|r| r.source == "user").collect();
                table(app, ui, &mut d, &users, "user");
                add_row(app, ui, &mut d);
                section(ui, &t, "Model Parameters");
                let mut groups: Vec<String> = Vec::new();
                for r in visible.iter().filter(|r| r.source != "user") {
                    if !groups.contains(&r.group) {
                        groups.push(r.group.clone());
                    }
                }
                for g in groups {
                    let members: Vec<&Row> = visible.iter().copied().filter(|r| r.source != "user" && r.group == g).collect();
                    egui::CollapsingHeader::new(RichText::new(&g).strong()).id_salt(("pgroup", &g)).default_open(true).show(ui, |ui| {
                        table(app, ui, &mut d, &members, &g);
                    });
                }
            });
        });
    // The title bar's close button, for scenarios and demos.
    if let Some(r) = ctx.memory(|m| m.area_rect(egui::Id::new("sc_params_dialog"))) {
        crate::scenario::publish_handle("params:close", egui::pos2(r.right() - 16.0, r.top() + 15.0));
        crate::scenario::publish_handle("params:title", egui::pos2(r.center().x, r.top() + 15.0));
    }
    if !open {
        app.params = None;
        return;
    }
    app.params = Some(d);
}

fn section(ui: &mut egui::Ui, t: &Tokens, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().color(t.text));
}

/// Live check of an expression typed for a parameter of this unit: Ok(shown value) or the message.
fn check(app: &mut SolveApp, e: &str, unit: &str) -> Result<String, String> {
    let r = app.session.execute("expr.evaluate", &json!({"expression": e, "unit": unit})).map_err(|e| e.to_string())?;
    if r["ok"].as_bool().unwrap_or(false) {
        let v = r["display_value"].as_f64().unwrap_or(0.0);
        Ok(if unit.is_empty() { format!("{v:.4}") } else { format!("{v:.4} {unit}") })
    } else {
        Err(r["error"].as_str().unwrap_or("invalid").to_string())
    }
}

fn table(app: &mut SolveApp, ui: &mut egui::Ui, d: &mut ParamsDialog, rows: &[&Row], salt: &str) {
    let t = Tokens::get();
    egui::Grid::new(("ptable", salt)).num_columns(7).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
        for h in ["", "Name", "Unit", "Expression", "Value", "Comment", ""] {
            ui.label(RichText::new(h).color(t.text_dim).small());
        }
        ui.end_row();
        for (i, r) in rows.iter().enumerate() {
            ui.horizontal(|ui| {
                // Favourite star.
                let star = if r.favorite { RichText::new("★").color(t.accent) } else { RichText::new("☆").color(t.text_dim) };
                if ui.add(egui::Button::new(star).frame(false)).on_hover_text(crate::i18n::tr_label("Favourite")).clicked() {
                    d.error = app.run("parameters.favorite", json!({"name": r.name, "favorite": !r.favorite})).err();
                }
                // Up and down past the neighbouring user parameter (#1187), in the user table only:
                // the favourites table lists favourites first whatever their order.
                if salt == "user" {
                    for (glyph, tip, neighbour, side) in
                        [("▲", "Move up", i.checked_sub(1).and_then(|j| rows.get(j)), "before"), ("▼", "Move down", rows.get(i + 1), "after")]
                    {
                        let button = egui::Button::new(RichText::new(glyph).small().color(t.text_dim)).frame_when_inactive(false);
                        let resp = ui.add_enabled(neighbour.is_some(), button).on_hover_text(crate::i18n::tr_label(tip));
                        crate::scenario::publish_handle(&format!("param:{}:{}", side, r.name), resp.rect.center());
                        if resp.clicked()
                            && let Some(n) = neighbour
                        {
                            d.error = app.run("parameters.move", json!({"name": r.name, side: n.name})).err();
                        }
                    }
                }
            });
            // Name: double-click to rename.
            match &mut d.renaming {
                Some((old, new)) if *old == r.name => {
                    let resp = ui.add_sized([110.0, 20.0], egui::TextEdit::singleline(new));
                    if resp.lost_focus() {
                        let (old, new) = (old.clone(), new.trim().to_string());
                        if !new.is_empty() && new != old {
                            d.error = app.run("parameters.rename", json!({"name": old, "new_name": new})).err();
                        }
                        d.renaming = None;
                    } else if !resp.has_focus() {
                        // Only take focus once: requesting it every frame resets the text box's
                        // focus lock, so arrow keys would move focus away and end the rename.
                        resp.request_focus();
                    }
                }
                _ => {
                    let name = if r.source == "user" { RichText::new(&r.name) } else { RichText::new(&r.name).color(t.text_dim) };
                    let resp =
                        ui.add(egui::Label::new(name).sense(egui::Sense::click())).on_hover_text(crate::i18n::tr_label("Double-click to rename"));
                    crate::scenario::publish_handle(&format!("param:name:{}", r.name), resp.rect.center());
                    if resp.double_clicked() {
                        d.renaming = Some((r.name.clone(), r.name.clone()));
                    }
                }
            }
            // Unit: user parameters choose it; others follow their input.
            if r.source == "user" {
                let mut sel = UNITS.iter().position(|u| *u == r.unit).unwrap_or(0);
                let before = sel;
                egui::ComboBox::from_id_salt(("punit", salt, &r.name)).width(56.0).selected_text(unit_label(&r.unit)).show_ui(ui, |ui| {
                    for (i, u) in UNITS.iter().enumerate() {
                        ui.selectable_value(&mut sel, i, unit_label(u));
                    }
                });
                if sel != before
                    && let Some(u) = UNITS.get(sel)
                {
                    d.error = app.run("parameters.change", json!({"name": r.name, "expression": r.expression, "unit": u})).err();
                }
            } else {
                ui.label(RichText::new(unit_label(&r.unit)).color(t.text_dim));
            }
            // Expression: live check, applied on Enter or leaving the field.
            let mut e = d.editing.get(&r.name).cloned().unwrap_or_else(|| r.expression.clone());
            let live = if e != r.expression { Some(check(app, &e, &r.unit)) } else { None };
            let bad = matches!(live, Some(Err(_))) || (live.is_none() && r.error.is_some());
            let mut te = egui::TextEdit::singleline(&mut e).desired_width(170.0);
            if bad {
                te = te.text_color(t.error);
            }
            // Per table: a favourite is listed twice, and one id would give both boxes the focus and
            // each key to both (#1767).
            let id = ui.make_persistent_id(("pexpr", salt, &r.name));
            let focused = ui.memory(|m| m.has_focus(id));
            let resp = ui.add_sized([170.0, 20.0], te.id(id));
            crate::scenario::publish_handle(&format!("param:{}", r.name), resp.rect.center());
            if salt == "fav" {
                crate::scenario::publish_handle(&format!("param:favorite:{}", r.name), resp.rect.center());
            }
            let completed = complete(ui, &resp, &mut e);
            if resp.changed() || completed {
                d.editing.insert(r.name.clone(), e.clone());
            }
            if resp.has_focus() && matches!(live, Some(Ok(_))) {
                d.preview = Some((r.name.clone(), e.clone()));
            }
            if !completed && (resp.lost_focus() || enter_for(ui, focused)) {
                d.preview = None;
                if e != r.expression {
                    let res = check(app, &e, &r.unit).and_then(|_| app.run("parameters.change", json!({"name": r.name, "expression": e})));
                    d.error = res.err();
                    if d.error.is_none() {
                        d.editing.remove(&r.name);
                    }
                } else {
                    d.editing.remove(&r.name);
                }
            }
            // Value, or why it has none.
            match (&live, &r.error) {
                (Some(Err(m)), _) => ui.label(RichText::new(m).color(t.error)).on_hover_text(m),
                (Some(Ok(v)), _) => ui.label(RichText::new(v).color(t.accent)),
                (None, Some(m)) => ui.label(RichText::new(m).color(t.error)).on_hover_text(m),
                (None, None) => ui.label(RichText::new(show_value(r)).color(t.text_dim)),
            };
            // Comment.
            let mut c = d.comments.get(&r.name).cloned().unwrap_or_else(|| r.comment.clone());
            let cr = ui.add_sized([150.0, 20.0], egui::TextEdit::singleline(&mut c).hint_text("comment"));
            if cr.changed() {
                d.comments.insert(r.name.clone(), c.clone());
            }
            if cr.lost_focus() {
                if c != r.comment {
                    d.error = app.run("parameters.comment", json!({"name": r.name, "comment": c})).err();
                }
                d.comments.remove(&r.name);
            }
            // Delete (user parameters; refused while used, the message says by what).
            if r.source == "user" {
                // Outlined on hover like the expression and comment fields; the label says what it does.
                if ui.add(egui::Button::new(RichText::new("Delete").small().color(t.text_dim)).frame_when_inactive(false)).clicked() {
                    d.error = app.run("parameters.delete", json!({"name": r.name})).err();
                }
            } else {
                ui.label("");
            }
            ui.end_row();
        }
    });
}

fn unit_label(u: &str) -> &str {
    if u.is_empty() { "none" } else { u }
}

/// Only a fresh Enter in this field submits; held keys and unrelated focus do not.
fn enter_for(ui: &mut egui::Ui, focused: bool) -> bool {
    focused
        && ui.input_mut(|i| {
            let fresh = i.raw.events.iter().any(|event| {
                matches!(event,
            egui::Event::Key { key: egui::Key::Enter, pressed: true, repeat: false, modifiers, .. } if *modifiers == egui::Modifiers::NONE)
            });
            if fresh {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            }
            fresh
        })
}

fn add_row(app: &mut SolveApp, ui: &mut egui::Ui, d: &mut ParamsDialog) {
    let t = Tokens::get();
    ui.horizontal(|ui| {
        let name_id = ui.make_persistent_id("pnew_name");
        let expr_id = ui.make_persistent_id("pnew_expr");
        let comment_id = ui.make_persistent_id("pnew_comment");
        let focused_id = ui.memory(|m| [name_id, expr_id, comment_id].into_iter().find(|id| m.has_focus(*id)));
        let name =
            ui.add(egui::TextEdit::singleline(&mut d.new_name).id(name_id).hint_text(crate::i18n::tr_label("new parameter")).desired_width(120.0));
        crate::scenario::publish_handle("param:new:name", name.rect.center());
        let unit_response = egui::ComboBox::from_id_salt("pnew_unit")
            .width(56.0)
            .selected_text(unit_label(UNITS.get(d.new_unit).copied().unwrap_or("mm")))
            .show_ui(ui, |ui| {
                for (i, u) in UNITS.iter().enumerate() {
                    ui.selectable_value(&mut d.new_unit, i, unit_label(u));
                }
            });
        crate::scenario::publish_handle("param:new:unit", unit_response.response.rect.center());
        let unit = UNITS.get(d.new_unit).copied().unwrap_or("mm");
        let bad = check(app, &d.new_expr, unit).is_err();
        let mut te = egui::TextEdit::singleline(&mut d.new_expr).id(expr_id).desired_width(170.0);
        if bad {
            te = te.text_color(t.error);
        }
        let resp = ui.add(te);
        crate::scenario::publish_handle("param:new:expression", resp.rect.center());
        let completed = complete(ui, &resp, &mut d.new_expr);
        let live = check(app, &d.new_expr, unit);
        match &live {
            Ok(v) => ui.label(RichText::new(v).color(t.text_dim)),
            Err(m) => ui.label(RichText::new(m).color(t.error)),
        };
        let comment = ui.add(egui::TextEdit::singleline(&mut d.new_comment).id(comment_id).hint_text("comment").desired_width(120.0));
        crate::scenario::publish_handle("param:new:comment", comment.rect.center());
        let submit = !completed && enter_for(ui, focused_id.is_some());
        let ok = !d.new_name.trim().is_empty() && live.is_ok();
        if ui.add_enabled(ok, egui::Button::new(crate::i18n::tr_label("Add"))).clicked() || submit {
            if let Err(error) = &live {
                d.error = Some(error.clone());
                if let Some(id) = focused_id {
                    ui.memory_mut(|m| m.request_focus(id));
                }
                return;
            }
            let p = json!({"name": d.new_name.trim(), "expression": d.new_expr, "unit": unit, "comment": d.new_comment});
            match app.run("parameters.add", p) {
                Ok(_) => {
                    d.new_name.clear();
                    d.new_expr = NEW_EXPR.into();
                    d.new_comment.clear();
                    d.error = None;
                    name.request_focus();
                }
                Err(e) => {
                    d.error = Some(e);
                    if let Some(id) = focused_id {
                        ui.memory_mut(|m| m.request_focus(id));
                    }
                }
            }
        }
    });
}

/// A completion offered under an expression box.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub text: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Clone, Default)]
struct Cache {
    revision: u64,
    all: Arc<Vec<Suggestion>>,
}

fn cache_id() -> egui::Id {
    egui::Id::new("sc_param_completions")
}

/// Keep the completion list (parameters, functions, constants, units) for this revision of the
/// design where expression boxes can reach it. Called once per frame.
pub fn publish(app: &mut SolveApp, ctx: &egui::Context) {
    let rev = app.session.revision;
    if ctx.data(|d| d.get_temp::<Cache>(cache_id())).is_some_and(|c| c.revision == rev) {
        return;
    }
    let mut all: Vec<Suggestion> = Vec::new();
    // Every name (no prefix), then the units (offered only after a letter is typed).
    for prefix in ["", "m", "c", "i", "f", "d", "r", "u", "y", "k", "n"] {
        if let Ok(v) = app.session.execute("parameters.complete", &json!({ "prefix": prefix })) {
            for s in v["suggestions"].as_array().into_iter().flatten() {
                let text = s["text"].as_str().unwrap_or_default().to_string();
                if !text.is_empty() && !all.iter().any(|x| x.text == text) {
                    all.push(Suggestion {
                        text,
                        kind: s["kind"].as_str().unwrap_or_default().into(),
                        detail: s["detail"].as_str().unwrap_or_default().into(),
                    });
                }
            }
        }
    }
    ctx.data_mut(|d| d.insert_temp(cache_id(), Cache { revision: rev, all: Arc::new(all) }));
}

/// The identifier being typed before `cursor` (chars) and where it starts.
pub(crate) fn word_before(text: &str, cursor: usize) -> (usize, String) {
    let chars: Vec<char> = text.chars().collect();
    let end = cursor.min(chars.len());
    let mut start = end;
    while start > 0 && chars.get(start - 1).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
        start -= 1;
    }
    (start, chars.get(start..end).map(|s| s.iter().collect()).unwrap_or_default())
}

/// Suggestions for the word being typed.
pub fn suggestions(ctx: &egui::Context, word: &str) -> Vec<Suggestion> {
    suggestions_for(ctx, word, false)
}

pub(crate) fn suggestions_for(ctx: &egui::Context, word: &str, explicit: bool) -> Vec<Suggestion> {
    if (!explicit && word.is_empty()) || word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Vec::new();
    }
    let all = ctx.data(|d| d.get_temp::<Cache>(cache_id())).map(|c| c.all).unwrap_or_default();
    // A complete token is ready to submit, even when longer names share its prefix.
    if !explicit && all.iter().any(|s| s.text == word) {
        return Vec::new();
    }
    let w = word.to_ascii_lowercase();
    all.iter()
        .filter(|s| s.text.to_ascii_lowercase().starts_with(&w) && (explicit || s.text.trim_end_matches('(') != word))
        .take(8)
        .cloned()
        .collect()
}

/// Offer keyboard and pointer completions under a focused expression box.
/// Returns true when a suggestion was accepted; callers must not submit that frame.
pub fn complete(ui: &mut egui::Ui, resp: &egui::Response, text: &mut String) -> bool {
    crate::expression_complete::complete(ui, resp, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_suggestions() {
        assert_eq!(word_before("2 * wid", 7), (4, "wid".to_string()));
        assert_eq!(word_before("width + 1", 5), (0, "width".to_string()));
        assert_eq!(word_before("1 + ", 4), (4, String::new()));
        assert_eq!(word_before("ü", 9), (0, "ü".to_string()));
        let ctx = egui::Context::default();
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        let _ = app.run("parameters.add", json!({"name": "width", "expression": "40 mm"}));
        publish(&mut app, &ctx);
        let s = suggestions(&ctx, "wi");
        assert_eq!(s.first().map(|x| x.text.as_str()), Some("width"));
        assert!(suggestions(&ctx, "ata").iter().any(|x| x.text == "atan2("));
        assert!(suggestions(&ctx, "1").is_empty());
        // The dialog's rows come from the engine.
        let r = rows(&mut app);
        assert!(r.iter().any(|x| x.name == "width" && x.source == "user"));
        assert!(check(&mut app, "width / 2", "mm").is_ok());
        assert!(check(&mut app, "width + 1 deg", "mm").is_err());
    }

    /// A favourite is listed twice, under Favorites and in its own table; typing into either of its
    /// expression boxes enters each key once (#1767: `1.1` came out `1.11.1`).
    #[test]
    fn a_favourite_parameter_takes_each_key_once() {
        for (favourite, row) in [(false, 0), (true, 0), (true, 1)] {
            let mut h = crate::scenario::Harness::new();
            h.app.home.open = false;
            h.app.run("parameters.add", json!({"name": "width", "expression": "10 mm"})).unwrap();
            if favourite {
                h.app.run("parameters.favorite", json!({"name": "width", "favorite": true})).unwrap();
            }
            h.step(&json!({"start": "parameters.change"})).unwrap();
            h.frames(4);
            // `param:width` is the box in the parameter's own table, `param:favorite:width` the one
            // under Favorites.
            h.step(&json!({"click": {"handle": if row == 0 { "param:width" } else { "param:favorite:width" }}})).unwrap();
            h.frames(2);
            h.step(&json!({"key": "A", "cmd": true})).unwrap();
            h.step(&json!({"text": "1.1"})).unwrap();
            h.frames(2);
            let editing = h.app.params.as_ref().unwrap().editing.get("width").cloned();
            assert_eq!(editing.as_deref(), Some("1.1"), "favourite {favourite}, row {row}");
        }
    }
}
