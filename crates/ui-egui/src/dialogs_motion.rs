//! Assembly motion dialogs: Contact Sets, Motion Study (keyframes on a timeline with playback),
//! Exploded View (a scale slider) and Configurations (a table of rows with Activate). What they
//! show while open (a study's pose, an explosion) is display only: pending occurrence moves,
//! computed on a scratch copy of the design and put back when the dialog closes.

use std::collections::BTreeMap;

use egui::{Color32, Pos2, RichText, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_engine::doc::Mat;
use solidcraft_engine::doc::joints::JointKind;

use crate::SolveApp;
use crate::dialogs::{Dialog, Kind, combo, field, row_label};
use crate::selection::{BODIES, SelInput};
use crate::theme::Tokens;

#[path = "configuration_rules_ui.rs"]
mod configuration_rules_ui;
#[path = "dialogs_motion_plot.rs"]
pub mod plot;

/// One keyed value of a motion study: a joint's value (index) at steps.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// Index into the design's joints.
    pub joint: usize,
    pub index: usize,
    /// (step, value expression), in step order.
    pub points: Vec<(u32, String)>,
}

#[derive(Clone, Debug)]
pub enum Mo {
    ContactSet {
        name: String,
        /// The last collision check.
        check: Option<Value>,
    },
    MotionStudy {
        name: String,
        /// Editing this study (its id).
        study: Option<String>,
        steps: String,
        tracks: Vec<Track>,
        step: f64,
        playing: bool,
        /// Playback starts over at the end (else it stops there).
        looping: bool,
        /// Playback speed (index into `SPEEDS`).
        speed: usize,
        /// The step the shown pose is for (and its revision).
        shown: Option<(u64, u64)>,
        saved: Option<BTreeMap<u64, Mat>>,
        error: Option<String>,
        /// The chart and the collision check by the timeline.
        analysis: Box<plot::Analysis>,
    },
    Explode {
        name: String,
        scale: f64,
        shown: Option<(u64, u64)>,
        saved: Option<BTreeMap<u64, Mat>>,
        error: Option<String>,
    },
    Configurations {
        new_row: String,
        error: Option<String>,
        rule_editor: configuration_rules_ui::Editor,
    },
}

impl Mo {
    pub fn title(&self) -> &'static str {
        match self {
            Mo::ContactSet { .. } => "CONTACT SET",
            Mo::MotionStudy { study: Some(_), .. } => "EDIT MOTION STUDY",
            Mo::MotionStudy { .. } => "MOTION STUDY",
            Mo::Explode { .. } => "EXPLODED VIEW",
            Mo::Configurations { .. } => "CONFIGURATIONS",
        }
    }

    /// A table that needs the wide dialog.
    pub fn wide(&self) -> bool {
        matches!(self, Mo::Configurations { .. } | Mo::MotionStudy { .. })
    }
}

/// The dialog for a motion command.
pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let n = |prefix: &str, list: &str| -> String {
        let taken: Vec<String> = app
            .session
            .scratch()
            .execute(list, &json!({}))
            .ok()
            .and_then(|v| v.as_object().and_then(|o| o.values().find_map(|x| x.as_array().cloned())))
            .unwrap_or_default()
            .iter()
            .filter_map(|x| x.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        (1..).map(|k| format!("{prefix}{k}")).find(|c| !taken.contains(c)).unwrap_or_default()
    };
    let (mo, inputs) = match id {
        "contact.create" => (Mo::ContactSet { name: n("Contact Set", "contact.list"), check: None }, vec![SelInput::new("Components", BODIES, true)]),
        "motion.study" => (
            Mo::MotionStudy {
                name: n("Motion Study", "motion.list"),
                study: None,
                steps: "100".into(),
                tracks: Vec::new(),
                step: 0.0,
                playing: false,
                looping: true,
                speed: 2,
                shown: None,
                saved: None,
                error: None,
                analysis: Box::default(),
            },
            // A point to plot, and the parts to check for collisions (each optional).
            vec![
                SelInput::new("Point", crate::dialogs_assembly::SNAPS, false),
                SelInput::new("Components", BODIES, true),
                SelInput::new("Against", BODIES, true),
            ],
        ),
        "explode.create" => (Mo::Explode { name: n("Exploded View", "explode.list"), scale: 1.0, shown: None, saved: None, error: None }, vec![]),
        "config.table" | "config.configure" => (Mo::Configurations { new_row: String::new(), error: None, rule_editor: Default::default() }, vec![]),
        _ => return None,
    };
    Some((Kind::Motion(mo), inputs))
}

// ---- pure mappings (tested) ----

/// Playback speeds offered (times `STEPS_PER_S`).
pub const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
/// Steps played per second at 1×.
const STEPS_PER_S: f64 = 24.0;

/// The next playback step: `by` steps on; at the end it starts over (looping) or stops there.
/// Returns the step and whether it still plays.
pub fn advance(step: f64, by: f64, total: u32, looping: bool) -> (f64, bool) {
    let last = f64::from(total);
    let next = step + by.max(0.0);
    if next <= last {
        (next, true)
    } else if looping && last > 0.0 {
        (next % (last + 1.0), true)
    } else {
        (last, false)
    }
}

#[derive(Clone, Copy)]
enum Transport {
    Start,
    Back,
    Play,
    Stop,
    Forward,
    End,
}

/// A media button drawn in code: bars and triangles.
fn transport(ui: &mut egui::Ui, what: Transport, tip: &str) -> egui::Response {
    let t = Tokens::get();
    let (r, resp) = ui.allocate_exact_size(vec2(26.0, 22.0), Sense::click());
    let fill = if resp.hovered() { t.hover } else { t.field };
    ui.painter().rect(r, 3.0, fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let c = r.center();
    let ink = if matches!(what, Transport::Play | Transport::Stop) { t.accent } else { t.icon };
    let p = ui.painter();
    let tri = |dir: f32, x: f32| {
        egui::Shape::convex_polygon(vec![pos2(x - 4.0 * dir, c.y - 5.0), pos2(x + 4.0 * dir, c.y), pos2(x - 4.0 * dir, c.y + 5.0)], ink, Stroke::NONE)
    };
    let bar = |x: f32| egui::Shape::rect_filled(egui::Rect::from_center_size(pos2(x, c.y), vec2(2.0, 10.0)), 0.0, ink);
    match what {
        Transport::Play => drop(p.add(tri(1.0, c.x + 1.0))),
        Transport::Stop => drop(p.rect_filled(egui::Rect::from_center_size(c, vec2(9.0, 9.0)), 1.0, ink)),
        Transport::Back => {
            p.add(bar(c.x + 4.0));
            p.add(tri(-1.0, c.x - 1.0));
        }
        Transport::Forward => {
            p.add(bar(c.x - 4.0));
            p.add(tri(1.0, c.x + 1.0));
        }
        Transport::Start => {
            p.add(bar(c.x - 6.0));
            p.add(tri(-1.0, c.x - 0.5));
            p.add(tri(-1.0, c.x + 5.0));
        }
        Transport::End => {
            p.add(tri(1.0, c.x - 5.0));
            p.add(tri(1.0, c.x + 0.5));
            p.add(bar(c.x + 6.0));
        }
    }
    resp.on_hover_text(tip)
}

/// Study `key` as the Motion Study dialog edits it: (id, name, steps, keys as tracks with values
/// in degrees and mm). `key` is an id first when `id_first`, else a name first (as
/// `motion.study` reads a number and text). The dialog keys whole steps of its timeline, so a
/// study keyed between them or off it (made by a command) is refused rather than changed.
pub fn study_tracks(s: &Session, key: &str, id_first: bool) -> Result<(u64, String, String, Vec<Track>), String> {
    let studies = &s.doc.assembly.studies;
    let (by_id, by_name) = (studies.iter().find(|m| m.id.to_string() == key), studies.iter().find(|m| m.name == key));
    let m = if id_first { by_id.or(by_name) } else { by_name.or(by_id) }.ok_or_else(|| format!("no motion study `{key}`"))?;
    let last = f64::from(m.steps);
    if m.keys.iter().flat_map(|k| &k.points).any(|(st, _)| st.fract() != 0.0 || !(0.0..=last).contains(st)) {
        return Err(format!("{} has keys between whole steps or off its timeline, which this dialog can't keep: edit it with motion.study", m.name));
    }
    let tracks = m
        .keys
        .iter()
        .filter_map(|k| {
            let joint = s.doc.assembly.joints.iter().position(|j| j.id == k.joint)?;
            let angle = s.doc.assembly.joints.get(joint)?.kind.is_angle(k.index);
            let value = |v: f64| {
                let (x, unit) = if angle { (v.to_degrees(), "deg") } else { (v, "mm") };
                // Short where that keeps the value; else all its digits, so OK keeps the key.
                let short = crate::dialogs_assembly::num(x, unit);
                let kept = short.split(' ').next().and_then(|n| n.parse::<f64>().ok()).is_some_and(|y| (y - x).abs() <= 1e-9 * x.abs().max(1.0));
                if kept { short } else { format!("{x} {unit}") }
            };
            let points = k.points.iter().map(|(st, v)| (st.round().clamp(0.0, f64::from(m.steps)) as u32, value(*v))).collect();
            Some(Track { joint, index: k.index, points })
        })
        .collect();
    Ok((m.id, m.name.clone(), m.steps.to_string(), tracks))
}

/// What the Motion Study dialog opens on when the control channel's `ui.start` passes `params`:
/// `study` (an id or a name), opened to edit.
pub fn preset(s: &Session, k: &mut Mo, p: &Value) -> Result<(), String> {
    let Mo::MotionStudy { name, study, steps, tracks, shown, .. } = k else { return Err("not a Motion Study dialog".into()) };
    let o = p.as_object().ok_or("`params` must be an object")?;
    if let Some(key) = o.keys().find(|k| k.as_str() != "study") {
        return Err(format!("unknown param `{key}` (Motion Study takes study)"));
    }
    let (key, id_first) = match o.get("study") {
        Some(Value::Number(n)) => (n.to_string(), true),
        Some(Value::String(x)) => (x.clone(), false),
        Some(_) => return Err("`study` must be an id or a name".into()),
        None => return Ok(()),
    };
    let (id, n, st, t) = study_tracks(s, &key, id_first)?;
    (*study, *name, *steps, *tracks, *shown) = (Some(id.to_string()), n, st, t, None);
    Ok(())
}

/// The study command's parameters.
pub fn study_params(s: &Session, name: &str, study: Option<&str>, steps: &str, tracks: &[Track]) -> Result<Value, String> {
    let steps: u32 = steps.trim().parse().map_err(|_| "steps must be a whole number".to_string())?;
    let keys: Vec<Value> = tracks
        .iter()
        .filter(|t| !t.points.is_empty())
        .map(|t| {
            let j = s.doc.assembly.joints.get(t.joint).ok_or("no such joint")?;
            Ok(json!({"joint": j.id, "index": t.index, "points": t.points.iter().map(|(st, v)| json!([st, v])).collect::<Vec<_>>()}))
        })
        .collect::<Result<_, &str>>()?;
    if keys.is_empty() {
        return Err("add a key for a joint first".into());
    }
    let mut p = json!({"name": name, "steps": steps, "keys": keys});
    // The id as a number: a study may be named like another's id.
    if let Some(st) = study {
        p["study"] = st.parse::<u64>().map_or_else(|_| json!(st), |id| json!(id));
    }
    Ok(p)
}

/// The configurations table: columns and rows (name, value per column).
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<(String, Vec<String>)>,
    pub active: Option<String>,
}

pub fn table(v: &Value) -> Table {
    let columns: Vec<String> =
        v.get("columns").and_then(Value::as_array).into_iter().flatten().filter_map(|c| c.as_str().map(str::to_string)).collect();
    let cell = |x: Option<&Value>| match x {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => {
            if *b {
                "suppressed".into()
            } else {
                "on".into()
            }
        }
        Some(Value::Null) | None => String::new(),
        Some(o) => o.to_string(),
    };
    let rows = v
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let name = r.get("name")?.as_str()?.to_string();
            let vals = r.get("values");
            Some((name, columns.iter().map(|c| cell(vals.and_then(|m| m.get(c)))).collect()))
        })
        .collect();
    Table { columns, rows, active: v.get("active").and_then(Value::as_str).map(str::to_string) }
}

/// The unit a value of this kind is shown in.
fn unit_of(k: solidcraft_engine::doc::expr::Kind) -> Option<&'static str> {
    match k {
        solidcraft_engine::doc::expr::Kind::Length => Some("mm"),
        solidcraft_engine::doc::expr::Kind::Angle => Some("deg"),
        _ => None,
    }
}

/// A column key ("param:width", "suppress:Fillet1") as (kind, name).
pub fn column_parts(c: &str) -> (&str, &str) {
    c.split_once(':').unwrap_or(("param", c))
}

/// The value a table cell edit sends: suppress columns take booleans, parameters expressions.
pub fn cell_value(column: &str, text: &str) -> Value {
    match column_parts(column).0 {
        "suppress" => match text.trim().to_ascii_lowercase().as_str() {
            "suppressed" | "true" | "yes" | "1" | "off" => json!(true),
            "unsuppressed" | "false" | "no" | "0" | "on" => json!(false),
            _ => json!(text),
        },
        _ => json!(text),
    }
}

// ---- display ----

/// Show `moves` as pending occurrence moves (display only), keeping what was there before.
fn display(app: &mut SolveApp, saved: &mut Option<BTreeMap<u64, Mat>>, moves: BTreeMap<u64, Mat>) {
    if saved.is_none() {
        *saved = Some(app.session.pending_moves.clone());
    }
    app.session.pending_moves = moves;
    app.session.revision += 1;
}

fn restore(app: &mut SolveApp, saved: &mut Option<BTreeMap<u64, Mat>>) {
    if let Some(m) = saved.take() {
        app.session.pending_moves = m;
        app.session.revision += 1;
    }
}

/// The dialog closed (`applied`: by OK): what it showed goes away, unless OK made it the view.
pub fn closed(app: &mut SolveApp, d: &mut Dialog, applied: bool) {
    let Kind::Motion(k) = &mut d.kind else { return };
    match k {
        Mo::MotionStudy { saved, .. } => restore(app, saved),
        // OK shows the new view itself (explode.show).
        Mo::Explode { saved, .. } if !applied => restore(app, saved),
        _ => {}
    }
}

/// The occurrence placements of `after` that differ from `before` (as pending moves).
fn moved(before: &Session, after: &Session) -> BTreeMap<u64, Mat> {
    let mut out = before.pending_moves.clone();
    for o in &after.doc.occurrences {
        let was = before.doc.occurrences.iter().find(|b| b.id == o.id).map(|b| b.transform);
        if was != Some(o.transform) {
            out.insert(o.id, o.transform);
        }
    }
    out
}

/// The study's pose at `step`, computed on a scratch copy.
fn study_pose(app: &SolveApp, params: &Value, step: u32) -> Result<BTreeMap<u64, Mat>, String> {
    let mut sc = app.session.scratch();
    let id = scratch_study(&mut sc, params)?;
    sc.execute("motion.play", &json!({"study": id, "step": step})).map_err(|e| e.to_string())?;
    Ok(moved(&app.session, &sc))
}

/// Save the study being edited as an animated GIF, seen from the viewport's camera. Drawn on a
/// scratch copy, so the dialog's keys need not be saved first and the design is not changed.
fn export_animation(app: &mut SolveApp, params: Value, name: &str) {
    // The view as the viewport shows it: its camera, and its grid and axes on or off.
    let (camera, guides) = (app.cam, crate::control::window_guides(app));
    let file = format!("{}.gif", Some(name.trim()).filter(|n| !n.is_empty()).unwrap_or("Motion Study"));
    app.pick_save(&file, &["gif"], move |app, path| {
        let Some(mut path) = path else { return };
        // A name typed without the extension is still a GIF (not a folder of frames).
        if !path.to_ascii_lowercase().ends_with(".gif") {
            path.push_str(".gif");
        }
        let mut sc = app.session.scratch();
        // Cut as the viewport is (a copy leaves the section out).
        sc.section = app.session.section;
        let made = sc.execute("motion.study", &params).and_then(|r| {
            sc.execute(
                "motion.export_animation",
                &json!({"study": r["study"], "path": path, "camera": camera, "grid": guides.grid, "axes": guides.axes}),
            )
        });
        match made {
            Ok(_) => app.set_status(crate::i18n::format("ui.saved_to", &[("path", &path)]), false),
            Err(e) => app.set_status(e.to_string(), true),
        }
    });
}

/// Makes (or edits) the study on a scratch copy, as OK would: its id there (a name may be
/// another study's too).
pub fn scratch_study(sc: &mut Session, params: &Value) -> Result<Value, String> {
    Ok(sc.execute("motion.study", params).map_err(|e| e.to_string())?["study"].clone())
}

/// The explosion at `scale`, computed on a scratch copy.
fn explode_pose(app: &SolveApp, scale: f64) -> Result<BTreeMap<u64, Mat>, String> {
    let mut sc = app.session.scratch();
    sc.execute("explode.create", &json!({"name": "__preview", "scale": scale})).map_err(|e| e.to_string())?;
    sc.execute("explode.show", &json!({ "view": "__preview" })).map_err(|e| e.to_string())?;
    Ok(sc.pending_moves.clone())
}

// ---- rows ----

fn joints_moving(app: &SolveApp) -> Vec<(usize, String, JointKind)> {
    app.session.doc.assembly.joints.iter().enumerate().filter(|(_, j)| j.kind != JointKind::Rigid).map(|(i, j)| (i, j.name.clone(), j.kind)).collect()
}

/// The dialog's rows; true when Enter was pressed in a value.
pub fn rows(app: &mut SolveApp, ui: &mut egui::Ui, k: &mut Mo, inputs: &mut [SelInput]) -> bool {
    let t = Tokens::get();
    let mut enter = false;
    match k {
        Mo::ContactSet { name, check } => {
            row_label(ui, "Name");
            ui.text_edit_singleline(name);
            ui.end_row();
            let list = app.session.execute("contact.list", &json!({})).unwrap_or(Value::Null);
            row_label(ui, "Contact");
            let mut on = list.get("enabled").and_then(Value::as_bool).unwrap_or(false);
            if ui.checkbox(&mut on, "enabled").changed() {
                let _ = app.run("contact.enable_sets", json!({ "enabled": on }));
            }
            ui.end_row();
            for s in list.get("sets").and_then(Value::as_array).cloned().unwrap_or_default() {
                let n = s.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let off = s.get("suppressed").and_then(Value::as_bool).unwrap_or(false);
                row_label(ui, &n);
                ui.horizontal(|ui| {
                    let count = s.get("occurrences").and_then(Value::as_array).map_or(0, Vec::len);
                    ui.label(RichText::new(format!("{count} components")).color(t.text_dim));
                    if ui.small_button(if off { "Unsuppress" } else { "Suppress" }).clicked() {
                        let _ = app.run("contact.edit", json!({"set": n, "suppressed": !off}));
                    }
                    if ui.small_button("×").on_hover_text(crate::i18n::tr_label("Delete the set")).clicked() {
                        let _ = app.run("contact.edit", json!({"set": n, "delete": true}));
                    }
                });
                ui.end_row();
            }
            row_label(ui, "");
            if ui.button(crate::i18n::tr_label("Check collisions")).clicked() {
                *check = Some(app.session.execute("contact.check", &json!({})).unwrap_or_else(|e| json!({"error": e.to_string()})));
            }
            ui.end_row();
            if let Some(c) = check {
                let hits = c.get("collisions").and_then(Value::as_array).cloned().unwrap_or_default();
                row_label(ui, "Collisions");
                if let Some(e) = c.get("error").and_then(Value::as_str) {
                    ui.label(RichText::new(e).color(t.error));
                } else if hits.is_empty() {
                    ui.label(RichText::new("none").color(t.text_dim));
                } else {
                    ui.vertical(|ui| {
                        for h in hits {
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}  {:.3} mm",
                                    name_of(&h["a"]),
                                    name_of(&h["b"]),
                                    h["depth_mm"].as_f64().unwrap_or(0.0)
                                ))
                                .color(t.error),
                            );
                        }
                    });
                }
                ui.end_row();
            }
        }
        Mo::MotionStudy { .. } => enter = motion_study_rows(app, ui, k, inputs, t),
        Mo::Explode { name, scale, shown, saved, error } => {
            row_label(ui, "Name");
            ui.text_edit_singleline(name);
            ui.end_row();
            row_label(ui, "Explode");
            ui.spacing_mut().slider_width = 160.0;
            ui.add(egui::Slider::new(scale, 0.0..=2.0).fixed_decimals(2));
            ui.end_row();
            let key = ((*scale * 1000.0).round() as u64, 0);
            if *shown != Some(key) {
                match explode_pose(app, *scale) {
                    Ok(m) => {
                        display(app, saved, m);
                        *error = None;
                    }
                    Err(e) => *error = Some(e),
                }
                *shown = Some(key);
            }
            if let Some(e) = error {
                row_label(ui, "");
                ui.label(RichText::new(e.as_str()).color(t.error));
                ui.end_row();
            }
        }
        Mo::Configurations { new_row, error, rule_editor } => configurations_rows(app, ui, new_row, error, rule_editor, t),
    }
    let _ = inputs;
    enter
}

/// The Motion Study rows; true when Enter was pressed in a value.
fn motion_study_rows(app: &mut SolveApp, ui: &mut egui::Ui, k: &mut Mo, inputs: &mut [SelInput], t: Tokens) -> bool {
    let Mo::MotionStudy { name, study, steps, tracks, step, playing, looping, speed, shown, saved, error, analysis } = k else { return false };
    let mut enter = false;
    // An existing study opens to edit (and to chart); New makes another from these keys.
    // By id: two studies may share a name.
    let studies: Vec<(String, String)> = app.session.doc.assembly.studies.iter().map(|m| (m.id.to_string(), m.name.clone())).collect();
    if !studies.is_empty() {
        let labels: Vec<&str> = std::iter::once("New study").chain(studies.iter().map(|(_, n)| n.as_str())).collect();
        let was = study.as_ref().and_then(|id| studies.iter().position(|(x, _)| x == id)).map_or(0, |i| i + 1);
        let mut pick = was;
        row_label(ui, "Study");
        combo(ui, "ms_study", &labels, &mut pick);
        ui.end_row();
        if pick != was {
            match pick.checked_sub(1).and_then(|i| studies.get(i)).map(|(id, _)| study_tracks(&app.session, id, true)) {
                Some(Ok((id, n, st, t))) => (*study, *name, *steps, *tracks, *shown, *playing) = (Some(id.to_string()), n, st, t, None, false),
                Some(Err(e)) => app.set_status(e, true),
                None => *study = None,
            }
        }
    }
    row_label(ui, "Name");
    ui.text_edit_singleline(name);
    ui.end_row();
    row_label(ui, "Steps");
    enter |= field(ui, steps);
    ui.end_row();
    let total = steps.trim().parse::<u32>().unwrap_or(100).clamp(1, 100_000);
    let moving = joints_moving(app);
    if moving.is_empty() {
        row_label(ui, "");
        ui.label(RichText::new(crate::i18n::tr_label("no joint that moves yet")).color(t.text_dim));
        ui.end_row();
        return enter;
    }
    // Playback: to start, step back, play/stop, step on, to end; loop and speed.
    row_label(ui, "Playback");
    ui.horizontal(|ui| {
        let last = f64::from(total);
        if transport(ui, Transport::Start, "To the start").clicked() {
            *step = 0.0;
            *playing = false;
        }
        if transport(ui, Transport::Back, "Step back").clicked() {
            *step = (step.round() - 1.0).max(0.0);
            *playing = false;
        }
        let (what, tip) = if *playing { (Transport::Stop, "Stop") } else { (Transport::Play, "Play") };
        if transport(ui, what, tip).clicked() {
            // Play from the start again when at the end.
            if !*playing && step.round() >= last {
                *step = 0.0;
            }
            *playing = !*playing;
        }
        if transport(ui, Transport::Forward, "Step on").clicked() {
            *step = (step.round() + 1.0).min(last);
            *playing = false;
        }
        if transport(ui, Transport::End, "To the end").clicked() {
            *step = last;
            *playing = false;
        }
        ui.add_space(6.0);
        ui.checkbox(looping, crate::i18n::tr_label("Loop"));
        let names: Vec<String> = SPEEDS.iter().map(|x| format!("{x}×")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        combo(ui, "ms_speed", &refs, speed);
    });
    ui.end_row();
    row_label(ui, "Step");
    ui.horizontal(|ui| {
        ui.spacing_mut().slider_width = 260.0;
        if ui.add(egui::Slider::new(step, 0.0..=f64::from(total)).integer().show_value(false)).dragged() {
            *playing = false;
        }
        ui.label(RichText::new(format!("{} / {total}", step.round() as u32)).color(t.text_dim));
    });
    ui.end_row();
    if *playing {
        let dt = f64::from(ui.input(|i| i.stable_dt).min(0.1));
        let fps = STEPS_PER_S * SPEEDS.get(*speed).copied().unwrap_or(1.0);
        (*step, *playing) = advance(*step, dt * fps, total, *looping);
        ui.ctx().request_repaint();
    }
    // The chart and the collision check, by the timeline.
    let study_now = study_params(&app.session, name, study.as_deref(), steps, tracks);
    plot::rows(app, ui, analysis, &study_now, step, playing, total, inputs);
    // Tracks: a row per keyed joint value, its keys on a strip under the steps.
    let now = step.round() as u32;
    let mut remove: Option<usize> = None;
    for (ti, tr) in tracks.iter_mut().enumerate() {
        let labels: Vec<String> = moving.iter().map(|(_, n, _)| n.clone()).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut pos = moving.iter().position(|(i, _, _)| *i == tr.joint).unwrap_or(0);
        row_label(ui, &format!("Track {}", ti + 1));
        ui.horizontal(|ui| {
            combo(ui, &format!("ms_joint{ti}"), &refs, &mut pos);
            if ui.small_button("×").on_hover_text(crate::i18n::tr_label("Remove the track")).clicked() {
                remove = Some(ti);
            }
        });
        ui.end_row();
        if let Some((ji, _, kind)) = moving.get(pos) {
            tr.joint = *ji;
            let dofs = kind.dofs();
            if dofs.len() > 1 {
                row_label(ui, "  Of");
                let caps: Vec<&str> = dofs.to_vec();
                combo(ui, &format!("ms_idx{ti}"), &caps, &mut tr.index);
                ui.end_row();
            } else {
                tr.index = 0;
            }
        }
        row_label(ui, "  Keys");
        key_strip(ui, tr, total, now, &t);
        ui.end_row();
        // The key at this step, if any: its value.
        if let Some(p) = tr.points.iter_mut().find(|(s, _)| *s == now) {
            // The key's value (Enter commits it; OK makes the study).
            row_label(ui, "Value");
            let _ = field(ui, &mut p.1);
            ui.end_row();
        }
        row_label(ui, "");
        ui.horizontal(|ui| {
            if ui.small_button("Key here").on_hover_text("Add a key at this step with the joint's value now").clicked()
                && !tr.points.iter().any(|(s, _)| *s == now)
            {
                let unit =
                    app.session.doc.assembly.joints.get(tr.joint).map(|j| if j.kind.is_angle(tr.index) { "deg" } else { "mm" }).unwrap_or("mm");
                tr.points.push((now, format!("0 {unit}")));
                tr.points.sort_by_key(|p| p.0);
            }
            if ui.small_button("Delete key").clicked() {
                tr.points.retain(|(s, _)| *s != now);
            }
        });
        ui.end_row();
    }
    if let Some(i) = remove {
        tracks.remove(i);
    }
    row_label(ui, "");
    let mut export = false;
    ui.horizontal(|ui| {
        if ui.button(crate::i18n::tr_label("Add Key")).on_hover_text(crate::i18n::tr_label("Key a joint's value at this step")).clicked()
            && let Some((ji, _, kind)) = moving.first()
        {
            let unit = if kind.is_angle(0) { "deg" } else { "mm" };
            tracks.push(Track { joint: *ji, index: 0, points: vec![(now, format!("0 {unit}"))] });
        }
        export = ui
            .button(crate::i18n::tr_label("Export Animation…"))
            .on_hover_text(crate::i18n::tr_label("Save the study as an animated GIF, seen as the viewport shows it"))
            .clicked();
    });
    ui.end_row();
    // Show the pose at this step (again when the step or the keys change).
    let params = study_params(&app.session, name, study.as_deref(), steps, tracks);
    if export {
        match &params {
            Ok(p) => export_animation(app, p.clone(), name),
            Err(e) => *error = Some(e.clone()),
        }
    }
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{params:?}").hash(&mut h);
        h.finish()
    };
    if *shown != Some((u64::from(now), key)) {
        match params.and_then(|p| study_pose(app, &p, now)) {
            Ok(m) => {
                display(app, saved, m);
                *error = None;
            }
            Err(e) => *error = Some(e),
        }
        *shown = Some((u64::from(now), key));
    }
    if let Some(e) = error {
        row_label(ui, "");
        ui.label(RichText::new(e.as_str()).color(t.text_dim));
        ui.end_row();
    }
    enter
}

/// The Configurations table, its rules and errors.
fn configurations_rows(
    app: &mut SolveApp,
    ui: &mut egui::Ui,
    new_row: &mut String,
    error: &mut Option<String>,
    rule_editor: &mut configuration_rules_ui::Editor,
    t: Tokens,
) {
    // Physical materials for the configuration table's Library: the library's, then the design's own.
    let material_names = app.session.doc.material_names();
    let revision = app.session.revision;
    let v = rule_editor.table.as_ref().filter(|(r, _)| *r == revision).map(|(_, v)| v.clone()).unwrap_or_else(|| {
        let value = app.session.execute("config.table", &json!({})).unwrap_or(Value::Null);
        rule_editor.table = Some((revision, value.clone()));
        value
    });
    let mut tb = table(&v);
    // Bare numbers show with their parameter's unit ("40" is 40 mm), as Fusion shows them.
    let kinds = app.session.doc.all_param_exprs();
    for (_, vals) in &mut tb.rows {
        for (c, val) in tb.columns.iter().zip(vals.iter_mut()) {
            let (kind, n) = column_parts(c);
            if kind == "param"
                && val.trim().parse::<f64>().is_ok()
                && let Some(unit) = app.session.doc.param(n).map(|parameter| parameter.unit.clone()).or_else(|| {
                    kinds.iter().find(|x| x.0 == n).and_then(|x| {
                        unit_of(x.2).map(|unit| {
                            if x.2 == solidcraft_engine::doc::expr::Kind::Length { app.session.doc.units.clone() } else { unit.to_string() }
                        })
                    })
                })
            {
                *val = format!("{} {unit}", val.trim());
            }
        }
    }
    let run = |app: &mut SolveApp, error: &mut Option<String>, id: &str, p: Value| {
        *error = app.run(id, p).err();
    };
    egui::ScrollArea::horizontal().id_salt("configuration_columns").auto_shrink([false, true]).show(ui, |ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        egui::Grid::new("configuration_table").num_columns(2).min_col_width(210.0).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
            // Header.
            ui.label(RichText::new(crate::i18n::tr_label("Configuration")).strong());
            ui.horizontal(|ui| {
                for c in &tb.columns {
                    ui.allocate_ui_with_layout(vec2(190.0, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let (kind, n) = column_parts(c);
                        let label = match kind {
                            "material" => format!("Material: {n}"),
                            "appearance" => format!("Appearance: {n}"),
                            "suppress" => format!("Suppress: {n}"),
                            _ => n.to_string(),
                        };
                        let label = v["targets"]
                            .as_array()
                            .and_then(|targets| targets.iter().find(|target| target["key"].as_str() == Some(c)))
                            .and_then(|target| target["label"].as_str())
                            .filter(|_| matches!(kind, "component_appearance" | "face_appearance"))
                            .map(str::to_string)
                            .unwrap_or(label);
                        ui.add_sized(vec2(156.0, 22.0), egui::Label::new(RichText::new(label).strong()).truncate()).on_hover_text(c);
                        if ui.small_button("×").on_hover_text(crate::i18n::tr_label("Remove the column")).clicked() {
                            let key = if kind == "suppress" { "feature" } else { kind };
                            run(app, error, "config.column", json!({ key: if kind == "face_appearance" {c.as_str()} else {n}, "delete": true }));
                        }
                    });
                }
            });
            ui.end_row();
            for (name, vals) in &tb.rows {
                let active = tb.active.as_deref() == Some(name.as_str());
                ui.horizontal(|ui| {
                    ui.add_sized(
                        vec2(100.0, 22.0),
                        egui::Label::new(if active { RichText::new(name).strong() } else { RichText::new(name) }).truncate(),
                    )
                    .on_hover_text(name);
                    if active {
                        ui.label(RichText::new(crate::i18n::tr_label("Active")).color(t.accent));
                        if v.get("matches").and_then(Value::as_bool) == Some(false) && ui.button("Apply").clicked() {
                            run(app, error, "config.activate", json!({ "row": name }));
                        }
                    } else if ui.button(crate::i18n::tr_label("Activate")).clicked() {
                        run(app, error, "config.activate", json!({ "row": name }));
                    }
                });
                ui.horizontal(|ui| {
                    for (c, val) in tb.columns.iter().zip(vals) {
                        ui.allocate_ui_with_layout(vec2(190.0, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let (kind, _) = column_parts(c);
                            let appearance = matches!(kind, "appearance" | "component_appearance" | "face_appearance");
                            let shown = if appearance {
                                solidcraft_engine::doc::config::appearance_cell(val)
                                    .ok()
                                    .flatten()
                                    .filter(|look| solidcraft_engine::doc::appearance::library(&look.name).as_ref() == Some(look))
                                    .map(|look| look.name)
                                    .unwrap_or_else(|| val.clone())
                            } else {
                                val.clone()
                            };
                            let style = kind == "material" || appearance;
                            if style {
                                let mut choice = None;
                                ui.menu_button("Library", |ui| {
                                    if ui
                                        .button(if kind == "appearance" {
                                            "Follow material"
                                        } else if appearance {
                                            "Follow inherited appearance"
                                        } else {
                                            "Default material"
                                        })
                                        .clicked()
                                    {
                                        choice = Some("Default".to_string());
                                        ui.close();
                                    }
                                    if kind == "material" {
                                        for name in material_names.iter().filter(|n| *n != "Default") {
                                            if ui.button(name).clicked() {
                                                choice = Some(name.to_string());
                                                ui.close();
                                            }
                                        }
                                    } else {
                                        for (name, _, _) in solidcraft_engine::doc::appearance::LIBRARY {
                                            if ui.button(name).clicked() {
                                                choice = Some(name.to_string());
                                                ui.close();
                                            }
                                        }
                                    }
                                });
                                if let Some(choice) = choice {
                                    run(app, error, "config.row", json!({"name": name, "values": { c: choice }}));
                                }
                            }
                            let id = (name.clone(), c.clone());
                            let mut text = rule_editor.drafts.get(&id).cloned().unwrap_or_else(|| shown.clone());
                            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(if style { 106.0 } else { 164.0 }));
                            if r.changed() {
                                rule_editor.drafts.insert(id.clone(), text.clone());
                            }
                            if r.lost_focus() {
                                rule_editor.drafts.remove(&id);
                                if text != shown {
                                    run(app, error, "config.row", json!({"name": name, "values": { c: cell_value(c, &text) }}));
                                }
                            }
                        });
                    }
                    if ui.small_button("×").on_hover_text(crate::i18n::tr_label("Delete the row")).clicked() {
                        run(app, error, "config.row", json!({"name": name, "delete": true}));
                    }
                });
                ui.end_row();
            }
        });
    });
    // New row (a copy of the active one) and new column.
    ui.horizontal(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.add(egui::TextEdit::singleline(new_row).hint_text(crate::i18n::tr_label("new configuration")).desired_width(160.0));
        if ui.button(crate::i18n::tr_label("Add Row")).on_hover_text(crate::i18n::tr_label("A new configuration, a copy of the active one")).clicked()
        {
            let name = if new_row.trim().is_empty() {
                (tb.rows.len() + 1..).map(|k| format!("Configuration{k}")).find(|n| !tb.rows.iter().any(|r| &r.0 == n)).unwrap_or_default()
            } else {
                new_row.trim().to_string()
            };
            let mut p = json!({ "name": name });
            if let Some(a) = &tb.active {
                p["from"] = json!(a);
            }
            run(app, error, "config.row", p);
            new_row.clear();
        }
        // A column: a parameter's value or a feature's suppression.
        let mut options: Vec<(String, Value)> = Vec::new();
        for p in app.session.doc.all_param_exprs() {
            if !tb.columns.contains(&format!("param:{}", p.0)) {
                options.push((format!("{} (parameter)", p.0), json!({ "param": p.0 })));
            }
        }
        for f in &app.session.doc.features {
            if !tb.columns.contains(&format!("suppress:{}", f.name)) {
                options.push((format!("Suppress {}", f.name), json!({ "feature": f.name })));
            }
        }
        for body in &app.session.model.state().bodies {
            for kind in ["material", "appearance"] {
                if !tb.columns.contains(&format!("{kind}:{}", body.name)) {
                    options.push((format!("{} ({kind})", body.name), json!({kind: body.name})));
                }
            }
        }
        for component in &app.session.doc.components {
            if component.id != 0 && !tb.columns.contains(&format!("component_appearance:{}", component.id)) {
                options.push((format!("{} (component appearance)", component.name), json!({"component_appearance":component.id})));
            }
        }
        for selected in &app.session.selection {
            if let solidcraft_engine::Sel::Face { body, point, .. } = selected {
                options.push((format!("Selected face on {body} (shared appearance)"), json!({"face_appearance":{"body":body,"point":point}})));
            }
        }
        let mut pick: Option<Value> = None;
        ui.menu_button("Add Column", |ui| {
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for (label, p) in &options {
                    if ui.button(label.as_str()).clicked() {
                        pick = Some(p.clone());
                        ui.close();
                    }
                }
                ui.menu_button("Face Appearance", |ui| {
                    ui.label(crate::i18n::tr_label("Semantic faces share their appearance across instances."));
                    let state = app.session.model.state();
                    for body in &state.bodies {
                        if body.body.is_mesh() {
                            continue;
                        }
                        ui.menu_button(format!("Faces on {}", body.name), |ui| {
                            if let Ok(faces) = body.body.faces((body.body.size() * 2e-3).max(1e-3)) {
                                let mut seen = std::collections::BTreeSet::new();
                                let mesh = body.mesh();
                                let mut points = std::collections::BTreeMap::new();
                                for (triangle, index) in mesh.triangles.iter().zip(&mesh.tri_face) {
                                    if let std::collections::btree_map::Entry::Vacant(entry) = points.entry(*index as usize)
                                        && let Some([a, b, c]) = mesh.tri(triangle)
                                    {
                                        entry.insert((a + b + c) * (1. / 3.));
                                    }
                                }
                                for face in faces {
                                    // A curved face centroid can lie on another surface.
                                    // Pick an interior triangle point on this actual face.
                                    let Some(point) = points.get(&face.index).copied() else { continue };
                                    let Ok(target) = app.session.doc.configuration_face_target(body, face.index, point) else { continue };
                                    let supported = if target.certificate.is_some() {
                                        // Capture read this exact face's current source certificate.
                                        // Enumerate its pieces once in the command, not per menu face.
                                        app.session.doc.certify_config_face(&target).is_ok()
                                    } else {
                                        app.session.doc.configuration_face_indices(body, &target).is_ok()
                                    };
                                    if !supported || !seen.insert(target.name.clone()) {
                                        continue;
                                    }
                                    let Some((_, role)) = target.name.split_once(':') else { continue };
                                    if ui.button(role).clicked() {
                                        pick = Some(json!({"face_appearance":{"target":target}}));
                                        ui.close();
                                    }
                                }
                            }
                        });
                    }
                });
            });
        });
        if let Some(p) = pick {
            // The first column starts the table (with a Default row of the current values).
            let (id, p) = if tb.columns.is_empty() {
                (
                    "config.configure",
                    if let Some(n) = p.get("param") {
                        json!({ "params": [n] })
                    } else if let Some(n) = p.get("feature") {
                        json!({ "features": [n] })
                    } else if let Some(n) = p.get("material") {
                        json!({ "materials": [n] })
                    } else if let Some(n) = p.get("component_appearance") {
                        json!({ "component_appearances": [n] })
                    } else if let Some(n) = p.get("face_appearance") {
                        json!({ "face_appearances": [n] })
                    } else {
                        json!({ "appearances": [p.get("appearance")] })
                    },
                )
            } else {
                ("config.column", p)
            };
            run(app, error, id, p);
        }
    });
    ui.end_row();
    configuration_rules_ui::show(ui, app, error, rule_editor);
    if let Some(styles) = v["style_status"].as_array() {
        for style in styles.iter().filter(|style| style["dormant"].as_bool() == Some(true)) {
            ui.add(
                egui::Label::new(format!("{}: appearance dormant while its feature is suppressed", style["target"].as_str().unwrap_or("Face")))
                    .wrap(),
            );
        }
    }
    if let Some(message) = v["style_error"].as_str() {
        ui.add(egui::Label::new(RichText::new(message).color(t.error)).wrap());
    }
    if let Some(e) = error {
        ui.label("");
        ui.label(RichText::new(e.as_str()).color(t.error));
        ui.end_row();
    }
}

/// A JSON name or id as text.
fn name_of(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())
}

/// A track's keys as diamonds over the steps, with the playhead.
fn key_strip(ui: &mut egui::Ui, tr: &Track, total: u32, now: u32, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(240.0, 16.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 3.0, t.field);
    let x = |s: u32| r.left() + 6.0 + (r.width() - 12.0) * s as f32 / total.max(1) as f32;
    // Keys joined by a line (the motion between them).
    let pts: Vec<Pos2> = tr.points.iter().map(|(s, _)| pos2(x(*s), r.center().y)).collect();
    if pts.len() > 1 {
        p.add(egui::Shape::line(pts.clone(), Stroke::new(1.5, t.accent.gamma_multiply(0.6))));
    }
    for q in pts {
        p.add(egui::Shape::convex_polygon(
            vec![q + vec2(0.0, -5.0), q + vec2(5.0, 0.0), q + vec2(0.0, 5.0), q + vec2(-5.0, 0.0)],
            t.accent,
            Stroke::new(1.0, Color32::WHITE),
        ));
    }
    let px = x(now);
    p.line_segment([pos2(px, r.top()), pos2(px, r.bottom())], Stroke::new(1.5, t.warning));
}

/// The commands OK runs.
pub fn commands(app: &SolveApp, k: &Mo, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    Ok(match k {
        Mo::ContactSet { name, .. } => {
            let mut occs: Vec<u64> = Vec::new();
            for x in inputs.first().map(|i| i.items.as_slice()).unwrap_or(&[]) {
                if let solidcraft_engine::Sel::Body { name } = x
                    && let Some(b) = app.session.world_state().body(name)
                {
                    let comp = app.session.doc.model_body_component(b);
                    if let Some(o) = app.session.doc.occurrence_of(comp).map(|o| o.id)
                        && !occs.contains(&o)
                    {
                        occs.push(o);
                    }
                }
            }
            if occs.is_empty() {
                // Only managing the sets: nothing to add.
                return Ok(Vec::new());
            }
            vec![("contact.create".into(), json!({"name": name, "occurrences": occs}))]
        }
        Mo::MotionStudy { name, study, steps, tracks, .. } => {
            vec![("motion.study".into(), study_params(&app.session, name, study.as_deref(), steps, tracks)?)]
        }
        Mo::Explode { name, scale, .. } => {
            vec![("explode.create".into(), json!({"name": name, "scale": scale})), ("explode.show".into(), json!({ "view": name }))]
        }
        Mo::Configurations { .. } => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Playback runs on, starts over at the end when looping, else stops on the last step.
    #[test]
    fn playback_advances_loops_and_stops() {
        assert_eq!(advance(3.0, 2.0, 10, true), (5.0, true));
        assert_eq!(advance(9.5, 2.0, 10, true), (0.5, true));
        assert_eq!(advance(9.5, 2.0, 10, false), (10.0, false));
        assert_eq!(advance(4.0, -3.0, 10, false), (4.0, true));
    }

    /// What a dialog shows goes away when it closes; an exploded view applied with OK stays.
    #[test]
    fn shown_poses_are_put_back() {
        let mut app = SolveApp::new(Session::default(), crate::Services::default());
        app.session.pending_moves.insert(9, solidcraft_engine::doc::IDENTITY);
        let before = app.session.pending_moves.clone();
        let mut m = solidcraft_engine::doc::IDENTITY;
        m[3] = [5.0, 0.0, 0.0, 1.0];
        let shown: BTreeMap<u64, Mat> = [(3, m)].into_iter().collect();
        for (kind, applied, kept) in [("study", true, false), ("explode", false, false), ("explode", true, true)] {
            let mut saved = None;
            display(&mut app, &mut saved, shown.clone());
            assert_eq!(app.session.pending_moves, shown);
            let mo = if kind == "study" {
                Mo::MotionStudy {
                    name: "S".into(),
                    study: None,
                    steps: "10".into(),
                    tracks: vec![],
                    step: 0.0,
                    playing: false,
                    looping: true,
                    speed: 2,
                    shown: None,
                    saved,
                    error: None,
                    analysis: Box::default(),
                }
            } else {
                Mo::Explode { name: "E".into(), scale: 1.0, shown: None, saved, error: None }
            };
            let mut d = Dialog::new(Kind::Motion(mo), vec![]);
            closed(&mut app, &mut d, applied);
            assert_eq!(app.session.pending_moves == shown, kept, "{kind} {applied}");
            app.session.pending_moves = before.clone();
        }
    }

    #[test]
    fn configuration_table_and_cells() {
        let v = json!({"columns": ["param:width", "suppress:Fillet1"], "rows": [
            {"name": "Default", "values": {"param:width": "80 mm", "suppress:Fillet1": false}},
            {"name": "Long", "values": {"param:width": "120 mm", "suppress:Fillet1": true}},
        ], "active": "Long"});
        let t = table(&v);
        assert_eq!(t.columns.len(), 2);
        assert_eq!(t.rows[1], ("Long".to_string(), vec!["120 mm".to_string(), "suppressed".to_string()]));
        assert_eq!(t.active.as_deref(), Some("Long"));
        assert_eq!(column_parts("suppress:Fillet1"), ("suppress", "Fillet1"));
        assert_eq!(cell_value("suppress:Fillet1", "Suppressed"), json!(true));
        assert_eq!(cell_value("suppress:Fillet1", "on"), json!(false));
        assert_eq!(cell_value("param:width", "90 mm"), json!("90 mm"));
    }

    fn click_rule_button(h: &mut crate::scenario::Harness, name: &str) {
        // The table header also contains this name. Its TextRun may be emitted
        // after the popup, so select the actual choice rather than matching text.
        let bounds = h
            .output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .rev()
            .find_map(|(_, n)| {
                (n.role() == egui::accesskit::Role::Button && !n.is_disabled() && n.label() == Some(name)).then(|| n.bounds()).flatten()
            })
            .expect("rule button");
        h.step(&json!({"click": [(bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0]})).unwrap();
    }

    fn select_rule_parameter(h: &mut crate::scenario::Harness, name: &str) {
        click_rule_button(h, name);
        assert!(h.widget("Choose…").is_none(), "parameter choice should close and update the dropdown");
    }

    #[test]
    fn configuration_compound_condition_and_rule_order_are_editable_in_dialog() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.resize", json!({"width":1400,"height":1100}));
        h.run(&json!([
            {"call":"engine.execute","params":{"command":"parameters.change","params":{"name":"length","expression":"40 mm"}}},
            {"call":"engine.execute","params":{"command":"config.configure","params":{"params":["length"]}}},
            {"start":"config.table"},{"widget":"Add or edit rule"},
            {"widget":"Always"},{"widget":"All"},{"widget":"Always"},{"widget":"Parameter"},
            {"widget":"Choose…"}
        ]))
        .unwrap();
        select_rule_parameter(&mut h, "length");
        h.step(&json!({"widget":"Save Rule"})).unwrap();
        assert!(
            matches!(&h.app.session.doc.configs.rules[0].condition,solidcraft_engine::doc::config::RuleCondition::All{conditions} if matches!(&conditions[0],solidcraft_engine::doc::config::RuleCondition::Parameter{..}))
        );
        h.run(&json!([
            {"call":"engine.execute","params":{"command":"config.rule","params":{"rule":{"id":"last","name":"Last drive","condition":{"type":"always"},"effect":{"type":"drive_parameter","parameter":"length","expression":"20 mm"}}}}},
            {"widget":"Preview Row"},{"widget":"Preview: Default"}
        ])).unwrap();
        assert!(h.widget("length = 20 mm").is_some());
        assert_eq!(h.app.session.doc.param("length").unwrap().expr, "40 mm");
        click_rule_button(&mut h, "↑");
        h.step(&json!({"widget":"Preview Row"})).unwrap();
        assert_eq!(h.app.session.doc.configs.rules[0].id, "last");
        assert!(h.widget("length = 10 mm").is_some());
        assert_eq!(h.app.session.doc.param("length").unwrap().expr, "40 mm");
    }

    #[test]
    fn configuration_rule_editor_creates_previews_edits_and_applies_driven_value() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.resize", json!({"width":1400,"height":1000}));
        h.run(&json!([
            {"call":"engine.execute","params":{"command":"parameters.change","params":{"name":"length","expression":"40 mm"}}},
            {"call":"engine.execute","params":{"command":"solid.box","params":{"length":"length","width":10,"height":10}}},
            {"call":"engine.execute","params":{"command":"config.configure","params":{"params":["length"]}}},
            {"start":"config.table"},
            {"widget":"Add or edit rule"},
            {"widget":"Choose…"}
        ]))
        .unwrap();
        select_rule_parameter(&mut h, "length");
        h.run(&json!([
            {"widget":"Save Rule"}
        ]))
        .unwrap();
        assert_eq!(h.app.session.doc.configs.rules.len(), 1);
        assert_eq!(h.app.session.doc.param("length").unwrap().expr, "40 mm");
        let before = h.app.session.doc.clone();
        h.run(&json!([{"widget":"Preview Row"}])).unwrap();
        assert_eq!(h.app.session.doc, before);
        h.run(&json!([{"widget":"Apply"}])).unwrap();
        assert_eq!(h.app.session.doc.param("length").unwrap().expr, "10 mm");
        assert!(h.app.session.doc.configuration_matches());
        h.run(&json!([
            {"widget":"10 mm","near":"Value"},{"key":"A","cmd":true},{"text":"20 mm"},{"widget":"Save Rule"},
            {"widget":"Apply"}
        ]))
        .unwrap();
        assert_eq!(h.app.session.doc.param("length").unwrap().expr, "20 mm");
        assert_eq!(h.app.session.doc.configs.rows[0].cells[0], "40 mm");
        let before = h.app.session.doc.clone();
        h.run(&json!([
            {"widget":"Drive parameter"},{"widget":"Range"},
            {"widget":"0 mm","near":"Minimum"},{"key":"A","cmd":true},{"text":"100 mm"},{"widget":"Save Rule"}
        ]))
        .unwrap();
        assert_eq!(h.app.session.doc, before, "failed rule edit is atomic");
        assert!(
            h.widget("configuration rules: rule `New rule` restricts the final value").is_some()
                || h.output
                    .accesskit_update
                    .as_ref()
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|(_, n)| n.label().or(n.value()).is_some_and(|text| text.contains("restricts the final value")))
        );
    }

    #[test]
    fn configuration_component_and_face_pickers_start_fresh_and_clear_inherited() {
        for (component, width) in [(true, 1400), (false, 900)] {
            let mut h = crate::scenario::Harness::new();
            h.call("ui.resize", json!({"width":width,"height":720}));
            if component {
                h.call("engine.execute", json!({"command":"component.create","params":{"name":"Part"}}));
            }
            h.run(&json!([
                {"call":"engine.execute","params":{"command":"solid.box","params":{"length":10,"width":10,"height":10,"body_name":"Block"}}},
                {"start":"config.table"}, {"widget":"Add Column"}
            ]))
            .unwrap();
            if component {
                h.step(&json!({"widget":"Part (component appearance)"})).unwrap();
                assert!(h.widget("Component: Part").is_some());
            } else {
                h.run(&json!([{"widget":"Face Appearance"},{"widget":"Faces on Block"},{"widget":"+z"}])).unwrap();
                assert!(h.widget("Face: Block / +z").is_some());
            }
            assert_eq!(h.app.session.doc.configs.rows.len(), 1);
            h.run(&json!([{"widget":"Library"},{"widget":"ABS - Red"},{"widget":"Apply"}])).unwrap();
            if component {
                assert_eq!(h.app.session.doc.appearances.components.values().next().unwrap().name, "ABS - Red");
            } else {
                assert_eq!(h.app.session.doc.appearances.faces[0].look.name, "ABS - Red");
                assert!(h.app.session.doc.appearances.faces[0].strict_name);
            }
            h.run(&json!([{"widget":"Library"},{"widget":"Follow inherited appearance"},{"widget":"Apply"}])).unwrap();
            assert!(h.app.session.doc.appearances.components.is_empty());
            assert!(h.app.session.doc.appearances.faces.is_empty());
            crate::scenario::check(&mut h, &json!({"windows_fit":{"max_width":560}})).unwrap();
        }
    }

    #[test]
    fn configuration_annulus_picker_exposes_both_certificates_despite_shared_legacy_name() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.resize", json!({"width":1400,"height":900}));
        h.run(&json!([
            {"call":"engine.execute","params":{"command":"sketch.create","params":{"plane":"XY"}}},
            {"call":"engine.execute","params":{"command":"sketch.circle.center","params":{"center":[0,0],"radius":10,"id":"outer"}}},
            {"call":"engine.execute","params":{"command":"sketch.circle.center","params":{"center":[0,0],"radius":9.999,"id":"inner"}}},
            {"call":"engine.execute","params":{"command":"sketch.finish","params":{}}},
            {"call":"engine.execute","params":{"command":"solid.extrude","params":{"distance":10,"profiles":[{"point":[9.9995,0]}],"body_name":"Ring"}}},
            {"start":"config.table"},{"widget":"Add Column"},{"widget":"Face Appearance"},{"widget":"Faces on Ring"}
        ]))
        .unwrap();
        assert!(h.widget("side:outer").is_some());
        assert!(h.widget("side:inner").is_some());
        h.run(&json!([{"widget":"side:outer"},{"widget":"Library"},{"widget":"ABS - Red"},{"widget":"Apply"}])).unwrap();
        let solidcraft_engine::doc::config::Column::FaceAppearance(target) = &h.app.session.doc.configs.columns[0] else { panic!() };
        assert!(matches!(&target.certificate, Some(solidcraft_engine::doc::config::FaceCertificate::ExtrudeSide { curve, .. }) if curve == "outer"));
        let state = h.app.session.model.state();
        let body = state.body("Ring").unwrap();
        let indices = h.app.session.doc.configuration_face_indices(body, target).unwrap();
        assert_eq!(indices.len(), 2);
        let painted = h.app.session.doc.face_colors(body);
        assert_eq!(painted.len(), 2);
        assert!(painted.iter().all(|(index, _)| indices.contains(index)));
    }

    #[test]
    fn configuration_style_picker_applies_then_inherits_material() {
        let steps: Value = serde_json::from_str(include_str!("../tests/scenarios/s80_configuration_styles.json")).unwrap();
        let mut harness = crate::scenario::Harness::new();
        for step in steps.as_array().unwrap() {
            harness.step(step).unwrap();
            match step.get("shot").and_then(Value::as_str) {
                Some("configuration_red") => {
                    let look = harness.app.session.doc.appearances.bodies.get("Block").unwrap();
                    assert_eq!(look.color, [196, 40, 40]);
                    assert!(harness.app.session.doc.configuration_matches());
                }
                Some("configuration_default") => {
                    assert!(!harness.app.session.doc.appearances.bodies.contains_key("Block"));
                    let state = harness.app.session.model.state();
                    let body = state.body("Block").unwrap();
                    let look = harness.app.session.doc.body_look("Block", body.feature).unwrap();
                    assert_eq!(look.name, "Steel");
                    assert_eq!(look.color, [158, 164, 172]);
                    assert!(harness.app.session.doc.configuration_matches());
                }
                _ => {}
            }
        }
    }

    #[test]
    fn configuration_table_actions_stay_on_one_line_at_normal_and_narrow_widths() {
        for width in [1400, 900] {
            let mut harness = crate::scenario::Harness::new();
            harness.call("ui.resize", json!({"width": width, "height": 720}));
            harness
                .run(&json!([
                    {"call":"engine.execute","params":{"command":"solid.box","params":{"length":40,"width":20,"height":10,"body_name":"Block"}}},
                    {"start":"config.table"}
                ]))
                .unwrap();
            assert_single_line(&harness, "Configuration");
            for name in ["span", "thickness", "depth", "clearance"] {
                harness.call("engine.execute", json!({"command":"parameters.change","params":{"name":name,"expression":"10 mm"}}));
            }
            harness
                .run(&json!([
                    {"call":"engine.execute","params":{"command":"config.configure","params":{
                        "params":["span","thickness","depth","clearance"],"materials":["Block"],"appearances":["Block"]
                    }}},
                    {"call":"engine.execute","params":{"command":"config.row","params":{"name":"Default","values":{"param:span":"20 mm"}}}}
                ]))
                .unwrap();
            harness.frames(4);
            for label in ["Configuration", "Active", "Apply", "Add Row", "Add Column"] {
                assert_single_line(&harness, label);
            }
            crate::scenario::check(&mut harness, &json!({"windows_fit":{"max_width":560}})).unwrap();
            for name in ["span", "thickness", "depth", "clearance"] {
                harness.call("engine.execute", json!({"command":"config.column","params":{"param":name,"delete":true}}));
            }
            harness.call("engine.execute", json!({"command":"config.column","params":{"appearance":"Block","delete":true}}));
            assert_single_line(&harness, "Material: Block");
            harness.call("engine.execute", json!({"command":"config.column","params":{"material":"Block","delete":true}}));
            harness.call("engine.execute", json!({"command":"config.column","params":{"feature":"Box1"}}));
            assert_single_line(&harness, "Suppress: Box1");
        }
        fn assert_single_line(harness: &crate::scenario::Harness, label: &str) {
            let tree = harness.output.accesskit_update.as_ref().unwrap();
            let bounds = tree
                .nodes
                .iter()
                .filter(|(_, n)| n.label() == Some(label) || n.value() == Some(label))
                .filter_map(|(_, n)| n.bounds())
                .collect::<Vec<_>>();
            assert!(
                !bounds.is_empty(),
                "missing {label}; labels/values: {:?}",
                tree.nodes.iter().filter_map(|(_, n)| n.label().or(n.value())).collect::<Vec<_>>()
            );
            assert!(bounds.iter().all(|b| b.y1 - b.y0 <= 24.0), "{label} wrapped: {bounds:?}");
        }
    }

    /// Export Animation saves the study the dialog shows (its keys need not be saved) as a GIF,
    /// also when the name is typed without `.gif`, and the design keeps no study.
    #[test]
    fn export_animation_saves_the_dialogs_study_as_a_gif() {
        // Drawn as the CLI draws (the headless device), as scenario shots are.
        #[cfg(not(target_arch = "wasm32"))]
        if solidcraft_engine::capture::installed().is_none() {
            solidcraft_engine::capture::install(std::sync::Arc::new(|scene, cam, w, h| {
                let (rgba, renderer) = solidcraft_gpu::capture::headless_rgba(scene, cam, w, h)?;
                Ok(solidcraft_engine::capture::Picture { rgba, renderer })
            }));
        }
        let mut h = crate::scenario::Harness::new();
        for (name, x) in [("A", 0), ("B", 30)] {
            h.app.run("component.activate", json!({"component": "root"})).unwrap();
            h.app.run("component.create", json!({ "name": name })).unwrap();
            h.app.run("solid.box", json!({"length": 20, "width": 20, "height": 10, "corner": [x, 0, 0]})).unwrap();
        }
        let a = json!({"occurrence": "A:1", "face": [10, 10, 10]});
        h.app.run("joint.create", json!({"type": "revolute", "a": a, "b": {"occurrence": "B:1", "face": [40, 10, 0]}})).unwrap();
        h.app.start("motion.study");
        h.frames(3);
        let Some(Kind::Motion(Mo::MotionStudy { name, steps, tracks, .. })) = h.app.dialog.as_mut().map(|d| &mut d.kind) else {
            panic!("no Motion Study dialog");
        };
        *name = "Swing".into();
        *steps = "4".into();
        *tracks = vec![Track { joint: 0, index: 0, points: vec![(0, "0 deg".into()), (4, "90 deg".into())] }];
        let dir = h.dir.clone();
        h.app.services.pick_save = Some(Box::new(move |name, extensions| {
            assert_eq!((name, extensions), ("Swing.gif", &["gif"][..]));
            Box::pin(std::future::ready(Some(dir.join("swing").to_string_lossy().into_owned())))
        }));
        h.frames(2);
        h.step(&json!({"widget": "Export Animation…"})).unwrap();
        h.frames(3);
        let gif = std::fs::read(h.dir.join("swing.gif")).unwrap();
        assert!(gif.starts_with(b"GIF89a"), "{:?}", h.app.status);
        assert!(h.app.status.as_ref().is_some_and(|s| !s.2), "{:?}", h.app.status);
        assert!(h.app.session.doc.assembly.studies.is_empty(), "exporting saves no study");
    }

    #[test]
    fn study_keys_by_joint_id() {
        let mut s = Session::default();
        for (name, x) in [("A", 0), ("B", 30)] {
            s.execute("component.activate", &json!({"component": "root"})).unwrap();
            s.execute("component.create", &json!({ "name": name })).unwrap();
            s.execute("solid.box", &json!({"length": 20, "width": 20, "height": 10, "corner": [x, 0, 0]})).unwrap();
        }
        s.execute(
            "joint.create",
            &json!({"type": "revolute", "a": {"occurrence": "A:1", "face": [10, 10, 10]}, "b": {"occurrence": "B:1", "face": [40, 10, 0]}}),
        )
        .unwrap();
        let tracks = vec![Track { joint: 0, index: 0, points: vec![(0, "0 deg".into()), (30, "90 deg".into())] }];
        let p = study_params(&s, "Study1", None, "60", &tracks).unwrap();
        assert_eq!(p["steps"], 60);
        assert_eq!(p["keys"][0]["joint"], s.doc.assembly.joints[0].id);
        assert_eq!(p["keys"][0]["points"], json!([[0, "0 deg"], [30, "90 deg"]]));
        assert!(study_params(&s, "S", None, "x", &tracks).is_err());
        assert!(study_params(&s, "S", None, "10", &[]).is_err());
    }
}
