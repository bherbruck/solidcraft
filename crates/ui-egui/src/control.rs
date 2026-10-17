//! Programmatic control of the running app (agents, tests).
//!
//! Methods (JSON lines over the host's transport, `{"id", "method", "params"}`):
//! - `engine.execute {command, params}`: run any command with JSON parameters (no dialogs)
//! - `engine.script {commands: [...]}`: run a command script
//! - `engine.batch {commands: [...], rollback?}`: the steps in one request, each as
//!   `engine.execute` runs it; stops at the first failure and goes back to the checkpoint
//!   (per-step results, `failed`, `undone`). Refused (`engine.batch unavailable: …`) while the
//!   agent cursor is shown or when a step loads a file
//! - `engine.commands`: every command with tab, panel, params and enablement
//! - `document.inspect {measure?}`: the design (parameters, timeline, bodies, sketches)
//! - `ui.inspect`: UI state, viewport rect, camera, tool and dialog
//! - `ui.drag {x0, y0, x1, y1, button?, shift?, ctrl?, steps?, hold?}` (box selection, navigation;
//!   `hold` keeps the button down that many frames before moving);
//!   `ui.selection` (selection, dialog inputs, hover, drawn sketch dimensions);
//!   `ui.sketchToScreen {points: [[x,y]…]}` (screen points of active-sketch coordinates); `ui.editFeature {feature}` (edit dialog)
//! - `ui.set {...UiState fields}`; `ui.view {view: front|back|top|bottom|left|right|iso|home|fit, animate?: bool}` (snaps unless animate)
//! - `ui.start {command}`: like clicking the toolbar button (starts tools/dialogs)
//! - `ui.click {x, y, button?, shift?}`, `ui.move {x, y}`: real
//!   pointer input in screen points; `ui.scroll {x, y, delta}`: mouse-wheel input in
//!   points (zooms); `ui.key {key, cmd?, shift?}`, `ui.text {text}`
//! - `ui.screenshot {path?}`: PNG of the window; `ui.render {path, width?, height?, grid?, axes?}`:
//!   headless CPU render of the model with the current camera, in the window's theme, with the
//!   section the viewport shows (no window needed); the ground grid and the origin's axes as the
//!   window shows them, unless `grid` or `axes` says
//! - `ui.menu {x?, y?, target?, close?}`: open a context menu at a point (the viewport's marking
//!   menu, or a browser menu with `target: {type: body|sketch|component, …}`); without a point,
//!   the open menu and its items. `ui.menuPick {item}` runs an item by id or label;
//!   `ui.rename {text?, commit?}` finishes the rename box
//! - `ui.window {action: minimize|maximize|restore|toggle|close}`: what the title bar's buttons do
//! - `ui.confirm {accept?}`: the Delete confirmation (without `accept`: what it lists)
//! - `ui.shortcut {command, key?, replace?}`: a command's key, or bind one ("" clears)
//! - `ui.help {item?: about|shortcuts|report}`: the Help menu; returns the version and diagnostics
//! - `ui.prefs {...}`: the preferences (default_units, nav, zoom_reverse, orbit_around, msaa,
//!   length_decimals, angle_decimals); returns them all
//! - `ui.capture {action?: capture|revert|cancel}`: the Capture Position question
//! - `ui.agent_cursor {show?, speed?: 1–5 (times) | "instant", follow_camera?}`: the agent cursor
//!   (#35); while it is shown, `engine.execute` first glides to the command's button and picks
//!   and replies once the command has run (`"animate": false` skips that one call)
//! - `ui.resize {width, height}`, `app.quit`

use std::sync::mpsc::Sender;

use serde_json::{Value, json};
use solidcraft_engine::io::vfs;
use solidcraft_engine::render::StandardView;
use solidcraft_engine::timing;

use crate::SolveApp;

pub type ControlResponse = Value;

pub struct ControlRequest {
    pub method: String,
    pub params: Value,
    pub reply: Sender<ControlResponse>,
}

impl ControlRequest {
    pub fn new(method: impl Into<String>, params: Value) -> (Self, std::sync::mpsc::Receiver<ControlResponse>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Self { method: method.into(), params, reply: tx }, rx)
    }
}

pub enum Outcome {
    Done(Value),
    Screenshot { path: Option<String> },
}

fn ok(v: Value) -> Outcome {
    Outcome::Done(json!({"ok": true, "result": v}))
}
fn err(e: impl std::fmt::Display) -> Outcome {
    Outcome::Done(json!({"ok": false, "error": e.to_string()}))
}
fn wrap(r: Result<Value, String>) -> Outcome {
    match r {
        Ok(v) => ok(v),
        Err(e) => err(e),
    }
}

fn key_from(name: &str) -> Option<egui::Key> {
    egui::Key::from_name(name).or(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Some(egui::Key::Enter),
        "esc" | "escape" => Some(egui::Key::Escape),
        "delete" => Some(egui::Key::Delete),
        "space" => Some(egui::Key::Space),
        "tab" => Some(egui::Key::Tab),
        _ => None,
    })
}

/// Run one command as `engine.execute` does: through `app.run`, then the first-body fit and the
/// camera the command asked for.
fn execute(app: &mut SolveApp, id: &str, params: Value) -> Result<Value, String> {
    let r = app.run(id, params);
    if r.is_ok()
        && id != "view.navigate"
        && app.session.active_sketch.is_none()
        && app.session.world_state().bodies.len() == 1
        && app.session.undo.len() <= 2
    {
        app.fit_view();
    }
    // After the first-body fit, so a camera the command asked for (`view.apply`) wins.
    app.follow_view_request(false);
    r
}

/// A batch step's command id.
fn command(step: &Value) -> Option<&str> {
    step.get("command").or_else(|| step.get("id")).and_then(Value::as_str)
}

/// Most steps in one `engine.batch` (as in the MCP `batch` tool).
const MAX_BATCH: usize = 10_000;

/// Commands that load a file: through `engine.execute` they go to the app's file loader
/// (`file_load::control`: off the window thread, with progress and Cancel), which a batch would
/// bypass.
const LOADS_A_FILE: [&str; 3] = ["doc.open", "file.insert_step", "file.insert_mesh"];

/// `engine.batch {commands, rollback?, timings?, profile?}`: the steps in one request, so in one
/// frame (#743). With `timings` the reply adds each step's time and `total_ms`, with `profile`
/// each step's kernel share too (`solidcraft_engine::timing`, #1833). Each
/// step runs as `engine.execute` runs it; the first failure stops the batch and, with
/// `rollback` (the default), the design goes back to its checkpoint. Step failures are part of
/// the reply; refusals (`engine.batch unavailable: …`) come before anything runs, so the client
/// can send the steps one by one instead.
fn batch(app: &mut SolveApp, p: &Value) -> Outcome {
    let Some(steps) = p.get("commands").and_then(Value::as_array).filter(|s| s.len() <= MAX_BATCH) else {
        return err(format!("engine.batch: commands must be an array of at most {MAX_BATCH} steps"));
    };
    // The agent cursor acts out each `engine.execute` on its own.
    if app.ui.agent_cursor.show {
        return err("engine.batch unavailable: the agent cursor is shown");
    }
    if let Some((i, c)) = steps.iter().enumerate().find_map(|(i, step)| command(step).filter(|c| LOADS_A_FILE.contains(c)).map(|c| (i, c))) {
        return err(format!("engine.batch unavailable: step {i} ({c}) loads a file"));
    }
    let rollback = p.get("rollback").and_then(Value::as_bool).unwrap_or(true);
    let profile = p.get("profile").and_then(Value::as_bool).unwrap_or(false);
    let timed = profile || p.get("timings").and_then(Value::as_bool).unwrap_or(false);
    let mut rows = Vec::new();
    let clock = timing::Clock::start();
    let checkpoint = if rollback { app.run("edit.checkpoint", json!({})).ok().and_then(|v| v["checkpoint"].as_u64()) } else { None };
    let mut results = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let (r, took) = timing::time(profile, || match command(step) {
            Some(id) => execute(app, id, step.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}))),
            None => Err("missing `command`".to_string()),
        });
        if timed {
            rows.push(took.to_json(i, command(step).unwrap_or_default()));
        }
        match r {
            Ok(v) => results.push(v),
            Err(e) => {
                let restored = checkpoint.is_some_and(|id| app.run("edit.restore_checkpoint", json!({"checkpoint": id})).is_ok());
                let failed = json!({"index": i, "command": command(step), "error": e});
                let reply = json!({"completed": i, "results": results, "failed": failed, "undone": if restored { i } else { 0 }});
                return ok(with_timings(reply, timed, rows, clock.ms()));
            }
        }
    }
    if let Some(id) = checkpoint {
        let _ = app.run("edit.restore_checkpoint", json!({"checkpoint": id, "forget": true}));
    }
    ok(with_timings(json!({"completed": results.len(), "results": results, "undone": 0}), timed, rows, clock.ms()))
}

/// A batch reply with `timings` and `total_ms` when they were asked for.
fn with_timings(mut reply: Value, timed: bool, rows: Vec<Value>, total_ms: f64) -> Value {
    if timed && let Some(o) = reply.as_object_mut() {
        o.insert("timings".into(), Value::Array(rows));
        o.insert("total_ms".into(), json!(timing::round_ms(total_ms)));
    }
    reply
}

/// A request from an MCP server carries the folders its agent may use (`allow_paths`); every
/// file the request reaches must lie inside them (docs/mcp.md). `None`: it names none.
pub fn request_scope(app: &SolveApp, params: &Value) -> Result<Option<vfs::PathScope>, String> {
    let Some(allowed) = params.get("allow_paths") else { return Ok(None) };
    let Some(roots) = allowed.as_array().and_then(|a| a.iter().map(|r| r.as_str().map(std::path::PathBuf::from)).collect::<Option<Vec<_>>>()) else {
        return Err("`allow_paths` must be a list of folders".into());
    };
    // Plus the app's own recovery folder and, when it is a safe one, the open design's folder.
    let roots = roots.into_iter().chain(solidcraft_engine::recovery::default_dir());
    Ok(Some(vfs::PathScope::new(roots).with_design(app.session.path.as_deref())))
}

/// What a picture of the view shows: the model as the window shows it (theme and colour vision
/// mode, the section the viewport shows: an open Section Analysis dialog's preview, a sketch's
/// Slice), with the grid and the origin's axes as `guides` says.
pub(crate) fn view_scene(app: &SolveApp, guides: solidcraft_engine::view::Guides) -> solidcraft_engine::render::Scene {
    solidcraft_engine::view::scene_with_palette_and_guides(
        &app.session,
        &app.cam,
        crate::viewport::section_plane(app),
        solidcraft_engine::view::world_curve_parts,
        &crate::viewport::scene_palette(app.ui.dark, app.preferences.color_vision),
        guides,
    )
}

/// The grid and the origin's axes as the window shows them (the Grid and Origin switches).
pub(crate) fn window_guides(app: &SolveApp) -> solidcraft_engine::view::Guides {
    solidcraft_engine::view::Guides { grid: app.ui.show_grid && !crate::viewport::active_view(app).hide_grid, axes: app.origin_visible() }
}

/// A picture of the view as the window shows it, RGBA, drawn on the GPU with the viewport's
/// pipelines (the renderer the app installs).
pub(crate) fn view_rgba(app: &SolveApp, w: u32, h: u32) -> Result<Vec<u8>, String> {
    solidcraft_engine::capture::rgba(&view_scene(app, window_guides(app)), &app.cam, w, h).map(|p| p.rgba)
}

/// [`view_rgba`] as PNG.
pub fn view_png(app: &SolveApp, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let rgba = view_rgba(app, w, h)?;
    solidcraft_engine::render::png(w as usize, h as usize, rgba).ok_or_else(|| "PNG encoding failed".to_string())
}

pub fn handle(app: &mut SolveApp, ctx: &egui::Context, req: &ControlRequest) -> Outcome {
    let scope = match request_scope(app, &req.params) {
        Ok(Some(scope)) => scope,
        Ok(None) => return dispatch(app, ctx, &req.method, &req.params),
        Err(e) => return err(e),
    };
    let mut p = req.params.clone();
    if let Some(o) = p.as_object_mut() {
        o.remove("allow_paths");
    }
    vfs::scoped(Some(scope), || dispatch(app, ctx, &req.method, &p))
}

fn dispatch(app: &mut SolveApp, ctx: &egui::Context, method: &str, p: &Value) -> Outcome {
    crate::render_workspace::sync_backend(app);
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    let f = |k: &str| p.get(k).and_then(Value::as_f64).filter(|x| x.is_finite());
    let b = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(false);
    match method {
        "ui.workspace" => {
            if let Some(workspace) = s("workspace") {
                if !matches!(workspace, "design" | "render") {
                    return err("workspace must be design or render");
                }
                if let Err(e) = crate::render_workspace::enter(app, workspace == "render") {
                    return err(e);
                }
            }
            ok(json!({"workspace":if crate::render_workspace::active(app) {"render"} else {"design"}}))
        }
        "engine.execute" | "command" => {
            let Some(id) = s("command").or(s("id")) else { return err("missing `command`") };
            wrap(execute(app, id, p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}))))
        }
        "engine.batch" => batch(app, p),
        "ui.agent_cursor" => {
            let s = &mut app.ui.agent_cursor;
            if let Some(v) = p.get("show").and_then(Value::as_bool) {
                s.show = v;
            }
            if let Some(v) = p.get("follow_camera").and_then(Value::as_bool) {
                s.follow_camera = v;
            }
            if let Some(v) = p.get("speed") {
                match crate::agent_cursor::Speed::from_value(v) {
                    Some(sp) => s.speed = sp,
                    None => return err("speed must be a number from 1 to 5, or \"instant\""),
                }
            }
            ok(json!(app.ui.agent_cursor))
        }
        "engine.script" => {
            let r = app.session.run_script(p).map(|v| json!(v)).map_err(|e| e.to_string());
            app.follow_view_request(false);
            wrap(r)
        }
        "engine.commands" => wrap(app.session.execute("engine.commands", &json!({})).map_err(|e| e.to_string())),
        "document.inspect" => wrap(app.session.execute("document.inspect", p).map_err(|e| e.to_string())),
        "ui.inspect" => ok(json!({
            "ui": serde_json::to_value(&app.ui).unwrap_or_default(),
            "visibility": app.session.visibility,
            "viewport": app.viewport.rect.map(|r| json!([r.left(), r.top(), r.width(), r.height()])),
            "camera": app.cam,
            "tool": app.tool.as_ref().map(|t| t.cmd),
            "dialog": app.dialog.as_ref().map(|d| format!("{d:?}")),
            "status": app.status.as_ref().map(|(text, _, error)| json!({"text": text, "error": error})),
            "hover": app.viewport.hover.as_ref().map(|h| format!("{h:?}")),
            "renderer": if app.viewport.gpu.is_some() { "gpu" } else { "cpu" },
            "frame_ms": app.frame_ms,
            "file_busy": app.file_load.busy(),
            "file_picker_busy": app.file_picker_busy(),
            "file_dialog_open": app.file_picker_busy(),
            "build_ms": app.viewport.build_ms,
            "display_lod": app.viewport.lod.lock().map(|l| serde_json::to_value(&l.stats).unwrap_or_default()).unwrap_or_default(),
            "preview": {"active": app.preview.active, "busy": app.preview.busy, "error": app.preview.error, "ms": app.preview.ms, "replaced": app.preview.replaced},
            "home": app.home.open,
            "recent_thumbnails_ready": app.home.ready_recents(),
            "documents": app.docs.count(),
            "quit_requested": app.quit_requested,
            "quitting": app.docs.quitting.is_some(),
        })),
        "ui.file" => {
            let Some(path) = s("path") else { return err("path is required") };
            if p.get("insert").and_then(Value::as_bool).unwrap_or(false) {
                app.insert_path_async(path);
            } else {
                app.open_path_async(path);
            }
            ok(json!({"busy": app.file_load.busy()}))
        }
        "ui.documents" => ok(crate::documents::control(app, p)),
        "ui.home" => {
            match p.get("open").and_then(Value::as_bool) {
                Some(true) => {
                    let rev = app.session.revision;
                    app.home.show(rev);
                }
                Some(false) => app.home.open = false,
                None => {}
            }
            if let Some(i) = p.get("sample").and_then(Value::as_u64) {
                crate::home::open_sample(app, i as usize);
            }
            ok(json!({"open": app.home.open, "recent": app.home.recent}))
        }
        "ui.set" => {
            // What is hidden lives in the session (`browser.visibility`); `ui.set` still sets it.
            const VIS: [(&str, &str); 5] = [
                ("hiddenBodies", "hidden_bodies"),
                ("hiddenOrigin", "hidden_origin"),
                ("hiddenSketches", "hidden_sketches"),
                ("shownSketches", "shown_sketches"),
                ("hiddenJoints", "hidden_joints"),
            ];
            let mut cur = serde_json::to_value(&app.ui).unwrap_or(json!({}));
            let mut vis = serde_json::to_value(&app.session.visibility).unwrap_or(json!({}));
            if let (Some(o), Some(vo), Some(src)) = (cur.as_object_mut(), vis.as_object_mut(), p.as_object()) {
                for (k, v) in src {
                    match VIS.iter().find(|(camel, _)| camel == k) {
                        Some((_, snake)) => vo.insert(snake.to_string(), v.clone()),
                        None => o.insert(k.clone(), v.clone()),
                    };
                }
            }
            match (serde_json::from_value::<crate::UiState>(cur), serde_json::from_value::<solidcraft_engine::Visibility>(vis)) {
                (Ok(u), Ok(v)) => {
                    if (u.tab == "RENDER") != crate::render_workspace::active(app)
                        && let Err(e) = crate::render_workspace::enter(app, u.tab == "RENDER")
                    {
                        return err(e);
                    }
                    app.ui = u;
                    app.session.visibility = v;
                    ok(serde_json::to_value(&app.ui).unwrap_or_default())
                }
                (Err(e), _) | (_, Err(e)) => err(e),
            }
        }
        "ui.view" => {
            let v = s("view").unwrap_or("home");
            app.viewport.orbit_snap = None;
            app.viewport.orbit_free = None;
            if p.get("animate").and_then(Value::as_bool).unwrap_or(false) {
                if v != "fit" && v != "home" && StandardView::parse(v).is_none() {
                    return err(format!("unknown view `{v}`"));
                }
                app.animate_view(v);
                return ok(json!({ "animating": true, "to": app.cam_anim.map(|a| a.to) }));
            }
            app.cancel_view_animation();
            match v {
                "fit" => app.fit_view(),
                "home" => {
                    app.cam.set_view(StandardView::Iso);
                    app.fit_view();
                }
                v => match StandardView::parse(v) {
                    Some(sv) => app.cam.set_view(sv),
                    None => return err(format!("unknown view `{v}`")),
                },
            }
            ok(json!(app.cam))
        }
        "ui.camera" => wrap(camera(app, p)),
        "ui.start" => {
            let Some(id) = s("command") else { return err("missing `command`") };
            // `params`: what the dialog opens on. Refused: nothing starts, or the dialog closes.
            let params = p.get("params").filter(|v| !v.is_null());
            if params.is_some() && id != "joint.drive" && id != "motion.study" {
                return err(format!("`{id}` takes no ui.start params yet (joint.drive takes joint, values; motion.study takes study)"));
            }
            app.start(id);
            if let Some(params) = params.filter(|_| id == "motion.study") {
                let Some(crate::dialogs::Kind::Motion(k)) = app.dialog.as_mut().map(|d| &mut d.kind) else {
                    return err("Motion Study did not open");
                };
                if let Err(e) = crate::dialogs_motion::preset(&app.session, k, params) {
                    crate::dialogs::cancel(app);
                    return err(e);
                }
            } else if let Some(params) = params {
                if app.tree.capture_prompt.is_some() {
                    return err("Drive Joints waits for the Capture Position question (ui.capture)");
                }
                let Some(crate::dialogs::Kind::Assembly(k)) = app.dialog.as_mut().map(|d| &mut d.kind) else {
                    let why = app.status.as_ref().filter(|s| s.2).map(|s| s.0.clone());
                    return err(why.unwrap_or_else(|| "Drive Joints did not open".into()));
                };
                if let Err(e) = crate::dialogs_assembly::preset(&app.session, k, params) {
                    crate::dialogs::cancel(app);
                    return err(e);
                }
            }
            ok(json!({"tool": app.tool.as_ref().map(|t| t.cmd), "dialog": app.dialog.as_ref().map(|d| format!("{d:?}"))}))
        }
        "ui.move" | "ui.click" => {
            let (Some(x), Some(y)) = (f("x"), f("y")) else { return err("missing x/y") };
            if x.abs() > 1e6 || y.abs() > 1e6 {
                return err("x/y must be within ±1000000 screen points");
            }
            let pos = egui::pos2(x as f32, y as f32);
            let modifiers = if method == "ui.move" {
                egui::Modifiers { shift: b("shift"), alt: b("alt"), ctrl: b("ctrl"), command: b("ctrl") || b("cmd"), mac_cmd: b("cmd") }
            } else {
                egui::Modifiers::NONE
            };
            app.synthetic.push(egui::Event::ModifiersChanged(modifiers));
            app.synthetic.push(egui::Event::PointerMoved(pos));
            if method == "ui.click" {
                let button = match s("button") {
                    Some("right") => egui::PointerButton::Secondary,
                    Some("middle") => egui::PointerButton::Middle,
                    _ => egui::PointerButton::Primary,
                };
                let modifiers = egui::Modifiers { shift: b("shift"), ctrl: b("ctrl"), command: b("ctrl"), alt: b("alt"), ..Default::default() };
                // `double: true` sends two clicks in one frame, which egui reads as a double-click.
                for _ in 0..if b("double") { 2 } else { 1 } {
                    app.synthetic.push(egui::Event::PointerButton { pos, button, pressed: true, modifiers });
                    app.synthetic.push(egui::Event::PointerButton { pos, button, pressed: false, modifiers });
                }
            }
            ok(Value::Null)
        }
        "ui.drag" => {
            let (Some(x0), Some(y0), Some(x1), Some(y1)) = (f("x0"), f("y0"), f("x1"), f("y1")) else { return err("missing x0/y0/x1/y1") };
            let button = match s("button") {
                Some("right") => egui::PointerButton::Secondary,
                Some("middle") => egui::PointerButton::Middle,
                _ => egui::PointerButton::Primary,
            };
            let modifiers = egui::Modifiers { shift: b("shift"), ctrl: b("ctrl"), command: b("ctrl"), alt: b("alt"), ..Default::default() };
            let (a, z) = (egui::pos2(x0 as f32, y0 as f32), egui::pos2(x1 as f32, y1 as f32));
            // Positioning precedes the press by a frame: it is not a modifier-only gesture.
            app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            app.synthetic.push(egui::Event::PointerMoved(a));
            app.synthetic.push(egui::Event::PointerButton { pos: a, button, pressed: true, modifiers });
            // `hold`: frames to keep the button down before moving (press-and-hold).
            for _ in 0..f("hold").unwrap_or(0.0).clamp(0.0, 600.0) as usize {
                app.synthetic.push(egui::Event::PointerMoved(a));
            }
            let steps = f("steps").unwrap_or(10.0).clamp(1.0, 200.0) as usize;
            for k in 1..=steps {
                app.synthetic.push(egui::Event::PointerMoved(a + (z - a) * (k as f32 / steps as f32)));
            }
            // `hold_end`: frames to keep the button down at the end (to look at a drag's snap).
            for _ in 0..f("hold_end").unwrap_or(0.0).clamp(0.0, 600.0) as usize {
                app.synthetic.push(egui::Event::PointerMoved(z));
            }
            app.synthetic.push(egui::Event::PointerButton { pos: z, button, pressed: false, modifiers });
            ok(Value::Null)
        }
        "ui.editFeature" => {
            let key = match p.get("feature") {
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::String(x)) => x.clone(),
                _ => return err("`feature` must be an id or name"),
            };
            let Some(id) = app.session.doc.find_feature(&key).map(|f| f.id) else { return err(format!("no feature `{key}`")) };
            app.edit_feature(id);
            ok(json!({"dialog": app.dialog.as_ref().map(|d| format!("{d:?}")), "marker": app.session.doc.marker}))
        }
        "ui.selection" => ok(json!({
            "selection": app.session.selection,
            "dialog": app.dialog.as_ref().map(|d| d.inputs.iter().map(|i| json!({"label": i.label, "items": i.items})).collect::<Vec<_>>()),
            "hover": app.viewport.hover.as_ref().map(|h| format!("{h:?}")),
            "dimensions": crate::dim_view::drawn().into_iter().map(|(id, p)| json!({"id": id, "x": p.x, "y": p.y})).collect::<Vec<_>>(),
            "dimension_selected": crate::dim_view::selected(app),
            "dimension_editing": crate::dim_view::editing(),
        })),
        "ui.triad" => {
            // Screen points of the 3D move triad's handles (x, y, z arrows and the plane square).
            let Some(rect) = app.viewport.rect else { return err("no viewport") };
            let proj = crate::viewport::projection(app, rect);
            ok(json!(
                crate::sketch3d::handle_points(app, &proj).into_iter().map(|(n, p)| json!({"handle": n, "x": p.x, "y": p.y})).collect::<Vec<_>>()
            ))
        }
        "ui.dialogInput" => {
            // What clicking a selection input's chip does: make it the one picks go to.
            let i = p.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            match app.dialog.as_mut() {
                Some(d) if i < d.inputs.len() => {
                    d.active = i;
                    ok(json!({"label": d.inputs.get(i).map(|x| x.label)}))
                }
                _ => err("no dialog input with that index"),
            }
        }
        "ui.handles" => {
            // The handles drawn last frame (toolbar buttons `toolbar:<id>`, panels, browser rows…).
            let list = crate::scenario::handles(s("prefix").unwrap_or(""));
            ok(json!(list.into_iter().map(|(n, p)| json!({"name": n, "x": p.x, "y": p.y})).collect::<Vec<_>>()))
        }
        "ui.at" => {
            if let Some(path) = p.get("occurrence").and_then(Value::as_array) {
                let ids: Option<Vec<u64>> = path.iter().map(Value::as_u64).collect();
                let Some(ids) = ids else { return err("occurrence path must contain integer ids") };
                let key = ids.iter().map(u64::to_string).collect::<Vec<_>>().join("/");
                let control = s("control").unwrap_or("row");
                return match crate::scenario::handle_at(&format!("{control}:occurrence:{key}")) {
                    Some(q) => ok(json!([q.x, q.y])),
                    None => err("occurrence row or control is not shown"),
                };
            }
            // A scenario point: {world: [x,y,z]} | {sketch: [x,y]} | {plane: "XY"} (the middle of an
            // origin plane's square) → screen point.
            if let Some(h) = p.get("handle").and_then(Value::as_str) {
                return match crate::scenario::handle_at(h) {
                    Some(q) => ok(json!([q.x, q.y])),
                    None => err(format!("handle {h} is not shown")),
                };
            }
            // A browser row by its label: the row itself, or its fold arrow.
            for (k, chevron) in [("browser", false), ("chevron", true)] {
                if let Some(label) = p.get(k).and_then(Value::as_str) {
                    let rows = crate::browser::drawn_rows();
                    let found = rows
                        .iter()
                        .find(|r| r.0 == label)
                        .and_then(|r| if chevron { r.2.map(|f| f.center()) } else { Some(r.1.left_center() + egui::vec2(120.0, 0.0)) });
                    return match found {
                        Some(q) => ok(json!([q.x, q.y])),
                        None => err(format!("no browser row `{label}`{} on screen", if chevron { " with a fold arrow" } else { "" })),
                    };
                }
            }
            if let Some(d) = p.get("dimension").and_then(Value::as_str) {
                return match crate::dim_view::text_at(d) {
                    Some(q) => ok(json!([q.x, q.y])),
                    None => err(format!("dimension {d} is not shown")),
                };
            }
            let Some(rect) = app.viewport.rect else { return err("no viewport") };
            let proj = crate::viewport::projection(app, rect);
            let num = |v: &Value, i: usize| v.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            let world = if let Some(w) = p.get("world") {
                Some(solidcraft_engine::geom::Vec3::new(num(w, 0), num(w, 1), num(w, 2)))
            } else if let Some(s) = p.get("sketch") {
                let st = app.session.world_state();
                app.session
                    .active_sketch
                    .and_then(|id| st.sketch(id))
                    .map(|ss| ss.plane.to_world(solidcraft_engine::geom::Vec2::new(num(s, 0), num(s, 1))))
            } else if let Some(n) = p.get("plane").and_then(Value::as_str) {
                crate::viewport::origin_plane_target(app, &proj, n)
            } else if let Some(n) = p.get("axis").and_then(Value::as_str) {
                // Along an origin axis, on its solid part.
                let size = crate::selection::origin_size(app.cam.half_height());
                crate::viewport::origin_axes(app).into_iter().find(|(name, _)| *name == n).map(|(_, d)| d * (size * 0.8))
            } else if let Some(n) = p.get("gizmo").and_then(Value::as_str) {
                // A handle of the move triad ("x", "xy", "rz", "ball"), moved `by` [x,y,z] mm or
                // turned `turn` degrees (rings).
                let by = p.get("by").map(|b| solidcraft_engine::geom::Vec3::new(num(b, 0), num(b, 1), num(b, 2))).unwrap_or_default();
                crate::gizmo::handle_point(n, by, p.get("turn").and_then(Value::as_f64).unwrap_or(0.0))
            } else {
                None
            };
            match world.and_then(|w| proj.to_screen(w)) {
                Some(q) => ok(json!([q.x, q.y])),
                None => err(format!("not on screen: {}", p)),
            }
        }
        "ui.worldToScreen" => {
            let Some(rect) = app.viewport.rect else { return err("no viewport") };
            let proj = crate::viewport::projection(app, rect);
            let pts: Vec<Value> = p
                .get("points")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(1000)
                .map(|v| {
                    let c = |i: usize| v.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                    proj.to_screen(solidcraft_engine::geom::Vec3::new(c(0), c(1), c(2))).map_or(Value::Null, |q| json!([q.x, q.y]))
                })
                .collect();
            ok(json!(pts))
        }
        "ui.sketchToScreen" => {
            // Screen points of active-sketch coordinates (for driving sketch interaction).
            let st = app.session.world_state();
            let (Some(rect), Some(ss)) = (app.viewport.rect, app.session.active_sketch.and_then(|s| st.sketch(s))) else {
                return err("no active sketch in a drawn viewport");
            };
            let proj = crate::viewport::projection(app, rect);
            let pts: Vec<Value> = p
                .get("points")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(1000)
                .map(|v| {
                    let c = |i: usize| v.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                    let q = solidcraft_engine::geom::Vec2::new(c(0), c(1));
                    proj.to_screen(ss.plane.to_world(q)).map_or(Value::Null, |p| json!([p.x, p.y]))
                })
                .collect();
            ok(json!(pts))
        }
        "ui.scroll" => {
            let (Some(x), Some(y)) = (f("x"), f("y")) else { return err("missing x/y") };
            app.synthetic.push(egui::Event::PointerMoved(egui::pos2(x as f32, y as f32)));
            // Mouse-wheel semantics (lines): zooms regardless of the trackpad pan
            // preference, so scripts and agents keep working. Trackpads report points.
            let speed = ctx.options(|o| o.input_options.line_scroll_speed).max(1.0);
            let lines = f("delta").unwrap_or(-100.0) / f64::from(speed);
            app.synthetic.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, lines as f32),
                modifiers: Default::default(),
                phase: egui::TouchPhase::Move,
            });
            ok(Value::Null)
        }
        "ui.key" => {
            let Some(key) = s("key").and_then(key_from) else { return err("unknown key") };
            let modifiers = egui::Modifiers { shift: b("shift"), command: b("cmd"), ctrl: b("cmd") || b("ctrl"), ..Default::default() };
            app.synthetic.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
            app.synthetic.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
            ok(Value::Null)
        }
        "ui.text" => {
            app.synthetic.push(egui::Event::Text(s("text").unwrap_or("").to_string()));
            ok(Value::Null)
        }
        "ui.menu" => {
            if b("close") {
                crate::context_menu::close(app);
                return ok(json!({"open": false}));
            }
            if let (Some(x), Some(y)) = (f("x"), f("y")) {
                let target = match p.get("target") {
                    Some(v) => match serde_json::from_value::<crate::context_menu::Target>(v.clone()) {
                        Ok(t) => t,
                        Err(e) => return err(e),
                    },
                    None => crate::context_menu::Target::Viewport,
                };
                crate::context_menu::request_open(app, egui::pos2(x as f32, y as f32), target);
                return ok(json!({"pending": true}));
            }
            ok(crate::context_menu::describe(app))
        }
        "ui.confirm" => match p.get("accept").and_then(Value::as_bool) {
            Some(a) => {
                if crate::delete::confirm(app, a) {
                    ok(json!({"accepted": a}))
                } else {
                    err("nothing is waiting for a confirmation")
                }
            }
            None => ok(json!({"pending": app.menu.confirm.as_ref().map(|c| c.report.clone())})),
        },
        "ui.shortcut" => {
            let Some(id) = s("command") else { return err("missing `command`") };
            match s("key") {
                Some(k) => match crate::keymap::bind(app, id, k, b("replace")) {
                    Ok(()) => ok(json!({"command": id, "key": crate::keymap::effective(app, id)})),
                    Err(e) => err(e),
                },
                None => ok(json!({"command": id, "key": crate::keymap::effective(app, id)})),
            }
        }
        "ui.browser" => ok(json!({
            "occurrences": crate::browser::drawn_occurrences().iter().map(|(path, label, r, open)| json!({"path": path, "label": label, "y": r.center().y, "open": open})).collect::<Vec<_>>(),
            "rows": crate::browser::drawn_rows().iter().map(|(label, r, _, open)| json!({"label": label, "y": r.center().y, "open": open})).collect::<Vec<_>>(),
            "selection": app.session.selection,
            "picked_components": app.tree.picked_components,
        })),
        "ui.capture" => match s("action") {
            Some(a) => match crate::browser::answer_capture(app, a) {
                Ok(()) => ok(json!({"pending_moves": app.session.pending_moves.len()})),
                Err(e) => err(e),
            },
            None => ok(json!({"question": app.tree.capture_prompt, "pending_moves": app.session.pending_moves.len()})),
        },
        "ui.prefs" => wrap(crate::prefs::control(app, p)),
        "ui.agent_chat" => wrap(crate::agent_chat::control(app, ctx, p)),
        "ui.help" => match s("item") {
            Some(i @ ("about" | "shortcuts" | "report" | "crashes")) => {
                crate::help::run(app, ctx, i);
                ok(
                    json!({"diagnostics": crate::help::diagnostics(app), "version": crate::help::VERSION, "commit": crate::help::COMMIT, "built": crate::help::BUILD_DATE}),
                )
            }
            _ => ok(
                json!({"diagnostics": crate::help::diagnostics(app), "version": crate::help::VERSION, "commit": crate::help::COMMIT, "built": crate::help::BUILD_DATE}),
            ),
        },
        "ui.selectOther" => wrap(crate::select_other::control(app, p)),
        "ui.menuPick" => {
            let Some(key) = s("item") else { return err("missing `item` (id or label)") };
            wrap(crate::context_menu::pick(app, key))
        }
        "ui.rename" => {
            if crate::context_menu::finish_rename(app, s("text"), p.get("commit").and_then(Value::as_bool).unwrap_or(true)) {
                ok(json!({"done": true}))
            } else {
                err("no rename in progress")
            }
        }
        "ui.window" => match crate::titlebar::window_action(app, ctx, s("action").unwrap_or("")) {
            Ok(()) => ok(json!({"custom_titlebar": app.custom_titlebar, "maximized": ctx.input(|i| i.viewport().maximized)})),
            Err(e) => err(e),
        },
        "ui.resize" => {
            let (Some(w), Some(h)) = (f("width"), f("height")) else { return err("missing width/height") };
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w.clamp(320.0, 8192.0) as f32, h.clamp(240.0, 8192.0) as f32)));
            ok(Value::Null)
        }
        "ui.screenshot" => match s("path").map(vfs::check) {
            Some(Err(e)) => err(e),
            _ => Outcome::Screenshot { path: s("path").map(str::to_string) },
        },
        "ui.render" => {
            let w = f("width").unwrap_or(1280.0).clamp(16.0, 8192.0) as u32;
            let h = f("height").unwrap_or(800.0).clamp(16.0, 8192.0) as u32;
            // As the window shows it (`view_scene`). The grid and the axes as the window shows
            // them, unless the request says: `grid` and `axes` leave them out or put them in.
            let shown = window_guides(app);
            let guides = solidcraft_engine::view::Guides {
                grid: p.get("grid").and_then(Value::as_bool).unwrap_or(shown.grid),
                axes: p.get("axes").and_then(Value::as_bool).unwrap_or(shown.axes),
            };
            // A path outside the request's folders is refused before anything is drawn.
            if let Some(Err(e)) = s("path").map(vfs::check) {
                return err(e);
            }
            let scene = view_scene(app, guides);
            let png = match solidcraft_engine::capture::png(&scene, &app.cam, w, h) {
                Ok(png) => png,
                Err(e) => return err(format!("render failed: {e}")),
            };
            // What drew it (gpu, gpu-software or cpu), and the warning a CPU render carries.
            let (renderer, warning) = (png.renderer, solidcraft_engine::capture::warning(png.renderer));
            match s("path") {
                Some(path) => match vfs::check(path).and_then(|()| std::fs::write(path, &png.bytes)) {
                    Ok(()) => ok(json!({"path": path, "bytes": png.bytes.len(), "renderer": renderer, "warning": warning})),
                    Err(e) => err(e),
                },
                None => ok(json!({"bytes": png.bytes.len(), "renderer": renderer, "warning": warning})),
            }
        }
        "app.quit" => {
            // `force`: quit without asking about unsaved designs.
            if p.get("force").and_then(Value::as_bool).unwrap_or(false) {
                app.quit_requested = true;
            } else {
                crate::documents::request_quit(app);
            }
            if app.quit_requested {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ok(json!({"quitting": app.quit_requested, "asking": app.docs.closing.is_some()}))
        }
        other => err(format!("unknown method `{other}`")),
    }
}

/// `ui.camera`: the viewport's orbit camera (`target`, `yaw`, `pitch`, `roll` in radians,
/// `distance` in mm, `fov`, `up`), or a new one. A field left out keeps its value, so a reply
/// passed back sets the same view again. `fit`: frame the listed occurrences (ids or names;
/// `root`: the bodies outside every component), or everything (`true`), keeping the direction;
/// `false` frames nothing.
/// Nothing changes when a field is refused.
fn camera(app: &mut SolveApp, p: &Value) -> Result<Value, String> {
    const FIELDS: [&str; 8] = ["target", "yaw", "pitch", "roll", "distance", "fov", "up", "fit"];
    let o = match p {
        Value::Null => serde_json::Map::new(),
        Value::Object(o) => o.clone(),
        _ => return Err("params must be an object".into()),
    };
    if let Some(k) = o.keys().find(|k| !FIELDS.contains(&k.as_str())) {
        return Err(format!("unknown camera field `{k}` (it takes {})", FIELDS.join(", ")));
    }
    let mut cam = app.cam;
    let sets = o.keys().any(|k| k != "fit");
    if sets {
        let mut v = serde_json::to_value(app.cam).map_err(|e| e.to_string())?;
        for (k, x) in o.iter().filter(|(k, _)| *k != "fit") {
            v[k] = x.clone();
        }
        cam = serde_json::from_value(v).map_err(|e| format!("bad camera: {e}"))?;
        // The distance cap is far past any Fit (so every reply replays) and still finite in f32.
        if !cam.is_valid() || cam.distance > 1e15 || cam.pitch.abs() > std::f64::consts::FRAC_PI_2 {
            return Err("the camera must be finite, with a distance up to 1e15 mm and a pitch within ±π/2".into());
        }
        // Straight up or down keeps a heading, as the standard views do.
        let lim = std::f64::consts::FRAC_PI_2 - 1e-6;
        cam.pitch = cam.pitch.clamp(-lim, lim);
        // Perspective and the up axis are settings the camera follows, not its own.
        if cam.fov != app.cam.fov {
            return Err("`fov` follows the perspective setting (ui.set {perspective})".into());
        }
        if cam.up != app.cam.up {
            return Err("`up` follows the up axis preference (ui.prefs {up_axis})".into());
        }
    }
    let fit = match o.get("fit") {
        None | Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) if !crate::render_workspace::active(app) && crate::viewport::view_bounds(app).is_empty() => {
            return Err("nothing is shown to fit".into());
        }
        Some(Value::Bool(true)) => Some(None),
        Some(Value::Array(list)) if !list.is_empty() && list.len() <= 10_000 => {
            let occs = list.iter().map(|v| occurrence(&app.session, v)).collect::<Result<Vec<u64>, String>>()?;
            let b = crate::dialogs_assembly::occurrence_bounds(&app.session, &occs);
            if b.is_empty() {
                return Err("those occurrences show nothing to fit".into());
            }
            Some(Some(b))
        }
        Some(_) => return Err("`fit` must be true, false or a list of occurrences".into()),
    };
    // A read leaves a view change that is animating alone.
    if !sets && fit.is_none() {
        return serde_json::to_value(app.cam).map_err(|e| e.to_string());
    }
    app.cancel_view_animation();
    app.viewport.orbit_snap = None;
    app.viewport.orbit_free = None;
    app.cam = cam;
    match fit {
        Some(None) => app.fit_view(),
        Some(Some(b)) => app.fit_bounds(b),
        None => {}
    }
    serde_json::to_value(app.cam).map_err(|e| e.to_string())
}

/// An occurrence by id or name (or a component's name: its first occurrence); 0 is the root.
fn occurrence(s: &solidcraft_engine::Session, v: &Value) -> Result<u64, String> {
    let key = match v {
        Value::Number(n) => n.to_string(),
        Value::String(x) => x.clone(),
        _ => return Err("an occurrence must be an id or a name".into()),
    };
    if key == "0" || key.eq_ignore_ascii_case("root") {
        return Ok(0);
    }
    s.doc
        .occurrences
        .iter()
        .find(|o| o.id.to_string() == key || o.name == key)
        .map(|o| o.id)
        .or_else(|| s.doc.find_component(&key).and_then(|c| s.doc.occurrence_of(c)).map(|o| o.id))
        .ok_or_else(|| format!("no occurrence `{key}`"))
}

/// Save a screenshot PNG.
pub fn save_screenshot(image: &egui::ColorImage, path: Option<&str>) -> Value {
    let [w, h] = image.size;
    let mut buf = Vec::with_capacity(w * h * 4);
    for c in &image.pixels {
        buf.extend_from_slice(&c.to_array());
    }
    let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, buf) else { return json!({"ok": false, "error": "bad image"}) };
    let path = path.map(str::to_string).unwrap_or_else(|| std::env::temp_dir().join("solidcraft-shot.png").to_string_lossy().to_string());
    match img.save(&path) {
        Ok(()) => json!({"ok": true, "result": {"path": path, "width": w, "height": h}}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use solidcraft_engine::{Sel, Session};

    use super::*;
    use crate::Services;

    #[test]
    fn touchpad_automation_moves_with_modifiers_and_releases_without_buttons() {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.nav = "touchpad".into();
        h.call("ui.move", json!({"x":800,"y":500}));
        let before = h.app.cam;
        h.call("ui.move", json!({"x":800,"y":500,"shift":true}));
        h.step(&json!({"move":[830,520],"shift":true})).unwrap();
        assert_ne!(h.app.cam.target, before.target);
        assert_eq!(h.app.cam.yaw, before.yaw);
        assert!(!h.ctx.input(|i| i.pointer.any_down()));
        h.call("ui.move", json!({"x":830,"y":520,"alt":true}));
        h.step(&json!({"move":[850,530],"alt":true})).unwrap();
        assert!((h.app.cam.yaw - before.yaw - 0.16).abs() < 1e-6);
        let after = h.app.cam;
        h.call("ui.move", json!({"x":800,"y":500}));
        h.frames(3);
        assert_eq!(h.app.cam, after);
        assert!(h.ctx.input(|i| i.modifiers.is_none()));
        assert!(h.app.session.selection.is_empty());
        assert!(h.app.viewport.boxsel.is_none());
    }

    #[test]
    fn touchpad_drag_positioning_releases_previous_motion_modifiers() {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.nav = "touchpad".into();
        h.call("ui.move", json!({"x":800,"y":500}));
        for modifier in ["shift", "alt"] {
            let mut params = json!({"x":800,"y":500});
            params[modifier] = json!(true);
            h.call("ui.move", params);
            let before = h.app.cam;
            h.call("ui.drag", json!({"x0":900,"y0":500,"x1":930,"y1":520,"shift":true,"steps":2}));
            assert_eq!(h.app.cam, before, "positioning a selection drag must not inherit {modifier} motion");
            assert!(!h.ctx.input(|i| i.pointer.any_down()));
        }
    }

    #[test]
    fn touchpad_text_prefix_does_not_collapse_queued_pointer_frames() {
        let mut app = SolveApp::new(Session::default(), Services::default());
        let ctx = egui::Context::default();
        let (req, _) = ControlRequest::new("ui.text", json!({"text":"prefix"}));
        assert!(matches!(handle(&mut app, &ctx, &req), Outcome::Done(_)));
        for params in [json!({"x":10,"y":10,"shift":true}), json!({"x":20,"y":20,"alt":true}), json!({"x":30,"y":30})] {
            let (req, _) = ControlRequest::new("ui.move", params);
            assert!(matches!(handle(&mut app, &ctx, &req), Outcome::Done(_)));
        }
        for (p, shift, alt) in [(10.0, true, false), (20.0, false, true), (30.0, false, false)] {
            let mut raw = egui::RawInput::default();
            app.raw_input_hook(&mut raw);
            let mut out = ctx.run_ui(raw, |ui| {
                ui.input(|i| {
                    assert_eq!(i.pointer.hover_pos(), Some(egui::pos2(p, p)));
                    assert_eq!((i.modifiers.shift, i.modifiers.alt), (shift, alt));
                });
            });
            out.textures_delta.clear();
        }
        assert!(app.synthetic.is_empty());
    }

    #[test]
    fn touchpad_automation_preserves_each_queued_moves_modifiers() {
        let mut app = SolveApp::new(Session::default(), Services::default());
        let ctx = egui::Context::default();
        for params in [json!({"x":10,"y":10,"shift":true}), json!({"x":20,"y":20,"alt":true}), json!({"x":30,"y":30})] {
            let (req, _) = ControlRequest::new("ui.move", params);
            assert!(matches!(handle(&mut app, &ctx, &req), Outcome::Done(_)));
        }
        for (p, shift, alt) in [(10.0, true, false), (20.0, false, true), (30.0, false, false)] {
            let mut raw = egui::RawInput::default();
            app.raw_input_hook(&mut raw);
            let mut out = ctx.run_ui(raw, |ui| {
                ui.input(|i| {
                    assert_eq!(i.pointer.hover_pos(), Some(egui::pos2(p, p)));
                    assert_eq!((i.modifiers.shift, i.modifiers.alt), (shift, alt));
                    assert!(!i.pointer.any_down());
                });
            });
            out.textures_delta.clear();
        }
        assert!(app.synthetic.is_empty());
    }

    fn render(app: &mut SolveApp, path: &std::path::Path) -> Vec<u8> {
        let (req, _rx) = ControlRequest::new("ui.render", json!({"path": path.to_string_lossy(), "width": 200, "height": 140}));
        match handle(app, &egui::Context::default(), &req) {
            Outcome::Done(v) => assert_eq!(v["ok"], true, "{v}"),
            Outcome::Screenshot { .. } => panic!("ui.render took a screenshot"),
        }
        std::fs::read(path).unwrap()
    }

    /// `ui.render` draws in the window's theme (#974): its background is the viewport's, light
    /// or dark.
    #[test]
    fn model_render_is_in_the_windows_theme() {
        if !crate::gpu::test_picture_renderer() {
            return;
        }
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let mut app = SolveApp::new(s, Services::default());
        let dir = std::env::temp_dir().join(format!("solidcraft-render-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for dark in [false, true, false] {
            app.ui.dark = dark;
            let png = render(&mut app, &dir.join(format!("dark-{dark}.png")));
            let img = image::load_from_memory(&png).unwrap().to_rgba8();
            // The top row is the background's top: its most common colour.
            let mut counts = std::collections::HashMap::new();
            for x in 0..img.width() {
                *counts.entry(img.get_pixel(x, 0).0).or_insert(0) += 1;
            }
            let top = counts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c).unwrap();
            let want = if dark { crate::theme::Tokens::dark() } else { crate::theme::Tokens::light() }.viewport_top;
            let near = top.iter().zip([want.r(), want.g(), want.b()]).all(|(a, b)| a.abs_diff(b) <= 2);
            assert!(near, "dark {dark}: the render's top row is {top:?}, the window's background {want:?}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A model render shows the section the window shows: the open Section Analysis dialog's
    /// preview too, not only a committed `inspect.section`.
    #[test]
    fn model_render_shows_the_section_dialog_preview() {
        if !crate::gpu::test_picture_renderer() {
            return;
        }
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let mut app = SolveApp::new(s, Services::default());
        let dir = std::env::temp_dir().join(format!("solidcraft-render-section-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let whole = render(&mut app, &dir.join("whole.png"));
        app.session.selection = vec![Sel::Plane { name: "XZ".into() }];
        app.start("inspect.section");
        let Some(crate::dialogs::Kind::Section { offset, .. }) = app.dialog.as_mut().map(|d| &mut d.kind) else { panic!("no section dialog") };
        *offset = "15 mm".into();
        assert!(app.session.section.is_none() && crate::viewport::section_plane(&app).is_some());
        let preview = render(&mut app, &dir.join("preview.png"));
        assert!(whole != preview, "the model render ignores the section the window shows");
        // Committed, it renders the same.
        app.run("inspect.section", json!({"plane": "XZ", "offset": 15})).unwrap();
        app.dialog = None;
        assert!(render(&mut app, &dir.join("committed.png")) == preview, "the committed section renders differently");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ui.render` with extra request parameters.
    fn render_with(app: &mut SolveApp, path: &std::path::Path, extra: serde_json::Value) -> Vec<u8> {
        let mut params = json!({"path": path.to_string_lossy(), "width": 200, "height": 140});
        for (k, v) in extra.as_object().into_iter().flatten() {
            params[k.as_str()] = v.clone();
        }
        let (req, _rx) = ControlRequest::new("ui.render", params);
        match handle(app, &egui::Context::default(), &req) {
            Outcome::Done(v) => assert_eq!(v["ok"], true, "{v}"),
            Outcome::Screenshot { .. } => panic!("ui.render took a screenshot"),
        }
        std::fs::read(path).unwrap()
    }

    /// A model render draws the ground grid (it cuts through a part that straddles Z 0) and the
    /// origin's axes as the window does, by the Grid and Origin switches, and `grid` and `axes`
    /// in the request say otherwise for that render.
    #[test]
    fn model_render_follows_the_grid_and_origin_switches_unless_asked() {
        if !crate::gpu::test_picture_renderer() {
            return;
        }
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20, "corner": [10, -15, -10]})).unwrap();
        let mut app = SolveApp::new(s, Services::default());
        app.cam.set_view(solidcraft_engine::render::StandardView::Front);
        let dir = std::env::temp_dir().join(format!("solidcraft-render-guides-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        let shot = |app: &mut SolveApp, extra: serde_json::Value| render_with(app, &path, extra);
        let all = shot(&mut app, json!({}));
        let no_grid = shot(&mut app, json!({"grid": false}));
        let no_axes = shot(&mut app, json!({"axes": false}));
        let bare = shot(&mut app, json!({"grid": false, "axes": false}));
        let pictures = [&all, &no_grid, &no_axes, &bare];
        for (i, a) in pictures.iter().enumerate() {
            for b in &pictures[i + 1..] {
                assert!(a != b, "two of the pictures are the same");
            }
        }
        // With the Grid switch off the render has no grid, and `grid: true` brings it back.
        app.ui.show_grid = false;
        assert!(shot(&mut app, json!({})) == no_grid);
        assert!(shot(&mut app, json!({"grid": true})) == all);
        // With the Origin off too it has neither; `axes: true` puts the axes back.
        app.ui.show_origin = false;
        assert!(shot(&mut app, json!({})) == bare);
        assert!(shot(&mut app, json!({"axes": true})) == no_grid);
        // The Origin and the Grid back on: the first picture again.
        app.ui.show_grid = true;
        app.ui.show_origin = true;
        assert!(shot(&mut app, json!({})) == all);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn batch(h: &mut crate::scenario::Harness, params: serde_json::Value) -> serde_json::Value {
        h.call("engine.batch", params)
    }

    fn inspect(h: &mut crate::scenario::Harness) -> serde_json::Value {
        h.call("document.inspect", json!({}))["result"].clone()
    }

    /// Timing fields (`ms`) differ from run to run.
    fn without_timings(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(o) => o.iter().filter(|(k, _)| *k != "ms").map(|(k, v)| (k.clone(), without_timings(v))).collect(),
            serde_json::Value::Array(a) => a.iter().map(without_timings).collect(),
            v => v.clone(),
        }
    }

    /// #743: a batch is one request through the app's queue, answered within a frame, however
    /// many steps it has.
    #[test]
    fn engine_batch_runs_in_one_frame() {
        let mut h = crate::scenario::Harness::new();
        let steps: Vec<_> = (0..40)
            .flat_map(|_| [json!({"command": "sketch.create", "params": {"plane": "XY"}}), json!({"command": "sketch.finish", "params": {}})])
            .collect();
        let (v, frames) = h.call_queued("engine.batch", json!({"commands": steps}));
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["result"]["completed"], 80, "{v}");
        assert_eq!(v["result"]["results"].as_array().map(Vec::len), Some(80));
        assert!(v["result"].get("failed").is_none(), "{v}");
        assert!(frames <= 2, "a batch of 80 steps took {frames} frames");
        assert_eq!(inspect(&mut h)["sketches"].as_array().map(Vec::len), Some(40));
    }

    /// #1833: `engine.batch` times its steps where they run, the failing one included, with the
    /// kernel's share when `profile` asks; unasked, the reply is as before.
    #[test]
    fn engine_batch_times_its_steps() {
        let mut h = crate::scenario::Harness::new();
        let steps = json!([
            {"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}},
            {"command": "solid.box", "params": {"length": 4, "width": 4, "height": 30, "operation": "join"}},
        ]);
        let v = batch(&mut h, json!({"commands": steps, "profile": true}));
        let rows = v["result"]["timings"].as_array().cloned().unwrap_or_default();
        assert_eq!(rows.len(), 2, "{v}");
        assert_eq!(rows[1]["index"], 1);
        assert!(rows[1]["profile"]["boolean_ms"].as_f64().is_some_and(|ms| ms > 0.0), "{v}");
        assert!(v["result"]["total_ms"].as_f64().is_some(), "{v}");
        let v = batch(&mut h, json!({"timings": true, "commands": [{"command": "NoSuchCommand"}]}));
        assert_eq!(v["result"]["timings"].as_array().map(Vec::len), Some(1), "{v}");
        assert!(v["result"]["timings"][0].get("profile").is_none(), "{v}");
        let v = batch(&mut h, json!({"commands": [{"command": "solid.box", "params": {"length": 1, "width": 1, "height": 1}}]}));
        assert!(v["result"].get("timings").is_none() && v["result"].get("total_ms").is_none(), "{v}");
    }

    #[test]
    fn engine_batch_rolls_back_at_the_first_failure() {
        let mut h = crate::scenario::Harness::new();
        assert_eq!(h.call("engine.execute", json!({"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}}))["ok"], true);
        let before = inspect(&mut h);
        let v = batch(
            &mut h,
            json!({"commands": [
                {"command": "solid.sphere", "params": {"radius": 3, "center": [50, 0, 0]}},
                {"command": "solid.extrude", "params": {"distance": "bogus +"}},
                {"command": "solid.sphere", "params": {"radius": 3, "center": [80, 0, 0]}},
            ]}),
        );
        assert_eq!(v["ok"], true, "{v}");
        let r = &v["result"];
        assert_eq!((r["completed"].clone(), r["failed"]["index"].clone(), r["undone"].clone()), (json!(1), json!(1), json!(1)), "{v}");
        assert_eq!(r["failed"]["command"], "solid.extrude");
        let after = inspect(&mut h);
        assert_eq!(after["undo"], before["undo"], "{after}");
        assert_eq!(after["bodies"], before["bodies"]);
        assert_eq!(without_timings(&after["timeline"]), without_timings(&before["timeline"]));
    }

    #[test]
    fn engine_batch_without_rollback_keeps_completed_steps() {
        let mut h = crate::scenario::Harness::new();
        let v = batch(
            &mut h,
            json!({"rollback": false, "commands": [
                {"command": "solid.sphere", "params": {"radius": 3}},
                {"command": "NoSuchCommand"},
            ]}),
        );
        assert_eq!(v["result"]["failed"]["index"], 1, "{v}");
        assert_eq!(v["result"]["undone"], 0, "{v}");
        assert_eq!(inspect(&mut h)["bodies"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn engine_batch_step_without_a_command_fails_that_step() {
        let mut h = crate::scenario::Harness::new();
        let v = batch(&mut h, json!({"commands": [{"params": {}}]}));
        assert_eq!(v["result"]["failed"]["index"], 0, "{v}");
        assert!(v["result"]["failed"]["error"].as_str().is_some_and(|e| e.contains("missing `command`")), "{v}");
    }

    #[test]
    fn engine_batch_is_refused_when_a_step_loads_a_file() {
        let mut h = crate::scenario::Harness::new();
        let v = batch(
            &mut h,
            json!({"commands": [
                {"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}},
                {"command": "doc.open", "params": {"path": "elsewhere.solidcraft"}},
            ]}),
        );
        assert_eq!(v["ok"], false, "{v}");
        assert_eq!(v["error"], "engine.batch unavailable: step 1 (doc.open) loads a file", "{v}");
        assert_eq!(inspect(&mut h)["bodies"].as_array().map(Vec::len), Some(0), "nothing may run before the refusal");
    }

    #[test]
    fn engine_batch_is_refused_while_the_agent_cursor_is_shown() {
        let mut h = crate::scenario::Harness::new();
        assert_eq!(h.call("ui.agent_cursor", json!({"show": true}))["ok"], true);
        let v = batch(&mut h, json!({"commands": [{"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}}]}));
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["error"].as_str().is_some_and(|e| e.starts_with("engine.batch unavailable")), "{v}");
        assert_eq!(inspect(&mut h)["bodies"].as_array().map(Vec::len), Some(0));
    }

    /// Every step of a batch from an MCP server stays inside the agent's folders (#205), as an
    /// `engine.execute` would.
    #[test]
    fn engine_batch_stays_inside_allow_paths() {
        let mut h = crate::scenario::Harness::new();
        let base = vfs::temp_dir().join(format!("solidcraft-batch-scope-{}", std::process::id()));
        let (root, out) = (base.join("root"), base.join("out"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let export = |d: &std::path::Path| json!({"command": "file.export", "params": {"path": d.join("a.step").to_string_lossy()}});
        let v = batch(
            &mut h,
            json!({"allow_paths": [root.to_string_lossy()], "commands": [
                {"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}},
                export(&root),
                export(&out),
            ]}),
        );
        assert_eq!(v["result"]["failed"]["index"], 2, "{v}");
        assert!(v["result"]["failed"]["error"].as_str().is_some_and(|e| e.contains("outside the folders this agent may use")), "{v}");
        assert!(root.join("a.step").exists());
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "nothing written outside");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An MCP server's `doc.open` and imports load off the window thread (`file_load`), where
    /// the agent's folders hold as they do for every other request.
    #[test]
    fn file_loads_stay_inside_allow_paths() {
        let mut h = crate::scenario::Harness::new();
        let base = vfs::temp_dir().join(format!("solidcraft-load-scope-{}", std::process::id()));
        let (root, out) = (base.join("root"), base.join("out"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
        for d in [&root, &out] {
            std::fs::write(d.join("part.solidcraft"), solidcraft_engine::io::write_design(&s.doc)).unwrap();
        }
        s.execute("file.export", &json!({"path": out.join("part.step").to_string_lossy()})).unwrap();
        let load = |command: &str, d: &std::path::Path, file: &str| json!({"command": command, "params": {"path": d.join(file).to_string_lossy()}, "allow_paths": [root.to_string_lossy()]});
        for (command, file) in [("doc.open", "part.solidcraft"), ("file.insert_step", "part.step")] {
            let (v, _) = h.call_queued("engine.execute", load(command, &out, file));
            assert!(v["ok"] == false && v.to_string().contains("outside the folders this agent may use"), "{command}: {v}");
        }
        let (v, _) = h.call_queued("engine.execute", load("doc.open", &root, "part.solidcraft"));
        assert_eq!(v["ok"], true, "{v}");
        let (v, _) = h.call_queued("engine.execute", json!({"command": "doc.open", "params": {"path": "x"}, "allow_paths": "/"}));
        assert!(v["error"].as_str().is_some_and(|e| e.contains("allow_paths")), "{v}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Each step runs as `engine.execute` runs it: same results, same design.
    #[test]
    fn engine_batch_matches_engine_execute() {
        let steps = [
            json!({"command": "sketch.create", "params": {"plane": "XY"}}),
            json!({"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [40, 30]}}),
            json!({"command": "sketch.finish", "params": {}}),
            json!({"command": "solid.extrude", "params": {"distance": 20}}),
        ];
        let mut a = crate::scenario::Harness::new();
        let batched = batch(&mut a, json!({"commands": steps}));
        let mut b = crate::scenario::Harness::new();
        let one_by_one: Vec<_> = steps.iter().map(|s| b.call("engine.execute", s.clone())["result"].clone()).collect();
        assert_eq!(without_timings(&batched["result"]["results"]), without_timings(&json!(one_by_one)), "{batched}");
        assert_eq!(without_timings(&inspect(&mut a)["timeline"]), without_timings(&inspect(&mut b)["timeline"]));
        assert_eq!(a.app.cam, b.app.cam, "the first-body fit must happen as for engine.execute");
    }

    /// A request carrying `allow_paths` (from an MCP server, #205) reaches files only inside
    /// those folders; the same request without it, like the user's own dialogs, is unconfined.
    #[test]
    fn requests_with_allow_paths_stay_inside_them() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
        let mut app = SolveApp::new(s, Services::default());
        let base = vfs::temp_dir().join(format!("solidcraft-control-scope-{}", std::process::id()));
        let (root, out) = (base.join("root"), base.join("out"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let at = |d: &std::path::Path, f: &str| d.join(f).to_string_lossy().to_string();
        let mut call = |method: &str, mut params: Value, scoped: bool| {
            if scoped {
                params["allow_paths"] = json!([root.to_string_lossy()]);
            }
            let (req, _rx) = ControlRequest::new(method, params);
            match handle(&mut app, &egui::Context::default(), &req) {
                Outcome::Done(v) => v,
                Outcome::Screenshot { path } => json!({"ok": true, "screenshot": path}),
            }
        };
        let export = |p: String| json!({"command": "file.export", "params": {"path": p}});
        assert_eq!(call("engine.execute", export(at(&root, "a.step")), true)["ok"], true);
        if crate::gpu::test_picture_renderer() {
            assert_eq!(call("ui.render", json!({"path": at(&root, "a.png"), "width": 32, "height": 32}), true)["ok"], true);
        }
        assert_eq!(call("ui.screenshot", json!({"path": at(&root, "b.png")}), true)["ok"], true);
        for (method, params) in [
            ("engine.execute", export(at(&out, "a.step"))),
            ("engine.execute", export(at(&root, "../out/a.step"))),
            ("engine.script", json!({"commands": [{"command": "file.export", "params": {"path": at(&out, "a.step")}}]})),
            ("ui.render", json!({"path": at(&out, "a.png"), "width": 32, "height": 32})),
            ("ui.screenshot", json!({"path": at(&out, "b.png")})),
        ] {
            let v = call(method, params, true);
            assert!(v["ok"] == false && v.to_string().contains("outside the folders this agent may use"), "{method}: {v}");
        }
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "nothing written outside");
        assert_eq!(call("engine.execute", export(at(&out, "a.step")), false)["ok"], true, "without allow_paths: unconfined");
        let bad = call("engine.execute", json!({"command": "file.new", "allow_paths": "/"}), false);
        assert!(bad["error"].as_str().is_some_and(|e| e.contains("allow_paths")), "{bad}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `ui.camera` reads the camera and sets it; its reply, passed back, sets the same view
    /// again. Fields left out keep their values, `fit` frames occurrences, and a refused field
    /// changes nothing.
    #[test]
    fn ui_camera_reads_sets_and_replays_the_view() {
        let mut s = Session::default();
        for (name, x) in [("A", 0), ("B", 100)] {
            s.execute("component.activate", &json!({"component": "root"})).unwrap();
            s.execute("component.create", &json!({ "name": name })).unwrap();
            s.execute("solid.box", &json!({"length": 20, "width": 20, "height": 10, "corner": [x, 0, 0]})).unwrap();
        }
        s.execute("component.activate", &json!({"component": "root"})).unwrap();
        let mut h = crate::scenario::Harness::new();
        h.app = SolveApp::new(s, Services::default());
        h.frames(2);
        let before = h.app.cam;
        // Reading during an animated view change leaves it running (no frames run between).
        fn raw(h: &mut crate::scenario::Harness, method: &str, params: Value) -> Value {
            let (req, _rx) = ControlRequest::new(method, params);
            match handle(&mut h.app, &h.ctx, &req) {
                Outcome::Done(v) => v,
                Outcome::Screenshot { .. } => Value::Null,
            }
        }
        raw(&mut h, "ui.view", json!({"view": "top", "animate": true}));
        let r = raw(&mut h, "ui.camera", Value::Null);
        assert!(r["ok"] == true && h.app.cam_anim.is_some(), "{r}");
        // Setting it, even to where it is now, ends the animation there.
        let yaw = h.app.cam.yaw;
        let r = raw(&mut h, "ui.camera", json!({ "yaw": yaw }));
        assert!(r["ok"] == true && h.app.cam_anim.is_none(), "{r}");
        h.app.cam = before;
        let mut cam = |p: Value| {
            let r = h.call("ui.camera", p);
            assert_eq!(r["ok"], true, "{r}");
            (r["result"].clone(), h.app.cam)
        };
        let (start, now) = cam(json!({}));
        assert_eq!(start, json!(now));
        let (set, _) = cam(json!({"target": [5, 6, 7], "yaw": 0.5, "pitch": -0.25, "distance": 300}));
        assert_eq!(
            (set["target"].clone(), set["yaw"].clone(), set["pitch"].clone(), set["distance"].clone()),
            (json!([5.0, 6.0, 7.0]), json!(0.5), json!(-0.25), json!(300.0))
        );
        assert_eq!(set["roll"], start["roll"], "a field left out keeps its value");
        let (top, _) = cam(json!({"pitch": std::f64::consts::FRAC_PI_2}));
        assert!(top["pitch"].as_f64().unwrap() < std::f64::consts::FRAC_PI_2, "straight down keeps a heading: {top}");
        let (again, now) = cam(set.clone());
        assert_eq!((again, json!(now)), (set.clone(), set.clone()), "the reply sets the same view again");
        let (fit, _) = cam(json!({"fit": ["B"]}));
        let t: Vec<f64> = fit["target"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        // Framed as Fit frames (the viewport's panels shift the target a little): on B, not A.
        assert!((100.0..120.0).contains(&t[0]) && (0.0..20.0).contains(&t[1]) && (0.0..10.0).contains(&t[2]), "{fit}");
        assert_eq!(fit["yaw"], set["yaw"], "fit keeps the direction");
        let fov = h.app.cam.fov + 0.25;
        for bad in [
            json!({"zoom": 2}),
            json!({"distance": -1}),
            json!({"distance": 1e300}),
            json!({"pitch": 2}),
            json!({"yaw": "x"}),
            json!({"fov": fov}),
            json!({"up": "y"}),
            json!({"yaw": 1, "fit": ["Nope"]}),
            json!({"fit": []}),
            json!({"fit": 1}),
            json!([1, 2]),
        ] {
            let before = h.app.cam;
            let r = h.call("ui.camera", bad.clone());
            assert_eq!(r["ok"], false, "{bad}: {r}");
            assert_eq!(h.app.cam, before, "{bad}");
        }
        // Nothing shown: nothing to fit.
        let mut empty = crate::scenario::Harness::new();
        let before = empty.app.cam;
        let r = empty.call("ui.camera", json!({"yaw": 1, "fit": true}));
        assert!(r["ok"] == false && empty.app.cam == before, "{r}");
    }

    /// `ui.start` params set what a dialog opens on: Drive Joints on a joint (by name or id) at
    /// the given values, driving nothing yet. A param it doesn't take is refused and closes it.
    #[test]
    fn ui_start_params_open_drive_joints_where_asked() {
        let mut s = Session::default();
        for (name, x) in [("A", 0), ("B", 30), ("C", 60)] {
            s.execute("component.activate", &json!({"component": "root"})).unwrap();
            s.execute("component.create", &json!({ "name": name })).unwrap();
            s.execute("solid.box", &json!({"length": 20, "width": 20, "height": 10, "corner": [x, 0, 0]})).unwrap();
        }
        s.execute("component.activate", &json!({"component": "root"})).unwrap();
        s.execute("occurrence.ground", &json!({"occurrence": "A:1", "grounded": true})).unwrap();
        let at = |o: &str, x: i32| json!({"occurrence": o, "point": [x, 10, 5], "z": [1, 0, 0]});
        s.execute("joint.create", &json!({"type": "revolute", "a": at("A", 25), "b": at("B", 25), "flip": true, "name": "Hinge"})).unwrap();
        s.execute("joint.create", &json!({"type": "slider", "a": at("B", 55), "b": at("C", 55), "flip": true, "name": "Rail"})).unwrap();
        let mut h = crate::scenario::Harness::new();
        h.app = SolveApp::new(s, Services::default());
        let before = h.app.session.doc.clone();
        let drive = |h: &crate::scenario::Harness| match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(crate::dialogs::Kind::Assembly(crate::dialogs_assembly::Asm::Drive { joint, values, .. })) => {
                (h.app.session.doc.assembly.joints[*joint].name.clone(), values.clone())
            }
            other => panic!("{other:?}"),
        };
        let r = h.call("ui.start", json!({"command": "joint.drive", "params": {"joint": "Rail", "values": [12.5]}}));
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(drive(&h), ("Rail".to_string(), vec!["12.5 mm".to_string()]));
        let hinge = h.app.session.doc.assembly.joints[0].id;
        h.call("ui.start", json!({"command": "joint.drive", "params": {"joint": hinge, "values": ["45 deg"]}}));
        assert_eq!(drive(&h), ("Hinge".to_string(), vec!["45 deg".to_string()]));
        assert_eq!(h.app.session.doc, before, "nothing is driven until OK");
        for bad in [json!({"joint": "Nope"}), json!({"joint": "Rail", "value": 3}), json!({"values": [1, 2]}), json!({"values": [f64::MAX]})] {
            let r = h.call("ui.start", json!({"command": "joint.drive", "params": bad}));
            assert_eq!(r["ok"], false, "{bad}: {r}");
            assert!(h.app.dialog.is_none(), "{bad}");
        }
        for command in ["solid.extrude", "sketch.create", "inspect.zebra"] {
            let r = h.call("ui.start", json!({"command": command, "params": {"distance": 5}}));
            assert!(r["ok"] == false && r["error"].as_str().unwrap().contains("takes no ui.start params"), "{r}");
            assert!(h.app.dialog.is_none() && h.app.tool.is_none() && h.app.session.analysis.is_none(), "{command} started nothing");
        }
    }
}
