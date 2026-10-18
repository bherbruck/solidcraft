//! User scenarios run headless: the real app (`SolveApp`) drawn frame by frame into an egui
//! context without a window, driven by the same requests as the control channel (clicks, keys,
//! drags, toolbar starts) the way a person would use it, with checks on the result.
//!
//! A scenario is a JSON list of steps:
//! - `{"start": "solid.extrude"}`: a toolbar click; `{"key": "E", "shift"?, "cmd"?}`; `{"text": "20"}`
//! - `{"widget": "Revolute", "near"?: "Type"}`: click a dialog field, choice or button by its
//!   text (with `near`: the one in that label's row)
//! - `{"click": AT, "double"?, "shift"?, "ctrl"?, "button"?}`, `{"move": AT, "shift"?, "alt"?, "ctrl"?, "cmd"?}`,
//!   `{"hold": AT, "frames"?}` (press and hold still: Select Other),
//!   `{"drag": [AT, AT], "shift"?, "ctrl"?, "steps"?}` where AT is `[x, y]` (screen), `{"world": [x,y,z]}`
//!   or `{"sketch": [x,y]}` (the sketch being edited)
//! - `{"queued": "engine.execute", "params": {…}, "min_frames"?: n, "max_frames"?: n}`: a request sent the way an
//!   agent's arrive (through the app's request queue, answered between frames, so the agent
//!   cursor can act it out first); fails if the answer came in fewer than `min_frames` frames
//! - `{"call": "ui.view", "params": {…}}`: any other control request (`fail: true` expects an
//!   error)
//! - `{"make_image": "pic.png"}`: a picture in the scenario's folder
//! - `{"choose_file": "part.step"}`: the next Open picker returns that file of the scenario's
//!   folder (Save pickers save there)
//! - `{"shot": "name"}`: a screenshot when run in a window; headless, a render of the model
//!   into `$SOLIDCRAFT_SCENARIO_SHOTS` when that is set; `{"debug": 1}`
//!   prints the UI state (`"widgets"`: also every widget's text; `"handles"`: every published
//!   handle, browser rows as `row:<label>` and their fold arrows as `fold:<label>`)
//! - `{"camera": {"yaw": rad, "pitch": rad}}` puts the camera at an orbit; `{"view_anim": "top",
//!   "max_deg": 10}` animates to a view and fails if any frame turns the view more than that
//! - AT may also be `{"plane": "XY"}` (the middle of an origin plane's square), `{"axis": "Z"}`,
//!   `{"dimension": "d1"}`, `{"handle": "arrow"}` (a manipulator handle: see [`publish_handle`]),
//!   `{"browser": "Bodies"}` (a browser row by its label) or `{"chevron": "Bodies"}` (its fold arrow)
//! - `{"autosave": true}`: autosave into the scenario's recovery folder; `{"restart": "crash" |
//!   "close"}`: the app dies (or closes) and a new one starts on the same folders
//! - `{"script": "examples/…/x.json"}`: run a command script through the engine; `"$DIR"` in a
//!   `call` step's string parameters is the scenario's folder
//! - `{"note": "…"}`: a comment; `{"pending": "why"}`: the scenario waits for a fix (its test
//!   is ignored)
//! - `{"expect": {…}}`: checks, see [`check`]; `{"until": {…}}` waits (frames) until they pass
//!   (`"status"` takes a substring of the status line, or null for none)
//!
//! The same files drive the live app over the control channel for screenshots.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::{Value, json};
use solidcraft_engine::Session;

use crate::control::{ControlRequest, Outcome, handle};
use crate::{Services, SolveApp};

thread_local! {
    /// Drag handles drawn this frame (name, screen point): manipulators publish theirs so
    /// scenarios (and the control channel's `ui.at {handle}`) can grab them.
    static HANDLES: RefCell<Vec<(String, egui::Pos2)>> = const { RefCell::new(Vec::new()) };
    /// Painted parts with no widget of their own (the toolbar, the workspace switcher…): rects
    /// and counts by name, for checks.
    static RECTS: RefCell<Vec<(String, egui::Rect)>> = const { RefCell::new(Vec::new()) };
    static COUNTS: RefCell<Vec<(String, f64)>> = const { RefCell::new(Vec::new()) };
    /// Widths a scenario's `same_width` checks remembered (by rect name).
    static WIDTHS: RefCell<Vec<(String, f32)>> = const { RefCell::new(Vec::new()) };
}

/// A manipulator's handle drawn this frame at `at` (screen), by name: "arrow" (a dialog's value
/// arrow), "triad_x"… Call it from the code that draws the handle, every frame it shows.
pub fn publish_handle(name: &str, at: egui::Pos2) {
    // The latest wins (lookups search from the end), so publishing is cheap for browser rows.
    HANDLES.with(|h| h.borrow_mut().push((name.to_string(), at)));
}

/// Where a handle was drawn last frame.
pub fn handle_at(name: &str) -> Option<egui::Pos2> {
    HANDLES.with(|h| h.borrow().iter().rfind(|(n, _)| n == name).map(|x| x.1))
}

/// Every handle drawn last frame whose name starts with `prefix`.
pub fn handles(prefix: &str) -> Vec<(String, egui::Pos2)> {
    HANDLES.with(|h| h.borrow().iter().filter(|(n, _)| n.starts_with(prefix)).cloned().collect())
}

/// A painted part's rect this frame, by name ("toolbar", "workspace_switcher").
pub fn publish_rect(name: &str, r: egui::Rect) {
    RECTS.with(|h| h.borrow_mut().push((name.to_string(), r)));
}

pub fn rect_of(name: &str) -> Option<egui::Rect> {
    RECTS.with(|h| h.borrow().iter().rfind(|(n, _)| n == name).map(|x| x.1))
}

/// A number about this frame's drawing, by name ("axis_view_highlights": the axis view's highlighted tips).
pub fn publish_count(name: &str, v: f64) {
    COUNTS.with(|h| h.borrow_mut().push((name.to_string(), v)));
}

pub fn count_of(name: &str) -> Option<f64> {
    COUNTS.with(|h| h.borrow().iter().rfind(|(n, _)| n == name).map(|x| x.1))
}

/// Forget last frame's handles, rects and counts (called as a frame starts).
pub fn clear_handles() {
    HANDLES.with(|h| h.borrow_mut().clear());
    RECTS.with(|h| h.borrow_mut().clear());
    COUNTS.with(|h| h.borrow_mut().clear());
}

/// `v` with "$DIR" in its strings replaced by `dir`.
fn with_dir(v: Value, dir: &str) -> Value {
    match v {
        Value::String(t) => Value::String(t.replace("$DIR", dir)),
        Value::Array(a) => Value::Array(a.into_iter().map(|x| with_dir(x, dir)).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, x)| (k, with_dir(x, dir))).collect()),
        other => other,
    }
}

/// A fresh folder for one scenario's files, under the temporary folder spelled with long names
/// (a short 8.3 `TEMP` would trip the short-name refusal before the scope checks the scenarios test).
fn scratch_dir() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = solidcraft_engine::io::vfs::temp_dir().join(format!("solidcraft-scenario-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&d);
    d
}

/// The app in a headless egui context.
pub struct Harness {
    pub app: SolveApp,
    pub ctx: egui::Context,
    /// The last frame's platform output (with its accessibility tree: dialog widgets are found
    /// by their text there).
    pub output: egui::PlatformOutput,
    /// Files the scenario saves and opens live here; file pickers answer from it.
    pub dir: PathBuf,
    /// The file the next Open picker returns (a `choose_file` step).
    next_file: Rc<RefCell<Option<String>>>,
    /// The app's request queue, opened by the first queued request.
    queue: Option<std::sync::mpsc::Sender<ControlRequest>>,
    time: f64,
    size: egui::Vec2,
    /// The camera as the last `camera` step left it.
    cam_mark: Option<solidcraft_engine::render::Camera>,
}

impl Default for Harness {
    fn default() -> Self {
        Harness::new()
    }
}

impl Drop for Harness {
    /// The scenario's files go with it (kept with SOLIDCRAFT_SCENARIO_KEEP set).
    fn drop(&mut self) {
        if solidcraft_engine::geom::env::var_os("SCENARIO_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl Harness {
    pub fn new() -> Harness {
        let mut h = Harness {
            app: SolveApp::new(Session::default(), Services::default()),
            ctx: egui::Context::default(),
            output: Default::default(),
            dir: scratch_dir(),
            next_file: Rc::new(RefCell::new(None)),
            queue: None,
            time: 0.0,
            size: egui::vec2(1600.0, 1000.0),
            cam_mark: None,
        };
        WIDTHS.with(|m| m.borrow_mut().clear());
        h.install();
        h.ctx.enable_accesskit();
        h.frames(3);
        h
    }

    /// File pickers answer like a person would: save into the scenario's folder, open the file
    /// the scenario chose.
    fn install(&mut self) {
        let dir = self.dir.clone();
        self.app.services.pick_save =
            Some(Box::new(move |name, _| Box::pin(std::future::ready(Some(dir.join(name).to_string_lossy().into_owned())))));
        let next = self.next_file.clone();
        self.app.services.pick_open = Some(Box::new(move |_| Box::pin(std::future::ready(next.borrow_mut().take()))));
    }

    /// Autosave into the scenario's recovery folder (as the desktop app does into the user's).
    fn start_autosave(&mut self) {
        crate::recovery_ui::start(&mut self.app, &self.dir.join("recovery"), std::time::Duration::ZERO);
    }

    /// Lay out and draw one frame (feeding queued pointer and key events).
    pub fn frame(&mut self) {
        self.time += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            time: Some(self.time),
            focused: true,
            ..Default::default()
        };
        self.app.raw_input_hook(&mut raw);
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        // Nothing is painted: drop the frame's texture changes.
        out.textures_delta.clear();
        self.output = out.platform_output;
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame();
        }
    }

    /// One frame in which the platform drops `files`, the pointer last seen at `at` (a drop
    /// itself carries no position with winit 0.30).
    pub fn drop_files(&mut self, at: egui::Pos2, files: Vec<egui::DroppedFileHandle>) {
        self.drag_files(at, Vec::new(), files);
    }

    /// One frame in which files are dragged over the window (`hovered`) or dropped, the pointer
    /// last seen at `at`.
    pub fn drag_files(&mut self, at: egui::Pos2, hovered: Vec<egui::HoveredFile>, dropped: Vec<egui::DroppedFileHandle>) {
        self.time += 1.0 / 60.0;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            time: Some(self.time),
            focused: true,
            events: vec![egui::Event::PointerMoved(at)],
            hovered_files: hovered,
            dropped_files: dropped,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        self.output = out.platform_output;
    }

    /// Frames until the queued input is used up (and the view has settled).
    pub fn settle(&mut self) {
        for _ in 0..2000 {
            // Queued input used up and the view no longer moving.
            if self.app.synthetic.is_empty() && self.app.cam_anim.is_none() && self.app.viewport.orbit_snap.is_none() {
                break;
            }
            self.frame();
        }
        // A file opening or recovering in the background: wait for it (as a person would).
        let began = std::time::Instant::now();
        while !self.app.file_load.idle() && began.elapsed() < std::time::Duration::from_secs(120) {
            self.frame();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.frames(4);
    }

    /// One control request, then frames until its input has been handled.
    /// Send a request through the app's queue, as the control channel does, and run frames
    /// until it is answered: (the reply, frames it took).
    pub fn call_queued(&mut self, method: &str, params: Value) -> (Value, usize) {
        let tx = self.queue.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            self.app.control_rx = Some(rx);
            tx
        });
        let (req, rx) = ControlRequest::new(method, params);
        if tx.send(req).is_err() {
            return (json!({"ok": false, "error": "the request queue is closed"}), 0);
        }
        for n in 1..=3000 {
            self.frame();
            if let Ok(v) = rx.try_recv() {
                self.settle();
                return (v, n);
            }
        }
        (json!({"ok": false, "error": "no answer in 3000 frames"}), 3000)
    }

    pub fn call(&mut self, method: &str, params: Value) -> Value {
        if method == "ui.resize"
            && let (Some(w), Some(h)) = (params["width"].as_f64(), params["height"].as_f64())
            && w.is_finite()
            && h.is_finite()
            && w > 0.0
            && h > 0.0
        {
            self.size = egui::vec2(w as f32, h as f32);
        }
        let (req, _rx) = ControlRequest::new(method, params);
        let out = match handle(&mut self.app, &self.ctx, &req) {
            Outcome::Done(v) => v,
            Outcome::Screenshot { .. } => json!({"ok": true, "result": null}),
        };
        self.settle();
        out
    }

    /// The centre of the visible widget whose label or value is `text` (the last one drawn wins:
    /// popups are drawn after what opened them).
    pub fn widget(&self, text: &str) -> Option<[f64; 2]> {
        self.widgets(text).into_iter().next()
    }

    /// The widget showing `text` nearest the label `near` (the field in a label's row).
    pub fn widget_near(&self, text: &str, near: &str) -> Option<[f64; 2]> {
        let [lx, ly] = self.widgets(near).into_iter().next()?;
        self.widgets(text).into_iter().filter(|[x, _]| *x > lx).min_by(|a, b| {
            let d = |p: &[f64; 2]| (p[1] - ly).abs() * 4.0 + (p[0] - lx).abs();
            d(a).total_cmp(&d(b))
        })
    }

    /// Centres of the visible widgets whose label or value is `text`, last drawn first.
    fn widgets(&self, text: &str) -> Vec<[f64; 2]> {
        let Some(tree) = self.output.accesskit_update.as_ref() else { return Vec::new() };
        let centre =
            |n: &egui::accesskit::Node| n.bounds().filter(|b| b.x1 > b.x0 && b.y1 > b.y0).map(|b| [(b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0]);
        let exact: Vec<[f64; 2]> =
            tree.nodes.iter().rev().filter(|(_, n)| n.label() == Some(text) || n.value() == Some(text)).filter_map(|(_, n)| centre(n)).collect();
        if !exact.is_empty() {
            return exact;
        }
        // A menu button's label ends with its shortcut ("Save Ctrl+S").
        tree.nodes
            .iter()
            .rev()
            .filter(|(_, n)| {
                n.role() == egui::accesskit::Role::Button && n.label().is_some_and(|l| l.strip_prefix(text).is_some_and(|r| r.starts_with(' ')))
            })
            .filter_map(|(_, n)| centre(n))
            .collect()
    }

    /// A step's point on screen.
    pub fn at(&mut self, v: &Value) -> Result<[f64; 2], String> {
        if v.is_array() {
            let x = v.get(0).and_then(Value::as_f64).ok_or_else(|| format!("not a point: {v}"))?;
            let y = v.get(1).and_then(Value::as_f64).ok_or_else(|| format!("not a point: {v}"))?;
            return Ok([x, y]);
        }
        let r = self.call("ui.at", v.clone());
        let p = &r["result"];
        match (p.get(0).and_then(Value::as_f64), p.get(1).and_then(Value::as_f64)) {
            (Some(x), Some(y)) => Ok([x, y]),
            _ => Err(format!("{v} is not on screen: {r}")),
        }
    }

    /// Animate the view to `v` (as the axis view does), sampling the camera every frame: no
    /// frame may turn the view (its right or back vector) by more than `max_deg`, the last
    /// included, so the move ends without a jump.
    fn view_anim(&mut self, v: &str, max_deg: f64) -> Result<(), String> {
        let basis = |c: &solidcraft_engine::render::Camera| c.basis();
        let mut prev = basis(&self.app.cam);
        self.app.animate_view(v);
        if self.app.cam_anim.is_none() && self.app.viewport.orbit_snap.is_none() {
            return Err(format!("view_anim {v}: no animation started"));
        }
        let mut worst = (0.0f64, 0usize);
        for i in 0..240 {
            self.frame();
            let now = basis(&self.app.cam);
            let ang = |a: solidcraft_engine::geom::Vec3, b: solidcraft_engine::geom::Vec3| a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
            let d = ang(prev.0, now.0).max(ang(prev.2, now.2));
            if d > worst.0 {
                worst = (d, i);
            }
            prev = now;
            if self.app.cam_anim.is_none() && self.app.viewport.orbit_snap.is_none() {
                if worst.0 > max_deg {
                    return Err(format!("view_anim {v}: frame {} of {} turns the view {:.1}° (limit {max_deg}°)", worst.1 + 1, i + 1, worst.0));
                }
                return Ok(());
            }
        }
        Err(format!("view_anim {v}: still animating after 240 frames"))
    }

    /// Run one step.
    pub fn step(&mut self, s: &Value) -> Result<(), String> {
        let flag = |k: &str| s.get(k).and_then(Value::as_bool).unwrap_or(false);
        let res =
            if let Some(id) = s.get("start").and_then(Value::as_str) {
                self.call("ui.start", json!({ "command": id }))
            } else if let Some(k) = s.get("key").and_then(Value::as_str) {
                self.call("ui.key", json!({"key": k, "shift": flag("shift"), "cmd": flag("cmd")}))
            } else if let Some(t) = s.get("text").and_then(Value::as_str) {
                self.call("ui.text", json!({ "text": t }))
            } else if let Some(at) = s.get("click") {
                let [x, y] = self.at(at)?;
                // Hover first, as a hand does.
                self.call("ui.move", json!({"x": x, "y": y}));
                let mut p = json!({"x": x, "y": y, "double": flag("double"), "shift": flag("shift"), "ctrl": flag("ctrl"), "alt": flag("alt")});
                if let Some(b) = s.get("button") {
                    p["button"] = b.clone();
                }
                self.call("ui.click", p)
            } else if let Some(text) = s.get("widget").and_then(Value::as_str) {
                // A dialog field, choice or button, found by its text.
                let found = match s.get("near").and_then(Value::as_str) {
                    Some(near) => self.widget_near(text, near),
                    None => self.widget(text),
                };
                let [x, y] = found.ok_or_else(|| format!("no widget `{text}` on screen"))?;
                self.call("ui.move", json!({"x": x, "y": y}));
                self.call("ui.click", json!({"x": x, "y": y}))
            } else if let Some(at) = s.get("hold") {
                // Press and hold still (Select Other): `frames` with the button down (default 40,
                // about 0.67 s), then let go.
                let [x, y] = self.at(at)?;
                self.call("ui.move", json!({"x": x, "y": y}));
                let frames = s.get("frames").and_then(Value::as_u64).unwrap_or(40);
                self.call(
                    "ui.drag",
                    json!({"x0": x, "y0": y, "x1": x, "y1": y, "steps": 1, "hold": frames, "shift": flag("shift"), "ctrl": flag("ctrl")}),
                )
            } else if let Some(at) = s.get("move") {
                let [x, y] = self.at(at)?;
                self.call("ui.move", json!({"x": x, "y": y,"shift":flag("shift"),"alt":flag("alt"),"ctrl":flag("ctrl"),"cmd":flag("cmd")}))
            } else if let Some(d) = s.get("drag") {
                let [x0, y0] = self.at(d.get(0).unwrap_or(&Value::Null))?;
                let [x1, y1] = self.at(d.get(1).unwrap_or(&Value::Null))?;
                let steps = s.get("steps").cloned().unwrap_or(json!(8));
                self.call(
                    "ui.drag",
                    json!({"x0": x0, "y0": y0, "x1": x1, "y1": y1, "steps": steps, "shift": flag("shift"), "ctrl": flag("ctrl"), "alt": flag("alt")}),
                )
            } else if let Some(m) = s.get("queued").and_then(Value::as_str) {
                let (v, frames) = self.call_queued(m, s.get("params").cloned().unwrap_or(json!({})));
                let min = s.get("min_frames").and_then(Value::as_u64).unwrap_or(0);
                if (frames as u64) < min {
                    return Err(format!("{m} was answered after {frames} frames: the agent cursor did not act it out ({min})"));
                }
                if let Some(max) = s.get("max_frames").and_then(Value::as_u64)
                    && frames as u64 > max
                {
                    return Err(format!("{m} was answered after {frames} frames, more than {max}"));
                }
                v
            } else if let Some(f) = s.get("script").and_then(Value::as_str) {
                // A command script (`{"commands": [{command, params}]}`, path from the repository
                // root) run through the engine, as `solidcraft-cli run` does.
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(f);
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{f}: {e}"))?;
                let v: Value = serde_json::from_str(&text).map_err(|e| format!("{f}: {e}"))?;
                for (i, c) in v["commands"].as_array().into_iter().flatten().enumerate() {
                    let r = self.call("engine.execute", json!({"command": c["command"], "params": c.get("params").cloned().unwrap_or(json!({}))}));
                    if r["ok"] != true {
                        return Err(format!("{f}: step {} ({}): {}", i + 1, c["command"], r["error"]));
                    }
                }
                self.frames(2);
                return Ok(());
            } else if let Some(m) = s.get("call").and_then(Value::as_str) {
                // "$DIR" in a string parameter is the scenario's folder.
                let params = with_dir(s.get("params").cloned().unwrap_or(json!({})), &self.dir.to_string_lossy());
                self.call(m, params)
            } else if s.get("note").is_some() || s.get("pending").is_some() {
                return Ok(());
            } else if let Some(name) = s.get("shot").and_then(Value::as_str) {
                // With SOLIDCRAFT_SCENARIO_SHOTS set: a render of the model as the camera sees it.
                if let Ok(dir) = solidcraft_engine::geom::env::var("SCENARIO_SHOTS").ok_or(()) {
                    // Drawn on the GPU: the headless device, as the CLI draws.
                    #[cfg(not(target_arch = "wasm32"))]
                    if solidcraft_engine::capture::installed().is_none() {
                        solidcraft_engine::capture::install(std::sync::Arc::new(|scene, cam, w, h| {
                            let (rgba, renderer) = solidcraft_gpu::capture::headless_rgba(scene, cam, w, h)?;
                            Ok(solidcraft_engine::capture::Picture { rgba, renderer })
                        }));
                    }
                    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
                    let r = self.call("ui.render", json!({"path": path.to_string_lossy(), "width": 1000, "height": 700}));
                    if r["ok"] != true {
                        return Err(format!("shot {name}: {}", r["error"]));
                    }
                }
                return Ok(());
            } else if let Some(c) = s.get("camera") {
                // Put the camera at an orbit: {"yaw": rad, "pitch": rad}.
                if let Some(y) = c.get("yaw").and_then(Value::as_f64) {
                    self.app.cam.yaw = y;
                }
                if let Some(p) = c.get("pitch").and_then(Value::as_f64) {
                    self.app.cam.pitch = p;
                }
                self.app.cam_anim = None;
                self.frames(2);
                // Where `camera.panned` and `camera.zoomed` checks measure from.
                self.cam_mark = Some(self.app.cam);
                return Ok(());
            } else if let Some(v) = s.get("view_anim").and_then(Value::as_str) {
                return self.view_anim(v, s.get("max_deg").and_then(Value::as_f64).unwrap_or(10.0));
            } else if s.get("autosave").is_some() {
                self.start_autosave();
                return Ok(());
            } else if let Some(how) = s.get("restart").and_then(Value::as_str) {
                // "crash": the app dies (its autosave lock goes with it, the entry stays);
                // "close": it closes normally. Then a new app starts on the same folders.
                let mut old = std::mem::replace(&mut self.app, SolveApp::new(Session::default(), Services::default()));
                if how == "close" {
                    crate::recovery_ui::close(&mut old);
                    // The desktop saves its settings on close and loads them on the next start.
                    let prefs = old.prefs();
                    self.app.load_prefs(&prefs);
                }
                drop(old);
                self.ctx = egui::Context::default();
                self.ctx.enable_accesskit();
                self.install();
                self.start_autosave();
                self.frames(3);
                return Ok(());
            } else if let Some(f) = s.get("make_image").and_then(Value::as_str) {
                // A picture to insert (a 200 x 100 checkerboard PNG in the scenario's folder).
                let img = image::RgbImage::from_fn(200, 100, |x, y| {
                    if (x / 20 + y / 20) % 2 == 0 { image::Rgb([230, 230, 230]) } else { image::Rgb([60, 90, 160]) }
                });
                img.save(self.dir.join(f)).map_err(|e| format!("make_image: {e}"))?;
                return Ok(());
            } else if let Some(f) = s.get("choose_file").and_then(Value::as_str) {
                *self.next_file.borrow_mut() = Some(self.dir.join(f).to_string_lossy().into_owned());
                return Ok(());
            } else if let Some(m) = s.get("print").and_then(Value::as_str) {
                let r = self.call(m, s.get("params").cloned().unwrap_or(json!({})));
                eprintln!("print {m}: {r}");
                return Ok(());
            } else if s.get("debug").is_some() {
                let ui = self.call("ui.inspect", json!({}));
                let sel = self.call("ui.selection", json!({}));
                eprintln!(
                    "debug: dialog {} tool {} hover {}\nselection {}\nstatus {:?}",
                    ui["result"]["dialog"], ui["result"]["tool"], ui["result"]["hover"], sel["result"]["selection"], self.app.status
                );
                if s["debug"] == json!("widgets") {
                    let texts: Vec<String> = self
                        .output
                        .accesskit_update
                        .iter()
                        .flat_map(|t| t.nodes.iter())
                        .filter_map(|(_, n)| {
                            let at = n.bounds().map(|b| format!(" @{:.0},{:.0}", (b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0)).unwrap_or_default();
                            n.label().or(n.value()).map(|l| format!("{:?} {l}{at}", n.role()))
                        })
                        .collect();
                    eprintln!("widgets: {texts:?}");
                }
                if s["debug"] == json!("handles") {
                    let names: Vec<String> = HANDLES.with(|h| h.borrow().iter().map(|(n, p)| format!("{n} @{:.0},{:.0}", p.x, p.y)).collect());
                    eprintln!("handles: {names:?}");
                }
                return Ok(());
            } else if let Some(e) = s.get("until") {
                // Background work (a sample being built, a preview): frames until the checks pass.
                let mut last = String::new();
                for _ in 0..600 {
                    match check(self, e) {
                        Ok(()) => return Ok(()),
                        Err(m) => last = m,
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    self.frames(2);
                }
                return Err(format!("timed out: {last}"));
            } else if let Some(e) = s.get("expect") {
                return check(self, e);
            } else {
                return Err(format!("unknown step {s}"));
            };
        let failed = res["ok"] != json!(true);
        if failed != flag("fail") {
            return Err(format!("{s}: {res}"));
        }
        Ok(())
    }

    /// Run a scenario (its steps); the error names the failing step.
    pub fn run(&mut self, steps: &Value) -> Result<(), String> {
        for (i, s) in steps.as_array().ok_or("a scenario is a list of steps")?.iter().enumerate() {
            self.step(s).map_err(|e| format!("step {i}: {e}"))?;
        }
        Ok(())
    }
}

fn approx(got: f64, want: &Value, what: &str) -> Result<(), String> {
    let (w, tol) = match want {
        Value::Array(a) => (a.first().and_then(Value::as_f64).unwrap_or(f64::NAN), a.get(1).and_then(Value::as_f64).unwrap_or(1e-3)),
        v => (v.as_f64().unwrap_or(f64::NAN), 1e-3),
    };
    if (got - w).abs() <= tol * w.abs().max(1.0) { Ok(()) } else { Err(format!("{what}: got {got}, want {w} (±{tol} rel)")) }
}

/// Checks: `bodies` (count), `volume` / `area` (total, number or [value, rel tol]), `faces`,
/// `features` (count), `errors` (timeline features in error, default 0), `dialog` (null: none
/// open, or a word its kind must contain), `tool` (null or command id), `status` (null, or a
/// substring of the status line), `sketching` (bool),
/// `selection` (count), `glyphs` ({constraint id: count}, none overlapping one another or a dimension value), `param` ({name: value}), `sketch_status` (solved|…), `dof`, `query`
/// ({command, params?, path, value}: a command's result).
pub fn check(h: &mut Harness, e: &Value) -> Result<(), String> {
    let doc = h.call("document.inspect", json!({"measure": true}))["result"].clone();
    let ui = h.call("ui.inspect", json!({}))["result"].clone();
    let sum = |k: &str| doc["bodies"].as_array().into_iter().flatten().filter_map(|b| b[k].as_f64()).sum::<f64>();
    let total = json!({"volume_mm3": sum("volume_mm3"), "area_mm2": sum("area_mm2"), "faces": sum("faces") as u64});
    let total = &total;
    let bodies = doc["bodies"].as_array().map_or(0, Vec::len);
    let obj = e.as_object().ok_or("expect takes an object")?;
    for (k, v) in obj {
        match k.as_str() {
            "bodies" => {
                if v.as_u64() != Some(bodies as u64) {
                    return Err(format!("bodies: got {bodies}, want {v}"));
                }
            }
            "volume" => approx(total["volume_mm3"].as_f64().unwrap_or(f64::NAN), v, "volume")?,
            "area" => approx(total["area_mm2"].as_f64().unwrap_or(f64::NAN), v, "area")?,
            "faces" => {
                if total["faces"] != *v {
                    return Err(format!("faces: got {}, want {v}", total["faces"]));
                }
            }
            "features" => {
                let n = doc["timeline"].as_array().map_or(0, Vec::len);
                if v.as_u64() != Some(n as u64) {
                    return Err(format!("features: got {n}, want {v}"));
                }
            }
            "errors" => {
                let bad: Vec<String> = doc["timeline"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|f| !f["error"].is_null())
                    .map(|f| format!("{}: {}", f["name"], f["error"]))
                    .collect();
                if v.as_u64() != Some(bad.len() as u64) {
                    return Err(format!("errors: {bad:?}"));
                }
            }
            "shown_handles" | "hidden_handles" => {
                // Published handles ("fold:Bodies", "row:Body1") drawn (or not) this frame.
                for n in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if handle_at(n).is_some() != (k == "shown_handles") {
                        return Err(format!("{n}: want {}", if k == "shown_handles" { "shown" } else { "hidden" }));
                    }
                }
            }
            "handle_order" => {
                // Published handles left to right in this order.
                let xs: Vec<(String, Option<f32>)> =
                    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(|n| (n.to_string(), handle_at(n).map(|p| p.x))).collect();
                if xs.iter().any(|x| x.1.is_none()) || xs.windows(2).any(|w| w[0].1 >= w[1].1) {
                    return Err(format!("handle order: got {xs:?}"));
                }
            }
            "camera" => {
                // {"camera": {"yaw_deg": y, "pitch_deg": p, "back"?: [x,y,z]}}: where the camera
                // looks from (yaw and pitch in its up-axis frame, ±0.5°),
                // once any view animation has finished.
                for _ in 0..240 {
                    if h.app.cam_anim.is_none() {
                        break;
                    }
                    h.frame();
                }
                let (yaw, pitch) = (h.app.cam.yaw.to_degrees(), h.app.cam.pitch.to_degrees());
                let near = |got: f64, want: &Value| want.as_f64().is_none_or(|w| ((got - w + 540.0).rem_euclid(360.0) - 180.0).abs() < 0.5);
                if !near(yaw, &v["yaw_deg"]) || !near(pitch, &v["pitch_deg"]) {
                    return Err(format!("camera yaw {yaw:.2}°, pitch {pitch:.2}°, want {v}"));
                }
                // Since the last `camera` step: "panned" (bool: the target moved) and "zoomed"
                // ("in", "out" or "no": the eye's distance from the target).
                if !v["panned"].is_null() || !v["zoomed"].is_null() {
                    let mark = h.cam_mark.ok_or("camera.panned/zoomed need a `camera` step first")?;
                    let c = &h.app.cam;
                    let moved = c.target.dist(mark.target) > 1e-6 * mark.distance.max(1.0);
                    if v["panned"].as_bool().is_some_and(|want| want != moved) {
                        return Err(format!("camera panned: got {moved}, want {}", v["panned"]));
                    }
                    let r = c.distance / mark.distance;
                    let zoomed = if r < 1.0 - 1e-6 {
                        "in"
                    } else if r > 1.0 + 1e-6 {
                        "out"
                    } else {
                        "no"
                    };
                    if v["zoomed"].as_str().is_some_and(|want| want != zoomed) {
                        return Err(format!("camera zoomed: got {zoomed} (distance ×{r:.4}), want {}", v["zoomed"]));
                    }
                }
                // "back": the world direction from the target to the eye.
                if let Some(want) = v["back"].as_array() {
                    let b = h.app.cam.back();
                    let w = |i: usize| want.get(i).and_then(Value::as_f64).unwrap_or(f64::NAN);
                    if (b.x - w(0)).abs() > 1e-3 || (b.y - w(1)).abs() > 1e-3 || (b.z - w(2)).abs() > 1e-3 {
                        return Err(format!("camera looks from {b:?}, want {want:?}"));
                    }
                }
            }
            "count" => {
                // {"count": {"name": n, "value": x}}: a published count.
                let n = v["name"].as_str().unwrap_or_default();
                let got = count_of(n);
                if got != v["value"].as_f64() {
                    return Err(format!("count {n}: got {got:?}, want {}", v["value"]));
                }
            }
            "same_width" => {
                // {"same_width": "name"}: the published rect is as wide as the first time this
                // scenario checked it (±0.5 px): a window that keeps its size.
                let n = v.as_str().unwrap_or_default();
                let w = rect_of(n).ok_or(format!("no rect {n}"))?.width();
                let first = WIDTHS.with(|m| {
                    let mut m = m.borrow_mut();
                    match m.iter().find(|(k, _)| k == n) {
                        Some(&(_, w0)) => w0,
                        None => {
                            m.push((n.to_string(), w));
                            w
                        }
                    }
                });
                if (w - first).abs() > 0.5 {
                    return Err(format!("{n} is {w:.1} px wide, was {first:.1} px"));
                }
            }
            "right_edge" => {
                // {"right_edge": {"inner": a, "outer": b, "tol": px}}: published rect a ends at
                // b's right edge (within tol).
                let (a, b) = (v["inner"].as_str().unwrap_or_default(), v["outer"].as_str().unwrap_or_default());
                let (ra, rb) = (rect_of(a).ok_or(format!("no rect {a}"))?, rect_of(b).ok_or(format!("no rect {b}"))?);
                let tol = v["tol"].as_f64().unwrap_or(2.0) as f32;
                if (ra.right() - rb.right()).abs() > tol {
                    return Err(format!("{a} ends at x={:.1}, {b} at x={:.1} (±{tol})", ra.right(), rb.right()));
                }
            }
            "spans" => {
                // {"spans": {"inner": a, "outer": b, "axis": "y", "tol": px}}: published rect a
                // covers b's extent along the axis (within tol).
                let (a, b) = (v["inner"].as_str().unwrap_or_default(), v["outer"].as_str().unwrap_or_default());
                let (ra, rb) = (rect_of(a).ok_or(format!("no rect {a}"))?, rect_of(b).ok_or(format!("no rect {b}"))?);
                let tol = v["tol"].as_f64().unwrap_or(4.0) as f32;
                let (lo, hi) = if v["axis"] == "x" { (ra.x_range(), rb.x_range()) } else { (ra.y_range(), rb.y_range()) };
                if (lo.min - hi.min).abs() > tol || (lo.max - hi.max).abs() > tol {
                    return Err(format!("{a} {lo:?} does not span {b} {hi:?} (±{tol})"));
                }
            }
            "disjoint" => {
                // {"disjoint": ["a", "b"]}: both published rects are on screen and neither
                // covers any part of the other (a dialog beside the Sketch Palette).
                let names: Vec<&str> = v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let (Some(a), Some(b)) = (names.first(), names.get(1)) else {
                    return Err("disjoint takes two published rect names".to_string());
                };
                let ra = rect_of(a).ok_or(format!("no rect {a}"))?;
                let rb = rect_of(b).ok_or(format!("no rect {b}"))?;
                if ra.intersect(rb).is_positive() {
                    return Err(format!("{a} {ra:?} overlaps {b} {rb:?}"));
                }
            }
            "dialog_docked" => {
                // The command dialog: docked at the viewport's right, its width in [min, max].
                // Its window id carries the dialog's title, so read the rect the dialog
                // publishes each frame rather than looking up a fixed id.
                let r = h.app.viewport.dialog_rect.filter(|_| h.app.dialog.is_some()).ok_or("no command dialog")?;
                let vp = h.app.viewport.rect.unwrap_or(egui::Rect::NOTHING);
                let (lo, hi) = (v["min"].as_f64().unwrap_or(260.0) as f32, v["max"].as_f64().unwrap_or(380.0) as f32);
                if r.width() < lo - 0.5 || r.width() > hi + 0.5 {
                    return Err(format!("dialog width {:.0}, want {lo}..{hi}", r.width()));
                }
                if (r.right() - vp.right()).abs() > 2.0 || r.top() < vp.top() || r.bottom() > vp.bottom() + 1.0 {
                    return Err(format!("dialog {r:?} is not docked right inside the viewport {vp:?}"));
                }
            }
            "windows_fit" => {
                // Every open window lies on screen and is at most `max_width` wide.
                let screen = h.ctx.content_rect();
                let max_w = v["max_width"].as_f64().unwrap_or(720.0) as f32;
                let rects: Vec<(egui::Id, egui::Rect)> = h.ctx.memory(|m| {
                    m.areas()
                        .visible_layer_ids()
                        .into_iter()
                        .filter(|l| l.order == egui::Order::Middle)
                        .filter_map(|l| m.area_rect(l.id).map(|r| (l.id, r)))
                        .collect()
                });
                for (id, r) in rects {
                    if r.width() > max_w || !screen.expand(1.0).contains_rect(r) {
                        return Err(format!("window {id:?} at {r:?} (width {:.0}) does not fit {screen:?} at ≤{max_w}", r.width()));
                    }
                }
            }
            "dialog" => {
                let d = &ui["dialog"];
                let ok = match v {
                    Value::Null => d.is_null(),
                    Value::String(w) => d.as_str().is_some_and(|d| d.contains(w.as_str())),
                    _ => false,
                };
                if !ok {
                    return Err(format!("dialog: got {d}, want {v}"));
                }
            }
            "tool" => {
                if ui["tool"] != *v {
                    return Err(format!("tool: got {}, want {v}", ui["tool"]));
                }
            }
            "status" => {
                // A substring of the status line's text, or null for no status at all.
                let got = &ui["status"]["text"];
                let ok = match v {
                    Value::Null => ui["status"].is_null(),
                    Value::String(w) => got.as_str().is_some_and(|g| g.contains(w.as_str())),
                    _ => false,
                };
                if !ok {
                    return Err(format!("status: got {}, want {v}", ui["status"]));
                }
            }
            "glyphs" => {
                // {constraint id: glyphs drawn for it}; glyphs never overlap one another.
                let g = crate::sketch_tools::glyph_positions();
                for (id, n) in v.as_object().ok_or("glyphs takes {constraint id: count}")? {
                    let got = g.iter().filter(|(x, _)| x == id).count();
                    if n.as_u64() != Some(got as u64) {
                        return Err(format!("glyphs of {id}: got {got}, want {n} (drawn: {g:?})"));
                    }
                }
                for (i, (a, p)) in g.iter().enumerate() {
                    if let Some((b, _)) = g.iter().skip(i + 1).find(|(_, q)| q.distance(*p) < 14.0) {
                        return Err(format!("glyphs of {a} and {b} overlap at {p:?}"));
                    }
                    let r = egui::Rect::from_center_size(*p, egui::vec2(14.0, 14.0));
                    if let Some((d, _)) = crate::dim_view::text_rects().into_iter().find(|(_, t)| t.intersects(r)) {
                        return Err(format!("glyph of {a} at {p:?} overlaps the value of dimension {d}"));
                    }
                }
            }
            "sketching" => {
                if doc["active_sketch"].is_null() == v.as_bool().unwrap_or(false) {
                    return Err(format!("sketching: want {v}, active sketch {}", doc["active_sketch"]));
                }
            }
            "selection" => {
                let n = doc["selection"].as_array().map_or(0, Vec::len);
                if v.as_u64() != Some(n as u64) {
                    return Err(format!("selection: got {n}, want {v}"));
                }
            }
            "param" => {
                for (name, want) in v.as_object().into_iter().flatten() {
                    let p = doc["params"].as_array().into_iter().flatten().find(|p| p["name"] == json!(name)).cloned().unwrap_or_default();
                    approx(p["value"].as_f64().unwrap_or(f64::NAN), want, &format!("param {name}"))?;
                }
            }
            "sketch_status" | "dof" => {
                let si = h.call("engine.execute", json!({"command": "sketch.inspect", "params": {}}))["result"].clone();
                let key = if k == "dof" { "dof" } else { "status" };
                if si[key] != *v {
                    return Err(format!("{k}: got {}, want {v}", si[key]));
                }
            }
            "file" => {
                // A file the scenario saved: it exists and is not empty.
                let p = h.dir.join(v.as_str().unwrap_or_default());
                if std::fs::metadata(&p).map_or(0, |m| m.len()) == 0 {
                    return Err(format!("no file {}", p.display()));
                }
            }
            "file_contains" => {
                // {file, text}: a saved text file holds the text.
                let p = h.dir.join(v["file"].as_str().unwrap_or_default());
                let body = std::fs::read_to_string(&p).unwrap_or_default();
                let t = v["text"].as_str().unwrap_or_default();
                if !body.contains(t) {
                    return Err(format!("{} does not contain `{t}`", p.display()));
                }
            }
            "hidden" => {
                // Bodies hidden in the view.
                let h = &ui["visibility"]["hidden_bodies"];
                if h != v {
                    return Err(format!("hidden bodies: got {h}, want {v}"));
                }
            }
            "canvas_width" => {
                // The first canvas's width (mm).
                let w = h.app.session.doc.canvases.first().map(|c| c.width).unwrap_or(f64::NAN);
                approx(w, v, "canvas width")?;
            }
            "section" => {
                if h.app.session.section.is_some() != v.as_bool().unwrap_or(false) {
                    return Err(format!("section view: want {v}"));
                }
            }
            "camera_matches" => {
                // The camera is at the saved named view (yaw, pitch, target).
                let n = v.as_str().unwrap_or_default();
                let nv = h.app.session.doc.named_views.iter().find(|x| x.name == n).ok_or_else(|| format!("no named view {n}"))?;
                let c = &h.app.cam;
                if (c.yaw - nv.yaw).abs() > 1e-6 || (c.pitch - nv.pitch).abs() > 1e-6 || c.target.dist(nv.target) > 1e-6 {
                    return Err(format!("camera is not at view {n}: yaw {} pitch {}, view yaw {} pitch {}", c.yaw, c.pitch, nv.yaw, nv.pitch));
                }
            }
            "named_views" => {
                let n = h.app.session.doc.named_views.len();
                if v.as_u64() != Some(n as u64) {
                    return Err(format!("named views: got {n}, want {v}"));
                }
            }
            "bbox" => {
                // {body, min?: [x|null, y|null, z|null], max?: […]}: where a body is (±1e-3 mm).
                let name = v["body"].as_str().unwrap_or_default();
                // Where it is in the assembly (measured as placed).
                let m = h.call("engine.execute", json!({"command": "inspect.measure", "params": {"bodies": [name]}}))["result"].clone();
                let b = m["bodies"].as_array().and_then(|a| a.first()).cloned().ok_or_else(|| format!("no body {name}: {m}"))?;
                for side in ["min", "max"] {
                    for (i, want) in v[side].as_array().into_iter().flatten().enumerate() {
                        let (Some(w), Some(g)) = (want.as_f64(), b["bbox"][side].get(i).and_then(Value::as_f64)) else { continue };
                        if (w - g).abs() > 1e-3 {
                            return Err(format!("{name} bbox {side}[{i}]: got {g}, want {w}"));
                        }
                    }
                }
            }
            "sketch_curves" => {
                // {sketch?, linked?, own?}: curves projected (linked) and drawn in a sketch.
                let mut params = json!({});
                if let Some(n) = v.get("sketch") {
                    params["sketch"] = n.clone();
                }
                let si = h.call("engine.execute", json!({"command": "sketch.inspect", "params": params}))["result"].clone();
                let curves = si["curves"].as_array().cloned().unwrap_or_default();
                let linked = curves.iter().filter(|c| !c["link"].is_null()).count();
                let own = curves.len() - linked;
                for (k, got) in [("linked", linked), ("own", own)] {
                    if let Some(w) = v.get(k).and_then(Value::as_u64)
                        && w != got as u64
                    {
                        return Err(format!("sketch curves {k}: got {got}, want {w}"));
                    }
                }
            }
            "body_volume" => {
                // {body, volume}: one body's volume.
                let name = v["body"].as_str().unwrap_or_default();
                let m = h.call("engine.execute", json!({"command": "inspect.measure", "params": {"bodies": [name]}}))["result"].clone();
                let got = m["bodies"][0]["volume_mm3"].as_f64().unwrap_or(f64::NAN);
                approx(got, &v["volume"], &format!("{name} volume"))?;
            }
            "linked_world_range" => {
                // {"x"|"y"|"z": [min, max]}: where the active sketch's projected (linked) points lie
                // in the world (±1e-3 mm).
                let si = h.call("engine.execute", json!({"command": "sketch.inspect", "params": {}}))["result"].clone();
                let pts: Vec<Value> = si["points"].as_array().into_iter().flatten().filter(|p| !p["link"].is_null()).cloned().collect();
                if pts.is_empty() {
                    return Err("the sketch has no projected points".into());
                }
                for (i, axis) in ["x", "y", "z"].iter().enumerate() {
                    let Some(want) = v.get(*axis) else { continue };
                    let xs: Vec<f64> = pts.iter().filter_map(|p| p["world"][i].as_f64()).collect();
                    let (lo, hi) = xs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &x| (a.min(x), b.max(x)));
                    let (wl, wh) = (want[0].as_f64().unwrap_or(f64::NAN), want[1].as_f64().unwrap_or(f64::NAN));
                    if (lo - wl).abs() > 1e-3 || (hi - wh).abs() > 1e-3 {
                        return Err(format!("projected points span {axis} {lo:.3}..{hi:.3}, want {wl}..{wh}"));
                    }
                }
            }
            "sketch_world" => {
                // {sketch, point, at: [x,y,z]}: where a sketch point is in the world (±1e-3 mm).
                let si = h.call("engine.execute", json!({"command": "sketch.inspect", "params": {"sketch": v["sketch"]}}))["result"].clone();
                let p = si["points"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["id"] == v["point"])
                    .cloned()
                    .ok_or_else(|| format!("no point {}", v["point"]))?;
                for i in 0..3 {
                    let (g, w) = (p["world"][i].as_f64().unwrap_or(f64::NAN), v["at"][i].as_f64().unwrap_or(f64::NAN));
                    if (g - w).abs() > 1e-3 {
                        return Err(format!("{} of {} is at {}, want {}", v["point"], v["sketch"], p["world"], v["at"]));
                    }
                }
            }
            "shown" | "not_shown" => {
                // A widget with this text is (not) on screen.
                let t = v.as_str().unwrap_or_default();
                if h.widget(t).is_some() != (k == "shown") {
                    return Err(format!("widget `{t}`: want {}", if k == "shown" { "shown" } else { "not shown" }));
                }
            }
            "home" => {
                if ui["home"] != *v {
                    return Err(format!("start page open: got {}, want {v}", ui["home"]));
                }
            }
            "prefs" => {
                // {key: value…}: those entries of `ui.prefs` (preferences, `dialog`, `dark`…).
                let got = h.call("ui.prefs", json!({}))["result"].clone();
                for (pk, pv) in v.as_object().into_iter().flatten() {
                    if got[pk.as_str()] != *pv {
                        return Err(format!("preference {pk}: got {}, want {pv}", got[pk.as_str()]));
                    }
                }
            }
            "file_busy" | "quit_requested" | "quitting" => {
                if ui[k] != *v {
                    return Err(format!("{k}: got {}, want {v}", ui[k]));
                }
            }
            "documents" => {
                if ui["documents"] != *v {
                    return Err(format!("open designs: got {}, want {v}", ui["documents"]));
                }
            }
            "name" => {
                if doc["name"] != *v {
                    return Err(format!("design name: got {}, want {v}", doc["name"]));
                }
            }
            "browser_has" | "browser_lacks" => {
                // Browser rows on screen, by label.
                let r = h.call("ui.browser", json!({}));
                let labels: Vec<String> =
                    r["result"]["rows"].as_array().into_iter().flatten().filter_map(|x| x["label"].as_str().map(str::to_string)).collect();
                for want in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
                    let there = labels.iter().any(|l| l == want);
                    if there != (k.as_str() == "browser_has") {
                        return Err(format!("browser row `{want}` {}: rows {labels:?}", if there { "still shown" } else { "missing" }));
                    }
                }
            }
            "browser_selected" => {
                // Nothing selected and no component picked (a fold click selects nothing).
                let r = h.call("ui.browser", json!({}));
                let none = r["result"]["selection"].as_array().is_none_or(|a| a.is_empty())
                    && r["result"]["picked_components"].as_array().is_none_or(|a| a.is_empty());
                if none == v.as_bool().unwrap_or(false) {
                    return Err(format!("browser selection: {}", r["result"]));
                }
            }
            "query" => {
                // {command, params?, path: [keys or indices], value}: a command's result at path.
                let cmd = v["command"].as_str().ok_or("query needs `command`")?;
                let r = h.call("engine.execute", json!({"command": cmd, "params": v.get("params").cloned().unwrap_or(json!({}))}));
                let mut at = &r["result"];
                for k in v["path"].as_array().into_iter().flatten() {
                    at = match k {
                        Value::Number(n) => &at[n.as_u64().unwrap_or(0) as usize],
                        Value::String(s) => &at[s.as_str()],
                        _ => &Value::Null,
                    };
                }
                let want = &v["value"];
                let ok = match (at.as_f64(), want.as_f64()) {
                    (Some(a), Some(w)) => (a - w).abs() <= 1e-6 * w.abs().max(1.0),
                    _ => at == want,
                };
                if !ok {
                    return Err(format!("query {cmd} {}: got {at}, want {want}", v["path"]));
                }
            }
            other => return Err(format!("unknown check `{other}`")),
        }
    }
    Ok(())
}
