//! Keyboard shortcuts: every command's default key comes from its `CommandSpec`; the user can
//! rebind or clear any of them (Preferences › Keyboard Shortcuts). A new key that another command
//! already uses is reported and only taken over on request; keys the app itself handles (Esc, S,
//! V, Delete, Ctrl+Z…) can't be bound. Changes are kept between runs.

use std::collections::BTreeMap;

use egui::RichText;
use serde_json::{Value, json};
use solidcraft_engine::command_specs;

use crate::SolveApp;
use crate::theme::Tokens;

/// Keys the application handles before any command shortcut.
pub const RESERVED: &[&str] = &[
    "Escape",
    "S",
    "V",
    "F2",
    "F6",
    "Delete",
    "Backspace",
    "Ctrl+Z",
    "Ctrl+Y",
    "Ctrl+S",
    "Ctrl+N",
    "Ctrl+O",
    "Ctrl+Comma",
    "Ctrl+Shift+A",
    "Ctrl+C",
    "Ctrl+X",
    "Ctrl+V",
    // View keys (`navigation::view_key`).
    "Left",
    "Right",
    "Up",
    "Down",
    "Shift+Left",
    "Shift+Right",
    "Shift+Up",
    "Shift+Down",
    "Ctrl+Left",
    "Ctrl+Right",
    "Ctrl+Up",
    "Ctrl+Down",
    "Ctrl+8",
];

#[derive(Default)]
pub struct Keymap {
    /// Command id → key (`""`: no key). Only the changed ones.
    pub custom: BTreeMap<String, String>,
    /// Asks for the Keyboard Shortcuts page of Preferences (File and Help menus).
    pub open: bool,
    /// Opened from Help: a list to read, not to edit.
    pub read_only: bool,
    filter: String,
    /// The command waiting for a key press.
    capturing: Option<String>,
    recorder: Recorder,
    /// A key that another command uses: (command, key, the other commands).
    conflict: Option<(String, String, Vec<String>)>,
    message: Option<String>,
}

/// A chord is one ordinary key plus modifier state, as in the platform key-event model.
/// The one-second settling period is an editor affordance, not a shortcut activation delay.
const RECORD_SETTLE_SECONDS: f64 = 1.0;

#[derive(Default)]
struct Recorder {
    key: Option<egui::Key>,
    modifiers: egui::Modifiers,
    changed_at: f64,
}

enum Recording {
    Waiting,
    Cancelled,
    MultipleKeys,
    Complete(String),
}

impl Recorder {
    fn add_modifiers(&mut self, modifiers: egui::Modifiers, now: f64) {
        if self.key.is_none() {
            // Before the ordinary key, preview only what is currently held. A modifier
            // pressed and released while choosing a chord must not become part of it.
            self.modifiers = modifiers;
        } else {
            let combined = self.modifiers | modifiers;
            if combined != self.modifiers {
                self.modifiers = combined;
                self.changed_at = now;
            }
        }
    }

    fn preview(&self) -> Option<String> {
        match self.key {
            Some(key) => Some(format!("{}…", display(&key_name(key, self.modifiers)))),
            None if !self.modifiers.is_none() => Some(format!("{}…", display(&modifier_prefix(self.modifiers)))),
            None => None,
        }
    }

    fn hint(&self, input: &egui::InputState) -> &'static str {
        if self.key.is_none() {
            if self.modifiers.is_none() { "ui.shortcut_capture_press" } else { "ui.shortcut_capture_add_key" }
        } else if !input.keys_down.is_empty() || !input.modifiers.is_none() {
            "ui.shortcut_capture_release"
        } else {
            "ui.shortcut_capture_settling"
        }
    }

    fn update(&mut self, input: &egui::InputState) -> Recording {
        // Cancel on an actual focus loss. The control channel can legitimately open the
        // recorder while the window is already in the background.
        if input.events.iter().any(|event| matches!(event, egui::Event::WindowFocused(false))) {
            return Recording::Cancelled;
        }
        for event in &input.events {
            match event {
                egui::Event::ModifiersChanged(modifiers) => self.add_modifiers(*modifiers, input.time),
                // egui also emits physical left/right modifier key events. They carry
                // modifier state but must never occupy the chord's ordinary-key slot.
                egui::Event::Key {
                    key:
                        egui::Key::ShiftLeft
                        | egui::Key::ShiftRight
                        | egui::Key::ControlLeft
                        | egui::Key::ControlRight
                        | egui::Key::AltLeft
                        | egui::Key::AltRight
                        | egui::Key::SuperLeft
                        | egui::Key::SuperRight,
                    modifiers,
                    ..
                } => self.add_modifiers(*modifiers, input.time),
                egui::Event::Key { key, pressed, repeat, modifiers, .. } => {
                    if *pressed && *key == egui::Key::Escape {
                        return Recording::Cancelled;
                    }
                    if *pressed && !repeat {
                        if self.key.is_some_and(|previous| previous != *key) {
                            return Recording::MultipleKeys;
                        }
                        if self.key.is_none() {
                            self.key = Some(*key);
                            self.modifiers = *modifiers;
                            self.changed_at = input.time;
                        }
                    }
                    self.add_modifiers(*modifiers, input.time);
                }
                _ => {}
            }
        }
        self.add_modifiers(input.modifiers, input.time);
        if let Some(key) = self.key
            && input.keys_down.is_empty()
            && input.modifiers.is_none()
            && input.time - self.changed_at >= RECORD_SETTLE_SECONDS
        {
            return Recording::Complete(key_name(key, self.modifiers));
        }
        Recording::Waiting
    }
}

impl Keymap {
    pub fn prefs(&self) -> Value {
        json!(self.custom)
    }
    pub fn load(&mut self, v: &Value) {
        self.custom =
            v.as_object().map(|o| o.iter().take(2000).filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
    }
}

/// A key press as a shortcut name: `E`, `Shift+E`, `Ctrl+Alt+K`, `F5`, `Delete`.
pub fn key_name(key: egui::Key, mods: egui::Modifiers) -> String {
    let mut s = modifier_prefix(mods);
    s.push_str(key.name());
    s
}

fn modifier_prefix(mods: egui::Modifiers) -> String {
    let mut s = String::new();
    if mods.command || mods.ctrl {
        s.push_str("Ctrl+");
    }
    // Keep the historical Ctrl/Cmd alias for existing bindings, but don't discard a
    // physical Control pressed together with macOS Command.
    if mods.mac_cmd && mods.ctrl {
        s.push_str("Cmd+");
    }
    if mods.alt {
        s.push_str("Alt+");
    }
    if mods.shift {
        s.push_str("Shift+");
    }
    s
}

/// The key a command runs from now.
pub fn effective(app: &SolveApp, id: &str) -> Option<String> {
    match app.keymap.custom.get(id) {
        Some(k) if k.is_empty() => None,
        Some(k) => Some(k.clone()),
        None => solidcraft_engine::find_command(id).and_then(|c| c.shortcut).map(str::to_string),
    }
}

/// Presentation only: stored Ctrl bindings retain their historical primary-modifier alias.
/// A recorded macOS Control+Command chord includes both names explicitly.
pub fn display(key: &str) -> String {
    display_for_platform(key, cfg!(target_os = "macos"))
}

fn display_for_platform(key: &str, mac: bool) -> String {
    let both = key.starts_with("Ctrl+Cmd+");
    key.split('+')
        .map(|part| match (mac, part) {
            (true, "Ctrl") if !both => "Cmd",
            (true, "Alt") => "Option",
            _ => part,
        })
        .collect::<Vec<_>>()
        .join("+")
}

pub fn display_for(app: &SolveApp, id: &str) -> Option<String> {
    effective(app, id).map(|key| display(&key))
}

/// Commands bound to `key` now.
pub fn commands_for(app: &SolveApp, key: &str) -> Vec<&'static str> {
    command_specs().into_iter().filter(|c| effective(app, c.id).as_deref() == Some(key)).map(|c| c.id).collect()
}

/// The enabled command a key press runs, if any.
pub fn command_for(app: &SolveApp, key: &str) -> Option<&'static str> {
    commands_for(app, key).into_iter().find(|id| solidcraft_engine::find_command(id).is_some_and(|c| c.info(&app.session).enabled))
}

/// Run a shortcut key: its enabled command; outside a sketch, a sketch tool's key opens Create
/// Sketch (on the selected plane or planar face, else on the plane picked next) and the tool
/// starts in the new sketch.
pub fn press(app: &mut SolveApp, key: &str) {
    if let Some(id) = command_for(app, key) {
        app.start(id);
    } else if let Some(id) = sketch_tool_for(app, key) {
        app.start("sketch.create");
        if let Some(d) = app.dialog.as_mut().filter(|d| matches!(d.kind, crate::dialogs::Kind::Sketch)) {
            d.then_start = Some(id);
        }
    }
}

/// Outside a sketch: the SKETCH-tab command bound to `key` that a new sketch would start. Not
/// Normal/Construction (a mode of the drawing in progress) nor the Finish Sketch panel.
fn sketch_tool_for(app: &SolveApp, key: &str) -> Option<&'static str> {
    if app.session.active_sketch.is_some() {
        return None;
    }
    commands_for(app, key).into_iter().find(|id| {
        solidcraft_engine::find_command(id).is_some_and(|c| c.tab == "SKETCH" && c.panel != "FINISH SKETCH" && c.id != "sketch.construction")
    })
}

/// Bind `key` to a command (`""` clears it). Refused for reserved keys, and when other commands
/// use the key unless `replace` (they lose it).
pub fn bind(app: &mut SolveApp, id: &str, key: &str, replace: bool) -> Result<(), String> {
    let spec = solidcraft_engine::find_command(id).ok_or_else(|| format!("no command `{id}`"))?;
    if RESERVED.contains(&key) {
        return Err(format!("{key} is used by SolidCraft itself"));
    }
    if !key.is_empty() {
        let others: Vec<&'static str> = commands_for(app, key).into_iter().filter(|o| *o != id).collect();
        if !others.is_empty() && !replace {
            let names: Vec<&str> = others.iter().filter_map(|o| solidcraft_engine::find_command(o).map(|c| c.label)).collect();
            return Err(format!("{key} is already used by {}", names.join(", ")));
        }
        for o in others {
            app.keymap.custom.insert(o.to_string(), String::new());
        }
    }
    if spec.shortcut.unwrap_or("") == key {
        app.keymap.custom.remove(id);
    } else {
        app.keymap.custom.insert(id.to_string(), key.to_string());
    }
    Ok(())
}

pub fn reset(app: &mut SolveApp, id: Option<&str>) {
    match id {
        Some(id) => drop(app.keymap.custom.remove(id)),
        None => app.keymap.custom.clear(),
    }
}

/// The Keyboard Shortcuts page of Preferences: search, rebind (click, then press the key), clear,
/// reset; a key another command uses is reported and taken over only on request.
pub fn page(app: &mut SolveApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    // Keep a pending chord until it has settled and all keys are released. Modifier-only
    // input never starts the timer, so modifiers can be assembled at the user's own pace.
    if let Some(id) = app.keymap.capturing.clone() {
        match ctx.input(|i| app.keymap.recorder.update(i)) {
            Recording::Waiting => {
                if app.keymap.recorder.key.is_some() {
                    let remaining = ctx.input(|i| RECORD_SETTLE_SECONDS - (i.time - app.keymap.recorder.changed_at));
                    if remaining > 0.0
                        && let Ok(delay) = std::time::Duration::try_from_secs_f64(remaining)
                    {
                        ctx.request_repaint_after(delay);
                    }
                }
            }
            Recording::Cancelled => {
                app.keymap.capturing = None;
                // Escape cancels recording first, without also closing Preferences.
                ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { key: egui::Key::Escape, .. })));
            }
            Recording::MultipleKeys => {
                app.keymap.capturing = None;
                app.keymap.message = Some(app.tr("ui.shortcut_multiple_keys").to_string());
            }
            Recording::Complete(name) => {
                app.keymap.capturing = None;
                match bind(app, &id, &name, false) {
                    Ok(()) => app.keymap.message = None,
                    Err(e) if e.contains("already used") => {
                        app.keymap.conflict = Some((id.clone(), name.clone(), commands_for(app, &name).iter().map(|s| s.to_string()).collect()));
                        app.keymap.message = Some(e);
                    }
                    Err(e) => app.keymap.message = Some(e),
                }
            }
        }
        // The recorder owns these events. In particular, Enter/Space must not activate
        // the focused binding button again or type into the filter while still pending.
        ctx.input_mut(|i| {
            i.events.retain(|event| !matches!(event, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_)));
        });
    }
    let t = Tokens::get();
    let mut action: Option<(String, &str)> = None;
    {
        {
            ui.horizontal(|ui| {
                let filter = ui.add(
                    egui::TextEdit::singleline(&mut app.keymap.filter)
                        .hint_text(crate::i18n::tr_label("Filter commands or keys"))
                        .desired_width(300.0),
                );
                crate::scenario::publish_handle("shortcut:filter", filter.rect.center());
                if filter.clicked() {
                    action = Some((String::new(), "cancel"));
                }
                if !app.keymap.read_only && ui.button(app.tr_label("Reset All")).clicked() {
                    action = Some((String::new(), "reset_all"));
                }
                if ui.button(app.tr_label("Export…")).on_hover_text(app.tr_label("Keys, S box favourites and the toolbar, as a file")).clicked() {
                    action = Some((String::new(), "export"));
                }
                if !app.keymap.read_only && ui.button(app.tr_label("Import…")).clicked() {
                    action = Some((String::new(), "import"));
                }
            });
            if let Some(m) = &app.keymap.message {
                ui.label(RichText::new(m).color(t.warning));
            }
            if let Some((id, key, _)) = app.keymap.conflict.clone() {
                ui.horizontal(|ui| {
                    if ui.button(format!("Use {key} here anyway")).clicked() {
                        action = Some((format!("{id}\n{key}"), "replace"));
                    }
                    if ui.button(app.tr_label("Cancel")).clicked() {
                        action = Some((String::new(), "cancel"));
                    }
                });
            }
            ui.label(
                RichText::new(
                    app.tr_label("Select Other: hold the left button still on the model (or right-click › Select Other…) to list everything under the cursor; click again on the same spot to select the next thing there. Double-click the middle button to go Home (Preferences › Navigation)."),
                )
                .size(11.0)
                .color(t.text_dim),
            );
            if !app.keymap.read_only {
                ui.label(RichText::new(app.tr("ui.shortcut_capture_hint")).size(11.0).color(t.text_dim));
                if app.keymap.capturing.is_some() {
                    let hint = ctx.input(|input| app.keymap.recorder.hint(input));
                    ui.label(RichText::new(app.tr(hint)).color(t.accent));
                }
            }
            ui.separator();
            let q = app.keymap.filter.to_lowercase();
            egui::ScrollArea::both().max_height(330.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("sc_keymap_grid").num_columns(4).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                    for c in command_specs() {
                        let key = effective(app, c.id).unwrap_or_default();
                        let translated_label = app.tr_label(c.label);
                        if !q.is_empty() && ![c.label, c.id, key.as_str(), translated_label].iter().any(|s| s.to_lowercase().contains(&q)) {
                            continue;
                        }
                        ui.label(translated_label);
                        ui.label(RichText::new(c.id).size(10.5).color(t.text_dim));
                        let capturing = app.keymap.capturing.as_deref() == Some(c.id);
                        let changed = app.keymap.custom.contains_key(c.id);
                        let text = if capturing {
                            app.keymap.recorder.preview().unwrap_or_else(|| app.tr_label("press a key…").to_string())
                        } else if key.is_empty() {
                            "—".to_string()
                        } else {
                            display(&key)
                        };
                        if app.keymap.read_only {
                            ui.label(RichText::new(text).strong());
                            ui.end_row();
                            continue;
                        }
                        let mut b = egui::Button::new(RichText::new(text).color(if capturing || changed { t.accent } else { t.text }))
                            .min_size(egui::vec2(80.0, 0.0));
                        if capturing {
                            b = b.fill(t.accent_soft).stroke(egui::Stroke::new(1.0, t.accent));
                        }
                        let response = ui.add(b).on_hover_text(app.tr("ui.shortcut_capture_hint"));
                        if ui.is_rect_visible(response.rect) {
                            crate::scenario::publish_handle(&format!("shortcut:{}", c.id), response.rect.center());
                        }
                        if response.clicked() {
                            action = Some((c.id.to_string(), "capture"));
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!key.is_empty(), egui::Button::new(app.tr_label("Clear")).small()).clicked() {
                                action = Some((c.id.to_string(), "clear"));
                            }
                            if ui.add_enabled(changed, egui::Button::new(app.tr_label("Reset")).small()).clicked() {
                                action = Some((c.id.to_string(), "reset"));
                            }
                        });
                        ui.end_row();
                    }
                });
            });
        }
    }
    crate::scenario::publish_count("shortcut_recording", f64::from(app.keymap.capturing.is_some()));
    if action.is_some() {
        app.keymap.capturing = None;
        app.keymap.recorder = Recorder::default();
    }
    match action {
        Some((id, "capture")) => {
            app.keymap.capturing = Some(id);
            app.keymap.recorder = Recorder::default();
            app.keymap.conflict = None;
            app.keymap.message = None;
        }
        Some((id, "clear")) => drop(bind(app, &id, "", false)),
        Some((id, "reset")) => reset(app, Some(&id)),
        Some((_, "reset_all")) => reset(app, None),
        Some((_, "export")) => {
            let text = crate::toolbar_custom::export(app).to_string();
            app.pick_save("SolidCraft shortcuts.json", &["json"], move |app, path| {
                if let Some(path) = path {
                    app.keymap.message = std::fs::write(&path, text).err().map(|e| format!("export: {e}"));
                }
            });
        }
        Some((_, "import")) => {
            app.pick_open(|app, path| {
                let Some(path) = path else { return };
                let r = std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
                    .and_then(|v| crate::toolbar_custom::import(app, &v));
                app.keymap.message = r.err().map(|e| format!("import: {e}"));
            });
        }
        Some((pair, "replace")) => {
            if let Some((id, key)) = pair.split_once('\n') {
                let _ = bind(app, id, key, true);
            }
            app.keymap.conflict = None;
            app.keymap.message = None;
        }
        Some((_, "cancel")) => {
            app.keymap.conflict = None;
            app.keymap.message = None;
        }
        _ => {}
    }
}

/// Stop waiting for a key (the dialog closed).
pub fn stop_capture(app: &mut SolveApp) {
    app.keymap.capturing = None;
    app.keymap.recorder = Recorder::default();
    app.keymap.conflict = None;
    app.keymap.message = None;
}

/// Is the page waiting for a key (so the key must not also run a command)?
pub fn capturing(app: &SolveApp) -> bool {
    app.prefs_window.open && app.prefs_window.section == crate::prefs::PAGE_SHORTCUTS && app.keymap.capturing.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> SolveApp {
        SolveApp::new(solidcraft_engine::Session::default(), Default::default())
    }

    fn recorder_frame(a: &mut SolveApp, ctx: &egui::Context, time: f64, events: Vec<egui::Event>) {
        let raw = egui::RawInput { time: Some(time), events, ..Default::default() };
        let _ = ctx.run_ui(raw, |ui| page(a, ui));
    }

    #[test]
    fn recorder_waits_for_late_modifiers_instead_of_binding_the_first_key_down() {
        let mut a = app();
        let ctx = egui::Context::default();
        a.keymap.capturing = Some("solid.chamfer".into());
        recorder_frame(
            &mut a,
            &ctx,
            0.0,
            vec![egui::Event::Key { key: egui::Key::M, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
        );
        assert!(a.keymap.capturing.is_some(), "the first key-down must remain a preview");
        assert_eq!(effective(&a, "solid.chamfer"), None);
    }

    fn event(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
    }

    #[test]
    fn control_input_preserves_primary_alias_and_explicit_physical_modifiers() {
        for (step, ctrl, mac_cmd) in [
            (json!({"key":"F12","pressed":true,"cmd":true}), true, false),
            (json!({"key":"F12","pressed":true,"cmd":true,"ctrl":false}), false, true),
            (json!({"key":"F12","pressed":true,"cmd":true,"ctrl":true}), true, true),
            (json!({"key":"F12","pressed":true,"ctrl":true}), true, false),
        ] {
            let mut h = crate::scenario::Harness::new();
            h.step(&step).unwrap();
            let modifiers = h.ctx.input(|i| i.modifiers);
            assert_eq!(modifiers.ctrl, ctrl, "{step}");
            assert_eq!(modifiers.mac_cmd, mac_cmd, "{step}");
            assert!(modifiers.command, "{step}");
        }
    }

    #[test]
    fn recorder_previews_each_modifier_and_explains_the_next_step() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.prefs", json!({"open":"Keyboard Shortcuts"}));
        h.step(&json!({"click":{"handle":"shortcut:filter"}})).unwrap();
        h.call("ui.text", json!({"text":"solid.chamfer"}));
        h.step(&json!({"click":{"handle":"shortcut:solid.chamfer"}})).unwrap();
        h.call("ui.key", json!({"key":"ShiftLeft","pressed":true,"shift":true}));
        assert!(h.widget("Shift+…").is_some(), "show Shift immediately, before any ordinary key");
        assert!(h.widget("Add one ordinary key. Escape cancels.").is_some());
        h.call("ui.key", json!({"key":"SuperLeft","pressed":true,"shift":true,"cmd":true,"ctrl":false}));
        let preview = if cfg!(target_os = "macos") { "Cmd+Shift+…" } else { "Ctrl+Shift+…" };
        assert!(h.widget(preview).is_some(), "show each newly held modifier");
        h.call("ui.key", json!({"key":"SuperLeft","pressed":false,"shift":true}));
        assert!(h.widget("Shift+…").is_some(), "remove released modifiers before an ordinary key");
        h.call("ui.key", json!({"key":"ShiftLeft","pressed":false}));
        assert!(h.widget("press a key…").is_some());
        assert_eq!(effective(&h.app, "solid.chamfer"), None);
        h.call("ui.key", json!({"key":"ShiftRight","pressed":true,"shift":true}));
        h.call("ui.key", json!({"key":"C","pressed":true,"shift":true}));
        assert!(h.widget("Shift+C…").is_some());
        assert!(h.widget("Release all keys to confirm. Escape cancels.").is_some());
        h.call("ui.key", json!({"key":"C","pressed":false,"shift":true}));
        h.call("ui.key", json!({"key":"ShiftRight","pressed":false}));
        assert!(h.widget("Confirming shortcut… Escape cancels.").is_some());
        assert!(h.widget("Shift+C…").is_some(), "retain the proposed chord until confirmed");
        assert_eq!(effective(&h.app, "solid.chamfer"), None);
        h.frames(65);
        assert_eq!(effective(&h.app, "solid.chamfer").as_deref(), Some("Shift+C"));
        assert!(h.widget("Shift+C").is_some());
    }

    #[test]
    fn physical_modifier_keys_are_not_the_ordinary_key_in_a_chord() {
        use egui::Key;
        for (key, mods, expected) in [
            (Key::ShiftLeft, egui::Modifiers::SHIFT, "Shift+C"),
            (Key::ShiftRight, egui::Modifiers::SHIFT, "Shift+C"),
            (Key::ControlLeft, egui::Modifiers::CTRL, "Ctrl+C"),
            (Key::ControlRight, egui::Modifiers::CTRL, "Ctrl+C"),
            (Key::AltLeft, egui::Modifiers::ALT, "Alt+C"),
            (Key::AltRight, egui::Modifiers::ALT, "Alt+C"),
            (Key::SuperLeft, egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND, "Ctrl+C"),
            (Key::SuperRight, egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND, "Ctrl+C"),
        ] {
            let ctx = egui::Context::default();
            let mut recorder = Recorder::default();
            for (time, events, complete) in [
                (0.0, vec![egui::Event::ModifiersChanged(mods), event(key, true, mods)], false),
                (5.0, vec![], false),
                (5.1, vec![event(Key::C, true, mods)], false),
                (5.2, vec![event(Key::C, false, mods)], false),
                (5.3, vec![event(key, false, egui::Modifiers::NONE), egui::Event::ModifiersChanged(egui::Modifiers::NONE)], false),
                (6.2, vec![], true),
            ] {
                let raw = egui::RawInput { time: Some(time), events, ..Default::default() };
                let _ = ctx.run_ui(raw, |ui| {
                    let result = ui.input(|input| recorder.update(input));
                    if complete {
                        assert!(matches!(result, Recording::Complete(ref binding) if binding == expected), "{key:?}");
                    } else {
                        assert!(matches!(result, Recording::Waiting), "{key:?} at {time}");
                    }
                });
                if time <= 5.0 {
                    assert!(recorder.key.is_none(), "{key:?} must not be recorded as an ordinary key");
                }
            }
        }
    }

    #[test]
    fn recorder_accumulates_late_modifiers_and_settles_for_one_second() {
        let mut a = app();
        let ctx = egui::Context::default();
        a.keymap.capturing = Some("sketch.mirror".into());
        recorder_frame(&mut a, &ctx, 0.0, vec![event(egui::Key::M, true, egui::Modifiers::NONE)]);
        recorder_frame(&mut a, &ctx, 0.4, vec![egui::Event::ModifiersChanged(egui::Modifiers::SHIFT)]);
        let mods = egui::Modifiers { shift: true, mac_cmd: true, command: true, ..Default::default() };
        recorder_frame(&mut a, &ctx, 0.8, vec![egui::Event::ModifiersChanged(mods)]);
        recorder_frame(&mut a, &ctx, 1.0, vec![event(egui::Key::M, false, mods), egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
        recorder_frame(&mut a, &ctx, 1.79, vec![]);
        assert!(a.keymap.capturing.is_some());
        assert_eq!(effective(&a, "sketch.mirror"), None);
        recorder_frame(&mut a, &ctx, 1.81, vec![]);
        assert!(a.keymap.capturing.is_none());
        assert_eq!(effective(&a, "sketch.mirror").as_deref(), Some("Ctrl+Shift+M"));
        let mut reloaded = app();
        reloaded.keymap.load(&a.keymap.prefs());
        assert_eq!(effective(&reloaded, "sketch.mirror"), effective(&a, "sketch.mirror"));
    }

    #[test]
    fn recorder_waits_indefinitely_for_modifier_only_input_and_for_release() {
        let mut a = app();
        let ctx = egui::Context::default();
        a.keymap.capturing = Some("sketch.mirror".into());
        let mods = egui::Modifiers { shift: true, ctrl: true, alt: true, mac_cmd: true, command: true };
        recorder_frame(&mut a, &ctx, 0.0, vec![egui::Event::ModifiersChanged(mods)]);
        recorder_frame(&mut a, &ctx, 10.0, vec![]);
        assert!(a.keymap.capturing.is_some());
        assert_eq!(effective(&a, "sketch.mirror"), None);
        recorder_frame(&mut a, &ctx, 12.0, vec![event(egui::Key::M, true, mods)]);
        let mut repeat = event(egui::Key::M, true, mods);
        if let egui::Event::Key { repeat, .. } = &mut repeat {
            *repeat = true;
        }
        recorder_frame(&mut a, &ctx, 14.0, vec![repeat]);
        assert!(a.keymap.capturing.is_some(), "held keys must not confirm");
        recorder_frame(&mut a, &ctx, 14.1, vec![event(egui::Key::M, false, mods)]);
        assert!(a.keymap.capturing.is_some(), "held modifiers must not confirm");
        recorder_frame(&mut a, &ctx, 14.2, vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
        assert_eq!(effective(&a, "sketch.mirror").as_deref(), Some("Ctrl+Cmd+Alt+Shift+M"));
    }

    #[test]
    fn recorder_rejects_two_ordinary_keys_and_escape_preserves_the_old_binding() {
        for second in [egui::Key::N, egui::Key::Escape] {
            let mut a = app();
            let ctx = egui::Context::default();
            a.keymap.capturing = Some("solid.extrude".into());
            recorder_frame(&mut a, &ctx, 0.0, vec![event(egui::Key::M, true, egui::Modifiers::SHIFT)]);
            recorder_frame(&mut a, &ctx, 0.5, vec![event(second, true, egui::Modifiers::SHIFT)]);
            assert!(a.keymap.capturing.is_none());
            assert_eq!(effective(&a, "solid.extrude").as_deref(), Some("E"));
            assert_eq!(a.keymap.message.is_some(), second != egui::Key::Escape);
        }
    }

    #[test]
    fn recorder_conflicts_only_after_settling_and_plain_keys_still_work() {
        let mut a = app();
        let ctx = egui::Context::default();
        a.keymap.capturing = Some("solid.chamfer".into());
        recorder_frame(&mut a, &ctx, 0.0, vec![event(egui::Key::M, true, egui::Modifiers::NONE), event(egui::Key::M, false, egui::Modifiers::NONE)]);
        assert!(a.keymap.conflict.is_none());
        recorder_frame(&mut a, &ctx, 1.1, vec![]);
        assert_eq!(a.keymap.conflict.as_ref().map(|(_, key, _)| key.as_str()), Some("M"));
        assert_eq!(effective(&a, "solid.move").as_deref(), Some("M"));
        assert_eq!(effective(&a, "solid.chamfer"), None);
        stop_capture(&mut a);
        a.keymap.capturing = Some("solid.extrude".into());
        recorder_frame(&mut a, &ctx, 2.0, vec![event(egui::Key::K, true, egui::Modifiers::NONE), event(egui::Key::K, false, egui::Modifiers::NONE)]);
        recorder_frame(&mut a, &ctx, 3.1, vec![]);
        assert_eq!(effective(&a, "solid.extrude").as_deref(), Some("K"));
    }

    #[test]
    fn recorder_cancels_on_focus_loss_and_dialog_close() {
        let mut a = app();
        let ctx = egui::Context::default();
        a.keymap.capturing = Some("solid.extrude".into());
        recorder_frame(&mut a, &ctx, 0.0, vec![event(egui::Key::K, true, egui::Modifiers::SHIFT)]);
        let raw = egui::RawInput { time: Some(0.5), focused: false, events: vec![egui::Event::WindowFocused(false)], ..Default::default() };
        let _ = ctx.run_ui(raw, |ui| page(&mut a, ui));
        assert!(a.keymap.capturing.is_none());
        assert_eq!(effective(&a, "solid.extrude").as_deref(), Some("E"));
        stop_capture(&mut a);
        assert!(a.keymap.recorder.key.is_none());
    }

    #[test]
    fn escape_cancels_only_recording_and_no_command_fires_during_capture() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.prefs", json!({"open": "Keyboard Shortcuts"}));
        h.app.keymap.capturing = Some("solid.chamfer".into());
        h.call("ui.key", json!({"key": "C", "pressed": true}));
        h.frames(2);
        assert!(h.app.tool.is_none() && h.app.dialog.is_none());
        assert!(h.app.session.active_sketch.is_none(), "C must not start a sketch while recording");
        h.call("ui.key", json!({"key": "Escape"}));
        h.frames(3);
        assert!(h.app.prefs_window.open, "Escape should cancel the recorder, not close Preferences");
        assert!(h.app.keymap.capturing.is_none());
        assert_eq!(effective(&h.app, "solid.chamfer"), None);
    }

    #[test]
    fn a_background_control_channel_recording_waits_for_an_actual_focus_loss() {
        let mut a = app();
        let ctx = egui::Context::default();
        a.keymap.capturing = Some("sketch.mirror".into());
        let raw =
            egui::RawInput { time: Some(0.0), focused: false, events: vec![event(egui::Key::M, true, egui::Modifiers::SHIFT)], ..Default::default() };
        let _ = ctx.run_ui(raw, |ui| page(&mut a, ui));
        assert!(a.keymap.capturing.is_some());
        assert_eq!(effective(&a, "sketch.mirror"), None);
    }

    #[test]
    fn enter_and_space_are_recorded_without_reactivating_the_focused_button() {
        for key in ["Enter", "Space"] {
            let mut h = crate::scenario::Harness::new();
            h.call("ui.prefs", json!({"open": "Keyboard Shortcuts"}));
            h.step(&json!({"click":{"handle":"shortcut:filter"}})).unwrap();
            h.call("ui.text", json!({"text":"sketch.mirror"}));
            h.step(&json!({"click":{"handle":"shortcut:sketch.mirror"}})).unwrap();
            h.call("ui.key", json!({"key":key}));
            assert!(h.app.keymap.capturing.is_some(), "{key} should still be pending");
            h.frames(65);
            assert_eq!(effective(&h.app, "sketch.mirror").as_deref(), Some(key));
            assert!(h.app.prefs_window.open);
        }
    }

    #[test]
    fn chord_display_and_legacy_primary_modifier_bindings_are_compatible() {
        assert_eq!(display_for_platform("Ctrl+Alt+Shift+M", true), "Cmd+Option+Shift+M");
        assert_eq!(display_for_platform("Ctrl+Cmd+Alt+Shift+M", true), "Ctrl+Cmd+Option+Shift+M");
        assert_eq!(display_for_platform("Ctrl+Alt+Shift+M", false), "Ctrl+Alt+Shift+M");
        let mut a = app();
        a.keymap.load(&json!({"solid.chamfer":"Shift+C", "solid.extrude":"Ctrl+Alt+E"}));
        assert_eq!(effective(&a, "solid.chamfer").as_deref(), Some("Shift+C"));
        let mods = egui::Modifiers { mac_cmd: true, command: true, alt: true, ..Default::default() };
        assert_eq!(commands_for(&a, &key_name(egui::Key::E, mods)), vec!["solid.extrude"]);
        let extra = egui::Modifiers { shift: true, ..mods };
        assert!(commands_for(&a, &key_name(egui::Key::E, extra)).is_empty());
        let both = egui::Modifiers { ctrl: true, ..extra };
        bind(&mut a, "sketch.mirror", &key_name(egui::Key::M, both), false).unwrap();
        assert_eq!(commands_for(&a, &key_name(egui::Key::M, both)), vec!["sketch.mirror"]);
        assert!(commands_for(&a, &key_name(egui::Key::M, extra)).is_empty());
    }

    #[test]
    fn rebinding_detects_conflicts_and_resets() {
        let mut a = app();
        assert_eq!(effective(&a, "solid.extrude").as_deref(), Some("E"));
        // Shift+E is free.
        bind(&mut a, "solid.extrude", "Shift+E", false).unwrap();
        assert_eq!(commands_for(&a, "Shift+E"), vec!["solid.extrude"]);
        assert!(commands_for(&a, "E").is_empty());
        // M belongs to Move/Copy: refused, then taken over on request.
        let e = bind(&mut a, "solid.revolve", "M", false).unwrap_err();
        assert!(e.contains("already used by Move/Copy"), "{e}");
        bind(&mut a, "solid.revolve", "M", true).unwrap();
        assert_eq!(commands_for(&a, "M"), vec!["solid.revolve"]);
        assert_eq!(effective(&a, "solid.move"), None);
        assert!(bind(&mut a, "solid.revolve", "Ctrl+Z", false).is_err(), "reserved");
        // Kept between runs.
        let mut b = app();
        b.keymap.load(&a.keymap.prefs());
        assert_eq!(effective(&b, "solid.revolve").as_deref(), Some("M"));
        reset(&mut b, None);
        assert_eq!(effective(&b, "solid.move").as_deref(), Some("M"));
        assert_eq!(key_name(egui::Key::K, egui::Modifiers { ctrl: true, shift: true, ..Default::default() }), "Ctrl+Shift+K");
    }
}

#[cfg(test)]
#[path = "sketch_shortcut_tests.rs"]
mod sketch_shortcut_tests;
