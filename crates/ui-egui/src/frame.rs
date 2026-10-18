//! The one place windows are made. Every command dialog, panel and pop-up window comes from
//! here, so they all shrink to their content within the same width limits (a test checks that no
//! other file creates an `egui::Window`, and that every dialog stays within the limits).
//!
//! - [`docked`]: command dialogs and command panels, flush to the right edge of the viewport.
//! - [`docked_on`]: the same, against either edge ([`Side`]), with [`dock_frame`] for its
//!   chrome. A dialog opened while the Sketch Palette holds the right edge takes the left.
//! - [`window`]: floating windows (Preferences, Parameters, prompts…), centred by default.
//! - [`right_aligned`]: right-align a row's last items without widening the window (a
//!   right-to-left layout would stretch it to its maximum width).
//! - [`heading`]: a panel's title row and its section headings, each on a band darker than the
//!   panel ([`Level`]), so every dialog marks them the same way.

use egui::{Align2, Pos2, Ui, WidgetText, vec2};

/// Width limits of a docked command dialog.
pub const DIALOG_MIN_W: f32 = 260.0;
pub const DIALOG_MAX_W: f32 = 380.0;
/// Docked dialogs holding a table (motion study, configurations, exploded views).
pub const DIALOG_WIDE_MAX_W: f32 = 560.0;
/// Floating windows: prompts and panels, and those holding a table (Parameters, Keyboard
/// Shortcuts).
pub const WINDOW_MAX_W: f32 = 460.0;
pub const WINDOW_WIDE_MAX_W: f32 = 760.0;

/// How wide a window may get.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Normal,
    Wide,
}

/// Which edge of the viewport a docked panel sits against. The Sketch Palette holds the right
/// edge while a sketch is open, so a dialog started there takes the left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Right,
    Left,
}

impl Side {
    /// The x of the viewport edge this side docks against.
    pub fn anchor_x(self, viewport: egui::Rect) -> f32 {
        match self {
            Side::Right => viewport.right(),
            Side::Left => viewport.left(),
        }
    }
}

/// The window frame of a docked panel: the two corners away from its edge are rounded, and the
/// shadow falls the same way, so a left dock mirrors a right one.
pub fn dock_frame(ctx: &egui::Context, side: Side) -> egui::Frame {
    let t = crate::theme::Tokens::get();
    let (corner_radius, shadow_x) = match side {
        Side::Right => (egui::CornerRadius { nw: t.radius_sm, sw: t.radius_sm, ne: 0, se: 0 }, -2),
        Side::Left => (egui::CornerRadius { nw: 0, sw: 0, ne: t.radius_sm, se: t.radius_sm }, 2),
    };
    egui::Frame::window(&ctx.global_style())
        .fill(t.dialog_bg)
        .stroke(egui::Stroke::new(1.0, t.dialog_border))
        .corner_radius(corner_radius)
        .shadow(egui::Shadow { offset: [shadow_x, 2], blur: 8, spread: 0, color: egui::Color32::from_black_alpha(40) })
}

/// A command dialog docked at `anchor` (its top right corner), shrink-wrapped to its content.
pub fn docked<'a>(title: impl Into<WidgetText>, id: egui::Id, anchor: Pos2, max_height: f32, width: Width) -> egui::Window<'a> {
    docked_on(title, id, anchor, max_height, width, Side::Right)
}

/// A command dialog docked against `side` at `anchor` (its top corner on that side),
/// shrink-wrapped to its content.
pub fn docked_on<'a>(title: impl Into<WidgetText>, id: egui::Id, anchor: Pos2, max_height: f32, width: Width, side: Side) -> egui::Window<'a> {
    let max = match width {
        Width::Normal => DIALOG_MAX_W,
        Width::Wide => DIALOG_WIDE_MAX_W,
    };
    let pivot = match side {
        Side::Right => Align2::RIGHT_TOP,
        Side::Left => Align2::LEFT_TOP,
    };
    egui::Window::new(title)
        .enabled(!crate::issue_report::cad_input_blocked())
        .id(id)
        .title_bar(false)
        .pivot(pivot)
        .fixed_pos(anchor)
        .constrain(false)
        .auto_sized()
        .collapsible(false)
        .max_width(max)
        .max_height(max_height.max(120.0))
}

/// How far the open docked dialog titled `title` was dragged from its docked place (#1335);
/// zero for a dialog that was not dragged, or once another dialog opens.
pub fn dock_offset(ctx: &egui::Context, title: &str) -> egui::Vec2 {
    ctx.data(|d| d.get_temp::<(String, egui::Vec2)>(egui::Id::new("sc_dock_offset")))
        .filter(|(t, _)| t == title)
        .map(|(_, v)| v)
        .unwrap_or(egui::Vec2::ZERO)
}

/// Forget a dragged dialog's place (it closed): the next one opens docked.
pub fn reset_dock_offset(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<(String, egui::Vec2)>(egui::Id::new("sc_dock_offset")));
}

/// A docked dialog's anchor moved by `offset`, kept so its docked corner stays in `vp` with
/// room for its title band.
pub fn offset_anchor(docked: Pos2, offset: egui::Vec2, vp: egui::Rect, side: Side) -> Pos2 {
    let p = docked + offset;
    let (lo, hi) = match side {
        Side::Right => (vp.left() + DIALOG_MIN_W, vp.right()),
        Side::Left => (vp.left(), vp.right() - DIALOG_MIN_W),
    };
    let x = if lo <= hi { p.x.clamp(lo, hi) } else { docked.x };
    let y = if vp.top() <= vp.bottom() - 40.0 { p.y.clamp(vp.top(), vp.bottom() - 40.0) } else { docked.y };
    egui::pos2(x, y)
}

/// Let `band` (a docked dialog's title band) drag the dialog: the drag adds to its offset
/// `moved` (as last frame left it; `applied` of it is where the dialog is now, after keeping
/// it on screen). Called before the band's buttons are added, so they keep their clicks.
pub fn drag_dock(ui: &Ui, band: egui::Rect, title: &str, moved: egui::Vec2, applied: egui::Vec2) {
    let response = ui.interact(band, ui.id().with("sc_dock_drag"), egui::Sense::drag());
    crate::scenario::publish_handle("dialog:title", band.center());
    // The drag runs from where the press started, so no movement is lost while egui decides
    // it is a drag, nor on the frame it ends.
    let base = egui::Id::new("sc_dock_drag_base");
    if response.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        ui.ctx().data_mut(|d| d.insert_temp(base, (applied, origin)));
    }
    if response.dragged() || response.drag_stopped() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let start = ui.ctx().data(|d| d.get_temp::<(egui::Vec2, Pos2)>(base));
        let to = match (start, ui.input(|i| i.pointer.latest_pos())) {
            (Some((start, from)), Some(now)) => start + (now - from),
            _ => applied + response.drag_delta(),
        };
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("sc_dock_offset"), (title.to_string(), to)));
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    } else if moved != applied {
        // Kept on screen: remember where it is, not where it was dragged off to.
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("sc_dock_offset"), (title.to_string(), applied)));
    }
}

/// A floating window, centred on the screen unless the caller places it, shrink-wrapped to its
/// content up to its width limit.
pub fn window<'a>(ctx: &egui::Context, title: impl Into<WidgetText>, width: Width) -> egui::Window<'a> {
    let max = match width {
        Width::Normal => WINDOW_MAX_W,
        Width::Wide => WINDOW_WIDE_MAX_W,
    };
    let c = ctx.content_rect().center();
    egui::Window::new(title)
        .enabled(!crate::issue_report::cad_input_blocked())
        .collapsible(false)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(c - vec2(0.0, 60.0))
        .max_width(max)
}

/// Add a button drawn with its own outline (`Button::stroke`) of `width`, keeping the same size
/// in every state (#1335). egui sizes a button's padding as the spacing minus the *theme's*
/// outline width for the state (none while idle, 1 px hovered), which keeps a themed button's
/// size steady, but a button with its own outline then lost a pixel of padding each side on
/// hover: it shrank, and the panel around it with it.
pub fn add_outlined(ui: &mut Ui, button: egui::Button<'_>, width: f32) -> egui::Response {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.bg_stroke.width = width;
        }
        ui.add(button)
    })
    .inner
}

/// Floating model controls share the report's read-only drawing boundary with windows.
pub fn area(id: egui::Id) -> egui::Area {
    egui::Area::new(id).enabled(!crate::issue_report::cad_input_blocked())
}

/// Heading levels inside a panel: its title, and the sections below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The panel's title row (`Tokens::heading_bg`).
    Title,
    /// A section heading (`Tokens::subheading_bg`).
    Section,
}

/// Horizontal inner margin of a heading band, on each side.
pub const HEADING_MARGIN: f32 = 4.0;

/// A heading row drawn on a band `width` wide (a content width, so the band never widens the
/// window); `add` lays out the row left to right in `width - 2 * HEADING_MARGIN`.
pub fn heading<R>(ui: &mut Ui, level: Level, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = crate::theme::Tokens::get();
    let fill = match level {
        Level::Title => t.heading_bg,
        Level::Section => t.subheading_bg,
    };
    let inner = (width - 2.0 * HEADING_MARGIN).max(0.0);
    egui::Frame::new()
        .fill(fill)
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(HEADING_MARGIN as i8, 1))
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.horizontal(add).inner
        })
        .inner
}

/// The width a window's content had last frame (at least `min`), for laying out rows that end at
/// the right edge.
pub fn content_width(ui: &Ui, key: egui::Id, min: f32) -> f32 {
    ui.ctx().data(|d| d.get_temp::<f32>(key)).unwrap_or(min).max(min)
}

/// Remember the content width for next frame (call at the end of the window's content).
pub fn remember_width(ui: &Ui, key: egui::Id) {
    let w = ui.min_rect().width();
    ui.ctx().data_mut(|d| d.insert_temp(key, w));
}

/// In a horizontal row: push what `add` draws (`width` wide) to the right edge of a content
/// `total` wide, without making the row any wider than that.
pub fn right_aligned<R>(ui: &mut Ui, total: f32, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let used = ui.min_rect().width();
    let gap = (total - used - width - ui.spacing().item_spacing.x).max(0.0);
    ui.add_space(gap);
    add(ui)
}

#[cfg(test)]
mod tests {
    #[test]
    fn feature_forms_use_available_height_and_keep_actions_visible() {
        use crate::scenario::Harness;
        use serde_json::json;
        for (width, height, scale) in [(1400.0, 900.0, 1.0), (1400.0, 950.0, 2.0)] {
            let mut h = Harness::new();
            h.app.preferences.ui_scale = scale;
            h.call("ui.resize", json!({"width":width / scale,"height":height / scale}));
            h.app.run("sketch.create", json!({"plane":"XY","name":"Base"})).unwrap();
            h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[10,10]})).unwrap();
            h.app.run("sketch.finish", json!({})).unwrap();
            h.step(&json!({"start":"solid.extrude"})).unwrap();
            h.frames(10);
            let dialog = h.app.viewport.dialog_rect.unwrap();
            let viewport = h.app.viewport.rect.unwrap();
            assert!(dialog.height() > 230.0, "scale={scale}: {dialog:?}");
            assert!(viewport.expand(1.0).contains_rect(dialog), "scale={scale}: {dialog:?} outside {viewport:?}");
            assert!(h.widget("Distance").is_some(), "distance must be visible without scrolling");
            if scale == 1.0 {
                assert!(h.widget("Operation").is_some(), "operation must be visible without scrolling");
            }
            assert!(dialog.contains(crate::scenario::handle_at("dialog:OK").unwrap()));
            assert!(h.widget("Cancel").is_some(), "cancel must be visible without scrolling");
            h.step(&json!({"widget":"Cancel"})).unwrap();
            assert!(h.app.dialog.is_none());
            h.step(&json!({"start":"solid.shell"})).unwrap();
            h.frames(8);
            let short = h.app.viewport.dialog_rect.unwrap();
            assert!(short.height() < dialog.height(), "short forms should shrink: {short:?}, {dialog:?}");
            assert!(h.widget("Cancel").is_some());
            h.step(&json!({"widget":"Cancel"})).unwrap();
            h.step(&json!({"start":"solid.hole"})).unwrap();
            if let Some(crate::dialogs::Dialog { kind: crate::dialogs::Kind::Hole { kind, opts, .. }, .. }) = h.app.dialog.as_mut() {
                *kind = 1;
                opts.tapped = true;
            }
            h.frames(8);
            let long = h.app.viewport.dialog_rect.unwrap();
            assert!(viewport.expand(1.0).contains_rect(long), "long form clipped: {long:?}");
            assert!(h.widget("Cancel").is_some(), "long-form actions must stay visible");
            assert!(long.contains(crate::scenario::handle_at("dialog:OK").unwrap()));
            h.step(&json!({"widget":"Cancel"})).unwrap();
            assert!(h.app.dialog.is_none());
        }
    }
    /// #1335: a dialog keeps its size while its Select chip is hovered, and its title band
    /// drags it; it opens docked again after it closes.
    #[test]
    fn a_docked_dialog_keeps_its_size_under_the_pointer_and_moves_by_its_title() {
        use crate::scenario::Harness;
        use serde_json::json;
        let mut h = Harness::new();
        h.call("ui.resize", json!({"width":1400.0,"height":900.0}));
        h.step(&json!({"start":"sketch.create"})).unwrap();
        h.frames(6);
        let rest = h.app.viewport.dialog_rect.unwrap();
        let chip = crate::scenario::handle_at("dialog:Plane").unwrap();
        h.call("ui.move", json!({"x": chip.x, "y": chip.y}));
        h.frames(6);
        let hovered = h.app.viewport.dialog_rect.unwrap();
        assert!((hovered.size() - rest.size()).length() < 0.01, "hovering resized the dialog: {rest:?} -> {hovered:?}");
        // Drag the published title band, independent of similarly labelled toolbar controls.
        let title = crate::scenario::handle_at("dialog:title").unwrap();
        h.call("ui.drag", json!({"x0": title.x, "y0": title.y, "x1": title.x - 300.0, "y1": title.y + 120.0, "steps": 8}));
        h.frames(4);
        let moved = h.app.viewport.dialog_rect.unwrap();
        assert!((moved.min - rest.min - egui::vec2(-300.0, 120.0)).length() < 2.0, "{rest:?} -> {moved:?}");
        assert!(h.app.dialog.is_some(), "dragging the title keeps the dialog open");
        // Dragged far off, it stays on screen.
        let title = crate::scenario::handle_at("dialog:title").unwrap();
        h.call("ui.drag", json!({"x0": title.x, "y0": title.y, "x1": title.x + 3000.0, "y1": title.y + 3000.0, "steps": 8}));
        h.frames(4);
        let viewport = h.app.viewport.rect.unwrap();
        assert!(viewport.contains(h.app.viewport.dialog_rect.unwrap().min));
        h.step(&json!({"key":"Escape"})).unwrap();
        h.frames(2);
        assert!(h.app.dialog.is_none());
        h.step(&json!({"start":"sketch.create"})).unwrap();
        h.frames(6);
        assert!((h.app.viewport.dialog_rect.unwrap().min - rest.min).length() < 0.01, "a new dialog opens docked");
    }

    /// Windows are made in this file only, so none can skip the width limits.
    #[test]
    fn windows_come_from_the_frame_helper() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut bad = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "rs") && p.file_name().is_some_and(|n| n != "frame.rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                if text.contains("Window::new(") {
                    bad.push(p.display().to_string());
                }
            }
        }
        assert!(bad.is_empty(), "use crate::frame::{{docked, window}} instead of egui::Window::new in {bad:?}");
    }
}
