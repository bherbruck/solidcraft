//! Sheet metal dialogs: Flange (base from profiles, edge flanges from picked edges, contour from
//! open sketch lines; the picks decide which), Hem, Unfold and Refold, Convert to Sheet Metal,
//! Sheet Metal Rules (the rule manager) and Create Flat Pattern (a flat view of the part with
//! its bend lines, the bend table and DXF export).

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, pos2, vec2};
use serde_json::{Map, Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::doc::expr::Kind as ValueKind;
use solidcraft_engine::doc::sheet::SheetRule;
use solidcraft_engine::doc::{FeatureKind, ProfileSel};
use solidcraft_engine::geom::Vec3;

use crate::SolveApp;
use crate::dialogs::{Dialog, Kind, combo, edge_sel, face_sel, field, profile_indices, row_label};
use crate::selection::{AXES, BODIES, CURVES, EDGES, PLANAR_FACES, PROFILES, SelInput};
use crate::theme::Tokens;

const FOLD_POSITIONS: [&str; 4] = ["centerline", "start", "end", "mould"];
const FOLD_POSITION_LABELS: [&str; 4] = ["Centerline", "Start of Bend", "End of Bend", "Outside Mould Line"];
const FLANGE_TYPES: [&str; 3] = ["Base", "Edge", "Contour"];
const POSITIONS: [&str; 3] = ["inside", "outside", "middle"];
const POSITION_LABELS: [&str; 3] = ["Inside", "Outside", "Middle"];
/// Rule fields the manager edits (parameter name, label).
const RULE_FIELDS: [(&str, &str); 8] = [
    ("thickness", "Thickness"),
    ("k_factor", "K Factor"),
    ("bend_radius", "Bend Radius"),
    ("relief_width", "Relief Width"),
    ("relief_depth", "Relief Depth"),
    ("corner_relief", "Corner Relief"),
    ("hem_gap", "Hem Gap"),
    ("gap", "Flange Gap"),
];
/// The corner relief shapes the manager offers (the rule's `corner_relief_shape`) and their labels.
const RELIEF_SHAPES: [&str; 4] = ["notch", "round", "rectangular", "tear"];
const RELIEF_SHAPE_LABELS: [&str; 4] = ["Notch", "Round", "Rectangular", "Tear"];
/// The corner relief shapes an edge flange offers: the rule's (index 0, nothing of its own), then
/// the shapes of `RELIEF_SHAPES` in order.
const FLANGE_RELIEF_LABELS: [&str; 5] = ["Rule", "Notch", "Round", "Rectangular", "Tear"];

#[derive(Clone, Debug)]
pub enum Sm {
    /// Base, edge or contour flange (`ty` follows the picks: profiles, edges, sketch lines).
    Flange {
        ty: usize,
        height: String,
        angle: String,
        radius: String,
        position: usize,
        flip: bool,
        distance: String,
        reverse: bool,
        rule: usize,
        /// Edge flanges: the edges with an angle and/or a height of their own (the picked point,
        /// angle, height; an empty text means the dialog's), from the rows per edge.
        own: Vec<(Vec3, String, String)>,
        /// Edge flanges: the corner relief shape of their own (index into `FLANGE_RELIEF_LABELS`;
        /// 0 is the rule's) and its size (empty: the rule's).
        relief: usize,
        relief_size: String,
    },
    Hem {
        length: String,
        gap: String,
        flip: bool,
    },
    /// Fold (bend) along a sketch line; `position`: where the line sits in the bend.
    Fold {
        angle: String,
        radius: String,
        position: usize,
        flip: bool,
    },
    Unfold {
        refold: bool,
    },
    Convert {
        rule: usize,
    },
    /// The rule manager: the rule shown (index into the rules, or past the end for a new one).
    Rules {
        pick: usize,
        name: String,
        values: Vec<String>,
        /// The corner relief shape (index into `RELIEF_SHAPES`).
        relief: usize,
        active: bool,
        loaded: Option<usize>,
    },
    /// The flat pattern of the picked (or last) sheet body.
    FlatPattern {
        data: Option<Result<Value, String>>,
        key: Option<(u64, Option<String>)>,
        status: Option<String>,
    },
}

impl Sm {
    pub fn title(&self) -> &'static str {
        match self {
            Sm::Flange { .. } => "FLANGE",
            Sm::Hem { .. } => "HEM",
            Sm::Fold { .. } => "FOLD",
            Sm::Unfold { refold: false } => "UNFOLD",
            Sm::Unfold { refold: true } => "REFOLD",
            Sm::Convert { .. } => "CONVERT TO SHEET METAL",
            Sm::Rules { .. } => "SHEET METAL RULES",
            Sm::FlatPattern { .. } => "FLAT PATTERN",
        }
    }

    pub fn previews(&self) -> bool {
        !matches!(self, Sm::FlatPattern { .. })
    }

    /// The value on the canvas (the flange's follows its picks).
    pub fn primary(&mut self, inputs: &[SelInput]) -> Option<(&'static str, ValueKind, &mut String)> {
        if let Sm::Flange { ty, .. } = self {
            *ty = flange_type(inputs);
        }
        Some(match self {
            Sm::Flange { ty: 1, height, .. } => ("Height", ValueKind::Length, height),
            Sm::Flange { ty: 2, distance, .. } => ("Distance", ValueKind::Length, distance),
            Sm::Hem { length, .. } => ("Length", ValueKind::Length, length),
            Sm::Fold { angle, .. } => ("Angle", ValueKind::Angle, angle),
            _ => return None,
        })
    }
}

fn new_flange(ty: usize) -> Sm {
    Sm::Flange {
        ty,
        height: "10 mm".into(),
        angle: "90 deg".into(),
        radius: String::new(),
        position: 0,
        flip: false,
        distance: "20 mm".into(),
        reverse: false,
        rule: 0,
        own: Vec::new(),
        relief: 0,
        relief_size: String::new(),
    }
}

/// The dialog for a sheet metal command.
pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let (sm, inputs) = match id {
        "sheet.flange" => (new_flange(0), vec![SelInput::new("Profile / Edges", PROFILES | EDGES | CURVES, true)]),
        "sheet.hem" => (Sm::Hem { length: "5 mm".into(), gap: String::new(), flip: false }, vec![SelInput::new("Edges", EDGES, true)]),
        "sheet.fold" => (
            Sm::Fold { angle: "90 deg".into(), radius: String::new(), position: 0, flip: false },
            vec![SelInput::new("Bend Line", AXES, false), SelInput::new("Stationary Side", PLANAR_FACES, false)],
        ),
        "sheet.unfold" => (Sm::Unfold { refold: false }, vec![SelInput::new("Body (last if none)", BODIES, false)]),
        "sheet.refold" => (Sm::Unfold { refold: true }, vec![SelInput::new("Body (last if none)", BODIES, false)]),
        "sheet.convert" => {
            (Sm::Convert { rule: 0 }, vec![SelInput::new("Fixed Face", PLANAR_FACES, false), SelInput::new("Bend Edges", EDGES, true)])
        }
        "sheet.manage_rules" => {
            let active = app.session.doc.sheet.active.clone();
            let rules = rule_list(app);
            let pick = active.and_then(|a| rules.iter().position(|r| r.name == a)).unwrap_or(0);
            (Sm::Rules { pick, name: String::new(), values: Vec::new(), relief: 0, active: true, loaded: None }, vec![])
        }
        "sheet.flat_pattern" => (Sm::FlatPattern { data: None, key: None, status: None }, vec![SelInput::new("Body (last if none)", BODIES, false)]),
        _ => return None,
    };
    Some((Kind::Sheet(sm), inputs))
}

/// The design's rules (the default steel rule when it has none).
fn rule_list(app: &SolveApp) -> Vec<SheetRule> {
    let r = &app.session.doc.sheet.rules;
    if r.is_empty() { vec![SheetRule::steel()] } else { r.clone() }
}

/// "(active rule)" and the rule names, for a rule box.
fn rule_names(app: &SolveApp) -> Vec<String> {
    std::iter::once("(active rule)".to_string()).chain(rule_list(app).into_iter().map(|r| r.name)).collect()
}

fn rule_combo(app: &SolveApp, ui: &mut egui::Ui, id: &str, rule: &mut usize) {
    let names = rule_names(app);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    row_label(ui, "Rule");
    combo(ui, id, &refs, rule);
    ui.end_row();
}

/// The flange type the picks make: edges an edge flange, sketch lines a contour, else base.
fn flange_type(inputs: &[SelInput]) -> usize {
    let items = inputs.first().map(|i| i.items.as_slice()).unwrap_or(&[]);
    if items.iter().any(|x| matches!(x, Sel::Edge { .. })) {
        1
    } else if items.iter().any(|x| matches!(x, Sel::SketchCurve { .. })) {
        2
    } else {
        0
    }
}

/// The dialog's own rows (after the selection inputs); true when Enter was pressed in a value.
pub fn rows(app: &mut SolveApp, ui: &mut egui::Ui, k: &mut Sm, inputs: &mut [SelInput]) -> bool {
    let t = Tokens::get();
    let mut enter = false;
    match k {
        Sm::Flange { ty, height, angle, radius, position, flip, distance, reverse, rule, own, relief, relief_size } => {
            *ty = flange_type(inputs);
            row_label(ui, "Type");
            ui.label(FLANGE_TYPES.get(*ty).copied().unwrap_or(""));
            ui.end_row();
            match *ty {
                1 => {
                    row_label(ui, "Height");
                    enter |= field(ui, height);
                    ui.end_row();
                    row_label(ui, "Angle");
                    enter |= field(ui, angle);
                    ui.end_row();
                    row_label(ui, "Bend Position");
                    combo(ui, "sm_pos", &POSITION_LABELS, position);
                    ui.end_row();
                    row_label(ui, "Bend Radius");
                    let r = ui.add(egui::TextEdit::singleline(radius).hint_text(crate::i18n::tr_label("from the rule")));
                    crate::params_dialog::complete(ui, &r, radius);
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                    // The relief at the corners these flanges make: the rule's unless set here.
                    row_label(ui, "Corner Relief Shape");
                    combo(ui, "sm_flange_relief", &FLANGE_RELIEF_LABELS, relief);
                    ui.end_row();
                    row_label(ui, "Corner Relief");
                    let r = ui.add(egui::TextEdit::singleline(relief_size).hint_text(crate::i18n::tr_label("from the rule")));
                    crate::params_dialog::complete(ui, &r, relief_size);
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                    // With several edges, a row for each: an angle and a height of its own, left
                    // empty for the values above (shown as the hints).
                    let points = edge_point_list(inputs);
                    if points.len() > 1 || own.iter().any(|o| !(o.1.trim().is_empty() && o.2.trim().is_empty())) {
                        row_label(ui, "Per Edge");
                        ui.label(RichText::new(crate::i18n::tr_label("angle, height")).weak())
                            .on_hover_text("An edge's own angle and height. Empty: the values above.");
                        ui.end_row();
                        let w = ((ui.spacing().text_edit_width - ui.spacing().item_spacing.x) / 2.0).max(40.0);
                        for (n, p) in points.iter().enumerate() {
                            let slot = own_slot(own, *p);
                            let Some((_, a, h)) = own.get_mut(slot) else { continue };
                            row_label(ui, &format!("Edge {}", n + 1));
                            ui.horizontal(|ui| {
                                for (text, hint, what, id) in
                                    [(a, angle.as_str(), "angle", "sm_edge_angle"), (h, height.as_str(), "height", "sm_edge_height")]
                                {
                                    let r = ui
                                        .add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(w).id_salt((id, n)))
                                        .on_hover_text(format!("The {what} of the edge picked at ({:.1}, {:.1}, {:.1})", p.x, p.y, p.z));
                                    // For scenarios and the control channel: `dialog:Edge 2 angle`.
                                    crate::scenario::publish_handle(&format!("dialog:Edge {} {what}", n + 1), r.rect.center());
                                    crate::params_dialog::complete(ui, &r, text);
                                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                }
                            });
                            ui.end_row();
                        }
                    }
                }
                2 => {
                    row_label(ui, "Distance");
                    enter |= field(ui, distance);
                    ui.end_row();
                    row_label(ui, "Other side");
                    ui.checkbox(reverse, "");
                    ui.end_row();
                    rule_combo(app, ui, "sm_rule", rule);
                }
                _ => rule_combo(app, ui, "sm_rule", rule),
            }
            row_label(ui, "Flip");
            ui.checkbox(flip, "");
            ui.end_row();
        }
        Sm::Hem { length, gap, flip } => {
            row_label(ui, "Type");
            ui.label(crate::i18n::tr_label("Flat"));
            ui.end_row();
            row_label(ui, "Length");
            enter |= field(ui, length);
            ui.end_row();
            row_label(ui, "Gap");
            let r = ui.add(egui::TextEdit::singleline(gap).hint_text(crate::i18n::tr_label("from the rule")));
            crate::params_dialog::complete(ui, &r, gap);
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.end_row();
            row_label(ui, "Flip");
            ui.checkbox(flip, "");
            ui.end_row();
        }
        Sm::Fold { angle, radius, position, flip } => {
            row_label(ui, "Bend Line Position");
            combo(ui, "sm_fold_pos", &FOLD_POSITION_LABELS, position);
            ui.end_row();
            row_label(ui, "Angle");
            enter |= field(ui, angle);
            ui.end_row();
            row_label(ui, "Bend Radius");
            let r = ui.add(egui::TextEdit::singleline(radius).hint_text(crate::i18n::tr_label("from the rule")));
            crate::params_dialog::complete(ui, &r, radius);
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.end_row();
            row_label(ui, "Flip");
            ui.checkbox(flip, "");
            ui.end_row();
        }
        Sm::Unfold { .. } => {}
        Sm::Convert { rule } => rule_combo(app, ui, "sm_conv_rule", rule),
        Sm::Rules { pick, name, values, relief, active, loaded } => {
            let rules = rule_list(app);
            let mut names: Vec<String> = rules.iter().map(|r| r.name.clone()).collect();
            names.push("New rule…".into());
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            row_label(ui, "Rule");
            combo(ui, "sm_rules", &refs, pick);
            ui.end_row();
            if *loaded != Some(*pick) {
                // Show the picked rule (a new one starts from the one shown before).
                let base = rules.get(*pick).cloned().unwrap_or_else(|| rules.first().cloned().unwrap_or_else(SheetRule::steel));
                *values = vec![
                    base.thickness.clone(),
                    base.k_factor.clone(),
                    base.bend_radius.clone(),
                    base.relief_width.clone(),
                    base.relief_depth.clone(),
                    base.corner_relief.clone(),
                    base.hem_gap.clone(),
                    base.gap.clone(),
                ];
                *relief = RELIEF_SHAPES.iter().position(|s| s.eq_ignore_ascii_case(base.corner_relief_shape.trim())).unwrap_or(0);
                *active = app.session.doc.sheet_rule(None).name == base.name || *pick >= rules.len();
                if *pick >= rules.len() {
                    *name = format!("Rule{}", rules.len() + 1);
                }
                *loaded = Some(*pick);
            }
            if *pick >= rules.len() {
                row_label(ui, "Name");
                ui.text_edit_singleline(name);
                ui.end_row();
            }
            for ((key, label), v) in RULE_FIELDS.iter().zip(values.iter_mut()) {
                if *key == "corner_relief" {
                    // The shape goes with its size, the row below.
                    row_label(ui, "Corner Relief Shape");
                    combo(ui, "sm_relief_shape", &RELIEF_SHAPE_LABELS, relief);
                    ui.end_row();
                }
                row_label(ui, label);
                enter |= field(ui, v);
                ui.end_row();
            }
            row_label(ui, "Active rule");
            ui.checkbox(active, "");
            ui.end_row();
            if *pick < rules.len() && !app.session.doc.sheet.rules.is_empty() {
                row_label(ui, "");
                if ui.button(crate::i18n::tr_label("Delete rule")).clicked() {
                    let n = rules.get(*pick).map(|r| r.name.clone()).unwrap_or_default();
                    match app.run("sheet.manage_rules", json!({"name": n, "delete": true})) {
                        Ok(_) => {
                            *pick = 0;
                            *loaded = None;
                        }
                        Err(e) => app.set_status(e, true),
                    }
                }
                ui.end_row();
            }
        }
        Sm::FlatPattern { data, key, status } => {
            let body = inputs.first().and_then(|i| i.items.first()).and_then(|x| if let Sel::Body { name } = x { Some(name.clone()) } else { None });
            let want = (app.session.revision, body.clone());
            if key.as_ref() != Some(&want) {
                let mut p = json!({});
                if let Some(b) = &body {
                    p["body"] = json!(b);
                }
                *data = Some(app.session.execute("sheet.flat_pattern", &p).map_err(|e| e.to_string()));
                *key = Some(want);
            }
            match data {
                Some(Ok(v)) => {
                    let size = &v["flat_size_mm"];
                    row_label(ui, "Body");
                    ui.label(v["body"].as_str().unwrap_or(""));
                    ui.end_row();
                    row_label(ui, "Flat size");
                    ui.label(format!(
                        "{} × {} × {}",
                        crate::prefs::show_mm(app, size[0].as_f64().unwrap_or(0.0), 1),
                        crate::prefs::show_mm(app, size[1].as_f64().unwrap_or(0.0), 1),
                        crate::prefs::show_mm(app, size[2].as_f64().unwrap_or(0.0), 1)
                    ));
                    ui.end_row();
                    let bends = v["bends"].as_array().cloned().unwrap_or_default();
                    row_label(ui, "Bends");
                    ui.label(format!("{}", bends.len()));
                    ui.end_row();
                    for b in bends.iter().take(50) {
                        let up = b["direction"] == "up";
                        let (up_col, down_col) = bend_colors();
                        row_label(ui, &format!("  Bend {}", b["bend"].as_u64().unwrap_or(0)));
                        ui.label(
                            RichText::new(format!(
                                "{} {:.1}°  R{}",
                                if up { "Up" } else { "Down" },
                                b["angle_deg"].as_f64().unwrap_or(0.0),
                                crate::prefs::show_mm(app, b["radius"].as_f64().unwrap_or(0.0), 1)
                            ))
                            .color(if up { up_col } else { down_col }),
                        );
                        ui.end_row();
                    }
                    row_label(ui, "");
                    if ui.button("Export DXF…").clicked() {
                        *status = Some(export_flat(app, v["body"].as_str().unwrap_or(""), "dxf"));
                    }
                    ui.end_row();
                    row_label(ui, "");
                    if ui.button("Export Bend Table…").clicked() {
                        *status = Some(export_flat(app, v["body"].as_str().unwrap_or(""), "csv"));
                    }
                    ui.end_row();
                    if let Some(s) = status {
                        row_label(ui, "");
                        ui.add(egui::Label::new(RichText::new(s.as_str()).color(t.text_dim)).wrap());
                        ui.end_row();
                    }
                }
                Some(Err(e)) => {
                    row_label(ui, "");
                    ui.add(egui::Label::new(RichText::new(e.as_str()).color(t.error)).wrap());
                    ui.end_row();
                }
                None => {}
            }
        }
    }
    enter
}

fn export_flat(app: &mut SolveApp, body: &str, extension: &str) -> String {
    let name = format!("{}-{body}.{extension}", app.session.doc.name);
    let body = body.to_string();
    let command = if extension == "csv" { "sheet.export_bend_table" } else { "sheet.export_dxf" };
    app.pick_save(&name, &[extension], move |app, path| {
        let message = if let Some(path) = path {
            match app.run(command, json!({"body": body, "path": path})) {
                Ok(_) => {
                    app.set_status(format!("Exported {path}"), false);
                    format!("Saved {path}")
                }
                Err(e) => e,
            }
        } else {
            "Export cancelled".into()
        };
        if let Some(d) = app.dialog.as_mut()
            && let Kind::Sheet(Sm::FlatPattern { status, .. }) = &mut d.kind
        {
            *status = Some(message);
        }
    });
    "Choose an export file…".into()
}

// ---- commands ----

fn rule_value(app: &SolveApp, rule: usize) -> Option<String> {
    rule.checked_sub(1).and_then(|i| rule_list(app).get(i).map(|r| r.name.clone()))
}

fn items(inputs: &[SelInput], i: usize) -> &[Sel] {
    inputs.get(i).map(|x| x.items.as_slice()).unwrap_or(&[])
}

fn pt(v: Vec3) -> Value {
    json!([v.x, v.y, v.z])
}

fn edge_points(inputs: &[SelInput]) -> Vec<Value> {
    items(inputs, 0).iter().filter_map(|x| if let Sel::Edge { point, .. } = x { Some(pt(*point)) } else { None }).collect()
}

/// The picked edges' points.
fn edge_point_list(inputs: &[SelInput]) -> Vec<Vec3> {
    items(inputs, 0).iter().filter_map(|x| if let Sel::Edge { point, .. } = x { Some(*point) } else { None }).collect()
}

/// Where the own angle and height of the edge picked at `p` are kept (an empty entry on first use).
fn own_slot(own: &mut Vec<(Vec3, String, String)>, p: Vec3) -> usize {
    own.iter().position(|o| o.0.dist(p) < 1e-9).unwrap_or_else(|| {
        own.push((p, String::new(), String::new()));
        own.len() - 1
    })
}

/// The edges for `sheet.flange`: a plain point, or `{point, angle?, height?}` for an edge with
/// an angle and/or a height of its own.
fn flange_edges(inputs: &[SelInput], own: &[(Vec3, String, String)]) -> Vec<Value> {
    edge_point_list(inputs)
        .into_iter()
        .map(|p| {
            let (a, h) = own.iter().find(|o| o.0.dist(p) < 1e-9).map(|o| (o.1.trim(), o.2.trim())).unwrap_or(("", ""));
            if a.is_empty() && h.is_empty() {
                return pt(p);
            }
            let mut e = json!({"point": pt(p)});
            for (k, v) in [("angle", a), ("height", h)] {
                if !v.is_empty() {
                    e[k] = json!(v);
                }
            }
            e
        })
        .collect()
}

/// The commands the dialog's OK runs (`extra`: parameters kept from the feature being edited).
pub fn commands(app: &SolveApp, k: &Sm, inputs: &[SelInput], extra: &Map<String, Value>) -> Result<Vec<(String, Value)>, String> {
    let s = &app.session;
    let (cmd, mut p): (&str, Value) = match k {
        Sm::Flange { height, angle, radius, position, flip, distance, reverse, rule, own, relief, relief_size, .. } => {
            let mut p = match flange_type(inputs) {
                1 => {
                    let mut p = json!({"type": "edge", "edges": flange_edges(inputs, own), "height": height, "angle": angle,
                        "position": POSITIONS.get(*position).copied().unwrap_or("inside")});
                    if !radius.trim().is_empty() {
                        p["radius"] = json!(radius);
                    }
                    if let Some(shape) = relief.checked_sub(1).and_then(|i| RELIEF_SHAPES.get(i)) {
                        p["corner_relief_shape"] = json!(shape);
                    }
                    if !relief_size.trim().is_empty() {
                        p["corner_relief"] = json!(relief_size);
                    }
                    p
                }
                2 => {
                    let ids: Vec<String> =
                        items(inputs, 0).iter().filter_map(|x| if let Sel::SketchCurve { id } = x { Some(id.clone()) } else { None }).collect();
                    let st = s.world_state();
                    let sketch = st
                        .sketches
                        .iter()
                        .rev()
                        .find(|ss| ids.iter().all(|id| ss.sketch.curve_index(id).is_some()))
                        .map(|ss| ss.feature)
                        .ok_or("pick lines of one sketch")?;
                    json!({"type": "contour", "sketch": sketch, "curves": ids, "distance": distance, "reverse": reverse})
                }
                _ => {
                    let ps: Vec<(u64, usize)> = items(inputs, 0)
                        .iter()
                        .filter_map(|x| if let Sel::Profile { sketch, index } = x { Some((*sketch, *index)) } else { None })
                        .collect();
                    let sketch = ps.first().map(|x| x.0).ok_or("select a profile, edges or sketch lines first")?;
                    let idx: Vec<usize> = ps.iter().filter(|x| x.0 == sketch).map(|x| x.1).collect();
                    json!({"type": "base", "sketch": sketch, "profiles": idx})
                }
            };
            p["flip"] = json!(flip);
            if let Some(r) = rule_value(app, *rule).filter(|_| p["type"] != "edge") {
                p["rule"] = json!(r);
            }
            ("sheet.flange", p)
        }
        Sm::Hem { length, gap, flip } => {
            let edges = edge_points(inputs);
            if edges.is_empty() {
                return Err("select sheet edges first".into());
            }
            let mut p = json!({"edges": edges, "length": length, "flip": flip});
            if !gap.trim().is_empty() {
                p["gap"] = json!(gap);
            }
            ("sheet.hem", p)
        }
        Sm::Fold { angle, radius, position, flip } => {
            let mut p = json!({"angle": angle, "position": FOLD_POSITIONS.get(*position).copied().unwrap_or("centerline"), "flip": flip});
            match items(inputs, 0).first() {
                Some(Sel::SketchCurve { id }) => {
                    let st = s.world_state();
                    let sketch = st
                        .sketches
                        .iter()
                        .rev()
                        .find(|ss| ss.sketch.curve_index(id).is_some())
                        .map(|ss| ss.feature)
                        .ok_or("the line's sketch is gone")?;
                    p["sketch"] = json!(sketch);
                    p["curve"] = json!(id);
                }
                // An edited fold made from two points keeps them (in `extra`).
                _ if extra.contains_key("points") => {}
                _ => return Err("select the bend line (a sketch line) first".into()),
            }
            if !radius.trim().is_empty() {
                p["radius"] = json!(radius);
            }
            if let Some(Sel::Face { point, .. }) = items(inputs, 1).first() {
                p["fixed"] = pt(*point);
            }
            ("sheet.fold", p)
        }
        Sm::Unfold { refold } => {
            let mut p = json!({});
            if let Some(Sel::Body { name }) = items(inputs, 0).first() {
                p["body"] = json!(name);
            }
            (if *refold { "sheet.refold" } else { "sheet.unfold" }, p)
        }
        Sm::Convert { rule } => {
            let Some(Sel::Face { body, point, .. }) = items(inputs, 0).first() else {
                return Err("select the fixed face first (a plate's large face)".into());
            };
            let mut p = json!({"body": body, "face": pt(*point)});
            // The sharp edges of a solid or shelled body that become bends (a plate has none).
            let bends: Vec<Value> =
                items(inputs, 1).iter().filter_map(|x| if let Sel::Edge { point, .. } = x { Some(pt(*point)) } else { None }).collect();
            if !bends.is_empty() {
                p["bends"] = json!(bends);
            }
            if let Some(r) = rule_value(app, *rule) {
                p["rule"] = json!(r);
            }
            ("sheet.convert", p)
        }
        Sm::Rules { pick, name, values, relief, active, .. } => {
            let rules = rule_list(app);
            let n = match rules.get(*pick) {
                Some(r) => r.name.clone(),
                None => name.trim().to_string(),
            };
            if n.is_empty() {
                return Err("give the rule a name".into());
            }
            let mut p = json!({"name": n, "active": active, "corner_relief_shape": RELIEF_SHAPES.get(*relief).copied().unwrap_or("notch")});
            for ((k, _), v) in RULE_FIELDS.iter().zip(values) {
                if !v.trim().is_empty() {
                    p[*k] = json!(v);
                }
            }
            ("sheet.manage_rules", p)
        }
        Sm::FlatPattern { .. } => return Ok(Vec::new()),
    };
    if let Value::Object(m) = &mut p {
        for (k, v) in extra {
            m.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Ok(vec![(cmd.to_string(), p)])
}

/// The dialog editing a sheet metal feature, filled from it.
pub fn for_feature(app: &SolveApp, kind: &FeatureKind) -> Option<(Kind, Vec<SelInput>, Map<String, Value>)> {
    let s = &app.session;
    let st = s.world_state();
    let rule_index = |r: &Option<String>| r.as_ref().and_then(|n| rule_list(app).iter().position(|x| &x.name == n)).map_or(0, |i| i + 1);
    let mut extra = Map::new();
    let (k, items): (Sm, Vec<Sel>) = match kind {
        FeatureKind::SheetBase { sketch, profiles, rule, flip } => {
            let ps = st.sketch(sketch.0).map(|ss| profile_indices(ss, profiles)).unwrap_or_default();
            let mut f = new_flange(0);
            if let Sm::Flange { flip: fl, rule: r, .. } = &mut f {
                *fl = *flip;
                *r = rule_index(rule);
            }
            (f, ps.into_iter().map(|index| Sel::Profile { sketch: sketch.0, index }).collect())
        }
        FeatureKind::SheetContour { curves, distance, rule, flip, reverse, .. } => {
            let mut f = new_flange(2);
            if let Sm::Flange { flip: fl, rule: r, distance: d, reverse: rv, .. } = &mut f {
                *fl = *flip;
                *r = rule_index(rule);
                *d = distance.to_string();
                *rv = *reverse;
            }
            (f, curves.iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect())
        }
        FeatureKind::SheetFlange { edges, height, angle, radius, position, flip, body, corner_relief_shape, corner_relief } => {
            if let Some(b) = body {
                extra.insert("body".into(), json!(b));
            }
            // The edges, and the angle and height some have of their own (kept by the point the
            // edge's selection carries).
            let (mut picked, mut own) = (Vec::new(), Vec::new());
            for e in edges {
                let Some(sel) = edge_sel(s, e.point.0) else { continue };
                if let (Sel::Edge { point, .. }, true) = (&sel, e.angle.is_some() || e.height.is_some()) {
                    own.push((
                        *point,
                        e.angle.as_deref().map(str::to_string).unwrap_or_default(),
                        e.height.as_deref().map(str::to_string).unwrap_or_default(),
                    ));
                }
                picked.push(sel);
            }
            let f = Sm::Flange {
                ty: 1,
                height: height.to_string(),
                angle: angle.to_string(),
                radius: radius.as_deref().map(str::to_string).unwrap_or_default(),
                position: POSITIONS.iter().position(|p| p == position).unwrap_or(0),
                flip: *flip,
                distance: "20 mm".into(),
                reverse: false,
                rule: 0,
                own,
                relief: corner_relief_shape
                    .as_deref()
                    .and_then(|name| RELIEF_SHAPES.iter().position(|s| s.eq_ignore_ascii_case(name.trim())))
                    .map_or(0, |i| i + 1),
                relief_size: corner_relief.as_deref().map(str::to_string).unwrap_or_default(),
            };
            (f, picked)
        }
        FeatureKind::SheetHem { edges, length, gap, flip, body } => {
            if let Some(b) = body {
                extra.insert("body".into(), json!(b));
            }
            (
                Sm::Hem { length: length.to_string(), gap: gap.as_deref().map(str::to_string).unwrap_or_default(), flip: *flip },
                edges.iter().filter_map(|p| edge_sel(s, p.0)).collect(),
            )
        }
        FeatureKind::SheetFold { curve, a, b, angle, radius, position, flip, fixed, body, .. } => {
            if let Some(bd) = body {
                extra.insert("body".into(), json!(bd));
            }
            let k = Sm::Fold {
                angle: angle.to_string(),
                radius: radius.as_deref().map(str::to_string).unwrap_or_default(),
                position: FOLD_POSITIONS.iter().position(|x| x == position || (*x == "mould" && position == "mold")).unwrap_or(0),
                flip: *flip,
            };
            let (_, mut ins) = start(app, "sheet.fold")?;
            match curve {
                Some(c) => {
                    if let Some(i) = ins.first_mut() {
                        i.items = vec![Sel::SketchCurve { id: c.clone() }];
                    }
                }
                // A fold given by two points keeps them.
                None => {
                    extra.insert("points".into(), json!([pt(a.0), pt(b.0)]));
                }
            }
            if let Some(i) = ins.get_mut(1) {
                i.items = fixed.and_then(|f| face_sel(s, f.0)).into_iter().collect();
            }
            return Some((Kind::Sheet(k), ins, extra));
        }
        FeatureKind::SheetUnfold { body, refold } => {
            (Sm::Unfold { refold: *refold }, body.iter().map(|n| Sel::Body { name: n.to_string() }).collect())
        }
        FeatureKind::SheetConvert { face, rule, bends, .. } => {
            let (_, mut ins) = start(app, "sheet.convert")?;
            if let Some(i) = ins.first_mut() {
                i.items = face_sel(s, face.0).into_iter().collect();
            }
            if let Some(i) = ins.get_mut(1) {
                i.items = bends.iter().filter_map(|p| edge_sel(s, p.0)).collect();
            }
            return Some((Kind::Sheet(Sm::Convert { rule: rule_index(rule) }), ins, extra));
        }
        _ => return None,
    };
    let id = match &k {
        Sm::Flange { .. } => "sheet.flange",
        Sm::Hem { .. } => "sheet.hem",
        Sm::Unfold { refold: false } => "sheet.unfold",
        Sm::Unfold { .. } => "sheet.refold",
        _ => "sheet.convert",
    };
    let (_, mut inputs) = start(app, id)?;
    if let Some(i) = inputs.first_mut() {
        i.items = items;
    }
    // Profiles stay a profile selection (`All` keeps them all).
    if let (FeatureKind::SheetBase { profiles: ProfileSel::All, .. }, Some(i)) = (kind, inputs.first())
        && i.items.is_empty()
    {
        extra.insert("profiles".into(), json!("all"));
    }
    Some((Kind::Sheet(k), inputs, extra))
}

// ---- view ----

/// The direction an edge flange or hem grows from its first edge: out of the sheet's larger
/// neighbouring face (flipped with Flip).
pub fn arrow(app: &SolveApp, d: &Dialog) -> Option<Vec3> {
    let Kind::Sheet(k) = &d.kind else { return None };
    let flip = match k {
        Sm::Flange { ty: 1, flip, .. } | Sm::Hem { flip, .. } => *flip,
        _ => return None,
    };
    let Sel::Edge { body, index, point } = d.inputs.first()?.items.first()? else { return None };
    let st = app.session.world_state();
    let m = st.body(body)?.mesh();
    let faces = m.edge_faces.get(*index)?;
    let area = |f: u32| -> f64 {
        m.triangles
            .iter()
            .zip(&m.tri_face)
            .filter(|(_, tf)| **tf == f)
            .filter_map(|(t, _)| m.tri(t))
            .map(|[a, b, c]| (b - a).cross(c - a).len() * 0.5)
            .sum()
    };
    let f = faces.iter().copied().max_by(|a, b| area(*a).total_cmp(&area(*b)))?;
    let n = m
        .triangles
        .iter()
        .zip(&m.tri_face)
        .filter(|(_, tf)| **tf == f)
        .filter_map(|(t, _)| m.tri(t))
        .min_by(|a, b| ((a[0] + a[1] + a[2]) / 3.0).dist(*point).total_cmp(&((b[0] + b[1] + b[2]) / 3.0).dist(*point)))
        .and_then(|[a, b, c]| (b - a).cross(c - a).normalized())?;
    Some(if flip { -n } else { n })
}

/// Bend line colours: up and down (theme tokens, legible on either theme and in every colour
/// vision mode).
fn bend_colors() -> (Color32, Color32) {
    let t = crate::theme::Tokens::get();
    (t.bend_up, t.bend_down)
}

/// The flat pattern view: the outline, cut-outs and bend lines (up and down in their colours)
/// drawn flat over the viewport, fitted to it. True when the dialog is a flat pattern.
pub fn overlay(app: &mut SolveApp, painter: &egui::Painter, rect: Rect, d: &Dialog) -> bool {
    let Kind::Sheet(Sm::FlatPattern { data, .. }) = &d.kind else { return false };
    let t = Tokens::get();
    let (up_col, down_col) = bend_colors();
    painter.rect_filled(rect, 0.0, t.panel);
    let Some(Ok(v)) = data else {
        painter.text(rect.center(), Align2::CENTER_CENTER, "no sheet metal body", FontId::proportional(14.0), t.text_dim);
        return true;
    };
    let read = |p: &Value| -> Option<(f64, f64)> { Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)) };
    let loops: Vec<Vec<(f64, f64)>> =
        v["outline"].as_array().into_iter().flatten().map(|l| l.as_array().into_iter().flatten().filter_map(read).collect()).collect();
    let holes: Vec<Vec<(f64, f64)>> =
        v["cutouts"].as_array().into_iter().flatten().map(|l| l.as_array().into_iter().flatten().filter_map(read).collect()).collect();
    let mut lo = (f64::INFINITY, f64::INFINITY);
    let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in loops.iter().flatten() {
        lo = (lo.0.min(p.0), lo.1.min(p.1));
        hi = (hi.0.max(p.0), hi.1.max(p.1));
    }
    if !(lo.0.is_finite() && hi.0 > lo.0 && hi.1 > lo.1) {
        return true;
    }
    // Fit all retained islands to the exposed viewport. The command panel can grow
    // to its width limit, so its actual edge, rather than a fixed reserve, bounds the view.
    let panel_left = painter
        .ctx()
        .memory(|m| m.area_rect(egui::Id::new(("sc_dialog", "FLAT PATTERN"))))
        .map(|panel| panel.left())
        .unwrap_or(rect.right() - crate::frame::DIALOG_MAX_W);
    let content = crate::shell::content_rect(app, rect);
    let area = Rect::from_min_max(content.min + vec2(40.0, 60.0), pos2(panel_left.min(content.right()) - 20.0, content.bottom() - 60.0));
    if area.width() <= 0.0 || area.height() <= 0.0 {
        return true;
    }
    if !app.flat_view.fitted {
        app.flat_view.fit(rect, area, lo, hi);
    }
    let cam = app.flat_view.cam;
    let scale = f64::from(rect.height()) / (2.0 * cam.half_height());
    let ox = f64::from(rect.center().x) - cam.target.x * scale;
    let oy = f64::from(rect.center().y) + cam.target.y * scale;
    let to = |p: (f64, f64)| -> Pos2 { pos2((ox + p.0 * scale) as f32, (oy - p.1 * scale) as f32) };
    for l in &loops {
        let pts: Vec<Pos2> = l.iter().map(|p| to(*p)).collect();
        if pts.len() >= 3 {
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.8, t.text)));
        }
    }
    for h in &holes {
        let pts: Vec<Pos2> = h.iter().map(|p| to(*p)).collect();
        if pts.len() >= 2 {
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.4, t.text)));
        }
    }
    // A pocket retains material below (or above) the cut. Dashed contours and a depth
    // annotation distinguish it from the solid through-cut boundaries.
    let pockets: Vec<&Value> = v["pockets"].as_array().into_iter().flatten().collect();
    let mut pocket_annotations: Vec<(Pos2, Vec<String>)> = Vec::new();
    for pocket in &pockets {
        let outer: Vec<(f64, f64)> = pocket["outline"].as_array().into_iter().flatten().filter_map(read).collect();
        for contour in std::iter::once(&pocket["outline"]).chain(pocket["cutouts"].as_array().into_iter().flatten()) {
            let mut pts: Vec<Pos2> = contour.as_array().into_iter().flatten().filter_map(read).map(to).collect();
            if pts.len() >= 3 {
                pts.push(pts[0]);
                painter.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.6, t.accent), 3.0, 3.0));
            }
        }
        if !outer.is_empty() {
            let bounds = outer.iter().map(|p| to(*p)).fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(p, p)));
            let anchor = bounds.center_top() - vec2(0.0, 6.0);
            if let Some(label) = pocket_label(app, pocket) {
                if let Some((_, labels)) = pocket_annotations.iter_mut().find(|(at, _)| at.distance(anchor) < 1.0) {
                    if !labels.contains(&label) {
                        labels.push(label);
                    }
                } else {
                    pocket_annotations.push((anchor, vec![label]));
                }
            }
        }
    }
    for (anchor, labels) in pocket_annotations {
        painter.text(anchor, Align2::CENTER_BOTTOM, labels.join("\n"), FontId::proportional(12.0), t.accent);
    }
    for b in v["bends"].as_array().into_iter().flatten() {
        let (Some(a), Some(e)) = (read(&b["start"]), read(&b["end"])) else { continue };
        let col = if b["direction"] == "up" { up_col } else { down_col };
        painter.extend(egui::Shape::dashed_line(&[to(a), to(e)], Stroke::new(1.6, col), 8.0, 5.0));
    }
    // Legend.
    let mut y = rect.top() + 14.0;
    painter.text(pos2(rect.left() + 14.0, y), Align2::LEFT_TOP, "FLAT PATTERN", FontId::proportional(13.0), t.text);
    for (label, col) in [("Bend up", up_col), ("Bend down", down_col)] {
        y += 20.0;
        painter.line_segment([pos2(rect.left() + 14.0, y + 7.0), pos2(rect.left() + 38.0, y + 7.0)], Stroke::new(2.0, col));
        painter.text(pos2(rect.left() + 44.0, y), Align2::LEFT_TOP, label, FontId::proportional(12.0), t.text_dim);
    }
    if !pockets.is_empty() {
        y += 20.0;
        painter.extend(egui::Shape::dashed_line(
            &[pos2(rect.left() + 14.0, y + 7.0), pos2(rect.left() + 38.0, y + 7.0)],
            Stroke::new(1.6, t.accent),
            3.0,
            3.0,
        ));
        painter.text(pos2(rect.left() + 44.0, y), Align2::LEFT_TOP, "Pocket boundary (material remains)", FontId::proportional(12.0), t.text_dim);
    }
    true
}

/// The flat pattern's own 2D view: a top-down orthographic camera over the flat plane (x right,
/// y up, millimetres), so the shared navigation moves it like any other camera. The Design
/// camera is left alone while this has the viewport.
pub struct FlatView {
    pub cam: solidcraft_engine::render::Camera,
    /// The camera frames the pattern; cleared by Fit and Home, and whenever the dialog is closed.
    pub fitted: bool,
}

impl Default for FlatView {
    fn default() -> Self {
        let mut cam = solidcraft_engine::render::Camera::default();
        cam.set_view(solidcraft_engine::render::StandardView::Top);
        cam.fov = 0.0;
        cam.up = solidcraft_engine::render::UpAxis::Z;
        FlatView { cam, fitted: false }
    }
}

impl FlatView {
    /// Frame the pattern's bounds `lo..hi` in `area`, the part of `rect` the command panel leaves.
    fn fit(&mut self, rect: Rect, area: Rect, lo: (f64, f64), hi: (f64, f64)) {
        let scale = (f64::from(area.width()) / (hi.0 - lo.0)).min(f64::from(area.height()) / (hi.1 - lo.1));
        self.cam.target.x = (lo.0 + hi.0) * 0.5 - f64::from(area.center().x - rect.center().x) / scale;
        self.cam.target.y = (lo.1 + hi.1) * 0.5 + f64::from(area.center().y - rect.center().y) / scale;
        self.cam.target.z = 0.0;
        self.cam.distance = f64::from(rect.height()) / (2.0 * scale) / 0.4;
        self.fitted = true;
    }
}

/// The Create Flat Pattern dialog is open, so the viewport shows its 2D view.
pub(crate) fn flat_active(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::Sheet(Sm::FlatPattern { .. })))
}

/// Camera input for the flat pattern: the shared navigation without orbit, driving the flat view
/// instead of the Design camera. The result reports nothing under the pointer, so the model
/// beneath neither hovers, selects nor navigates.
pub(crate) fn navigate(app: &mut SolveApp, ui: &mut egui::Ui, rect: Rect, resp: &egui::Response) -> crate::navigation::CameraInput {
    use crate::navigation::{NavCaps, handle};
    let design = std::mem::replace(&mut app.cam, app.flat_view.cam);
    let mut input = handle(app, ui, rect, resp, NavCaps { orbit: false, pick: false });
    app.flat_view.cam = std::mem::replace(&mut app.cam, design);
    app.viewport.mouse = None;
    input.inside = false;
    input.hover = None;
    input
}

fn pocket_label(app: &SolveApp, pocket: &Value) -> Option<String> {
    let depth = pocket["depth_mm"].as_f64()?;
    if !depth.is_finite() || depth <= 0.0 {
        return None;
    }
    let side = match pocket["side"].as_str()? {
        "top" => "Top",
        "bottom" => "Bottom",
        "internal" => "Internal",
        _ => return None,
    };
    Some(format!("{side} pocket: {} deep", crate::prefs::show_mm(app, depth, 1)))
}

#[cfg(test)]
#[path = "dialogs_sheet_tests.rs"]
mod tests;
