//! The Bill of Materials window: `assembly.bom`'s rows (flat or top level, grouped by part
//! number or by definition, a part's sub-parts in its row or not), with Export CSV… and Export
//! JSON…. Picking a row edits its part
//! number, description and revision through one `component.metadata.set` per definition. The
//! toolbar's Bill of Materials (`ui.start` of `assembly.bom`) opens it.

use egui::RichText;
use serde_json::{Value, json};

use crate::SolveApp;
use crate::theme::Tokens;

/// The window's state; the rows come from the engine.
#[derive(Default)]
pub struct BomPanel {
    /// Top level instead of flat.
    pub top: bool,
    /// One row per definition instead of per part number.
    pub by_definition: bool,
    /// A part with a part number keeps its sub-parts (a bolt's washers and nut) in its row.
    pub group_children: bool,
    /// The rows for (design revision, top, by_definition, group_children).
    cache: Option<(u64, [bool; 3], Value)>,
    /// The picked row's index, and its fields as edited.
    pub picked: Option<usize>,
    pub fields: [String; 3],
    /// The last refusal.
    pub error: Option<String>,
}

impl BomPanel {
    fn params(&self) -> Value {
        json!({"levels": if self.top { "top" } else { "flat" }, "group_by": [if self.by_definition { "definition" } else { "part_number" }],
            "group_children": self.group_children})
    }

    /// The bill of materials for the design as it is now (computed again only after a change).
    fn rows(&mut self, app: &mut SolveApp) -> Value {
        let revision = app.session.revision;
        let key = [self.top, self.by_definition, self.group_children];
        if let Some((r, k, v)) = &self.cache
            && (*r, *k) == (revision, key)
        {
            return v.clone();
        }
        let v = match app.session.execute("assembly.bom", &self.params()) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(e.to_string());
                json!({"rows": [], "total_occurrences": 0})
            }
        };
        self.cache = Some((revision, key, v.clone()));
        v
    }
}

/// Open the window.
pub fn open(app: &mut SolveApp) {
    app.bom = Some(BomPanel::default());
}

fn text(row: &Value, k: &str) -> String {
    match &row[k] {
        Value::String(t) => t.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// Apply the picked row's fields to each of its definitions (one undo step each).
fn apply(app: &mut SolveApp, d: &mut BomPanel, row: &Value) {
    let [pn, desc, rev] = d.fields.clone();
    for c in row["components"].as_array().into_iter().flatten() {
        let r = app.run("component.metadata.set", json!({"component": c, "part_number": pn, "description": desc, "revision": rev}));
        if let Err(e) = r {
            d.error = Some(e);
            return;
        }
    }
    d.error = None;
}

/// Draw the window when it is open. Called once per frame.
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(mut d) = app.bom.take() else { return };
    let t = Tokens::get();
    let mut open = true;
    let bom = d.rows(app);
    let rows: Vec<Value> = bom["rows"].as_array().cloned().unwrap_or_default();
    let (mut export, mut apply_row) = (None, None);
    let title = RichText::new(crate::i18n::tr_label("Bill of Materials")).strong().size(13.0);
    crate::frame::window(ctx, title, crate::frame::Width::Wide).id(egui::Id::new("sc_bom")).open(&mut open).resizable(true).show(ctx, |ui| {
        ui.horizontal(|ui| {
            let was = (d.top, d.by_definition, d.group_children);
            ui.selectable_value(&mut d.top, false, "Parts only");
            ui.selectable_value(&mut d.top, true, "Top level");
            ui.separator();
            ui.selectable_value(&mut d.by_definition, false, "By part number");
            ui.selectable_value(&mut d.by_definition, true, "By definition");
            ui.separator();
            ui.checkbox(&mut d.group_children, "Sub-parts in their part's row");
            if was != (d.top, d.by_definition, d.group_children) {
                d.picked = None;
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            egui::Grid::new("sc_bom_grid").num_columns(6).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["Item", "Part number", "Description", "Rev", "Qty", "Definition"] {
                    ui.label(RichText::new(h).strong().color(t.text_dim));
                }
                ui.end_row();
                for (i, row) in rows.iter().enumerate() {
                    let picked = d.picked == Some(i);
                    if ui.selectable_label(picked, text(row, "item")).clicked() {
                        d.picked = if picked { None } else { Some(i) };
                        d.fields = [text(row, "part_number"), text(row, "description"), text(row, "revision")];
                    }
                    ui.label(text(row, "part_number"));
                    ui.label(text(row, "description"));
                    ui.label(text(row, "revision"));
                    ui.label(text(row, "count"));
                    ui.label(RichText::new(text(row, "definition")).color(t.text_dim));
                    ui.end_row();
                    for c in row["children"].as_array().into_iter().flatten() {
                        ui.label(RichText::new(text(c, "item")).color(t.text_dim));
                        ui.label(text(c, "part_number"));
                        ui.label(text(c, "description"));
                        ui.label(text(c, "revision"));
                        ui.label(text(c, "count"));
                        ui.label(RichText::new(text(c, "definition")).color(t.text_dim));
                        ui.end_row();
                    }
                }
            });
        });
        ui.label(RichText::new(format!("{} rows, {} placed", rows.len(), text(&bom, "total_occurrences"))).small().color(t.text_dim));
        if let Some(row) = d.picked.and_then(|i| rows.get(i)) {
            ui.separator();
            egui::Grid::new("sc_bom_edit").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                for (label, field) in ["Part number", "Description", "Revision"].into_iter().zip(d.fields.iter_mut()) {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(field).desired_width(280.0));
                    ui.end_row();
                }
            });
            if ui.button("Apply").clicked() {
                apply_row = Some(row.clone());
            }
        }
        if let Some(e) = &d.error {
            ui.label(RichText::new(e).color(t.error));
        }
        ui.separator();
        ui.horizontal(|ui| {
            for (label, ext) in [("Export CSV…", "csv"), ("Export JSON…", "json")] {
                if ui.button(label).clicked() {
                    export = Some(ext);
                }
            }
        });
    });
    if let Some(row) = apply_row {
        apply(app, &mut d, &row);
    }
    if let Some(ext) = export {
        let mut params = d.params();
        params["format"] = json!(ext);
        app.pick_save_command(&format!("bill-of-materials.{ext}"), &[ext], "assembly.bom", params);
    }
    if open {
        app.bom = Some(d);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::scenario::Harness;

    #[test]
    fn the_window_lists_counts_and_edits_a_part_number() {
        let mut h = Harness::new();
        h.app.run("component.create", json!({"name": "Plate"})).unwrap();
        h.app.run("solid.box", json!({"length": 40, "width": 20, "height": 3})).unwrap();
        h.app.run("component.activate", json!({"component": 0})).unwrap();
        h.app.run("occurrence.copy", json!({"component": "Plate", "translate": [50, 0, 0]})).unwrap();
        h.step(&json!({"start": "assembly.bom"})).unwrap();
        assert!(h.app.bom.is_some(), "ui.start opens the window");
        h.frames(2);
        assert!(h.widget("Export CSV…").is_some());
        assert!(h.widget("1 rows, 2 placed").is_some());
        let undo = h.app.session.undo.len();
        h.step(&json!({"widget": "1"})).unwrap();
        let d = h.app.bom.as_mut().unwrap();
        assert_eq!(d.picked, Some(0));
        d.fields[0] = "PL-100".into();
        h.frames(2);
        h.step(&json!({"widget": "Apply"})).unwrap();
        let m = h.app.session.execute("component.metadata", &json!({"component": "Plate"})).unwrap();
        assert_eq!(m["part_number"], "PL-100");
        assert_eq!(h.app.session.undo.len(), undo + 1, "one undo step");
        h.frames(2);
        assert!(h.widget("PL-100").is_some(), "the list follows the change");
    }

    #[test]
    fn sub_parts_show_under_their_part() {
        let mut h = Harness::new();
        let bolt = h.app.run("parts.insert", json!({"family": "hex_bolt", "size": "M6", "length": 30, "point": [0, 0, 0]})).unwrap();
        h.app.run("component.activate", json!({"component": bolt["component"]})).unwrap();
        h.app.run("parts.insert", json!({"family": "hex_nut", "size": "M6", "point": [0, 0, -4]})).unwrap();
        h.app.run("component.activate", json!({"component": 0})).unwrap();
        h.step(&json!({"start": "assembly.bom"})).unwrap();
        h.frames(2);
        assert!(h.widget("2 rows, 2 placed").is_some());
        h.step(&json!({"widget": "Sub-parts in their part's row"})).unwrap();
        h.frames(2);
        assert!(h.widget("1 rows, 2 placed").is_some());
        assert!(h.widget("1.1").is_some(), "the nut under its bolt");
    }
}
