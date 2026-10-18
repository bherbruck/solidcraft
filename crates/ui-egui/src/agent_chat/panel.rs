//! The agent chat panel: docked left or right (switched by dragging its header to the other side,
//! or in Preferences), resizable, folded to a thin strip with Minimise, closed with ×. Its side,
//! width and fold are preferences, so they come back next time.

use egui::{Align, Align2, Color32, Id, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};

use super::*;
use crate::theme::Tokens;

pub const MIN_WIDTH: f32 = 280.0;
pub const MAX_WIDTH: f32 = 760.0;
/// The folded panel's width.
const STRIP: f32 = 30.0;
/// The keyboard shortcut that shows and hides the panel (also from its own text field).
pub const SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT), egui::Key::A);

fn panel_id(left: bool) -> Id {
    Id::new(if left { "sc_agent_chat_left" } else { "sc_agent_chat_right" })
}

/// Forget the panel's own idea of its width, so the preference applies on the next frame.
pub fn forget_width(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        for left in [true, false] {
            d.remove::<egui::containers::panel::PanelState>(panel_id(left));
        }
    });
}

/// Show or hide the panel (a folded panel unfolds first).
pub fn toggle(app: &mut SolveApp) {
    let c = &mut app.agent_chat;
    if !c.open {
        c.open = true;
        app.preferences.agent_chat_minimized = false;
    } else if app.preferences.agent_chat_minimized {
        app.preferences.agent_chat_minimized = false;
    } else {
        c.open = false;
    }
}

/// The tooltip of the chat's buttons: its name and shortcut.
pub fn tip() -> String {
    let key = if cfg!(target_os = "macos") { "Cmd+Shift+A" } else { "Ctrl+Shift+A" };
    format!("{} ({key})", tr("ui.agent_chat"))
}

/// The panel, when the chat is turned on and open. Call before the central viewport is laid out.
pub fn show(app: &mut SolveApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    poll(app, &ctx);
    // With AI features off nothing of the chat is drawn, and the shortcut does nothing.
    if !shown(app) {
        return;
    }
    super::settings::dialog(app, &ctx);
    if ctx.input_mut(|i| i.consume_shortcut(&SHORTCUT)) {
        toggle(app);
    }
    // A window that isn't drawn this frame closes; the next one opens at the saved place.
    if !(app.agent_chat.open && app.preferences.agent_chat_popped) {
        app.agent_chat.window_placed = false;
    }
    // New chat, when the chat has focus (its input, or the pointer over the panel).
    let in_chat = ctx.memory(|m| m.has_focus(Id::new("sc_agent_chat_input")))
        || ctx.pointer_hover_pos().is_some_and(|p| app.agent_chat.panel_rect.is_some_and(|r| r.contains(p)));
    if app.agent_chat.open && in_chat && ctx.input_mut(|i| i.consume_shortcut(&NEW_CHAT_SHORTCUT)) {
        request_new_chat(app);
    }
    if !app.agent_chat.open {
        return;
    }
    // Look for installed agents the first time the panel opens (in the background).
    #[cfg(not(target_arch = "wasm32"))]
    if app.agent_chat.detection.is_none() && app.agent_chat.detecting.is_none() {
        refresh(app, &ctx);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if app.preferences.agent_chat_popped {
        window(app, &ctx);
        return;
    }
    let t = Tokens::get();
    let left = app.preferences.agent_chat_side == "left";
    let side = |id: Id| if left { egui::Panel::left(id) } else { egui::Panel::right(id) };
    if app.preferences.agent_chat_minimized {
        side(Id::new("sc_agent_chat_strip")).exact_size(STRIP).resizable(false).frame(egui::Frame::NONE.fill(t.panel)).show(ui, |ui| strip(app, ui));
        return;
    }
    let width = app.preferences.agent_chat_width.clamp(MIN_WIDTH, MAX_WIDTH);
    let r = side(panel_id(left))
        .default_size(width)
        .size_range(MIN_WIDTH..=MAX_WIDTH)
        .resizable(true)
        .frame(egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin::symmetric(8, 6)))
        .show(ui, |ui| body(app, ui, left));
    crate::scenario::publish_rect("agent_chat:panel", r.response.rect);
    app.agent_chat.panel_rect = Some(r.response.rect);
    let w = r.response.rect.width();
    if w.is_finite() && (MIN_WIDTH..=MAX_WIDTH).contains(&w) {
        app.preferences.agent_chat_width = w;
    }
}

/// New chat (when the chat has focus).
pub const NEW_CHAT_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT), egui::Key::L);

/// Clear the chat, asking first when something would be lost.
fn request_new_chat(app: &mut SolveApp) {
    if new_chat_needs_confirm(app) {
        app.agent_chat.confirm_new = true;
    } else {
        new_chat(app);
    }
}

/// A saved window place (`[x, y, width, height]`) that can be used: finite, a usable size, and
/// not absurdly far away. `x` and `y` may be negative: a screen left of or above the main one.
pub fn window_place(saved: Option<[f32; 4]>) -> Option<([f32; 2], [f32; 2])> {
    let [x, y, w, h] = saved?;
    let usable = [x, y, w, h].iter().all(|v| v.is_finite())
        && (MIN_WIDTH..=10_000.0).contains(&w)
        && (200.0..=10_000.0).contains(&h)
        && x.abs() <= 100_000.0
        && y.abs() <= 100_000.0;
    usable.then_some(([x, y], [w, h]))
}

/// The chat window's settings. The saved place and size go only to a window that is opening:
/// egui turns a changed place into a move, and the system rounds what it is given, so giving it
/// every frame made the window chase its own rounding (it shook a pixel up and down).
pub fn window_builder(app: &SolveApp) -> egui::ViewportBuilder {
    let title = format!("{} — {}", app.session.doc.name, tr("ui.agent_chat"));
    let builder = egui::ViewportBuilder::default().with_title(title).with_min_inner_size([MIN_WIDTH, 320.0]);
    if app.agent_chat.window_placed {
        return builder;
    }
    match window_place(app.preferences.agent_chat_window) {
        Some((pos, size)) => builder.with_position(pos).with_inner_size(size),
        None => builder.with_inner_size([app.preferences.agent_chat_width.clamp(MIN_WIDTH, MAX_WIDTH), 720.0]),
    }
}

/// The chat in its own window, named after the design: a native window on the desktop, which can
/// go to another screen, or a window floating inside the app where the system can't open one.
/// The system's close button closes the chat (it opens there again); Dock puts it back.
#[cfg(not(target_arch = "wasm32"))]
fn window(app: &mut SolveApp, ctx: &egui::Context) {
    let builder = window_builder(app);
    app.agent_chat.window_placed = true;
    // The side panel isn't drawn: its place no longer counts as "in the chat".
    app.agent_chat.panel_rect = None;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("sc_agent_chat_window"), builder, |ui, class| {
        let t = Tokens::get();
        let frame = egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin::symmetric(8, 6));
        let r = egui::CentralPanel::default().frame(frame).show(ui, |ui| body(app, ui, false));
        if class == egui::ViewportClass::EmbeddedWindow {
            // Floating inside the app: its handles are the main window's, as the panel's are.
            crate::scenario::publish_rect("agent_chat:window", r.response.rect);
            app.agent_chat.panel_rect = Some(r.response.rect);
            return;
        }
        let ctx = ui.ctx().clone();
        if ctx.input_mut(|i| i.consume_shortcut(&SHORTCUT)) {
            toggle(app);
        }
        // The whole window is the chat, so New chat works anywhere in it.
        if ctx.input_mut(|i| i.consume_shortcut(&NEW_CHAT_SHORTCUT)) {
            request_new_chat(app);
        }
        let (outer, inner, close) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect, i.viewport().close_requested()));
        if let (Some(o), Some(n)) = (outer, inner) {
            app.preferences.agent_chat_window = Some([o.min.x, o.min.y, n.width(), n.height()]);
        }
        if close {
            app.agent_chat.open = false;
        }
    });
}

/// Whether the chat's own window should become active because the pointer is over its text box:
/// only when it is in a window of its own that isn't active and the app's main window is (never
/// taking the focus from another app).
pub fn take_focus_for_text(popped: bool, over_text: bool, chat_active: Option<bool>, main_active: Option<bool>) -> bool {
    popped && over_text && chat_active == Some(false) && main_active == Some(true)
}

/// A small drawn button: `kind` is `minimize`, `close`, `newchat`, `stopturn`, `pop_out` or `dock`.
fn glyph_button(ui: &mut egui::Ui, kind: &str, tip: &str) -> egui::Response {
    let t = Tokens::get();
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 4.0, t.hover);
    }
    let c = r.center();
    let s = Stroke::new(1.4, t.text);
    let p = ui.painter();
    match kind {
        "minimize" => {
            p.line_segment([c + vec2(-5.0, 4.0), c + vec2(5.0, 4.0)], s);
        }
        "close" => {
            p.line_segment([c + vec2(-4.5, -4.5), c + vec2(4.5, 4.5)], s);
            p.line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], s);
        }
        // A window with an arrow leaving it by its top right corner.
        "pop_out" => {
            p.line_segment([c + vec2(-1.0, -5.0), c + vec2(-5.0, -5.0)], s);
            p.line_segment([c + vec2(-5.0, -5.0), c + vec2(-5.0, 5.0)], s);
            p.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, 5.0)], s);
            p.line_segment([c + vec2(5.0, 5.0), c + vec2(5.0, 1.0)], s);
            p.line_segment([c + vec2(-1.0, 1.0), c + vec2(5.5, -5.5)], s);
            p.line_segment([c + vec2(1.5, -5.5), c + vec2(5.5, -5.5)], s);
            p.line_segment([c + vec2(5.5, -5.5), c + vec2(5.5, -1.5)], s);
        }
        // A window with its right side filled: the panel docked.
        "dock" => {
            p.rect_stroke(Rect::from_center_size(c, vec2(11.0, 10.0)), 1.0, s, egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(c + vec2(1.5, -5.0), c + vec2(5.5, 5.0)), 0.0, t.text);
        }
        "newchat" => {
            // A speech bubble with a plus.
            ui.painter().rect_stroke(Rect::from_center_size(c, vec2(14.0, 11.0)), 2.5, s, egui::StrokeKind::Inside);
            ui.painter().line_segment([c + vec2(-2.5, 0.0), c + vec2(2.5, 0.0)], Stroke::new(1.2, t.text));
            ui.painter().line_segment([c + vec2(0.0, -2.5), c + vec2(0.0, 2.5)], Stroke::new(1.2, t.text));
        }
        "stopturn" => {
            ui.painter().rect_filled(Rect::from_center_size(c, vec2(9.0, 9.0)), 1.5, t.error);
        }
        _ => {}
    }
    crate::scenario::publish_handle(&format!("agent_chat:{kind}"), r.center());
    resp.on_hover_text(tip)
}

fn state_color(state: State) -> Color32 {
    let t = Tokens::get();
    match state {
        State::Ready | State::Busy => t.accent,
        State::NeedsAuth | State::Exited => t.warning,
        _ => t.text_dim,
    }
}

/// The folded panel: the chat icon (unfolds it), the agent's state, and a mark while something
/// waits for the user.
fn strip(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let r = ui.max_rect();
    let button = Rect::from_center_size(pos2(r.center().x, r.top() + 22.0), vec2(26.0, 26.0));
    let resp = ui.interact(button, Id::new("sc_agent_chat_unfold"), Sense::click());
    crate::scenario::publish_handle("agent_chat:unfold", button.center());
    if resp.hovered() {
        ui.painter().rect_filled(button, 4.0, t.hover);
    }
    crate::icons::paint(ui.painter(), button.shrink(4.0), "agent_chat", t.icon, t.icon_fill, t.accent);
    let state = app.agent_chat.state();
    ui.painter().circle_filled(pos2(r.center().x, button.bottom() + 10.0), 3.5, state_color(state));
    let waiting = app.agent_chat.waiting();
    if waiting > 0 {
        let at = button.right_top() + vec2(-3.0, 3.0);
        ui.painter().circle_filled(at, 6.5, t.warning);
        ui.painter().text(at, Align2::CENTER_CENTER, waiting.min(9).to_string(), egui::FontId::proportional(9.5), t.panel);
    }
    ui.painter().line_segment(
        if app.preferences.agent_chat_side == "left" { [r.right_top(), r.right_bottom()] } else { [r.left_top(), r.left_bottom()] },
        Stroke::new(1.0, t.border),
    );
    let tip = format!("{} · {}", tip(), state.label());
    if resp.on_hover_text(tip).clicked() {
        app.preferences.agent_chat_minimized = false;
    }
}

/// The header: a grip to drag the panel to the other side, the title and state, Connect or Stop,
/// Pop out (or Dock, in its own window), Minimise and Close. In its own window the system moves
/// and minimises it, so there is no grip and no Minimise.
fn header(app: &mut SolveApp, ui: &mut egui::Ui, left: bool) {
    let t = Tokens::get();
    let state = app.agent_chat.state();
    let popped = app.preferences.agent_chat_popped;
    ui.horizontal(|ui| {
        let title = {
            let (grip, _) = ui.allocate_exact_size(vec2(if popped { 0.0 } else { 10.0 }, 18.0), Sense::hover());
            if !popped {
                for row in 0..3 {
                    for col in 0..2 {
                        ui.painter().circle_filled(grip.left_top() + vec2(2.5 + col as f32 * 5.0, 4.0 + row as f32 * 5.0), 1.1, t.text_dim);
                    }
                }
            }
            let l = ui.label(RichText::new(tr("ui.agent_chat")).strong().size(14.0));
            grip.union(l.rect)
        };
        let mut s = state.label().to_string();
        if let Some(turn) = &app.agent_chat.turn
            && state == State::Busy
        {
            let limit = app.preferences.agent_chat_turn_limit;
            s = format!("{s} {}", trf("ui.agent_chat_changes_of", &[("n", &turn.changes.to_string()), ("limit", &limit.to_string())]));
        }
        ui.label(RichText::new(s).color(state_color(state)).small());
        // Drag the header across the window to dock on the other side.
        if !popped {
            let drag = ui.interact(title, Id::new("sc_agent_chat_grip"), Sense::drag());
            crate::scenario::publish_handle("agent_chat:grip", title.center());
            dock_drag(app, ui, &drag, left);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if glyph_button(ui, "close", tr("ui.agent_chat_close")).clicked() {
                app.agent_chat.open = false;
            }
            if !popped && glyph_button(ui, "minimize", tr("ui.agent_chat_minimize")).clicked() {
                app.preferences.agent_chat_minimized = true;
            }
            #[cfg(not(target_arch = "wasm32"))]
            if glyph_button(
                ui,
                "newchat",
                &format!("{} ({})", tr("ui.agent_chat_new"), if cfg!(target_os = "macos") { "Cmd+Shift+L" } else { "Ctrl+Shift+L" }),
            )
            .clicked()
            {
                request_new_chat(app);
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let (kind, tip) = if popped { ("dock", "ui.agent_chat_dock") } else { ("pop_out", "ui.agent_chat_pop_out") };
                if glyph_button(ui, kind, tr(tip)).clicked() {
                    app.preferences.agent_chat_popped = !popped;
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let b = if state.running() {
                    ui.button(tr("ui.agent_chat_stop")).on_hover_text(tr("ui.agent_chat_stop_tip"))
                } else {
                    ui.button(tr("ui.agent_chat_connect"))
                };
                crate::scenario::publish_handle("agent_chat:connect", b.rect.center());
                if b.clicked() {
                    if state.running() {
                        disconnect(app);
                    } else {
                        connect(app, ui.ctx());
                    }
                }
            }
        });
    });
}

/// While the header is dragged: the cursor grabs and the side it would dock on lights up; on
/// release over the other half of the window the panel moves there.
fn dock_drag(app: &mut SolveApp, ui: &egui::Ui, drag: &egui::Response, left: bool) {
    if drag.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    let screen = ui.ctx().content_rect();
    let target = |x: f32| x < screen.center().x;
    if drag.dragged()
        && let Some(p) = ui.ctx().pointer_interact_pos()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let to_left = target(p.x);
        let w = app.preferences.agent_chat_width.clamp(MIN_WIDTH, MAX_WIDTH);
        let top = screen.top() + crate::toolbar::APP_BAR_HEIGHT;
        let zone = if to_left {
            Rect::from_min_max(pos2(screen.left(), top), pos2(screen.left() + w, screen.bottom()))
        } else {
            Rect::from_min_max(pos2(screen.right() - w, top), pos2(screen.right(), screen.bottom()))
        };
        let t = Tokens::get();
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("sc_agent_chat_dock_hint")));
        painter.rect(zone.shrink(4.0), 8.0, t.accent.gamma_multiply(0.18), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        painter.text(
            zone.center(),
            Align2::CENTER_CENTER,
            tr(if to_left { "ui.agent_chat_dock_left" } else { "ui.agent_chat_dock_right" }),
            egui::FontId::proportional(14.0),
            t.text,
        );
    }
    if drag.drag_stopped()
        && let Some(p) = ui.ctx().pointer_interact_pos().or_else(|| ui.ctx().pointer_latest_pos())
        && target(p.x) != left
    {
        app.preferences.agent_chat_side = if left { "right" } else { "left" }.into();
    }
}

fn body(app: &mut SolveApp, ui: &mut egui::Ui, left: bool) {
    header(app, ui, left);
    #[cfg(not(target_arch = "wasm32"))]
    switcher(app, ui);
    #[cfg(target_arch = "wasm32")]
    {
        ui.label(tr("ui.agent_chat_desktop_only"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    desktop_body(app, ui);
}

#[cfg(not(target_arch = "wasm32"))]
fn desktop_body(app: &mut SolveApp, ui: &mut egui::Ui) {
    if app.agent_chat.detection.is_none() && app.agent_chat.detecting.is_none() {
        refresh(app, ui.ctx());
    }
    let state = app.agent_chat.state();
    if app.agent_chat.confirm_new {
        let t = Tokens::get();
        egui::Frame::NONE.fill(t.accent_soft).corner_radius(6.0).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(tr("ui.agent_chat_new_confirm"));
            ui.horizontal(|ui| {
                let yes = ui.button(tr("ui.agent_chat_new_yes"));
                crate::scenario::publish_handle("agent_chat:new_confirm", yes.rect.center());
                if yes.clicked() {
                    new_chat(app);
                }
                if ui.button(tr("ui.agent_chat_cancel")).clicked() {
                    app.agent_chat.confirm_new = false;
                }
            });
        });
    }
    picker(app, ui);
    if state == State::NeedsAuth {
        auth_box(app, ui);
    }
    ui.separator();
    // The input sits at the bottom; the conversation takes the rest.
    let input_h = 92.0;
    let avail = (ui.available_height() - input_h).max(80.0);
    egui::ScrollArea::vertical().id_salt("sc_agent_chat_scroll").max_height(avail).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        conversation(app, ui);
    });
    ui.separator();
    let busy = state == State::Busy;
    let edit = egui::TextEdit::multiline(&mut app.agent_chat.input)
        .hint_text(tr(if busy { "ui.agent_chat_placeholder_busy" } else { "ui.agent_chat_placeholder" }))
        .desired_rows(2)
        .desired_width(f32::INFINITY)
        .id_salt("sc_agent_chat_input");
    // Enter sends (queues while the agent works); Ctrl/Cmd+Enter sends now; Esc stops. They are
    // read before the field sees them so no newline is typed.
    let focused = ui.memory(|m| m.has_focus(Id::new("sc_agent_chat_input")));
    let (steer, esc) = if focused {
        ui.input_mut(|i| (i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter), busy && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)))
    } else {
        (false, false)
    };
    let r = ui.add(edit);
    crate::scenario::publish_rect("agent_chat:input", r.rect);
    // macOS shows a window's cursors only while it is the active window: over the text box of a
    // chat window that isn't, the pointer stays an arrow. Coming from the main window, the chat
    // window becomes active, so the text cursor shows and the first click places the caret.
    let (chat_active, main_active) = (ui.ctx().input(|i| i.viewport().focused), ui.ctx().input_for(egui::ViewportId::ROOT, |i| i.viewport().focused));
    if take_focus_for_text(app.preferences.agent_chat_popped, r.hovered(), chat_active, main_active) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    if esc {
        cancel(app);
    }
    ui.horizontal(|ui| {
        let send_b = ui.button(tr(if busy { "ui.agent_chat_queue" } else { "ui.agent_chat_send" }));
        crate::scenario::publish_handle("agent_chat:send", send_b.rect.center());
        if (send_b.clicked() || enter || steer) && !app.agent_chat.input.trim().is_empty() {
            let text = std::mem::take(&mut app.agent_chat.input).trim_end_matches('\n').to_string();
            send(app, ui.ctx(), text);
            if steer {
                send_now(app, ui.ctx());
            }
        } else if steer {
            // Nothing typed: Ctrl+Enter sends what is already queued.
            send_now(app, ui.ctx());
        }
        if busy && glyph_button(ui, "stopturn", tr("ui.agent_chat_stop_turn_tip")).clicked() {
            cancel(app);
        }
        // A picture of the view goes with the message to agents that take pictures.
        let images = app.agent_chat.images || !state.running();
        let attach = ui
            .add_enabled(images, egui::Checkbox::new(&mut app.agent_chat.attach_view, tr("ui.agent_chat_attach_view")))
            .on_hover_text(tr("ui.agent_chat_attach_view_tip"))
            .on_disabled_hover_text(tr("ui.agent_chat_no_images"));
        crate::scenario::publish_handle("agent_chat:attach", attach.rect.center());
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn login_chip(ui: &mut egui::Ui, login: &str) {
    let t = Tokens::get();
    let (text, color) = match login {
        "signed_in" => (tr("ui.agent_chat_signed_in"), t.accent),
        "key" => (tr("ui.agent_chat_key_in_env"), t.accent),
        "signed_out" => (tr("ui.agent_chat_not_signed_in"), t.warning),
        _ => return,
    };
    egui::Frame::NONE.stroke(Stroke::new(1.0, color)).corner_radius(8.0).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
        ui.label(RichText::new(text).color(color).size(11.0));
    });
}

/// A small mark for an agent's kind: a terminal prompt for a program, a cloud for an endpoint.
pub(super) fn kind_icon(ui: &mut egui::Ui, kind: agents::Kind) {
    let t = Tokens::get();
    let (r, _) = ui.allocate_exact_size(vec2(16.0, 14.0), Sense::hover());
    let p = ui.painter();
    let s = Stroke::new(1.3, t.text_dim);
    match kind {
        agents::Kind::Acp => {
            p.rect_stroke(r.shrink(1.0), 2.0, s, egui::StrokeKind::Inside);
            p.line_segment([r.left_top() + vec2(4.0, 4.0), r.left_top() + vec2(7.0, 7.0)], s);
            p.line_segment([r.left_top() + vec2(7.0, 7.0), r.left_top() + vec2(4.0, 10.0)], s);
            p.line_segment([r.left_top() + vec2(8.5, 10.0), r.left_top() + vec2(12.0, 10.0)], s);
        }
        agents::Kind::Api => {
            let c = r.center();
            p.circle_stroke(c + vec2(-3.0, 1.5), 3.2, s);
            p.circle_stroke(c + vec2(1.0, -1.0), 4.2, s);
            p.circle_stroke(c + vec2(4.0, 2.0), 2.8, s);
        }
    }
}

/// The header's second row: the agent this conversation uses (a compact switcher), the agent's
/// own mode when it has modes, and the state of its login.
#[cfg(not(target_arch = "wasm32"))]
fn switcher(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let all = agents::entries(app);
    let current = agents::current(app);
    ui.horizontal(|ui| {
        let label = current.as_ref().map_or_else(
            || tr(if app.agent_chat.detecting.is_some() { "ui.agent_chat_detecting" } else { "ui.agent_chat_none_found_short" }).to_string(),
            |e| e.name.clone(),
        );
        let models = models_shown(app);
        let w =
            (ui.available_width() - if app.agent_chat.modes.is_empty() { 4.0 } else { 116.0 } - if models { 146.0 } else { 0.0 }).clamp(90.0, 260.0);
        let mut picked: Option<String> = None;
        let mut add = false;
        let combo = egui::ComboBox::from_id_salt("sc_agent_switch").width(w).selected_text(RichText::new(label).strong()).show_ui(ui, |ui| {
            for e in &all {
                let sel = current.as_ref().is_some_and(|c| c.id == e.id);
                let r = ui.horizontal(|ui| {
                    kind_icon(ui, e.kind);
                    let r = ui.selectable_label(sel, &e.name);
                    ui.label(RichText::new(e.detail()).color(t.text_dim).size(11.0));
                    login_chip(ui, e.login);
                    r
                });
                crate::scenario::publish_handle(&format!("agent_chat:agent:{}", e.id), r.inner.rect.center());
                if r.inner.clicked() {
                    picked = Some(e.id.clone());
                }
            }
            ui.separator();
            if ui.selectable_label(false, tr("ui.ai_add_agent")).clicked() {
                add = true;
            }
            if ui.selectable_label(false, tr("ui.ai_settings")).clicked() {
                crate::prefs::open_at(app, crate::prefs::PAGE_AI);
            }
        });
        crate::scenario::publish_handle("agent_chat:switcher", combo.response.rect.center());
        combo.response.on_hover_text(tr("ui.ai_switch_tip"));
        if add {
            super::settings::open_add(app);
        }
        if let Some(id) = picked
            && current.as_ref().is_none_or(|c| c.id != id)
        {
            disconnect(app);
            app.agent_chat.agent_id = Some(id);
        }
        // The agent's own modes (ACP `modes`): its own tools follow them.
        if !app.agent_chat.modes.is_empty() {
            let modes = app.agent_chat.modes.clone();
            let cur = modes.iter().find(|(id, _)| *id == app.agent_chat.mode).map_or_else(|| app.agent_chat.mode.clone(), |(_, n)| n.clone());
            let mut pick: Option<String> = None;
            let r = egui::ComboBox::from_id_salt("sc_agent_mode").width(100.0).selected_text(cur).show_ui(ui, |ui| {
                for (id, name) in &modes {
                    if ui.selectable_label(*id == app.agent_chat.mode, name).clicked() {
                        pick = Some(id.clone());
                    }
                }
            });
            crate::scenario::publish_handle("agent_chat:mode", r.response.rect.center());
            r.response.on_hover_text(tr("ui.ai_agent_mode_tip"));
            if let Some(m) = pick {
                set_mode(app, &m);
            }
        }
        // The agent's models (ACP `models`, or an endpoint's list): shown only when it reports some.
        if models {
            let list = app.agent_chat.models.clone();
            let cur = list.iter().find(|m| m.id == app.agent_chat.model).map_or_else(|| app.agent_chat.model.clone(), |m| m.name.clone());
            let mut pick: Option<String> = None;
            let r = egui::ComboBox::from_id_salt("sc_agent_model").width(140.0).truncate().selected_text(cur).show_ui(ui, |ui| {
                for m in &list {
                    let row = ui.selectable_label(m.id == app.agent_chat.model, &m.name);
                    let row = if m.description.is_empty() { row } else { row.on_hover_text(&m.description) };
                    crate::scenario::publish_handle(&format!("agent_chat:model:{}", m.id), row.rect.center());
                    if row.clicked() {
                        pick = Some(m.id.clone());
                    }
                }
            });
            crate::scenario::publish_handle("agent_chat:model", r.response.rect.center());
            r.response.on_hover_text(tr("ui.agent_chat_model_tip"));
            if let Some(m) = pick {
                set_model(app, &m);
            }
        }
    });
    if let Some(a) = &app.agent_chat.auth_status {
        ui.label(RichText::new(a).color(t.text_dim).size(11.0));
    }
}

/// Before the first connection: what's installed, how to install the rest, and Add agent.
#[cfg(not(target_arch = "wasm32"))]
fn picker(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let open = app.agent_chat.picking.unwrap_or(!app.agent_chat.state().running() && app.agent_chat.items.is_empty());
    if !open {
        return;
    }
    let all = agents::entries(app);
    let current = agents::current(app);
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr("ui.agent_chat_pick")).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let refreshing = app.agent_chat.detecting.is_some();
            if ui.add_enabled(!refreshing, egui::Button::new(tr("ui.agent_chat_refresh")).small()).clicked() {
                refresh(app, ui.ctx());
            }
            if ui.small_button(tr("ui.ai_add_agent")).clicked() {
                super::settings::open_add(app);
            }
        });
    });
    if app.agent_chat.detecting.is_some() {
        ui.label(RichText::new(tr("ui.agent_chat_detecting")).color(t.text_dim));
    }
    for e in &all {
        let selected = current.as_ref().is_some_and(|c| c.id == e.id);
        let fill = if selected { t.accent_soft } else { Color32::TRANSPARENT };
        let r = egui::Frame::NONE.fill(fill).corner_radius(5.0).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                kind_icon(ui, e.kind);
                ui.label(RichText::new(&e.name).strong().color(t.text));
                ui.label(RichText::new(e.detail()).color(t.text_dim).size(11.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| login_chip(ui, e.login));
            });
        });
        let resp = ui.interact(r.response.rect, Id::new(("sc_agent_pick", &e.id)), Sense::click());
        crate::scenario::publish_handle(&format!("agent_chat:pick:{}", e.id), resp.rect.center());
        if resp.clicked() {
            app.agent_chat.agent_id = Some(e.id.clone());
        }
    }
    let missing: Vec<discover::Missing> = app.agent_chat.detection.as_ref().map(|d| d.missing.clone()).unwrap_or_default();
    if !missing.is_empty() {
        let heading = if all.is_empty() { tr("ui.agent_chat_none_found") } else { tr("ui.agent_chat_not_installed") };
        egui::CollapsingHeader::new(RichText::new(heading).color(t.text_dim)).id_salt("sc_agent_missing").default_open(all.is_empty()).show(
            ui,
            |ui| {
                for m in missing {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(m.name).strong());
                        if ui.small_button(tr("ui.agent_chat_copy")).on_hover_text(m.install).clicked() {
                            ui.ctx().copy_text(m.install.to_string());
                        }
                    });
                    ui.add(egui::Label::new(RichText::new(m.install).monospace().size(11.0).color(t.text_dim)).selectable(true).wrap());
                }
            },
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn auth_box(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    egui::Frame::group(ui.style()).stroke(Stroke::new(1.0, t.warning)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(tr("ui.agent_chat_auth_title")).strong());
        ui.label(RichText::new(tr("ui.agent_chat_auth_needed")).color(t.text_dim));
        let methods = app.agent_chat.auth.clone();
        let base = app.agent_chat.argv.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
        if methods.is_empty() {
            ui.label(tr("ui.agent_chat_auth_none"));
        }
        for m in methods {
            ui.add_space(4.0);
            let desc = m.description.trim();
            ui.label(RichText::new(if desc.is_empty() || desc == m.name { m.name.clone() } else { format!("{} — {desc}", m.name) }).strong());
            match &m.kind {
                acp::AuthKind::Terminal { args, .. } => {
                    ui.label(tr("ui.agent_chat_auth_terminal"));
                    let cmd = format!("{base} {}", args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "));
                    ui.horizontal_wrapped(|ui| {
                        egui::Frame::NONE.fill(t.toolbar).corner_radius(4.0).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(&cmd).monospace().size(11.0)).selectable(true).wrap());
                        });
                        if ui.small_button(tr("ui.agent_chat_copy")).clicked() {
                            ui.ctx().copy_text(cmd.clone());
                        }
                    });
                }
                acp::AuthKind::Agent => {
                    let b = ui.button(trf("ui.agent_chat_auth_agent", &[("name", &m.name)]));
                    crate::scenario::publish_handle(&format!("agent_chat:auth:{}", m.id), b.rect.center());
                    if b.clicked() {
                        app.agent_chat.authenticate(&m.id);
                    }
                }
                acp::AuthKind::EnvVar { vars } => {
                    ui.label(trf("ui.agent_chat_auth_env", &[("vars", &vars.join(", "))]));
                }
            }
        }
        ui.add_space(4.0);
        if ui.button(tr("ui.agent_chat_auth_retry")).clicked() {
            connect(app, ui.ctx());
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn conversation(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let mut answer: Option<(usize, Option<String>)> = None;
    // Allow (true), Deny (false), Always allow (None).
    let mut approval: Option<(u64, Option<bool>)> = None;
    let mut undo: Option<usize> = None;
    let current_doc = app.session.doc.clone();
    for (n, item) in app.agent_chat.items.iter().enumerate() {
        match item {
            Item::User { text, image } => {
                ui.add_space(6.0);
                egui::Frame::NONE.fill(t.accent_soft).corner_radius(6.0).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(RichText::new(text).color(t.text)).selectable(true).wrap());
                    if *image {
                        ui.label(RichText::new(tr("ui.agent_chat_view_attached")).color(t.text_dim).size(11.0));
                    }
                });
            }
            Item::Agent { thought, text } => {
                if *thought {
                    ui.add(egui::Label::new(RichText::new(text).italics().color(t.text_dim)).selectable(true).wrap());
                } else {
                    super::markdown_view::show(ui, n, text);
                }
            }
            Item::Tool { title, kind, status, output, .. } => {
                tool_card(ui, n, title, kind, *status, output.as_deref());
            }
            Item::Plan(entries) => {
                for (c, s) in entries {
                    let (color, filled) = match s.as_str() {
                        "completed" => (t.accent, true),
                        "in_progress" => (t.warning, true),
                        _ => (t.text_dim, false),
                    };
                    ui.horizontal(|ui| {
                        status_dot(ui, color, filled);
                        ui.label(RichText::new(c).color(t.text_dim));
                    });
                }
            }
            Item::Permission { title, options, answer: given, .. } => {
                egui::Frame::group(ui.style()).stroke(Stroke::new(1.0, t.accent)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(trf("ui.agent_chat_permission", &[("title", title)])).strong());
                    match given {
                        Some(a) => {
                            ui.label(RichText::new(trf("ui.agent_chat_answered", &[("option", a)])).color(t.text_dim));
                        }
                        None => {
                            ui.horizontal_wrapped(|ui| {
                                for o in options {
                                    let b = egui::Button::new(&o.name);
                                    let b = if o.kind.starts_with("allow") { b.fill(t.accent_soft) } else { b };
                                    let r = ui.add(b);
                                    crate::scenario::publish_handle(&format!("agent_chat:permission:{}", o.id), r.rect.center());
                                    if r.clicked() {
                                        answer = Some((n, Some(o.id.clone())));
                                    }
                                }
                            });
                        }
                    }
                });
            }
            Item::Approval { id, title, tool, answer: given } => {
                egui::Frame::group(ui.style()).stroke(Stroke::new(1.5, t.warning)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(tr("ui.agent_chat_approval")).strong());
                    ui.add(egui::Label::new(RichText::new(title).monospace().size(11.5)).selectable(true).wrap());
                    match given {
                        Some(true) => {
                            ui.label(RichText::new(tr("ui.agent_chat_allowed")).color(t.text_dim));
                        }
                        Some(false) => {
                            ui.label(RichText::new(tr("ui.agent_chat_denied")).color(t.text_dim));
                        }
                        None => {
                            ui.horizontal(|ui| {
                                let allow = ui.add(egui::Button::new(tr("ui.agent_chat_allow")).fill(t.accent_soft));
                                let always = ui.button(tr("ui.ai_always_allow")).on_hover_text(trf("ui.ai_always_allow_tip", &[("tool", tool)]));
                                let deny = ui.button(tr("ui.agent_chat_deny"));
                                crate::scenario::publish_handle("agent_chat:allow", allow.rect.center());
                                crate::scenario::publish_handle("agent_chat:always", always.rect.center());
                                crate::scenario::publish_handle("agent_chat:deny", deny.rect.center());
                                if allow.clicked() {
                                    approval = Some((*id, Some(true)));
                                }
                                if always.clicked() {
                                    approval = Some((*id, None));
                                }
                                if deny.clicked() {
                                    approval = Some((*id, Some(false)));
                                }
                            });
                        }
                    }
                });
            }
            Item::Changes { steps, doc, undone } => {
                ui.horizontal(|ui| {
                    let text = trf("ui.agent_chat_changes", &[("n", &steps.to_string())]);
                    ui.label(RichText::new(text).color(t.text_dim).size(11.5));
                    if *undone {
                        ui.label(RichText::new(tr("ui.agent_chat_undone")).color(t.text_dim).size(11.5));
                    } else if Arc::ptr_eq(doc, &current_doc) {
                        let b = ui.button(tr("ui.agent_chat_undo_changes")).on_hover_text(tr("ui.ai_undo_tip"));
                        crate::scenario::publish_handle("agent_chat:undo", b.rect.center());
                        if b.clicked() {
                            undo = Some(n);
                        }
                    }
                });
            }
            Item::Notice { text, error } => {
                ui.label(RichText::new(text).color(if *error { t.error } else { t.text_dim }));
            }
        }
        ui.add_space(2.0);
    }
    let (mut send_now_clicked, mut send_queue_clicked) = (false, false);
    let mut remove: Option<usize> = None;
    let mut edit_done: Option<(usize, Option<String>)> = None;
    let busy = app.agent_chat.state() == State::Busy;
    let stopped = app.agent_chat.stopped;
    let editing = app.agent_chat.pending_edit.clone();
    let mut start_edit: Option<(usize, String)> = None;
    let mut edit_buf = editing.as_ref().map(|(_, b)| b.clone());
    for (n, p) in app.agent_chat.pending.iter().enumerate() {
        ui.add_space(6.0);
        egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.accent)).corner_radius(6.0).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let tag = if stopped && !busy { "ui.agent_chat_queued_stopped" } else { "ui.agent_chat_queued" };
                ui.label(RichText::new(tr(tag)).color(t.accent).strong().size(11.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let x = ui.add(egui::Button::new(RichText::new("×").size(14.0)).frame(false)).on_hover_text(tr("ui.agent_chat_remove"));
                    crate::scenario::publish_handle(&format!("agent_chat:pending_remove:{n}"), x.rect.center());
                    if x.clicked() {
                        remove = Some(n);
                    }
                    if editing.as_ref().is_none_or(|(i, _)| *i != n) {
                        let e = ui.add(egui::Button::new(RichText::new(tr("ui.agent_chat_edit")).size(11.5)).frame(false));
                        crate::scenario::publish_handle(&format!("agent_chat:pending_edit:{n}"), e.rect.center());
                        if e.clicked() {
                            start_edit = Some((n, p.text.clone()));
                        }
                    }
                });
            });
            match (&editing, edit_buf.as_mut()) {
                (Some((i, _)), Some(buf)) if *i == n => {
                    let te = ui.add(egui::TextEdit::multiline(buf).desired_rows(2).desired_width(f32::INFINITY));
                    crate::scenario::publish_rect("agent_chat:pending_input", te.rect);
                    ui.horizontal(|ui| {
                        if ui.button(tr("ui.agent_chat_save")).clicked() {
                            edit_done = Some((n, Some(buf.clone())));
                        }
                        if ui.button(tr("ui.agent_chat_cancel")).clicked() {
                            edit_done = Some((n, None));
                        }
                    });
                }
                _ => {
                    ui.add(egui::Label::new(RichText::new(&p.text).color(t.text)).selectable(true).wrap());
                    if p.image {
                        ui.label(RichText::new(tr("ui.agent_chat_view_attached")).color(t.text_dim).size(11.0));
                    }
                }
            }
        });
    }
    if !app.agent_chat.pending.is_empty() && editing.is_none() {
        ui.horizontal(|ui| {
            if busy {
                let b = ui.button(tr("ui.agent_chat_send_now")).on_hover_text(tr("ui.agent_chat_send_now_tip"));
                crate::scenario::publish_handle("agent_chat:send_now", b.rect.center());
                send_now_clicked = b.clicked();
            } else {
                let b = ui.add(egui::Button::new(tr("ui.agent_chat_send")).fill(t.accent_soft));
                crate::scenario::publish_handle("agent_chat:send_queue", b.rect.center());
                send_queue_clicked = b.clicked();
            }
        });
    }
    if let Some(start) = start_edit {
        app.agent_chat.pending_edit = Some(start);
    } else if let (Some(buf), Some(slot)) = (edit_buf, app.agent_chat.pending_edit.as_mut()) {
        slot.1 = buf;
    }
    if app.agent_chat.items.is_empty() && app.agent_chat.pending.is_empty() {
        ui.add_space(12.0);
        ui.label(RichText::new(tr("ui.agent_chat_intro")).color(t.text_dim));
        ui.add_space(6.0);
        ui.label(RichText::new(tr("ui.agent_chat_privacy")).color(t.text_dim).size(11.0));
    }
    if let Some((n, text)) = edit_done {
        if let (Some(text), Some(p)) = (text.filter(|t| !t.trim().is_empty()), app.agent_chat.pending.get_mut(n)) {
            p.text = text;
        }
        app.agent_chat.pending_edit = None;
    }
    if let Some(n) = remove {
        remove_pending(app, n);
    }
    if send_now_clicked {
        send_now(app, ui.ctx());
    }
    if send_queue_clicked {
        send_now(app, ui.ctx());
    }
    if let Some((n, o)) = answer {
        answer_permission(app, n, o.as_deref());
    }
    match approval {
        Some((id, Some(allow))) => approve(app, id, allow),
        Some((id, None)) => approve_always(app, id),
        None => {}
    }
    if let Some(n) = undo {
        undo_changes(app, n);
    }
}

/// A tool call as a card: its state, name and kind on one line; a click unfolds what it returned.
#[cfg(not(target_arch = "wasm32"))]
fn tool_card(ui: &mut egui::Ui, n: usize, title: &str, kind: &str, status: Option<ToolStatus>, output: Option<&str>) {
    let t = Tokens::get();
    let (color, filled, state) = match status {
        Some(ToolStatus::Completed) => (t.accent, true, "ui.ai_tool_done"),
        Some(ToolStatus::Failed) => (t.error, true, "ui.ai_tool_failed"),
        Some(ToolStatus::InProgress) => (t.warning, true, "ui.ai_tool_running"),
        _ => (t.text_dim, false, "ui.ai_tool_waiting"),
    };
    egui::Frame::NONE.fill(t.toolbar).stroke(Stroke::new(1.0, t.border)).corner_radius(6.0).inner_margin(egui::Margin::symmetric(6, 3)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            let id = ui.make_persistent_id(("sc_agent_tool", n));
            let mut state_ = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
            let head = ui.horizontal(|ui| {
                status_dot(ui, color, filled);
                // The state and kind keep their room at the right; the title is cut short before them.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(tr(state)).size(10.5).color(color));
                    if !kind.is_empty() && kind != "other" {
                        ui.label(RichText::new(kind).size(10.5).color(t.text_dim));
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(title).monospace().size(11.5).color(t.text)).truncate()).on_hover_text(title);
                    });
                });
            });
            let click = ui.interact(head.response.rect, id.with("head"), Sense::click());
            crate::scenario::publish_handle(&format!("agent_chat:tool:{n}"), head.response.rect.center());
            if output.is_some() && click.clicked() {
                state_.toggle(ui);
            }
            if output.is_some() {
                click.on_hover_text(tr("ui.ai_tool_unfold"));
            }
            state_.show_body_unindented(ui, |ui| {
                if let Some(o) = output {
                    ui.separator();
                    egui::ScrollArea::vertical().id_salt(("sc_agent_tool_out", n)).max_height(180.0).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(o).monospace().size(11.0)).selectable(true).wrap());
                    });
                }
            });
        },
    );
}

/// A small status mark drawn as a shape (no glyph a system font might lack): filled when the
/// step is running or done, hollow while it waits.
#[cfg(not(target_arch = "wasm32"))]
fn status_dot(ui: &mut egui::Ui, color: Color32, filled: bool) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    if filled {
        ui.painter().circle_filled(r.center(), 4.0, color);
    } else {
        ui.painter().circle_stroke(r.center(), 3.5, Stroke::new(1.2, color));
    }
}

/// A custom command as the picker shows it: the program's file name and its arguments, cut short.
pub(super) fn short_command(line: &str) -> String {
    let argv = acp::split_command(line);
    let mut parts: Vec<String> = argv.iter().map(|a| a.rsplit(['/', '\\']).next().unwrap_or(a).to_string()).collect();
    if parts.is_empty() {
        parts.push(line.to_string());
    }
    let s = parts.join(" ");
    if s.chars().count() > 40 { format!("{}…", s.chars().take(40).collect::<String>()) } else { s }
}

/// Quote an argument for the command field when it holds spaces.
pub(super) fn quote(a: &str) -> String {
    if a.is_empty() || a.contains(char::is_whitespace) || a.contains('"') { format!("\"{}\"", a.replace('"', "\\\"")) } else { a.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_commands_show_short_in_the_picker() {
        assert_eq!(short_command("python3 -I /very/long/path/to/mock_agent.py"), "python3 -I mock_agent.py");
        assert!(short_command(&"x".repeat(100)).ends_with('…'));
    }

    #[test]
    fn the_shortcut_shows_folds_and_hides() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        toggle(&mut app);
        assert!(app.agent_chat.open);
        app.preferences.agent_chat_minimized = true;
        toggle(&mut app);
        assert!(app.agent_chat.open && !app.preferences.agent_chat_minimized, "a folded panel unfolds");
        toggle(&mut app);
        assert!(!app.agent_chat.open);
    }
}
