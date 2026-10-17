//! Edit Text: the shared feature dialog (OK/Cancel, expression fields) for the source of a
//! sketch text's outlines; OK runs `sketch.edit_text`. The canvas follows the fields as they
//! change (live `sketch.edit_text`); the whole edit is one undo step and Cancel puts the text
//! back. The Text tool places a text and opens this dialog on it, so a new text is typed here too.
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use solidcraft_engine::{
    Sel,
    doc::expr::Kind as ValueKind,
    sketch::{LinkKind, LinkSource, TextAlign, TextStyle},
};

use crate::SolveApp;
use crate::dialogs::{Dialog, Kind};
use crate::selection::SelInput;

/// Field rows after the text: position, height (lengths), angle, letter spacing (a length) and
/// line spacing (a factor).
const LABELS: [&str; FIELDS] = ["X", "Y", "Height", "Angle", "Letter Spacing", "Line Spacing"];
const KINDS: [ValueKind; FIELDS] =
    [ValueKind::Length, ValueKind::Length, ValueKind::Length, ValueKind::Angle, ValueKind::Length, ValueKind::Unitless];
const FIELDS: usize = 6;
/// The field shown in degrees, and the one without a unit.
const ANGLE: usize = 3;
/// The fields before this one (X, Y, Height, Angle) can hold an expression of parameters.
const ANGLE_END: usize = 4;
const LINE_SPACING: usize = 5;

/// No input is active: a click on the canvas picks no path until the Path chip is clicked.
const NO_INPUT: usize = usize::MAX;

/// The Edit Text dialog's state: the text being edited and its values as expressions.
#[derive(Clone, Debug)]
pub struct Tx {
    sketch: u64,
    pub(crate) link: String,
    pub(crate) text: String,
    initial_text: String,
    /// The font as typed: an installed font's family name or a font file's path; empty is the built-in font.
    pub(crate) font: String,
    initial_font: String,
    values: [String; FIELDS],
    /// The fields as first shown; an untouched field keeps its exact saved value.
    initial: [String; FIELDS],
    /// The saved source: x, y, height (mm), angle (rad), letter spacing (mm) and line spacing.
    original: [f64; FIELDS],
    /// The expressions saved behind x, y, height and angle (a field shows its expression, and
    /// keeps it while untouched).
    exprs: [Option<String>; FIELDS],
    /// Bold, italic, alignment, the path (the Path input's curve) and its direction as chosen.
    pub(crate) style: TextStyle,
    initial_style: TextStyle,
    /// The Path chip was clicked: the next curve clicked on the canvas is the path.
    picking: bool,
    pub(crate) unit: &'static str,
    /// Focus the text with all of it selected (typing replaces it).
    focus: bool,
    /// Filters the font list.
    font_filter: String,
    /// The checkpoint from before the dialog changed the design (its first live change, or the
    /// Text tool placing the text): Cancel goes back to it.
    start: Option<u64>,
    /// The undo history holds the step from before the dialog: later changes fold into it.
    folded: bool,
    /// The last parameters the canvas was given (tried once, whether they worked or not).
    tried: Option<Value>,
    /// The source the live preview made: a design that shows it was changed by the dialog itself.
    shown: Option<Source>,
}

/// A text source as the dialog sees it: text, font (empty: built-in), its values, its style and
/// the expressions behind its values.
type Source = (String, String, [f64; FIELDS], TextStyle, [Option<String>; FIELDS]);

/// What the Text tool puts down, ready to be typed over.
const PLACEHOLDER: &str = "Text";

/// Resolve an outline or filled letter only when all its boundaries belong to one text.
pub(crate) fn entity_for_selection(app: &SolveApp, selection: &Sel) -> Option<String> {
    let sid = app.session.active_sketch?;
    let state = app.session.world_state();
    let ss = state.sketch(sid)?;
    let ids = match selection {
        Sel::SketchCurve { id } => vec![id.clone()],
        Sel::Profile { sketch, index } if *sketch == sid => ss.profiles.get(*index)?.curve_ids(),
        // A point of a letter's outline stands for its text (a double-click often lands on one).
        Sel::SketchPoint { id } => {
            let point = ss.sketch.resolve_point(id).and_then(|i| ss.sketch.points.get(i))?;
            let link = point.link.as_deref()?;
            vec![ss.sketch.curves.iter().find(|curve| curve.link.as_deref() == Some(link))?.id.clone()]
        }
        _ => return None,
    };
    let mut link = None;
    for id in &ids {
        let curve = ss.sketch.curves.iter().find(|curve| &curve.id == id)?;
        let source = ss.sketch.link(curve.link.as_deref()?)?;
        if source.kind != LinkKind::Text || link.is_some_and(|old| old != source.id.as_str()) {
            return None;
        }
        link = Some(source.id.as_str());
    }
    ids.into_iter().next()
}

thread_local! {
    /// Shift is held in the viewport: a click picks one letter of a text, not the whole text.
    static ONE_LETTER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The viewport's modifiers each frame: Shift picks letters one by one.
pub(crate) fn set_one_letter(on: bool) {
    ONE_LETTER.with(|c| c.set(on));
}

pub(crate) fn one_letter() -> bool {
    ONE_LETTER.with(std::cell::Cell::get)
}

/// A text is picked whole: the profiles of its letters when profile `index` of `sketch` is
/// bounded by one text alone (none when other geometry shares it). The letters' counters (the
/// inside of an O) are left out; inside one, a letter is a letter again. The picked profile
/// comes first when it is a letter.
pub(crate) fn text_profiles(s: &solidcraft_engine::Session, sketch: u64, index: usize) -> Option<Vec<usize>> {
    let state = s.world_state();
    let ss = state.sketch(sketch)?;
    let text_of = |profile: &solidcraft_engine::sketch::Profile| -> Option<String> {
        let mut link: Option<&str> = None;
        for id in profile.curve_ids() {
            let curve = ss.sketch.curves.iter().find(|curve| curve.id == id)?;
            let source = ss.sketch.link(curve.link.as_deref()?)?;
            if source.kind != LinkKind::Text || link.is_some_and(|old| old != source.id.as_str()) {
                return None;
            }
            link = Some(source.id.as_str());
        }
        link.map(str::to_owned)
    };
    let link = text_of(ss.profiles.get(index)?)?;
    let sorted = |ids: &[String]| {
        let mut ids = ids.to_vec();
        ids.sort();
        ids
    };
    let mine: Vec<usize> = (0..ss.profiles.len()).filter(|&i| ss.profiles.get(i).and_then(text_of).as_deref() == Some(link.as_str())).collect();
    // A profile whose outer loop is a hole of another one lies inside it.
    let parent = |i: usize| -> Option<usize> {
        let outer = sorted(&ss.profiles.get(i)?.outer_curves);
        mine.iter().copied().find(|&j| j != i && ss.profiles.get(j).is_some_and(|p| p.hole_curves.iter().any(|hole| sorted(hole) == outer)))
    };
    let depth = |i: usize| {
        let (mut at, mut depth) = (i, 0usize);
        while let Some(up) = parent(at) {
            at = up;
            depth += 1;
            if depth > mine.len() {
                break;
            }
        }
        depth
    };
    let mut letters: Vec<usize> = mine.iter().copied().filter(|&i| depth(i) % 2 == 0).collect();
    letters.sort_by_key(|&i| i != index);
    (!letters.is_empty()).then_some(letters)
}

/// Only a complete selection belonging to one text source can seed the editor.
pub(crate) fn selected_entity(app: &SolveApp) -> Option<String> {
    let sk = app.session.doc.sketch(app.session.active_sketch?).ok()?;
    let mut selected = None;
    let mut link = None;
    for selection in &app.session.selection {
        let id = entity_for_selection(app, selection)?;
        let curve = sk.curves.iter().find(|curve| curve.id == id)?;
        let source = sk.link(curve.link.as_deref()?)?;
        if source.kind != LinkKind::Text || link.is_some_and(|old| old != source.id.as_str()) {
            return None;
        }
        link = Some(source.id.as_str());
        selected = Some(id.clone());
    }
    selected
}

/// The link of the one text the selection belongs to (see [`selected_entity`]).
pub(crate) fn selected_link(app: &SolveApp) -> Option<String> {
    let entity = selected_entity(app)?;
    let sk = app.session.doc.sketch(app.session.active_sketch?).ok()?;
    sk.curves.iter().find(|curve| curve.id == entity)?.link.clone()
}

/// Show every outline affected by the open editor without changing feature selections.
pub(crate) fn highlighted(app: &SolveApp) -> Vec<Sel> {
    let Some(Kind::Text(tx)) = app.dialog.as_ref().map(|dialog| &dialog.kind) else { return Vec::new() };
    let Ok(sk) = app.session.doc.sketch(tx.sketch) else { return Vec::new() };
    sk.curves.iter().filter(|curve| curve.link.as_deref() == Some(tx.link.as_str())).map(|curve| Sel::SketchCurve { id: curve.id.clone() }).collect()
}

/// A value shown in a field: rounded so binary noise does not show, with its unit.
fn shown(value: f64, unit: &str) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    let text = if text == "-0" { "0" } else { text };
    if unit.is_empty() { text.to_string() } else { format!("{text} {unit}") }
}

/// The saved text source behind `link` in the active sketch: its text, its font (empty: the built-in
/// font), and x, y, height and angle.
fn source(app: &SolveApp, sketch: u64, link: &str) -> Option<Source> {
    if app.session.active_sketch != Some(sketch) {
        return None;
    }
    let sk = app.session.doc.sketch(sketch).ok()?;
    let source = sk.link(link).filter(|source| source.kind == LinkKind::Text)?;
    let LinkSource::Text { text, at, height, angle, font, style, exprs } = &source.source else { return None };
    let values = [at.x, at.y, *height, *angle, style.letter_spacing, style.line_spacing];
    let exprs = [exprs.x.clone(), exprs.y.clone(), exprs.height.clone(), exprs.angle.clone(), None, None];
    Some((text.clone(), font.clone().unwrap_or_default(), values, style.clone(), exprs))
}

/// The fields as shown: a value, or the expression behind it.
fn fields(original: &[f64; FIELDS], exprs: &[Option<String>; FIELDS], unit: &'static str, scale: f64) -> [String; FIELDS] {
    std::array::from_fn(|i| {
        if let Some(Some(e)) = exprs.get(i) {
            return e.clone();
        }
        let value = original.get(i).copied().unwrap_or_default();
        match i {
            ANGLE => shown(value.to_degrees(), "deg"),
            LINE_SPACING => shown(value, ""),
            _ => shown(value * scale, unit),
        }
    })
}

/// The dialog for a text: its Path input holds the curve the text follows; no input is active
/// until the Path chip is clicked.
fn dialog(tx: Box<Tx>) -> Dialog {
    let inputs = inputs_for(&tx);
    let mut d = Dialog::new(Kind::Text(tx), inputs);
    d.active = NO_INPUT;
    d
}

fn inputs_for(tx: &Tx) -> Vec<SelInput> {
    let mut path = SelInput::new("Path", crate::selection::CURVES, false);
    path.items.extend(tx.style.path.iter().map(|id| Sel::SketchCurve { id: id.clone() }));
    vec![path]
}

/// Each frame, before the rows: the path is the Path input's curve; a new pick ends picking.
pub(crate) fn sync_path(d: &mut Dialog) {
    let Kind::Text(k) = &mut d.kind else { return };
    let picked = d.inputs.first().and_then(|i| i.items.first()).and_then(|sel| match sel {
        Sel::SketchCurve { id } => Some(id.clone()),
        _ => None,
    });
    if picked != k.style.path {
        k.style.path = picked;
        k.picking = false;
    }
    if !k.picking {
        d.active = NO_INPUT;
    }
}

/// The dialog's kind for the text that `entity` (an outline curve) belongs to.
fn kind_for(app: &SolveApp, entity: &str) -> Option<Kind> {
    let sketch = app.session.active_sketch?;
    let sk = app.session.doc.sketch(sketch).ok()?;
    let link = sk.curves.iter().find(|curve| curve.id == entity).and_then(|curve| curve.link.clone())?;
    let (text, font, original, style, exprs) = source(app, sketch, &link)?;
    let (scale, unit) = crate::prefs::unit_scale(&app.session.doc.units);
    let values = fields(&original, &exprs, unit, scale);
    let mut tx = Tx {
        sketch,
        link,
        initial_text: text.clone(),
        text,
        initial_font: font.clone(),
        font,
        initial: values.clone(),
        values,
        original,
        exprs,
        unit,
        initial_style: style.clone(),
        style,
        picking: false,
        focus: true,
        font_filter: String::new(),
        start: None,
        folded: false,
        tried: None,
        shown: None,
    };
    // The canvas already shows the fields as they open.
    tx.tried = edit_params(app, &tx).ok();
    Some(Kind::Text(Box::new(tx)))
}

/// `Dialog::for_command`: Edit Text with one text's outlines selected.
pub(crate) fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    if id != "sketch.edit_text" {
        return None;
    }
    let Kind::Text(tx) = kind_for(app, &selected_entity(app)?)? else { return None };
    let inputs = inputs_for(&tx);
    Some((Kind::Text(tx), inputs))
}

/// Open the dialog for the text an outline (picked with the Edit Text tool) belongs to.
pub(crate) fn open(app: &mut SolveApp, entity: &str) {
    match kind_for(app, entity) {
        Some(Kind::Text(tx)) => {
            app.dialog = Some(dialog(tx));
            app.tool = None;
        }
        _ => app.set_status("Choose an outline belonging to sketch text", true),
    }
}

/// The Text tool's click: put a placeholder text down at `at` (sketch coordinates) and open
/// Edit Text on it, its text selected so that typing replaces it. Cancel takes it away again;
/// OK keeps it as one undo step.
pub(crate) fn place(app: &mut SolveApp, at: solidcraft_engine::geom::Vec2, height: f64) {
    let Some(checkpoint) = app.session.execute("edit.checkpoint", &json!({})).ok().and_then(|r| r["checkpoint"].as_u64()) else { return };
    let placed = app.run("sketch.text", json!({"text": PLACEHOLDER, "at": [at.x, at.y], "height": height}));
    let curve = placed.as_ref().ok().and_then(|v| v["curves"].get(0)?.as_str().map(str::to_owned));
    match curve.and_then(|curve| kind_for(app, &curve)) {
        Some(Kind::Text(mut tx)) => {
            tx.start = Some(checkpoint);
            tx.folded = true;
            app.dialog = Some(dialog(tx));
            app.tool = None;
        }
        _ => {
            let forget = placed.is_err();
            let _ = app.session.execute("edit.restore_checkpoint", &json!({"checkpoint": checkpoint, "forget": forget}));
            app.set_status(placed.err().unwrap_or_else(|| "the text could not be placed".into()), true);
        }
    }
}

/// Run one of the dialog's changes: the undo history keeps one step for the whole dialog (the
/// first change's, which goes back to the design from before it).
fn folded<T>(app: &mut SolveApp, k: &mut Tx, run: impl FnOnce(&mut SolveApp) -> T) -> T {
    let mut undo = std::mem::take(&mut app.session.undo);
    let out = run(app);
    let pushed = std::mem::take(&mut app.session.undo);
    if !k.folded && !pushed.is_empty() {
        undo.extend(pushed);
        k.folded = true;
    }
    app.session.undo = undo;
    out
}

/// Each frame: show the fields on the canvas (a live edit) when they changed. Fields that make
/// no text (an empty one, a half-typed value) leave the last good preview.
pub(crate) fn preview(app: &mut SolveApp, k: &mut Tx) {
    let Ok(params) = edit_params(app, k) else { return };
    if k.tried.as_ref() == Some(&params) {
        return;
    }
    k.tried = Some(params.clone());
    if k.start.is_none() {
        let Some(checkpoint) = app.session.execute("edit.checkpoint", &json!({})).ok().and_then(|r| r["checkpoint"].as_u64()) else { return };
        k.start = Some(checkpoint);
    }
    let mut live = params;
    live["live"] = json!(true);
    if folded(app, k, |app| app.session.execute("sketch.edit_text", &live)).is_ok() {
        k.shown = source(app, k.sketch, &k.link);
    }
}

/// OK: the fields' final edit (which also brings the features after the sketch up to date), in
/// the dialog's one undo step.
pub(crate) fn apply(app: &mut SolveApp, k: &mut Tx) -> Result<(), String> {
    let params = edit_params(app, k)?;
    folded(app, k, |app| app.run("sketch.edit_text", params))?;
    k.shown = source(app, k.sketch, &k.link);
    Ok(())
}

/// The dialog closed: applied, its changes stay; otherwise the design goes back to how it was
/// before them (a placed text goes away).
pub(crate) fn closed(app: &mut SolveApp, k: &mut Tx, applied: bool) {
    if let Some(checkpoint) = k.start.take() {
        let _ = app.session.execute("edit.restore_checkpoint", &json!({"checkpoint": checkpoint, "forget": applied}));
    }
}

/// Is the Edit Text dialog open?
pub(crate) fn editing(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::Text(_)))
}

/// Keep the dialog in step with the design: false once its text is gone (sketch finished, the
/// text deleted or undone). Other design changes leave it open; when the saved source itself
/// changed (undo, a script), fields the user has not touched follow it.
pub(crate) fn refresh(app: &SolveApp, k: &mut Tx) -> bool {
    let Some(current) = source(app, k.sketch, &k.link) else { return false };
    // What the live preview made is the dialog's own doing.
    if k.shown.as_ref() == Some(&current) {
        return true;
    }
    let (text, font, original, style, exprs) = current;
    if style != k.initial_style {
        if k.style == k.initial_style {
            k.style = style.clone();
        }
        k.initial_style = style;
    }
    if original != k.original || exprs != k.exprs {
        let (scale, unit) = crate::prefs::unit_scale(&app.session.doc.units);
        let fresh = fields(&original, &exprs, unit, scale);
        for ((value, initial), new) in k.values.iter_mut().zip(k.initial.iter_mut()).zip(fresh) {
            if value == initial {
                *value = new.clone();
            }
            *initial = new;
        }
        k.original = original;
        k.exprs = exprs;
        k.unit = unit;
    }
    if text != k.initial_text {
        if k.text == k.initial_text {
            k.text = text.clone();
        }
        k.initial_text = text;
    }
    if font != k.initial_font {
        if k.font == k.initial_font {
            k.font = font.clone();
        }
        k.initial_font = font;
    }
    true
}

/// The dialog's rows; true when Enter was pressed in a value field or in the text (Shift+Enter
/// starts a new line there). `inputs` and `active`: the dialog's Path input.
pub(crate) fn rows(ui: &mut egui::Ui, k: &mut Tx, inputs: &mut [SelInput], active: &mut usize) -> bool {
    let mut enter = false;
    crate::dialogs::row_label(ui, "Text");
    let width = ui.spacing().text_edit_width;
    let id = egui::Id::new("sketch_text_content");
    if ui.memory(|m| m.has_focus(id)) {
        enter |= ui.input_mut(|i| !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    }
    let response = ui.add(egui::TextEdit::multiline(&mut k.text).id(id).desired_rows(3).desired_width(width));
    crate::scenario::publish_handle("dialog:Text", response.rect.center());
    if k.focus {
        response.request_focus();
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(k.text.chars().count()));
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), id);
        k.focus = false;
    }
    ui.end_row();
    crate::dialogs::row_label(ui, "Font");
    font_row(ui, k, width);
    ui.end_row();
    crate::dialogs::row_label(ui, "Style");
    ui.horizontal(|ui| {
        let bold = ui.toggle_value(&mut k.style.bold, egui::RichText::new("B").strong()).on_hover_text(crate::i18n::tr("sketch_text.bold"));
        crate::scenario::publish_handle("dialog:Bold", bold.rect.center());
        let italic = ui.toggle_value(&mut k.style.italic, egui::RichText::new("I").italics()).on_hover_text(crate::i18n::tr("sketch_text.italic"));
        crate::scenario::publish_handle("dialog:Italic", italic.rect.center());
    });
    ui.end_row();
    crate::dialogs::row_label(ui, "Alignment");
    ui.horizontal(|ui| {
        for (align, id, handle) in [
            (TextAlign::Left, "sketch_text.align_left", "dialog:Align left"),
            (TextAlign::Center, "sketch_text.align_center", "dialog:Align center"),
            (TextAlign::Right, "sketch_text.align_right", "dialog:Align right"),
        ] {
            let response = ui.selectable_value(&mut k.style.align, align, crate::i18n::tr(id));
            crate::scenario::publish_handle(handle, response.rect.center());
        }
    });
    ui.end_row();
    // On a path the curve places the text: X, Y and the angle do not.
    let on_path = k.style.path.is_some();
    for (i, (label, value)) in LABELS.iter().zip(k.values.iter_mut()).enumerate() {
        crate::dialogs::row_label(ui, label);
        let placed = on_path && matches!(i, 0 | 1 | ANGLE);
        if placed {
            let response = ui.add_enabled_ui(false, |ui| crate::dialogs::field(ui, value));
            response.response.on_disabled_hover_text(crate::i18n::tr("sketch_text.on_path"));
        } else {
            enter |= crate::dialogs::field(ui, value);
        }
        ui.end_row();
    }
    path_row(ui, k, inputs, active);
    enter
}

/// The curve the text follows: a chip that, clicked, takes the next curve clicked on the canvas;
/// × takes the text off it; Reverse runs it the other way along.
fn path_row(ui: &mut egui::Ui, k: &mut Tx, inputs: &mut [SelInput], active: &mut usize) {
    let t = crate::theme::Tokens::get();
    crate::dialogs::row_label(ui, "Path");
    ui.horizontal(|ui| {
        let on = k.style.path.is_some();
        let text = if on {
            egui::RichText::new(crate::i18n::format("dialog.selected_count", &[("count", "1")])).color(egui::Color32::WHITE)
        } else {
            egui::RichText::new(crate::i18n::tr("dialog.select")).color(t.text)
        };
        let stroke = if k.picking { egui::Stroke::new(1.5, t.accent) } else { egui::Stroke::new(1.0, t.dialog_border.gamma_multiply(1.6)) };
        let chip = egui::Button::new(text).fill(if on { t.chip } else { egui::Color32::TRANSPARENT }).stroke(stroke).min_size(egui::vec2(0.0, 22.0));
        let chip = crate::frame::add_outlined(ui, chip, stroke.width).on_hover_text(crate::i18n::tr("sketch_text.path_hint"));
        crate::scenario::publish_handle("dialog:Path", chip.rect.center());
        if chip.clicked() {
            k.picking = true;
            *active = 0;
        }
        if on {
            let clear = ui.small_button("\u{d7}").on_hover_text(crate::i18n::tr("sketch_text.path_clear"));
            crate::scenario::publish_handle("dialog:Path clear", clear.rect.center());
            if clear.clicked() {
                for input in inputs.iter_mut() {
                    input.items.clear();
                }
                k.style.path = None;
                k.picking = false;
            }
        }
    });
    ui.end_row();
    if k.style.path.is_some() {
        crate::dialogs::row_label(ui, "Reverse");
        let reverse = ui.checkbox(&mut k.style.reverse, "");
        crate::scenario::publish_handle("dialog:Reverse", reverse.rect.center());
        ui.end_row();
    }
}

/// The installed font families, listed once in the background (none on the web).
pub(crate) fn families(ctx: &egui::Context) -> Option<Arc<Vec<String>>> {
    static FAMILIES: OnceLock<Arc<Vec<String>>> = OnceLock::new();
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if let Some(list) = FAMILIES.get() {
        return Some(list.clone());
    }
    if !STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new().name("font list".into()).spawn(move || {
                let _ = FAMILIES.set(Arc::new(solidcraft_engine::sketch::installed_families()));
                ctx.request_repaint();
            });
            if spawned.is_err() {
                let _ = FAMILIES.set(Arc::new(Vec::new()));
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = FAMILIES.set(Arc::new(Vec::new()));
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    FAMILIES.get().cloned()
}

/// Does the font search name a font file rather than filter the list?
fn is_font_file(s: &str) -> bool {
    let lower = s.trim().to_ascii_lowercase();
    lower.contains(['/', '\\']) || [".ttf", ".otf", ".ttc", ".otc"].iter().any(|e| lower.ends_with(e))
}

/// Height of the font list (points).
const FONT_LIST_PX: f32 = 260.0;

/// The font: a list of the installed families (the built-in font first) with a search box; a
/// font file's path typed there can be used too.
fn font_row(ui: &mut egui::Ui, k: &mut Tx, width: f32) {
    let builtin = crate::i18n::format("sketch_text.builtin_font", &[("family", solidcraft_engine::sketch::DEFAULT_FONT_FAMILY)]);
    let current = if k.font.trim().is_empty() { builtin.clone() } else { k.font.clone() };
    let mut chosen = None;
    let response = egui::ComboBox::from_id_salt("sketch_text_font")
        .width(width)
        .height(FONT_LIST_PX + 80.0)
        .selected_text(current)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let search = egui::TextEdit::singleline(&mut k.font_filter)
                .id_salt("sketch_text_font_search")
                .hint_text(crate::i18n::tr("sketch_text.font_search"));
            let search = ui.add(search.desired_width(f32::INFINITY));
            crate::scenario::publish_handle("dialog:Font search", search.rect.center());
            let typed = k.font_filter.trim();
            if is_font_file(typed) {
                let file = ui.selectable_label(false, crate::i18n::format("sketch_text.use_font_file", &[("path", typed)]));
                crate::scenario::publish_handle("dialog:Font file", file.rect.center());
                if file.clicked() {
                    chosen = Some(typed.to_string());
                    ui.close();
                }
                return;
            }
            let filter = typed.to_lowercase();
            // The list scrolls below the search box, at one height whatever it holds.
            let list = egui::ScrollArea::vertical()
                .id_salt("sketch_text_font_list")
                .max_height(FONT_LIST_PX)
                .min_scrolled_height(FONT_LIST_PX)
                .auto_shrink([false, false]);
            list.show(ui, |ui| {
                if builtin.to_lowercase().contains(&filter) {
                    let item = ui.selectable_label(k.font.trim().is_empty(), &builtin);
                    crate::scenario::publish_handle("dialog:Font built-in", item.rect.center());
                    if item.clicked() {
                        chosen = Some(String::new());
                        ui.close();
                    }
                }
                match families(ui.ctx()) {
                    None => {
                        ui.label(crate::i18n::tr("sketch_text.loading_fonts"));
                    }
                    Some(list) => {
                        for family in list.iter().filter(|f| f.to_lowercase().contains(&filter)) {
                            if ui.selectable_label(k.font.trim() == family, family).clicked() {
                                chosen = Some(family.clone());
                                ui.close();
                            }
                        }
                    }
                }
            });
        })
        .response;
    crate::scenario::publish_handle("dialog:Font", response.rect.center());
    if let Some(font) = chosen {
        k.font = font;
        k.font_filter.clear();
    }
}

/// The command OK runs. Fields take expressions with units and parameters; a bare number is
/// in the design's units (lengths) or degrees (angle).
pub(crate) fn commands(app: &SolveApp, k: &Tx) -> Result<Vec<(String, Value)>, String> {
    Ok(vec![("sketch.edit_text".into(), edit_params(app, k)?)])
}

/// `sketch.edit_text`'s parameters for the fields as they are. X, Y, Height and Angle go as
/// numbers, or as their expressions when these name parameters (the text then follows them): an
/// untouched field that holds one keeps it.
fn edit_params(app: &SolveApp, k: &Tx) -> Result<Value, String> {
    let doc = &app.session.doc;
    let mut out = [0.0; FIELDS];
    let mut sent: [Value; FIELDS] = std::array::from_fn(|_| Value::Null);
    for (i, slot) in out.iter_mut().enumerate() {
        let (Some(value), Some(initial), Some(original), Some(kind), Some(label)) =
            (k.values.get(i), k.initial.get(i), k.original.get(i), KINDS.get(i), LABELS.get(i))
        else {
            return Err("missing value".into());
        };
        let given = k.exprs.get(i).and_then(Option::as_ref).filter(|_| value == initial);
        *slot = if value == initial {
            *original
        } else {
            let typed = doc.with_design_unit(value.trim(), *kind);
            let v = doc.eval(&typed, *kind).map_err(|e| format!("{label}: {e}"))?;
            if !v.is_finite() {
                return Err(format!("{label}: not a finite number"));
            }
            v
        };
        let names_parameters = i < ANGLE_END && !solidcraft_engine::doc::expr::references(value).is_empty();
        sent[i] = match given {
            Some(e) => json!(e),
            None if value != initial && names_parameters => json!(value.trim()),
            None => json!(if i == ANGLE { slot.to_degrees() } else { *slot }),
        };
    }
    let [_, _, _, _, letter_spacing, line_spacing] = out;
    let [sent_x, sent_y, sent_height, sent_angle, ..] = sent;
    let align = match k.style.align {
        TextAlign::Left => "left",
        TextAlign::Center => "center",
        TextAlign::Right => "right",
    };
    let mut params = json!({
        "link": k.link, "text": k.text, "at": [sent_x, sent_y], "height": sent_height,
        "bold": k.style.bold, "italic": k.style.italic, "align": align,
        "letter_spacing": letter_spacing, "line_spacing": line_spacing,
        "path": k.style.path.clone().unwrap_or_default(), "reverse": k.style.reverse,
    });
    // Omitting an unchanged angle keeps its exact saved radians (or its expression).
    if k.values.get(ANGLE) != k.initial.get(ANGLE) {
        params["angle"] = sent_angle;
    }
    // An untouched font stays as saved; an emptied one is the built-in font.
    if k.font.trim() != k.initial_font.trim() {
        params["font"] = json!(k.font.trim());
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Harness;

    fn setup() -> (Harness, String, String) {
        let mut h = Harness::new();
        h.app.home.open = false;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        let result = h.app.run("sketch.text", json!({"text":"I","at":[0,0],"height":10})).unwrap();
        let link = result["link"].as_str().unwrap().to_owned();
        let curve = result["curves"][0].as_str().unwrap().to_owned();
        h.app.session.selection = vec![Sel::SketchCurve { id: curve.clone() }];
        h.frames(3);
        (h, link, curve)
    }

    fn tx(app: &SolveApp) -> &Tx {
        match app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::Text(k)) => k,
            _ => panic!("Edit Text is not open"),
        }
    }

    #[test]
    fn letter_interior_context_menu_opens_and_highlights_the_whole_text() {
        let (mut h, link, _) = setup();
        h.app.run("sketch.edit_text", json!({"link":link,"text":"II"})).unwrap();
        h.app.session.selection.clear();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 25.0;
        h.frames(3);
        let sid = h.app.session.active_sketch.unwrap();
        let at = h.app.session.world_state().sketch(sid).unwrap().profiles[0].centroid;
        let [x, y] = h.at(&json!({"sketch":[at.x,at.y]})).unwrap();
        h.call("ui.menu", json!({"x":x,"y":y}));
        h.frames(3);
        assert!(matches!(h.app.session.selection.as_slice(), [Sel::Profile { .. }]));
        let item = crate::context_menu::items(&h.app, &crate::context_menu::Target::Viewport)
            .into_iter()
            .find(|item| item.id == "sketch.edit_text")
            .unwrap();
        crate::context_menu::run_item(&mut h.app, &item, egui::Pos2::ZERO);
        h.frames(3);
        assert_eq!(tx(&h.app).link, link);
        let sk = h.app.session.doc.sketch(sid).unwrap();
        for curve in sk.curves.iter().filter(|curve| curve.link.as_deref() == Some(&link)) {
            assert!(h.app.highlighted().contains(&Sel::SketchCurve { id: curve.id.clone() }));
        }
        h.step(&json!({"widget":"Cancel"})).unwrap();
        assert!(highlighted(&h.app).is_empty());
    }

    /// Explode Text (#1842): the text's outlines stay where they are as ordinary sketch curves,
    /// the text itself goes, and one undo brings it back.
    #[test]
    fn explode_text_leaves_its_outlines_as_normal_curves() {
        let (mut h, link, _) = setup();
        h.app.run("sketch.edit_text", json!({"link":link,"text":"TO"})).unwrap();
        let sid = h.app.session.active_sketch.unwrap();
        let outline = |h: &Harness| {
            let sk = h.app.session.doc.sketch(sid).unwrap();
            let mut ids: Vec<(String, Option<String>)> = sk.curves.iter().map(|c| (c.id.clone(), c.link.clone())).collect();
            ids.sort();
            ids
        };
        let before = outline(&h);
        assert!(before.len() > 4 && before.iter().all(|(_, l)| l.as_deref() == Some(link.as_str())));
        let profiles = h.app.session.world_state().sketch(sid).unwrap().profiles.len();
        h.app.session.selection = vec![Sel::SketchCurve { id: before[0].0.clone() }];
        let undo = h.app.session.undo.len();
        let item = crate::context_menu::items(&h.app, &crate::context_menu::Target::Viewport)
            .into_iter()
            .find(|item| item.label == "Explode Text")
            .expect("Explode Text is offered for a selected text");
        assert!(item.enabled);
        crate::context_menu::run_item(&mut h.app, &item, egui::Pos2::ZERO);
        h.frames(2);
        let after = outline(&h);
        assert_eq!(after.iter().map(|c| &c.0).collect::<Vec<_>>(), before.iter().map(|c| &c.0).collect::<Vec<_>>(), "every outline stays");
        assert!(after.iter().all(|(_, l)| l.is_none()), "no outline belongs to the text");
        let sk = h.app.session.doc.sketch(sid).unwrap();
        assert!(sk.link(&link).is_none(), "the text is gone");
        assert_eq!(h.app.session.world_state().sketch(sid).unwrap().profiles.len(), profiles, "the letters still make the same profiles");
        assert_eq!(h.app.session.undo.len(), undo + 1);
        // No longer text: nothing to edit or explode.
        assert!(selected_entity(&h.app).is_none());
        assert!(!crate::context_menu::items(&h.app, &crate::context_menu::Target::Viewport).iter().any(|item| item.label == "Explode Text"));
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(outline(&h), before, "undo brings the text back");
    }

    #[test]
    fn edit_text_tool_accepts_a_letter_interior_without_preselection() {
        let (mut h, link, _) = setup();
        h.app.session.selection.clear();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 25.0;
        h.frames(3);
        let sid = h.app.session.active_sketch.unwrap();
        let at = h.app.session.world_state().sketch(sid).unwrap().profiles[0].centroid;
        h.step(&json!({"start":"sketch.edit_text"})).unwrap();
        h.step(&json!({"click":{"sketch":[at.x,at.y]}})).unwrap();
        assert_eq!(tx(&h.app).link, link);
    }

    #[test]
    fn text_profile_resolution_checks_holes_and_rejects_mixed_sources() {
        let (mut h, link, _) = setup();
        h.app.run("sketch.edit_text", json!({"link":link,"text":"O"})).unwrap();
        let sid = h.app.session.active_sketch.unwrap();
        let state = h.app.session.world_state();
        let profile = state.sketch(sid).unwrap().profiles.iter().position(|p| !p.hole_curves.is_empty()).unwrap();
        let selection = Sel::Profile { sketch: sid, index: profile };
        assert!(entity_for_selection(&h.app, &selection).is_some());
        h.app.run("sketch.text", json!({"text":"I","at":[20,0],"height":10})).unwrap();
        let profiles = h.app.session.world_state().sketch(sid).unwrap().profiles.len();
        h.app.session.selection = (0..profiles).map(|index| Sel::Profile { sketch: sid, index }).collect();
        assert!(selected_entity(&h.app).is_none());
        assert!(entity_for_selection(&h.app, &Sel::Profile { sketch: sid + 1, index: 0 }).is_none());
    }

    #[test]
    fn profile_cut_by_ordinary_geometry_does_not_identify_text() {
        let (mut h, _, _) = setup();
        h.app.run("sketch.line", json!({"points":[[-10,3],[10,3]]})).unwrap();
        let sid = h.app.session.active_sketch.unwrap();
        let count = h.app.session.world_state().sketch(sid).unwrap().profiles.len();
        assert!(count >= 2);
        for index in 0..count {
            assert!(entity_for_selection(&h.app, &Sel::Profile { sketch: sid, index }).is_none());
        }
    }

    fn ok(h: &mut Harness) {
        h.step(&json!({"click":{"handle":"dialog:OK"}})).unwrap();
        h.frames(2);
    }

    fn replace(h: &mut Harness, handle: &str, value: &str) {
        h.step(&json!({"click":{"handle":handle}})).unwrap();
        h.step(&json!({"key":"A","cmd":true})).unwrap();
        h.step(&json!({"text":value})).unwrap();
    }

    #[test]
    fn selected_text_menu_edits_source_position_and_size_with_one_undo_and_reopen() {
        let (mut h, link, _) = setup();
        let before = h.app.session.doc.to_json();
        let undo = h.app.session.undo.len();
        let item = crate::context_menu::items(&h.app, &crate::context_menu::Target::Viewport)
            .into_iter()
            .find(|item| item.id == "sketch.edit_text")
            .unwrap();
        crate::context_menu::run_item(&mut h.app, &item, egui::Pos2::ZERO);
        h.frames(3);
        assert!(editing(&h.app));
        for (field, value) in [("dialog:Text", "O"), ("dialog:X", "25"), ("dialog:Y", "12"), ("dialog:Height", "20"), ("dialog:Angle", "90")] {
            replace(&mut h, field, value);
        }
        // The canvas already shows the edit, as one undo step.
        let sid = h.app.session.active_sketch.unwrap();
        let shown = |h: &Harness| h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source.clone();
        assert!(matches!(shown(&h), LinkSource::Text { text, height, .. } if text == "O" && (height - 20.0).abs() < 1e-12));
        assert_eq!(h.app.session.undo.len(), undo + 1);
        ok(&mut h);
        assert!(!editing(&h.app));
        assert_eq!(h.app.session.undo.len(), undo + 1);
        let sid = h.app.session.active_sketch.unwrap();
        let sk = h.app.session.doc.sketch(sid).unwrap();
        let expected = LinkSource::Text {
            text: "O".into(),
            at: solidcraft_engine::geom::Vec2::new(25.0, 12.0),
            height: 20.0,
            angle: std::f64::consts::FRAC_PI_2,
            font: None,
            style: TextStyle::default(),
            exprs: Default::default(),
        };
        assert_eq!(sk.link(&link).unwrap().source, expected);
        assert!(!h.app.session.world_state().sketch(sid).unwrap().profiles.is_empty());
        let changed = h.app.session.doc.to_json();
        let reopened = solidcraft_engine::Session::new(solidcraft_engine::doc::Document::from_json(&changed).unwrap());
        assert_eq!(reopened.doc.sketch(sid).unwrap().link(&link).unwrap().source, expected);
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), before);
        h.app.run("edit.redo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), changed);
    }

    #[test]
    fn text_editor_converts_document_units_and_preserves_untouched_source_values() {
        let (mut h, link, _) = setup();
        h.app.run("document.units", json!({"units":"in"})).unwrap();
        h.app.run("sketch.edit_text", json!({"link":link,"at":[3.123456789,7.987654321],"height":9.123456789,"angle":17.123456789})).unwrap();
        let sid = h.app.session.active_sketch.unwrap();
        let original = h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source.clone();
        h.app.start("sketch.edit_text");
        h.frames(3);
        assert_eq!(tx(&h.app).unit, "in");
        replace(&mut h, "dialog:Text", "O");
        ok(&mut h);
        let LinkSource::Text { at, height, angle, .. } = original else { panic!("text source") };
        assert_eq!(
            h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source,
            LinkSource::Text { text: "O".into(), at, height, angle, font: None, style: TextStyle::default(), exprs: Default::default() }
        );
        h.app.session.selection = vec![Sel::SketchCurve { id: h.app.session.doc.sketch(sid).unwrap().curves[0].id.clone() }];
        h.app.start("sketch.edit_text");
        h.frames(3);
        let undo = h.app.session.undo.len();
        replace(&mut h, "dialog:X", "1.2345");
        replace(&mut h, "dialog:Height", "0.25");
        ok(&mut h);
        let LinkSource::Text { at: updated, height: size, angle: rotation, .. } = h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source
        else {
            panic!("text source")
        };
        assert!((updated.x - 31.3563).abs() < 1e-12);
        assert_eq!(updated.y, at.y);
        assert!((size - 6.35).abs() < 1e-12);
        assert_eq!(rotation, angle);
        assert_eq!(h.app.session.undo.len(), undo + 1);
    }

    #[test]
    fn the_font_row_sets_keeps_and_clears_the_font_of_a_text() {
        // The repository's second font, BIZ UDPGothic: its "I" is 17.6385 mm² at a capital height of 10 mm, the built-in
        // font's 13.5557 mm².
        let font = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/biz-udp-gothic/BIZUDPGothic-Regular.ttf");
        let (mut h, link, _) = setup();
        // The dialog closes with the selection; the text's first outline is selected again to open it.
        let reopen = |h: &mut Harness| {
            let sid = h.app.session.active_sketch.unwrap();
            let id = h.app.session.doc.sketch(sid).unwrap().curves[0].id.clone();
            h.app.session.selection = vec![Sel::SketchCurve { id }];
            h.app.start("sketch.edit_text");
            h.frames(3);
        };
        let area = |h: &Harness| -> f64 {
            let sid = h.app.session.active_sketch.unwrap();
            h.app.session.world_state().sketch(sid).unwrap().profiles.iter().map(|p| p.area).sum()
        };
        assert!((area(&h) - 13.5557).abs() < 2e-3);
        h.app.start("sketch.edit_text");
        h.frames(3);
        assert_eq!(tx(&h.app).font, "");
        // A font file is typed in the list's search box and chosen from it.
        let choose_file = |h: &mut Harness, path: &str| {
            h.step(&json!({"click":{"handle":"dialog:Font"}})).unwrap();
            h.frames(2);
            h.step(&json!({"click":{"handle":"dialog:Font search"}})).unwrap();
            h.step(&json!({"text":path})).unwrap();
            h.frames(2);
            h.step(&json!({"click":{"handle":"dialog:Font file"}})).unwrap();
            h.frames(3);
        };
        // A font that is not there shows nothing new, keeps the dialog open with the reason at OK, and the design
        // as it was.
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len());
        let missing = concat!(env!("CARGO_MANIFEST_DIR"), "/no-such-font-12345.ttf");
        choose_file(&mut h, missing);
        assert_eq!(tx(&h.app).font, missing);
        assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len()), before);
        ok(&mut h);
        assert!(editing(&h.app));
        assert!(h.app.dialog.as_ref().unwrap().error.as_ref().is_some_and(|e| e.contains("font file")), "{:?}", h.app.dialog.as_ref().unwrap().error);
        assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len()), before);
        // A font file: other outlines on the canvas at once, one undo step, and the dialog shows the font when it is
        // opened again.
        choose_file(&mut h, font);
        assert!((area(&h) - 17.6385).abs() < 2e-3, "the canvas follows the font");
        ok(&mut h);
        assert!(!editing(&h.app));
        assert_eq!(h.app.session.undo.len(), before.1 + 1);
        let sid = h.app.session.active_sketch.unwrap();
        let font_of = |h: &Harness| match &h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source {
            LinkSource::Text { font, .. } => font.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(font_of(&h).as_deref(), Some(font));
        assert!((area(&h) - 17.6385).abs() < 2e-3);
        reopen(&mut h);
        assert_eq!(tx(&h.app).font, font);
        // Changing only the text keeps the font: two I's of it.
        replace(&mut h, "dialog:Text", "II");
        ok(&mut h);
        assert_eq!(font_of(&h).as_deref(), Some(font));
        assert!((area(&h) - 2.0 * 17.6385).abs() < 2e-3);
        // The built-in font, first in the list: two I's of it.
        reopen(&mut h);
        h.step(&json!({"click":{"handle":"dialog:Font"}})).unwrap();
        h.frames(2);
        h.step(&json!({"click":{"handle":"dialog:Font built-in"}})).unwrap();
        h.frames(2);
        assert_eq!(tx(&h.app).font, "");
        ok(&mut h);
        assert_eq!(font_of(&h), None);
        assert!((area(&h) - 2.0 * 13.5557).abs() < 2e-3);
    }

    #[test]
    fn invalid_text_values_and_cancel_leave_document_and_history_unchanged() {
        let (mut h, _, _) = setup();
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len());
        h.app.start("sketch.edit_text");
        h.frames(3);
        for value in ["NaN", "inf", "-5", "0"] {
            replace(&mut h, "dialog:Height", value);
            ok(&mut h);
            assert!(h.app.dialog.as_ref().unwrap().error.is_some());
            assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len()), before);
        }
        h.step(&json!({"widget":"Cancel"})).unwrap();
        assert!(!editing(&h.app));
        assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len()), before);
    }

    #[test]
    fn text_editor_rejects_mixed_sources_and_leaves_with_document_or_command() {
        let (mut h, _, curve) = setup();
        let other = h.app.run("sketch.text", json!({"text":"O","at":[30,0],"height":10})).unwrap();
        h.app.session.selection.push(Sel::SketchCurve { id: other["curves"][0].as_str().unwrap().into() });
        assert!(selected_entity(&h.app).is_none());
        assert!(!crate::context_menu::items(&h.app, &crate::context_menu::Target::Viewport).iter().any(|item| item.id == "sketch.edit_text"));
        h.app.session.selection = vec![Sel::SketchCurve { id: curve }];
        h.app.start("sketch.edit_text");
        assert!(editing(&h.app));
        h.app.start("sketch.line");
        assert!(!editing(&h.app));
        h.app.start("sketch.edit_text");
        assert!(editing(&h.app));
        crate::documents::open_tab(&mut h.app, solidcraft_engine::Session::default()).unwrap();
        assert!(!editing(&h.app));
    }

    #[test]
    fn edit_text_without_preselection_picks_a_real_outline() {
        let (mut h, link, _) = setup();
        h.app.session.selection.clear();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 40.0;
        h.frames(3);
        let sid = h.app.session.active_sketch.unwrap();
        let polyline = h.app.session.doc.sketch(sid).unwrap().polyline(0);
        let midpoint = (polyline[0] + polyline[1]) * 0.5;
        h.step(&json!({"start":"sketch.edit_text"})).unwrap();
        assert!(h.app.tool.is_some());
        h.step(&json!({"click":{"sketch":[midpoint.x,midpoint.y]}})).unwrap();
        assert_eq!(tx(&h.app).link, link);
        assert!(h.app.tool.is_none());
        h.step(&json!({"widget":"Cancel"})).unwrap();
    }

    #[test]
    fn issue_report_preserves_text_edit_and_owns_keyboard_input() {
        let (mut h, _, _) = setup();
        h.app.start("sketch.edit_text");
        h.frames(3);
        replace(&mut h, "dialog:Text", "Pending text");
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len());
        h.app.help.menu = true;
        h.frames(2);
        h.step(&json!({"click":{"handle":"menu:report"}})).unwrap();
        h.frames(3);
        let token = h.app.shot_token;
        crate::issue_report::receive(&mut h.app, token, std::sync::Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::GREEN; 4])));
        h.frames(3);
        let focus = egui::Id::new("issue_report_title");
        h.ctx.memory_mut(|memory| memory.request_focus(focus));
        h.frames(2);
        assert_eq!(h.ctx.memory(|memory| memory.focused()), Some(focus));
        h.step(&json!({"text":"report-only-marker"})).unwrap();
        h.step(&json!({"key":"Escape"})).unwrap();
        assert_eq!(tx(&h.app).text, "Pending text");
        assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len()), before);
        h.step(&json!({"widget":"Close draft"})).unwrap();
        ok(&mut h);
        assert!(!editing(&h.app));
        // The pending text was already on the canvas, in the dialog's one undo step.
        assert_eq!(h.app.session.undo.len(), before.1);
        let sid = h.app.session.active_sketch.unwrap();
        let sk = h.app.session.doc.sketch(sid).unwrap();
        assert!(sk.links.iter().any(|l| matches!(&l.source, LinkSource::Text { text, .. } if text == "Pending text")));
    }

    /// Selects the text `link` again (an edit makes its outlines anew).
    fn select_text(h: &mut Harness, link: &str) {
        let sid = h.app.session.active_sketch.unwrap();
        let sk = h.app.session.doc.sketch(sid).unwrap();
        let id = sk.curves[sk.link_curves(link)[0]].id.clone();
        h.app.session.selection = vec![Sel::SketchCurve { id }];
        h.frames(3);
    }

    fn exprs_of(h: &Harness, link: &str) -> solidcraft_engine::sketch::TextExprs {
        let sid = h.app.session.active_sketch.unwrap();
        let LinkSource::Text { exprs, .. } = h.app.session.doc.sketch(sid).unwrap().link(link).unwrap().source.clone() else { panic!("text source") };
        exprs
    }

    #[test]
    fn a_field_that_names_parameters_stays_an_expression_and_is_shown_as_typed() {
        let (mut h, link, _) = setup();
        h.app.run("parameters.add", json!({"name":"tall","expression":"20 mm"})).unwrap();
        h.app.run("parameters.add", json!({"name":"turn","expression":"90 deg"})).unwrap();
        h.app.start("sketch.edit_text");
        h.frames(3);
        replace(&mut h, "dialog:X", "tall + 5 mm");
        replace(&mut h, "dialog:Height", "tall / 2");
        replace(&mut h, "dialog:Angle", "turn");
        ok(&mut h);
        assert!(!editing(&h.app), "{:?}", h.app.dialog.as_ref().and_then(|d| d.error.clone()));
        let sid = h.app.session.active_sketch.unwrap();
        let LinkSource::Text { at, height, angle, exprs, .. } = h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source.clone() else {
            panic!("text source")
        };
        assert_eq!(
            (exprs.x.as_deref(), exprs.y.as_deref(), exprs.height.as_deref(), exprs.angle.as_deref()),
            (Some("tall + 5 mm"), None, Some("tall / 2"), Some("turn"))
        );
        assert!((at.x - 25.0).abs() < 1e-9 && (height - 10.0).abs() < 1e-9 && (angle - std::f64::consts::FRAC_PI_2).abs() < 1e-12);

        // Opened again, the fields show the expressions; a number typed in one of the others
        // leaves them alone.
        select_text(&mut h, &link);
        h.app.start("sketch.edit_text");
        h.frames(3);
        assert_eq!(tx(&h.app).values[0], "tall + 5 mm");
        assert_eq!((tx(&h.app).values[2].as_str(), tx(&h.app).values[3].as_str()), ("tall / 2", "turn"));
        replace(&mut h, "dialog:Y", "7");
        ok(&mut h);
        let exprs = exprs_of(&h, &link);
        assert_eq!(
            (exprs.x.as_deref(), exprs.y.as_deref(), exprs.height.as_deref(), exprs.angle.as_deref()),
            (Some("tall + 5 mm"), None, Some("tall / 2"), Some("turn"))
        );

        // A parameter changes while the saved numbers are not yet worked out again: OK on an
        // untouched dialog keeps the expressions, and the numbers follow the parameter.
        h.app.run("parameters.change", json!({"name":"tall","expression":"40 mm"})).unwrap();
        select_text(&mut h, &link);
        h.app.start("sketch.edit_text");
        h.frames(3);
        ok(&mut h);
        let LinkSource::Text { at, height, exprs, .. } = h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source.clone() else {
            panic!("text source")
        };
        assert_eq!((exprs.x.as_deref(), exprs.height.as_deref()), (Some("tall + 5 mm"), Some("tall / 2")));
        assert!((at.x - 45.0).abs() < 1e-9 && (at.y - 7.0).abs() < 1e-9 && (height - 20.0).abs() < 1e-9, "{at:?} {height}");

        // A number typed over an expression replaces it.
        select_text(&mut h, &link);
        h.app.start("sketch.edit_text");
        h.frames(3);
        replace(&mut h, "dialog:Height", "6");
        ok(&mut h);
        let exprs = exprs_of(&h, &link);
        assert_eq!((exprs.height, exprs.x.as_deref()), (None, Some("tall + 5 mm")));
        // A name that is no parameter is an error in the dialog; nothing changes.
        select_text(&mut h, &link);
        h.app.start("sketch.edit_text");
        h.frames(3);
        replace(&mut h, "dialog:Height", "nope * 2");
        h.frames(2);
        ok(&mut h);
        assert!(editing(&h.app), "the dialog stays open on a field that cannot be used");
    }

    #[test]
    fn fields_take_expressions_with_units_and_parameters() {
        let (mut h, link, _) = setup();
        h.app.run("parameters.add", json!({"name":"gap","expression":"4 mm","unit":"mm"})).unwrap();
        h.app.run("document.units", json!({"units":"in"})).unwrap();
        h.app.start("sketch.edit_text");
        h.frames(3);
        replace(&mut h, "dialog:X", "10 mm * 2 + 0.5 in");
        replace(&mut h, "dialog:Y", "gap * 3");
        replace(&mut h, "dialog:Height", "0.5");
        replace(&mut h, "dialog:Angle", "45 + 45");
        ok(&mut h);
        assert!(!editing(&h.app), "{:?}", h.app.dialog.as_ref().and_then(|d| d.error.clone()));
        let sid = h.app.session.active_sketch.unwrap();
        let LinkSource::Text { at, height, angle, .. } = h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source.clone() else {
            panic!("text source")
        };
        assert!((at.x - 32.7).abs() < 1e-9);
        assert!((at.y - 12.0).abs() < 1e-9);
        assert!((height - 12.7).abs() < 1e-9);
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn unrelated_design_changes_keep_the_dialog_open() {
        let (mut h, _, _) = setup();
        h.app.start("sketch.edit_text");
        h.frames(3);
        replace(&mut h, "dialog:Text", "Kept");
        h.app.run("sketch.line", json!({"points":[[50,0],[60,0]]})).unwrap();
        h.frames(3);
        assert!(editing(&h.app));
        assert_eq!(tx(&h.app).text, "Kept");
        // Undone: the line, the dialog's change so far, then the text itself.
        for _ in 0..3 {
            h.app.run("edit.undo", json!({})).unwrap();
        }
        h.frames(3);
        assert!(!editing(&h.app), "the text it edits is gone");
    }
    /// The text sources of the active sketch.
    fn texts(h: &Harness) -> Vec<LinkSource> {
        let sid = h.app.session.active_sketch.unwrap();
        h.app.session.doc.sketch(sid).unwrap().links.iter().filter(|l| l.kind == LinkKind::Text).map(|l| l.source.clone()).collect()
    }

    /// A sketch on XY seen from the top, with the Text tool started.
    fn text_tool() -> (Harness, usize) {
        let mut h = Harness::new();
        h.app.home.open = false;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 60.0;
        h.frames(3);
        let undo = h.app.session.undo.len();
        h.step(&json!({"start":"sketch.text"})).unwrap();
        (h, undo)
    }

    #[test]
    fn the_text_tool_places_a_text_and_types_it_in_edit_text_live() {
        let (mut h, undo) = text_tool();
        h.step(&json!({"click":{"sketch":[5,5]}})).unwrap();
        h.frames(2);
        // A placeholder is down at once, Edit Text is open on it with the text selected, and the tool has ended.
        assert!(editing(&h.app));
        assert!(h.app.tool.is_none());
        let at = solidcraft_engine::geom::Vec2::new(5.0, 5.0);
        assert!(matches!(texts(&h).as_slice(), [LinkSource::Text { text, at: a, .. }] if text == PLACEHOLDER && a.dist(at) < 1e-9));
        // Typing replaces it, and the canvas follows every key.
        h.step(&json!({"text":"Hi"})).unwrap();
        h.frames(2);
        assert!(matches!(texts(&h).as_slice(), [LinkSource::Text { text, .. }] if text == "Hi"));
        h.step(&json!({"text":"!"})).unwrap();
        h.frames(2);
        assert!(matches!(texts(&h).as_slice(), [LinkSource::Text { text, .. }] if text == "Hi!"));
        // Enter in the text is OK: one undo step for placing and typing, and the model is up to date.
        h.step(&json!({"key":"Enter"})).unwrap();
        h.frames(2);
        assert!(!editing(&h.app));
        assert!(matches!(texts(&h).as_slice(), [LinkSource::Text { text, .. }] if text == "Hi!"));
        assert_eq!(h.app.session.undo.len(), undo + 1);
        assert!(!h.app.session.model.is_stale());
        h.app.run("edit.undo", json!({})).unwrap();
        assert!(texts(&h).is_empty());
    }

    #[test]
    fn cancelling_a_placed_text_takes_it_away_with_its_history() {
        let (mut h, undo) = text_tool();
        let before = h.app.session.doc.to_json();
        h.step(&json!({"click":{"sketch":[0,0]}})).unwrap();
        h.step(&json!({"text":"Gone"})).unwrap();
        h.frames(2);
        assert_eq!(texts(&h).len(), 1);
        h.step(&json!({"widget":"Cancel"})).unwrap();
        h.frames(2);
        assert!(!editing(&h.app));
        assert_eq!(h.app.session.doc.to_json(), before);
        assert_eq!(h.app.session.undo.len(), undo);
    }

    #[test]
    fn edit_text_shows_changes_live_and_cancel_puts_the_text_back() {
        let (mut h, link, _) = setup();
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len());
        h.app.start("sketch.edit_text");
        h.frames(3);
        // Opening changes nothing.
        assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len()), before);
        replace(&mut h, "dialog:Text", "OK");
        replace(&mut h, "dialog:Height", "20");
        h.frames(2);
        let sid = h.app.session.active_sketch.unwrap();
        let source = |h: &Harness| h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source.clone();
        assert!(matches!(source(&h), LinkSource::Text { text, height, .. } if text == "OK" && height == 20.0));
        assert_eq!(h.app.session.undo.len(), before.1 + 1, "every change folds into one step");
        // The fields stay as typed while the canvas follows them.
        assert_eq!(tx(&h.app).text, "OK");
        // A value that is not one yet keeps the last good preview.
        replace(&mut h, "dialog:Height", "2 +");
        h.frames(2);
        assert!(matches!(source(&h), LinkSource::Text { height, .. } if height == 20.0));
        h.step(&json!({"widget":"Cancel"})).unwrap();
        h.frames(2);
        assert!(!editing(&h.app));
        assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len()), before);
    }

    #[test]
    fn shift_enter_starts_a_new_line_in_the_text() {
        let (mut h, link, _) = setup();
        h.app.start("sketch.edit_text");
        h.frames(3);
        h.step(&json!({"text":"A"})).unwrap();
        h.step(&json!({"key":"Enter","shift":true})).unwrap();
        h.step(&json!({"text":"B"})).unwrap();
        h.frames(2);
        assert!(editing(&h.app));
        assert_eq!(tx(&h.app).text, "A\nB");
        h.step(&json!({"key":"Enter"})).unwrap();
        h.frames(2);
        assert!(!editing(&h.app));
        let sid = h.app.session.active_sketch.unwrap();
        assert!(matches!(&h.app.session.doc.sketch(sid).unwrap().link(&link).unwrap().source, LinkSource::Text { text, .. } if text == "A\nB"));
    }

    /// "IO" (10 mm) seen from the top: the profile of the I, and those of the O's ring and its counter.
    fn letters() -> (Harness, u64, usize, usize, usize) {
        let (mut h, link, _) = setup();
        h.app.run("sketch.edit_text", json!({"link":link,"text":"IO"})).unwrap();
        h.app.session.selection.clear();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 60.0;
        h.frames(3);
        let sid = h.app.session.active_sketch.unwrap();
        let state = h.app.session.world_state();
        let profiles = &state.sketch(sid).unwrap().profiles;
        assert_eq!(profiles.len(), 3);
        let sorted = |v: &[String]| {
            let mut v = v.to_vec();
            v.sort();
            v
        };
        let ring = profiles.iter().position(|p| !p.hole_curves.is_empty()).unwrap();
        let counter = profiles.iter().position(|p| sorted(&p.outer_curves) == sorted(&profiles[ring].hole_curves[0])).unwrap();
        let i = (0..3).find(|&i| i != ring && i != counter).unwrap();
        (h, sid, i, ring, counter)
    }

    #[test]
    fn a_text_is_picked_whole_without_its_counters() {
        let (h, sid, i, ring, counter) = letters();
        let mut whole = text_profiles(&h.app.session, sid, i).unwrap();
        assert_eq!(whole[0], i, "the picked letter comes first");
        whole.sort();
        let mut want = vec![i, ring];
        want.sort();
        assert_eq!(whole, want);
        // Inside the O's counter, the text is picked too (its letters).
        assert_eq!(text_profiles(&h.app.session, sid, counter).map(|v| v.len()), Some(2));
        assert!(text_profiles(&h.app.session, sid, 99).is_none());
    }

    #[test]
    fn clicking_a_letter_selects_the_whole_text_for_an_extrude() {
        let (mut h, sid, i, ring, _) = letters();
        let at = h.app.session.world_state().sketch(sid).unwrap().profiles[i].centroid;
        h.step(&json!({"click":{"sketch":[at.x,at.y]}})).unwrap();
        h.frames(2);
        let has = |sel: &[Sel], index: usize| sel.iter().any(|s| matches!(s, Sel::Profile { sketch, index: x } if *sketch == sid && *x == index));
        assert_eq!(h.app.session.selection.len(), 2, "{:?}", h.app.session.selection);
        assert!(has(&h.app.session.selection, i) && has(&h.app.session.selection, ring));
        // Extrude picks it whole as well.
        h.app.session.selection.clear();
        h.app.finish_sketch();
        h.frames(3);
        h.step(&json!({"start":"solid.extrude"})).unwrap();
        h.frames(2);
        h.step(&json!({"click":{"world":[at.x,at.y,0.0]}})).unwrap();
        h.frames(2);
        let picked = h.app.dialog.as_ref().unwrap().inputs[0].items.clone();
        assert_eq!(picked.len(), 2, "{picked:?}");
        assert!(has(&picked, i) && has(&picked, ring));
        // Shift takes one letter out again (and would put it back alone).
        h.step(&json!({"click":{"world":[at.x,at.y,0.0]},"shift":true})).unwrap();
        h.frames(2);
        let picked = h.app.dialog.as_ref().unwrap().inputs[0].items.clone();
        assert_eq!(picked.len(), 1, "{picked:?}");
        assert!(has(&picked, ring));
    }

    #[test]
    fn shift_clicking_a_letter_selects_that_letter_alone() {
        let (mut h, sid, i, ring, _) = letters();
        let at = h.app.session.world_state().sketch(sid).unwrap().profiles[i].centroid;
        h.step(&json!({"click":{"sketch":[at.x,at.y]},"shift":true})).unwrap();
        h.frames(2);
        assert_eq!(h.app.session.selection, vec![Sel::Profile { sketch: sid, index: i }]);
        // It is shown alone too: the rest of the text is not drawn as selected.
        let shown = h.app.session.selection.iter().flat_map(|sel| crate::viewport::grouped_selection(&h.app.session, sel)).collect::<Vec<_>>();
        assert!(!shown.contains(&Sel::Profile { sketch: sid, index: ring }));
        // A plain click picks the whole text again.
        h.step(&json!({"click":{"sketch":[at.x,at.y]}})).unwrap();
        h.frames(2);
        assert_eq!(h.app.session.selection.len(), 2);
    }

    /// The style of text `link` in the active sketch.
    fn style_of(h: &Harness, link: &str) -> TextStyle {
        let sid = h.app.session.active_sketch.unwrap();
        match &h.app.session.doc.sketch(sid).unwrap().link(link).unwrap().source {
            LinkSource::Text { style, .. } => style.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn style_alignment_and_spacing_show_live_and_apply_in_one_step() {
        let (mut h, link, _) = setup();
        let undo = h.app.session.undo.len();
        h.app.start("sketch.edit_text");
        h.frames(3);
        h.step(&json!({"click":{"handle":"dialog:Italic"}})).unwrap();
        h.step(&json!({"click":{"handle":"dialog:Align center"}})).unwrap();
        replace(&mut h, "dialog:Letter Spacing", "1.5");
        replace(&mut h, "dialog:Line Spacing", "1.25");
        h.frames(2);
        // On the canvas before OK.
        let live = style_of(&h, &link);
        assert!(live.italic && !live.bold && live.align == TextAlign::Center, "{live:?}");
        assert_eq!((live.letter_spacing, live.line_spacing), (1.5, 1.25));
        ok(&mut h);
        assert!(!editing(&h.app));
        assert_eq!(style_of(&h, &link), live);
        assert_eq!(h.app.session.undo.len(), undo + 1);
        // Reopened, the dialog shows them.
        let sid = h.app.session.active_sketch.unwrap();
        h.app.session.selection = vec![Sel::SketchCurve { id: h.app.session.doc.sketch(sid).unwrap().curves[0].id.clone() }];
        h.app.start("sketch.edit_text");
        h.frames(3);
        assert_eq!(tx(&h.app).style, live);
        assert_eq!(tx(&h.app).values[4], "1.5 mm");
        assert_eq!(tx(&h.app).values[5], "1.25");
    }

    #[test]
    fn bold_in_a_font_without_a_bold_face_is_refused_with_the_reason() {
        let (mut h, link, _) = setup();
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len());
        h.app.start("sketch.edit_text");
        h.frames(3);
        h.step(&json!({"click":{"handle":"dialog:Bold"}})).unwrap();
        h.frames(2);
        assert!(!style_of(&h, &link).bold, "the canvas keeps the last text it could make");
        ok(&mut h);
        assert!(editing(&h.app));
        let error = h.app.dialog.as_ref().unwrap().error.clone().unwrap_or_default();
        assert!(error.contains("no bold face"), "{error}");
        h.step(&json!({"widget":"Cancel"})).unwrap();
        assert_eq!((h.app.session.doc.to_json(), h.app.session.undo.len()), before);
    }

    #[test]
    fn the_path_chip_takes_the_next_curve_clicked_and_the_text_follows_it() {
        let (mut h, link, curve) = setup();
        let line = h.app.run("sketch.line", json!({"points":[[-20,-30],[40,-30]]})).unwrap()["curves"][0].as_str().unwrap().to_string();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 120.0;
        h.frames(3);
        let sid = h.app.session.active_sketch.unwrap();
        h.app.session.selection = vec![Sel::SketchCurve { id: curve }];
        h.app.start("sketch.edit_text");
        h.frames(3);
        assert_eq!(style_of(&h, &link).path, None, "the selected text is not its own path");
        // A click on the canvas before the chip picks nothing.
        h.step(&json!({"click":{"sketch":[10,-30]}})).unwrap();
        h.frames(2);
        assert_eq!(tx(&h.app).style.path, None);
        // The chip, then the line: the text stands on it at once.
        h.step(&json!({"click":{"handle":"dialog:Path"}})).unwrap();
        h.step(&json!({"click":{"sketch":[10,-30]}})).unwrap();
        h.frames(3);
        assert_eq!(tx(&h.app).style.path.as_deref(), Some(line.as_str()));
        assert_eq!(style_of(&h, &link).path.as_deref(), Some(line.as_str()));
        let state = h.app.session.world_state();
        let sk = &state.sketch(sid).unwrap().sketch;
        let low = sk.link_curves(&link).into_iter().flat_map(|ci| sk.polyline(ci)).map(|p| p.y).fold(f64::MAX, f64::min);
        assert!((low + 30.0).abs() < 1e-6, "on the line: {low}");
        // Reverse, then OK: one undo step.
        h.step(&json!({"click":{"handle":"dialog:Reverse"}})).unwrap();
        h.frames(2);
        assert!(style_of(&h, &link).reverse);
        // × takes it off again.
        h.step(&json!({"click":{"handle":"dialog:Path clear"}})).unwrap();
        h.frames(2);
        assert_eq!(style_of(&h, &link).path, None);
        h.step(&json!({"widget":"Cancel"})).unwrap();
        assert_eq!(style_of(&h, &link), TextStyle::default());
    }
}
