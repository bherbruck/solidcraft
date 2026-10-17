//! Application bar, the workspace toolbar (tabs and panels) and keyboard shortcuts.

use crate::icons::glyphs::caret;
use crate::icons::glyphs::pin_icon;
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::theme::Tokens;
use crate::{SolveApp, icons};

pub const TABS: &[&str] = &["SOLID", "SURFACE", "MESH", "SHEET METAL", "PLASTIC", "UTILITIES"];

/// Legacy paint arguments and non-SVG affordances follow the same semantic palette as the icon.
/// The requested icon id determines its meaning on ribbons, menus and drag previews alike.
fn command_accent(icon: &str) -> Color32 {
    crate::icon_svg::primary(icon, crate::theme::is_dark(), crate::theme::vision())
}

/// Panels shown on each tab (in order). Commands come from the registry by (tab, panel).
pub fn panels(tab: &str) -> &'static [&'static str] {
    if let Some(p) = crate::workspace::panels(tab) {
        return p;
    }
    match tab {
        "SOLID" => &["CREATE", "MODIFY", "ASSEMBLE", "CONSTRUCT", "INSPECT", "INSERT", "SELECT"],
        "SKETCH" => &["CREATE", "MODIFY", "PATTERN", "CONSTRAINTS", "INSPECT", "INSERT", "SELECT", "FINISH SKETCH"],
        "SURFACE" => &["CREATE", "MODIFY", "ASSEMBLE", "CONSTRUCT", "INSPECT", "INSERT", "SELECT"],
        "MESH" => &["CREATE", "PREPARE", "MODIFY", "ASSEMBLE", "CONSTRUCT", "INSPECT", "INSERT", "SELECT"],
        "SHEET METAL" => &["CREATE", "MODIFY", "ASSEMBLE", "CONSTRUCT", "INSPECT", "INSERT", "SELECT"],
        "PLASTIC" => &["CREATE", "MODIFY", "ASSEMBLE", "CONSTRUCT", "INSPECT", "INSERT", "SELECT"],
        "RENDER" => &["SCENE"],
        _ => &["MAKE", "ADD-INS", "UTILITY", "INSPECT", "SELECT"],
    }
}

/// How many commands of a panel get a button on a fresh (or reset) toolbar. The user's own
/// layout (`toolbar_custom`) has no cap: every pinned command gets a button.
pub fn promoted(tab: &str, panel: &str) -> usize {
    match (tab, panel) {
        ("SOLID", "CREATE") => 4,
        ("SOLID", "MODIFY") => 4,
        ("SKETCH", "CONSTRAINTS") => 6,
        ("SKETCH", "PATTERN") => 3,
        (_, "FINISH SKETCH") => 1,
        _ => 2,
    }
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

/// One panel as [`fit`] sees it: how many buttons it has and whether they carry a label under the
/// icon (Sketch > Create).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelSize {
    pub buttons: usize,
    pub labelled: bool,
}

/// Where [`fit`] put a panel: how many of its buttons it draws, at which stride, and whether
/// their labels show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub fit: Fit,
    /// Buttons drawn (the first ones of the panel).
    pub shown: usize,
    /// Distance from one button to the next.
    pub stride: f32,
}

impl Plan {
    /// The panel's width on the toolbar (0 in the » menu).
    pub fn width(&self) -> f32 {
        match self.fit {
            Fit::More => 0.0,
            Fit::Collapsed => COLLAPSED_WIDTH,
            Fit::Buttons => (self.shown.max(1) as f32 * self.stride).max(PANEL_MIN) + 8.0,
        }
    }

    /// Whether labelled buttons show their label (shortened below [`LABELLED_STRIDE`]).
    pub fn labels(&self) -> bool {
        self.stride >= LABEL_MIN_STRIDE
    }
}

/// A panel's width with `buttons` buttons.
pub fn panel_width(buttons: usize) -> f32 {
    (buttons.max(1) as f32 * STRIDE).max(PANEL_MIN) + 8.0
}

/// An icon button's stride.
pub const STRIDE: f32 = 40.0;
/// A labelled button's stride with its whole label (Sketch > Create).
pub const LABELLED_STRIDE: f32 = 68.0;
/// Below this stride a labelled button drops its label and shows just the icon.
pub const LABEL_MIN_STRIDE: f32 = 50.0;
/// The narrowest a panel gets (beside its 8 px margins), so its label stays readable.
const PANEL_MIN: f32 = 64.0;

/// A collapsed panel's width: one button's panel.
pub const COLLAPSED_WIDTH: f32 = PANEL_MIN + 8.0;

/// The gap after each panel (its separator).
const PANEL_GAP: f32 = 2.0;

/// The » button's room at the end of the panels, kept only while some panel is in it.
pub const MORE_ROOM: f32 = 40.0;

/// Fit panels into `room`, giving up as little as it takes, one small step at a time:
/// 1. labelled buttons narrow, their labels shortening and then going (icons only);
/// 2. panels hand their last buttons to their drop-down one at a time, the panel showing the
///    most buttons first (the rightmost of equals), down to one button (collapsed);
/// 3. the last panels go into the » menu, and any room that frees goes back to buttons.
///
/// Every step frees at most one button's width, so the toolbar never leaves more than a button
/// of blank space while something is hidden. The result depends only on `room` (no state from
/// earlier frames), so it never flickers. Nothing is dropped.
pub fn fit(panels: &[PanelSize], room: f32) -> Vec<Plan> {
    let room = if room.is_finite() { room } else { 0.0 };
    if let Some(plans) = shrink(panels, room) {
        return plans;
    }
    // 3. The last panels into the » menu (which takes its own room), as few as it takes; the
    // ones left keep as many buttons as fit.
    for kept in (0..panels.len()).rev() {
        if let Some(mut plans) = shrink(panels.get(..kept).unwrap_or_default(), room - MORE_ROOM) {
            plans.extend(panels.iter().skip(kept).map(|_| Plan { fit: Fit::More, shown: 0, stride: STRIDE }));
            return plans;
        }
    }
    panels.iter().map(|_| Plan { fit: Fit::More, shown: 0, stride: STRIDE }).collect()
}

/// The room plans take on the toolbar (each panel and its separator).
fn used(plans: &[Plan]) -> f32 {
    plans.iter().filter(|p| p.fit != Fit::More).map(|p| p.width() + PANEL_GAP).sum()
}

/// Steps 1 and 2 of [`fit`]: every panel on the toolbar, or None when even one button each
/// doesn't fit.
fn shrink(panels: &[PanelSize], room: f32) -> Option<Vec<Plan>> {
    let mut plans: Vec<Plan> =
        panels.iter().map(|p| Plan { fit: Fit::Buttons, shown: p.buttons, stride: if p.labelled { LABELLED_STRIDE } else { STRIDE } }).collect();
    if used(&plans) <= room {
        return Some(plans);
    }
    // 1. Labelled buttons narrow together, to the widest whole-pixel stride that fits (icons
    // only at worst).
    let narrow = |plans: &mut [Plan], stride: f32| {
        for (p, s) in plans.iter_mut().zip(panels) {
            if s.labelled {
                p.stride = stride;
            }
        }
    };
    let mut stride = LABELLED_STRIDE;
    while stride > STRIDE {
        stride -= 1.0;
        narrow(&mut plans, stride);
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
            .filter(|(_, p)| p.fit == Fit::Buttons && p.shown > 1)
            .max_by(|(i, a), (j, b)| a.shown.cmp(&b.shown).then(i.cmp(j)));
        let (i, _) = pick?;
        if let Some(p) = plans.get_mut(i) {
            p.shown -= 1;
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

/// What the right-hand group keeps clear between Report a bug and the window's caption buttons:
/// Help, Preferences and the version.
const RIGHT_GROUP: f32 = 120.0;

/// The centre of the Help (?) button: left of Preferences.
fn help_x(bar: Rect, captions: f32) -> f32 {
    bar.right() - captions - 101.0
}

/// The Report a bug button: labelled when there is room for its text (`label_width`, measured in
/// the current language), otherwise just the icon.
fn report_rect(bar: Rect, captions: f32, tabs_start: f32, label_width: f32) -> (Rect, bool) {
    let right = bar.right() - captions - RIGHT_GROUP;
    // Icon, text and a margin; never narrower than the English button.
    let full = (REPORT_TEXT_X + label_width + 10.0).max(124.0);
    let labeled = right - tabs_start >= 216.0 + full;
    let width = if labeled { full } else { 28.0 };
    (Rect::from_min_size(pos2(right - width, bar.top() + 4.0), vec2(width, 26.0)), labeled)
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
    let font = FontId::proportional(12.5);
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
    let fill = if hot { t.accent.lerp_to_gamma(t.app_bar_text, 0.18) } else { t.accent };
    let radius = PILL_HEIGHT / 2.0;
    ui.painter().rect_filled(rect, radius, fill);
    if response.has_focus() {
        ui.painter().rect_stroke(rect.expand(2.0), radius + 2.0, Stroke::new(1.0, t.app_bar_text), egui::StrokeKind::Outside);
    }
    let glyph_x = if label.is_some() { rect.left() + icon_x } else { rect.center().x };
    let glyph = Rect::from_center_size(pos2(glyph_x, rect.center().y), vec2(16.0, 16.0));
    icons::paint(ui.painter(), glyph, icon, t.app_bar_text, fill, t.app_bar_text);
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
            ui.painter().text(pos2(left + 36.0, r.center().y), Align2::LEFT_CENTER, "SolidCraft", FontId::proportional(14.0), t.app_bar_text);
        }
        let mut x = left + if compact_actions { 60.0 } else { 128.0 };
        // While a file loads the design waits: no menus or commands from the bar.
        let idle = !app.file_load.busy();
        // The File menu: a text button (New, Open, Save, Save As, Export…). Its handle keeps the
        // old `appbar:folder` name too, for scripts written against the folder button.
        let file = {
            let label = app.tr("ui.file").to_string();
            let galley = ui.painter().layout_no_wrap(label.clone(), FontId::proportional(13.0), t.app_bar_text);
            let br = Rect::from_min_size(pos2(x - 13.0, r.center().y - 13.0), vec2(galley.size().x + 16.0, 26.0));
            x = br.right() + 17.0;
            let resp = ui.interact(br, ui.id().with(("ab", "file")), if idle { Sense::click() } else { Sense::hover() });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, idle, &label));
            crate::scenario::publish_handle("appbar:file", br.center());
            crate::scenario::publish_handle("appbar:folder", br.center());
            crate::scenario::publish_rect("appbar:file", br);
            let open = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("sc_file_menu")).unwrap_or(false));
            if idle && (resp.hovered() || open) {
                ui.painter().rect_filled(br, 4.0, t.app_bar_hover);
            }
            let ink = if idle { t.app_bar_text } else { t.app_bar_text.gamma_multiply(0.35) };
            ui.painter().galley(pos2(br.left() + 8.0, br.center().y - galley.size().y / 2.0), galley, ink);
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("sc_file_menu_x"), br.left()));
            idle && resp.clicked()
        };
        // A disabled button is dimmed and ignores clicks (its tooltip still shows).
        let click = |ui: &mut egui::Ui, at: f32, icon: &str, tip: &str, enabled: bool| -> bool {
            let width = if compact_actions { 22.0 } else { 26.0 };
            let br = Rect::from_center_size(pos2(at, r.center().y), vec2(width, 26.0));
            let resp = ui.interact(br, ui.id().with(("ab", icon)), if enabled { Sense::click() } else { Sense::hover() });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tip));
            crate::scenario::publish_handle(&format!("appbar:{icon}"), br.center());
            crate::scenario::publish_rect(&format!("appbar:{icon}"), br);
            if enabled && resp.hovered() {
                ui.painter().rect_filled(br, 4.0, Color32::from_white_alpha(30));
            }
            icons::paint_on(ui.painter(), br.shrink(5.0), icon, Some(true), if enabled { 1.0 } else { 0.35 });
            let clicked = resp.on_hover_text(tip).clicked();
            enabled && clicked
        };
        let step = if compact_actions { 22.0 } else { 30.0 };
        // Undo and Redo act on the design, which Home hides. Save lives in the File menu (Ctrl+S).
        let shown = design_shown(app) && idle;
        let tip = |id: &str, key: &str| {
            let label = app.tr(id);
            if shown { format!("{label} ({key})") } else { crate::i18n::format("ui.needs_design", &[("command", label)]) }
        };
        let (undo_tip, redo_tip) = (tip("ui.undo", "Ctrl+Z"), tip("ui.redo", "Ctrl+Y"));
        // A very narrow window keeps its room for the tabs (Home first); Ctrl+Z and Ctrl+Y still undo and redo.
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
        // Document tabs (Home first), on the left after File, Undo and Redo.
        let captions = if app.custom_titlebar { crate::titlebar::WIDTH } else { 0.0 };
        let tabs_start = x - step + 25.0;
        let report_text = crate::i18n::tr("ui.report_a_bug");
        let report_font = FontId::proportional(13.0);
        let label_width = ui.painter().layout_no_wrap(report_text.to_string(), report_font.clone(), t.app_bar_text).size().x;
        let (report, report_label) = report_rect(r, captions, tabs_start, label_width);
        // Help, on the right between Report a bug and Preferences.
        if click(ui, help_x(r, captions), "help", app.tr("ui.help"), true) {
            app.help.menu = !app.help.menu;
            let at = help_x(r, captions);
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("sc_help_opened"), true);
                d.insert_temp(egui::Id::new("sc_help_anchor"), at);
            });
        }
        // A newer release: its Update button, left of Report a bug (`update`), in the space up to
        // the bar's own buttons on the left (File, Undo and Redo, which end before `x`).
        let span = Rect::from_min_max(pos2(x, report.top()), pos2(report.left() - 6.0, report.bottom()));
        let update = crate::update::button(app, ui, span, report_label);
        let tabs_end = update.map_or(report.left(), |u| u.left()) - 8.0;
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
        ui.painter().rect_filled(report, 4.0, if response.hovered() || response.has_focus() { t.hover } else { t.app_bar });
        ui.painter().rect_stroke(report, 4.0, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let icon = Rect::from_center_size(pos2(report.left() + 14.0, report.center().y), vec2(18.0, 18.0));
        icons::paint_on(ui.painter(), icon, "bug_report", Some(true), 1.0);
        if report_label {
            ui.painter().text(pos2(report.left() + REPORT_TEXT_X, report.center().y), Align2::LEFT_CENTER, report_text, report_font, t.app_bar_text);
        }
        if response.on_hover_text(crate::i18n::tr_label("Report a bug — review a screenshot and open a GitHub draft")).clicked() {
            crate::help::run(app, &ui.ctx().clone(), "report");
        }
        // Preferences, at the top right like Fusion's profile menu (also Ctrl+,).
        let gear = Rect::from_center_size(pos2(r.right() - captions - 70.0, r.center().y), vec2(24.0, 24.0));
        let gresp = ui.interact(gear, ui.id().with("sc_prefs_button"), Sense::click());
        if gresp.hovered() {
            ui.painter().rect_filled(gear, 4.0, Color32::from_white_alpha(30));
        }
        icons::paint_on(ui.painter(), gear.shrink(4.0), "settings", Some(true), 1.0);
        crate::scenario::publish_handle("titlebar:preferences", gear.center());
        let prefs_tip = if cfg!(target_os = "macos") {
            format!("{} (Cmd+,)", app.tr("ui.preferences"))
        } else {
            format!("{} (Ctrl+,)", app.tr("ui.preferences"))
        };
        if gresp.on_hover_text(prefs_tip).clicked() {
            crate::prefs::open_at(app, 0);
        }
        ui.painter().text(
            pos2(r.right() - 12.0 - captions, r.center().y),
            Align2::RIGHT_CENTER,
            format!("v{}", crate::update::current_version()),
            FontId::proportional(11.0),
            Color32::from_rgb(160, 168, 182),
        );
        if app.custom_titlebar {
            crate::titlebar::caption_buttons(app, ui, r);
        }
    });
    file_menu(app, ui.ctx());
    let help_x = ui.ctx().data(|d| d.get_temp::<f32>(egui::Id::new("sc_help_anchor")).unwrap_or(300.0));
    crate::help::show(app, &ui.ctx().clone(), pos2(help_x - 14.0, 34.0));
}

fn file_menu(app: &mut SolveApp, ctx: &egui::Context) {
    let id = egui::Id::new("sc_file_menu");
    if !ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        return;
    }
    let mut close = false;
    let resp = crate::frame::area(egui::Id::new("sc_file_area"))
        .fixed_pos(pos2(ctx.data(|d| d.get_temp::<f32>(egui::Id::new("sc_file_menu_x"))).unwrap_or(100.0), 34.0))
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

pub fn toolbar(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let sketching = app.session.active_sketch.is_some();
    egui::Panel::top("sc_toolbar").exact_size(92.0).frame(egui::Frame::NONE.fill(t.toolbar)).show(ui, |ui| {
        let r = ui.max_rect();
        let painter = ui.painter().clone();
        // Right-click on the toolbar's empty space: its own menu. Interacted first, so every
        // button, tab and panel label is above it.
        let bg = ui.interact(r, ui.id().with("toolbar_bg"), Sense::click());
        if bg.secondary_clicked() {
            crate::context_menu::open_for(app, bg.interact_pointer_pos().unwrap_or(r.left_bottom()), crate::context_menu::Target::Toolbar);
        }
        // Workspace selector and tabs.
        let tabs_y = r.top() + 2.0;
        // The workspace switcher spans the toolbar's height (the tab row and the panel row).
        let ws = Rect::from_min_max(pos2(r.left() + 6.0, r.top() + 4.0), pos2(r.left() + 104.0, r.bottom() - 5.0));
        let wresp = ui.interact(ws, ui.id().with("workspace_switcher"), Sense::click());
        painter.rect(ws, 4.0, if wresp.hovered() { t.hover } else { t.field }, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let workspace_icon = if crate::render_workspace::active(app) { "ws_render" } else { "ws_design" };
        icons::paint(
            &painter,
            Rect::from_center_size(pos2(ws.center().x, ws.top() + 24.0), vec2(26.0, 26.0)),
            workspace_icon,
            t.icon,
            t.icon_fill,
            t.accent,
        );
        painter.text(
            pos2(ws.center().x - 5.0, ws.top() + 52.0),
            Align2::CENTER_CENTER,
            app.tr_label(if crate::render_workspace::active(app) { "RENDER" } else { crate::toolbar_custom::WORKSPACE }),
            FontId::proportional(12.0),
            t.text,
        );
        caret(&painter, pos2(ws.center().x + 27.0, ws.top() + 52.0), t.text);
        painter.text(pos2(ws.center().x, ws.top() + 69.0), Align2::CENTER_CENTER, app.tr("ui.workspace"), FontId::proportional(10.0), t.text_dim);
        if wresp.on_hover_text(app.tr("ui.change_workspace")).clicked() {
            crate::context_menu::toggle_for(app, ws, ws.left_bottom(), crate::context_menu::Target::Workspace);
        }
        crate::scenario::publish_rect("toolbar", r);
        crate::scenario::publish_rect("workspace_switcher", ws);
        let mut x = r.left() + 112.0;
        let mut tabs: Vec<&str> = Vec::new();
        if sketching {
            tabs.push("SKETCH");
        }
        if crate::render_workspace::active(app) {
            tabs.push("RENDER");
        } else {
            tabs.extend(TABS.iter());
        }
        for tab in tabs {
            let display_tab = app.tr_label(tab);
            let text_width = painter.layout_no_wrap(display_tab.to_string(), FontId::proportional(12.0), t.text).size().x;
            let w = 12.0 + text_width.max(28.0);
            let tr = Rect::from_min_size(pos2(x, tabs_y), vec2(w, 22.0));
            let active = app.ui.tab == tab;
            let resp = ui.interact(tr, ui.id().with(("tab", tab)), Sense::click());
            crate::scenario::publish_rect(&format!("toolbar_tab:{tab}"), tr);
            if active {
                let c = if tab == "SKETCH" { t.sketch_accent } else { t.accent };
                painter.line_segment([pos2(tr.left() + 4.0, tr.bottom() - 1.0), pos2(tr.right() - 4.0, tr.bottom() - 1.0)], Stroke::new(2.0, c));
            } else if resp.hovered() {
                painter.rect_filled(tr, 4.0, t.hover);
            }
            let col = if tab == "SKETCH" {
                t.sketch_accent
            } else if active {
                t.text
            } else {
                t.text_dim
            };
            painter.text(tr.center(), Align2::CENTER_CENTER, display_tab, FontId::proportional(12.0), col);
            if resp.clicked() {
                app.ui.tab = tab.to_string();
            }
            x += w + 4.0;
        }
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
        let mut px = r.left() + 112.0;
        let top = r.top() + 26.0;
        let mut drop: Option<(&'static str, &'static str, Option<&'static str>)> = None;
        let mut menu: Option<(egui::Pos2, &'static str)> = None;
        // A drop-down row's pin icon clicked: pin or unpin that command.
        let mut toggle: Option<&'static str> = None;
        // The open drop-down (a button dropped on it leaves the toolbar).
        let mut popup_rect: Option<Rect> = None;
        let mut overflow = Vec::new();
        // Every panel with all its buttons (the user's pins have no cap); a toolbar too narrow
        // for them collapses panels into their drop-down, then into the » menu.
        let lists: Vec<_> = crate::toolbar_custom::panel_lists(&tab)
            .into_iter()
            .map(|(panel, cmds, promote)| {
                let shown = crate::toolbar_custom::buttons(app, &tab, panel, &cmds, promote);
                (panel, cmds, shown)
            })
            .collect();
        let sizes: Vec<PanelSize> =
            lists.iter().map(|(panel, _, shown)| PanelSize { buttons: shown.len(), labelled: tab == "SKETCH" && *panel == "CREATE" }).collect();
        // The AI button keeps the toolbar row's right end, whatever the ribbon's width
        // (docs/agent-chat.md); panels that no longer fit go to the » menu as usual.
        let right = if crate::agent_chat::toolbar_button_shown(app) { r.right() - AI_BUTTON_ROOM } else { r.right() };
        let plans = fit(&sizes, right - 4.0 - px);
        crate::scenario::publish_count("toolbar_collapsed", plans.iter().filter(|p| p.fit == Fit::Collapsed).count() as f64);
        crate::scenario::publish_count("toolbar_more", plans.iter().filter(|p| p.fit == Fit::More).count() as f64);
        crate::scenario::publish_count(
            "toolbar_trimmed",
            plans.iter().zip(&sizes).filter(|(p, s)| p.fit == Fit::Buttons && p.shown < s.buttons).count() as f64,
        );
        for ((panel, cmds, shown), plan) in lists.into_iter().zip(plans) {
            let panel: &&str = &panel;
            if plan.fit == Fit::More {
                overflow.push((*panel, menu_commands(&cmds, &shown)));
                continue;
            }
            let collapsed = plan.fit == Fit::Collapsed;
            let width = plan.width();
            let sketch_create = tab == "SKETCH" && *panel == "CREATE";
            let stride = plan.stride;
            // Sketch > Create's buttons carry their label while there's room for one.
            let labelled = sketch_create && plan.labels();
            // Buttons that went into the drop-down for want of room are still listed there.
            let trimmed = plan.shown < shown.len();
            let enabled_panel = !cmds.is_empty() || *panel == "SELECT";
            // Per tab: panels of the same name on other tabs (MODIFY, CREATE…) list other commands,
            // and the drop-down's area remembers its size by this id (#874).
            let popup_id = ui.id().with(("panel_menu", &tab, panel));
            // Dropped on the panel's free space (all of a collapsed panel): the button goes to
            // its end.
            let drawn = if collapsed { 0 } else { plan.shown.min(shown.len()) };
            // Buttons narrower than the panel's minimum width (a one-button panel) are centred.
            let inset = if drawn == 0 { 0.0 } else { ((width - 8.0 - (drawn as f32 * stride - 2.0)) / 2.0).max(0.0).floor() };
            let bx = px + 4.0 + inset;
            let free_x = bx + drawn as f32 * stride;
            let free = Rect::from_min_size(pos2(free_x, top + 2.0), vec2((px + width - 4.0 - free_x).max(8.0), 38.0));
            let fresp = ui.interact(free, ui.id().with(("panel_free", panel)), if collapsed { Sense::click() } else { Sense::hover() });
            let mut opened = false;
            if collapsed {
                // One button: the panel's first command's icon over a caret; it opens the
                // drop-down, which lists every command of the panel.
                let br = Rect::from_center_size(free.center(), vec2(38.0, 38.0));
                crate::scenario::publish_handle(&format!("collapsed:{tab}:{panel}"), br.center());
                if fresp.hovered() && enabled_panel {
                    painter.rect_filled(br, 4.0, t.hover);
                }
                if let Some(c) = shown.first().or(cmds.first()) {
                    let acc = command_accent(c.icon);
                    icons::paint(&painter, Rect::from_center_size(br.center() - vec2(0.0, 3.0), vec2(26.0, 26.0)), c.icon, t.icon, t.icon_fill, acc);
                }
                caret(&painter, pos2(br.center().x, br.bottom() - 3.0), t.text_dim);
                let tip = format!("{} ({})", app.tr_label(panel), shown.len().max(cmds.len()));
                if fresp.on_hover_text(tip).clicked() && enabled_panel {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("sc_panel_open"), popup_id));
                    opened = true;
                }
            }
            for (i, c) in shown.iter().enumerate().take(drawn) {
                // Only the family's default pin follows the part's choice. Explicitly pinned
                // variants remain exact commands, including in customised toolbars.
                let family = crate::sketch_families::family(c.id).filter(|f| sketch_create && f.commands.first().copied() == Some(c.id));
                let selected = family.and_then(|f| app.sketch_choices.selected(f)).unwrap_or(c);
                let whole = Rect::from_min_size(pos2(bx + i as f32 * stride, top + 2.0), vec2(stride - 2.0, 38.0));
                let mut br = whole;
                if family.is_some() {
                    br.max.y -= 12.0;
                }
                let info = selected.info(&app.session);
                let resp = ui.interact(br, ui.id().with(("cmd", c.id)), Sense::click_and_drag());
                crate::scenario::publish_handle(&format!("toolbar:{}", c.id), br.center());
                // The button itself (`toolbar:<id>` is also a drop-down row's while one is open).
                crate::scenario::publish_handle(&format!("button:{}", c.id), br.center());
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
                if running.as_deref() == Some(selected.id) {
                    painter.rect_filled(br, 4.0, t.border.gamma_multiply(0.7));
                } else if resp.hovered() && info.enabled {
                    painter.rect_filled(br, 4.0, t.hover);
                }
                let (ink, fill, acc) =
                    if info.enabled { (t.icon, t.icon_fill, command_accent(selected.icon)) } else { (t.border, t.panel_header, t.border) };
                let icon = match selected.id {
                    "sketch.finish" => "finish",
                    _ => selected.icon,
                };
                let icon_rect = if labelled {
                    Rect::from_center_size(pos2(br.center().x, br.top() + 13.0), vec2(24.0, 24.0))
                } else if sketch_create {
                    Rect::from_center_size(br.center(), vec2(24.0, 24.0))
                } else {
                    br.shrink(6.0)
                };
                icons::paint(&painter, icon_rect, icon, ink, fill, acc);
                let label = match c.id {
                    "parameters.change" if *panel == "CONFIGURE" => "Parameters",
                    "config.table" if *panel == "CONFIGURE" => "Configurations",
                    _ => selected.label,
                };
                let tr_label = app.tr_label(label);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled() && info.enabled, tr_label));
                let tip = match &info.disabled_reason {
                    Some(why) => format!("{}\n{why}", tr_label),
                    None => format!("{}{}", tr_label, selected.shortcut.map(|k| format!("  ({k})")).unwrap_or_default()),
                };
                if resp.on_hover_text(tip).clicked() && info.enabled {
                    let id = selected.id;
                    app.start(id);
                }
                if let Some(f) = family {
                    let ar = Rect::from_min_max(pos2(whole.left(), br.bottom()), whole.right_bottom());
                    let arrow = ui.interact(ar, ui.id().with(("family", f.label)), Sense::click());
                    crate::scenario::publish_handle(&format!("family:{}", f.label), ar.center());
                    if arrow.hovered() {
                        painter.rect_filled(ar, 3.0, t.hover);
                    }
                    if labelled {
                        let g = short_label(&painter, app.tr_label(f.label), ar.width() - 12.0, t.text);
                        painter.galley(ar.center() - vec2(4.0 + g.size().x / 2.0, g.size().y / 2.0), g, t.text);
                        caret(&painter, pos2(ar.right() - 5.0, ar.center().y), t.text);
                    } else {
                        caret(&painter, ar.center(), t.text);
                    }
                    arrow.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} variants", f.label)));
                    egui::Popup::menu(&arrow).show(|ui| {
                        let variants: Vec<_> = f.commands.iter().filter_map(|id| solidcraft_engine::find_command(id)).collect();
                        dropdown_keys(app, ui, &variants);
                        sketch_menu_rows(app, ui, &variants, &mut menu, &mut toggle);
                    });
                } else if labelled {
                    let g = short_label(&painter, app.tr_label(c.label), br.width() - 4.0, t.text);
                    painter.galley(pos2(br.center().x - g.size().x / 2.0, br.bottom() - 5.0 - g.size().y / 2.0), g, t.text);
                }
            }
            if shown.is_empty() && !collapsed {
                let br = Rect::from_min_size(pos2(px + 4.0, top + 2.0), vec2(width - 8.0, 38.0));
                painter.text(br.center(), Align2::CENTER_CENTER, "—", FontId::proportional(14.0), t.border);
            }
            // Panel label with drop-down.
            let label = app.tr_label(panel).to_string();
            let lc = if *panel == "FINISH SKETCH" {
                t.sketch_accent
            } else if enabled_panel {
                t.text
            } else {
                t.text_dim
            };
            // A label too wide for its panel (beside the caret) wraps between words onto a
            // second line; a single word wider than the panel stays whole. Each line is
            // centred, and the hover area grows to hold them.
            let font = FontId::proportional(10.5);
            let widest_word = label.split_whitespace().map(|w| painter.layout_no_wrap(w.to_string(), font.clone(), lc).size().x).fold(0.0, f32::max);
            let mut job = egui::text::LayoutJob::simple(label, font, lc, (width - 14.0).max(widest_word + 1.0));
            job.wrap.max_rows = 2;
            job.halign = egui::Align::Center;
            let g = painter.layout_job(job);
            let gw = g.size().x;
            let lr = Rect::from_center_size(pos2(px + width / 2.0, top + 51.0), vec2(width, (g.size().y + 2.0).max(18.0)));
            let lresp = ui.interact(lr, ui.id().with(("panel", panel)), Sense::click());
            crate::scenario::publish_handle(&format!("panel:{tab}:{panel}"), lr.center());
            if lresp.hovered() && enabled_panel {
                painter.rect_filled(lr, 3.0, t.hover);
            }
            // Centred rows sit around x = 0, so the galley is placed at the text's centre.
            let gx = lr.center().x - 4.5;
            let gy = lr.center().y - g.size().y / 2.0;
            painter.galley(pos2(gx, gy), g, lc);
            caret(&painter, pos2(gx + gw / 2.0 + 6.0, lr.center().y), lc);
            // The label toggles its drop-down (#1522).
            if lresp.clicked() {
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
                (0..drawn).any(|i| Rect::from_min_size(pos2(bx + i as f32 * stride, top + 2.0), vec2(stride - 2.0, 38.0)).contains(p))
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
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(200.0);
                        if *panel == "SELECT" {
                            selection_filter(app, ui);
                        } else if cmds.is_empty() {
                            ui.label(egui::RichText::new(crate::i18n::tr_label("Not available yet")).color(t.text_dim));
                        }
                        if sketch_create {
                            sketch_create_menu(app, ui, &cmds, &mut menu, &mut toggle);
                            return;
                        }
                        let row_w = dropdown_width(app, ui, &cmds);
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
                        let room = (ui.ctx().content_rect().bottom() - ui.cursor().top() - 12.0).max(ROW_H * 3.0);
                        egui::ScrollArea::vertical().id_salt("panel_menu_rows").max_height(room).min_scrolled_height(room).show(ui, |ui| {
                            for c in &cmds {
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
            painter.line_segment([pos2(px, top + 4.0), pos2(px, top + 58.0)], Stroke::new(1.0, t.border));
            px += 2.0;
        }
        if !overflow.is_empty() {
            let at = Rect::from_min_size(pos2(px.min(right - 40.0), top + 4.0), vec2(38.0, 52.0));
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
        if right < r.right() {
            ai_button(app, ui, Rect::from_min_max(pos2(right + 4.0, r.top() + 4.0), pos2(r.right() - 6.0, r.bottom() - 5.0)));
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

/// A button's label on one line in `max_width`, shortened with an ellipsis when it doesn't fit.
fn short_label(painter: &egui::Painter, text: &str, max_width: f32, color: Color32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), FontId::proportional(10.0), color);
    job.wrap = egui::text::TextWrapping { max_width: max_width.max(8.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    painter.layout_job(job)
}

/// Room the AI button takes at the toolbar row's right end.
const AI_BUTTON_ROOM: f32 = 52.0;

/// The AI button: shows and hides the agent chat panel (lit while it is open).
fn ai_button(app: &mut SolveApp, ui: &mut egui::Ui, at: Rect) {
    let t = Tokens::get();
    let resp = ui.interact(at, ui.id().with("ai_button"), Sense::click());
    crate::scenario::publish_handle("toolbar:ai", at.center());
    crate::scenario::publish_rect("toolbar:ai", at);
    let open = app.agent_chat.open;
    let painter = ui.painter();
    painter.line_segment([pos2(at.left() - 4.0, at.top() + 4.0), pos2(at.left() - 4.0, at.bottom() - 4.0)], Stroke::new(1.0, t.border));
    if open || resp.hovered() {
        painter.rect_filled(at, 4.0, if open { t.accent_soft } else { t.hover });
    }
    icons::paint(
        painter,
        Rect::from_center_size(pos2(at.center().x, at.top() + 24.0), vec2(26.0, 26.0)),
        "agent_chat",
        t.icon,
        t.icon_fill,
        t.accent,
    );
    painter.text(pos2(at.center().x, at.top() + 52.0), Align2::CENTER_CENTER, crate::i18n::tr("ui.ai_button"), FontId::proportional(11.0), t.text);
    let waiting = app.agent_chat.waiting();
    if waiting > 0 {
        let c = pos2(at.right() - 6.0, at.top() + 9.0);
        painter.circle_filled(c, 6.0, t.warning);
        painter.text(c, Align2::CENTER_CENTER, waiting.min(9).to_string(), FontId::proportional(9.5), t.toolbar);
    }
    if resp.on_hover_text(crate::agent_chat::panel_tip()).clicked() {
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

const ROW_H: f32 = 24.0;

/// The id of a drop-down row (for keyboard focus).
/// A drop-down row's id, the same inside and outside the rows' scroll area (one drop-down is
/// open at a time).
fn dropdown_row_id(id: &str) -> egui::Id {
    egui::Id::new(("dropdown_row", id))
}

/// How wide a panel drop-down's rows are: the widest label and shortcut, at least 200.
fn dropdown_width(app: &SolveApp, ui: &egui::Ui, cmds: &[&'static solidcraft_engine::CommandSpec]) -> f32 {
    let t = Tokens::get();
    let font = FontId::proportional(13.0);
    cmds.iter()
        .map(|c| {
            let label = ui.painter().layout_no_wrap(app.tr_label(c.label).to_string(), font.clone(), t.text).size().x;
            let key = c.shortcut.map(|k| ui.painter().layout_no_wrap(k.to_string(), font.clone(), t.text).size().x + 24.0).unwrap_or(0.0);
            6.0 + 18.0 + 8.0 + label + key + 30.0
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
    // Rows reached with the arrow keys scroll into view in a drop-down taller than the window.
    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::ArrowUp)) {
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
    let ir = Rect::from_center_size(pos2(rect.left() + 6.0 + 9.0, rect.center().y), vec2(18.0, 18.0));
    icons::paint(p, ir, c.icon, ink, fill, acc);
    p.text(pos2(ir.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, &label, FontId::proportional(13.0), text);
    if let Some(k) = c.shortcut {
        p.text(pos2(pr.left() - 4.0, rect.center().y), Align2::RIGHT_CENTER, k, FontId::proportional(13.0), t.text_dim);
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
    let room = (ui.ctx().content_rect().bottom() - ui.cursor().top() - 12.0).max(ROW_H * 3.0);
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
    p.rect(r, 4.0, t.toolbar.gamma_multiply(0.92), Stroke::new(1.0, if removing { t.error } else { t.accent }), egui::StrokeKind::Inside);
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
    ui.label(egui::RichText::new(crate::i18n::tr_label("Selection filter")).color(t.text_dim).size(11.0));
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
                let at = ctx.input(|i| i.pointer.latest_pos());
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
        buttons.iter().map(|&b| PanelSize { buttons: b, labelled: false }).collect()
    }

    fn used_with_more(plans: &[Plan]) -> f32 {
        used(plans) + if plans.iter().any(|p| p.fit == Fit::More) { MORE_ROOM } else { 0.0 }
    }

    #[test]
    fn panels_give_up_one_button_at_a_time_then_move_to_more_and_none_is_dropped() {
        // A panel with 14 buttons and three default ones.
        let panels = sizes(&[14, 4, 2, 2]);
        let full: f32 = [14, 4, 2, 2].iter().map(|&b| panel_width(b) + PANEL_GAP).sum();
        let all = |plans: &[Plan]| plans.iter().map(|p| (p.fit, p.shown)).collect::<Vec<_>>();
        assert_eq!(all(&fit(&panels, full)), [(Fit::Buttons, 14), (Fit::Buttons, 4), (Fit::Buttons, 2), (Fit::Buttons, 2)]);
        // A little short: the widest panel hands one button to its drop-down; the others keep all theirs.
        assert_eq!(all(&fit(&panels, full - 10.0)), [(Fit::Buttons, 13), (Fit::Buttons, 4), (Fit::Buttons, 2), (Fit::Buttons, 2)]);
        // Every panel down to one button (collapsed) fits.
        let collapsed = 4.0 * (COLLAPSED_WIDTH + PANEL_GAP);
        assert_eq!(all(&fit(&panels, collapsed)), [(Fit::Collapsed, 1); 4]);
        // Narrower: the last one goes into the » menu, and the room it frees goes back to buttons.
        let more = fit(&panels, collapsed - 1.0);
        assert_eq!(more.iter().map(|p| p.fit).collect::<Vec<_>>(), [Fit::Buttons, Fit::Buttons, Fit::Collapsed, Fit::More]);
        assert!(more.iter().take(3).all(|p| p.shown >= 1));
        // Then all of them.
        assert!(fit(&panels, 0.0).iter().all(|p| p.fit == Fit::More));
        assert_eq!(fit(&panels, f32::NAN).len(), 4);
        // No cap: 40 px a button however many there are.
        assert_eq!(panel_width(30) - panel_width(20), 400.0);
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
            let panels: Vec<PanelSize> = layout.iter().map(|&(buttons, labelled)| PanelSize { buttons, labelled }).collect();
            let mut last: Option<(f32, usize)> = None;
            for room in (0..=2000).rev().map(|r| r as f32) {
                let plans = fit(&panels, room);
                let width = used_with_more(&plans);
                let hidden = plans
                    .iter()
                    .zip(&panels)
                    .any(|(p, s)| p.fit != Fit::Buttons || p.shown < s.buttons || p.stride < if s.labelled { LABELLED_STRIDE } else { STRIDE });
                let more = plans.iter().filter(|p| p.fit == Fit::More).count();
                if more < panels.len() {
                    assert!(width <= room + 0.01, "{layout:?} at {room}: {width} overruns");
                }
                // One button: an icon's stride, or a collapsed panel's when the next panel waits in the » menu.
                let button = if more > 0 { COLLAPSED_WIDTH + PANEL_GAP } else { STRIDE };
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
                    if p.fit != Fit::More && s.buttons > 0 {
                        assert!(p.shown >= 1, "{layout:?} at {room}: a panel without icons");
                    }
                }
            }
        }
    }

    fn captured_frame(h: &mut crate::scenario::Harness) {
        let token = h.app.shot_token;
        crate::issue_report::receive(&mut h.app, token, std::sync::Arc::new(egui::ColorImage::new([2, 2], vec![Color32::GREEN; 4])));
        h.frames(3);
    }

    #[test]
    fn header_report_reserves_controls_at_normal_and_double_scale_widths() {
        for physical_width in [900.0, 1400.0] {
            for scale in [1.0, 2.0] {
                let width = physical_width / scale;
                let bar = Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 34.0));
                for captions in [0.0, crate::titlebar::WIDTH] {
                    // The English label, and a longer one (Russian is about 125 px).
                    for label_width in [80.0, 125.0] {
                        let (report, labeled) = report_rect(bar, captions, 354.0, label_width);
                        assert!(bar.contains_rect(report));
                        let prefs = Rect::from_center_size(pos2(width - captions - 70.0, 17.0), vec2(24.0, 24.0));
                        let help = Rect::from_center_size(pos2(help_x(bar, captions), 17.0), vec2(26.0, 26.0));
                        assert!(!report.intersects(prefs) && !report.intersects(help) && !help.intersects(prefs));
                        assert!(report.right() < help.left() && help.right() < prefs.left());
                        let full = (REPORT_TEXT_X + label_width + 10.0).max(124.0);
                        assert_eq!(labeled, width - captions - RIGHT_GROUP - 354.0 >= 216.0 + full);
                        if labeled {
                            assert!(REPORT_TEXT_X + label_width < report.width(), "{label_width}: the text overruns its button");
                        }
                    }
                }
            }
        }
    }

    /// The title bar: the File menu, Undo, Redo and every tab on the left; Help, Report a bug,
    /// Preferences and the window's buttons on the right. Home is a tab and the theme a
    /// preference, so neither has a button of its own, and Save is in the File menu (Ctrl+S).
    #[test]
    fn title_bar_keeps_tabs_left_and_help_right() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.custom_titlebar = true;
        h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
        h.call("ui.resize", json!({"width":1400,"height":720}));
        h.frames(3);
        for gone in ["appbar:home", "appbar:sun", "appbar:moon", "appbar:save"] {
            assert!(crate::scenario::handle_at(gone).is_none(), "{gone} is no longer in the title bar");
        }
        let at = |name: &str| crate::scenario::handle_at(name).unwrap_or_else(|| panic!("{name}")).x;
        let [report, _] = h.widget("Report a bug").unwrap();
        let file = h.widget("File").expect("the File menu has a label");
        assert_eq!(file[0] as f32, at("appbar:file"));
        assert!(at("appbar:file") < at("appbar:undo") && at("appbar:undo") < at("appbar:redo"));
        assert!(at("appbar:redo") < at("tab:home") && at("tab:home") < at("tab:design:0"));
        assert!((at("tab:design:0") as f64) < report && report < at("appbar:help") as f64);
        assert!(at("appbar:help") < at("titlebar:preferences"));
        h.step(&json!({"click":{"handle":"appbar:help"}})).unwrap();
        assert!(h.app.help.menu, "? opens the Help menu");
        h.step(&json!({"key":"Escape"})).unwrap();
        h.step(&json!({"click":{"handle":"tab:home"}})).unwrap();
        assert!(h.app.home.open, "the Home tab shows the start page");
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
        assert!((en - 124.0).abs() < 0.5, "{en}");
        // "Сообщить об ошибке" needs more than the English button's 124 px.
        assert!(ru > 124.0 + 10.0, "{ru}");
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
        h.call("ui.resize", json!({"width":1200,"height":480}));
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.frames(4);
        h.step(&json!({"click":{"handle":"panel:SKETCH:CREATE"}})).unwrap();
        h.frames(4);
        let menu = crate::scenario::rect_of("toolbar_dropdown").expect("the drop-down is open");
        assert!(menu.bottom() <= 480.0, "the drop-down runs past the window: {menu:?}");
        assert!(crate::scenario::handle_at("dropdown:sketch.intersect").is_some_and(|p| !menu.contains(p)), "Intersect starts below the fold");
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
        let open = |h: &mut crate::scenario::Harness, tab: &str| {
            h.app.ui.tab = tab.into();
            h.frames(4);
            h.step(&json!({"click":{"handle":format!("panel:{tab}:MODIFY")}})).unwrap();
            h.frames(6);
            let menu = crate::scenario::rect_of("toolbar_dropdown").expect("the drop-down is open");
            let rows = crate::scenario::handles("dropdown:");
            h.step(&json!({"key":"Escape"})).unwrap();
            h.frames(4);
            (menu, rows)
        };
        let (short, short_rows) = open(&mut h, "MESH");
        let (menu, rows) = open(&mut h, "PLASTIC");
        assert!(rows.len() > short_rows.len(), "PLASTIC MODIFY lists more than MESH MODIFY: {rows:?} vs {short_rows:?}");
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
            h.step(&json!({"click":{"handle":"panel:SOLID:CREATE"}})).unwrap();
            let label = crate::scenario::handle_at("panel:SOLID:CREATE").unwrap();
            let destination = match target {
                "gap" => label - vec2(0.0, 10.0),
                "left margin" => pos2(label.x - panel_width(4) / 2.0 + 1.0, label.y - 30.0),
                "right margin" => pos2(label.x + panel_width(4) / 2.0 - 1.0, label.y - 30.0),
                "button" => crate::scenario::handle_at("button:solid.extrude").unwrap(),
                _ => label,
            };
            h.step(&json!({"drag":[{"handle":"dropdown:solid.cylinder"}, [destination.x,destination.y]],"steps":12})).unwrap();
            h.step(&json!({"key":"Escape"})).unwrap();
            let order = if target == "button" {
                json!(["button:sketch.create", "button:solid.cylinder", "button:solid.extrude", "button:solid.revolve", "button:solid.sweep"])
            } else {
                json!(["button:sketch.create", "button:solid.extrude", "button:solid.revolve", "button:solid.sweep", "button:solid.cylinder"])
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
        h.step(&json!({"drag":[{"handle":"button:solid.extrude"},{"handle":"panel:SOLID:MODIFY"}],"steps":12})).unwrap();
        h.step(&json!({"expect":{"handle_order":["button:solid.fillet","button:solid.chamfer","button:solid.scale","button:solid.extrude"],"tool":null,"dialog":null,"errors":0}})).unwrap();
        h.step(&json!({"restart":"close"})).unwrap();
        h.step(&json!({"expect":{"handle_order":["button:solid.scale","button:solid.extrude"]}})).unwrap();
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

        h.step(&json!({"click": {"handle": "panel:SOLID:CREATE"}})).unwrap();
        h.frames(2);
        assert!(rect_of("toolbar_dropdown").is_some(), "the first click opens the panel's list");
        h.step(&json!({"click": {"handle": "panel:SOLID:CREATE"}})).unwrap();
        h.frames(2);
        assert!(rect_of("toolbar_dropdown").is_none(), "the second click closes it");
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
            let tab = if sketch { "SKETCH" } else { "SOLID" };
            h.app.ui.tab = tab.into();
            for w in [1920.0, 1600.0, 1440.0, 1366.0, 1280.0, 1100.0, 900.0, 700.0] {
                h.call("ui.resize", json!({"width":w,"height":900}));
                h.frames(3);
                let at = format!("{tab} at {w}");
                let bar = crate::scenario::rect_of("toolbar").unwrap();
                let ai = crate::scenario::rect_of("toolbar:ai");
                let right = ai.map_or(bar.right(), |a| a.left() - 4.0);
                let count = |name: &str| crate::scenario::count_of(name).unwrap_or(-1.0);
                let (collapsed, trimmed, more) = (count("toolbar_collapsed"), count("toolbar_trimmed"), count("toolbar_more"));
                let lists = crate::toolbar_custom::panel_lists(tab);
                let mut end = bar.left();
                let mut on_bar = 0;
                for (panel, cmds, promote) in &lists {
                    let Some(rect) = crate::scenario::rect_of(&format!("panel:{tab}:{panel}")) else { continue };
                    on_bar += 1;
                    end = end.max(rect.right() + PANEL_GAP);
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
                let button = if more > 0.0 { COLLAPSED_WIDTH + PANEL_GAP } else { STRIDE };
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
                    assert!(o.right() <= right + 4.0, "{at}: the » button under the AI button");
                }
                // The tab row stops short of the AI button.
                for t in TABS.iter().chain(["SKETCH"].iter()) {
                    if let (Some(r), Some(ai)) = (crate::scenario::rect_of(&format!("toolbar_tab:{t}")), ai) {
                        assert!(r.right() < ai.left(), "{at}: the {t} tab runs into the AI button");
                    }
                }
                // The title bar: its document tabs end before Report a bug, Help and Preferences.
                let report = crate::scenario::rect_of("titlebar:report").unwrap();
                let tabs_end = crate::scenario::handles("tab:").iter().map(|(_, p)| p.x).fold(0.0, f32::max);
                assert!(tabs_end < report.left(), "{at}: the document tabs run into Report a bug");
                let help = crate::scenario::handle_at("appbar:help").unwrap();
                let prefs = crate::scenario::handle_at("titlebar:preferences").unwrap();
                assert!(report.right() < help.x && help.x < prefs.x, "{at}: the title bar's right-hand controls overlap");
            }
        }
    }
}
