//! The agent cursor (#35): a second, clearly marked pointer that acts out what an agent does
//! over the control channel or MCP. Before an `engine.execute` runs, the pointer glides to the
//! command's toolbar button (or its panel when the button sits in a drop-down) and clicks it,
//! then glides to each pick in the parameters (faces, edges and points projected to the screen,
//! sketch points, the plane of a new sketch), and only then does the command run. The reply
//! waits for the animation, so an agent's timing stays honest. With "follow camera" on, a new
//! sketch turns the view to face it and finishing the sketch turns back.
//!
//! The pointer is drawn by the app, so screen recordings capture it. It is off by default (View
//! settings on the navigation bar, or `ui.agent_cursor {show, speed, follow_camera}`); a call
//! with `"animate": false` skips it.

use std::collections::VecDeque;
use std::sync::mpsc::Sender;

#[cfg(test)]
use egui::vec2;
use egui::{Pos2, Stroke, pos2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_engine::geom::{Vec2, Vec3};

use crate::SolveApp;
use crate::control::ControlResponse;

/// How fast the pointer moves: 1× to 5×, or Instant (no animation: the command runs at once
/// and the pointer jumps to where it acted, with a click ripple). Saved and sent as a number,
/// or "instant"; the old "slow" and "normal" read as 1×.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Speed {
    Times(f64),
    Instant,
}

impl Default for Speed {
    fn default() -> Self {
        Speed::Times(1.0)
    }
}

impl Speed {
    /// From a number (1–5) or a name ("instant"; "slow" and "normal" are 1×).
    pub fn from_value(v: &Value) -> Option<Speed> {
        match v {
            Value::Number(n) => n.as_f64().filter(|x| x.is_finite() && (1.0..=5.0).contains(x)).map(Speed::Times),
            Value::String(s) => match s.to_ascii_lowercase().as_str() {
                "instant" => Some(Speed::Instant),
                "slow" | "normal" => Some(Speed::Times(1.0)),
                "fast" => Some(Speed::Times(2.0)),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn label(self) -> String {
        match self {
            Speed::Instant => "Instant".into(),
            Speed::Times(x) => {
                let s = format!("{x:.1}");
                format!("{}×", s.trim_end_matches(".0"))
            }
        }
    }
    /// Seconds to glide to a stop, and to click there.
    fn times(self) -> (f64, f64) {
        match self {
            Speed::Instant => (0.0, 0.0),
            Speed::Times(x) => {
                let x = x.clamp(1.0, 5.0);
                (0.45 / x, 0.25 / x)
            }
        }
    }
}

impl Serialize for Speed {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Speed::Instant => s.serialize_str("instant"),
            Speed::Times(x) => s.serialize_f64(*x),
        }
    }
}

impl<'de> Deserialize<'de> for Speed {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(Speed::from_value(&v).unwrap_or_default())
    }
}

/// The speed as a slider: 1× to 5× in half steps, and Instant at the far right.
pub fn speed_slider(ui: &mut egui::Ui, speed: &mut Speed) -> egui::Response {
    let mut v = match *speed {
        Speed::Times(x) => x.clamp(1.0, 5.0),
        Speed::Instant => 5.5,
    };
    let r = ui.add(
        egui::Slider::new(&mut v, 1.0..=5.5).step_by(0.5).custom_formatter(|v, _| if v > 5.25 { "Instant".into() } else { Speed::Times(v).label() }),
    );
    if r.changed() {
        *speed = if v > 5.25 { Speed::Instant } else { Speed::Times(v) };
    }
    r
}

/// The settings (kept with the other view settings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub show: bool,
    pub speed: Speed,
    pub follow_camera: bool,
}

/// A request held back while the pointer acts it out.
pub struct Job {
    method: String,
    params: Value,
    reply: Sender<ControlResponse>,
    stops: VecDeque<Pos2>,
    /// The leg under way: where it started and when.
    from: Pos2,
    started: f64,
    /// Clicked at the current stop (waiting out the click).
    clicked: bool,
}

/// The pointer's state.
#[derive(Default)]
pub struct AgentCursor {
    pub pos: Option<Pos2>,
    job: Option<Job>,
    /// Click ripples: where and when.
    ripples: Vec<(Pos2, f64)>,
    /// The view before a followed sketch (restored when it finishes).
    saved_view: Option<solidcraft_engine::render::Camera>,
}

impl AgentCursor {
    /// A request is being acted out (later requests wait behind it).
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }
}

/// Keys whose [x, y, z] values are directions or amounts, not places to point at.
const NOT_PLACES: [&str; 14] =
    ["translate", "dir1", "dir2", "axis", "direction", "normal", "pull", "color", "x_dir", "y_dir", "scale", "values", "limits", "size"];

/// Where the pointer goes for a command: its toolbar button (or its panel), then each pick.
pub fn stops(app: &SolveApp, command: &str, params: &Value) -> Vec<Pos2> {
    let mut out = Vec::new();
    if let Some(spec) = solidcraft_engine::find_command(command) {
        let button = crate::scenario::handle_at(&format!("toolbar:{}", spec.id))
            .or_else(|| crate::workspace::destination(spec).and_then(|(t, p)| crate::scenario::handle_at(&format!("panel:{t}:{p}"))));
        out.extend(button);
    }
    let Some(rect) = app.viewport.rect else { return out };
    let proj = crate::viewport::projection(app, rect);
    for w in picks(app, command, params) {
        if let Some(p) = proj.to_screen(w).filter(|p| rect.contains(*p)) {
            out.push(p);
        }
    }
    out
}

/// The places a command's parameters pick, in the world: [x, y, z] points (faces, edges,
/// vertices, positions), active-sketch [x, y] points, and a new sketch's plane.
pub fn picks(app: &SolveApp, command: &str, params: &Value) -> Vec<Vec3> {
    let id = solidcraft_engine::find_command(command).map_or(command, |c| c.id);
    let mut out = Vec::new();
    if id == "sketch.create"
        && let Some(n) = params.get("plane").and_then(Value::as_str)
        && let Some((_, _, q)) = crate::viewport::origin_planes(app).into_iter().find(|(name, _, _)| *name == n)
    {
        out.push((q[0] + q[1] + q[2] + q[3]) / 4.0);
    }
    let st = app.session.world_state();
    let sketch = app.session.active_sketch.and_then(|s| st.sketch(s)).map(|ss| ss.plane);
    walk(params, None, &mut |key, v| {
        let num = |i: usize| v.get(i).and_then(Value::as_f64).filter(|x| x.is_finite());
        let place = !key.is_some_and(|k| NOT_PLACES.contains(&k));
        match (v.as_array().map(Vec::len), num(0), num(1), num(2)) {
            (Some(3), Some(x), Some(y), Some(z)) if place => out.push(Vec3::new(x, y, z)),
            (Some(2), Some(x), Some(y), None) if place => {
                if let Some(pl) = &sketch {
                    out.push(pl.to_world(Vec2::new(x, y)));
                }
            }
            _ => {}
        }
    });
    // A profile feature without picks takes the profiles of the active (else the last) sketch:
    // point at them.
    if out.is_empty() && matches!(id, "solid.extrude" | "solid.revolve") {
        let ss =
            app.session.active_sketch.and_then(|s| st.sketch(s)).or_else(|| st.sketches.iter().rev().find(|s| !s.profiles.is_empty()).map(|s| &**s));
        if let Some(ss) = ss {
            out.extend(ss.profiles.iter().take(4).map(|p| ss.plane.to_world(p.region.interior_point())));
        }
    }
    out
}

/// Visit every value with the key it sits under (array items inherit their array's key).
fn walk<'a>(v: &'a Value, key: Option<&'a str>, f: &mut dyn FnMut(Option<&'a str>, &'a Value)) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                walk(x, Some(k.as_str()), f);
            }
        }
        Value::Array(a) => {
            // A point is a leaf; a list of points or other values is walked.
            if a.len() >= 2 && a.len() <= 3 && a.iter().all(Value::is_number) {
                f(key, v);
            } else {
                for x in a {
                    walk(x, key, f);
                }
            }
        }
        _ => {}
    }
}

/// Take over a control request when the pointer should act it out first: returns the request
/// back when it should simply run.
pub fn intercept(app: &mut SolveApp, method: &str, params: &Value, reply: &Sender<ControlResponse>) -> bool {
    let s = app.ui.agent_cursor;
    if !s.show || !matches!(method, "engine.execute" | "command") {
        return false;
    }
    if params.get("animate").and_then(Value::as_bool) == Some(false) {
        return false;
    }
    let Some(cmd) = params.get("command").or(params.get("id")).and_then(Value::as_str) else { return false };
    let inner = params.get("params").cloned().unwrap_or(json!({}));
    let stops: VecDeque<Pos2> = stops(app, cmd, &inner).into();
    if s.speed == Speed::Instant {
        // No animation: the command runs now; the pointer jumps to where it acted.
        if let Some(last) = stops.back() {
            app.agent.pos = Some(*last);
            app.agent.ripples.push((*last, app.now));
        }
        return false;
    }
    if stops.is_empty() {
        return false;
    }
    let from = app.cursor_start();
    app.agent.job =
        Some(Job { method: method.to_string(), params: params.clone(), reply: reply.clone(), stops, from, started: app.now, clicked: false });
    true
}

impl SolveApp {
    /// Where the agent pointer starts: where it was, else the middle of the viewport.
    fn cursor_start(&self) -> Pos2 {
        self.agent.pos.or_else(|| self.viewport.rect.map(|r| r.center())).unwrap_or(pos2(600.0, 400.0))
    }
}

fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Move the pointer along; when the last stop is clicked, run the held request and reply.
pub fn step(app: &mut SolveApp, ctx: &egui::Context) {
    let now = app.now;
    app.agent.ripples.retain(|(_, t)| now - t < 0.6);
    if !app.agent.ripples.is_empty() {
        ctx.request_repaint();
    }
    let (glide, click) = app.ui.agent_cursor.speed.times();
    let Some(job) = app.agent.job.as_mut() else { return };
    ctx.request_repaint();
    let Some(&to) = job.stops.front() else {
        finish(app, ctx);
        return;
    };
    let t = if glide > 0.0 { (now - job.started) / glide } else { 1.0 };
    if !job.clicked {
        let e = ease(t) as f32;
        app.agent.pos = Some(job.from + (to - job.from) * e);
        if t >= 1.0 {
            job.clicked = true;
            job.started = now;
            app.agent.ripples.push((to, now));
        }
        return;
    }
    if now - job.started >= click {
        job.stops.pop_front();
        job.from = to;
        job.started = now;
        job.clicked = false;
        if job.stops.is_empty() {
            finish(app, ctx);
        }
    }
}

/// Run the held request now (as a call without animation) and send its reply.
fn finish(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(job) = app.agent.job.take() else { return };
    let mut params = job.params;
    params["animate"] = json!(false);
    let cmd = params.get("command").or(params.get("id")).and_then(Value::as_str).unwrap_or_default().to_string();
    let id = solidcraft_engine::find_command(&cmd).map_or(cmd.clone(), |c| c.id.to_string());
    let follow = app.ui.agent_cursor.follow_camera;
    if follow && id == "sketch.create" {
        app.agent.saved_view = Some(app.cam);
    }
    let (req, _rx) = crate::control::ControlRequest::new(job.method, params);
    let out = match crate::control::handle(app, ctx, &req) {
        crate::control::Outcome::Done(v) => v,
        crate::control::Outcome::Screenshot { .. } => json!({"ok": false, "error": "not a command"}),
    };
    let ran = out.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if follow && ran {
        if id == "sketch.create" {
            crate::dialogs::look_at_sketch(app);
        } else if id == "sketch.finish"
            && let Some(cam) = app.agent.saved_view.take()
        {
            app.animate_to(cam);
        }
    }
    let _ = job.reply.send(out);
}

/// The pointer's colour: a violet no part of the UI uses.
use crate::icons::pointer::agent;

/// Draw the pointer, its "Agent" tag and the click ripples over everything.
pub fn paint(app: &SolveApp, ctx: &egui::Context) {
    if !app.ui.agent_cursor.show {
        return;
    }
    let Some(p) = app.agent.pos else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::new("sc_agent_cursor")));
    for (at, t0) in &app.agent.ripples {
        let k = ((app.now - t0) / 0.6).clamp(0.0, 1.0) as f32;
        painter.circle_stroke(*at, 6.0 + 22.0 * k, Stroke::new(3.0 * (1.0 - k) + 0.5, agent().gamma_multiply(1.0 - k)));
    }
    crate::icons::pointer::paint(&painter, p);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use solidcraft_engine::Session;

    fn app() -> SolveApp {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let mut a = SolveApp::new(s, Services::default());
        a.viewport.rect = Some(egui::Rect::from_min_size(pos2(250.0, 120.0), vec2(1100.0, 760.0)));
        a.fit_view();
        a
    }

    #[test]
    fn picks_are_the_places_in_the_parameters() {
        let a = app();
        // Points and lists of points, not directions or colours.
        let p = json!({"edges": [[20, 0, 20], [40, 15, 20]], "position": [5, 5, 20], "translate": [1, 2, 3], "color": [255, 0, 0], "radius": 2});
        let w = picks(&a, "solid.fillet", &p);
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w.contains(&Vec3::new(20.0, 0.0, 20.0)) && w.contains(&Vec3::new(5.0, 5.0, 20.0)));
        // A new sketch's origin plane is a place too (the middle of its square).
        assert_eq!(picks(&a, "sketch.create", &json!({"plane": "XY"})).len(), 1);
    }

    #[test]
    fn picks_land_where_the_viewport_draws_them() {
        let a = app();
        let rect = a.viewport.rect.unwrap();
        let proj = crate::viewport::projection(&a, rect);
        let at = Vec3::new(20.0, 15.0, 20.0);
        let s = stops(&a, "solid.hole", &json!({"position": [20, 15, 20], "diameter": 5}));
        let want = proj.to_screen(at).unwrap();
        assert!(s.iter().any(|p| p.distance(want) < 0.01), "{s:?} vs {want:?}");
        assert!(rect.contains(want));
        // Off-screen places are skipped rather than pointed at outside the view.
        assert!(stops(&a, "solid.hole", &json!({"position": [1e6, 0, 0]})).iter().all(|p| rect.contains(*p)));
    }

    #[test]
    fn speeds_read_old_and_new_settings() {
        // Saved settings from before #79: slow and normal are 1×, instant stays.
        for (v, want) in [
            (json!("slow"), Speed::Times(1.0)),
            (json!("normal"), Speed::Times(1.0)),
            (json!("instant"), Speed::Instant),
            (json!(3), Speed::Times(3.0)),
        ] {
            assert_eq!(Speed::from_value(&v), Some(want), "{v}");
            let s: Settings = serde_json::from_value(json!({"show": true, "speed": v})).unwrap();
            assert_eq!(s.speed, want);
        }
        assert_eq!(Speed::from_value(&json!(9)), None);
        assert_eq!(serde_json::to_value(Speed::Instant).unwrap(), json!("instant"));
        assert_eq!(serde_json::to_value(Speed::Times(2.5)).unwrap(), json!(2.5));
        assert_eq!((Speed::Times(1.0).label(), Speed::Times(2.5).label(), Speed::Instant.label()), ("1×".into(), "2.5×".into(), "Instant".into()));
        // Faster glides take less time.
        assert!(Speed::Times(5.0).times().0 < Speed::Times(1.0).times().0);
    }
}
