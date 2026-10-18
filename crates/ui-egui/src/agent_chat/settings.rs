//! Settings → AI: the master switch, the agents (found and added, each with Test, Default and,
//! for added ones, Remove), permissions (global and per agent, and the "always allow" answers),
//! and the panel. Plus the Add agent dialog: an installed ACP agent, a custom command, or an
//! OpenAI-compatible endpoint (whose key goes to the system keychain).

use egui::{RichText, Stroke};

use super::agents::{self, AgentConfig, Kind, Mode};
use super::*;
use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddKind {
    #[default]
    Installed,
    Custom,
    Api,
}

/// The Add agent dialog's fields.
#[derive(Default)]
pub struct AddState {
    pub kind: AddKind,
    pub name: String,
    pub command: String,
    pub installed: usize,
    pub preset: usize,
    pub base_url: String,
    pub model: String,
    pub key: String,
    pub error: Option<String>,
    test: Option<Test>,
}

/// A connection test running or done.
pub enum Test {
    Running(std::sync::mpsc::Receiver<Result<String, String>>),
    Done(Result<String, String>),
}

impl Test {
    fn poll(&mut self) {
        if let Test::Running(rx) = self
            && let Ok(r) = rx.try_recv()
        {
            *self = Test::Done(r);
        }
    }
}

/// Settings state that isn't preferences: the dialog and the tests.
#[derive(Default)]
pub struct State {
    pub add: Option<AddState>,
    pub tests: std::collections::HashMap<String, Test>,
}

/// Open the Add agent dialog.
pub fn open_add(app: &mut SolveApp) {
    let preset = agents::API_PRESETS.first();
    app.agent_chat.settings.add = Some(AddState {
        base_url: preset.map(|p| p.1.to_string()).unwrap_or_default(),
        name: preset.map(|p| p.0.to_string()).unwrap_or_default(),
        ..Default::default()
    });
}

/// Run a connection test in the background (it starts the agent briefly, or asks the endpoint
/// for its models).
#[cfg(not(target_arch = "wasm32"))]
fn start_test(app: &SolveApp, e: &agents::Entry, key: Option<String>) -> Test {
    let (tx, rx) = std::sync::mpsc::channel();
    match e.kind {
        Kind::Acp => {
            let argv = e.argv.clone();
            std::thread::spawn(move || {
                let _ = tx.send(super::acp_client::test_agent(&argv, &working_dir()));
            });
        }
        Kind::Api => {
            let Some(http) = app.services.agent_http.clone() else { return Test::Done(Err(tr("ui.ai_no_http").to_string())) };
            let (url, model) = (e.base_url.clone(), e.model.clone());
            std::thread::spawn(move || {
                let _ = tx.send(super::api_client::test_endpoint(&http, &url, key.as_deref(), &model));
            });
        }
    }
    Test::Running(rx)
}

fn test_line(ui: &mut egui::Ui, t: &Option<&Test>) {
    let tk = Tokens::get();
    match t {
        Some(Test::Running(_)) => {
            ui.label(RichText::new(tr("ui.ai_testing")).color(tk.text_dim).size(11.0));
        }
        Some(Test::Done(Ok(m))) => {
            ui.label(RichText::new(m.clone()).color(tk.accent).size(11.0));
        }
        Some(Test::Done(Err(e))) => {
            ui.add(egui::Label::new(RichText::new(e).color(tk.error).size(11.0)).wrap());
        }
        None => {}
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong().size(13.0));
    ui.separator();
}

fn note(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get();
    ui.add(egui::Label::new(RichText::new(text).size(11.0).color(t.text_dim)).wrap());
}

/// The AI page of Settings.
pub fn page(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let r = ui.checkbox(&mut app.preferences.ai_features, RichText::new(tr("ui.ai_show_features")).strong());
    crate::scenario::publish_handle("prefs:ai_features", r.rect.center());
    note(ui, tr("ui.ai_show_features_note"));
    if cfg!(target_arch = "wasm32") {
        note(ui, tr("ui.agent_chat_desktop_only"));
        return;
    }
    if !app.preferences.ai_features {
        return;
    }
    ui.add_enabled_ui(true, |ui| {
        let r = ui.checkbox(&mut app.preferences.ai_toolbar_button, tr("ui.ai_toolbar_button"));
        crate::scenario::publish_handle("prefs:ai_toolbar_button", r.rect.center());
    });
    note(ui, tr("ui.ai_toolbar_button_note"));
    #[cfg(not(target_arch = "wasm32"))]
    {
        if app.agent_chat.detection.is_none() && app.agent_chat.detecting.is_none() {
            refresh(app, &ui.ctx().clone());
        }
        agents_section(app, ui);
    }
    section(ui, tr("ui.ai_permissions"));
    for m in Mode::ALL {
        let r = ui.radio_value(&mut app.preferences.ai_permissions, m, m.label());
        crate::scenario::publish_handle(&format!("prefs:ai_mode:{}", m.id()), r.rect.center());
        note(
            ui,
            tr(match m {
                Mode::Ask => "ui.ai_mode_ask_note",
                Mode::Edits => "ui.ai_mode_edits_note",
                Mode::All => "ui.ai_mode_all_note",
            }),
        );
    }
    if app.preferences.ai_permissions == Mode::All {
        egui::Frame::NONE.stroke(Stroke::new(1.0, t.warning)).corner_radius(4.0).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(tr("ui.ai_mode_all_warning")).color(t.warning)).wrap());
        });
    }
    note(ui, tr("ui.ai_modes_scope"));
    // "Always allow" answers.
    let remembered: Vec<(String, Vec<String>)> =
        app.preferences.agent_always_allow.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.clone(), v.clone())).collect();
    ui.add_space(4.0);
    ui.label(RichText::new(tr("ui.ai_always_allowed")).strong());
    if remembered.is_empty() {
        note(ui, tr("ui.ai_always_allowed_none"));
    }
    let names: std::collections::HashMap<String, String> = agents::entries(app).into_iter().map(|e| (e.id, e.name)).collect();
    for (agent, tools) in remembered {
        ui.horizontal_wrapped(|ui| {
            ui.label(names.get(&agent).cloned().unwrap_or_else(|| agent.clone()));
            for tool in &tools {
                ui.label(RichText::new(tool).monospace().size(11.0).color(t.text_dim));
            }
            if ui.small_button(tr("ui.ai_reset")).clicked() {
                app.preferences.agent_always_allow.remove(&agent);
            }
        });
    }
    section(ui, tr("ui.ai_panel"));
    egui::Grid::new("sc_ai_panel_grid").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
        ui.label(tr("ui.agent_chat_side"));
        ui.horizontal(|ui| {
            let p = &mut app.preferences;
            let (left, popped) = (p.agent_chat_side == "left", p.agent_chat_popped);
            if ui.radio(left && !popped, tr("ui.agent_chat_left")).clicked() {
                p.agent_chat_side = "left".into();
                p.agent_chat_popped = false;
            }
            if ui.radio(!left && !popped, tr("ui.agent_chat_right")).clicked() {
                p.agent_chat_side = "right".into();
                p.agent_chat_popped = false;
            }
            #[cfg(not(target_arch = "wasm32"))]
            if ui.radio(popped, tr("ui.agent_chat_own_window")).on_hover_text(tr("ui.agent_chat_pop_out")).clicked() {
                p.agent_chat_popped = true;
            }
        });
        ui.end_row();
        ui.label(tr("ui.agent_chat_turn_limit"));
        ui.add(egui::DragValue::new(&mut app.preferences.agent_chat_turn_limit).range(1..=200));
        ui.end_row();
    });
    ui.add_space(6.0);
    note(ui, tr("ui.agent_chat_privacy_note"));
}

/// The agents: found and added, with Test, Default, a permission override and Remove.
#[cfg(not(target_arch = "wasm32"))]
fn agents_section(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    section(ui, tr("ui.ai_agents"));
    for test in app.agent_chat.settings.tests.values_mut() {
        test.poll();
    }
    let all = agents::entries(app);
    let d = &app.preferences.agent_default;
    let default = if all.iter().any(|e| &e.id == d) { Some(d.clone()) } else { all.first().map(|e| e.id.clone()) };
    if app.agent_chat.detecting.is_some() {
        ui.label(RichText::new(tr("ui.agent_chat_detecting")).color(t.text_dim));
    } else if all.is_empty() {
        note(ui, tr("ui.agent_chat_none_found"));
    }
    let mut remove: Option<String> = None;
    let mut test: Option<agents::Entry> = None;
    for e in &all {
        egui::Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(5.0).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                super::panel::kind_icon(ui, e.kind);
                ui.label(RichText::new(&e.name).strong());
                ui.label(RichText::new(e.detail()).color(t.text_dim).size(11.0));
                super::panel::login_chip(ui, e.login);
                if !e.found {
                    ui.label(RichText::new(tr("ui.ai_added")).color(t.text_dim).size(11.0));
                }
            });
            ui.horizontal(|ui| {
                let is_default = default.as_deref() == Some(e.id.as_str());
                if ui.radio(is_default, tr("ui.ai_default")).clicked() {
                    app.preferences.agent_default = e.id.clone();
                }
                let b = ui.small_button(tr("ui.ai_test"));
                crate::scenario::publish_handle(&format!("prefs:ai_test:{}", e.id), b.rect.center());
                if b.on_hover_text(tr(if e.kind == Kind::Acp { "ui.ai_test_acp_tip" } else { "ui.ai_test_api_tip" })).clicked() {
                    test = Some(e.clone());
                }
                // Its own permission mode, or the global one.
                let cur = app.preferences.agent_permissions.get(&e.id).copied();
                let shown = cur.map_or_else(|| tr("ui.ai_mode_global").to_string(), |m| m.label().to_string());
                let mut pick: Option<Option<Mode>> = None;
                egui::ComboBox::from_id_salt(("sc_ai_mode", &e.id)).width(150.0).selected_text(shown).show_ui(ui, |ui| {
                    if ui.selectable_label(cur.is_none(), tr("ui.ai_mode_global")).clicked() {
                        pick = Some(None);
                    }
                    for m in Mode::ALL {
                        if ui.selectable_label(cur == Some(m), m.label()).clicked() {
                            pick = Some(Some(m));
                        }
                    }
                });
                match pick {
                    Some(Some(m)) => {
                        app.preferences.agent_permissions.insert(e.id.clone(), m);
                    }
                    Some(None) => {
                        app.preferences.agent_permissions.remove(&e.id);
                    }
                    None => {}
                }
                if !e.found && ui.small_button(tr("ui.ai_remove")).clicked() {
                    remove = Some(e.id.clone());
                }
            });
            test_line(ui, &app.agent_chat.settings.tests.get(&e.id));
        });
        ui.add_space(3.0);
    }
    ui.horizontal(|ui| {
        let b = ui.button(tr("ui.ai_add_agent"));
        crate::scenario::publish_handle("prefs:ai_add", b.rect.center());
        if b.clicked() {
            open_add(app);
        }
        if ui.add_enabled(app.agent_chat.detecting.is_none(), egui::Button::new(tr("ui.agent_chat_refresh"))).clicked() {
            refresh(app, &ui.ctx().clone());
        }
    });
    if let Some(e) = test {
        let key = if e.key { app.services.secrets.as_ref().and_then(|s| s.get(&e.id).ok().flatten()) } else { None };
        let started = start_test(app, &e, key);
        app.agent_chat.settings.tests.insert(e.id.clone(), started);
    }
    if let Some(id) = remove {
        app.preferences.agents.retain(|a| a.id != id);
        app.preferences.agent_permissions.remove(&id);
        app.preferences.agent_always_allow.remove(&id);
        if let Some(s) = &app.services.secrets {
            let _ = s.delete(&id);
        }
        if app.agent_chat.agent_id.as_deref() == Some(id.as_str()) {
            app.agent_chat.agent_id = None;
        }
    }
    if app.agent_chat.settings.tests.values().any(|t| matches!(t, Test::Running(_))) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
}

/// The Add agent dialog (drawn every frame while it is open).
pub fn dialog(app: &mut SolveApp, ctx: &egui::Context) {
    if app.agent_chat.settings.add.is_none() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let found: Vec<agents::Entry> = agents::entries(app).into_iter().filter(|e| e.found).collect();
    #[cfg(target_arch = "wasm32")]
    let found: Vec<agents::Entry> = Vec::new();
    let has_keychain = app.services.secrets.is_some();
    let mut open = true;
    let mut add = false;
    let mut test = false;
    let t = Tokens::get();
    egui::Window::new(tr("ui.ai_add_agent_title"))
        .id(egui::Id::new("sc_ai_add"))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .open(&mut open)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let Some(s) = app.agent_chat.settings.add.as_mut() else { return };
            if let Some(test) = s.test.as_mut() {
                test.poll();
            }
            ui.horizontal(|ui| {
                for (k, label) in
                    [(AddKind::Installed, "ui.ai_add_installed"), (AddKind::Custom, "ui.ai_add_custom"), (AddKind::Api, "ui.ai_add_api")]
                {
                    let r = ui.selectable_value(&mut s.kind, k, tr(label));
                    crate::scenario::publish_handle(&format!("ai_add:{label}"), r.rect.center());
                    if r.clicked() {
                        s.error = None;
                        s.test = None;
                        s.command.clear();
                        s.name.clear();
                        if k == AddKind::Api
                            && let Some((name, url, _)) = agents::API_PRESETS.get(s.preset)
                        {
                            s.name = name.to_string();
                            s.base_url = url.to_string();
                        }
                    }
                }
            });
            ui.separator();
            egui::Grid::new("sc_ai_add_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| match s.kind {
                AddKind::Installed => {
                    ui.label(tr("ui.agent_chat_pick"));
                    if found.is_empty() {
                        ui.label(RichText::new(tr("ui.agent_chat_none_found_short")).color(t.text_dim));
                    } else {
                        let sel = found.get(s.installed).map(|e| e.name.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("sc_ai_add_installed").width(260.0).selected_text(sel).show_ui(ui, |ui| {
                            for (i, e) in found.iter().enumerate() {
                                if ui.selectable_value(&mut s.installed, i, format!("{}  {}", e.name, e.detail())).clicked() {
                                    s.command = e.argv.iter().map(|a| super::panel::quote(a)).collect::<Vec<_>>().join(" ");
                                    s.name = format!("{} ({})", e.name, tr("ui.ai_copy_suffix"));
                                }
                            }
                        });
                    }
                    ui.end_row();
                    if s.command.is_empty()
                        && let Some(e) = found.get(s.installed)
                    {
                        s.command = e.argv.iter().map(|a| super::panel::quote(a)).collect::<Vec<_>>().join(" ");
                        s.name = format!("{} ({})", e.name, tr("ui.ai_copy_suffix"));
                    }
                    ui.label(tr("ui.ai_name"));
                    ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(300.0));
                    ui.end_row();
                    ui.label(tr("ui.agent_chat_command"));
                    ui.add(egui::TextEdit::singleline(&mut s.command).desired_width(300.0));
                    ui.end_row();
                    ui.label("");
                    ui.add(egui::Label::new(RichText::new(tr("ui.ai_add_installed_note")).size(11.0).color(t.text_dim)).wrap());
                    ui.end_row();
                }
                AddKind::Custom => {
                    ui.label(tr("ui.ai_name"));
                    ui.add(egui::TextEdit::singleline(&mut s.name).hint_text("My agent").desired_width(300.0));
                    ui.end_row();
                    ui.label(tr("ui.agent_chat_command"));
                    ui.add(egui::TextEdit::singleline(&mut s.command).hint_text("my-agent --acp").desired_width(300.0));
                    ui.end_row();
                    ui.label("");
                    ui.add(egui::Label::new(RichText::new(tr("ui.agent_chat_custom_hint")).size(11.0).color(t.text_dim)).wrap());
                    ui.end_row();
                }
                AddKind::Api => {
                    ui.label(tr("ui.ai_preset"));
                    let presets: Vec<String> =
                        agents::API_PRESETS.iter().map(|p| p.0.to_string()).chain(std::iter::once(tr("ui.ai_custom_url").to_string())).collect();
                    let sel = presets.get(s.preset).cloned().unwrap_or_default();
                    egui::ComboBox::from_id_salt("sc_ai_add_preset").width(200.0).selected_text(sel).show_ui(ui, |ui| {
                        for (i, p) in presets.iter().enumerate() {
                            if ui.selectable_value(&mut s.preset, i, p).clicked() {
                                if let Some((name, url, _)) = agents::API_PRESETS.get(i) {
                                    s.base_url = url.to_string();
                                    s.name = name.to_string();
                                } else {
                                    s.base_url.clear();
                                }
                            }
                        }
                    });
                    ui.end_row();
                    ui.label(tr("ui.ai_name"));
                    ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(300.0));
                    ui.end_row();
                    ui.label(tr("ui.ai_base_url"));
                    ui.add(egui::TextEdit::singleline(&mut s.base_url).hint_text("https://example.com/v1").desired_width(300.0));
                    ui.end_row();
                    ui.label(tr("ui.ai_model"));
                    ui.add(egui::TextEdit::singleline(&mut s.model).hint_text("qwen3:14b").desired_width(300.0));
                    ui.end_row();
                    ui.label(tr("ui.ai_key"));
                    ui.add_enabled(
                        has_keychain,
                        egui::TextEdit::singleline(&mut s.key).password(true).hint_text(tr("ui.ai_key_hint")).desired_width(300.0),
                    );
                    ui.end_row();
                    ui.label("");
                    let note = if has_keychain { tr("ui.ai_key_note") } else { tr("ui.ai_no_keychain") };
                    ui.add(egui::Label::new(RichText::new(note).size(11.0).color(t.text_dim)).wrap());
                    ui.end_row();
                    ui.label("");
                    ui.horizontal(|ui| {
                        if ui.button(tr("ui.ai_test")).clicked() {
                            test = true;
                        }
                        test_line(ui, &s.test.as_ref());
                    });
                    ui.end_row();
                }
            });
            if let Some(e) = &s.error {
                ui.label(RichText::new(e).color(t.error));
            }
            ui.separator();
            ui.horizontal(|ui| {
                let b = ui.add(egui::Button::new(tr("ui.ai_add")).fill(t.accent_soft));
                crate::scenario::publish_handle("ai_add:add", b.rect.center());
                if b.clicked() {
                    add = true;
                }
            });
        });
    if test {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(s) = app.agent_chat.settings.add.as_ref() {
            let url = agents::normalize_url(&s.base_url);
            let e = agents::Entry {
                id: String::new(),
                name: s.name.clone(),
                kind: Kind::Api,
                argv: Vec::new(),
                version: None,
                login: "",
                found: false,
                base_url: url.clone().unwrap_or_default(),
                model: s.model.trim().to_string(),
                key: false,
            };
            let key = Some(s.key.trim().to_string()).filter(|k| !k.is_empty());
            let result = match url {
                Ok(_) => start_test(app, &e, key),
                Err(err) => Test::Done(Err(err)),
            };
            if let Some(s) = app.agent_chat.settings.add.as_mut() {
                s.test = Some(result);
            }
        }
    }
    if add {
        match add_agent(app) {
            Ok(()) => open = false,
            Err(e) => {
                if let Some(s) = app.agent_chat.settings.add.as_mut() {
                    s.error = Some(e);
                }
            }
        }
    }
    if !open {
        app.agent_chat.settings.add = None;
    }
    if app.agent_chat.settings.add.as_ref().is_some_and(|s| matches!(s.test, Some(Test::Running(_)))) {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}

/// Add what the dialog holds; its key goes to the keychain.
fn add_agent(app: &mut SolveApp) -> Result<(), String> {
    let Some(s) = app.agent_chat.settings.add.as_ref() else { return Ok(()) };
    let name = s.name.trim().chars().take(80).collect::<String>();
    let mut c = AgentConfig { name, ..Default::default() };
    let mut key: Option<String> = None;
    match s.kind {
        AddKind::Installed | AddKind::Custom => {
            let cmd = s.command.trim();
            if acp::split_command(cmd).is_empty() {
                return Err(tr("ui.ai_need_command").to_string());
            }
            c.kind = Kind::Acp;
            c.command = cmd.chars().take(4096).collect();
            c.id = agents::new_id(&app.preferences, "custom");
        }
        AddKind::Api => {
            c.kind = Kind::Api;
            c.base_url = agents::normalize_url(&s.base_url)?;
            c.model = s.model.trim().to_string();
            if c.model.is_empty() {
                return Err(tr("ui.ai_need_model").to_string());
            }
            key = Some(s.key.trim().to_string()).filter(|k| !k.is_empty());
            if key.is_some() && !agents::key_safe_for(&c.base_url) {
                return Err(tr("ui.ai_key_needs_https").to_string());
            }
            c.id = agents::new_id(&app.preferences, "api");
        }
    }
    if c.name.is_empty() {
        c.name = c.id.clone();
    }
    if let Some(k) = key {
        let Some(store) = &app.services.secrets else { return Err(tr("ui.ai_no_keychain").to_string()) };
        store.set(&c.id, &k).map_err(|e| trf("ui.ai_keychain_error", &[("error", &e)]))?;
        c.key = true;
    }
    app.preferences.agents.push(c);
    Ok(())
}
