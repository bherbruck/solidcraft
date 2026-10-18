//! The S box: a command search that opens at the cursor (the S key). It searches every command by
//! name, id, toolbar tab and panel with a forgiving (fuzzy) match. With nothing typed it lists
//! the recent commands first. Pinned commands sit in a row of icons on top, a small toolbar of
//! favourites. Up/Down choose, Enter runs, Esc closes. Recent and pinned commands are kept
//! between runs.

use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_engine::command_specs;

use crate::theme::Tokens;
use crate::{SolveApp, icons};

/// How many recent commands are kept.
const RECENT: usize = 12;
const W: f32 = 340.0;

#[derive(Default)]
pub struct ShortcutBox {
    /// Where it opened (the cursor), while open.
    at: Option<Pos2>,
    /// The highlighted row.
    index: usize,
    /// What is typed.
    pub text: String,
    /// Ask the field for focus on the next frame. Set when the box opens, then cleared.
    /// Requesting focus on every frame restarts the text edit's held key, so a Wayland
    /// repeat never sees the release and the key keeps repeating after the app loses focus (#724).
    focus: bool,
    /// Most recent first.
    pub recent: Vec<String>,
    pub pinned: Vec<String>,
}

impl ShortcutBox {
    pub fn prefs(&self) -> Value {
        json!({ "recent": self.recent, "pinned": self.pinned })
    }
    pub fn load(&mut self, v: &Value) {
        let list = |k: &str| -> Vec<String> {
            v.get(k).and_then(Value::as_array).map(|a| a.iter().take(64).filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        self.recent = list("recent");
        self.pinned = list("pinned");
    }
    /// A command was run from the box: it goes to the front of the recent list.
    pub fn used(&mut self, id: &str) {
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_string());
        self.recent.truncate(RECENT);
    }
    pub fn toggle_pin(&mut self, id: &str) {
        if self.pinned.iter().any(|p| p == id) {
            self.pinned.retain(|p| p != id);
        } else {
            self.pinned.push(id.to_string());
        }
    }
}

/// How well `q` matches `s` (0 = not at all): a prefix beats a word start beats a substring
/// beats the letters in order with gaps.
pub fn score(q: &str, s: &str) -> u32 {
    if q.is_empty() {
        return 1;
    }
    let s = s.to_lowercase();
    if s.starts_with(q) {
        return 1000 - s.len().min(500) as u32;
    }
    if s.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q)) {
        return 800;
    }
    if s.contains(q) {
        return 600;
    }
    // Letters in order: fewer gaps score higher.
    let mut gaps = 0u32;
    let mut it = s.chars();
    let mut last_hit = true;
    for qc in q.chars() {
        let mut found = false;
        for c in it.by_ref() {
            if c == qc {
                found = true;
                break;
            }
            if last_hit {
                gaps += 1;
            }
            last_hit = false;
        }
        if !found {
            return 0;
        }
        last_hit = true;
    }
    300u32.saturating_sub(gaps * 20).max(10)
}

/// Commands matching `q`, best first: (id, label, icon, where it lives on the toolbar). With an
/// empty query, recent commands come first, then everything by name.
pub fn search(app: &SolveApp, q: &str) -> Vec<(&'static str, &'static str, &'static str, String)> {
    let q = q.trim().to_lowercase();
    let mut v: Vec<(u32, &'static str, &'static str, &'static str, String)> = command_specs()
        .into_iter()
        .filter_map(|c| {
            let (tab, panel) = crate::workspace::destination(c).unwrap_or(("", ""));
            let place = if tab.is_empty() { String::new() } else { format!("{} › {}", app.tr_label(tab), app.tr_label(panel)) };
            let s = score(&q, c.label).max(score(&q, c.id) * 9 / 10).max(score(&q, tab) / 3).max(score(&q, panel) / 3);
            let recent = app.sbox.recent.iter().position(|r| r == c.id).map_or(0, |i| 2000 - i as u32);
            // Commands that can run now come before those that can't.
            let usable = if c.info(&app.session).enabled { 150 } else { 0 };
            (s > 0).then_some((s + usable + if q.is_empty() { recent } else { recent / 20 }, c.id, c.label, c.icon, place))
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(b.2)));
    v.into_iter().map(|(_, id, label, icon, place)| (id, label, icon, place)).collect()
}

/// Open the box at a point (the cursor), or at the middle of the window.
pub fn open(app: &mut SolveApp, at: Option<Pos2>) {
    app.ui.palette_open = true;
    app.sbox.at = at;
    app.sbox.index = 0;
    app.sbox.text.clear();
    app.sbox.focus = true;
}

fn run(app: &mut SolveApp, id: &str) {
    app.sbox.used(id);
    app.ui.palette_open = false;
    app.sbox.at = None;
    app.sbox.text.clear();
    app.start(id);
}

pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    if !app.ui.palette_open {
        app.sbox.at = None;
        app.sbox.focus = false;
        return;
    }
    let t = Tokens::get();
    let screen = ctx.content_rect();
    // Opened without `open` (a feature edit that falls through to the box): focus once.
    if app.sbox.at.is_none() {
        app.sbox.focus = true;
    }
    let at = *app.sbox.at.get_or_insert_with(|| ctx.input(|i| i.pointer.latest_pos()).unwrap_or(screen.center()));
    let list = search(app, &app.sbox.text);
    let (up, down, enter, esc) = ctx.input(|i| {
        (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape))
    });
    if down {
        app.sbox.index = (app.sbox.index + 1).min(list.len().saturating_sub(1));
    }
    if up {
        app.sbox.index = app.sbox.index.saturating_sub(1);
    }
    let mut picked: Option<&'static str> = None;
    let mut pin: Option<&'static str> = None;
    let mut menu: Option<(Pos2, &'static str)> = None;
    let w = W.min((screen.width() - 8.0).max(32.0));
    let h = 380.0_f32.min((screen.height() - 8.0).max(120.0));
    let pos = pos2(
        at.x.clamp(screen.left() + 4.0, (screen.right() - w - 4.0).max(screen.left() + 4.0)),
        at.y.clamp(screen.top() + 4.0, (screen.bottom() - h).max(screen.top() + 4.0)),
    );
    let resp = crate::frame::area(egui::Id::new("sc_sbox")).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
        crate::context_menu::menu_frame(ui, |ui| {
            ui.set_width(w - 10.0);
            // Pinned commands: a mini toolbar.
            if !app.sbox.pinned.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for id in app.sbox.pinned.clone() {
                        let Some(c) = solidcraft_engine::find_command(&id) else { continue };
                        let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 4.0, t.hover);
                        }
                        icons::paint(ui.painter(), r.shrink(5.0), c.icon, t.icon, t.icon_fill, t.accent);
                        if resp.on_hover_text(c.label).clicked() {
                            picked = Some(c.id);
                        }
                    }
                });
                ui.separator();
            }
            let te = ui.add(
                egui::TextEdit::singleline(&mut app.sbox.text)
                    .hint_text(crate::i18n::tr_label("Search commands"))
                    .desired_width(w - 20.0)
                    .id(egui::Id::new("sc_sbox_text")),
            );
            if std::mem::take(&mut app.sbox.focus) {
                te.request_focus();
            }
            if te.changed() {
                app.sbox.index = 0;
            }
            ui.add_space(4.0);
            if app.sbox.text.trim().is_empty() && !app.sbox.recent.is_empty() {
                ui.label(egui::RichText::new(crate::i18n::tr_label("RECENT")).size(10.0).color(t.text_dim));
            }
            egui::ScrollArea::vertical().max_height(h - 90.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, (id, label, icon, place)) in list.iter().enumerate().take(80) {
                    let enabled = solidcraft_engine::find_command(id).is_some_and(|c| c.info(&app.session).enabled);
                    let (r, resp) = ui.allocate_exact_size(vec2(w - 20.0, 26.0), Sense::click());
                    let lit = i == app.sbox.index || resp.hovered();
                    if lit {
                        ui.painter().rect_filled(r, 4.0, t.hover);
                        if i == app.sbox.index && (up || down) {
                            ui.scroll_to_rect(r, None);
                        }
                    }
                    let ink = if enabled { t.icon } else { t.border };
                    icons::paint(
                        ui.painter(),
                        Rect::from_center_size(pos2(r.left() + 14.0, r.center().y), vec2(16.0, 16.0)),
                        icon,
                        ink,
                        t.icon_fill,
                        t.accent,
                    );
                    let col = if enabled { t.text } else { t.text_dim };
                    ui.painter().text(pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, *label, FontId::proportional(13.0), col);
                    let key = crate::keymap::effective(app, id).unwrap_or_default();
                    let right = if key.is_empty() { place.clone() } else { format!("{key}   {place}") };
                    ui.painter().text(pos2(r.right() - 26.0, r.center().y), Align2::RIGHT_CENTER, right, FontId::proportional(10.5), t.text_dim);
                    // The pin.
                    let pinned = app.sbox.pinned.iter().any(|p| p == id);
                    let pr = Rect::from_center_size(pos2(r.right() - 11.0, r.center().y), vec2(16.0, 16.0));
                    let presp = ui.interact(pr, ui.id().with(("pin", *id)), Sense::click());
                    if pinned || lit {
                        let c = pr.center();
                        let pts: Vec<Pos2> = (0..10)
                            .map(|k| {
                                let a = std::f32::consts::PI * (k as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
                                let rr = if k % 2 == 0 { 6.0 } else { 2.6 };
                                c + vec2(a.cos() * rr, a.sin() * rr)
                            })
                            .collect();
                        if pinned {
                            ui.painter().add(egui::Shape::convex_polygon(pts.clone(), t.warning, Stroke::NONE));
                        }
                        ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.0, if pinned { t.warning } else { t.text_dim })));
                    }
                    if presp.on_hover_text(if pinned { "Unpin" } else { "Pin to the top of the S box" }).clicked() {
                        pin = Some(id);
                    } else if resp.clicked() && enabled {
                        picked = Some(id);
                    }
                    if resp.secondary_clicked() {
                        menu = Some((resp.interact_pointer_pos().unwrap_or(r.left_bottom()), id));
                    }
                }
            });
        });
    });
    crate::scenario::publish_rect("ribbon:search_popup", resp.response.rect);
    if let Some(id) = pin {
        app.sbox.toggle_pin(id);
    }
    // A row's menu: pin to the toolbar or the shortcuts.
    if let Some((at, id)) = menu {
        crate::context_menu::open_for(app, at, crate::context_menu::Target::ToolbarCommand { id: id.to_string() });
        return;
    }
    if enter
        && picked.is_none()
        && let Some((id, ..)) = list.get(app.sbox.index)
    {
        picked = Some(id);
    }
    let outside = ctx.input(|i| i.pointer.any_pressed()) && !resp.response.contains_pointer();
    if let Some(id) = picked {
        run(app, id);
    } else if esc || outside {
        app.ui.palette_open = false;
        app.sbox.at = None;
        app.sbox.text.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_scores_prefixes_over_words_over_letters() {
        assert!(score("ext", "Extrude") > score("ext", "Offset Text"));
        assert!(score("pres", "Press Pull") > score("ppl", "Press Pull"));
        assert!(score("ppl", "Press Pull") > 0);
        assert_eq!(score("xyz", "Extrude"), 0);
    }

    fn frame(app: &mut SolveApp, ctx: &egui::Context) {
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| show(app, ui.ctx()));
        out.textures_delta.clear();
    }

    /// The field takes focus when the box opens and is not asked again on later frames: asking on
    /// every frame restarts the text edit's held key, which kept a Wayland key repeat running (#724).
    #[test]
    fn the_search_field_asks_for_focus_once_per_opening() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        let ctx = egui::Context::default();
        let field = egui::Id::new("sc_sbox_text");
        open(&mut app, None);
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert!(ctx.memory(|m| m.has_focus(field)), "the field has focus once the box opens");
        ctx.memory_mut(|m| m.surrender_focus(field));
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert!(!ctx.memory(|m| m.has_focus(field)), "the field asked for focus again while open");
        app.ui.palette_open = false;
        frame(&mut app, &ctx);
        open(&mut app, None);
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert!(ctx.memory(|m| m.has_focus(field)), "reopening the box focuses the field again");
    }

    #[test]
    fn search_finds_by_name_id_and_panel_and_puts_recent_first() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        assert_eq!(search(&app, "extr").first().map(|x| x.0), Some("solid.extrude"));
        assert_eq!(search(&app, "solid.fil").first().map(|x| x.0), Some("solid.fillet"));
        assert!(search(&app, "constrain").iter().any(|x| x.0 == "sketch.constraint.parallel"), "by panel");
        app.sbox.used("solid.shell");
        app.sbox.used("solid.revolve");
        let all = search(&app, "");
        assert_eq!((all[0].0, all[1].0), ("solid.revolve", "solid.shell"));
        app.sbox.toggle_pin("solid.extrude");
        let mut other = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        other.sbox.load(&app.sbox.prefs());
        assert_eq!((other.sbox.pinned.clone(), other.sbox.recent.len()), (vec!["solid.extrude".to_string()], 2));
    }
}
