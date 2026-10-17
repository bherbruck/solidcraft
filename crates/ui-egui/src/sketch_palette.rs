//! The Sketch Palette: a panel docked at the right while a sketch is being edited, like a
//! command dialog. Feature Options: the modes of the creation tool that is running, as icon
//! buttons. Options: the line type of the selected curves (normal, construction, centerline),
//! Look At, and the display toggles kept with the sketch (grid, snap, slice, profiles, points,
//! dimensions, constraints, projected geometry, construction geometry, 3D sketch); the sketch's
//! degrees of freedom; Finish Sketch. Toggles run `sketch.options`, line types
//! `sketch.construction` / `sketch.centerline`.
//!
//! The line type is also a drawing mode: with nothing selected, Construction (or X) makes what
//! is drawn next construction geometry until it is switched back.

use std::cell::Cell;

use egui::{Color32, RichText, Stroke, vec2};
use serde_json::{Value, json};
use solidcraft_engine::Sel;

use crate::SolveApp;
use crate::frame::Level;
use crate::theme::Tokens;

thread_local! {
    /// The drawing line type (0 normal, 1 construction, 2 centerline) and the sketch it is for.
    static MODE: Cell<(u8, Option<u64>)> = const { Cell::new((0, None)) };
    static FOLDED: Cell<bool> = const { Cell::new(false) };
    static OPTIONS_FOLDED: Cell<bool> = const { Cell::new(false) };
    static FEATURE_FOLDED: Cell<bool> = const { Cell::new(false) };
}

/// Palette width (logical px).
const WIDTH: f32 = 236.0;

/// The display toggles: option key and label. Plain nouns throughout, no "Show " prefix: the
/// checkbox itself already says whether each one is on.
///
/// Eight of these pass their English source text directly, like the rest of this file's
/// `app.tr_label(...)` calls: `Locale::label` looks up a literal English string against every
/// other English catalog entry to find its id. "Profile" and "Points" cannot do that safely:
/// both already name something else under that same lookup (`SelInput::new("Profile", ...)` /
/// `SelInput::new("Points", ...)` in `dialogs.rs`, the Extrude/Sweep/Shell/Loft and
/// point-selection dialogs' own row labels, and "Profile" is also `context_menu.rs`'s "Show
/// Profile" / "Hide Profile" action by way of the existing `ui.show_profile` key). Giving the
/// palette row the same bare text would silently repoint one of those unrelated lookups to
/// whichever translation this file happens to carry. `ui.palette_profile` and
/// `ui.palette_points` are dedicated keys instead, carrying exactly "Profile" and "Points" in
/// English; passing the id itself (`Locale::label` falls back to a direct id lookup when the
/// text it was given does not match any other entry) reaches them without going through the
/// shared text lookup at all. See `i18n::tests::id_labels_resolve_to_their_own_translation_not_an_english_fallback`
/// and `i18n_ui_tests::sketch_palette_profile_and_points_rows_show_their_own_translation` for
/// proof they resolve to the real translation in German, Japanese and Korean rather than
/// silently falling back to English.
///
/// The id-fallback check still runs the text lookup first: a future English catalog entry
/// whose value happened to be the literal string "ui.palette_profile" or "ui.palette_points"
/// would silently steal this lookup (`Locale::label`'s `ids.get(label)` would then match
/// before the id fallback ever runs). Nobody writes UI copy that looks like a dotted id, so
/// this is a remote hazard, not a likely one, but it is why these two ids are spelled out here
/// rather than reused anywhere else.
pub const TOGGLES: [(&str, &str); 10] = [
    ("grid", "Sketch Grid"),
    ("snap", "Snap"),
    ("slice", "Slice"),
    ("show_profile", "ui.palette_profile"),
    ("show_points", "ui.palette_points"),
    ("show_dimensions", "Dimensions"),
    ("show_constraints", "Constraints"),
    ("show_projected", "Projected Geometries"),
    ("show_construction", "Construction Geometries"),
    ("sketch_3d", "3D Sketch"),
];

/// The degrees-of-freedom line: "Fully constrained" or "N degrees of freedom".
pub fn dof_text(dof: usize, ok: bool) -> String {
    if !ok {
        "Constraints conflict".into()
    } else if dof == 0 {
        "Fully constrained".into()
    } else if dof == 1 {
        "1 degree of freedom".into()
    } else {
        format!("{dof} degrees of freedom")
    }
}

/// The line type new curves get in the sketch being edited (0 normal, 1 construction,
/// 2 centerline, 3 infinite); a new sketch starts normal.
pub fn mode(app: &SolveApp) -> u8 {
    let (m, sk) = MODE.with(Cell::get);
    if sk.is_some() && sk == app.session.active_sketch { m } else { 0 }
}

/// Switch the drawing line type (with a status hint).
pub fn set_mode(app: &mut SolveApp, m: u8) {
    MODE.with(|c| c.set((m, app.session.active_sketch)));
    let hint = match m {
        1 => "Construction mode: new curves are construction geometry (X or the palette's Normal to leave)",
        2 => "Centerline mode: new lines are centerlines",
        3 => "Infinite mode: new lines are infinite construction lines",
        _ => "Normal mode",
    };
    app.set_status(hint, false);
}

/// X / the toolbar's Construction: convert the selected curves, or with nothing selected
/// switch construction mode on or off.
pub fn toggle_construction(app: &mut SolveApp) {
    let curves = selected_curves(app);
    if curves.is_empty() && app.session.active_sketch.is_none() {
        app.set_status("Construction mode is for drawing in a sketch: edit a sketch first", false);
    } else if curves.is_empty() {
        let m = if mode(app) == 1 { 0 } else { 1 };
        set_mode(app, m);
    } else {
        let _ = app.run("sketch.construction", json!({ "curves": curves }));
    }
}

/// Curves a drawing command just made take the drawing line type.
pub fn apply_mode(app: &mut SolveApp, id: &str, result: &Value) {
    let m = mode(app);
    if m == 0 || !solidcraft_engine::find_command(id).is_some_and(|c| c.tab == "SKETCH" && c.panel == "CREATE") {
        return;
    }
    let curves: Vec<String> = result["curves"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_string)).collect();
    if curves.is_empty() {
        return;
    }
    let lines: Vec<String> = if m == 2 || m == 3 {
        let st = app.session.model.state();
        let Some(ss) = app.session.active_sketch.and_then(|s| st.sketch(s)) else { return };
        curves
            .iter()
            .filter(|c| {
                ss.sketch
                    .curve_index(c)
                    .and_then(|i| ss.sketch.curves.get(i))
                    .is_some_and(|c| matches!(c.kind, solidcraft_engine::sketch::CurveKind::Line { .. }))
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let rest: Vec<String> = curves.iter().filter(|c| !lines.contains(c)).cloned().collect();
    // Part of the drawing's own undo step.
    let depth = app.session.undo.len();
    if !lines.is_empty() {
        let line_type = if m == 3 { "sketch.infinite" } else { "sketch.centerline" };
        let _ = app.session.execute(line_type, &json!({ "curves": lines, "value": true }));
    }
    if !rest.is_empty() {
        let _ = app.session.execute("sketch.construction", &json!({ "curves": rest, "value": true }));
    }
    app.session.undo.truncate(depth);
}

/// Selected curves of the active sketch.
fn selected_curves(app: &SolveApp) -> Vec<String> {
    app.session.selection.iter().filter_map(|s| if let Sel::SketchCurve { id } = s { Some(id.clone()) } else { None }).collect()
}

/// The line types among the selected curves of the active sketch, indexed like the palette's
/// buttons (0 normal, 1 construction, 2 centerline, 3 infinite); a mixed selection marks each of
/// its types.
fn selected_linetypes(app: &SolveApp) -> [bool; 4] {
    let mut types = [false; 4];
    let curves = selected_curves(app);
    if curves.is_empty() {
        return types;
    }
    let st = app.session.world_state();
    let Some(ss) = app.session.active_sketch.and_then(|s| st.sketch(s)) else { return types };
    for c in curves.iter().filter_map(|id| ss.sketch.curve_index(id).and_then(|i| ss.sketch.curves.get(i))) {
        let k = if c.infinite {
            3
        } else if c.centerline {
            2
        } else if c.construction {
            1
        } else {
            0
        };
        if let Some(t) = types.get_mut(k) {
            *t = true;
        }
    }
    types
}

/// Set the line type of the selected curves: 0 normal, 1 construction, 2 centerline, 3 infinite
/// (the last two for lines).
pub fn set_linetype(app: &mut SolveApp, kind: u8) {
    let curves = selected_curves(app);
    if curves.is_empty() {
        app.set_status("Select sketch curves first", false);
        return;
    }
    let lines: Vec<String> = {
        let st = app.session.world_state();
        let Some(ss) = app.session.active_sketch.and_then(|s| st.sketch(s)) else { return };
        curves
            .iter()
            .filter(|id| {
                ss.sketch
                    .curve_index(id)
                    .and_then(|i| ss.sketch.curves.get(i))
                    .is_some_and(|c| matches!(c.kind, solidcraft_engine::sketch::CurveKind::Line { .. }))
            })
            .cloned()
            .collect()
    };
    let r = match kind {
        0 => {
            if !lines.is_empty() {
                let _ = app.run("sketch.centerline", json!({ "curves": lines, "value": false }));
            }
            app.run("sketch.construction", json!({ "curves": curves, "value": false }))
        }
        1 => {
            if !lines.is_empty() {
                let _ = app.run("sketch.infinite", json!({ "curves": lines, "value": false }));
            }
            app.run("sketch.construction", json!({ "curves": curves, "value": true }))
        }
        3 if lines.is_empty() => Err("an infinite line must be a line".to_string()),
        3 => app.run("sketch.infinite", json!({ "curves": lines, "value": true })),
        _ if lines.is_empty() => Err("a centerline must be a line".to_string()),
        _ => app.run("sketch.centerline", json!({ "curves": lines, "value": true })),
    };
    if let Err(e) = r {
        app.set_status(e, true);
    }
}

/// The palette's content width in `vp`; it gives way to the viewport on narrow views.
fn width(vp: egui::Rect) -> f32 {
    WIDTH.min((vp.width() * 0.46 - 16.0).max(120.0))
}

/// The viewport the palette docks into.
fn viewport(app: &SolveApp, ctx: &egui::Context) -> egui::Rect {
    app.viewport.rect.unwrap_or_else(|| ctx.content_rect())
}

/// Is a sketch being edited? That is the palette's own condition.
pub fn editing(app: &SolveApp) -> bool {
    app.session.active_sketch.and_then(|s| app.session.world_state().sketch(s).map(|_| ())).is_some()
}

/// Has the viewport room for the palette and a command dialog side by side? A narrow view has
/// not, and there the dialog keeps the right edge on its own.
pub fn fits_beside_a_dialog(app: &SolveApp, ctx: &egui::Context) -> bool {
    let vp = viewport(app, ctx);
    vp.width() >= width(vp) + crate::frame::DIALOG_MIN_W + 40.0
}

/// Is the palette on screen? It stays for as long as a sketch is being edited, a command dialog
/// open or not: the dialog takes the left edge of the viewport and the palette keeps the right.
pub fn visible(app: &SolveApp, ctx: &egui::Context) -> bool {
    editing(app) && (app.dialog.is_none() || fits_beside_a_dialog(app, ctx))
}

/// A small disclosure triangle for the Options section: pointing right when collapsed, down
/// when open. Kept separate from the palette's own "−"/"+" fold, which stays a plain glyph.
fn disclosure_triangle(p: &egui::Painter, c: egui::Pos2, open: bool, col: Color32) {
    let pts = if open {
        vec![egui::pos2(c.x - 4.0, c.y - 2.0), egui::pos2(c.x + 4.0, c.y - 2.0), egui::pos2(c.x, c.y + 3.0)]
    } else {
        vec![egui::pos2(c.x - 2.5, c.y - 4.0), egui::pos2(c.x - 2.5, c.y + 4.0), egui::pos2(c.x + 3.0, c.y)]
    };
    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
}

/// The creation family of the tool that is running, and the exact variant it is running, when
/// that family has more than one mode to switch between.
///
/// Family membership comes from the declared table in `sketch_families`, never from the shape of
/// the id. `sketch.line` and `sketch.line.midpoint` are one family although only one of them
/// carries a variant suffix, and `sketch.ellipse`, `sketch.conic` and `sketch.text` carry no
/// family at all. The table also fixes the order the modes are shown in, which a derived rule
/// cannot express.
///
/// A tool outside a family has nothing to switch to, so the section stays hidden rather than
/// showing a single button that does nothing. The palette's vertical space is already budgeted
/// down to the footer, so an inert row would cost the Options list real height.
fn active_feature(app: &SolveApp) -> Option<(&'static crate::sketch_families::Family, &'static str)> {
    let id = app.tool.as_ref()?.cmd;
    let family = crate::sketch_families::family(id)?;
    (family.commands.len() > 1).then_some((family, id))
}

/// One row of the Feature Options section: the family's name, then its modes as icon buttons
/// right-aligned in the same column every other palette row ends at. Five buttons do not fit one
/// palette-wide row, so the group wraps onto as many rows as it needs.
fn feature_row(app: &SolveApp, ui: &mut egui::Ui, width: f32, family: &crate::sketch_families::Family, current: &str) -> Option<&'static str> {
    let t = Tokens::get();
    let mut start = None;
    ui.horizontal(|ui| {
        ui.label(app.tr_label(family.label));
        let btn_w = 28.0;
        let btn_h = 22.0;
        let sp = ui.spacing().item_spacing.x;
        let room = (width - ui.min_rect().width() - sp).max(btn_w);
        let fits = ((room + sp) / (btn_w + sp)).floor().clamp(1.0, family.commands.len() as f32) as usize;
        let per_row = fits.max(1);
        let group_w = per_row as f32 * btn_w + per_row.saturating_sub(1) as f32 * sp;
        crate::frame::right_aligned(ui, width, group_w, |ui| {
            ui.vertical(|ui| {
                for chunk in family.commands.chunks(per_row) {
                    ui.horizontal(|ui| {
                        for spec in chunk.iter().filter_map(|id| solidcraft_engine::find_command(id)) {
                            let on = spec.id == current;
                            let enabled = spec.info(&app.session).enabled;
                            let resp = ui.add_enabled(enabled, egui::Button::new("").selected(on).min_size(vec2(btn_w, btn_h)));
                            let side = (resp.rect.width().min(resp.rect.height()) - 4.0).max(12.0);
                            let icon_rect = egui::Rect::from_center_size(resp.rect.center(), vec2(side, side));
                            let (ink, fill, acc) = if enabled { (t.icon, t.icon_fill, t.accent) } else { (t.border, t.panel_header, t.border) };
                            crate::icons::paint(ui.painter(), icon_rect, spec.icon, ink, fill, acc);
                            crate::scenario::publish_handle(&format!("sketch_palette:feature:{}", spec.id), resp.rect.center());
                            // The button draws an icon and no text of its own, so give a screen
                            // reader the command's own name and whether it is the mode in use.
                            let label = app.tr_label(spec.label).to_string();
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, &label));
                            if resp.on_hover_text(app.tr_label(spec.label)).clicked() && enabled {
                                start = Some(spec.id);
                            }
                        }
                    });
                }
            });
        });
    });
    start
}

/// Draw the palette (while a sketch is being edited).
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    app.viewport.palette_rect = None;
    let _language = crate::i18n::scope(&app.preferences.language);
    if !visible(app, ctx) {
        return;
    }
    let (opts, dof_line, ok, open_ends, untied_ends) = {
        let st = app.session.world_state();
        let Some(ss) = app.session.active_sketch.and_then(|s| st.sketch(s)) else { return };
        (
            ss.sketch.view.clone(),
            dof_text(ss.report.dof, ss.report.ok()),
            ss.report.ok() && ss.report.dof == 0,
            ss.ends.open.len(),
            ss.ends.untied.len(),
        )
    };
    let t = Tokens::get();
    let vp = viewport(app, ctx);
    let bottom = vp.bottom().min(ctx.content_rect().bottom()) - 8.0;
    // Prefer the space below Axis View, but reclaim it on short/HiDPI views.
    let top = (vp.top() + crate::viewport::AXIS_VIEW_CLEARANCE).min((bottom - 360.0).max(vp.top()));
    let anchor = egui::pos2(vp.right(), top);
    let frame = crate::frame::dock_frame(ctx, crate::frame::Side::Right);
    let shown = |k: &str| match k {
        "grid" => !opts.hide_grid,
        "snap" => !opts.no_snap,
        "slice" => opts.slice,
        "show_profile" => !opts.hide_profile,
        "show_points" => !opts.hide_points,
        "show_dimensions" => !opts.hide_dimensions,
        "show_constraints" => !opts.hide_constraints,
        "show_projected" => !opts.hide_projected,
        "show_construction" => !opts.hide_construction,
        "sketch_3d" => opts.three_d,
        _ => false,
    };
    let mut set: Option<(String, bool)> = None;
    let mut linetype: Option<u8> = None;
    let mut look = false;
    let mut finish = false;
    let mut tie = false;
    let mut start: Option<&'static str> = None;
    let feature = active_feature(app);
    let has_curves = !selected_curves(app).is_empty();
    let selected_types = selected_linetypes(app);
    let margin_bottom = f32::from(frame.inner_margin.bottom);
    let response = crate::frame::docked(
        app.tr("ui.heading_sketch_palette"),
        egui::Id::new("sc_sketch_palette"),
        anchor,
        bottom - anchor.y,
        crate::frame::Width::Normal,
    )
    .frame(frame)
    .show(ctx, |ui| {
        let width = width(vp);
        ui.set_width(width);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        let line = ui.text_style_height(&egui::TextStyle::Body);
        let gap = ui.spacing().item_spacing.y;
        crate::frame::heading(ui, Level::Title, width, |ui| {
            let folded = FOLDED.with(Cell::get);
            if ui.add(egui::Button::new(RichText::new(if folded { "+" } else { "−" }).size(14.0)).frame(false)).clicked() {
                FOLDED.with(|f| f.set(!folded));
            }
            ui.label(RichText::new(app.tr("ui.heading_sketch_palette")).strong().size(12.5));
        });
        if FOLDED.with(Cell::get) {
            return;
        }
        // Feature Options sits above Options: the modes of the creation tool that is running.
        if let Some((family, current)) = feature {
            let ffold = FEATURE_FOLDED.with(Cell::get);
            crate::frame::heading(ui, Level::Section, width, |ui| {
                let (tri_rect, tri_resp) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::click());
                disclosure_triangle(ui.painter(), tri_rect.center(), !ffold, t.text);
                crate::scenario::publish_handle("sketch_palette:feature_fold", tri_rect.center());
                let label_resp = ui.add(egui::Label::new(RichText::new(app.tr("ui.feature_options")).size(12.0)).sense(egui::Sense::click()));
                if tri_resp.clicked() || label_resp.clicked() {
                    FEATURE_FOLDED.with(|f| f.set(!ffold));
                }
            });
            if !ffold {
                let section = ui.vertical(|ui| feature_row(app, ui, width, family, current));
                start = section.inner;
                crate::scenario::publish_rect("sketch_palette:feature_options", section.response.rect);
            }
            ui.add_space(2.0);
        }
        let ofold = OPTIONS_FOLDED.with(Cell::get);
        crate::frame::heading(ui, Level::Section, width, |ui| {
            let (tri_rect, tri_resp) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::click());
            disclosure_triangle(ui.painter(), tri_rect.center(), !ofold, t.text);
            crate::scenario::publish_handle("sketch_palette:options_fold", tri_rect.center());
            let label_resp = ui.add(egui::Label::new(RichText::new(app.tr("ui.options")).size(12.0)).sense(egui::Sense::click()));
            if tri_resp.clicked() || label_resp.clicked() {
                OPTIONS_FOLDED.with(|f| f.set(!ofold));
            }
        });
        if !ofold {
            // Keep room for the footer: status heading and line, a line, Finish Sketch, a line.
            let ends_rows = if open_ends > 0 { line + gap } else { 0.0 } + if untied_ends > 0 { ui.spacing().interact_size.y + gap } else { 0.0 };
            let footer = 5.0 * line + ui.spacing().interact_size.y + 6.0 * gap + 8.0 + ends_rows;
            let options_height = (bottom - ui.cursor().top() - footer).max(24.0);
            let options =
                egui::ScrollArea::vertical().id_salt("sc_palette_options").max_height(options_height).auto_shrink([false, true]).show(ui, |ui| {
                    // Every row below: its label at the left, its control right-aligned at the
                    // same column (`frame::right_aligned`), so every row lines up instead of
                    // each control sitting wherever its label happened to end.
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(app.tr("ui.linetype"));
                            // Taller and wider than the old text-only buttons. Construction and
                            // Centerline need a round dot and a clear gap to read apart. 30 by
                            // 20 px left too little square room for either.
                            let btn_w = 36.0;
                            let group_w = 4.0 * btn_w + 3.0 * ui.spacing().item_spacing.x;
                            crate::frame::right_aligned(ui, width, group_w, |ui| {
                                ui.horizontal(|ui| {
                                    // With a selection: its line type, and a click converts it;
                                    // without: the mode new curves are drawn in. Each button
                                    // still carries its old glyph as hidden (transparent) text.
                                    // That keeps its hit target, size and accessible name
                                    // exactly as before. A drawn icon (icons.rs) paints over it.
                                    // Construction and Centerline always draw in the theme's
                                    // construction orange. Normal's icon never changes with the
                                    // accent colour. It stays neutral in every case.
                                    let current = mode(app);
                                    for (i, (label, icon, tip)) in [
                                        ("—", "normal", "Normal"),
                                        ("- -", "construction", "Construction"),
                                        ("-·-", "centerline", "Centerline"),
                                        ("↔", "infinite", "Infinite"),
                                    ]
                                    .iter()
                                    .enumerate()
                                    {
                                        let on = if has_curves { selected_types.get(i).copied().unwrap_or(false) } else { current == i as u8 };
                                        let tr_tip = app.tr_label(tip);
                                        let hover = if has_curves {
                                            crate::i18n::format("sketch.make_selection", &[("linetype", tr_tip)])
                                        } else {
                                            crate::i18n::format("sketch.draw_curves", &[("linetype", tr_tip)])
                                        };
                                        let btn = egui::Button::new(RichText::new(*label).color(Color32::TRANSPARENT))
                                            .selected(on)
                                            .min_size(vec2(btn_w, 28.0));
                                        let resp = ui.add(btn).on_hover_text(hover);
                                        let accent = if *icon == "normal" { t.icon } else { t.construction_geom };
                                        // A square icon area. The button's own rect is
                                        // text-shaped, wider than tall. A flattened diagonal
                                        // leaves less room for a dash pattern's gaps to read.
                                        let side = (resp.rect.width().min(resp.rect.height()) - 3.0).max(12.0);
                                        let icon_rect = egui::Rect::from_center_size(resp.rect.center(), vec2(side, side));
                                        crate::icons::paint(ui.painter(), icon_rect, icon, t.icon, t.icon_fill, accent);
                                        if resp.clicked() {
                                            linetype = Some(i as u8);
                                        }
                                    }
                                });
                            });
                        });
                        ui.horizontal(|ui| {
                            // One label, one icon button (the "lookat" glyph already used on the
                            // viewport's own navigation overlay): not the word "Look At" twice.
                            // The button carries no visible text of its own; `labelled_by` gives
                            // it the row's own label as its accessible name, so a screen reader
                            // still hears "Look At" the way the old text button said it.
                            let label_resp = ui.label(app.tr("ui.look_at"));
                            let icon_w = 28.0;
                            let resp = crate::frame::right_aligned(ui, width, icon_w, |ui| {
                                let r = ui.add(egui::Button::new("").min_size(vec2(icon_w, 20.0)));
                                let side = (r.rect.width().min(r.rect.height()) - 4.0).max(12.0);
                                let icon_rect = egui::Rect::from_center_size(r.rect.center(), vec2(side, side));
                                crate::icons::paint(ui.painter(), icon_rect, "lookat", t.icon, t.icon_fill, t.accent);
                                crate::scenario::publish_handle("sketch_palette:look_at", r.rect.center());
                                r.labelled_by(label_resp.id)
                            });
                            let resp = resp.on_hover_text(app.tr("ui.look_at"));
                            if resp.clicked() {
                                look = true;
                            }
                        });
                    });
                    for (k, label) in TOGGLES {
                        ui.horizontal(|ui| {
                            // The label is still part of the toggle: clicking it flips the
                            // checkbox too, same as before the checkbox moved to its own column.
                            let label_resp = ui.add(egui::Label::new(app.tr_label(label)).sense(egui::Sense::click()));
                            let mut on = shown(k);
                            let box_w = ui.spacing().interact_size.y;
                            let cb = crate::frame::right_aligned(ui, width, box_w, |ui| {
                                ui.add(egui::Checkbox::without_text(&mut on)).labelled_by(label_resp.id)
                            });
                            if cb.changed() {
                                set = Some((k.to_string(), on));
                            } else if label_resp.clicked() {
                                set = Some((k.to_string(), !on));
                            }
                        });
                    }
                });
            crate::scenario::publish_rect("sketch_palette:options", options.inner_rect);
        }
        ui.add_space(4.0);
        // The status changes when the sketch needs attention, so it gets its own heading.
        crate::frame::heading(ui, Level::Section, width, |ui| {
            ui.label(RichText::new(app.tr("ui.sketch_status")).size(12.0));
        });
        ui.label(RichText::new(crate::i18n::tr_dyn(&dof_line)).color(if ok { t.sketch_accent } else { t.text_dim }));
        // Where a profile can fail (the viewport rings and diamonds them).
        if open_ends > 0 {
            let text = crate::i18n::format("sketch.open_ends", &[("count", &open_ends.to_string())]);
            ui.label(RichText::new(text).color(t.warning)).on_hover_text(app.tr("ui.open_ends_tip"));
        }
        if untied_ends > 0 {
            ui.horizontal(|ui| {
                let text = crate::i18n::format("sketch.untied_ends", &[("count", &untied_ends.to_string())]);
                ui.label(RichText::new(text).color(t.warning)).on_hover_text(app.tr("ui.untied_ends_tip"));
                let button = ui.button(app.tr("ui.tie_ends")).on_hover_text(app.tr("ui.tie_ends_tip"));
                crate::scenario::publish_handle("sketch_palette:tie_ends", button.rect.center());
                if button.clicked() {
                    tie = true;
                }
            });
        }
        // One line between the status and Finish Sketch, and below it to the edge.
        ui.add_space((line - gap).max(0.0));
        // A row as tall as the button, so the gap below it is just the line.
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 0.0), egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let button = ui.add(egui::Button::new(RichText::new(app.tr("ui.finish_sketch")).color(Color32::WHITE)).fill(t.accent));
            crate::scenario::publish_rect("sketch_palette:finish", button.rect);
            crate::scenario::publish_handle("sketch_palette:finish", button.rect.center());
            if button.clicked() {
                finish = true;
            }
        });
        ui.add_space((line - margin_bottom).max(0.0));
    });
    app.viewport.palette_rect = response.map(|r| r.response.rect);
    if let Some(r) = app.viewport.palette_rect {
        crate::scenario::publish_rect("sketch_palette:outer", r);
    }
    if let Some((k, on)) = set {
        let mut p = serde_json::Map::new();
        p.insert(k, Value::Bool(on));
        if let Err(e) = app.run("sketch.options", Value::Object(p)) {
            app.set_status(e, true);
        }
    }
    if let Some(k) = linetype {
        if has_curves {
            set_linetype(app, k);
        } else {
            set_mode(app, k);
        }
    }
    if let Some(id) = start {
        // The same path a toolbar click takes. It drops the half-drawn shape and starts the
        // chosen mode from its first point, so a partly placed 3-point rectangle does not
        // carry its stale points into a 2-point one.
        app.start(id);
    }
    if look {
        crate::dialogs::look_at_sketch_again(app);
    }
    if tie && let Err(e) = app.run("sketch.loose_ends.tie", json!({})) {
        app.set_status(e, true);
    }
    if finish {
        app.finish_sketch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_shortcut_keeps_the_drawing_tool_and_commits_its_line_type() {
        for command in ["sketch.line", "sketch.rectangle.two_point", "sketch.circle.center"] {
            let mut h = crate::scenario::Harness::new();
            h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
            set_mode(&mut h.app, 0);
            h.step(&json!({"start":command})).unwrap();
            h.step(&json!({"key":"X"})).unwrap();
            assert_eq!(h.app.tool.as_ref().map(|t| t.cmd), Some(command));
            assert_eq!(mode(&h.app), 1);
            h.step(&json!({"click":{"sketch":[0,0]}})).unwrap();
            h.step(&json!({"move":{"sketch":[20,10]}})).unwrap();
            let before = h.app.tool.clone().unwrap();
            let history = h.app.session.undo.len();
            // The toolbar takes the same start path, even while a dimension box has focus.
            h.app.start("sketch.construction");
            h.app.start("sketch.construction");
            let after = h.app.tool.as_ref().unwrap();
            assert_eq!(after.cmd, before.cmd);
            assert_eq!(after.pts, before.pts);
            assert_eq!(after.dims_stage, before.dims_stage);
            assert_eq!(after.dims.iter().map(|d| &d.value).collect::<Vec<_>>(), before.dims.iter().map(|d| &d.value).collect::<Vec<_>>());
            assert_eq!(h.app.session.undo.len(), history);
            h.step(&json!({"click":{"sketch":[20,10]}})).unwrap();
            let state = h.app.session.world_state();
            let sketch = &state.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch;
            assert!(!sketch.curves.is_empty(), "{command}");
            assert!(sketch.curves.iter().all(|c| c.construction), "{command}");
            h.app.run("edit.undo", json!({})).unwrap();
            assert!(h.app.session.world_state().sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch.curves.is_empty());
            set_mode(&mut h.app, 0);
        }
    }

    #[test]
    fn palette_footer_and_scrolled_lower_option_remain_reachable_at_scale_and_resize() {
        for (scale, height) in [(1.0, 900.0), (2.0, 900.0), (2.0, 720.0)] {
            FOLDED.with(|v| v.set(false));
            OPTIONS_FOLDED.with(|v| v.set(false));
            let mut h = crate::scenario::Harness::new();
            h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
            h.app.run("sketch.circle.center", json!({"center":[0,0],"radius":1})).unwrap();
            h.call("ui.resize", json!({"width":1400,"height":height}));
            h.call("ui.prefs", json!({"ui_scale":scale}));
            h.settle();
            let baseline = h.app.session.doc.to_json();
            let undo = h.app.session.undo.len();
            let vp = h.app.viewport.rect.unwrap();
            let footer = crate::scenario::rect_of("sketch_palette:finish").unwrap();
            assert!(vp.contains_rect(footer), "scale{scale} height{height}: {footer:?} outside {vp:?}");
            let options = crate::scenario::rect_of("sketch_palette:options").unwrap();
            assert!(vp.contains_rect(options));
            h.call("ui.scroll", json!({"x":options.center().x,"y":options.center().y,"delta":-1000}));
            h.settle();
            assert_eq!(h.app.session.doc.to_json(), baseline);
            assert_eq!(h.app.session.undo.len(), undo);
            let [x, y] = h.widget("3D Sketch").unwrap();
            assert!(options.contains(egui::pos2(x as f32, y as f32)), "lower option must be visible after scrolling");
            h.call("ui.click", json!({"x":x,"y":y}));
            h.settle();
            assert!(h.app.session.world_state().sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch.view.three_d);
            assert_eq!(h.app.session.undo.len(), undo + 1);
            h.app.run("edit.undo", json!({})).unwrap();
            assert_eq!(h.app.session.doc.to_json(), baseline);
            h.settle();
            let footer = crate::scenario::rect_of("sketch_palette:finish").unwrap();
            h.call("ui.click", json!({"x":footer.center().x,"y":footer.center().y}));
            h.settle();
            assert!(h.app.session.active_sketch.is_none(), "Finish Sketch must work at scale{scale} height{height}");
            assert_eq!(h.app.session.doc.to_json(), baseline);
        }
    }

    /// A command dialog opened while sketching used to hide the palette outright. The two now
    /// share the viewport, the dialog at the left edge and the palette at the right, and a view
    /// too narrow for both keeps the dialog alone.
    #[test]
    fn a_dialog_opened_while_sketching_keeps_the_palette_beside_it() {
        FOLDED.with(|v| v.set(false));
        OPTIONS_FOLDED.with(|v| v.set(false));
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[40,30]})).unwrap();
        h.call("ui.resize", json!({"width":1400,"height":900}));
        h.settle();
        assert!(h.app.viewport.palette_rect.is_some(), "the palette is there while sketching");
        h.app.start("inspect.measure");
        h.settle();
        assert!(h.app.dialog.is_some(), "the command opens a dialog");
        let palette = h.app.viewport.palette_rect.expect("the palette stays on screen while a dialog is open");
        let dialog = h.app.viewport.dialog_rect.unwrap();
        assert!(!palette.intersect(dialog).is_positive(), "palette {palette:?} overlaps dialog {dialog:?}");
        assert!(dialog.left() < palette.left(), "the dialog takes the left edge: {dialog:?}, {palette:?}");
        let finish = crate::scenario::rect_of("sketch_palette:finish").expect("Finish Sketch stays reachable");
        assert!(palette.contains_rect(finish));
        // Neither panel ever covers the other, however narrow the view gets.
        for width in [1200.0, 1000.0, 860.0, 720.0, 600.0] {
            h.call("ui.resize", json!({"width":width,"height":800}));
            h.settle();
            if let (Some(p), Some(d)) = (h.app.viewport.palette_rect, h.app.viewport.dialog_rect) {
                assert!(!p.intersect(d).is_positive(), "width {width}: palette {p:?} overlaps dialog {d:?}");
            }
            assert!(h.app.dialog.is_some(), "width {width}: the dialog stays open");
        }
    }

    #[test]
    fn linetype_buttons_follow_the_selection_and_fall_back_to_the_mode() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        let curve = |r: Value| r["curves"][0].as_str().unwrap().to_string();
        let normal = curve(h.app.run("sketch.line", json!({"points":[[0,0],[20,0]],"infer":false,"connect":false})).unwrap());
        let cons = curve(h.app.run("sketch.line", json!({"points":[[0,10],[20,10]],"infer":false,"connect":false})).unwrap());
        let center = curve(h.app.run("sketch.line", json!({"points":[[0,20],[20,20]],"infer":false,"connect":false})).unwrap());
        h.app.run("sketch.construction", json!({"curves":[cons],"value":true})).unwrap();
        h.app.run("sketch.centerline", json!({"curves":[center],"value":true})).unwrap();
        // Which of Normal, Construction and Centerline the palette shows pressed.
        let pressed = |h: &mut crate::scenario::Harness, ids: &[&String]| {
            h.app.session.selection = ids.iter().map(|id| Sel::SketchCurve { id: (*id).clone() }).collect();
            h.settle();
            let tree = h.output.accesskit_update.as_ref().unwrap();
            ["—", "- -", "-·-"]
                .map(|label| tree.nodes.iter().any(|(_, n)| n.label() == Some(label) && n.toggled() == Some(egui::accesskit::Toggled::True)))
        };
        assert_eq!(pressed(&mut h, &[&normal]), [true, false, false]);
        assert_eq!(pressed(&mut h, &[&cons]), [false, true, false]);
        assert_eq!(pressed(&mut h, &[&center]), [false, false, true]);
        // A mixed selection shows each of its line types.
        assert_eq!(pressed(&mut h, &[&normal, &center]), [true, false, true]);
        // Nothing selected: the drawing mode.
        assert_eq!(pressed(&mut h, &[]), [true, false, false]);
        set_mode(&mut h.app, 1);
        assert_eq!(pressed(&mut h, &[]), [false, true, false]);
        // A selection overrides the mode until it is cleared.
        assert_eq!(pressed(&mut h, &[&normal]), [true, false, false]);
        assert_eq!(pressed(&mut h, &[]), [false, true, false]);
    }

    #[test]
    fn look_at_is_one_label_and_an_icon_button_that_animates_the_camera() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.settle();
        // Point the camera somewhere clearly not looking straight down the sketch plane, so the
        // click's effect is unambiguous regardless of where `sketch.create` happened to leave
        // it. `Harness::call` settles any camera animation to completion before it returns, so
        // checking `cam_anim` right after the click (as an earlier version of this test did)
        // never sees it: the animation is always finished and cleared by then. Check where the
        // camera actually ended up instead.
        h.app.cam.yaw = 1.0;
        h.app.cam.pitch = 0.2;
        h.settle();
        // The row's own label is the only place the words "Look At" are drawn; the control
        // next to it is an icon button with no visible text of its own.
        assert!(h.widget("Look At").is_some(), "the row label must still be reachable by name");
        let at = crate::scenario::handle_at("sketch_palette:look_at").unwrap();
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        let (yaw, pitch) = (h.app.cam.yaw, h.app.cam.pitch);
        assert!((pitch - std::f64::consts::FRAC_PI_2).abs() < 0.01, "must look straight down the XY sketch plane: pitch={pitch}");
        assert!(yaw.abs() < 0.01, "must align with the sketch's own x axis: yaw={yaw}");
    }

    #[test]
    fn status_counts_loose_ends_and_tie_closes_the_untied_ones() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        // A square of separate lines (closed by position only) and one lone line (two open ends).
        let c = [[0, 0], [10, 0], [10, 10], [0, 10]];
        for i in 0..4 {
            h.app.run("sketch.line", json!({"points": [c[i], c[(i + 1) % 4]], "infer": false, "connect": false})).unwrap();
        }
        h.app.run("sketch.line", json!({"points": [[20, 0], [30, 5]], "infer": false, "connect": false})).unwrap();
        h.settle();
        assert!(h.widget("Open 2").is_some(), "the status counts the open ends");
        assert!(h.widget("Untied 4").is_some(), "the status counts the untied ends");
        let at = crate::scenario::handle_at("sketch_palette:tie_ends").unwrap();
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        h.settle();
        assert!(h.widget("Untied 4").is_none(), "Tie constrains every untied end");
        assert!(h.widget("Open 2").is_some(), "open ends have nothing to tie to");
        let r = h.app.run("sketch.loose_ends", json!({})).unwrap();
        assert!(r["untied"].as_array().is_some_and(Vec::is_empty), "{r}");
    }

    #[test]
    fn options_disclosure_triangle_folds_and_unfolds_the_section() {
        OPTIONS_FOLDED.with(|v| v.set(false));
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.settle();
        assert!(h.widget("Sketch Grid").is_some(), "options are open by default");
        let at = crate::scenario::handle_at("sketch_palette:options_fold").unwrap();
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        h.settle();
        assert!(h.widget("Sketch Grid").is_none(), "the triangle must fold the options section");
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        h.settle();
        assert!(h.widget("Sketch Grid").is_some(), "the triangle must unfold it again");
        OPTIONS_FOLDED.with(|v| v.set(false));
    }

    #[test]
    fn toggle_row_label_and_checkbox_both_flip_the_option() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.settle();
        let active = h.app.session.active_sketch.unwrap();
        let slice_on = |h: &mut crate::scenario::Harness| h.app.session.world_state().sketch(active).unwrap().sketch.view.slice;
        assert!(!slice_on(&mut h));
        // Clicking the row's label (not just the checkbox) toggles the option, same as before
        // the checkbox and the label became separate widgets in their own column.
        let [x, y] = h.widget("Slice").unwrap();
        h.call("ui.click", json!({"x": x, "y": y}));
        h.settle();
        assert!(slice_on(&mut h), "clicking the \"Slice\" label must toggle it on");
    }

    /// The modes the Feature Options row draws pressed, in family order.
    fn pressed_modes(h: &crate::scenario::Harness, family: &crate::sketch_families::Family) -> Vec<&'static str> {
        let tree = h.output.accesskit_update.as_ref().unwrap();
        family
            .commands
            .iter()
            .copied()
            .filter(|id| {
                let label = solidcraft_engine::find_command(id).unwrap().label;
                tree.nodes.iter().any(|(_, n)| n.label() == Some(label) && n.toggled() == Some(egui::accesskit::Toggled::True))
            })
            .collect()
    }

    #[test]
    fn feature_options_show_every_mode_of_the_running_tool_and_switch_between_them() {
        FOLDED.with(|v| v.set(false));
        FEATURE_FOLDED.with(|v| v.set(false));
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.start("sketch.circle.center");
        h.settle();
        let circle = crate::sketch_families::family("sketch.circle.center").unwrap();
        assert_eq!(circle.commands.len(), 5, "the Circle family is the five circle modes");
        for id in circle.commands {
            assert!(crate::scenario::handle_at(&format!("sketch_palette:feature:{id}")).is_some(), "{id} needs a mode button");
        }
        assert!(h.widget("Circle").is_some(), "the row names the family");
        assert_eq!(pressed_modes(&h, circle), ["sketch.circle.center"]);
        let at = crate::scenario::handle_at("sketch_palette:feature:sketch.circle.three_tangent").unwrap();
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        h.settle();
        assert_eq!(h.app.tool.as_ref().map(|t| t.cmd), Some("sketch.circle.three_tangent"));
        assert_eq!(pressed_modes(&h, circle), ["sketch.circle.three_tangent"]);
        // Another family replaces the row with its own modes.
        h.app.start("sketch.rectangle.two_point");
        h.settle();
        let rect = crate::sketch_families::family("sketch.rectangle.two_point").unwrap();
        assert_eq!(rect.commands.len(), 4);
        assert_eq!(pressed_modes(&h, rect), ["sketch.rectangle.two_point"]);
        assert!(crate::scenario::handle_at("sketch_palette:feature:sketch.circle.center").is_none(), "only the running family is shown");
    }

    #[test]
    fn switching_mode_mid_draw_restarts_the_tool_and_leaves_the_design_alone() {
        FOLDED.with(|v| v.set(false));
        FEATURE_FOLDED.with(|v| v.set(false));
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.start("sketch.rectangle.three_point");
        h.step(&json!({"click":{"sketch":[0,0]}})).unwrap();
        h.settle();
        assert_eq!(h.app.tool.as_ref().map(|t| t.pts.len()), Some(1), "one corner is placed");
        let baseline = h.app.session.doc.to_json();
        let undo = h.app.session.undo.len();
        let at = crate::scenario::handle_at("sketch_palette:feature:sketch.rectangle.two_point").unwrap();
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        h.settle();
        let tool = h.app.tool.as_ref().unwrap();
        assert_eq!(tool.cmd, "sketch.rectangle.two_point");
        assert!(tool.pts.is_empty(), "the half-drawn rectangle is dropped, not carried into the new mode");
        assert_eq!(h.app.session.doc.to_json(), baseline, "switching modes never changes the design");
        assert_eq!(h.app.session.undo.len(), undo, "switching modes adds no undo step");
    }

    #[test]
    fn feature_options_stay_hidden_without_a_tool_that_has_other_modes() {
        FOLDED.with(|v| v.set(false));
        FEATURE_FOLDED.with(|v| v.set(false));
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.settle();
        assert!(crate::scenario::rect_of("sketch_palette:feature_options").is_none(), "no creation tool, no section");
        // Ellipse has no siblings to switch to, so a section would hold one inert button.
        h.app.start("sketch.ellipse");
        h.settle();
        assert!(crate::sketch_families::family("sketch.ellipse").is_none());
        assert!(crate::scenario::rect_of("sketch_palette:feature_options").is_none(), "a tool outside a family shows no section");
        h.app.start("sketch.arc.three_point");
        h.settle();
        assert!(crate::scenario::rect_of("sketch_palette:feature_options").is_some(), "a family with siblings shows the section");
        // Finishing the tool takes the section away again.
        h.step(&json!({"key":"Escape"})).unwrap();
        h.settle();
        assert!(h.app.tool.is_none());
        assert!(crate::scenario::rect_of("sketch_palette:feature_options").is_none());
    }

    #[test]
    fn feature_options_disclosure_triangle_folds_and_unfolds_the_section() {
        FOLDED.with(|v| v.set(false));
        FEATURE_FOLDED.with(|v| v.set(false));
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.start("sketch.slot.center_to_center");
        h.settle();
        let button = "sketch_palette:feature:sketch.slot.arc_center";
        assert!(crate::scenario::handle_at(button).is_some());
        let at = crate::scenario::handle_at("sketch_palette:feature_fold").unwrap();
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        h.settle();
        assert!(crate::scenario::handle_at(button).is_none(), "the triangle must fold the section");
        assert!(h.widget("Sketch Grid").is_some(), "folding Feature Options leaves Options open");
        let at = crate::scenario::handle_at("sketch_palette:feature_fold").unwrap();
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        h.settle();
        assert!(crate::scenario::handle_at(button).is_some(), "the triangle must unfold it again");
        FEATURE_FOLDED.with(|v| v.set(false));
    }

    #[test]
    fn dof_readout() {
        assert_eq!(dof_text(0, true), "Fully constrained");
        assert_eq!(dof_text(1, true), "1 degree of freedom");
        assert_eq!(dof_text(4, true), "4 degrees of freedom");
        assert_eq!(dof_text(4, false), "Constraints conflict");
    }
}
