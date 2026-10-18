//! Agent chat: a side panel that talks to a coding agent the user already has (Claude Code,
//! Codex, Antigravity, OpenCode, …) over the Agent Client Protocol, and gives that agent
//! SolidCraft's MCP server, bridged to this window, as its way to read and change the open design
//! (docs/agent-chat.md, docs/design/in-app-agent-chat.md).
//!
//! Desktop only: the agent is a child process. Off unless turned on in Preferences. SolidCraft
//! never handles the agent's credentials: an agent that needs a login says so, and the panel
//! shows the agent's own login command for the user to run.
//!
//! - `backend.rs`: the [`backend::ChatBackend`] seam; `acp_client.rs` is the ACP backend.
//! - `gate.rs`: what the agent's tool requests may do (run, ask the user first, or never).
//! - `panel.rs`: the panel itself (docking, minimising, the picker, the conversation).
//! - Here: the conversation, turns (each one undoes as one step), and the requests waiting for
//!   Allow or Deny.

// The web build shows only a note: the process side is desktop only.
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

pub mod acp;
#[cfg(not(target_arch = "wasm32"))]
mod acp_client;
pub mod agents;
#[cfg(not(target_arch = "wasm32"))]
mod api_client;
pub mod backend;
#[cfg(not(target_arch = "wasm32"))]
pub mod discover;
pub mod gate;
mod markdown;
mod markdown_view;
mod panel;
#[cfg(not(target_arch = "wasm32"))]
mod process;
pub mod settings;

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use serde_json::{Value, json};

use crate::i18n::{format as trf, tr};
use crate::{ControlRequest, SolveApp};
use backend::{ChatBackend, Event, PermissionOption, PromptPart, ToolStatus};

pub use panel::{show, tip as panel_tip, toggle};

/// Most entries kept in the conversation; older ones are dropped.
const MAX_ITEMS: usize = 2000;
/// Longest message sent.
const MAX_PROMPT: usize = 100_000;
/// How long a request waits for Allow or Deny before it is declined (the agent's MCP server gives
/// up on a request after five minutes).
const APPROVAL_TIMEOUT_MS: f64 = 240_000.0;
/// Size of the picture of the view sent with a message.
const VIEW_IMAGE: (u32, u32) = (960, 600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    Starting,
    Ready,
    Busy,
    NeedsAuth,
    Exited,
}

impl State {
    fn id(self) -> &'static str {
        match self {
            State::Off => "off",
            State::Starting => "starting",
            State::Ready => "ready",
            State::Busy => "busy",
            State::NeedsAuth => "auth",
            State::Exited => "exited",
        }
    }
    fn label(self) -> &'static str {
        tr(match self {
            State::Off => "ui.agent_chat_state_off",
            State::Starting => "ui.agent_chat_state_starting",
            State::Ready => "ui.agent_chat_state_ready",
            State::Busy => "ui.agent_chat_state_busy",
            State::NeedsAuth => "ui.agent_chat_state_auth",
            State::Exited => "ui.agent_chat_state_exited",
        })
    }
    fn running(self) -> bool {
        matches!(self, State::Starting | State::Ready | State::Busy | State::NeedsAuth)
    }
}

#[derive(Clone, Debug)]
pub enum Item {
    User {
        text: String,
        image: bool,
    },
    Agent {
        thought: bool,
        text: String,
    },
    Tool {
        id: String,
        title: String,
        kind: String,
        status: Option<ToolStatus>,
        output: Option<String>,
    },
    Plan(Vec<(String, String)>),
    /// The agent asks before running something of its own (its options, its words).
    Permission {
        key: u64,
        title: String,
        options: Vec<PermissionOption>,
        answer: Option<String>,
    },
    /// A request of the agent's that SolidCraft holds for the user (files, replacing the design,
    /// or any change while the mode is "ask"); `tool`: what "Always allow" remembers.
    Approval {
        id: u64,
        title: String,
        tool: String,
        answer: Option<bool>,
    },
    /// A message's changes, merged into one undo step; `doc`: the design right after it.
    Changes {
        steps: usize,
        doc: Arc<solidcraft_engine::doc::Document>,
        undone: bool,
    },
    Notice {
        text: String,
        error: bool,
    },
}

/// The message being answered: what undo and the limits need.
struct Turn {
    /// The design and the undo depth before it.
    start_doc: Arc<solidcraft_engine::doc::Document>,
    start_undo: usize,
    /// Requests that ran commands, and requests of any kind.
    changes: u32,
    requests: u32,
    limit_told: bool,
    label: String,
}

/// A message typed while the agent works: shown in the transcript as "Queued" until it is sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub text: String,
    pub image: bool,
}

/// A request held for Allow or Deny.
struct Held {
    id: u64,
    req: ControlRequest,
    since: f64,
}

/// Starts the chat's own control endpoint for the agent's MCP server (the desktop host provides
/// it): requests arriving there go to the sender; the port it listens on.
pub type ControlStarter = Box<dyn FnMut(Sender<ControlRequest>) -> Option<u16>>;

#[derive(Default)]
pub struct AgentChat {
    /// The panel is shown (View menu, the application bar's chat button, Ctrl+Shift+A).
    pub open: bool,
    pub items: Vec<Item>,
    pub input: String,
    state: Option<State>,
    agent: String,
    images: bool,
    /// Send a picture of the view with the next message.
    pub attach_view: bool,
    auth: Vec<acp::AuthMethod>,
    auth_status: Option<String>,
    /// A message typed before the agent was ready.
    queued: Option<(String, bool)>,
    /// Messages typed while the agent works, in order. They go as one prompt when the turn ends
    /// (or at once with Send now); after Stop they wait for Send.
    pub pending: Vec<Pending>,
    /// The queued message being edited, and its text so far.
    pub pending_edit: Option<(usize, String)>,
    /// Send now was pressed: the queue goes as soon as the cancelled turn has ended.
    steering: bool,
    /// The turn was stopped: its queue is not sent by itself.
    stopped: bool,
    /// Why the last turn ended.
    ended: Option<String>,
    /// New chat while a message ran: its late end is not reported.
    discard_end: bool,
    /// The mode to put back when the fresh session is ready.
    restore_mode: Option<String>,
    /// New chat is waiting for the user's yes (a message is running or queued).
    pub confirm_new: bool,
    /// Where the panel was drawn last (for its shortcuts).
    pub panel_rect: Option<egui::Rect>,
    /// The last message sent, kept to send again after a sign-in.
    last: Option<(String, bool)>,
    /// The command the running agent was started with.
    argv: Vec<String>,
    /// The agent picked for this conversation (the header's switcher); `None`: the default.
    pub agent_id: Option<String>,
    /// The agent the running backend is (for permissions and "always allow").
    running_id: String,
    /// The popped-out chat window was given its saved place (it is open); see `panel::window_builder`.
    pub(crate) window_placed: bool,
    /// The agent's own modes, and the current one.
    modes: Vec<(String, String)>,
    mode: String,
    /// The models the agent offers (none: no picker), and the current one.
    pub models: Vec<acp::ModelInfo>,
    pub model: String,
    /// An API endpoint's models, kept per endpoint (its base URL) for the next connection.
    models_cache: std::collections::HashMap<String, Vec<acp::ModelInfo>>,
    /// The running API agent's endpoint (empty for other agents).
    running_endpoint: String,
    backend: Option<Box<dyn ChatBackend>>,
    turn: Option<Turn>,
    held: Vec<Held>,
    next_approval: u64,
    /// The chat's control endpoint: the port, and its requests.
    pub tools_port: Option<u16>,
    tools_rx: Option<Receiver<ControlRequest>>,
    tools_tx: Option<Sender<ControlRequest>>,
    /// Settings → AI: the Add agent dialog and connection tests.
    pub settings: settings::State,
    /// Desktop host hook: start the endpoint on demand.
    pub control_starter: Option<ControlStarter>,
    /// The picker is unfolded (it folds to one line once an agent is connected).
    picking: Option<bool>,
    #[cfg(not(target_arch = "wasm32"))]
    detection: Option<discover::Detection>,
    #[cfg(not(target_arch = "wasm32"))]
    detecting: Option<Receiver<discover::Detection>>,
}

impl Drop for AgentChat {
    fn drop(&mut self) {
        self.stop_backend(true);
    }
}

impl AgentChat {
    pub fn state(&self) -> State {
        self.state.unwrap_or(State::Off)
    }
    fn push(&mut self, item: Item) {
        if self.items.len() >= MAX_ITEMS {
            self.items.drain(..MAX_ITEMS / 10);
        }
        self.items.push(item);
    }
    fn notice(&mut self, text: impl Into<String>, error: bool) {
        self.push(Item::Notice { text: text.into(), error });
    }
    /// Requests waiting for Allow or Deny.
    pub fn waiting(&self) -> usize {
        self.held.len() + self.items.iter().filter(|i| matches!(i, Item::Permission { answer: None, .. })).count()
    }

    fn stop_backend(&mut self, wait: bool) {
        self.steering = false;
        if let Some(mut b) = self.backend.take() {
            b.cancel();
            b.shutdown(wait);
        }
        for h in self.held.drain(..) {
            let _ = h.req.reply.send(json!({"ok": false, "error": "the agent chat stopped"}));
        }
        for i in &mut self.items {
            if let Item::Approval { answer: a @ None, .. } = i {
                *a = Some(false);
            }
            if let Item::Permission { answer: a @ None, .. } = i {
                *a = Some(tr("ui.agent_chat_cancel").to_string());
            }
        }
    }

    /// Answer the agent's own permission question at `index` (`None`: cancelled).
    pub fn answer(&mut self, index: usize, option: Option<&str>) {
        let Some(Item::Permission { key, options, answer, .. }) = self.items.get_mut(index) else { return };
        if answer.is_some() {
            return;
        }
        let label =
            option.and_then(|o| options.iter().find(|x| x.id == o)).map_or_else(|| tr("ui.agent_chat_cancel").to_string(), |o| o.name.clone());
        *answer = Some(label);
        let key = *key;
        if let Some(b) = self.backend.as_mut() {
            b.answer_permission(key, option);
        }
    }

    /// Ask the agent to sign in by itself (an "agent" login method: it opens the browser).
    pub fn authenticate(&mut self, method: &str) {
        if let Some(b) = self.backend.as_mut() {
            b.authenticate(method);
            self.state = Some(State::Starting);
        }
    }

    /// Take in what the backend reported.
    fn on_event(&mut self, e: Event) -> Option<Outcome> {
        match e {
            Event::Ready { agent, images } => {
                self.agent = agent;
                self.images = images;
                self.state = Some(State::Ready);
                self.picking = Some(false);
                // Keep the mode across a new chat.
                if let Some(m) = self.restore_mode.take()
                    && self.modes.iter().any(|(id, _)| *id == m)
                    && let Some(b) = self.backend.as_mut()
                {
                    b.set_mode(&m);
                    self.mode = m;
                }
            }
            Event::Text { thought, text } => {
                if let Some(Item::Agent { thought: t, text: last }) = self.items.last_mut()
                    && *t == thought
                {
                    last.push_str(&text);
                } else {
                    self.push(Item::Agent { thought, text });
                }
            }
            Event::Tool { id, title, kind, status, output } => {
                let existing = self.items.iter_mut().rev().find(|i| matches!(i, Item::Tool { id: x, .. } if *x == id));
                if let Some(Item::Tool { title: t, kind: k, status: s, output: o, .. }) = existing {
                    if let Some(v) = title {
                        *t = v;
                    }
                    if let Some(v) = kind {
                        *k = v;
                    }
                    if status.is_some() {
                        *s = status;
                    }
                    if output.is_some() {
                        *o = output;
                    }
                } else {
                    self.push(Item::Tool { id, title: title.unwrap_or_default(), kind: kind.unwrap_or_default(), status, output });
                }
            }
            Event::Plan(entries) => {
                // A plan is replaced whole: update this turn's plan where it is.
                let turn = self.items.iter().rposition(|i| matches!(i, Item::User { .. })).unwrap_or(0);
                if let Some(Item::Plan(p)) = self.items.iter_mut().skip(turn).rev().find(|i| matches!(i, Item::Plan(_))) {
                    *p = entries;
                } else {
                    self.push(Item::Plan(entries));
                }
            }
            Event::Permission { key, title, options } => return Some(Outcome::Permission { key, title, options }),
            Event::Modes { current, available } => {
                if !available.is_empty() {
                    self.modes = available;
                }
                self.mode = current;
            }
            Event::Models { current, available } => {
                if !available.is_empty() {
                    if !self.running_endpoint.is_empty() {
                        self.models_cache.insert(self.running_endpoint.clone(), available.clone());
                    }
                    self.models = available;
                }
                self.model = current;
            }
            Event::AuthRequired { methods } => {
                self.auth = methods;
                self.state = Some(State::NeedsAuth);
                self.picking = Some(false);
                // What was asked goes again once the login is done.
                if self.queued.is_none() {
                    self.queued = self.last.take();
                }
            }
            Event::AuthStatus(s) => self.auth_status = Some(s),
            Event::TurnEnded { reason } => {
                if self.state() == State::Busy {
                    self.state = Some(State::Ready);
                }
                self.ended = Some(reason.clone());
                if std::mem::take(&mut self.discard_end) {
                    return Some(Outcome::TurnEnd);
                }
                if !matches!(reason.as_str(), "" | "auth" | "error") {
                    self.notice(format!("({reason})"), false);
                }
                return Some(Outcome::TurnEnd);
            }
            Event::Error(e) => {
                self.notice(e, true);
                if !self.state().running() || self.state() == State::Starting {
                    self.state = Some(State::Exited);
                }
            }
            Event::Exited { tail } => {
                self.state = Some(State::Exited);
                self.steering = false;
                let last: Vec<String> = tail.into_iter().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
                let detail = if last.is_empty() { String::new() } else { format!("\n{}", last.join("\n")) };
                self.notice(format!("{}{detail}", tr("ui.agent_chat_exited")), true);
                self.stop_backend(false);
                return Some(Outcome::TurnEnd);
            }
        }
        None
    }
}

/// What an event asks of the app.
enum Outcome {
    /// A message was answered (or the agent stopped): the turn's bookkeeping ends.
    TurnEnd,
    /// The agent asks before running something of its own: answered by the mode, or asked.
    Permission { key: u64, title: String, options: Vec<PermissionOption> },
}

/// The AI features are on (the master switch in Settings → AI) and this is the desktop app.
pub fn shown(app: &SolveApp) -> bool {
    app.preferences.ai_features && cfg!(not(target_arch = "wasm32"))
}

/// The AI button at the toolbar row's right end is shown.
pub fn toolbar_button_shown(app: &SolveApp) -> bool {
    shown(app) && app.preferences.ai_toolbar_button
}

/// Whether a permission question is about one of SolidCraft's own tools (which SolidCraft's own
/// gate checks): the agents name MCP tools after their server.
fn solidcraft_tool(title: &str) -> bool {
    title.to_ascii_lowercase().contains("solidcraft")
}

/// `solidcraft-cli` next to this program, else on the search path.
#[cfg(not(target_arch = "wasm32"))]
fn find_cli() -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) { "solidcraft-cli.exe" } else { "solidcraft-cli" };
    if let Some(p) = solidcraft_engine::geom::env::var("CLI").map(std::path::PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(name)))
        .filter(|p| p.is_file())
        .or_else(|| discover::find_in("solidcraft-cli", &discover::search_dirs()))
}

/// The agent's working folder: an empty folder of SolidCraft's own, so an agent's own file tools
/// (where it has them) see nothing of the user's.
fn working_dir() -> std::path::PathBuf {
    let d = solidcraft_engine::recovery::data_dir()
        .map(|d| d.join("agent-chat").join("work"))
        .unwrap_or_else(|| std::env::temp_dir().join("solidcraft-agent-chat"));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Start the agent: the chat's control endpoint for its MCP bridge if needed, the process.
#[cfg(not(target_arch = "wasm32"))]
pub fn connect(app: &mut SolveApp, ctx: &egui::Context) {
    disconnect(app);
    let Some(entry) = agents::current(app) else {
        app.agent_chat.notice(tr("ui.agent_chat_none_found"), true);
        return;
    };
    // An API agent's key comes from the keychain for this connection only.
    let key = if entry.kind == agents::Kind::Api && entry.key {
        match app.services.secrets.as_ref().map(|s| s.get(&entry.id)) {
            Some(Ok(k)) => k,
            Some(Err(e)) => {
                app.agent_chat.notice(trf("ui.ai_keychain_error", &[("error", &e)]), true);
                return;
            }
            None => None,
        }
    } else {
        None
    };
    let http = app.services.agent_http.clone();
    let chat = &mut app.agent_chat;
    if chat.tools_port.is_none()
        && let Some(start) = chat.control_starter.as_mut()
    {
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(port) = start(tx.clone()) {
            chat.tools_port = Some(port);
            chat.tools_rx = Some(rx);
            chat.tools_tx = Some(tx);
        }
    }
    let cli = find_cli();
    if chat.tools_port.is_none() {
        chat.notice(tr("ui.agent_chat_no_control"), true);
    } else if cli.is_none() {
        chat.notice(tr("ui.agent_chat_no_cli"), true);
    }
    let bridge = cli.zip(chat.tools_port).map(|(cli, port)| acp::McpBridge { cli, port });
    let wake = ctx.clone();
    let start = backend::StartContext { cwd: working_dir(), bridge, tools: chat.tools_tx.clone(), wake: Box::new(move || wake.request_repaint()) };
    let started: Result<Box<dyn ChatBackend>, String> = match entry.kind {
        agents::Kind::Acp if entry.argv.is_empty() => Err(tr("ui.agent_chat_none_found").to_string()),
        agents::Kind::Acp => acp_client::AcpBackend::start(&entry.argv, start).map(|b| Box::new(b) as Box<dyn ChatBackend>),
        agents::Kind::Api => match http {
            Some(http) => api_client::ApiBackend::start(
                api_client::ApiConfig { name: entry.name.clone(), base_url: entry.base_url.clone(), model: entry.model.clone(), key, http },
                start,
            )
            .map(|b| Box::new(b) as Box<dyn ChatBackend>),
            None => Err(tr("ui.ai_no_http").to_string()),
        },
    };
    match started {
        Ok(b) => {
            chat.backend = Some(b);
            chat.argv = entry.argv.clone();
            chat.running_id = entry.id.clone();
            chat.modes.clear();
            chat.mode.clear();
            chat.models.clear();
            chat.model.clear();
            chat.running_endpoint = if entry.kind == agents::Kind::Api { entry.base_url.clone() } else { String::new() };
            // An endpoint's models from the last time, until the new list arrives.
            if let Some(list) = chat.models_cache.get(&chat.running_endpoint) {
                chat.models = list.clone();
                chat.model = entry.model.clone();
            }
            chat.state = Some(State::Starting);
            chat.auth.clear();
            chat.auth_status = None;
            chat.agent.clear();
        }
        Err(e) => {
            chat.notice(trf("ui.agent_chat_failed_start", &[("error", &e)]), true);
            chat.state = Some(State::Exited);
        }
    }
}

/// Stop the agent (and everything it started); the conversation stays.
pub fn disconnect(app: &mut SolveApp) {
    end_turn(app);
    let chat = &mut app.agent_chat;
    chat.stop_backend(false);
    if chat.state.is_some() {
        chat.state = Some(State::Off);
    }
}

/// The app is quitting: end the agent and its helpers before the window goes.
pub fn shutdown(app: &mut SolveApp) {
    end_turn(app);
    app.agent_chat.stop_backend(true);
}

/// Most messages waiting in the queue.
const MAX_PENDING: usize = 20;
/// Said to the agent before a message that interrupted it, so it knows its last step may be
/// incomplete.
const INTERRUPT_NOTE: &str = "[The user interrupted your previous step.]";

/// Send what the user typed: now if the agent is ready, queued behind the running message if it
/// is working, else once it is ready (starting it if needed).
pub fn send(app: &mut SolveApp, ctx: &egui::Context, text: String) {
    let text: String = text.chars().take(MAX_PROMPT).collect();
    if text.trim().is_empty() {
        return;
    }
    let image = app.agent_chat.attach_view && app.agent_chat.images;
    if app.agent_chat.state() == State::Busy {
        let chat = &mut app.agent_chat;
        if chat.pending.len() >= MAX_PENDING {
            chat.notice(tr("ui.agent_chat_queue_full"), false);
        } else {
            chat.pending.push(Pending { text, image });
        }
        return;
    }
    // Messages left waiting by Stop go first, in one prompt with this one.
    if !app.agent_chat.pending.is_empty() {
        app.agent_chat.pending.push(Pending { text, image });
        send_queue(app, ctx, false);
        return;
    }
    submit(app, ctx, text, image, false);
}

/// Put `text` in the conversation and send it (`interrupted`: the agent is told it cut in on its
/// previous step).
fn submit(app: &mut SolveApp, ctx: &egui::Context, text: String, image: bool, interrupted: bool) {
    app.agent_chat.push(Item::User { text: text.clone(), image });
    match app.agent_chat.state() {
        State::Ready => start_turn_with(app, text, image, interrupted.then_some(INTERRUPT_NOTE)),
        s => {
            app.agent_chat.queued = Some((text, image));
            #[cfg(not(target_arch = "wasm32"))]
            if !s.running() {
                connect(app, ctx);
            }
            #[cfg(target_arch = "wasm32")]
            let _ = (s, ctx);
        }
    }
}

/// Send the queued messages as one prompt, in order.
fn send_queue(app: &mut SolveApp, ctx: &egui::Context, interrupted: bool) {
    let chat = &mut app.agent_chat;
    chat.pending_edit = None;
    if chat.pending.is_empty() {
        return;
    }
    let all = std::mem::take(&mut chat.pending);
    let image = all.iter().any(|p| p.image);
    let text = all.into_iter().map(|p| p.text).collect::<Vec<_>>().join("\n\n");
    let text: String = text.chars().take(MAX_PROMPT).collect();
    submit(app, ctx, text, image, interrupted);
}

/// Send now: stop the running step and send the queue as the next prompt once the agent has said
/// it stopped (its note tells the agent it was interrupted). With nothing running, sends the queue.
pub fn send_now(app: &mut SolveApp, ctx: &egui::Context) {
    if app.agent_chat.pending.is_empty() {
        return;
    }
    if app.agent_chat.state() == State::Busy {
        app.agent_chat.steering = true;
        app.agent_chat.stopped = false;
        interrupt(app);
    } else {
        send_queue(app, ctx, false);
    }
}

/// New chat: stop what runs (as Stop does), clear the conversation and the queue, and start a
/// fresh session on the same agent (no restart); the agent and its mode stay, the design's undo
/// history is untouched.
pub fn new_chat(app: &mut SolveApp) {
    let running = app.agent_chat.state() == State::Busy;
    if running {
        cancel(app);
    }
    decline_all(app, "the chat was cleared");
    end_turn(app);
    let chat = &mut app.agent_chat;
    chat.confirm_new = false;
    chat.items.clear();
    chat.pending.clear();
    chat.pending_edit = None;
    chat.queued = None;
    chat.last = None;
    chat.steering = false;
    chat.stopped = false;
    chat.ended = None;
    chat.discard_end = running;
    if matches!(chat.state(), State::Ready | State::Busy) {
        chat.restore_mode = Some(chat.mode.clone()).filter(|m| !m.is_empty());
        chat.state = Some(State::Starting);
        if let Some(b) = chat.backend.as_mut() {
            b.new_session();
        }
    }
}

/// Whether New chat would throw something away that the user may want (so it asks first).
pub fn new_chat_needs_confirm(app: &SolveApp) -> bool {
    app.agent_chat.state() == State::Busy || !app.agent_chat.pending.is_empty()
}

/// Take a queued message out of the queue.
pub fn remove_pending(app: &mut SolveApp, index: usize) {
    let chat = &mut app.agent_chat;
    if index < chat.pending.len() {
        chat.pending.remove(index);
    }
    chat.pending_edit = None;
}

/// The turn ended: the queue goes on its own after a normal end, and after a Send now; not after
/// Stop, a sign-in request or an error.
fn after_turn(app: &mut SolveApp, ctx: &egui::Context) {
    let reason = app.agent_chat.ended.take().unwrap_or_default();
    let steering = std::mem::take(&mut app.agent_chat.steering);
    if app.agent_chat.state() != State::Ready || app.agent_chat.pending.is_empty() {
        return;
    }
    if steering && !matches!(reason.as_str(), "auth" | "error") {
        send_queue(app, ctx, reason == "cancelled");
    } else if !app.agent_chat.stopped && reason.is_empty() {
        send_queue(app, ctx, false);
    }
}

fn start_turn(app: &mut SolveApp, text: String, image: bool) {
    start_turn_with(app, text, image, None);
}

/// Start a message; `note` goes before the text for the agent, not into the conversation.
fn start_turn_with(app: &mut SolveApp, text: String, image: bool, note: Option<&str>) {
    let sent = match note {
        Some(n) => format!("{n}\n\n{text}"),
        None => text.clone(),
    };
    let mut parts = vec![PromptPart::Text(sent)];
    if image {
        match crate::control::view_png(app, VIEW_IMAGE.0, VIEW_IMAGE.1) {
            Ok(png) => parts.push(PromptPart::Image { mime: "image/png".into(), base64: backend::base64(&png) }),
            Err(e) => log::warn!("the view's picture is left out of the prompt: {e}"),
        }
    }
    let label: String = text.lines().next().unwrap_or("").chars().take(48).collect();
    app.agent_chat.turn =
        Some(Turn { start_doc: app.session.doc.clone(), start_undo: app.session.undo.len(), changes: 0, requests: 0, limit_told: false, label });
    app.agent_chat.last = Some((text, image));
    app.agent_chat.state = Some(State::Busy);
    app.agent_chat.stopped = false;
    app.agent_chat.steering = false;
    app.agent_chat.ended = None;
    if let Some(b) = app.agent_chat.backend.as_mut() {
        b.prompt(&parts);
    }
}

/// Stop: ask the agent to stop the message it is on; requests waiting for approval are declined.
/// Messages queued meanwhile stay queued (they wait for Send).
pub fn cancel(app: &mut SolveApp) {
    app.agent_chat.steering = false;
    app.agent_chat.stopped = true;
    interrupt(app);
}

/// Ask the agent to stop the message it is on and decline what waits for approval.
fn interrupt(app: &mut SolveApp) {
    decline_all(app, "the user stopped the message");
    if let Some(b) = app.agent_chat.backend.as_mut() {
        b.cancel();
    }
    for i in &mut app.agent_chat.items {
        if let Item::Permission { answer: a @ None, .. } = i {
            *a = Some(tr("ui.agent_chat_cancel").to_string());
        }
    }
}

fn decline_all(app: &mut SolveApp, why: &str) {
    let ids: Vec<u64> = app.agent_chat.held.iter().map(|h| h.id).collect();
    for id in ids {
        approve_with(app, id, false, why);
    }
}

/// Allow or deny a held request.
pub fn approve(app: &mut SolveApp, id: u64, allow: bool) {
    approve_with(app, id, allow, "the user declined it");
}

/// Allow a held request and remember to allow its tool for this agent from now on.
pub fn approve_always(app: &mut SolveApp, id: u64) {
    let tool = app.agent_chat.items.iter().find_map(|i| match i {
        Item::Approval { id: x, tool, .. } if *x == id => Some(tool.clone()),
        _ => None,
    });
    if let Some(tool) = tool {
        let agent = app.agent_chat.running_id.clone();
        agents::remember(&mut app.preferences, &agent, &tool);
    }
    approve(app, id, true);
}

/// Answer the agent's own permission question at `index` with one of its options (`None`:
/// cancelled); an "always allow" answer is remembered for this agent and tool.
pub fn answer_permission(app: &mut SolveApp, index: usize, option: Option<&str>) {
    if let (Some(o), Some(Item::Permission { title, options, answer: None, .. })) = (option, app.agent_chat.items.get(index))
        && options.iter().any(|x| x.id == o && x.kind == "allow_always")
    {
        let (agent, title) = (app.agent_chat.running_id.clone(), title.clone());
        agents::remember(&mut app.preferences, &agent, &title);
    }
    app.agent_chat.answer(index, option);
}

/// The agent asks before running something of its own. SolidCraft's own tools are checked by
/// SolidCraft's gate, so they're allowed here (no double question); otherwise the mode or an
/// earlier "always allow" may answer, and the chat says so; otherwise the user is asked.
fn on_permission(app: &mut SolveApp, key: u64, title: String, options: Vec<PermissionOption>) {
    let agent = app.agent_chat.running_id.clone();
    let mode = agents::mode_for(&app.preferences, &agent);
    let remembered = agents::always_allowed(&app.preferences, &agent, &title);
    let why = if solidcraft_tool(&title) {
        Some("ui.ai_auto_ours")
    } else if remembered {
        Some("ui.ai_auto_remembered")
    } else if mode == agents::Mode::All {
        Some("ui.ai_auto_mode")
    } else {
        None
    };
    let once = options.iter().find(|o| o.kind == "allow_once");
    let always = options.iter().find(|o| o.kind == "allow_always");
    let pick = if remembered { always.or(once) } else { once.or(always) }.map(|o| o.id.clone());
    match (why, pick) {
        (Some(why), Some(option)) => {
            if let Some(b) = app.agent_chat.backend.as_mut() {
                b.answer_permission(key, Some(&option));
            }
            app.agent_chat.push(Item::Notice { text: trf("ui.ai_auto_allowed", &[("title", &title), ("why", tr(why))]), error: false });
        }
        _ => app.agent_chat.push(Item::Permission { key, title, options, answer: None }),
    }
}

fn approve_with(app: &mut SolveApp, id: u64, allow: bool, why: &str) {
    let chat = &mut app.agent_chat;
    let Some(i) = chat.held.iter().position(|h| h.id == id) else { return };
    let h = chat.held.remove(i);
    for item in &mut chat.items {
        if let Item::Approval { id: x, answer, .. } = item
            && *x == id
        {
            *answer = Some(allow);
        }
    }
    if allow {
        app.control_held.push_back(h.req);
    } else {
        let _ = h.req.reply.send(json!({"ok": false, "error": format!("not run: {why}")}));
    }
}

/// The message is answered: its changes become one undo step, and what still waits is declined.
fn end_turn(app: &mut SolveApp) {
    decline_all(app, "the message ended");
    let Some(turn) = app.agent_chat.turn.take() else { return };
    let steps = merge_undo(&mut app.session.undo, &turn.start_doc, turn.start_undo, &trf("ui.agent_chat_undo_label", &[("message", &turn.label)]));
    if steps > 0 {
        let doc = app.session.doc.clone();
        app.agent_chat.push(Item::Changes { steps, doc, undone: false });
    }
}

/// Merge the undo steps made since `start_doc` (at depth `start_undo`) into one, labelled `label`;
/// how many steps it took in (0: none were made, or the history no longer reaches that far).
fn merge_undo(
    undo: &mut Vec<solidcraft_engine::Snapshot>,
    start_doc: &Arc<solidcraft_engine::doc::Document>,
    start_undo: usize,
    label: &str,
) -> usize {
    // The first change of the turn saved the design as it was when the turn began. When the
    // history is full its oldest steps go, so look for it at or below where it was put.
    let Some(first) = (0..undo.len().min(start_undo + 1)).rev().find(|&i| undo.get(i).is_some_and(|s| Arc::ptr_eq(&s.doc, start_doc))) else {
        return 0;
    };
    let steps = undo.len() - first;
    undo.truncate(first + 1);
    if let Some(s) = undo.get_mut(first) {
        s.label = label.to_string();
    }
    steps
}

/// Undo a message's changes from the conversation (only while they are the last thing done).
pub fn undo_changes(app: &mut SolveApp, index: usize) {
    let Some(Item::Changes { doc, undone, .. }) = app.agent_chat.items.get(index) else { return };
    if *undone || !Arc::ptr_eq(doc, &app.session.doc) {
        return;
    }
    if app.run("edit.undo", json!({})).is_ok()
        && let Some(Item::Changes { undone, .. }) = app.agent_chat.items.get_mut(index)
    {
        *undone = true;
    }
}

/// The requests the agent's tools sent through the chat's endpoint: each one passes the gate.
fn pump_tools(app: &mut SolveApp) {
    let Some(rx) = app.agent_chat.tools_rx.take() else { return };
    let limit = app.preferences.agent_chat_turn_limit.clamp(1, 1000);
    for mut req in rx.try_iter().take(64) {
        let refuse = |req: ControlRequest, e: String| {
            let _ = req.reply.send(json!({"ok": false, "error": e}));
        };
        let undo_len = app.session.undo.len();
        let Some(turn) = app.agent_chat.turn.as_mut() else {
            refuse(req, "the agent chat has no message in progress".into());
            continue;
        };
        turn.requests += 1;
        if turn.requests > gate::MAX_REQUESTS_PER_TURN {
            refuse(req, format!("too many requests in one message (at most {})", gate::MAX_REQUESTS_PER_TURN));
            continue;
        }
        // Requests without folders of their own stay inside the chat's working folder.
        if let Some(o) = req.params.as_object_mut()
            && !o.contains_key("allow_paths")
        {
            o.insert("allow_paths".into(), json!([working_dir().to_string_lossy()]));
        }
        let undo_left = undo_len.saturating_sub(turn.start_undo);
        // The permission mode (the agent's own, else the global one) and "always allow" answers.
        let mode = agents::mode_for(&app.preferences, &app.agent_chat.running_id);
        let tool = gate::tool_key(&req.method, &req.params);
        let always = agents::always_allowed(&app.preferences, &app.agent_chat.running_id, &tool);
        let verdict = match gate::request(&req.method, &req.params, undo_left) {
            gate::Verdict::Ask(_) if mode == agents::Mode::All || always => gate::Verdict::Run { change: true },
            gate::Verdict::Run { change: true } if mode == agents::Mode::Ask && !always => {
                gate::Verdict::Ask(gate::describe(&req.method, &req.params))
            }
            v => v,
        };
        match verdict {
            gate::Verdict::Run { change } => {
                if change {
                    if turn.changes >= limit {
                        let first = !turn.limit_told;
                        turn.limit_told = true;
                        refuse(req, format!("the limit of {limit} changes for one message is reached: tell the user what is left to do"));
                        if first {
                            app.agent_chat.notice(trf("ui.agent_chat_limit", &[("limit", &limit.to_string())]), false);
                        }
                        continue;
                    }
                    turn.changes += 1;
                }
                app.control_held.push_back(req);
            }
            gate::Verdict::Ask(title) => {
                let chat = &mut app.agent_chat;
                chat.next_approval += 1;
                let id = chat.next_approval;
                chat.push(Item::Approval { id, title, tool, answer: None });
                chat.held.push(Held { id, req, since: crate::now_ms() });
            }
            gate::Verdict::Refuse(e) => {
                app.agent_chat.notice(trf("ui.agent_chat_refused", &[("reason", &e)]), false);
                refuse(req, e);
            }
        }
    }
    app.agent_chat.tools_rx = Some(rx);
    // Questions nobody answered: the agent stopped waiting long ago.
    let now = crate::now_ms();
    let stale: Vec<u64> = app.agent_chat.held.iter().filter(|h| now - h.since > APPROVAL_TIMEOUT_MS).map(|h| h.id).collect();
    for id in stale {
        approve_with(app, id, false, "nobody answered in time");
    }
}

/// Read what the agent sent since the last frame, its tool requests, and the agent search.
pub fn poll(app: &mut SolveApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(rx) = &app.agent_chat.detecting
        && let Ok(d) = rx.try_recv()
    {
        app.agent_chat.detection = Some(d);
        app.agent_chat.detecting = None;
    }
    // AI features turned off: nothing keeps running, nothing is looked for.
    if !shown(app) {
        if app.agent_chat.backend.is_some() || app.agent_chat.open {
            disconnect(app);
            app.agent_chat.open = false;
        }
        return;
    }
    let events = app.agent_chat.backend.as_mut().map(|b| b.poll()).unwrap_or_default();
    for e in events {
        let ready = matches!(e, Event::Ready { .. });
        match app.agent_chat.on_event(e) {
            Some(Outcome::TurnEnd) => {
                end_turn(app);
                after_turn(app, ctx);
            }
            Some(Outcome::Permission { key, title, options }) => on_permission(app, key, title, options),
            None => {}
        }
        if ready {
            apply_saved_model(app);
        }
        if ready && let Some((text, image)) = app.agent_chat.queued.take() {
            start_turn(app, text, image);
        }
    }
    pump_tools(app);
    if !app.agent_chat.held.is_empty() || app.agent_chat.state() == State::Busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Look for installed agents in the background (cached; Refresh looks again).
#[cfg(not(target_arch = "wasm32"))]
pub fn refresh(app: &mut SolveApp, ctx: &egui::Context) {
    if app.agent_chat.detecting.is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(discover::detect());
        ctx.request_repaint();
    });
    app.agent_chat.detecting = Some(rx);
}

/// Choose the model for this chat (the header's model picker): the agent switches to it, and it is
/// remembered for this agent; an API endpoint's agent also keeps it as its default model.
pub fn set_model(app: &mut SolveApp, model: &str) {
    if !app.agent_chat.models.iter().any(|m| m.id == model) {
        return;
    }
    if let Some(b) = app.agent_chat.backend.as_mut() {
        b.set_model(model);
    }
    app.agent_chat.model = model.to_string();
    let agent = app.agent_chat.running_id.clone();
    if let Some(a) = app.preferences.agents.iter_mut().find(|a| a.id == agent && a.kind == agents::Kind::Api) {
        a.model = model.to_string();
    }
    app.preferences.agent_models.insert(agent, model.to_string());
}

/// A new session is ready: put back the model last chosen for this agent, when it offers it.
fn apply_saved_model(app: &mut SolveApp) {
    let chat = &app.agent_chat;
    let Some(saved) = app.preferences.agent_models.get(&chat.running_id).cloned() else { return };
    if saved != chat.model && chat.models.iter().any(|m| m.id == saved) {
        set_model(app, &saved);
    }
}

/// Whether the model picker shows: only for an agent that reports models.
pub fn models_shown(app: &SolveApp) -> bool {
    !app.agent_chat.models.is_empty() && app.agent_chat.state().running()
}

/// Switch the agent to one of its own modes (the header's mode picker).
pub fn set_mode(app: &mut SolveApp, mode: &str) {
    if app.agent_chat.modes.iter().any(|(id, _)| id == mode)
        && let Some(b) = app.agent_chat.backend.as_mut()
    {
        b.set_mode(mode);
        app.agent_chat.mode = mode.to_string();
    }
}

/// `ui.agent_chat` on the control channel (tests and screenshots): `enabled`, `command`, `open`,
/// `side`, `minimized`, `width`, `refresh`, `connect`, `stop`, `send`, `attach_view`,
/// `permission` (an option id, or `"cancel"`), `approve` (`true`/`false`, the oldest held
/// request), `cancel` (Stop), `send_now` (steer: stop, then send the queue), `remove_pending` (index), `undo` (the newest message's changes); returns the panel's state.
pub fn control(app: &mut SolveApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    if let Some(e) = p.get("features").and_then(Value::as_bool) {
        app.preferences.ai_features = e;
    }
    if let Some(b) = p.get("toolbar_button").and_then(Value::as_bool) {
        app.preferences.ai_toolbar_button = b;
    }
    if let Some(m) = p.get("permissions") {
        let Some(mode) = m.as_str().and_then(agents::Mode::parse) else { return Err("permissions must be ask, edits or all".into()) };
        app.preferences.ai_permissions = mode;
    }
    // Add an agent (as the Add agent dialog does), for tests and screenshots: {id?, name, kind, command | base_url + model}.
    if let Some(a) = p.get("add_agent") {
        let mut c: agents::AgentConfig = serde_json::from_value(a.clone()).map_err(|e| format!("add_agent: {e}"))?;
        if c.id.is_empty() {
            c.id = agents::new_id(&app.preferences, if c.kind == agents::Kind::Api { "api" } else { "custom" });
        }
        c.key = false;
        app.preferences.agents.retain(|x| x.id != c.id);
        app.preferences.agents.push(c);
    }
    if let Some(id) = p.get("agent").and_then(Value::as_str) {
        if !agents::entries(app).iter().any(|e| e.id == id) {
            return Err(format!("no agent `{id}`"));
        }
        if app.agent_chat.agent_id.as_deref() != Some(id) {
            disconnect(app);
            app.agent_chat.agent_id = Some(id.to_string());
        }
    }
    // The Add agent dialog: open on a kind (`installed`, `custom`, `api`), or `false` to close.
    if let Some(d) = p.get("add_dialog") {
        match d.as_str() {
            Some(k) => {
                let kind = match k {
                    "installed" => settings::AddKind::Installed,
                    "custom" => settings::AddKind::Custom,
                    "api" => settings::AddKind::Api,
                    _ => return Err("add_dialog must be installed, custom, api or false".into()),
                };
                settings::open_add(app);
                if let Some(a) = app.agent_chat.settings.add.as_mut() {
                    a.kind = kind;
                    if kind != settings::AddKind::Api {
                        a.name.clear();
                    }
                }
            }
            None => app.agent_chat.settings.add = None,
        }
    }
    if let Some(id) = p.get("default_agent").and_then(Value::as_str) {
        app.preferences.agent_default = id.to_string();
    }
    if let Some(m) = p.get("mode").and_then(Value::as_str) {
        set_mode(app, m);
    }
    if let Some(m) = p.get("model").and_then(Value::as_str) {
        set_model(app, m);
    }
    if !shown(app) {
        return Ok(status(app));
    }
    if let Some(o) = p.get("open").and_then(Value::as_bool) {
        app.agent_chat.open = o;
    }
    if let Some(s) = p.get("side").and_then(Value::as_str) {
        if !matches!(s, "left" | "right") {
            return Err("side must be left or right".into());
        }
        app.preferences.agent_chat_side = s.into();
    }
    if let Some(m) = p.get("minimized").and_then(Value::as_bool) {
        app.preferences.agent_chat_minimized = m;
    }
    if let Some(w) = p.get("popped").and_then(Value::as_bool) {
        app.preferences.agent_chat_popped = w;
    }
    if let Some(w) = p.get("width").and_then(Value::as_f64).filter(|w| w.is_finite()) {
        app.preferences.agent_chat_width = (w as f32).clamp(panel::MIN_WIDTH, panel::MAX_WIDTH);
        panel::forget_width(ctx);
    }
    if let Some(a) = p.get("attach_view").and_then(Value::as_bool) {
        app.agent_chat.attach_view = a;
    }
    if let Some(pick) = p.get("picker").and_then(Value::as_bool) {
        app.agent_chat.picking = Some(pick);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if p.get("refresh").and_then(Value::as_bool) == Some(true) {
            refresh(app, ctx);
        }
        if p.get("stop").and_then(Value::as_bool) == Some(true) {
            disconnect(app);
        }
        if p.get("connect").and_then(Value::as_bool) == Some(true) {
            connect(app, ctx);
        }
    }
    if let Some(text) = p.get("send").and_then(Value::as_str) {
        send(app, ctx, text.to_string());
    }
    if let Some(o) = p.get("permission").and_then(Value::as_str) {
        let Some(n) = app.agent_chat.items.iter().rposition(|i| matches!(i, Item::Permission { answer: None, .. })) else {
            return Err("no permission request is waiting".into());
        };
        answer_permission(app, n, (o != "cancel").then_some(o));
    }
    if let Some(a) = p.get("approve") {
        let Some(id) = app.agent_chat.held.first().map(|h| h.id) else { return Err("no request is waiting for approval".into()) };
        match (a.as_bool(), a.as_str()) {
            (Some(allow), _) => approve(app, id, allow),
            (_, Some("always")) => approve_always(app, id),
            _ => return Err("approve must be true, false or \"always\"".into()),
        }
    }
    if p.get("cancel").and_then(Value::as_bool) == Some(true) {
        cancel(app);
    }
    if p.get("new_chat").and_then(Value::as_bool) == Some(true) {
        new_chat(app);
    }
    if p.get("send_now").and_then(Value::as_bool) == Some(true) {
        send_now(app, ctx);
    }
    if let Some(n) = p.get("remove_pending").and_then(Value::as_u64) {
        remove_pending(app, n as usize);
    }
    if p.get("undo").and_then(Value::as_bool) == Some(true) {
        let Some(n) = app.agent_chat.items.iter().rposition(|i| matches!(i, Item::Changes { .. })) else { return Err("no changes to undo".into()) };
        undo_changes(app, n);
    }
    Ok(status(app))
}

pub fn status(app: &SolveApp) -> Value {
    let c = &app.agent_chat;
    let items: Vec<Value> = c
        .items
        .iter()
        .map(|i| match i {
            Item::User { text, image } => json!({"user": text, "image": image}),
            Item::Agent { thought, text } => json!({"agent": text, "thought": thought}),
            Item::Tool { id, title, kind, status, output } => {
                json!({"tool": id, "title": title, "kind": kind, "status": status.map(|s| format!("{s:?}")), "output": output})
            }
            Item::Plan(p) => json!({"plan": p}),
            Item::Permission { title, options, answer, .. } => {
                json!({"permission": title, "options": options.iter().map(|o| json!({"id": o.id, "name": o.name, "kind": o.kind})).collect::<Vec<_>>(), "answer": answer})
            }
            Item::Approval { title, answer, .. } => json!({"approval": title, "answer": answer}),
            Item::Changes { steps, undone, .. } => json!({"changes": steps, "undone": undone}),
            Item::Notice { text, error } => json!({"notice": text, "error": error}),
        })
        .collect();
    #[cfg(not(target_arch = "wasm32"))]
    let detection = c.detection.as_ref().map(|d| {
        json!({
            "found": d.found.iter().map(|f| json!({"id": f.id, "name": f.name, "argv": f.argv, "version": f.version, "login": f.login.id()})).collect::<Vec<_>>(),
            "missing": d.missing.iter().map(|m| json!({"id": m.id, "install": m.install})).collect::<Vec<_>>(),
            "ms": d.took_ms,
        })
    });
    #[cfg(target_arch = "wasm32")]
    let detection: Option<Value> = None;
    let pid = c.backend.as_ref().and_then(|b| b.pid());
    json!({
        "features": app.preferences.ai_features,
        "toolbarButton": app.preferences.ai_toolbar_button,
        "permissions": app.preferences.ai_permissions.id(),
        "agents": agents::entries(app).iter().map(|e| json!({"id": e.id, "name": e.name, "kind": if e.kind == agents::Kind::Api { "api" } else { "acp" }, "found": e.found, "version": e.version, "login": e.login})).collect::<Vec<_>>(),
        "currentAgent": agents::current(app).map(|e| e.id),
        "models": c.models.iter().map(|m| json!({"id": m.id, "name": m.name, "description": m.description})).collect::<Vec<_>>(),
        "model": c.model,
        "modes": c.modes.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
        "mode": c.mode,
        "open": c.open,
        "side": app.preferences.agent_chat_side,
        "minimized": app.preferences.agent_chat_minimized,
        "popped": app.preferences.agent_chat_popped,
        "window": app.preferences.agent_chat_window,
        "width": app.preferences.agent_chat_width,
        "state": c.state().id(),
        "agent": c.agent,
        "images": c.images,
        "authStatus": c.auth_status,
        "authMethods": c.auth.iter().map(|m| json!({"id": m.id, "name": m.name, "kind": match &m.kind { acp::AuthKind::Agent => "agent", acp::AuthKind::Terminal { .. } => "terminal", acp::AuthKind::EnvVar { .. } => "env_var" }})).collect::<Vec<_>>(),
        "toolsPort": c.tools_port,
        "pid": pid,
        "waiting": c.waiting(),
        "pending": c.pending.iter().map(|p| json!({"text": p.text, "image": p.image})).collect::<Vec<_>>(),
        "stopped": c.stopped,
        "turn": c.turn.as_ref().map(|t| json!({"changes": t.changes, "requests": t.requests})),
        "detection": detection,
        "items": items,
    })
}

#[cfg(test)]
mod tests;
