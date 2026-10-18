use super::*;
use std::sync::mpsc::channel;

fn chat() -> AgentChat {
    let mut c = AgentChat::default();
    c.state = Some(State::Ready);
    c
}

#[test]
fn streamed_text_joins_and_tool_updates_change_their_call() {
    let mut c = chat();
    for e in [
        Event::Text { thought: false, text: "Mak".into() },
        Event::Text { thought: false, text: "ing a box".into() },
        Event::Tool { id: "t".into(), title: Some("batch".into()), kind: Some("other".into()), status: Some(ToolStatus::Pending), output: None },
        Event::Tool { id: "t".into(), title: None, kind: None, status: Some(ToolStatus::Completed), output: Some("ok".into()) },
    ] {
        c.on_event(e);
    }
    assert_eq!(c.items.len(), 2);
    assert!(matches!(&c.items[0], Item::Agent { text, .. } if text == "Making a box"));
    assert!(matches!(&c.items[1], Item::Tool { title, status: Some(ToolStatus::Completed), output: Some(o), .. } if title == "batch" && o == "ok"));
}

#[test]
fn a_sign_in_request_keeps_the_question_for_after_the_login() {
    let mut c = chat();
    c.last = Some(("make a cube".into(), false));
    c.state = Some(State::Busy);
    assert!(c.on_event(Event::TurnEnded { reason: "auth".into() }).is_some());
    c.on_event(Event::AuthRequired { methods: vec![] });
    assert_eq!(c.state(), State::NeedsAuth);
    assert_eq!(c.queued, Some(("make a cube".into(), false)));
}

#[test]
fn a_plan_update_replaces_the_turns_plan() {
    let mut c = chat();
    c.push(Item::User { text: "go".into(), image: false });
    c.on_event(Event::Plan(vec![("a".into(), "pending".into())]));
    c.on_event(Event::Text { thought: false, text: "working".into() });
    c.on_event(Event::Plan(vec![("a".into(), "completed".into())]));
    let plans: Vec<_> = c.items.iter().filter(|i| matches!(i, Item::Plan(_))).collect();
    assert_eq!(plans.len(), 1);
    assert!(matches!(plans[0], Item::Plan(p) if p[0].1 == "completed"));
}

#[test]
fn a_permission_is_answered_once() {
    let mut c = chat();
    let opts = vec![PermissionOption { id: "allow".into(), name: "Allow".into(), kind: "allow_once".into() }];
    c.push(Item::Permission { key: 5, title: "batch".into(), options: opts, answer: None });
    assert_eq!(c.waiting(), 1);
    c.answer(0, Some("allow"));
    c.answer(0, None);
    assert!(matches!(&c.items[0], Item::Permission { answer: Some(a), .. } if a == "Allow"));
    assert_eq!(c.waiting(), 0);
}

/// An app with the chat's endpoint wired to a channel the test feeds, and a message in progress.
fn app_in_a_turn() -> (SolveApp, Sender<ControlRequest>) {
    let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
    // The default mode: modelling runs, files ask.
    assert_eq!(app.preferences.ai_permissions, agents::Mode::Edits);
    let (tx, rx) = channel();
    app.agent_chat.tools_rx = Some(rx);
    app.agent_chat.state = Some(State::Busy);
    start_turn(&mut app, "two boxes".into(), false);
    (app, tx)
}

fn ask(tx: &Sender<ControlRequest>, method: &str, params: Value) -> Receiver<Value> {
    let (req, rx) = ControlRequest::new(method, params);
    tx.send(req).unwrap();
    rx
}

fn frame(app: &mut SolveApp) {
    let ctx = egui::Context::default();
    poll(app, &ctx);
    app.drain_control(&ctx);
}

fn boxed(x: f64) -> Value {
    json!({"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0]}})
}

/// Everything the agent changed in one message undoes as one step, and the conversation offers
/// that undo while nothing else has happened since.
#[test]
fn a_messages_changes_undo_as_one_step() {
    let (mut app, tx) = app_in_a_turn();
    let before = app.session.doc.clone();
    let depth = app.session.undo.len();
    let replies: Vec<_> = [0.0, 20.0, 40.0].into_iter().map(|x| ask(&tx, "engine.execute", boxed(x))).collect();
    frame(&mut app);
    for r in replies {
        assert_eq!(r.recv_timeout(std::time::Duration::from_secs(30)).unwrap()["ok"], true);
    }
    assert_eq!(app.session.undo.len(), depth + 3);
    assert_eq!(app.agent_chat.turn.as_ref().map(|t| t.changes), Some(3));
    app.agent_chat.on_event(Event::TurnEnded { reason: String::new() });
    end_turn(&mut app);
    assert_eq!(app.session.undo.len(), depth + 1, "one step");
    assert!(app.session.undo.last().is_some_and(|s| s.label.contains("two boxes")));
    let n = app.agent_chat.items.iter().rposition(|i| matches!(i, Item::Changes { steps: 3, .. })).expect("the changes are listed");
    undo_changes(&mut app, n);
    assert!(Arc::ptr_eq(&app.session.doc, &before), "back to the design before the message");
    assert!(matches!(app.agent_chat.items[n], Item::Changes { undone: true, .. }));
}

/// Saving waits for Allow or Deny; Deny answers the agent without running it, Allow runs it.
#[test]
fn file_actions_wait_for_the_user() {
    let (mut app, tx) = app_in_a_turn();
    let dir = std::env::temp_dir().join(format!("sc-agent-chat-approve-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.solidcraft");
    let save = json!({"command": "file.save", "params": {"path": path.to_string_lossy()}, "allow_paths": [dir.to_string_lossy()]});
    let denied = ask(&tx, "engine.execute", save.clone());
    frame(&mut app);
    assert_eq!(app.agent_chat.waiting(), 1);
    assert!(denied.try_recv().is_err(), "nothing answered before the user decides");
    let id = app.agent_chat.held[0].id;
    approve(&mut app, id, false);
    let r = denied.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(r["error"].as_str().is_some_and(|e| e.contains("declined")), "{r}");
    assert!(!path.exists());
    let allowed = ask(&tx, "engine.execute", save);
    frame(&mut app);
    let id = app.agent_chat.held[0].id;
    approve(&mut app, id, true);
    frame(&mut app);
    let r = allowed.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Crash reports and app methods never run for the agent, requests outside a message don't
/// either, and a message stops taking changes at the limit.
#[test]
fn the_gate_refuses_and_limits() {
    let (mut app, tx) = app_in_a_turn();
    app.preferences.agent_chat_turn_limit = 2;
    let crash = ask(&tx, "engine.execute", json!({"command": "diagnostics.crash_reports", "params": {}}));
    let quit = ask(&tx, "app.quit", json!({"force": true}));
    let ok: Vec<_> = (0..2).map(|i| ask(&tx, "engine.execute", boxed(f64::from(i) * 20.0))).collect();
    let over = ask(&tx, "engine.execute", boxed(80.0));
    let read = ask(&tx, "document.inspect", json!({}));
    frame(&mut app);
    let get = |r: Receiver<Value>| r.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
    assert!(get(crash)["error"].as_str().is_some_and(|e| e.contains("crash reports")));
    assert!(get(quit)["error"].as_str().is_some_and(|e| e.contains("not available")));
    for r in ok {
        assert_eq!(get(r)["ok"], true);
    }
    assert!(get(over)["error"].as_str().is_some_and(|e| e.contains("limit of 2")));
    assert_eq!(get(read)["ok"], true, "reads go on past the limit");
    end_turn(&mut app);
    let late = ask(&tx, "engine.execute", boxed(100.0));
    frame(&mut app);
    assert!(get(late)["error"].as_str().is_some_and(|e| e.contains("no message in progress")));
}

#[test]
fn merging_finds_the_turns_start_when_the_history_was_trimmed() {
    // Distinct designs: what counts is which `Arc` a snapshot holds.
    let doc = |_: usize| Arc::new(solidcraft_engine::doc::Document::default());
    let snap = |d: &Arc<solidcraft_engine::doc::Document>| solidcraft_engine::Snapshot {
        label: String::new(),
        doc: d.clone(),
        active_sketch: None,
        pending_moves: Default::default(),
    };
    let docs: Vec<_> = (0..6).map(doc).collect();
    // The turn began at depth 3 with docs[3]; the history then lost its oldest step.
    let mut undo: Vec<_> = docs[1..6].iter().map(snap).collect();
    assert_eq!(merge_undo(&mut undo, &docs[3], 3, "Agent"), 3);
    assert_eq!(undo.len(), 3);
    assert!(Arc::ptr_eq(&undo[2].doc, &docs[3]) && undo[2].label == "Agent");
    // A start that is no longer in the history: nothing is merged.
    let mut undo: Vec<_> = docs[1..6].iter().map(snap).collect();
    assert_eq!(merge_undo(&mut undo, &doc(9), 3, "Agent"), 0);
    assert_eq!(undo.len(), 5);
}

/// People who don't want AI never see it: with "Show AI features" off there is no button, the
/// shortcut does nothing, no panel draws, nothing is looked for and nothing starts, even when
/// asked over the control channel. Turned on, the button is there; discovery waits for the panel.
#[test]
fn with_ai_features_off_nothing_of_the_chat_shows_or_runs() {
    use crate::scenario::{handle_at, rect_of};
    let mut h = crate::scenario::Harness::new();
    let shortcut = json!({"key": "A", "shift": true, "cmd": true});
    h.app.preferences.ai_features = false;
    h.frames(3);
    assert!(handle_at("toolbar:ai").is_none(), "no toolbar button");
    h.call("ui.key", shortcut.clone());
    h.frames(3);
    h.call("ui.agent_chat", json!({"open": true, "send": "hello", "connect": true}));
    h.frames(3);
    assert!(!h.app.agent_chat.open && rect_of("agent_chat:panel").is_none(), "no panel");
    assert!(h.app.agent_chat.detection.is_none() && h.app.agent_chat.detecting.is_none(), "nothing looked for");
    assert!(h.app.agent_chat.backend.is_none() && h.app.agent_chat.tools_port.is_none(), "nothing started");
    // The View settings menu has no chat entry either.
    if let Some(view) = handle_at("nav:view") {
        h.call("ui.click", json!({"x": view.x, "y": view.y}));
        h.frames(3);
        assert!(handle_at("view:agent_chat").is_none(), "no menu entry");
        h.call("ui.click", json!({"x": view.x, "y": view.y}));
    }
    // On (the default): the button shows; discovery waits until the panel opens.
    h.app.preferences.ai_features = true;
    h.frames(3);
    assert!(handle_at("toolbar:ai").is_some(), "the toolbar button");
    assert!(h.app.agent_chat.detection.is_none() && h.app.agent_chat.detecting.is_none(), "nothing looked for yet");
    h.call("ui.key", shortcut.clone());
    h.frames(3);
    assert!(h.app.agent_chat.open && rect_of("agent_chat:panel").is_some(), "Ctrl+Shift+A opens the panel");
    assert!(h.app.agent_chat.detection.is_some() || h.app.agent_chat.detecting.is_some(), "the panel looks for agents");
    assert!(h.app.agent_chat.backend.is_none(), "but starts none until a chat begins");
    // Without the toolbar button the shortcut still works.
    h.app.preferences.ai_toolbar_button = false;
    h.call("ui.key", shortcut);
    h.frames(3);
    assert!(handle_at("toolbar:ai").is_none() && !h.app.agent_chat.open);
}

#[test]
fn ai_features_and_the_toolbar_button_are_on_by_default() {
    let p = crate::prefs::Prefs::default();
    assert!(p.ai_features && p.ai_toolbar_button);
}

/// Pop out puts the chat in its own window (a native window on the desktop, which can go to
/// another screen; embedded here, where the test context has one viewport), and Dock puts it back
/// in the side panel. The control channel does the same.
#[test]
fn the_chat_pops_out_into_its_own_window_and_docks_back() {
    use crate::scenario::{handle_at, rect_of};
    let mut h = crate::scenario::Harness::new();
    h.call("ui.agent_chat", json!({"open": true}));
    h.frames(3);
    assert!(rect_of("agent_chat:panel").is_some() && rect_of("agent_chat:window").is_none(), "docked by default");
    assert!(!h.app.preferences.agent_chat_popped);
    let pop = handle_at("agent_chat:pop_out").expect("a pop-out button in the header");
    h.call("ui.click", json!({"x": pop.x, "y": pop.y}));
    h.frames(3);
    assert!(h.app.preferences.agent_chat_popped);
    assert!(rect_of("agent_chat:panel").is_none(), "the side panel is gone");
    assert!(rect_of("agent_chat:window").is_some(), "the chat is in its own window");
    assert_eq!(status(&h.app)["popped"], json!(true));
    // A window has its own minimise; the strip and the side drag are the docked panel's.
    assert!(handle_at("agent_chat:minimize").is_none() && handle_at("agent_chat:grip").is_none());
    let dock = handle_at("agent_chat:dock").expect("a dock button in the popped-out header");
    h.call("ui.click", json!({"x": dock.x, "y": dock.y}));
    h.frames(3);
    assert!(!h.app.preferences.agent_chat_popped);
    assert!(rect_of("agent_chat:panel").is_some() && rect_of("agent_chat:window").is_none(), "back in the side panel");
    h.call("ui.agent_chat", json!({"popped": true}));
    h.frames(3);
    assert!(rect_of("agent_chat:window").is_some() && rect_of("agent_chat:panel").is_none());
    // Closing the chat keeps where it lives: it opens popped out again.
    h.call("ui.agent_chat", json!({"open": false}));
    h.frames(2);
    assert!(rect_of("agent_chat:window").is_none());
    h.call("ui.agent_chat", json!({"open": true}));
    h.frames(3);
    assert!(rect_of("agent_chat:window").is_some());
}

/// The saved place is given to the window once, when it opens; after that the system owns it.
/// Given every frame, it chased the system's own rounding: a window that shook a pixel up and
/// down (686, 687, 686…) on a Retina screen.
#[test]
fn the_window_place_is_given_once_when_it_opens() {
    let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
    app.preferences.agent_chat_window = Some([1967.0, 160.0, 569.0, 686.0]);
    let first = panel::window_builder(&app);
    assert_eq!(first.position, Some(egui::pos2(1967.0, 160.0)));
    assert_eq!(first.inner_size, Some(egui::vec2(569.0, 686.0)));
    app.agent_chat.window_placed = true;
    app.preferences.agent_chat_window = Some([1967.0, 160.0, 569.0, 687.0]);
    let later = panel::window_builder(&app);
    assert_eq!((later.position, later.inner_size), (None, None), "nothing to move or resize once it is open");
    assert!(later.title.is_some());
    // Docked or closed, the next pop-out opens at the saved place again.
    let mut h = crate::scenario::Harness::new();
    h.call("ui.agent_chat", json!({"open": true, "popped": true}));
    h.frames(2);
    assert!(h.app.agent_chat.window_placed);
    h.call("ui.agent_chat", json!({"popped": false}));
    h.frames(2);
    assert!(!h.app.agent_chat.window_placed);
    h.call("ui.agent_chat", json!({"popped": true}));
    h.frames(2);
    h.call("ui.agent_chat", json!({"open": false}));
    h.frames(2);
    assert!(!h.app.agent_chat.window_placed);
}

/// macOS shows the text cursor only in the active window: the chat's own window becomes active
/// when the pointer comes over its text box from the app's main window, and never takes the focus
/// from another app.
#[test]
fn the_chat_window_takes_the_focus_for_its_text_box_from_the_main_window_only() {
    assert!(panel::take_focus_for_text(true, true, Some(false), Some(true)));
    assert!(!panel::take_focus_for_text(true, true, Some(false), Some(false)), "another app is active");
    assert!(!panel::take_focus_for_text(true, true, Some(true), Some(false)), "already active");
    assert!(!panel::take_focus_for_text(true, false, Some(false), Some(true)), "not over the text box");
    assert!(!panel::take_focus_for_text(false, true, Some(false), Some(true)), "docked: the main window shows it");
    assert!(!panel::take_focus_for_text(true, true, None, Some(true)), "unknown: leave it");
}

/// Whether the chat is popped out, and its window's place and size, are preferences; ones saved
/// before they existed load docked, and an unusable saved place is ignored.
#[test]
fn the_popped_out_window_and_its_place_are_remembered() {
    use crate::prefs::Prefs;
    let mut p = Prefs::default();
    assert!(!p.agent_chat_popped && p.agent_chat_window.is_none());
    p.agent_chat_popped = true;
    p.agent_chat_window = Some([-1600.0, 80.0, 420.0, 700.0]);
    let back: Prefs = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert!(back.agent_chat_popped);
    assert_eq!(back.agent_chat_window, Some([-1600.0, 80.0, 420.0, 700.0]), "a place on a screen left of the main one is kept");
    let mut old = serde_json::to_value(Prefs::default()).unwrap();
    old.as_object_mut().unwrap().remove("agent_chat_popped");
    old.as_object_mut().unwrap().remove("agent_chat_window");
    let old: Prefs = serde_json::from_value(old).unwrap();
    assert!(!old.agent_chat_popped && old.agent_chat_window.is_none());
    assert_eq!(panel::window_place(Some([10.0, 20.0, 400.0, 600.0])), Some(([10.0, 20.0], [400.0, 600.0])));
    for bad in [[f32::NAN, 0.0, 400.0, 600.0], [0.0, 0.0, 10.0, 600.0], [0.0, 0.0, 400.0, f32::INFINITY], [1e9, 0.0, 400.0, 600.0]] {
        assert_eq!(panel::window_place(Some(bad)), None, "{bad:?}");
    }
}

/// "Ask every time": a modelling command waits for Allow; "Always allow" lets the same command
/// through from then on for this agent. "Allow everything": even a save runs without a question.
#[test]
fn permission_modes_decide_what_asks() {
    let (mut app, tx) = app_in_a_turn();
    app.agent_chat.running_id = "claude".into();
    app.preferences.ai_permissions = agents::Mode::Ask;
    let first = ask(&tx, "engine.execute", boxed(0.0));
    frame(&mut app);
    assert_eq!(app.agent_chat.waiting(), 1, "a modelling command asks");
    let id = app.agent_chat.held[0].id;
    approve_always(&mut app, id);
    frame(&mut app);
    assert_eq!(first.recv_timeout(std::time::Duration::from_secs(30)).unwrap()["ok"], true);
    assert!(agents::always_allowed(&app.preferences, "claude", "solid.box"));
    let second = ask(&tx, "engine.execute", boxed(20.0));
    frame(&mut app);
    assert_eq!(app.agent_chat.waiting(), 0, "remembered");
    assert_eq!(second.recv_timeout(std::time::Duration::from_secs(30)).unwrap()["ok"], true);
    // Another agent keeps asking.
    app.agent_chat.running_id = "codex".into();
    let _third = ask(&tx, "engine.execute", boxed(40.0));
    frame(&mut app);
    assert_eq!(app.agent_chat.waiting(), 1);
    decline_all(&mut app, "test");
    // Allow everything, set for this agent only.
    app.preferences.agent_permissions.insert("codex".into(), agents::Mode::All);
    let dir = std::env::temp_dir().join(format!("sc-agent-chat-all-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("b.solidcraft");
    let save = ask(
        &tx,
        "engine.execute",
        json!({"command": "file.save", "params": {"path": path.to_string_lossy()}, "allow_paths": [dir.to_string_lossy()]}),
    );
    frame(&mut app);
    frame(&mut app);
    assert_eq!(app.agent_chat.waiting(), 0);
    assert_eq!(save.recv_timeout(std::time::Duration::from_secs(30)).unwrap()["ok"], true);
    // Crash reports stay off limits whatever the mode.
    let crash = ask(&tx, "engine.execute", json!({"command": "diagnostics.crash_reports"}));
    frame(&mut app);
    assert_eq!(crash.recv_timeout(std::time::Duration::from_secs(5)).unwrap()["ok"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The agent's own questions: SolidCraft's tools are allowed (its gate checks them), an earlier
/// "Always allow" answers by itself, and both leave a note; anything else is asked.
#[test]
fn the_agents_own_questions_are_answered_by_the_mode_or_asked() {
    let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
    app.agent_chat.running_id = "claude".into();
    let opts = || {
        vec![
            PermissionOption { id: "allow".into(), name: "Allow".into(), kind: "allow_once".into() },
            PermissionOption { id: "always".into(), name: "Always".into(), kind: "allow_always".into() },
            PermissionOption { id: "no".into(), name: "Reject".into(), kind: "reject_once".into() },
        ]
    };
    on_permission(&mut app, 1, "mcp__solidcraft__batch".into(), opts());
    assert!(matches!(app.agent_chat.items.last(), Some(Item::Notice { text, .. }) if text.contains("mcp__solidcraft__batch")));
    on_permission(&mut app, 2, "Run ls".into(), opts());
    assert!(matches!(app.agent_chat.items.last(), Some(Item::Permission { answer: None, .. })));
    let n = app.agent_chat.items.len() - 1;
    answer_permission(&mut app, n, Some("always"));
    assert!(agents::always_allowed(&app.preferences, "claude", "Run ls"));
    on_permission(&mut app, 3, "Run ls".into(), opts());
    assert!(matches!(app.agent_chat.items.last(), Some(Item::Notice { .. })), "remembered");
}

// ---- Messages typed while the agent works ----

use std::sync::Mutex;

/// A backend that records what the panel asks of it, and reports what the test pushes.
#[derive(Clone, Default)]
struct Script {
    calls: Arc<Mutex<Vec<String>>>,
    events: Arc<Mutex<Vec<Event>>>,
}

impl Script {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn report(&self, e: Event) {
        self.events.lock().unwrap().push(e);
    }
}

impl ChatBackend for Script {
    fn poll(&mut self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
    fn prompt(&mut self, parts: &[PromptPart]) {
        let text = parts.iter().find_map(|p| if let PromptPart::Text(t) = p { Some(t.clone()) } else { None }).unwrap_or_default();
        self.calls.lock().unwrap().push(format!("prompt: {text}"));
    }
    fn cancel(&mut self) {
        self.calls.lock().unwrap().push("cancel".into());
    }
    fn new_session(&mut self) {
        self.calls.lock().unwrap().push("new_session".into());
    }
    fn set_model(&mut self, model: &str) {
        self.calls.lock().unwrap().push(format!("set_model: {model}"));
    }
    fn answer_permission(&mut self, _: u64, _: Option<&str>) {}
    fn authenticate(&mut self, _: &str) {}
    fn shutdown(&mut self, _: bool) {}
}

/// An app whose agent is scripted and ready.
fn app_with_script() -> (SolveApp, Script, egui::Context) {
    let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
    let script = Script::default();
    app.agent_chat.backend = Some(Box::new(script.clone()));
    app.agent_chat.state = Some(State::Ready);
    (app, script, egui::Context::default())
}

fn ended(script: &Script, reason: &str) {
    script.report(Event::TurnEnded { reason: reason.into() });
}

fn texts(app: &SolveApp) -> Vec<String> {
    app.agent_chat.pending.iter().map(|p| p.text.clone()).collect()
}

/// While the agent works a message is queued, not dropped and not sent; it is not in the
/// transcript as a sent message, but waits as a pending one that can be edited and removed.
#[test]
fn a_message_typed_while_busy_is_queued() {
    let (mut app, script, ctx) = app_with_script();
    send(&mut app, &ctx, "make a box".into());
    assert_eq!(app.agent_chat.state(), State::Busy);
    send(&mut app, &ctx, "then a cylinder".into());
    send(&mut app, &ctx, "   ".into());
    send(&mut app, &ctx, "and a sphere".into());
    assert_eq!(texts(&app), ["then a cylinder", "and a sphere"]);
    assert_eq!(script.calls(), ["prompt: make a box"], "nothing else was sent");
    let users = app.agent_chat.items.iter().filter(|i| matches!(i, Item::User { .. })).count();
    assert_eq!(users, 1, "queued messages are not sent messages yet");
    // Remove one; edit the other.
    remove_pending(&mut app, 0);
    assert_eq!(texts(&app), ["and a sphere"]);
    app.agent_chat.pending[0].text = "and a cone".into();
    assert_eq!(app.agent_chat.waiting(), 0);
    assert!(status(&app)["pending"][0]["text"] == "and a cone");
}

/// When the turn ends normally the queue goes as the next prompt, all messages in order in one.
#[test]
fn the_queue_is_sent_when_the_turn_ends() {
    let (mut app, script, ctx) = app_with_script();
    send(&mut app, &ctx, "first".into());
    send(&mut app, &ctx, "second".into());
    send(&mut app, &ctx, "third".into());
    ended(&script, "");
    poll(&mut app, &ctx);
    assert_eq!(script.calls(), ["prompt: first", "prompt: second\n\nthird"]);
    assert!(app.agent_chat.pending.is_empty());
    assert_eq!(app.agent_chat.state(), State::Busy, "the next turn is under way");
    assert!(matches!(app.agent_chat.items.last(), Some(Item::User { text, .. }) if text == "second\n\nthird"));
    // Nothing queued: the end of the turn just ends it.
    ended(&script, "");
    poll(&mut app, &ctx);
    assert_eq!(app.agent_chat.state(), State::Ready);
    assert_eq!(script.calls().len(), 2);
}

/// Stop does not send the queue by itself; it stays, with a Send that sends it later.
#[test]
fn stop_keeps_the_queue() {
    let (mut app, script, ctx) = app_with_script();
    send(&mut app, &ctx, "first".into());
    send(&mut app, &ctx, "later".into());
    cancel(&mut app);
    assert_eq!(script.calls(), ["prompt: first", "cancel"]);
    ended(&script, "cancelled");
    poll(&mut app, &ctx);
    assert_eq!(app.agent_chat.state(), State::Ready);
    assert_eq!(texts(&app), ["later"], "still queued");
    assert_eq!(script.calls().len(), 2, "not sent");
    // Send (the bubble's button) sends it, without the interruption note.
    send_now(&mut app, &ctx);
    assert_eq!(script.calls(), ["prompt: first", "cancel", "prompt: later"]);
    assert!(app.agent_chat.pending.is_empty());
}

/// Send now: cancel first, the prompt only after the agent has ended the cancelled turn, with a
/// note that the previous step was interrupted (shown without it in the conversation).
#[test]
fn send_now_cancels_then_sends_with_a_note() {
    let (mut app, script, ctx) = app_with_script();
    send(&mut app, &ctx, "build a gearbox".into());
    send(&mut app, &ctx, "use 12 teeth instead".into());
    send_now(&mut app, &ctx);
    assert_eq!(script.calls(), ["prompt: build a gearbox", "cancel"], "cancel alone, the prompt waits for the end");
    poll(&mut app, &ctx);
    assert_eq!(script.calls().len(), 2);
    ended(&script, "cancelled");
    poll(&mut app, &ctx);
    assert_eq!(script.calls(), ["prompt: build a gearbox".to_string(), "cancel".into(), format!("prompt: {INTERRUPT_NOTE}\n\nuse 12 teeth instead")]);
    assert!(matches!(app.agent_chat.items.last(), Some(Item::User { text, .. }) if text == "use 12 teeth instead"));
    // Stop after Send now wins: nothing more is sent by itself.
    send(&mut app, &ctx, "one more".into());
    send_now(&mut app, &ctx);
    cancel(&mut app);
    ended(&script, "cancelled");
    poll(&mut app, &ctx);
    assert_eq!(texts(&app), ["one more"]);
}

/// The steered message also goes after a turn that ended by itself just before the cancel arrived.
#[test]
fn send_now_after_the_turn_already_ended_still_sends() {
    let (mut app, script, ctx) = app_with_script();
    send(&mut app, &ctx, "a".into());
    send(&mut app, &ctx, "b".into());
    send_now(&mut app, &ctx);
    ended(&script, "");
    poll(&mut app, &ctx);
    assert_eq!(script.calls().last().map(String::as_str), Some("prompt: b"), "no note: nothing was interrupted");
}

/// What an interrupted turn changed undoes as one step, and the steered message starts its own.
#[test]
fn an_interrupted_turns_changes_undo_as_one_step() {
    let (mut app, script, ctx) = app_with_script();
    let (tx, rx) = channel();
    app.agent_chat.tools_rx = Some(rx);
    let before = app.session.doc.clone();
    let depth = app.session.undo.len();
    send(&mut app, &ctx, "two boxes".into());
    let replies: Vec<_> = [0.0, 20.0].into_iter().map(|x| ask(&tx, "engine.execute", boxed(x))).collect();
    frame(&mut app);
    for r in replies {
        assert_eq!(r.recv_timeout(std::time::Duration::from_secs(30)).unwrap()["ok"], true);
    }
    send(&mut app, &ctx, "stop, make it one".into());
    send_now(&mut app, &ctx);
    ended(&script, "cancelled");
    poll(&mut app, &ctx);
    assert_eq!(app.session.undo.len(), depth + 1, "the interrupted message is one step");
    let n = app.agent_chat.items.iter().rposition(|i| matches!(i, Item::Changes { steps: 2, .. })).expect("its changes are listed");
    // The steered message is on its own turn.
    assert!(app.agent_chat.turn.as_ref().is_some_and(|t| t.label.contains("stop, make it one") && t.changes == 0));
    undo_changes(&mut app, n);
    assert!(Arc::ptr_eq(&app.session.doc, &before));
}

/// The same with a real agent process speaking ACP: `session/cancel` goes out, the agent answers
/// the held prompt with the cancelled stop reason, and only then does the queued text go as a new
/// `session/prompt`.
#[cfg(unix)]
#[test]
fn send_now_with_an_acp_agent_sends_cancel_then_the_prompt() {
    let dir = std::env::temp_dir().join(format!("sc-agent-chat-steer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("received.log");
    // A fake agent: answers initialize and session/new; holds a prompt whose text says "slow"
    // until it gets session/cancel (then answers it "cancelled"); answers other prompts at once.
    let script = r#"
log="$1"; held=""
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"fake","version":"1"}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"s1"}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      case "$line" in
        *slow*) held="$id" ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
      esac ;;
    *'"method":"session/cancel"'*)
      if [ -n "$held" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"cancelled"}}\n' "$held"; held=""; fi ;;
  esac
done
"#;
    let argv = vec!["sh".to_string(), "-c".to_string(), script.to_string(), "fake-agent".to_string(), log.to_string_lossy().into_owned()];
    let start = backend::StartContext { cwd: dir.clone(), bridge: None, tools: None, wake: Box::new(|| {}) };
    let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
    app.agent_chat.backend = Some(Box::new(acp_client::AcpBackend::start(&argv, start).unwrap()));
    app.agent_chat.state = Some(State::Starting);
    let ctx = egui::Context::default();
    let until = |app: &mut SolveApp, what: &str, done: &dyn Fn(&SolveApp) -> bool| {
        let t = std::time::Instant::now();
        while !done(app) {
            assert!(t.elapsed() < std::time::Duration::from_secs(20), "timed out waiting for {what}");
            poll(app, &ctx);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    until(&mut app, "the agent to be ready", &|a| a.agent_chat.state() == State::Ready);
    send(&mut app, &ctx, "slow job".into());
    send(&mut app, &ctx, "do this instead".into());
    assert_eq!(texts(&app), ["do this instead"]);
    send_now(&mut app, &ctx);
    until(&mut app, "the steered turn to finish", &|a| a.agent_chat.state() == State::Ready && a.agent_chat.pending.is_empty());
    let methods: Vec<String> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            let m = v["method"].as_str()?.to_string();
            let text = v.pointer("/params/prompt/0/text").and_then(Value::as_str).unwrap_or("");
            Some(if text.is_empty() { m } else { format!("{m}: {text}") })
        })
        .collect();
    assert_eq!(
        methods,
        [
            "initialize".to_string(),
            "session/new".into(),
            "session/prompt: slow job".into(),
            "session/cancel".into(),
            format!("session/prompt: {INTERRUPT_NOTE}\n\ndo this instead"),
        ]
    );
    disconnect(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// New chat: the running message is stopped, the transcript and the queue are cleared, a fresh
/// session starts on the same agent, the mode stays, and the design's undo history is untouched.
#[test]
fn new_chat_clears_everything_and_keeps_the_agent_and_mode() {
    let (mut app, script, ctx) = app_with_script();
    app.agent_chat.modes = vec![("plan".into(), "Plan".into()), ("code".into(), "Code".into())];
    app.agent_chat.mode = "plan".into();
    app.agent_chat.agent_id = Some("claude".into());
    let depth = app.session.undo.len();
    assert!(!new_chat_needs_confirm(&app));
    send(&mut app, &ctx, "first".into());
    send(&mut app, &ctx, "queued".into());
    assert!(new_chat_needs_confirm(&app), "a running message and a queue ask first");
    new_chat(&mut app);
    assert_eq!(script.calls(), ["prompt: first", "cancel", "new_session"]);
    assert!(app.agent_chat.items.is_empty() && app.agent_chat.pending.is_empty());
    assert_eq!(app.agent_chat.state(), State::Starting);
    assert_eq!(app.agent_chat.agent_id.as_deref(), Some("claude"));
    // The cancelled message's late end is not reported; the fresh session is ready again.
    ended(&script, "cancelled");
    script.report(Event::Modes { current: "code".into(), available: vec![] });
    script.report(Event::Ready { agent: "fake".into(), images: false });
    poll(&mut app, &ctx);
    assert_eq!(app.agent_chat.state(), State::Ready);
    assert!(app.agent_chat.items.is_empty(), "no notice about the old message");
    assert_eq!(app.agent_chat.mode, "plan", "the mode is kept");
    assert_eq!(app.session.undo.len(), depth);
    assert!(!new_chat_needs_confirm(&app));
}

/// With a real ACP agent, New chat sends `session/new` on the same process (no restart).
#[cfg(unix)]
#[test]
fn new_chat_with_an_acp_agent_opens_a_new_session_on_the_same_process() {
    let dir = std::env::temp_dir().join(format!("sc-agent-chat-new-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("received.log");
    let script = r#"
log="$1"; n=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"agentInfo":{"name":"fake","version":"1"}}}\n' "$id" ;;
    *'"method":"session/new"'*) n=$((n+1)); printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"s%s"}}\n' "$id" "$n" ;;
    *'"method":"session/prompt"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
  esac
done
"#;
    let argv = vec!["sh".to_string(), "-c".to_string(), script.to_string(), "fake-agent".to_string(), log.to_string_lossy().into_owned()];
    let start = backend::StartContext { cwd: dir.clone(), bridge: None, tools: None, wake: Box::new(|| {}) };
    let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
    app.agent_chat.backend = Some(Box::new(acp_client::AcpBackend::start(&argv, start).unwrap()));
    app.agent_chat.state = Some(State::Starting);
    let ctx = egui::Context::default();
    let until = |app: &mut SolveApp, done: &dyn Fn(&SolveApp) -> bool| {
        let t = std::time::Instant::now();
        while !done(app) {
            assert!(t.elapsed() < std::time::Duration::from_secs(20), "timed out");
            poll(app, &ctx);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    until(&mut app, &|a| a.agent_chat.state() == State::Ready);
    send(&mut app, &ctx, "hello".into());
    until(&mut app, &|a| a.agent_chat.state() == State::Ready);
    assert!(!app.agent_chat.items.is_empty());
    let pid = app.agent_chat.backend.as_ref().and_then(|b| b.pid());
    new_chat(&mut app);
    assert!(app.agent_chat.items.is_empty());
    until(&mut app, &|a| a.agent_chat.state() == State::Ready);
    assert_eq!(app.agent_chat.backend.as_ref().and_then(|b| b.pid()), pid, "same process");
    send(&mut app, &ctx, "again".into());
    until(&mut app, &|a| a.agent_chat.state() == State::Ready);
    let log_text = std::fs::read_to_string(&log).unwrap();
    let methods: Vec<String> =
        log_text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter_map(|v| v["method"].as_str().map(String::from)).collect();
    assert_eq!(methods, ["initialize", "session/new", "session/prompt", "session/new", "session/prompt"]);
    assert!(log_text.lines().last().is_some_and(|l| l.contains("\"s2\"")), "the prompt goes to the new session: {log_text}");
    disconnect(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

fn models_reply(current: &str) -> Event {
    let m = |id: &str| acp::ModelInfo { id: id.into(), name: id.to_uppercase(), description: String::new() };
    Event::Models { current: current.into(), available: vec![m("fast"), m("smart")] }
}

/// Choosing a model tells the agent, shows it as current and remembers it for this agent; a model
/// the agent did not offer is ignored.
#[test]
fn choosing_a_model_sets_it_and_remembers_it() {
    let (mut app, script, ctx) = app_with_script();
    app.agent_chat.running_id = "claude".into();
    script.report(models_reply("fast"));
    poll(&mut app, &ctx);
    assert!(models_shown(&app));
    assert_eq!(app.agent_chat.model, "fast");
    set_model(&mut app, "smart");
    set_model(&mut app, "nonsense");
    assert_eq!(script.calls(), ["set_model: smart"]);
    assert_eq!(app.agent_chat.model, "smart");
    assert_eq!(app.preferences.agent_models.get("claude").map(String::as_str), Some("smart"));
    assert_eq!(status(&app)["model"], "smart");
    // The agent switching its own model is shown too.
    script.report(Event::Models { current: "fast".into(), available: vec![] });
    poll(&mut app, &ctx);
    assert_eq!(app.agent_chat.model, "fast");
    assert_eq!(app.agent_chat.models.len(), 2, "the list stays");
}

/// The next session puts back the model chosen last time, when the agent offers it.
#[test]
fn the_remembered_model_is_set_on_the_next_session() {
    let (mut app, script, ctx) = app_with_script();
    app.agent_chat.running_id = "claude".into();
    app.preferences.agent_models.insert("claude".into(), "smart".into());
    script.report(models_reply("fast"));
    script.report(Event::Ready { agent: "fake".into(), images: false });
    poll(&mut app, &ctx);
    assert_eq!(script.calls(), ["set_model: smart"]);
    assert_eq!(app.agent_chat.model, "smart");
    // Already on it, or no longer offered: nothing is sent.
    script.report(models_reply("smart"));
    script.report(Event::Ready { agent: "fake".into(), images: false });
    poll(&mut app, &ctx);
    app.preferences.agent_models.insert("claude".into(), "gone".into());
    script.report(models_reply("fast"));
    script.report(Event::Ready { agent: "fake".into(), images: false });
    poll(&mut app, &ctx);
    assert_eq!(script.calls().len(), 1);
}

/// An agent that reports no models gets no picker; another agent's choice is not applied to it.
#[test]
fn an_agent_without_models_shows_no_picker() {
    let (mut app, script, ctx) = app_with_script();
    app.agent_chat.running_id = "other".into();
    app.preferences.agent_models.insert("claude".into(), "smart".into());
    script.report(Event::Ready { agent: "fake".into(), images: false });
    poll(&mut app, &ctx);
    assert!(!models_shown(&app));
    assert!(script.calls().is_empty());
    assert_eq!(status(&app)["models"], json!([]));
}

/// With a real ACP agent the choice goes out as `session/set_model` with the session and model.
#[cfg(unix)]
#[test]
fn set_model_goes_to_an_acp_agent() {
    let dir = std::env::temp_dir().join(format!("sc-agent-chat-model-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("received.log");
    let script = r#"
log="$1"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"agentInfo":{"name":"fake","version":"1"}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"s1","models":{"currentModelId":"fast","availableModels":[{"modelId":"fast","name":"Fast"},{"modelId":"smart","name":"Smart"}]}}}\n' "$id" ;;
    *'"method":"session/set_model"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
  esac
done
"#;
    let argv = vec!["sh".to_string(), "-c".to_string(), script.to_string(), "fake-agent".to_string(), log.to_string_lossy().into_owned()];
    let start = backend::StartContext { cwd: dir.clone(), bridge: None, tools: None, wake: Box::new(|| {}) };
    let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
    app.agent_chat.backend = Some(Box::new(acp_client::AcpBackend::start(&argv, start).unwrap()));
    app.agent_chat.state = Some(State::Starting);
    app.agent_chat.running_id = "fake".into();
    app.preferences.agent_models.insert("fake".into(), "smart".into());
    let ctx = egui::Context::default();
    let t = std::time::Instant::now();
    loop {
        poll(&mut app, &ctx);
        let logged = std::fs::read_to_string(&log).unwrap_or_default();
        if logged.contains("session/set_model") {
            break;
        }
        assert!(t.elapsed() < std::time::Duration::from_secs(20), "no set_model: {logged}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(models_shown(&app) && app.agent_chat.model == "smart", "the remembered model was put back");
    let logged = std::fs::read_to_string(&log).unwrap();
    let set: Value = logged.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v["method"] == "session/set_model").unwrap();
    assert_eq!(set["params"], json!({"sessionId": "s1", "modelId": "smart"}));
    // A choice made in the picker goes out too.
    set_model(&mut app, "fast");
    let t = std::time::Instant::now();
    while std::fs::read_to_string(&log).unwrap_or_default().matches("session/set_model").count() < 2 {
        assert!(t.elapsed() < std::time::Duration::from_secs(20));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    disconnect(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An agent's text is Markdown: a code block shows with its Copy button and a link is a button
/// of its own, instead of the raw markers.
#[test]
fn an_agents_markdown_is_drawn_as_blocks() {
    let mut h = crate::scenario::Harness::new();
    h.call("ui.agent_chat", json!({"open": true}));
    h.frames(3);
    h.app.agent_chat.items.push(Item::Agent {
        thought: false,
        text: "# Result\n\n- a **bold** point\n- another\n\n```\nlet x = 1;\n```\n\nSee [the docs](https://example.com/a).".into(),
    });
    h.frames(3);
    assert!(h.widget("Copy").is_some(), "the code block has its Copy button");
    assert!(h.widget("↗ example.com").is_some(), "the link is a button that names where it goes");
    // A hostile reply draws too (no panic, bounded work).
    h.app.agent_chat.items.push(Item::Agent { thought: false, text: "*".repeat(50_000) + &"[".repeat(5_000) + &"`a".repeat(5_000) });
    h.frames(3);
}
