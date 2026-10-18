//! Application bar, the workspace toolbar (tabs and panels) and keyboard shortcuts.

use crate::icons::glyphs::pin_icon;
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::theme::Tokens;
use crate::{SolveApp, icons};

pub use crate::workspace::TABS;

/// Legacy paint arguments and non-SVG affordances follow the same semantic palette as the icon.
/// The requested icon id determines its meaning on ribbons, menus and drag previews alike.
fn command_accent(icon: &str) -> Color32 {
    crate::icon_svg::primary(icon, crate::theme::is_dark(), crate::theme::vision())
}

/// Accent for a tab or a panel with a more specific semantic role.
fn panel_accent(t: &Tokens, tab: &str, panel: &str) -> Color32 {
    if panel == "CONSTRAIN" {
        return t.glyph_text;
    }
    crate::theme::TabAccent::for_tab(t, tab).color
}

/// Panels shown on each tab (in order). Commands come from the registry by (tab, panel).
pub fn panels(tab: &str) -> Vec<&'static str> {
    crate::workspace::panels(tab)
}

/// How a panel is drawn on a toolbar that may be too narrow for every button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Its first buttons (all of them, or the ones that fit); the panel's drop-down lists every
    /// command.
    Buttons,
    /// One button that opens the panel's drop-down (which lists every command, pinned or not).
    Collapsed,
    /// In the » menu at the end of the toolbar.
    More,
}

/// A ribbon button's preferred and minimum widths. The constraint gallery is a fixed-width
/// block; ordinary buttons narrow as their labels wrap and truncate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonSize {
    pub width: f32,
    pub min_width: f32,
}

/// One panel as [`fit`] sees it, in button order (the gallery, if present, is last).
#[derive(Clone, Debug, PartialEq)]
pub struct PanelSize {
    pub buttons: Vec<ButtonSize>,
}

/// Where [`fit`] put a panel: its first buttons and their individual fitted widths.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub fit: Fit,
    pub shown: usize,
    pub widths: Vec<f32>,
}

impl Plan {
    fn full(panel: &PanelSize) -> Self {
        Self { fit: Fit::Buttons, shown: panel.buttons.len(), widths: panel.buttons.iter().map(|b| b.width).collect() }
    }

    fn more() -> Self {
        Self { fit: Fit::More, shown: 0, widths: Vec::new() }
    }

    /// The panel's width on the toolbar (0 in the » menu).
    pub fn width(&self) -> f32 {
        match self.fit {
            Fit::More => 0.0,
            Fit::Collapsed => COLLAPSED_WIDTH,
            Fit::Buttons => panel_width(self.widths.get(..self.shown).unwrap_or_default()),
        }
    }
}

/// A panel's width with these individual button widths.
pub fn panel_width(buttons: &[f32]) -> f32 {
    (buttons.iter().sum::<f32>() + buttons.len().saturating_sub(1) as f32 * BUTTON_GAP + 8.0).max(COLLAPSED_WIDTH)
}

/// A collapsed panel's width: one button's panel.
pub const COLLAPSED_WIDTH: f32 = 72.0;

/// The gap after each panel (its separator).
const PANEL_GAP: f32 = 1.0;
pub const TAB_ROW_HEIGHT: f32 = 30.0;
pub const GROUP_HEIGHT: f32 = 86.0;
pub const RIBBON_HEIGHT: f32 = TAB_ROW_HEIGHT + GROUP_HEIGHT + 1.0;
const WORKSPACE_WIDTH: f32 = 88.0;
const BUTTON_HEIGHT: f32 = 60.0;
const BUTTON_GAP: f32 = 3.0;
const BUTTON_MIN_WIDTH: f32 = 54.0;
const BUTTON_MAX_WIDTH: f32 = 76.0;
const BUTTON_LABEL_PADDING: f32 = 10.0;
const SECTION_HEIGHT: f32 = 20.0;

/// The » button's room at the end of the panels, kept only while some panel is in it.
pub const MORE_ROOM: f32 = 40.0;

/// Fit panels into `room`, giving up as little as it takes, one small step at a time:
/// 1. buttons narrow from their measured label widths to their minimum, wrapping and truncating;
/// 2. panels hand their last buttons to their drop-down one at a time, the panel showing the
///    most buttons first (the rightmost of equals), down to one button (collapsed);
/// 3. the last panels go into the » menu, and any room that frees goes back to buttons.
///
/// Each removal frees at most one button (or the fixed-width constraint gallery), so the toolbar
/// never leaves a larger gap while something is hidden. The result depends only on `room`
/// (no state from earlier frames), so it never flickers. Nothing is dropped.
pub fn fit(panels: &[PanelSize], room: f32) -> Vec<Plan> {
    let room = if room.is_finite() { room } else { 0.0 };
    if let Some(plans) = shrink(panels, room) {
        return plans;
    }
    // 3. The last panels into the » menu (which takes its own room), as few as it takes; the
    // ones left keep as many buttons as fit.
    for kept in (0..panels.len()).rev() {
        if let Some(mut plans) = shrink(panels.get(..kept).unwrap_or_default(), room - MORE_ROOM) {
            plans.extend(panels.iter().skip(kept).map(|_| Plan::more()));
            return plans;
        }
    }
    panels.iter().map(|_| Plan::more()).collect()
}

/// The room plans take on the toolbar (each panel and its separator).
fn used(plans: &[Plan]) -> f32 {
    plans.iter().filter(|p| p.fit != Fit::More).map(|p| p.width() + PANEL_GAP).sum()
}

/// Steps 1 and 2 of [`fit`]: every panel on the toolbar, or None when even one button each
/// doesn't fit.
fn shrink(panels: &[PanelSize], room: f32) -> Option<Vec<Plan>> {
    let mut plans: Vec<Plan> = panels.iter().map(Plan::full).collect();
    if used(&plans) <= room {
        return Some(plans);
    }
    // 1. Narrow together in one-pixel steps, keeping each button's measured width until it
    // reaches its minimum. The gallery's minimum equals its full width.
    for reduction in 1..=((BUTTON_MAX_WIDTH - BUTTON_MIN_WIDTH) as usize) {
        for (plan, panel) in plans.iter_mut().zip(panels) {
            for (width, button) in plan.widths.iter_mut().zip(&panel.buttons) {
                *width = (button.width - reduction as f32).max(button.min_width);
            }
        }
        if used(&plans) <= room {
            return Some(plans);
        }
    }
    // 2. One button at a time into the drop-downs: from the panel showing the most (the
    // rightmost of equals), down to one button each.
    while used(&plans) > room {
        let pick = plans
            .iter()
            .enumerate()
            .filter(|(_, p)| p.fit == Fit::Buttons && (p.shown > 1 || p.width() > COLLAPSED_WIDTH))
            .max_by(|(i, a), (j, b)| a.shown.cmp(&b.shown).then(i.cmp(j)));
        let (i, _) = pick?;
        if let Some(p) = plans.get_mut(i) {
            p.shown = p.shown.saturating_sub(1).max(1);
            if p.shown == 1 {
                p.fit = Fit::Collapsed;
            }
        }
    }
    Some(plans)
}

/// The commands a panel's drop-down (or its » submenu) lists: its own, then any button moved
/// into it from another panel.
fn menu_commands(
    cmds: &[&'static solidcraft_engine::CommandSpec],
    shown: &[&'static solidcraft_engine::CommandSpec],
) -> Vec<&'static solidcraft_engine::CommandSpec> {
    let mut out = cmds.to_vec();
    for c in shown {
        if !out.iter().any(|o| o.id == c.id) {
            out.push(*c);
        }
    }
    out
}

/// Room for the version between Report a bug and the caption buttons.
const RIGHT_GROUP: f32 = 60.0;
const CLUSTER_BUTTONS: f32 = 108.0;

/// The centre of Help, between Settings and Report a bug.
fn help_x(report: Rect) -> f32 {
    report.left() - 18.0
}

/// The Report a bug button: labelled when there is room for its text (`label_width`, measured in
/// the current language), otherwise just the icon.
fn report_rect(bar: Rect, captions: f32, tabs_start: f32, label_width: f32) -> (Rect, bool) {
    let right = bar.right() - captions - if bar.width() < 640.0 { 8.0 } else { RIGHT_GROUP };
    // Icon, text and a margin that mirrors the icon side.
    let full = REPORT_TEXT_X + label_width + 11.0;
    let labeled = right - CLUSTER_BUTTONS - tabs_start >= 216.0 + full;
    let width = if labeled { full } else { 28.0 };
    (Rect::from_min_size(pos2(right - width, bar.center().y - 13.0), vec2(width, 26.0)), labeled)
}

/// The height of an application-bar pill ([`offer_pill`]), a little under the bar's 26 px buttons.
pub(crate) const PILL_HEIGHT: f32 = 22.0;

/// An application-bar pill for something on offer (Update) at the right of `span` (the bar's free
/// space): accent-filled, with `icon` and the first of `labels` that fits (the icon alone when
/// none does or `labeled` is false). Every label but the last also leaves `keep` points of the
/// span free (for the document tabs). None when even the icon doesn't fit: it never covers the
/// bar's other buttons. `handle` names it for scenarios.
pub(crate) fn offer_pill(
    ui: &mut egui::Ui,
    span: Rect,
    labeled: bool,
    labels: &[&str],
    keep: f32,
    icon: &str,
    handle: &str,
) -> Option<(Rect, egui::Response)> {
    let t = Tokens::get();
    let font = FontId::proportional(t.font_label);
    let (icon_x, text_x, right_pad) = (16.0, 29.0, 11.0);
    let last = labels.len().saturating_sub(1);
    let label = labels.iter().enumerate().filter(|_| labeled).find_map(|(i, text)| {
        let galley = ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font.clone(), t.app_bar_text));
        let width = text_x + galley.size().x + right_pad;
        let room = if i < last { span.width() - keep } else { span.width() };
        (width <= room).then_some((*text, width))
    });
    let width = label.map_or(PILL_HEIGHT + 4.0, |(_, w)| w);
    if width > span.width() {
        return None;
    }
    let rect = Rect::from_min_size(pos2(span.right() - width, span.center().y - PILL_HEIGHT / 2.0), vec2(width, PILL_HEIGHT));
    let response = ui.interact(rect, ui.id().with(("sc_offer_pill", handle)), Sense::click());
    let name = label.map(|(text, _)| text).or(labels.first().copied()).unwrap_or_default().to_string();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &name));
    crate::scenario::publish_handle(handle, rect.center());
    crate::scenario::publish_rect(handle, rect);
    let hot = response.hovered() || response.has_focus();
    let fill = if hot { t.app_bar_hover_button } else { t.accent };
    let radius = PILL_HEIGHT / 2.0;
    ui.painter().rect_filled(rect, radius, fill);
    if response.has_focus() {
        ui.painter().rect_stroke(rect.expand(2.0), radius + 2.0, Stroke::new(1.0, t.app_bar_text), egui::StrokeKind::Outside);
    }
    let glyph_x = if label.is_some() { rect.left() + icon_x } else { rect.center().x };
    let glyph = Rect::from_center_size(pos2(glyph_x, rect.center().y), vec2(16.0, 16.0));
    icons::paint_on(ui.painter(), glyph, icon, Some(true), 1.0);
    if let Some((text, _)) = label {
        ui.painter().text(pos2(rect.left() + text_x, rect.center().y), Align2::LEFT_CENTER, text, font, t.app_bar_text);
    }
    Some((rect, response))
}

/// Where the Report a bug text starts, from the button's left edge.
const REPORT_TEXT_X: f32 = 29.0;

/// The application bar's height: the window's title bar on desktop builds.
pub const APP_BAR_HEIGHT: f32 = 34.0;

pub fn app_bar(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    egui::Panel::top("sc_appbar").exact_size(APP_BAR_HEIGHT).show_separator_line(false).frame(egui::Frame::NONE.fill(t.app_bar)).show(ui, |ui| {
        let r = ui.max_rect();
        // The bar is the window's title bar: its empty space moves the window.
        if app.custom_titlebar || app.integrated_titlebar {
            crate::titlebar::drag_area(ui, r);
        }
        let left = r.left() + if app.integrated_titlebar { crate::titlebar::TRAFFIC_LIGHTS } else { 0.0 };
        // Keep the current header controls and product name when merging the new mark.
        let compact_actions = r.right() - left < 640.0;
        // Brand mark: the SolidCraft logo, drawn in code from the SVG's own coordinates
        // (viewBox 357 x 412). Same 20 px height and centre as the old block.
        {
            let c = pos2(left + 20.0, r.center().y);
            crate::icons::glyphs::brand_mark(ui.painter(), c);
        }
        if !compact_actions {
            ui.painter().text(
                pos2(left + 36.0, r.center().y),
                Align2::LEFT_CENTER,
                "SolidCraft",
                FontId::proportional(t.font_heading),
                t.app_bar_text,
            );
        }
        let mut x = left + if compact_actions { 56.0 } else { 128.0 };
        let home = Rect::from_center_size(pos2(x - 4.0, r.center().y), vec2(32.0, 32.0));
        let idle = !app.file_load.busy();
        let home_response = ui.interact(home, ui.id().with("appbar_home"), if idle { Sense::click() } else { Sense::hover() });
        let home_tip = app.tr("ui.start_page");
        home_response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, idle, home_tip));
        crate::scenario::publish_handle("appbar:home", home.center());
        crate::scenario::publish_handle("titlebar:home", home.center());
        // Compatibility for scenarios that used the former Home tab.
        crate::scenario::publish_handle("tab:home", home.center());
        crate::scenario::publish_rect("titlebar:home", home);
        crate::scenario::publish_rect("appbar:home", home);
        if idle && (home_response.hovered() || home_response.has_focus()) {
            ui.painter().rect_filled(home.shrink(3.0), t.radius_sm, t.app_bar_hover_button);
        }
        icons::paint_app_bar(ui.painter(), home.shrink(7.0), "home", if idle { 1.0 } else { 0.35 });
        if home_response.on_hover_text(home_tip).clicked() && idle {
            app.home.show(app.session.revision);
        }
        x = home.right() + 8.0;
        app_bar_separator(ui, r, x, "brand");
        x += 16.0;
        // While a file loads the design waits: no menus or commands from the bar.
        // The File menu: a text button (New, Open, Save, Save As, Export…). Its handle keeps the
        // old `appbar:folder` name too, for scripts written against the folder button.
        let file = {
            let label = app.tr("ui.file").to_string();
            let galley = ui.painter().layout_no_wrap(label.clone(), FontId::proportional(t.font_button), t.app_bar_text);
            let br = Rect::from_min_size(pos2(x - 13.0, r.center().y - 13.0), vec2(galley.size().x + 16.0, 26.0));
            x = br.right() + 17.0;
            let resp = ui.interact(br, ui.id().with(("ab", "file")), if idle { Sense::click() } else { Sense::hover() });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, idle, &label));
            crate::scenario::publish_handle("appbar:file", br.center());
            crate::scenario::publish_handle("appbar:folder", br.center());
            crate::scenario::publish_rect("appbar:file", br);
            let open = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("sc_file_menu")).unwrap_or(false));
            if idle && (resp.hovered() || resp.has_focus() || open) {
                ui.painter().rect_filled(br, t.radius_sm, t.app_bar_hover);
            }
            let ink = if idle { t.app_bar_text } else { t.app_bar_text.gamma_multiply(0.35) };
            ui.painter().galley(pos2(br.left() + 8.0, br.center().y - galley.size().y / 2.0), galley, ink);
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("sc_file_menu_x"), br.left()));
            idle && resp.clicked()
        };
        // A disabled button is dimmed and ignores clicks (its tooltip still shows).
        let click = |ui: &mut egui::Ui, at: f32, icon: &str, tip: &str, enabled: bool| -> bool {
            let width = if compact_actions && matches!(icon, "save" | "undo" | "redo") { 22.0 } else { 32.0 };
            let br = Rect::from_center_size(pos2(at, r.center().y), vec2(width, 32.0));
            let resp = ui.interact(br, ui.id().with(("ab", icon)), if enabled { Sense::click() } else { Sense::hover() });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tip));
            crate::scenario::publish_handle(&format!("appbar:{icon}"), br.center());
            crate::scenario::publish_rect(&format!("appbar:{icon}"), br);
            crate::scenario::publish_handle(&format!("titlebar:{icon}"), br.center());
            crate::scenario::publish_rect(&format!("titlebar:{icon}"), br);
            if enabled && (resp.hovered() || resp.has_focus()) {
                ui.painter().rect_filled(br.shrink(3.0), t.radius_sm, t.app_bar_hover_button);
            }
            icons::paint_app_bar(ui.painter(), Rect::from_center_size(br.center(), vec2(18.0, 18.0)), icon, if enabled { 1.0 } else { 0.35 });
            let clicked = resp.on_hover_text(tip).clicked();
            enabled && clicked
        };
        let step = if compact_actions { 22.0 } else { 34.0 };
        // Save, Undo and Redo use the same actions as the File menu and shortcuts.
        let shown = design_shown(app) && idle;
        let tip = |id: &str, key: &str| {
            let label = app.tr(id);
            if shown { format!("{label} ({key})") } else { crate::i18n::format("ui.needs_design", &[("command", label)]) }
        };
        let (save_tip, undo_tip, redo_tip) = (tip("ui.save", "Ctrl+S"), tip("ui.undo", "Ctrl+Z"), tip("ui.redo", "Ctrl+Y"));
        if click(ui, x, "save", &save_tip, shown) {
            save(app);
        }
        x += step;
        // Narrow windows retain Save; Undo and Redo remain available from their shortcuts and menus.
        if !compact_actions {
            if click(ui, x, "undo", &undo_tip, shown) {
                let _ = app.run("edit.undo", json!({}));
            }
            x += step;
            if click(ui, x, "redo", &redo_tip, shown) {
                let _ = app.run("edit.redo", json!({}));
            }
            x += step;
        }
        if file {
            app.ui.palette_open = false;
            let ctx = ui.ctx().clone();
            let open = ctx.data(|d| d.get_temp::<bool>(egui::Id::new("sc_file_menu")).unwrap_or(false));
            let pass = ctx.cumulative_pass_nr();
            // The File button toggles the menu. Remember the pass it opened on so the same
            // click isn't also treated as a click outside the menu (which would close it at once).
            ctx.data_mut(|d| {
                d.insert_temp(egui::Id::new("sc_file_menu"), !open);
                d.insert_temp(egui::Id::new("sc_file_menu_opened"), pass);
            });
        }
        // Document tabs follow the file actions; the house opens the start page.
        let captions = if app.custom_titlebar { crate::titlebar::WIDTH } else { 0.0 };
        let separator_x = x - step + if compact_actions { 19.0 } else { 24.0 };
        app_bar_separator(ui, r, separator_x, "documents");
        let tabs_start = separator_x + 10.0;
        let report_text = crate::i18n::tr("ui.report_a_bug");
        let report_font = FontId::proportional(t.font_button);
        let label_width = ui.painter().layout_no_wrap(report_text.to_string(), report_font.clone(), t.app_bar_text).size().x;
        let (report, report_label) = report_rect(r, captions, tabs_start, label_width);
        // Assistant, Settings, Help and Report a bug form the right-hand cluster.
        if click(ui, help_x(report), "help", app.tr("ui.help"), true) {
            app.help.menu = !app.help.menu;
            let at = help_x(report);
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("sc_help_opened"), true);
                d.insert_temp(egui::Id::new("sc_help_anchor"), at);
            });
        }
        // A newer release: its Update button stays before the right cluster, in the space up to
        // the bar's own file actions on the left.
        let span = Rect::from_min_max(pos2(x, report.top()), pos2(report.left() - CLUSTER_BUTTONS - 2.0, report.bottom()));
        let update_labeled = report_label;
        let update = crate::update::button(app, ui, span, update_labeled);
        let tabs_end = update.map_or(report.left() - CLUSTER_BUTTONS, |u| u.left()) - 8.0;
        // Narrow title bars can fit only part of the tab strip. Its hit regions must not
        // extend into the persistent report and window controls.
        let tabs_rect = Rect::from_min_max(pos2(tabs_start.min(tabs_end), r.top()), pos2(tabs_end, r.bottom()));
        let mut tabs_ui = ui.new_child(egui::UiBuilder::new().max_rect(tabs_rect));
        tabs_ui.set_clip_rect(tabs_rect.intersect(ui.clip_rect()));
        crate::documents::tabs(app, &mut tabs_ui, r, tabs_start, tabs_end);
        let response = ui.interact(report, ui.id().with("sc_report_button"), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), report_text));
        crate::scenario::publish_handle("titlebar:report", report.center());
        crate::scenario::publish_rect("titlebar:report", report);
        ui.painter().rect_filled(report, t.radius_sm, if response.hovered() || response.has_focus() { t.app_bar_hover_button } else { t.app_bar });
        ui.painter().rect_stroke(report, t.radius_sm, Stroke::new(1.0, t.app_bar_separator), egui::StrokeKind::Inside);
        let icon = Rect::from_center_size(pos2(report.left() + 14.0, report.center().y), vec2(18.0, 18.0));
        icons::paint_app_bar(ui.painter(), icon, "bug_report", 1.0);
        if report_label {
            ui.painter().text(pos2(report.left() + REPORT_TEXT_X, report.center().y), Align2::LEFT_CENTER, report_text, report_font, t.app_bar_text);
        }
        if response.on_hover_text(crate::i18n::tr_label("Report a bug — review a screenshot and open a GitHub draft")).clicked() {
            crate::help::run(app, &ui.ctx().clone(), "report");
        }
        // Settings sits immediately left of Help (also Ctrl+,).
        let gear = Rect::from_center_size(pos2(report.left() - 52.0, r.center().y), vec2(32.0, 32.0));
        let gresp = ui.interact(gear, ui.id().with("sc_prefs_button"), Sense::click());
        if gresp.hovered() || gresp.has_focus() {
            ui.painter().rect_filled(gear.shrink(3.0), t.radius_sm, t.app_bar_hover_button);
        }
        icons::paint_app_bar(ui.painter(), gear.shrink(7.0), "settings", 1.0);
        crate::scenario::publish_rect("titlebar:preferences", gear);
        if crate::agent_chat::app_bar_button_shown(app) {
            assistant_button(app, ui, Rect::from_center_size(pos2(report.left() - 86.0, r.center().y), vec2(32.0, 32.0)));
        }
        crate::scenario::publish_handle("titlebar:preferences", gear.center());
        let prefs_tip = if cfg!(target_os = "macos") {
            format!("{} (Cmd+,)", app.tr("ui.preferences"))
        } else {
            format!("{} (Ctrl+,)", app.tr("ui.preferences"))
        };
        if gresp.on_hover_text(prefs_tip).clicked() {
            crate::prefs::open_at(app, 0);
        }
        if r.width() >= 640.0 {
            ui.painter().text(
                pos2(r.right() - 12.0 - captions, r.center().y),
                Align2::RIGHT_CENTER,
                format!("v{}", crate::update::current_version()),
                FontId::proportional(t.font_small),
                t.app_bar_text,
            );
        }
        if app.custom_titlebar {
            crate::titlebar::caption_buttons(app, ui, r);
        }
    });
    file_menu(app, ui.ctx());
    let help_x = ui.ctx().data(|d| d.get_temp::<f32>(egui::Id::new("sc_help_anchor")).unwrap_or(300.0));
    crate::help::show(app, &ui.ctx().clone(), pos2(help_x - 14.0, APP_BAR_HEIGHT));
}

fn file_menu(app: &mut SolveApp, ctx: &egui::Context) {
    let id = egui::Id::new("sc_file_menu");
    if !ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        return;
    }
    let mut close = false;
    let resp = crate::frame::area(egui::Id::new("sc_file_area"))
        .fixed_pos(pos2(ctx.data(|d| d.get_temp::<f32>(egui::Id::new("sc_file_menu_x"))).unwrap_or(100.0), APP_BAR_HEIGHT))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(190.0);
                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
                ui.visuals_mut().widgets.hovered.weak_bg_fill = Tokens::get().hover;
                let item_if = |ui: &mut egui::Ui, label: &str, key: &str, enabled: bool| {
                    ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(key).min_size(vec2(180.0, 22.0))).clicked()
                };
                let item = |ui: &mut egui::Ui, label: &str, key: &str| item_if(ui, label, key, true);
                // Insert, Save and Export act on the design, which Home hides.
                let shown = design_shown(app);
                if item(ui, app.tr("ui.new_design"), "Ctrl+N") {
                    crate::documents::new_design(app);
                    close = true;
                }
                if item(ui, &format!("{}…", app.tr("ui.open")), "Ctrl+O") {
                    app.pick_open(|app, path| {
                        if let Some(path) = path {
                            app.open_path_async(&path);
                        }
                    });
                    close = true;
                }
                ui.menu_button(app.tr("ui.open_recent"), |ui| {
                    for path in app.home.recent.clone() {
                        let missing = !solidcraft_engine::io::vfs::exists(&path);
                        let label = format!("{path}{}", if missing { " (missing)" } else { "" });
                        if ui.button(label).clicked() {
                            crate::home::open_recent(app, &path);
                            close = true;
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(app.tr("ui.clear_recent")).clicked() {
                        app.home.recent.clear();
                        app.home.opened_at.clear();
                        close = true;
                        ui.close();
                    }
                });
                if item_if(ui, "Insert STEP…", "", shown) {
                    app.start("file.insert_step");
                    close = true;
                }
                if item_if(ui, "Insert Mesh…", "", shown) {
                    app.start("file.insert_mesh");
                    close = true;
                }
                if item_if(ui, &format!("{}…", app.tr_label("Insert Design")), "", shown) {
                    app.start("component.insert_design");
                    close = true;
                }
                if item_if(ui, app.tr("ui.save"), "Ctrl+S", shown) {
                    save(app);
                    close = true;
                }
                if item_if(ui, &format!("{}…", app.tr("ui.save_as")), "", shown) {
                    save_as(app);
                    close = true;
                }
                if item(ui, "Save As JSON…", "") {
                    save_as_with_extension(app, "json");
                    close = true;
                }
                ui.separator();
                if item_if(ui, "Save as mesh…", "", shown) {
                    app.start("file.save_mesh");
                    close = true;
                }
                if item_if(ui, "Export STEP…", "", shown) {
                    export(app, "step");
                    close = true;
                }
                if item_if(ui, app.tr_label("Export Current View as DXF…"), "", shown) {
                    app.start("file.export_view_dxf");
                    close = true;
                }
                if item_if(ui, "Export OBJ…", "", shown) {
                    export(app, "obj");
                    close = true;
                }
                if item_if(ui, "Export URDF…", "", shown) {
                    export(app, "urdf");
                    close = true;
                }
                if item(ui, "Export Design as JSON…", "") {
                    export(app, "json");
                    close = true;
                }
                ui.separator();
                if item_if(ui, &format!("{}…", app.tr_label("Design Properties")), "", shown) {
                    app.start("document.metadata");
                    close = true;
                }
                ui.separator();
                // The browser can install SolidCraft as an app (`install`): only while it offers to.
                if let Some(offer) = crate::install::offered(app)
                    && ui
                        .add(egui::Button::new(crate::i18n::tr("install.menu")).min_size(vec2(180.0, 22.0)))
                        .on_hover_text(crate::i18n::tr("install.button_tip"))
                        .clicked()
                {
                    crate::install::start(app, offer);
                    close = true;
                }
                if item(ui, &format!("{}…", app.tr("ui.preferences")), "") {
                    app.prefs_window.open = true;
                    close = true;
                }
                if item(ui, &format!("{}…", app.tr("ui.keyboard_shortcuts")), "") {
                    app.keymap.open = true;
                    app.keymap.read_only = false;
                    close = true;
                }
                if item(ui, app.tr("ui.quit"), "") {
                    crate::documents::request_quit(app);
                    close = true;
                }
            });
        });
    let just_opened = ctx.data(|d| d.get_temp::<u64>(egui::Id::new("sc_file_menu_opened"))) == Some(ctx.cumulative_pass_nr());
    if close || (!just_opened && resp.response.clicked_elsewhere()) {
        ctx.data_mut(|d| d.insert_temp(id, false));
    }
}

/// Whether the open design is on screen. Home hides it, so commands that act on it (save,
/// undo, insert, export, tool shortcuts) are off there.
pub fn design_shown(app: &SolveApp) -> bool {
    !app.home.open
}

pub fn save(app: &mut SolveApp) {
    if crate::browser::needs_capture(app, crate::browser::SAVE) {
        app.tree.capture_prompt = Some(crate::browser::SAVE.into());
        return;
    }
    if app.session.path.is_some() {
        let _ = app.run("file.save", json!({}));
    } else {
        save_as(app);
    }
}

fn save_as(app: &mut SolveApp) {
    save_as_with_extension(app, solidcraft_engine::io::DESIGN_EXT);
}

fn save_as_with_extension(app: &mut SolveApp, extension: &str) {
    let name = format!("{}.{extension}", app.session.doc.name);
    app.pick_save(&name, &[solidcraft_engine::io::DESIGN_EXT, "json"], |app, path| {
        if let Some(path) = path {
            let _ = app.run("file.save_as", json!({"path": path}));
        }
        crate::documents::saved_for_close(app);
    });
}

fn export(app: &mut SolveApp, ext: &str) {
    export_as(app, ext, None);
}

/// Export to a file with extension `ext`, in `format` when the extension alone doesn't name it.
fn export_as(app: &mut SolveApp, ext: &str, format: Option<&str>) {
    let name = format!("{}.{ext}", app.session.doc.name);
    let format = format.map(str::to_string);
    app.pick_save(&name, &[ext], move |app, path| {
        let Some(path) = path else { return };
        let mut params = json!({"path": path});
        if let Some(format) = format {
            params["format"] = json!(format);
        }
        if app.run("file.export", params).is_ok() {
            app.set_status(format!("Exported {path}"), false);
        }
    });
}

/// Choose at most two lines at spaces or after `/`, then truncate each line at
/// Unicode character boundaries. An oversized word stays on one line with an ellipsis.
fn ribbon_button_lines(painter: &egui::Painter, label: &str, width: f32) -> Vec<String> {
    let mut lines = ribbon_label_lines(painter, label, width);
    for line in &mut lines {
        if ribbon_label_width(painter, line) > width {
            while !line.is_empty() {
                line.pop();
                if ribbon_label_width(painter, &format!("{}…", line.trim_end())) <= width {
                    break;
                }
            }
            *line = format!("{}…", line.trim_end());
        }
    }
    lines
}

fn ribbon_label_width(painter: &egui::Painter, text: &str) -> f32 {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), FontId::proportional(12.0), Color32::WHITE, f32::INFINITY);
    job.halign = egui::Align::Center;
    painter.layout_job(job).size().x
}

fn ribbon_button_width(painter: &egui::Painter, label: &str) -> f32 {
    let widest = ribbon_label_lines(painter, label, BUTTON_MAX_WIDTH - BUTTON_LABEL_PADDING)
        .iter()
        .map(|line| ribbon_label_width(painter, line))
        .fold(0.0_f32, f32::max);
    (widest + BUTTON_LABEL_PADDING).clamp(BUTTON_MIN_WIDTH, BUTTON_MAX_WIDTH)
}

fn ribbon_label_lines(painter: &egui::Painter, label: &str, width: f32) -> Vec<String> {
    let measure = |text: &str| ribbon_label_width(painter, text);
    let normalized = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let label = normalized.as_str();
    let mut end = 0;
    for token in label.split_inclusive(|c: char| c.is_whitespace() || c == '/') {
        let next = end + token.len();
        if end > 0 && measure(label.get(..next).unwrap_or_default().trim()) > width {
            break;
        }
        end = next;
    }
    let (first, rest) = label.split_at(end);
    let mut lines = vec![first.trim().to_owned()];
    if !rest.trim().is_empty() {
        lines.push(rest.trim().to_owned());
    }
    lines
}

fn ribbon_button_galley(painter: &egui::Painter, label: &str, width: f32, color: Color32) -> std::sync::Arc<egui::Galley> {
    let lines = ribbon_button_lines(painter, label, width);
    // Explicit newlines only: egui must never split an oversized word.
    let mut job = egui::text::LayoutJob::simple(lines.join("\n"), FontId::proportional(12.0), color, f32::INFINITY);
    job.halign = egui::Align::Center;
    painter.layout_job(job)
}

/// Ribbon headings use the translated labels, with normal word casing instead of registry caps.
fn ribbon_label(app: &SolveApp, label: &str) -> String {
    app.tr_label(label)
        .split_whitespace()
        .map(|word| {
            let lower = word.to_lowercase();
            let mut chars = lower.chars();
            chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn ribbon_text(ui: &egui::Ui, at: egui::Pos2, text: &str, size: f32, color: Color32, bold: bool) {
    let text = egui::RichText::new(text).size(size).color(color);
    let text = if bold { text.strong() } else { text };
    let galley = egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
    let origin = at - galley.size() / 2.0;
    // egui's `strong` only changes colour. A small overstrike adds weight while retaining
    // the user's interface font and its language fallbacks, without another font asset.
    if bold {
        ui.painter().galley(origin - vec2(0.2, 0.0), galley.clone(), color);
    }
    ui.painter().galley(origin + vec2(if bold { 0.2 } else { 0.0 }, 0.0), galley, color);
}

/// The ribbon and S shortcut share one popup anchor, including after resizing.
fn search_anchor(ctx: &egui::Context) -> Option<egui::Pos2> {
    ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("sc_ribbon_search"))).map(|r| r.left_bottom() + vec2(0.0, 3.0))
}

fn ribbon_tabs(app: &mut SolveApp, ui: &mut egui::Ui, row: Rect) {
    let t = Tokens::get();
    let tabs = crate::workspace::visible_tabs(app);
    let widths: Vec<_> =
        tabs.iter().map(|tab| ui.painter().layout_no_wrap(ribbon_label(app, tab), FontId::proportional(13.0), t.text).size().x + 24.0).collect();
    let tabs_width: f32 = widths.iter().sum::<f32>()
        + tabs.len().saturating_sub(1) as f32 * 2.0
        + tabs.iter().filter(|tab| matches!(**tab, "SKETCH" | "SHEET METAL")).count() as f32 * 6.0;
    let search_width = if row.width() >= tabs_width + 188.0 + 16.0 { 188.0 } else { 24.0 };
    let search = Rect::from_min_size(pos2((row.right() - search_width - 6.0).max(row.left()), row.top() + 3.0), vec2(search_width, 24.0));
    let resp = ui.interact(search, ui.id().with("ribbon_search"), Sense::click());
    crate::scenario::publish_handle("ribbon:search", search.center());
    crate::scenario::publish_rect("ribbon:search", search);
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("sc_ribbon_search"), search));
    ui.painter().rect(search, 5.0, t.field, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    // The existing zoom glyph is the icon system's magnifier; no new artwork.
    icons::paint_icon(ui.painter(), Rect::from_center_size(search.left_center() + vec2(12.0, 0.0), vec2(16.0, 16.0)), "zoom", 1.0);
    let label = app.tr_label("Search commands");
    if search_width > 24.0 {
        ui.painter().text(search.left_center() + vec2(29.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(12.5), t.text_dim);
        let cap = Rect::from_center_size(search.right_center() - vec2(14.0, 0.0), vec2(16.0, 16.0));
        ui.painter().rect(cap, 3.0, t.toolbar, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        ribbon_text(ui, cap.center(), "S", 11.0, t.text_dim, false);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if resp.on_hover_text(label).clicked() {
        crate::shortcut_box::open(app, search_anchor(ui.ctx()));
    }
    let changed = ui.ctx().data_mut(|d| {
        let id = egui::Id::new("sc_ribbon_visible_tab");
        let state = (app.ui.tab.clone(), row.width());
        let changed = d.get_temp::<(String, f32)>(id) != Some(state.clone());
        d.insert_temp(id, state);
        changed
    });
    let span = Rect::from_min_max(row.left_top() + vec2(4.0, 0.0), pos2((search.left() - 6.0).max(row.left() + 4.0), row.bottom()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(span), |ui| {
        ui.set_clip_rect(span);
        egui::ScrollArea::horizontal()
            .id_salt("ribbon_tabs")
            .animated(false)
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                let (all, _) = ui.allocate_exact_size(vec2(tabs_width, TAB_ROW_HEIGHT), Sense::hover());
                let mut x = all.left();
                for (tab, width) in tabs.into_iter().zip(widths) {
                    let contextual = matches!(tab, "SKETCH" | "SHEET METAL");
                    if contextual {
                        x += 6.0;
                    }
                    let tr = Rect::from_min_size(pos2(x, all.top() + 3.0), vec2(width, 24.0));
                    let active = app.ui.tab == tab;
                    if active && changed {
                        ui.scroll_to_rect(tr, Some(egui::Align::Center));
                    }
                    let response = ui.interact(tr, ui.id().with(("tab", tab)), Sense::click());
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, ribbon_label(app, tab)));
                    let accent = crate::theme::TabAccent::for_tab(&t, tab);
                    let fill = if contextual {
                        accent.color
                    } else if active {
                        accent.soft
                    } else if response.hovered() {
                        t.hover
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(tr, t.radius_sm.saturating_add(1), fill);
                    ribbon_text(
                        ui,
                        tr.center(),
                        &ribbon_label(app, tab),
                        13.0,
                        if contextual {
                            t.on_accent
                        } else if active {
                            selected_ink(&accent)
                        } else {
                            t.text
                        },
                        active || contextual,
                    );
                    if ui.clip_rect().contains(tr.center()) {
                        crate::scenario::publish_handle(&format!("tab:{tab}"), tr.center());
                        crate::scenario::publish_rect(&format!("tab:{tab}"), tr);
                        crate::scenario::publish_rect(&format!("toolbar_tab:{tab}"), tr);
                        let aliases: &[&str] = match tab {
                            "MODEL" => &["SOLID", "PLASTIC"],
                            "SURFACE" => &["MESH"],
                            "MANAGE" => &["UTILITIES"],
                            _ => &[],
                        };
                        for alias in aliases {
                            crate::scenario::publish_handle(&format!("tab:{alias}"), tr.center());
                        }
                    }
                    if response.clicked() {
                        app.ui.tab = tab.to_string();
                    }
                    x += width + 2.0;
                }
            });
    });
    if matches!(app.ui.tab.as_str(), "SKETCH" | "SHEET METAL") {
        ui.painter().line_segment(
            [row.left_bottom() - vec2(0.0, 1.0), row.right_bottom() - vec2(0.0, 1.0)],
            Stroke::new(2.0, panel_accent(&t, &app.ui.tab, "")),
        );
    } else {
        ui.painter().line_segment(
            [row.left_bottom() - vec2(0.0, 0.5), row.right_bottom() - vec2(0.0, 0.5)],
            Stroke::new(1.0, t.toolbar.lerp_to_gamma(t.border, 0.5)),
        );
    }
}

/// The dropdown chevron every ribbon menu shares.
fn chevron(painter: &egui::Painter, at: egui::Pos2, ink: Color32) {
    crate::icons::paint_ink(painter, Rect::from_center_size(at, vec2(10.0, 10.0)), "chevron", ink);
}

/// The label colour on a selected tab or open section: on the dark ribbon a lighter tint of the accent.
fn selected_ink(accent: &crate::theme::TabAccent) -> Color32 {
    if crate::theme::is_dark() { accent.color.lerp_to_gamma(Tokens::get().on_accent, 0.5) } else { accent.color }
}

pub fn toolbar(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    crate::workspace::sync_context(app);
    egui::Panel::top("sc_toolbar").exact_size(RIBBON_HEIGHT).show_separator_line(false).frame(egui::Frame::NONE.fill(t.toolbar)).show(ui, |ui| {
        let r = ui.max_rect();
        let painter = ui.painter().clone();
        let bg = ui.interact(r, ui.id().with("toolbar_bg"), Sense::click());
        if bg.secondary_clicked() {
            crate::context_menu::open_for(app, bg.interact_pointer_pos().unwrap_or(r.left_bottom()), crate::context_menu::Target::Toolbar);
        }
        let ws = Rect::from_min_size(r.left_top(), vec2(WORKSPACE_WIDTH, RIBBON_HEIGHT - 1.0));
        let wresp = ui.interact(ws, ui.id().with("workspace_switcher"), Sense::click());
        let open = app.viewport.context_menu.is_some() && app.menu.target == crate::context_menu::Target::Workspace;
        if wresp.hovered() || open {
            painter.rect_filled(ws, 0.0, t.hover);
        }
        painter.rect_filled(Rect::from_min_size(ws.left_top(), vec2(WORKSPACE_WIDTH, 3.0)), 0.0, t.accent);
        painter.line_segment([ws.right_top(), ws.right_bottom()], Stroke::new(1.0, t.border));
        let render = crate::render_workspace::active(app);
        let workspace_icon = if render { "ws_render" } else { "ws_design" };
        icons::paint_icon(&painter, Rect::from_center_size(pos2(ws.center().x, ws.top() + 39.0), vec2(30.0, 30.0)), workspace_icon, 1.0);
        ribbon_text(ui, pos2(ws.center().x, ws.top() + 69.0), app.tr(if render { "ui.render" } else { "ui.design" }), 13.0, t.text, true);
        let label = app.tr("ui.ribbon_workspace");
        let label_width = painter.layout_no_wrap(label.to_string(), FontId::proportional(11.0), t.text_dim).size().x;
        ribbon_text(ui, pos2(ws.center().x - 6.0, ws.top() + 89.0), label, 11.0, t.text_dim, false);
        chevron(&painter, pos2(ws.center().x - 6.0 + label_width / 2.0 + 7.0, ws.top() + 89.5), t.text_dim);
        if wresp.on_hover_text(app.tr("ui.change_workspace")).clicked() {
            crate::context_menu::toggle_for(app, ws, ws.left_bottom(), crate::context_menu::Target::Workspace);
        }
        crate::scenario::publish_rect("toolbar", r);
        crate::scenario::publish_rect("workspace_switcher", ws);
        crate::scenario::publish_rect("ribbon:workspace", ws);
        crate::scenario::publish_handle("ribbon:workspace", ws.center());
        ribbon_tabs(app, ui, Rect::from_min_max(pos2(ws.right() + 1.0, r.top()), pos2(r.right(), r.top() + TAB_ROW_HEIGHT)));
        // Panels.
        let tab = app.ui.tab.clone();
        // The command whose tool or dialog is open right now, shown pressed in the toolbar.
        let running: Option<String> = match (&app.tool, &app.dialog) {
            (Some(tool), _) => Some(tool.cmd.to_string()),
            // Create Sketch never lands in last_command (it is not repeatable).
            (None, Some(d)) if matches!(d.kind, crate::dialogs::Kind::Sketch) => Some("sketch.create".to_string()),
            (None, Some(_)) => app.last_command.as_ref().map(|(id, _)| id.clone()),
            _ => None,
        };
        let mut px = r.left() + WORKSPACE_WIDTH + 1.0;
        let top = r.top() + TAB_ROW_HEIGHT;
        let mut drop: Option<(&'static str, &'static str, Option<&'static str>)> = None;
        let mut menu: Option<(egui::Pos2, &'static str)> = None;
        // A drop-down row's pin icon clicked: pin or unpin that command.
        let mut toggle: Option<&'static str> = None;
        // The open drop-down (a button dropped on it leaves the toolbar).
        let mut popup_rect: Option<Rect> = None;
        let mut overflow = Vec::new();
        // Every panel with all its buttons (the user's pins have no cap); a toolbar too narrow
        // for them narrows the buttons, then trims panels into their drop-down and the » menu.
        let lists: Vec<_> = crate::toolbar_custom::panel_lists(&tab)
            .into_iter()
            .map(|(panel, cmds, promote)| {
                let mut shown = crate::toolbar_custom::buttons(app, &tab, panel, &cmds, promote);
                // Gallery cells are painted together after the large buttons, in reading order.
                if tab == "SKETCH" && panel == "CONSTRAIN" {
                    shown.sort_by_key(|c| crate::workspace::CONSTRAINT_GALLERY.iter().position(|id| *id == c.id).map(|i| i + 1).unwrap_or(0));
                }
                (panel, cmds, shown)
            })
            .collect();
        let sizes: Vec<PanelSize> = lists
            .iter()
            .map(|(panel, _, shown)| {
                let mut widths: Vec<_> = shown
                    .iter()
                    .filter(|c| !(tab == "SKETCH" && *panel == "CONSTRAIN" && crate::workspace::CONSTRAINT_GALLERY.contains(&c.id)))
                    .map(|c| {
                        let family = crate::sketch_families::family(c.id)
                            .filter(|f| tab == "SKETCH" && *panel == "CREATE" && f.commands.first().copied() == Some(c.id));
                        let label = family.map_or_else(|| crate::workspace::ribbon_button_label(c), |f| f.label);
                        ButtonSize { width: ribbon_button_width(&painter, app.tr_label(label)), min_width: BUTTON_MIN_WIDTH }
                    })
                    .collect();
                if tab == "SKETCH" && *panel == "CONSTRAIN" && shown.iter().any(|c| crate::workspace::CONSTRAINT_GALLERY.contains(&c.id)) {
                    widths.push(ButtonSize { width: CONSTRAINT_GRID_WIDTH, min_width: CONSTRAINT_GRID_WIDTH });
                }
                PanelSize { buttons: widths }
            })
            .collect();
        let right = r.right();
        // Keep the Sketch exit group at the far right and reachable at every width.
        let finish_plan = sizes.last().filter(|_| tab == "SKETCH").map(Plan::full);
        let finish_width = finish_plan.as_ref().map_or(0.0, |p| p.width().max(80.0));
        let normal_count = sizes.len().saturating_sub(usize::from(finish_plan.is_some()));
        let mut plans = fit(sizes.get(..normal_count).unwrap_or_default(), right - 4.0 - px - finish_width);
        plans.extend(finish_plan);
        crate::scenario::publish_count("toolbar_collapsed", plans.iter().filter(|p| p.fit == Fit::Collapsed).count() as f64);
        crate::scenario::publish_count("toolbar_more", plans.iter().filter(|p| p.fit == Fit::More).count() as f64);
        crate::scenario::publish_count(
            "toolbar_trimmed",
            plans.iter().zip(&sizes).filter(|(p, s)| p.fit == Fit::Buttons && p.shown < s.buttons.len()).count() as f64,
        );
        for ((panel, cmds, shown), plan) in lists.into_iter().zip(plans) {
            let panel: &&str = &panel;
            if plan.fit == Fit::More {
                overflow.push((*panel, menu_commands(&cmds, &shown)));
                continue;
            }
            let collapsed = plan.fit == Fit::Collapsed;
            let trimmed = plan.shown < plan.widths.len();
            let finish = tab == "SKETCH" && *panel == "FINISH SKETCH";
            let width = if finish { finish_width } else { plan.width() };
            if finish {
                px = right - finish_width;
                painter.line_segment([pos2(px, top), pos2(px, r.bottom() - 1.0)], Stroke::new(1.0, t.border));
            }
            let button_widths = &plan.widths;
            let sketch_create = tab == "SKETCH" && *panel == "CREATE";
            let enabled_panel = !cmds.is_empty() || *panel == "SELECT";
            // Per tab: panels of the same name on other tabs (MODIFY, CREATE…) list other commands,
            // and the drop-down's area remembers its size by this id (#874).
            let popup_id = ui.id().with(("panel_menu", &tab, panel));
            // Dropped on the panel's free space (all of a collapsed panel): the button goes to
            // its end.
            let gallery: Vec<_> = shown
                .iter()
                .copied()
                .filter(|c| tab == "SKETCH" && *panel == "CONSTRAIN" && crate::workspace::CONSTRAINT_GALLERY.contains(&c.id))
                .collect();
            let large: Vec<_> = shown.iter().copied().filter(|c| !gallery.iter().any(|g| g.id == c.id)).collect();
            let drawn = if collapsed { 0 } else { plan.shown.min(button_widths.len()) };
            // Buttons narrower than the panel's minimum width (a one-button panel) are centred.
            let buttons_width = button_widths.iter().take(drawn).sum::<f32>() + drawn.saturating_sub(1) as f32 * BUTTON_GAP;
            let inset = if drawn == 0 { 0.0 } else { ((width - 8.0 - buttons_width) / 2.0).max(0.0).floor() };
            let bx = px + 4.0 + inset;
            let free_x = bx + buttons_width + BUTTON_GAP;
            let free = Rect::from_min_size(pos2(free_x, top + 2.0), vec2((px + width - 4.0 - free_x).max(0.0), BUTTON_HEIGHT));
            let fresp = ui.interact(free, ui.id().with(("panel_free", panel)), if collapsed { Sense::click() } else { Sense::hover() });
            let mut opened = false;
            if collapsed {
                // One button: the panel's first command's icon over a caret; it opens the
                // drop-down, which lists every command of the panel.
                let br = Rect::from_center_size(free.center(), vec2((width - 8.0).max(0.0), BUTTON_HEIGHT));
                crate::scenario::publish_handle(&format!("collapsed:{tab}:{panel}"), br.center());
                if fresp.hovered() && enabled_panel {
                    painter.rect_filled(br, t.radius_sm, t.hover);
                }
                if let Some(c) = shown.first().or(cmds.first()) {
                    let acc = command_accent(c.icon);
                    icons::paint(&painter, Rect::from_center_size(br.center() - vec2(0.0, 3.0), vec2(26.0, 26.0)), c.icon, t.icon, t.icon_fill, acc);
                }
                chevron(&painter, pos2(br.center().x, br.bottom() - 3.0), t.text_dim);
                let tip = format!("{} ({})", app.tr_label(panel), shown.len().max(cmds.len()));
                if fresp.on_hover_text(tip).clicked() && enabled_panel {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("sc_panel_open"), popup_id));
                    opened = true;
                }
            }
            let mut button_x = bx;
            for (c, button_width) in large.iter().zip(button_widths.iter().copied()).take(drawn) {
                // Only the family's default pin follows the part's choice. Explicitly pinned
                // variants remain exact commands, including in customised toolbars.
                let family = crate::sketch_families::family(c.id).filter(|f| sketch_create && f.commands.first().copied() == Some(c.id));
                let selected = family.and_then(|f| app.sketch_choices.selected(f)).unwrap_or(c);
                let whole = Rect::from_min_size(pos2(button_x, top + 2.0), vec2(button_width, BUTTON_HEIGHT));
                button_x += button_width + BUTTON_GAP;
                let mut br = whole;
                if family.is_some() {
                    br.max.y = whole.top() + 32.0;
                }
                let info = selected.info(&app.session);
                let resp = ui.interact(br, ui.id().with(("cmd", c.id)), Sense::click_and_drag());
                crate::scenario::publish_handle(&format!("toolbar:{}", c.id), br.center());
                // The button itself (`toolbar:<id>` is also a drop-down row's while one is open).
                crate::scenario::publish_handle(&format!("button:{}", c.id), br.center());
                crate::scenario::publish_rect(&format!("button:{}", c.id), whole);
                if selected.id != c.id {
                    crate::scenario::publish_handle(&format!("button:{}", selected.id), br.center());
                }
                if resp.drag_started() {
                    resp.dnd_set_drag_payload(ToolbarDrag { id: c.id, from_menu: false });
                }
                if let Some(d) = resp.dnd_hover_payload::<ToolbarDrag>()
                    && d.id != c.id
                {
                    painter
                        .line_segment([pos2(br.left() - 1.0, br.top() + 2.0), pos2(br.left() - 1.0, br.bottom() - 2.0)], Stroke::new(2.0, t.accent));
                }
                if let Some(d) = resp.dnd_release_payload::<ToolbarDrag>()
                    && d.id != c.id
                {
                    drop = Some((d.id, *panel, Some(c.id)));
                }
                if resp.secondary_clicked() {
                    menu = Some((resp.interact_pointer_pos().unwrap_or(br.left_bottom()), c.id));
                }
                let accent = crate::theme::TabAccent::for_tab(&t, &tab);
                let family_open = family.is_some_and(|f| egui::Popup::is_id_open(ui.ctx(), ui.id().with(("family", f.label)).with("popup")));
                if !finish && (resp.is_pointer_button_down_on() || running.as_deref() == Some(selected.id) || family_open) {
                    painter.rect(whole, t.radius_sm, accent.soft, Stroke::new(1.0, accent.color), egui::StrokeKind::Inside);
                } else if resp.hovered() && info.enabled {
                    painter.rect_filled(whole, t.radius_sm, t.hover);
                }
                let icon = if selected.id == "sketch.finish" { "finish" } else { selected.icon };
                let side = if finish { 30.0 } else { 26.0 };
                let icon_rect = Rect::from_center_size(pos2(br.center().x, whole.top() + 16.0), vec2(side, side));
                icons::paint_icon(&painter, icon_rect, icon, if info.enabled { 1.0 } else { 0.35 });
                let tr_label = app.tr_label(selected.label).to_string();
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled() && info.enabled, &tr_label));
                if ribbon_tooltip(app, resp, selected, info.disabled_reason.as_deref()).clicked() && info.enabled {
                    let id = selected.id;
                    app.start(id);
                }
                if let Some(f) = family {
                    let ar = Rect::from_min_max(pos2(whole.left(), whole.top() + 30.0), pos2(whole.right(), whole.top() + 48.0));
                    let arrow = ui.interact(ar, ui.id().with(("family", f.label)), Sense::click());
                    crate::scenario::publish_handle(&format!("family:{}", f.label), ar.center());
                    if arrow.hovered() {
                        painter.rect_filled(ar, t.radius_sm, t.hover);
                    }
                    // The title sits on the same line as the other buttons' labels.
                    let title_y = whole.top() + 39.0;
                    let galley = ribbon_button_galley(&painter, app.tr_label(f.label), ar.width() - 16.0, t.text);
                    painter.galley(pos2(ar.center().x - 4.0, title_y - galley.size().y / 2.0), galley, t.text);
                    chevron(&painter, pos2(ar.right() - 6.0, title_y + 0.5), t.text_dim);
                    arrow.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, app.tr_label(f.label)));
                    egui::Popup::menu(&arrow).show(|ui| {
                        let variants: Vec<_> = f.commands.iter().filter_map(|id| solidcraft_engine::find_command(id)).collect();
                        dropdown_keys(app, ui, &variants);
                        sketch_menu_rows(app, ui, &variants, &mut menu, &mut toggle);
                    });
                } else {
                    let label = app.tr_label(crate::workspace::ribbon_button_label(selected));
                    let color = if info.enabled { t.text } else { t.text_dim };
                    let galley = ribbon_button_galley(&painter, label, whole.width() - BUTTON_LABEL_PADDING, color);
                    painter.galley(pos2(whole.center().x, whole.top() + 32.0), galley, t.text);
                }
            }
            if !collapsed && drawn > large.len() {
                if !gallery.is_empty() {
                    // A quiet rule sets the grid apart from the large button before it.
                    painter.line_segment(
                        [pos2(button_x - 1.5, top + 8.0), pos2(button_x - 1.5, top + 46.0)],
                        Stroke::new(1.0, t.toolbar.lerp_to_gamma(t.border, 0.5)),
                    );
                }
                for c in &gallery {
                    let index = crate::workspace::CONSTRAINT_GALLERY.iter().position(|id| *id == c.id).unwrap_or(0);
                    let cell =
                        Rect::from_min_size(pos2(button_x + (index % 3) as f32 * 27.0, top + 6.0 + (index / 3) as f32 * 23.0), vec2(24.0, 20.0));
                    let info = c.info(&app.session);
                    let resp = ui.interact(cell, ui.id().with(("cmd", c.id)), Sense::click_and_drag());
                    let active = running.as_deref() == Some(c.id);
                    painter.rect(
                        cell,
                        4.0,
                        if active {
                            t.panel.lerp_to_gamma(command_accent(c.icon), 0.18)
                        } else if resp.hovered() && info.enabled {
                            t.hover
                        } else {
                            t.panel
                        },
                        Stroke::new(1.0, if active { command_accent(c.icon) } else { t.border }),
                        egui::StrokeKind::Inside,
                    );
                    icons::paint_icon(
                        &painter,
                        Rect::from_center_size(cell.center(), vec2(15.0, 15.0)),
                        c.icon,
                        if info.enabled { 1.0 } else { 0.35 },
                    );
                    for prefix in ["toolbar", "button", "ribbon:constraint"] {
                        crate::scenario::publish_handle(&format!("{prefix}:{}", c.id), cell.center());
                        crate::scenario::publish_rect(&format!("{prefix}:{}", c.id), cell);
                    }
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, info.enabled, app.tr_label(c.label)));
                    if resp.drag_started() {
                        resp.dnd_set_drag_payload(ToolbarDrag { id: c.id, from_menu: false });
                    }
                    if let Some(d) = resp.dnd_release_payload::<ToolbarDrag>()
                        && d.id != c.id
                    {
                        drop = Some((d.id, *panel, Some(c.id)));
                    }
                    if resp.secondary_clicked() {
                        menu = Some((resp.interact_pointer_pos().unwrap_or(cell.left_bottom()), c.id));
                    }
                    if ribbon_tooltip(app, resp, c, info.disabled_reason.as_deref()).clicked() && info.enabled {
                        app.start(c.id);
                    }
                }
            }
            // A 20 px label row is also the section's existing command-list button.
            let lr = Rect::from_min_size(pos2(px + 3.0, top + GROUP_HEIGHT - SECTION_HEIGHT - 3.0), vec2((width - 6.0).max(0.0), SECTION_HEIGHT));
            let lresp = ui.interact(lr, ui.id().with(("panel", panel)), Sense::click());
            crate::scenario::publish_handle(&format!("panel:{tab}:{panel}"), lr.center());
            crate::scenario::publish_handle(&format!("ribbon:section:{tab}:{panel}"), lr.center());
            crate::scenario::publish_rect(&format!("ribbon:section:{tab}:{panel}"), lr);
            let panel_open = ui.ctx().data(|d| d.get_temp::<egui::Id>(egui::Id::new("sc_panel_open"))) == Some(popup_id);
            let accent = crate::theme::TabAccent::for_tab(&t, &tab);
            let hot = lresp.hovered() && enabled_panel;
            if panel_open || hot {
                painter.rect_filled(lr, 4.0, if panel_open { accent.soft } else { t.hover });
            }
            let label = if finish { app.tr("ui.ribbon_exit").to_string() } else { ribbon_label(app, panel) };
            let label_width = painter.layout_no_wrap(label.clone(), FontId::proportional(11.5), t.text_dim).size().x;
            let ink = if panel_open { selected_ink(&accent) } else { t.text_dim };
            ribbon_text(ui, lr.center() - vec2(if finish { 0.0 } else { 6.0 }, 0.0), &label, 11.5, ink, panel_open || hot);
            if !finish {
                chevron(&painter, pos2(lr.center().x + label_width / 2.0 + 1.0, lr.center().y + 0.5), ink);
            }
            if lresp.clicked() && enabled_panel {
                ui.ctx().data_mut(|d| {
                    let key = egui::Id::new("sc_panel_open");
                    if d.get_temp::<egui::Id>(key) == Some(popup_id) {
                        d.remove::<egui::Id>(key);
                    } else {
                        d.insert_temp(key, popup_id);
                    }
                });
            }
            // Labels, gaps and margins append; actual buttons keep their insert-before target.
            // Respect the current layer so a dropdown covering the panel is not a drop target.
            let panel_rect = Rect::from_min_max(pos2(px, top + 2.0), pos2(px + width, r.bottom()));
            crate::scenario::publish_rect(&format!("panel:{tab}:{panel}"), panel_rect);
            let over_button = ui.input(|input| input.pointer.hover_pos()).is_some_and(|p| {
                let mut x = bx;
                button_widths.iter().take(drawn).any(|width| {
                    let rect = Rect::from_min_size(pos2(x, top + 2.0), vec2(*width, BUTTON_HEIGHT));
                    x += width + BUTTON_GAP;
                    rect.contains(p)
                })
            });
            if !over_button && ui.rect_contains_pointer(panel_rect) && egui::DragAndDrop::payload::<ToolbarDrag>(ui.ctx()).is_some() {
                painter.line_segment([pos2(px + width - 4.0, top + 4.0), pos2(px + width - 4.0, top + 38.0)], Stroke::new(2.0, t.accent));
                if ui.input(|input| input.pointer.any_released())
                    && let Some(d) = egui::DragAndDrop::take_payload::<ToolbarDrag>(ui.ctx())
                {
                    drop = Some((d.id, *panel, None));
                }
            }
            let open = ui.ctx().data(|d| d.get_temp::<egui::Id>(egui::Id::new("sc_panel_open"))) == Some(popup_id);
            if open {
                let cmds = if collapsed || trimmed { menu_commands(&cmds, &shown) } else { cmds.clone() };
                let area = crate::frame::area(popup_id).fixed_pos(lr.left_bottom()).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
                    section_frame(ui).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.set_min_width(200.0);
                        if *panel == "SELECT" {
                            selection_filter(app, ui);
                        } else if cmds.is_empty() {
                            ui.label(egui::RichText::new(crate::i18n::tr_label("Not available yet")).color(t.text_dim));
                        }
                        if sketch_create {
                            sketch_create_menu(app, ui, &cmds, &mut menu, &mut toggle);
                            section_footer(ui);
                            return;
                        }
                        let grouped = crate::workspace::grouped_commands(&tab, panel, &cmds);
                        let cmds: Vec<_> = grouped.iter().map(|(_, c)| *c).collect();
                        let row_w = dropdown_width(app, ui, &cmds);
                        ui.set_width(row_w);
                        // Up and Down move the keyboard focus through the enabled rows (from none
                        // focused: the first or the last), instead of egui's own spatial move.
                        let (down, up) = ui.input(|i| (i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowUp)));
                        if down || up {
                            let enabled: Vec<&str> = cmds.iter().filter(|c| c.info(&app.session).enabled).map(|c| c.id).collect();
                            let focused = ui.ctx().memory(|m| m.focused());
                            let at = enabled.iter().position(|id| focused == Some(dropdown_row_id(id)));
                            let n = enabled.len();
                            let next = match (at, down) {
                                (None, true) => Some(0),
                                (None, false) => n.checked_sub(1),
                                (Some(i), true) => Some((i + 1) % n.max(1)),
                                (Some(i), false) => Some((i + n.max(1) - 1) % n.max(1)),
                            };
                            if let Some(id) = next.and_then(|i| enabled.get(i)) {
                                ui.ctx().memory_mut(|m| {
                                    m.request_focus(dropdown_row_id(id));
                                    m.move_focus(egui::FocusDirection::None);
                                });
                            }
                        }
                        // The rows scroll when the drop-down would run past the window's bottom.
                        // `min_scrolled_height` lets the rows grow past the size the area kept from
                        // an earlier, shorter list (#874); they still shrink to fit their content.
                        let room = (ui.ctx().content_rect().bottom() - ui.cursor().top() - 64.0).max(ROW_H * 3.0);
                        egui::ScrollArea::vertical().id_salt("panel_menu_rows").max_height(room).min_scrolled_height(room).show(ui, |ui| {
                            for (heading, c) in &grouped {
                                if let Some(heading) = heading {
                                    section_heading(ui, heading, row_w);
                                }
                                let pinned = crate::toolbar_custom::pinned_on(app, &tab, c.id).is_some();
                                match dropdown_row(app, ui, c, row_w, pinned) {
                                    RowAction::None => {}
                                    RowAction::Menu(at) => menu = Some((at, c.id)),
                                    RowAction::TogglePin => toggle = Some(c.id),
                                    RowAction::Start => {
                                        app.start(c.id);
                                        ui.ctx().data_mut(|d| d.remove::<egui::Id>(egui::Id::new("sc_panel_open")));
                                    }
                                }
                            }
                        });
                        section_footer(ui);
                    });
                });
                popup_rect = Some(area.response.rect);
                crate::scenario::publish_rect("toolbar_dropdown", area.response.rect);
                // A drag out of the drop-down ends with a release elsewhere: not a click elsewhere.
                let dragging = egui::DragAndDrop::has_payload_of_type::<ToolbarDrag>(ui.ctx());
                if (area.response.clicked_elsewhere() && !lresp.clicked() && !opened && !dragging) || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    ui.ctx().data_mut(|d| d.remove::<egui::Id>(egui::Id::new("sc_panel_open")));
                }
            }
            px += width;
            painter.line_segment([pos2(px, top + 4.0), pos2(px, top + GROUP_HEIGHT - 3.0)], Stroke::new(1.0, t.border));
            px += PANEL_GAP;
        }
        if !overflow.is_empty() {
            let at = Rect::from_min_size(pos2(px.min(right - finish_width - 40.0), top + 4.0), vec2(38.0, BUTTON_HEIGHT));
            ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
                let response = ui.menu_button("»", |ui| {
                    for (panel, cmds) in &overflow {
                        ui.menu_button(*panel, |ui| {
                            if tab == "SKETCH" && *panel == "CREATE" {
                                sketch_create_menu(app, ui, cmds, &mut menu, &mut toggle);
                                return;
                            }
                            if *panel == "SELECT" {
                                selection_filter(app, ui);
                            }
                            for c in cmds {
                                let info = c.info(&app.session);
                                let response = ui.add_enabled(info.enabled, egui::Button::new(c.label).shortcut_text(c.shortcut.unwrap_or("")));
                                crate::scenario::publish_handle(&format!("toolbar:{}", c.id), response.rect.center());
                                if response.clicked() {
                                    app.start(c.id);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
                crate::scenario::publish_handle("toolbar:overflow", response.response.rect.center());
                crate::scenario::publish_rect("toolbar:overflow", response.response.rect);
                response.response.on_hover_text(crate::i18n::tr_label("More toolbar panels"));
            });
        }
        painter.line_segment([pos2(r.left(), r.bottom() - 0.5), pos2(r.right(), r.bottom() - 0.5)], Stroke::new(1.0, t.border));
        // Dragging a command: its icon follows the pointer; a toolbar button dragged off the
        // toolbar (or into the open drop-down) shows it will be removed.
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let over_popup = |p: egui::Pos2| popup_rect.is_some_and(|pr| pr.contains(p));
        let leaves = |p: egui::Pos2| !r.contains(p) || over_popup(p);
        if let (Some(d), Some(p)) = (egui::DragAndDrop::payload::<ToolbarDrag>(ui.ctx()), pointer) {
            let removing = !d.from_menu && leaves(p);
            drag_ghost(ui.ctx(), d.id, p, removing);
            if ui.input(|i| i.pointer.any_released()) && drop.is_none() {
                egui::DragAndDrop::clear_payload(ui.ctx());
                if removing {
                    crate::toolbar_custom::remove(app, &tab, d.id);
                }
            }
        }
        if let Some((id, panel, before)) = drop {
            crate::toolbar_custom::reorder(app, &tab, id, panel, before);
        }
        if let Some(id) = toggle {
            crate::toolbar_custom::toggle(app, &tab, id);
        }
        if let Some((at, id)) = menu {
            ui.ctx().data_mut(|d| d.remove::<egui::Id>(egui::Id::new("sc_panel_open")));
            crate::context_menu::open_for(app, at, crate::context_menu::Target::ToolbarCommand { id: id.to_string() });
        }
    });
}

/// A short divider between the brand, file actions and document tabs.
fn app_bar_separator(ui: &egui::Ui, bar: Rect, x: f32, group: &str) {
    let rect = Rect::from_center_size(pos2(x, bar.center().y), vec2(1.0, 20.0));
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        Stroke::new(1.0, Tokens::get().app_bar.lerp_to_gamma(Tokens::get().app_bar_separator, 0.4)),
    );
    crate::scenario::publish_rect(&format!("titlebar:separator:{group}"), rect);
}

/// The chat panel's compact entry point. State stays in agent_chat; the bar only paints it.
fn assistant_button(app: &mut SolveApp, ui: &mut egui::Ui, at: Rect) {
    use crate::agent_chat::AssistantMark;
    let t = Tokens::get();
    let resp = ui.interact(at, ui.id().with("assistant_button"), Sense::click());
    crate::scenario::publish_handle("titlebar:assistant", at.center());
    crate::scenario::publish_rect("titlebar:assistant", at);
    let mark = crate::agent_chat::assistant_mark(app);
    let tip = crate::agent_chat::assistant_tip(mark);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &tip));
    // The highlight is a 26 px square, like the other icon buttons; the mark sits on the bubble's corner.
    let mut ring = t.app_bar;
    if app.agent_chat.open || resp.hovered() || resp.has_focus() {
        let fill = if app.agent_chat.open { t.app_bar_hover_strong } else { t.app_bar_hover_button };
        ui.painter().rect_filled(at.shrink(3.0), t.radius_sm, fill);
        ring = t.app_bar.blend(fill);
    }
    icons::paint_app_bar(ui.painter(), at.shrink(7.0), "assistant", 1.0);
    let c = at.min + vec2(23.5, 9.5);
    match mark {
        AssistantMark::Quiet => {}
        AssistantMark::Ready | AssistantMark::NeedsAuth | AssistantMark::Stopped => {
            let color = match mark {
                AssistantMark::Ready => t.agent_ok,
                AssistantMark::NeedsAuth => t.warning,
                _ => t.error,
            };
            ui.painter().circle_filled(c, 5.5, ring);
            ui.painter().circle_filled(c, 4.0, color);
        }
        AssistantMark::Busy => {
            let phase = if app.preferences.look_at_anim {
                ui.ctx().request_repaint();
                ui.input(|i| i.time) as f32 * 3.0
            } else {
                0.0
            };
            ui.painter().circle_filled(c, 6.0, ring);
            let points = (0..=24)
                .map(|i| {
                    let angle = phase + i as f32 / 24.0 * std::f32::consts::TAU * 0.75;
                    c + vec2(angle.cos(), angle.sin()) * 4.0
                })
                .collect();
            ui.painter().add(egui::Shape::line(points, Stroke::new(1.8, t.agent_busy)));
        }
        AssistantMark::Waiting(count) => {
            let text = count.to_string();
            let font = FontId::proportional(t.font_badge);
            let galley = ui.painter().layout_no_wrap(text, font, t.app_bar);
            let badge = Rect::from_center_size(c, vec2((galley.size().x + 6.0).max(14.0), 14.0));
            ui.painter().rect_filled(badge.expand(1.5), 8.5, ring);
            ui.painter().rect_filled(badge, 7.0, t.warning);
            ui.painter().galley(badge.center() - galley.size() / 2.0, galley, t.app_bar);
        }
    }
    if resp.on_hover_text(tip).clicked() {
        crate::agent_chat::toggle(app);
    }
}

/// What a click on a panel drop-down row asks for.
enum RowAction {
    None,
    Start,
    Menu(egui::Pos2),
    TogglePin,
}

const ROW_H: f32 = 30.0;

const CONSTRAINT_GRID_WIDTH: f32 = 82.0;

/// Registry parameter syntax is not a user-facing description. Reuse existing catalog prose
/// where present; other commands intentionally show only their name and registered shortcut.
fn ribbon_tooltip(app: &SolveApp, response: egui::Response, command: &solidcraft_engine::CommandSpec, disabled: Option<&str>) -> egui::Response {
    response.on_hover_ui(|ui| {
        let t = Tokens::get();
        let name = app.tr_label(command.label);
        let name_width = ui.painter().layout_no_wrap(name.to_string(), FontId::proportional(14.0), t.text).size().x;
        let key_width = command
            .shortcut
            .map(|key| ui.painter().layout_no_wrap(key.to_string(), FontId::monospace(12.0), t.text_dim).size().x + 24.0)
            .unwrap_or(0.0);
        ui.set_width(280.0_f32.max(name_width + key_width + 12.0));
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(name_width, 20.0), Sense::hover());
            ribbon_text(ui, rect.center(), name, 14.0, t.text, true);
            if let Some(key) = command.shortcut {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::Frame::new()
                        .fill(t.panel_header)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(3.0)
                        .inner_margin(egui::Margin::symmetric(5, 1))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(key).monospace().color(t.text_dim));
                        });
                });
            }
        });
        let description = match command.id {
            "render.scene" => Some(app.tr("render.preview_help")),
            "render.start" => Some(app.tr("render.image_help")),
            _ => None,
        };
        if let Some(description) = description {
            ui.add(egui::Label::new(description).wrap());
        }
        if let Some(reason) = disabled {
            ui.add(egui::Label::new(egui::RichText::new(reason).color(t.text_dim)).wrap());
        }
    })
}

fn section_frame(ui: &egui::Ui) -> egui::Frame {
    let t = Tokens::get();
    egui::Frame::popup(ui.style()).fill(t.panel).stroke(Stroke::new(1.0, t.border)).corner_radius(8.0)
}

fn section_heading(ui: &mut egui::Ui, label: &str, width: f32) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
    ui.painter().text(
        rect.left_center() + vec2(6.0, 0.0),
        Align2::LEFT_CENTER,
        crate::i18n::tr_label(label).to_uppercase(),
        FontId::proportional(11.0),
        t.text_dim,
    );
}

fn section_footer(ui: &mut egui::Ui) {
    ui.separator();
    ui.add_space(4.0);
    ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tr("ui.ribbon_pin_footer")).size(11.0).color(Tokens::get().text_dim)).wrap());
    ui.add_space(4.0);
}

/// Stable row id inside and outside the scrolling list, for keyboard focus.
fn dropdown_row_id(id: &str) -> egui::Id {
    egui::Id::new(("dropdown_row", id))
}

/// How wide a panel drop-down's rows are: the widest label and shortcut, at least 200.
fn dropdown_width(app: &SolveApp, ui: &egui::Ui, cmds: &[&'static solidcraft_engine::CommandSpec]) -> f32 {
    let t = Tokens::get();
    let font = FontId::proportional(t.font_button);
    cmds.iter()
        .map(|c| {
            let label = ui.painter().layout_no_wrap(app.tr_label(c.label).to_string(), font.clone(), t.text).size().x;
            let key = c
                .shortcut
                .map(|k| ui.painter().layout_no_wrap(k.to_string(), FontId::monospace(t.font_button), t.text).size().x + 24.0)
                .unwrap_or(0.0);
            6.0 + 20.0 + 8.0 + label + key + 30.0
        })
        .fold(200.0, f32::max)
}

/// One command row of a panel drop-down: icon, label and shortcut, with the hover background
/// under the pointer or keyboard focus (like every menu).
fn dropdown_row(app: &SolveApp, ui: &mut egui::Ui, c: &'static solidcraft_engine::CommandSpec, width: f32, pinned: bool) -> RowAction {
    let t = Tokens::get();
    let info = c.info(&app.session);
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
    // Every row drags onto the toolbar (to pin it there); enabled rows also start their command.
    let sense = if info.enabled { Sense::click_and_drag() } else { Sense::drag() };
    let resp = ui.interact(rect, dropdown_row_id(c.id), sense);
    // Focus can settle on the following frame, after the key event has gone.
    // Keep the focused row visible even then in a drop-down taller than the window.
    if resp.has_focus() && !ui.clip_rect().contains_rect(rect) {
        resp.scroll_to_me_animation(None, egui::style::ScrollAnimation::none());
    }
    if resp.drag_started() {
        resp.dnd_set_drag_payload(ToolbarDrag { id: c.id, from_menu: true });
    }
    // The pin at the row's end (above the row, so it takes its own clicks).
    let pr = Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y), vec2(20.0, 20.0));
    let pin = ui.interact(pr, ui.id().with(("dropdown_pin", c.id)), Sense::click());
    let pointer_on = (resp.hovered() || pin.hovered()) && !egui::DragAndDrop::has_any_payload(ui.ctx());
    let label = app.tr_label(c.label).to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled() && info.enabled, &label));
    crate::scenario::publish_handle(&format!("toolbar:{}", c.id), rect.center());
    crate::scenario::publish_handle(&format!("dropdown:{}", c.id), rect.center());
    let hot = info.enabled && (pointer_on || resp.has_focus());
    let p = ui.painter();
    if hot {
        p.rect_filled(rect, 3.0, t.hover);
        crate::scenario::publish_handle(&format!("dropdown_hot:{}", c.id), rect.center());
    }
    // The icon keeps its semantic accent in a drop-down as well as on the ribbon.
    let (ink, fill, acc, text) = if info.enabled {
        let acc = command_accent(c.icon);
        (t.icon, t.icon_fill, acc, t.text)
    } else {
        (t.border, t.panel_header, t.border, t.text_dim)
    };
    let ir = Rect::from_center_size(pos2(rect.left() + 6.0 + 10.0, rect.center().y), vec2(20.0, 20.0));
    icons::paint(p, ir, c.icon, ink, fill, acc);
    p.text(pos2(ir.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, &label, FontId::proportional(t.font_button), text);
    if let Some(k) = c.shortcut {
        p.text(pos2(pr.left() - 4.0, rect.center().y), Align2::RIGHT_CENTER, k, FontId::monospace(t.font_button), t.text_dim);
    }
    if pointer_on || resp.has_focus() {
        if pin.hovered() {
            p.rect_filled(pr, 3.0, t.border.gamma_multiply(0.6));
        }
        pin_icon(p, pr, pinned, if pinned { t.accent } else { t.text_dim });
        crate::scenario::publish_handle(&format!("dropdown_pin:{}", c.id), pr.center());
    }
    let tip = if pinned { "Remove from Toolbar" } else { "Pin to Toolbar" };
    if pin.on_hover_text(tip).clicked() {
        return RowAction::TogglePin;
    }
    let resp = match &info.disabled_reason {
        Some(why) if !info.enabled => resp.on_hover_text(why),
        _ => resp,
    };
    if resp.secondary_clicked() {
        return RowAction::Menu(resp.interact_pointer_pos().unwrap_or(rect.left_bottom()));
    }
    if resp.clicked() && info.enabled {
        return RowAction::Start;
    }
    RowAction::None
}

/// Creation menus keep related variants together, including collapsed and overflow panels.
fn sketch_create_menu(
    app: &mut SolveApp,
    ui: &mut egui::Ui,
    cmds: &[&'static solidcraft_engine::CommandSpec],
    menu: &mut Option<(egui::Pos2, &'static str)>,
    toggle: &mut Option<&'static str>,
) {
    let room = (ui.ctx().content_rect().bottom() - ui.cursor().top() - 64.0).max(ROW_H * 3.0);
    egui::ScrollArea::vertical().id_salt("sketch_create_rows").max_height(room).show(ui, |ui| {
        for f in crate::sketch_families::FAMILIES {
            let variants: Vec<_> = f.commands.iter().filter_map(|id| cmds.iter().find(|c| c.id == *id).copied()).collect();
            if !variants.is_empty() {
                let response = ui.menu_button(app.tr_label(f.label).to_string(), |ui| {
                    dropdown_keys(app, ui, &variants);
                    sketch_menu_rows(app, ui, &variants, menu, toggle);
                });
                crate::scenario::publish_handle(&format!("create_family:{}", f.label), response.response.rect.center());
            }
        }
        ui.separator();
        let rest: Vec<_> = cmds.iter().copied().filter(|c| crate::sketch_families::family(c.id).is_none()).collect();
        dropdown_keys(app, ui, &rest);
        sketch_menu_rows(app, ui, &rest, menu, toggle);
    });
}

/// Family popups use the same ordered arrow-key navigation as a panel's command list.
fn dropdown_keys(app: &SolveApp, ui: &egui::Ui, cmds: &[&'static solidcraft_engine::CommandSpec]) {
    let (down, up) = ui.input(|i| (i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowUp)));
    if !down && !up {
        return;
    }
    // A submenu renders before the remaining rows of its parent. Handle a key once, but
    // leave the event available for the focused row's scroll-into-view behaviour.
    let handled = egui::Id::new("sketch_menu_key_pass");
    let pass = ui.ctx().cumulative_pass_nr();
    if ui.ctx().data(|d| d.get_temp::<u64>(handled)) == Some(pass) {
        return;
    }
    ui.ctx().data_mut(|d| d.insert_temp(handled, pass));
    let enabled: Vec<_> = cmds.iter().filter(|c| c.info(&app.session).enabled).collect();
    if enabled.is_empty() {
        return;
    }
    let focused = ui.ctx().memory(|m| m.focused());
    let at = enabled.iter().position(|c| focused == Some(dropdown_row_id(c.id)));
    let n = enabled.len();
    let next = match (at, down) {
        (None, true) => 0,
        (None, false) => n - 1,
        (Some(i), true) => (i + 1) % n,
        (Some(i), false) => (i + n - 1) % n,
    };
    if let Some(c) = enabled.get(next) {
        ui.ctx().memory_mut(|m| {
            m.request_focus(dropdown_row_id(c.id));
            m.move_focus(egui::FocusDirection::None);
        });
    }
}

fn sketch_menu_rows(
    app: &mut SolveApp,
    ui: &mut egui::Ui,
    cmds: &[&'static solidcraft_engine::CommandSpec],
    menu: &mut Option<(egui::Pos2, &'static str)>,
    toggle: &mut Option<&'static str>,
) {
    let width = dropdown_width(app, ui, cmds);
    for c in cmds {
        let pinned = crate::toolbar_custom::pinned_on(app, "SKETCH", c.id).is_some();
        match dropdown_row(app, ui, c, width, pinned) {
            RowAction::None => {}
            RowAction::Menu(at) => *menu = Some((at, c.id)),
            RowAction::TogglePin => *toggle = Some(c.id),
            RowAction::Start => {
                app.start(c.id);
                ui.ctx().data_mut(|d| d.remove::<egui::Id>(egui::Id::new("sc_panel_open")));
                ui.close();
            }
        }
    }
}

/// A command being dragged: a toolbar button, or a row of a panel drop-down (`from_menu`).
#[derive(Clone, Copy, Debug)]
struct ToolbarDrag {
    id: &'static str,
    from_menu: bool,
}

/// The dragged command's icon under the pointer, crossed out when releasing removes it.
fn drag_ghost(ctx: &egui::Context, id: &str, at: egui::Pos2, removing: bool) {
    let t = Tokens::get();
    let Some(c) = solidcraft_engine::find_command(id) else { return };
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("sc_toolbar_drag")));
    let r = Rect::from_center_size(at + vec2(14.0, 16.0), vec2(30.0, 30.0));
    p.rect(r, t.radius_sm, t.toolbar.gamma_multiply(0.92), Stroke::new(1.0, if removing { t.error } else { t.accent }), egui::StrokeKind::Inside);
    icons::paint(&p, r.shrink(5.0), c.icon, t.icon, t.icon_fill, command_accent(c.icon));
    if removing {
        let s = Stroke::new(2.0, t.error);
        let x = Rect::from_center_size(r.right_top(), vec2(9.0, 9.0));
        p.circle_filled(x.center(), 7.0, t.toolbar);
        crate::icons::glyphs::cross(&p, x, s);
    }
    crate::scenario::publish_handle(if removing { "toolbar_drag:remove" } else { "toolbar_drag" }, at);
}

/// The SELECT panel: what a click in the view picks.
fn selection_filter(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    ui.label(egui::RichText::new(crate::i18n::tr_label("Selection filter")).color(t.text_dim).size(t.font_small));
    ui.checkbox(&mut app.ui.pick_bodies, crate::i18n::tr_label("Select bodies (a face picks its body)"));
    for (k, label) in [
        ("faces", "Faces"),
        ("edges", "Edges"),
        ("vertices", "Vertices"),
        ("sketch", "Sketch geometry and profiles"),
        ("sketch_curves", "Sketch curves"),
        ("sketch_points", "Sketch points"),
        ("dimensions", "Sketch dimensions"),
        ("constraints", "Sketch constraints"),
    ] {
        let mut on = !app.ui.pick_off.iter().any(|x| x == k);
        if ui.checkbox(&mut on, crate::i18n::tr_label(label)).changed() {
            app.ui.pick_off.retain(|x| x != k);
            if !on {
                app.ui.pick_off.push(k.to_string());
            }
        }
    }
}

/// Global keyboard shortcuts (when no text field has focus).
pub fn shortcuts(app: &mut SolveApp, ctx: &egui::Context) {
    if app.file_load.busy() {
        return;
    }
    // The Keyboard Shortcuts window is waiting for a key: it is not a shortcut.
    if crate::keymap::capturing(app) {
        return;
    }
    // A dimension value that is taking the keyboard: letters belong to it (a parameter name
    // starting with L or D), even on the frame before its field holds the focus.
    if ctx.egui_wants_keyboard_input() || crate::dim_view::editing() || crate::dialogs_save_mesh::popup_open(ctx) {
        return;
    }
    let (keys, mods) = ctx.input(|i| {
        let keys: Vec<egui::Key> =
            i.events.iter().filter_map(|e| if let egui::Event::Key { key, pressed: true, .. } = e { Some(*key) } else { None }).collect();
        (keys, i.modifiers)
    });
    // Home hides the design: only New, Open and Preferences work there.
    let shown = design_shown(app);
    if shown {
        crate::edit_clipboard::handle(app, ctx);
    } else {
        crate::edit_clipboard::flush(ctx);
    }
    for k in keys {
        use egui::Key;
        if shown && crate::navigation::view_key(app, ctx, k, mods) {
            continue;
        }
        if mods.command {
            match k {
                Key::N => crate::documents::new_design(app),
                Key::Comma => crate::prefs::open_at(app, 0),
                Key::O => {
                    app.pick_open(|app, path| {
                        if let Some(path) = path {
                            app.open_path_async(&path);
                        }
                    });
                }
                _ if !shown => {}
                Key::Z => drop(app.run("edit.undo", json!({}))),
                Key::Y => drop(app.run("edit.redo", json!({}))),
                // Copy, Cut and Paste (`edit_clipboard::handle`).
                Key::C | Key::X | Key::V if !mods.shift && !mods.alt => {}
                Key::S => save(app),
                _ => crate::keymap::press(app, &crate::keymap::key_name(k, mods)),
            }
            continue;
        }
        if !shown {
            continue;
        }
        match k {
            Key::Escape => {
                app.esc_handled = true;
                if crate::sketch_text_move::cancel_drag(app) {
                    continue;
                }
                if crate::select_other::is_open(app) {
                    crate::select_other::close(app);
                } else if app.tool.is_some() {
                    crate::tools::finish(app);
                } else if app.dialog.is_some() {
                    crate::dialogs::cancel(app);
                } else if !app.session.selection.is_empty() || app.session.active_sketch.is_none() {
                    app.tree.picked_joints.clear();
                    let _ = app.run("select.clear", json!({}));
                }
                // An open sketch with nothing left to cancel stays open (#1804). Finish Sketch
                // is what leaves it, so a second Escape after putting a tool down is harmless.
            }
            Key::S if !mods.shift && !mods.alt => {
                let at = search_anchor(ctx).or_else(|| ctx.input(|i| i.pointer.latest_pos()));
                crate::shortcut_box::open(app, at);
            }
            Key::V => crate::context_menu::toggle_visibility(app),
            Key::F2 => {
                let at = ctx.input(|i| i.pointer.latest_pos()).unwrap_or(ctx.content_rect().center());
                crate::context_menu::rename_selection(app, at);
            }
            Key::F6 => app.animate_view("fit"),
            Key::Delete | Key::Backspace => {
                if !crate::timeline_left::delete_selected_folders(app) {
                    crate::viewport::delete_selection(app);
                }
            }
            _ => crate::keymap::press(app, &crate::keymap::key_name(k, mods)),
        }
    }
}

#[cfg(test)]
mod header_report_tests {
    use super::*;

    #[test]
    fn file_menu_json_save_and_export_use_commands_and_keep_distinct_paths() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":2,"width":3,"height":4})).unwrap();
        let dir = h.dir.clone();
        h.app.services.pick_save =
            Some(Box::new(move |name, _extensions| Box::pin(std::future::ready(Some(dir.join(name).to_string_lossy().into_owned())))));
        h.frames(3);
        h.step(&json!({"click":{"handle":"appbar:folder"}})).unwrap();
        h.step(&json!({"widget":"Save As JSON…"})).unwrap();
        let saved = h.app.session.path.clone().unwrap();
        let bytes = std::fs::read(&saved).unwrap();
        assert!(!solidcraft_engine::io::is_design_container(&bytes));
        assert_eq!(solidcraft_engine::io::read_design(&bytes).unwrap(), *h.app.session.doc);
        h.app.session.doc_mut().name = "Exported copy".into();
        h.frames(3);
        h.step(&json!({"click":{"handle":"appbar:folder"}})).unwrap();
        h.step(&json!({"widget":"Export Design as JSON…"})).unwrap();
        assert_eq!(h.app.session.path.as_deref(), Some(saved.as_str()));
        assert!(h.dir.join("Exported copy.json").is_file());
        h.app.run("file.save", json!({})).unwrap();
        assert!(!solidcraft_engine::io::is_design_container(&std::fs::read(saved).unwrap()), "subsequent Save keeps a .json choice");
    }

    fn sizes(buttons: &[usize]) -> Vec<PanelSize> {
        buttons.iter().map(|&b| PanelSize { buttons: vec![ButtonSize { width: BUTTON_MIN_WIDTH, min_width: BUTTON_MIN_WIDTH }; b] }).collect()
    }

    fn used_with_more(plans: &[Plan]) -> f32 {
        used(plans) + if plans.iter().any(|p| p.fit == Fit::More) { MORE_ROOM } else { 0.0 }
    }

    #[test]
    fn panels_give_up_one_button_at_a_time_then_move_to_more_and_none_is_dropped() {
        // A panel with 14 buttons and three default ones.
        let panels = sizes(&[14, 4, 2, 2]);
        let full: f32 = [14, 4, 2, 2].iter().map(|&b| panel_width(&vec![BUTTON_MIN_WIDTH; b]) + PANEL_GAP).sum();
        let all = |plans: &[Plan]| plans.iter().map(|p| (p.fit, p.shown)).collect::<Vec<_>>();
        assert_eq!(all(&fit(&panels, full)), [(Fit::Buttons, 14), (Fit::Buttons, 4), (Fit::Buttons, 2), (Fit::Buttons, 2)]);
        // A little short: the widest panel hands one button to its drop-down; the others keep all theirs.
        assert_eq!(all(&fit(&panels, full - 10.0)), [(Fit::Buttons, 13), (Fit::Buttons, 4), (Fit::Buttons, 2), (Fit::Buttons, 2)]);
        // Every panel down to one button (collapsed) fits.
        let collapsed = 4.0 * (COLLAPSED_WIDTH + PANEL_GAP);
        assert_eq!(all(&fit(&panels, collapsed)), [(Fit::Collapsed, 1); 4]);
        // Narrower: the last one goes into the » menu, and the room it frees goes back to buttons.
        let more = fit(&panels, collapsed - 1.0);
        assert_eq!(more.iter().map(|p| p.fit).collect::<Vec<_>>(), [Fit::Collapsed, Fit::Collapsed, Fit::Collapsed, Fit::More]);
        assert!(more.iter().take(3).all(|p| p.shown >= 1));
        let more = fit(&sizes(&[14, 4, 2, 2, 2, 2]), 5.0 * (COLLAPSED_WIDTH + PANEL_GAP) + MORE_ROOM - 1.0);
        assert_eq!(more.last().unwrap().fit, Fit::More);
        assert!(more.iter().any(|p| p.fit == Fit::Buttons && p.shown > 1), "overflow room restores buttons");
        // Then all of them.
        assert!(fit(&panels, 0.0).iter().all(|p| p.fit == Fit::More));
        assert_eq!(fit(&panels, f32::NAN).len(), 4);
        // No cap: every additional button keeps its width and gap.
        assert_eq!(panel_width(&[BUTTON_MIN_WIDTH; 30]) - panel_width(&[BUTTON_MIN_WIDTH; 20]), 10.0 * (BUTTON_MIN_WIDTH + BUTTON_GAP));
    }

    /// #2108: whatever the room, the toolbar uses it. It never leaves more than one button's width
    /// blank while something is hidden, never runs past the room, and shrinks steadily as the
    /// room does (so a few pixels never flip it back and forth).
    #[test]
    fn panels_fill_the_room_they_have_at_every_width() {
        let layouts = [
            // SOLID: CREATE, MODIFY, ASSEMBLE, CONFIGURE, CONSTRUCT, INSPECT, INSERT, SELECT.
            vec![(4, false), (4, false), (3, false), (2, false), (2, false), (2, false), (2, false), (0, false)],
            // SKETCH: labelled CREATE, then MODIFY … FINISH SKETCH.
            vec![(8, true), (2, false), (3, false), (6, false), (2, false), (2, false), (0, false), (1, false)],
            // Many pins on one panel.
            vec![(14, false), (4, false), (2, false), (2, false)],
        ];
        for layout in layouts {
            let panels: Vec<PanelSize> = layout
                .iter()
                .map(|&(buttons, labelled)| PanelSize {
                    buttons: (0..buttons)
                        .map(|i| ButtonSize {
                            width: if labelled { BUTTON_MAX_WIDTH - (i % 3) as f32 * 5.0 } else { BUTTON_MIN_WIDTH },
                            min_width: BUTTON_MIN_WIDTH,
                        })
                        .collect(),
                })
                .collect();
            let mut last: Option<(f32, usize)> = None;
            for room in (0..=2000).rev().map(|r| r as f32) {
                let plans = fit(&panels, room);
                let width = used_with_more(&plans);
                let hidden = plans
                    .iter()
                    .zip(&panels)
                    .any(|(p, s)| p.fit != Fit::Buttons || p.shown < s.buttons.len() || p.widths.iter().zip(&s.buttons).any(|(w, b)| *w < b.width));
                let more = plans.iter().filter(|p| p.fit == Fit::More).count();
                if more < panels.len() {
                    assert!(width <= room + 0.01, "{layout:?} at {room}: {width} overruns");
                }
                // One button: an icon's stride, or a collapsed panel's when the next panel waits in the » menu.
                let button = if more > 0 { COLLAPSED_WIDTH + PANEL_GAP } else { BUTTON_MIN_WIDTH + BUTTON_GAP };
                if hidden && more < panels.len() {
                    assert!(room - width < button, "{layout:?} at {room}: {} px blank while something is hidden", room - width);
                }
                // Fewer pixels never bring a panel back out of the » menu.
                if let Some((_, prev_more)) = last {
                    assert!(more >= prev_more, "{layout:?} at {room}: a panel came back out of the » menu");
                }
                last = Some((width, more));
                // A panel shows its icons: one at the very least.
                for (p, s) in plans.iter().zip(&panels) {
                    if p.fit != Fit::More && !s.buttons.is_empty() {
                        assert!(p.shown >= 1, "{layout:?} at {room}: a panel without icons");
                    }
                }
            }
        }
    }

    #[test]
    fn variable_width_buttons_narrow_before_rightmost_equal_panels_lose_buttons() {
        let panels = vec![PanelSize {
            buttons: vec![
                ButtonSize { width: 76.0, min_width: BUTTON_MIN_WIDTH },
                ButtonSize { width: 63.0, min_width: BUTTON_MIN_WIDTH },
                ButtonSize { width: 54.0, min_width: BUTTON_MIN_WIDTH },
            ],
        }];
        let full = used(&panels.iter().map(Plan::full).collect::<Vec<_>>());
        let narrow = fit(&panels, full - 1.0);
        assert_eq!(narrow[0].shown, 3);
        assert_eq!(narrow[0].widths, [75.0, 62.0, 54.0]);
        let minimum = panel_width(&[BUTTON_MIN_WIDTH; 3]) + PANEL_GAP;
        let narrow = fit(&panels, minimum);
        assert_eq!(narrow[0].shown, 3);
        assert_eq!(narrow[0].widths, [BUTTON_MIN_WIDTH; 3]);
        assert_eq!(fit(&panels, minimum - 1.0)[0].shown, 2);

        let equal = sizes(&[3, 3]);
        let full = used(&equal.iter().map(Plan::full).collect::<Vec<_>>());
        assert_eq!(fit(&equal, full - 1.0).iter().map(|p| p.shown).collect::<Vec<_>>(), [3, 2]);
    }

    #[test]
    fn constraint_gallery_stays_fixed_and_hands_all_cells_to_the_menu_together() {
        let panels = vec![PanelSize {
            buttons: vec![
                ButtonSize { width: BUTTON_MAX_WIDTH, min_width: BUTTON_MIN_WIDTH },
                ButtonSize { width: CONSTRAINT_GRID_WIDTH, min_width: CONSTRAINT_GRID_WIDTH },
            ],
        }];
        let minimum = panel_width(&[BUTTON_MIN_WIDTH, CONSTRAINT_GRID_WIDTH]) + PANEL_GAP;
        let narrow = fit(&panels, minimum);
        assert_eq!(narrow[0].shown, 2);
        assert_eq!(narrow[0].widths, [BUTTON_MIN_WIDTH, CONSTRAINT_GRID_WIDTH]);
        let collapsed = fit(&panels, minimum - 1.0);
        assert_eq!((collapsed[0].fit, collapsed[0].shown), (Fit::Collapsed, 1));
    }

    fn captured_frame(h: &mut crate::scenario::Harness) {
        let token = h.app.shot_token;
        crate::issue_report::receive(&mut h.app, token, std::sync::Arc::new(egui::ColorImage::new([2, 2], vec![Color32::GREEN; 4])));
        h.frames(3);
    }

    #[test]
    fn panel_accents_share_the_tab_mapping_but_constraints_keep_their_role() {
        for dark in [false, true] {
            for vision in crate::theme::ColorVision::ALL {
                let t = Tokens::themed(dark, vision);
                for tab in TABS.iter().copied().chain(["SKETCH", "FORM", "RENDER"]) {
                    assert_eq!(panel_accent(&t, tab, "CREATE"), crate::theme::TabAccent::for_tab(&t, tab).color);
                    assert_eq!(panel_accent(&t, tab, "CONSTRAIN"), t.glyph_text);
                }
            }
        }
    }

    #[test]
    fn header_report_reserves_controls_at_normal_and_double_scale_widths() {
        for physical_width in [900.0, 1400.0] {
            for scale in [1.0, 2.0] {
                let width = physical_width / scale;
                let bar = Rect::from_min_size(egui::Pos2::ZERO, vec2(width, APP_BAR_HEIGHT));
                for captions in [0.0, crate::titlebar::WIDTH] {
                    // The English label, and a longer one (Russian is about 125 px).
                    for label_width in [80.0, 125.0] {
                        let (report, labeled) = report_rect(bar, captions, 354.0, label_width);
                        assert!(bar.contains_rect(report));
                        let prefs = Rect::from_center_size(pos2(report.left() - 52.0, APP_BAR_HEIGHT / 2.0), vec2(32.0, 32.0));
                        let help = Rect::from_center_size(pos2(help_x(report), APP_BAR_HEIGHT / 2.0), vec2(32.0, 32.0));
                        assert!(!report.intersects(prefs) && !report.intersects(help) && !help.intersects(prefs));
                        assert!(prefs.right() < help.left() && help.right() < report.left());
                        let full = REPORT_TEXT_X + label_width + 11.0;
                        assert_eq!(
                            labeled,
                            width - captions - if width < 640.0 { 8.0 } else { RIGHT_GROUP } - CLUSTER_BUTTONS - 354.0 >= 216.0 + full
                        );
                        if labeled {
                            assert!(REPORT_TEXT_X + label_width < report.width(), "{label_width}: the text overruns its button");
                        }
                    }
                }
            }
        }
    }

    /// The 34 px bar groups brand/home, File/Save/Undo/Redo, documents, then the right cluster.
    #[test]
    fn title_bar_keeps_tabs_left_and_help_right() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.custom_titlebar = true;
        h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
        h.call("ui.resize", json!({"width":1400,"height":720}));
        h.frames(3);
        for gone in ["appbar:sun", "appbar:moon", "toolbar:ai"] {
            assert!(crate::scenario::handle_at(gone).is_none(), "{gone} is no longer in the title bar");
        }
        let at = |name: &str| crate::scenario::handle_at(name).unwrap_or_else(|| panic!("{name}")).x;
        let [report, _] = h.widget("Report a bug").unwrap();
        let file = h.widget("File").expect("the File menu has a label");
        assert_eq!(file[0] as f32, at("appbar:file"));
        assert_eq!(APP_BAR_HEIGHT, 34.0);
        let rect = |name: &str| crate::scenario::rect_of(name).unwrap();
        let first = rect("titlebar:separator:brand");
        let second = rect("titlebar:separator:documents");
        assert_eq!(first.size(), vec2(1.0, 20.0));
        assert_eq!(second.size(), vec2(1.0, 20.0));
        assert_eq!(first.center().y, 17.0);
        assert_eq!(second.center().y, 17.0);
        assert!(rect("titlebar:home").right() < first.left());
        assert!(first.right() < rect("appbar:file").left());
        assert!(at("appbar:file") < at("titlebar:save") && at("titlebar:save") < at("appbar:undo") && at("appbar:undo") < at("appbar:redo"));
        assert!(rect("appbar:redo").right() < second.left());
        assert!(second.right() < rect("tab:design:0").left());
        assert!(at("tab:design:0") < at("titlebar:assistant"));
        assert!(at("titlebar:assistant") < at("titlebar:preferences"));
        for name in ["titlebar:assistant", "titlebar:preferences", "appbar:help"] {
            assert_eq!(rect(name).size(), vec2(32.0, 32.0));
            assert!(rect(name).top() >= 0.0 && rect(name).bottom() <= APP_BAR_HEIGHT);
        }
        assert!(at("titlebar:preferences") < at("appbar:help") && (at("appbar:help") as f64) < report);
        assert!(at("appbar:home") < at("appbar:file"));
        assert!(h.widget("Start page").is_some(), "the house button has an accessible name");
        h.step(&json!({"click":{"handle":"appbar:help"}})).unwrap();
        assert!(h.app.help.menu, "? opens the Help menu");
        h.step(&json!({"key":"Escape"})).unwrap();
        h.step(&json!({"click":{"handle":"appbar:home"}})).unwrap();
        assert!(h.app.home.open, "the house button shows the start page");
        let dark = h.app.ui.dark;
        h.call("ui.prefs", json!({"theme": if dark { "light" } else { "dark" }}));
        h.frames(2);
        assert_ne!(h.app.ui.dark, dark, "the theme is a preference");
    }

    #[test]
    fn header_report_button_widens_for_a_longer_translation() {
        let width_in = |language: &str| {
            let mut h = crate::scenario::Harness::new();
            h.app.home.open = false;
            h.app.custom_titlebar = true;
            h.app.preferences.language = language.into();
            h.call("ui.resize", json!({"width":1400.0,"height":720}));
            h.frames(3);
            let center = crate::scenario::handle_at("titlebar:report").expect("the report button is drawn");
            // The button's right edge is fixed; its width is twice the distance to its centre.
            2.0 * (1400.0 - crate::titlebar::WIDTH - RIGHT_GROUP - center.x)
        };
        let (en, ru) = (width_in("en"), width_in("ru"));
        assert!((en - 115.2).abs() < 0.5, "{en}");
        // "Сообщить об ошибке" needs more than the English button's 115 px.
        assert!(ru > en + 10.0, "{ru}");
    }

    #[test]
    fn header_report_actual_click_opens_capture_without_changing_healthy_design() {
        for width in [450.0, 700.0, 900.0, 1400.0] {
            let mut h = crate::scenario::Harness::new();
            h.app.home.open = false;
            h.app.custom_titlebar = true;
            h.app.run("solid.box", json!({"length":35,"width":12,"height":4})).unwrap();
            h.call("ui.resize", json!({"width":width,"height":720}));
            h.frames(3);
            let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.revision);
            let point = h.widget("Report a bug").expect("compact and full buttons retain an accessible name");
            h.step(&json!({"click":point})).unwrap();
            assert!(h.app.issue_report.draft.is_some(), "width {width}: header click must reach the report");
            assert!(crate::issue_report::pending(&h.app));
            assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.revision));
            captured_frame(&mut h);
            // Existing review content scrolls vertically when the window is narrow.
            if width < 700.0 {
                let [x, y] = h.widget("Close draft").unwrap();
                h.call("ui.scroll", json!({"x":x,"y":y - 60.0,"delta":-1200}));
                // Let the scroll animation finish before locating the click target.
                h.frames(60);
            }
            h.step(&json!({"widget":"Close draft"})).unwrap();
            assert!(h.app.issue_report.draft.is_none(), "width {width}: review closes through its actual button");
            h.step(&json!({"click":{"handle":"tab:home"}})).unwrap();
            assert!(h.app.home.open, "width {width}: the Home tab remains reachable beside Report");
            assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.revision));
        }
    }

    #[test]
    fn header_report_preserves_new_dimension_draft_and_single_commit() {
        crate::dim_view::cancel();
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.line", json!({"points":[[0,0],[20,0]]})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 80.0;
        h.app.cam.target = solidcraft_engine::geom::Vec3::new(10.0, 0.0, 0.0);
        h.settle();
        h.step(&json!({"start":"sketch.dimension"})).unwrap();
        for point in [[0, 0, 0], [20, 0, 0], [10, 5, 0]] {
            h.step(&json!({"click":{"world":point}})).unwrap();
        }
        h.step(&json!({"text":"30 mm"})).unwrap();
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.revision);
        h.step(&json!({"widget":"Report a bug"})).unwrap();
        h.frames(3);
        assert!(h.app.issue_report.draft.is_some() && crate::dim_view::editing());
        captured_frame(&mut h);
        h.ctx.memory_mut(|m| m.request_focus(egui::Id::new("issue_report_body")));
        h.step(&json!({"text":"Describe this dimension problem."})).unwrap();
        h.step(&json!({"key":"Enter"})).unwrap();
        assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.revision));
        h.step(&json!({"widget":"Close draft"})).unwrap();
        h.frames(2);
        assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("sc_dim_edit")));
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(!crate::dim_view::editing());
        assert_eq!(h.app.session.undo.len(), before.1 + 1);
        assert_eq!(h.app.session.doc.param("d1").unwrap().expr, "30 mm");
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), before.0);
    }
}

#[cfg(test)]
mod home_tests {
    use super::*;

    /// Home hides the design: its Undo and Redo buttons, the File menu's Insert, Save
    /// and Export, and the design's keyboard shortcuts leave it alone. They work again once
    /// the design is back.
    #[test]
    fn design_actions_are_off_on_home() {
        // A design with history both ways, a selection and unsaved changes, behind Home.
        let on_home = || {
            let mut h = crate::scenario::Harness::new();
            h.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Main"})).unwrap();
            h.app.run("solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [20, 0, 0]})).unwrap();
            h.app.run("edit.undo", json!({})).unwrap();
            h.app.session.selection = vec![solidcraft_engine::Sel::Body { name: "Main".into() }];
            let rev = h.app.session.revision;
            h.app.home.show(rev);
            h.frames(2);
            h
        };
        let mut h = on_home();
        assert!(!design_shown(&h.app));
        // Everything an action could change: the design, its history and file, files written,
        // the selection and visibility, and whether it left Home or opened a tool, dialog or the command box.
        let state = |h: &crate::scenario::Harness| {
            let files = std::fs::read_dir(&h.dir).map_or(0, |d| d.count());
            let doc = serde_json::to_value(&*h.app.session.doc).unwrap();
            let busy = (h.app.tool.is_some(), h.app.dialog.is_some(), h.app.ui.palette_open);
            (
                doc,
                h.app.session.undo.len(),
                h.app.session.redo.len(),
                h.app.session.path.clone(),
                files,
                h.app.session.selection.clone(),
                h.app.session.visibility.hidden_bodies.clone(),
                h.app.home.open,
                busy,
            )
        };
        let click = |h: &mut crate::scenario::Harness, handle: &str| {
            let at = crate::scenario::handle_at(handle).unwrap();
            h.call("ui.click", json!({"x": at.x, "y": at.y}));
        };
        let mut actions: Vec<(String, Box<dyn Fn(&mut crate::scenario::Harness)>)> = Vec::new();
        for icon in ["undo", "redo"] {
            actions.push((format!("{icon} button"), Box::new(move |h| click(h, &format!("appbar:{icon}")))));
        }
        for (key, cmd) in [("S", true), ("Z", true), ("Y", true), ("Delete", false), ("V", false), ("E", false), ("S", false)] {
            let name = if cmd { format!("Ctrl+{key}") } else { key.to_string() };
            actions.push((name, Box::new(move |h| drop(h.call("ui.key", json!({"key": key, "cmd": cmd}))))));
        }
        for item in ["Insert STEP…", "Insert Mesh…", "Save", "Save As…", "Export STL…", "Export STEP…", "Export 3MF…", "Export OBJ…", "Export URDF…"]
        {
            actions.push((
                format!("File › {item}"),
                Box::new(move |h| {
                    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("sc_file_menu"), false));
                    click(h, "appbar:folder");
                    let _ = h.step(&json!({"widget": item}));
                }),
            ));
        }
        // Each action on its own design, so one cannot hide another.
        let mut acted = Vec::new();
        for (name, act) in &actions {
            let mut h = on_home();
            let before = state(&h);
            act(&mut h);
            h.settle();
            if state(&h) != before {
                acted.push(name.clone());
            }
        }
        assert!(acted.is_empty(), "these acted on the hidden design from Home: {acted:?}");

        h.app.home.open = false;
        h.frames(2);
        let at = crate::scenario::handle_at("appbar:redo").unwrap();
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        assert_eq!(h.app.session.redo.len(), 0, "Redo works once Home is closed");
        h.call("ui.key", json!({"key": "Z", "cmd": true}));
        h.settle();
        assert_eq!(h.app.session.redo.len(), 1, "Ctrl+Z works once Home is closed");
        h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("sc_file_menu"), false));
        click(&mut h, "appbar:file");
        h.step(&json!({"widget": "Save"})).unwrap();
        h.settle();
        assert!(h.app.session.path.is_some() && !h.app.session.is_dirty(), "the design saves once Home is closed");
    }

    /// File › Export URDF… writes the robot description and its meshes next to the picked file.
    #[test]
    fn file_menu_exports_a_urdf() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10})).unwrap();
        h.frames(2);
        h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("sc_file_menu"), false));
        let at = crate::scenario::handle_at("appbar:folder").unwrap();
        h.call("ui.click", json!({"x": at.x, "y": at.y}));
        let _ = h.step(&json!({"widget": "Export URDF…"}));
        h.settle();
        let files: Vec<String> = std::fs::read_dir(&h.dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(files.iter().any(|f| f.ends_with(".urdf")) && files.iter().any(|f| f.ends_with("-base.stl")), "{files:?}");
    }

    /// The disabled buttons' tooltip is translated in every catalog (not English fallback).
    #[test]
    fn needs_design_tooltip_is_translated() {
        let english = crate::i18n::Locale::for_language("en").text("ui.needs_design");
        assert!(english.contains("{command}"));
        for catalog in crate::i18n::LOCALES.iter().filter(|c| c.id != "en") {
            let text = crate::i18n::Locale::for_language(catalog.id).text("ui.needs_design");
            assert!(text != english && text.contains("{command}"), "{} must translate ui.needs_design", catalog.id);
        }
    }
}

#[cfg(test)]
mod dropdown_tests {
    use super::*;

    /// A drop-down taller than the window stops at its bottom and scrolls: the last row is
    /// reached with the arrow keys and comes into view.
    #[test]
    fn a_tall_drop_down_scrolls_inside_a_short_window() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.call("ui.resize", json!({"width":1200,"height":320}));
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.frames(4);
        h.step(&json!({"click":{"handle":"panel:SKETCH:CREATE"}})).unwrap();
        h.frames(4);
        let menu = crate::scenario::rect_of("toolbar_dropdown").expect("the drop-down is open");
        assert!(menu.bottom() <= 320.0, "the drop-down runs past the window: {menu:?}");
        assert!(crate::scenario::handle_at("dropdown:sketch.text").is_some_and(|p| !menu.contains(p)), "Text starts below the fold");
        // Up from no focus goes to the last enabled row; it scrolls into view.
        h.step(&json!({"key":"ArrowUp"})).unwrap();
        h.frames(10);
        let hot = crate::scenario::handles("dropdown_hot:");
        let [(_, at)] = hot.as_slice() else { panic!("one highlighted row: {hot:?}") };
        let menu = crate::scenario::rect_of("toolbar_dropdown").unwrap();
        assert!(menu.contains(*at), "the focused last row is out of view: {at:?} in {menu:?}");
    }

    /// Every row of a drop-down is shown when the window has room, even after a shorter list was
    /// shown first: another tab's panel of the same name, or the same panel collapsed (#874).
    #[test]
    fn a_drop_down_shows_all_its_rows_after_a_shorter_one() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.call("ui.resize", json!({"width":1600,"height":1000}));
        let open = |h: &mut crate::scenario::Harness, tab: &str, panel: &str| {
            h.app.ui.tab = tab.into();
            h.frames(4);
            h.step(&json!({"click":{"handle":format!("panel:{tab}:{panel}")}})).unwrap();
            h.frames(6);
            let menu = crate::scenario::rect_of("toolbar_dropdown").expect("the drop-down is open");
            let rows = crate::scenario::handles("dropdown:");
            h.step(&json!({"key":"Escape"})).unwrap();
            h.frames(4);
            (menu, rows)
        };
        let (short, short_rows) = open(&mut h, "MANAGE", "RULES");
        let (menu, rows) = open(&mut h, "MODEL", "MODIFY");
        assert!(rows.len() > short_rows.len(), "MODEL MODIFY lists more than MANAGE RULES: {rows:?} vs {short_rows:?}");
        assert!(menu.height() > short.height(), "the longer list is taller: {menu:?} vs {short:?}");
        for (id, at) in &rows {
            assert!(menu.contains(*at), "{id} at {at:?} is cut off by the drop-down {menu:?}");
        }
    }
}

#[cfg(test)]
mod panel_drop_tests {
    use super::*;

    #[test]
    fn panel_label_gap_and_margins_append_while_buttons_insert_before() {
        for target in ["label", "gap", "left margin", "right margin", "button"] {
            let mut h = crate::scenario::Harness::new();
            h.step(&json!({"click":{"handle":"panel:MODEL:CREATE"}})).unwrap();
            let label = crate::scenario::handle_at("panel:MODEL:CREATE").unwrap();
            let panel = crate::scenario::rect_of("panel:MODEL:CREATE").unwrap();
            let destination = match target {
                "gap" => label - vec2(0.0, 10.0),
                "left margin" => pos2(panel.left() + 1.0, label.y - 30.0),
                "right margin" => pos2(panel.right() - 1.0, label.y - 30.0),
                "button" => crate::scenario::handle_at("button:solid.extrude").unwrap(),
                _ => label,
            };
            h.step(&json!({"drag":[{"handle":"dropdown:solid.cylinder"}, [destination.x,destination.y]],"steps":12})).unwrap();
            h.step(&json!({"key":"Escape"})).unwrap();
            let order = if target == "button" {
                json!(["button:solid.cylinder", "button:solid.extrude", "button:solid.revolve", "button:solid.hole"])
            } else {
                json!(["button:solid.extrude", "button:solid.revolve", "button:solid.hole", "button:solid.cylinder"])
            };
            h.step(&json!({"expect":{"handle_order":order,"tool":null,"dialog":null,"errors":0}})).unwrap();
            assert!(h.app.session.doc.features.is_empty(), "{target}: pinning must not run the command");
            h.step(&json!({"restart":"close"})).unwrap();
            h.step(&json!({"expect":{"handle_order":order}})).unwrap();
        }
    }

    #[test]
    fn panel_label_drop_moves_a_button_between_panels() {
        let mut h = crate::scenario::Harness::new();
        h.step(&json!({"drag":[{"handle":"button:solid.extrude"},{"handle":"panel:MODEL:MODIFY"}],"steps":12})).unwrap();
        h.step(&json!({"expect":{"handle_order":["button:solid.fillet","button:solid.chamfer","button:solid.shell","button:solid.extrude"],"tool":null,"dialog":null,"errors":0}})).unwrap();
        h.step(&json!({"restart":"close"})).unwrap();
        h.step(&json!({"expect":{"handle_order":["button:solid.shell","button:solid.extrude"]}})).unwrap();
    }
}

#[cfg(test)]
mod dropdown_toggle_tests {
    use crate::scenario::{Harness, rect_of};
    use serde_json::json;

    /// A second click on the button that opened a drop-down closes it (#1522): the workspace
    /// switcher's menu, and a panel's command list.
    #[test]
    fn a_second_click_on_its_button_closes_a_drop_down() {
        let mut h = Harness::new();
        h.frames(3);
        let ws = rect_of("workspace_switcher").unwrap().center();
        h.step(&json!({"click": [ws.x, ws.y]})).unwrap();
        assert!(h.app.viewport.context_menu.is_some(), "the first click opens the workspace menu");
        h.step(&json!({"click": [ws.x, ws.y]})).unwrap();
        h.frames(2);
        assert!(h.app.viewport.context_menu.is_none(), "the second click closes it instead of reopening it");
        h.step(&json!({"click": [ws.x, ws.y]})).unwrap();
        assert!(h.app.viewport.context_menu.is_some(), "and a third opens it again");
        h.step(&json!({"key": "Escape"})).unwrap();

        h.step(&json!({"click": {"handle": "panel:MODEL:CREATE"}})).unwrap();
        h.frames(2);
        assert!(rect_of("toolbar_dropdown").is_some(), "the first click opens the panel's list");
        h.step(&json!({"click": {"handle": "panel:MODEL:CREATE"}})).unwrap();
        h.frames(2);
        assert!(rect_of("toolbar_dropdown").is_none(), "the second click closes it");
    }
}

#[cfg(test)]
mod app_bar_tests {
    use super::*;

    #[test]
    fn bar_hover_inks_and_dividers_remain_readable_in_both_themes() {
        fn luminance(c: Color32) -> f32 {
            let c = egui::Rgba::from(c);
            0.2126 * c.r() + 0.7152 * c.g() + 0.0722 * c.b()
        }
        fn contrast(a: Color32, b: Color32) -> f32 {
            let (a, b) = (luminance(a), luminance(b));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        }
        for dark in [false, true] {
            for vision in crate::theme::ColorVision::ALL {
                let t = Tokens::themed(dark, vision);
                for fill in [t.app_bar_hover, t.app_bar_hover_subtle, t.app_bar_hover_button, t.app_bar_hover_strong] {
                    let channel = |fg: u8, bg: u8| fg.saturating_add((u16::from(bg) * u16::from(255 - fill.a()) / 255) as u8);
                    let bg = Color32::from_rgb(channel(fill.r(), t.app_bar.r()), channel(fill.g(), t.app_bar.g()), channel(fill.b(), t.app_bar.b()));
                    assert!(contrast(t.app_bar_text, bg) >= 4.5, "dark={dark}, {vision:?}, {fill:?}");
                }
                assert!(contrast(t.app_bar_separator, t.app_bar) >= 3.0);
                assert!(contrast(t.app_bar, t.warning) >= 4.5, "approval counts need dark ink on amber");
            }
        }
    }

    #[test]
    fn save_button_uses_the_existing_save_flow_for_new_and_named_designs() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("solid.box", json!({"length":2,"width":3,"height":4})).unwrap();
        h.frames(3);
        h.step(&json!({"click":{"handle":"titlebar:save"}})).unwrap();
        let path = h.app.session.path.clone().expect("Save chooses a path for an untitled design");
        h.app.run("solid.box", json!({"length":1,"width":1,"height":1,"corner":[10,0,0]})).unwrap();
        h.step(&json!({"click":{"handle":"titlebar:save"}})).unwrap();
        assert_eq!(h.app.session.path.as_deref(), Some(path.as_str()));
        let saved = solidcraft_engine::io::read_design(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved, *h.app.session.doc);
    }
}

#[cfg(test)]
mod ribbon_label_tests {
    use super::*;

    #[test]
    fn every_promoted_label_wraps_only_at_word_boundaries() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter().clone();
            let specs = solidcraft_engine::command_specs();
            for section in crate::workspace::SECTIONS {
                let (commands, count) = crate::workspace::layout(section.tab, section.panel, &specs).unwrap();
                for command in commands.iter().take(count) {
                    for catalog in crate::i18n::LOCALES {
                        let locale = crate::i18n::Locale::for_language(catalog.id);
                        let label = locale.label(crate::workspace::ribbon_button_label(command));
                        let width = ribbon_button_width(&painter, label) - BUTTON_LABEL_PADDING;
                        let galley = ribbon_button_galley(&painter, label, width, Color32::WHITE);
                        assert!(galley.rows.len() <= 2, "{}: {label}", command.id);
                        assert!(galley.size().x <= width + 0.01, "{}: {label}: {} > {width}", command.id, galley.size().x);
                        let first = galley.rows.first().unwrap().text();
                        let visible = first.strip_suffix('…').unwrap_or(&first);
                        assert!(label.starts_with(visible), "{}: {label} -> {first}", command.id);
                        if let Some(second) = galley.rows.get(1) {
                            let second = second.text();
                            let next = second.strip_suffix('…').unwrap_or(&second);
                            // The second line must start at a legal source boundary,
                            // including when the first line was ellipsized.
                            assert!(
                                label.char_indices().any(|(at, ch)| {
                                    let end = at + ch.len_utf8();
                                    (ch.is_whitespace() || ch == '/')
                                        && label[end..].trim_start().starts_with(next)
                                        && (first.ends_with('…') || label[..end].trim() == first)
                                }),
                                "{}: {label} -> {first:?}, {second:?} breaks inside a word",
                                command.id
                            );
                        }
                    }
                }
            }
            for label in ["Rectangular", "Dimension", "Coincident"] {
                let width = ribbon_button_width(&painter, label);
                assert_eq!(ribbon_button_lines(&painter, label, width - BUTTON_LABEL_PADDING), vec![label]);
                assert!((BUTTON_MIN_WIDTH..=BUTTON_MAX_WIDTH).contains(&width));
            }
            let finish = solidcraft_engine::find_command("sketch.finish").unwrap();
            let label = crate::workspace::ribbon_button_label(finish);
            let width = ribbon_button_width(&painter, label);
            assert_eq!(ribbon_button_lines(&painter, label, width - BUTTON_LABEL_PADDING), vec!["Finish", "Sketch"]);
            // Exercise full labels and pinned commands too, not just compact defaults.
            for (label, width, expected) in [
                ("Coincident", 50.0, vec!["Coincident"]),
                ("Rectangular Pattern", 50.0, vec!["Rectangular", "Pattern"]),
                ("Move/Copy", 40.0, vec!["Move/", "Copy"]),
            ] {
                let lines = ribbon_button_lines(&painter, label, width);
                // Exact glyph widths depend on the bundled font: verify the source
                // word stays on its own line, with only an optional suffix omitted.
                assert_eq!(lines.len(), expected.len(), "{label}: {lines:?}");
                for (actual, word) in lines.iter().zip(expected) {
                    assert!(actual == word || actual.strip_suffix('…').is_some_and(|prefix| word.starts_with(prefix)), "{label}: {lines:?}");
                }
            }
            let lines = ribbon_button_lines(&painter, "one two three four five", 50.0);
            assert_eq!(lines.len(), 2);
            assert!(lines[1].ends_with('…'));
            let lines = ribbon_button_lines(&painter, "長い日本語のラベル 続き", 50.0);
            assert_eq!(lines.len(), 2);
            assert!(lines[0].ends_with('…'));
        });
        output.textures_delta.clear();
    }
}

#[cfg(test)]
mod ribbon_gallery_tests {
    use super::*;
    use crate::scenario::{Harness, rect_of};

    #[test]
    fn constraint_gallery_preserves_button_handles_order_and_pin_actions() {
        let mut h = Harness::new();
        h.call("ui.resize", json!({"width":1800,"height":900}));
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.frames(3);
        let dimension = rect_of("button:sketch.dimension").unwrap();
        let mut cells = Vec::new();
        for id in crate::workspace::CONSTRAINT_GALLERY {
            let cell = rect_of(&format!("ribbon:constraint:{id}")).unwrap();
            assert_eq!(cell.size(), vec2(24.0, 20.0));
            assert_eq!(rect_of(&format!("button:{id}")), Some(cell));
            assert!(cell.left() > dimension.right());
            cells.push(cell);
        }
        for row in cells.chunks(3) {
            assert!(row.windows(2).all(|pair| pair[0].right() < pair[1].left() && pair[0].top() == pair[1].top()));
        }
        for col in 0..3 {
            assert_eq!(cells[col].left(), cells[col + 3].left());
            assert!(cells[col].bottom() < cells[col + 3].top());
        }
        h.step(&json!({"click":{"handle":"ribbon:constraint:sketch.constraint.equal"}})).unwrap();
        assert_eq!(h.app.tool.as_ref().map(|t| t.cmd), Some("sketch.constraint.equal"));
        h.step(&json!({"key":"Escape"})).unwrap();
        crate::toolbar_custom::remove(&mut h.app, "SKETCH", "sketch.constraint.equal");
        h.frames(2);
        assert!(rect_of("ribbon:constraint:sketch.constraint.equal").is_none());
        crate::toolbar_custom::pin(&mut h.app, "SKETCH", "sketch.constraint.equal").unwrap();
        h.frames(2);
        assert!(rect_of("ribbon:constraint:sketch.constraint.equal").is_some());
    }
}

#[cfg(test)]
mod fill_width_tests {
    use super::*;

    /// #2108: at every common window width, in the Solid and the Sketch tab, the toolbar uses the
    /// width it has. No blank run wider than one button while something is hidden, every panel
    /// on the toolbar shows its icons, the » menu only when even one button a panel won't fit,
    /// and neither the tab row nor the title bar's own controls run into each other.
    #[test]
    fn toolbar_fills_its_width_from_wide_to_narrow_windows() {
        for sketch in [false, true] {
            let mut h = crate::scenario::Harness::new();
            h.app.home.open = false;
            h.app.custom_titlebar = true;
            h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
            if sketch {
                h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
            }
            let tab = if sketch { "SKETCH" } else { "MODEL" };
            h.app.ui.tab = tab.into();
            for w in [1920.0, 1600.0, 1440.0, 1366.0, 1280.0, 1100.0, 900.0, 700.0] {
                h.call("ui.resize", json!({"width":w,"height":900}));
                h.frames(3);
                let at = format!("{tab} at {w}");
                let bar = crate::scenario::rect_of("toolbar").unwrap();
                let finish = crate::scenario::rect_of("panel:SKETCH:FINISH SKETCH");
                let right = finish.map_or(bar.right() - 4.0, |f| f.left() - 4.0);
                let count = |name: &str| crate::scenario::count_of(name).unwrap_or(-1.0);
                let (collapsed, trimmed, more) = (count("toolbar_collapsed"), count("toolbar_trimmed"), count("toolbar_more"));
                let lists = crate::toolbar_custom::panel_lists(tab);
                let mut end = bar.left();
                let mut on_bar = 0;
                for (panel, cmds, promote) in &lists {
                    let Some(rect) = crate::scenario::rect_of(&format!("panel:{tab}:{panel}")) else { continue };
                    on_bar += 1;
                    if *panel != "FINISH SKETCH" {
                        end = end.max(rect.right() + PANEL_GAP);
                    }
                    if crate::toolbar_custom::buttons(&h.app, tab, panel, cmds, *promote).is_empty() {
                        continue;
                    }
                    let icons = crate::scenario::handles("button:").iter().filter(|(_, p)| rect.contains(*p)).count()
                        + usize::from(crate::scenario::handle_at(&format!("collapsed:{tab}:{panel}")).is_some());
                    assert!(icons >= 1, "{at}: {panel} shows no icons");
                }
                assert_eq!(on_bar + more as usize, lists.len(), "{at}: every panel is on the toolbar or in the » menu");
                let overflow = crate::scenario::rect_of("toolbar:overflow");
                assert_eq!(overflow.is_some(), more > 0.0, "{at}: the » menu shows exactly when a panel is in it");
                let gap = right - end - if more > 0.0 { MORE_ROOM } else { 0.0 };
                assert!(gap >= -0.5, "{at}: the panels run {} px past the room", -gap);
                let hidden = collapsed + trimmed + more > 0.0;
                let button = if more > 0.0 {
                    COLLAPSED_WIDTH + PANEL_GAP
                } else if sketch {
                    CONSTRAINT_GRID_WIDTH + BUTTON_GAP
                } else {
                    BUTTON_MIN_WIDTH + BUTTON_GAP
                };
                if hidden {
                    assert!(gap < button, "{at}: {gap} px left blank while buttons are hidden");
                }
                // The » menu only when one button a panel can't fit.
                if w >= 900.0 {
                    assert_eq!(more, 0.0, "{at}: the » menu while there is room");
                }
                if w >= 1600.0 {
                    assert!(!hidden, "{at}: buttons hidden on a wide window");
                }
                if let Some(o) = overflow {
                    assert!(o.right() <= right + 4.0, "{at}: the » button overlaps Finish or the right edge");
                }
                if sketch {
                    let finish = finish.expect("Finish group stays visible");
                    assert_eq!(finish.right(), bar.right(), "{at}: Finish stays anchored right");
                    assert!(crate::scenario::handle_at("button:sketch.finish").is_some(), "{at}: Finish stays reachable");
                }
                // The assistant lives in the title bar; visible ribbon tabs fit before search.
                let search = crate::scenario::rect_of("ribbon:search").unwrap();
                for t in TABS.iter().chain(["SKETCH"].iter()) {
                    if let Some(r) = crate::scenario::rect_of(&format!("toolbar_tab:{t}")) {
                        assert!(r.right() < search.left(), "{at}: the {t} tab runs into search");
                    }
                }
                // The title bar keeps document tabs before Assistant, Preferences, Help and Report.
                let report = crate::scenario::rect_of("titlebar:report").unwrap();
                let tabs_end = crate::scenario::handles("tab:design:").iter().map(|(_, p)| p.x).fold(0.0, f32::max);
                assert!(tabs_end < report.left(), "{at}: the document tabs run into Report a bug");
                let assistant = crate::scenario::rect_of("titlebar:assistant").unwrap();
                let help = crate::scenario::rect_of("appbar:help").unwrap();
                let prefs = crate::scenario::rect_of("titlebar:preferences").unwrap();
                assert!(tabs_end < assistant.left(), "{at}: the document tabs run into Assistant");
                assert!(
                    assistant.right() <= prefs.left() && prefs.right() <= help.left() && help.right() <= report.left(),
                    "{at}: the title bar's right-hand controls overlap"
                );
            }
        }
    }
}
