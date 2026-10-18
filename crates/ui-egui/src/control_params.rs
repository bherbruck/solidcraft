//! The control methods' params as types. What a request leaves out is filled in or refused
//! here, once, so the methods read plain values instead of testing every field.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use solidcraft_engine::geom::{Vec2, Vec3};

/// Most points one `ui.worldToScreen` or `ui.sketchToScreen` answers.
const MAX_POINTS: usize = 1000;

/// A method's params as `T`; no params (`null`) reads as `{}`.
pub(crate) fn parse<T: DeserializeOwned>(p: &Value) -> Result<T, String> {
    let p = if p.is_null() { Value::Object(Default::default()) } else { p.clone() };
    serde_json::from_value(p).map_err(|e| format!("bad params: {e}"))
}

/// A field as `T`, `None` when it is absent.
fn field<T: DeserializeOwned>(p: &Value, k: &str) -> Result<Option<T>, String> {
    p.get(k).map(|v| serde_json::from_value(v.clone()).map_err(|e| format!("bad `{k}`: {e}"))).transpose()
}

fn yes() -> bool {
    true
}

/// A screen point (`x`, `y`) within ±1000000 points.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(try_from = "RawXy")]
pub(crate) struct Xy(pub egui::Pos2);

#[derive(Deserialize)]
struct RawXy {
    x: f64,
    y: f64,
}

impl TryFrom<RawXy> for Xy {
    type Error = String;
    fn try_from(r: RawXy) -> Result<Self, String> {
        if r.x.abs() > 1e6 || r.y.abs() > 1e6 {
            return Err("x/y must be within ±1000000 screen points".into());
        }
        Ok(Xy(egui::pos2(r.x as f32, r.y as f32)))
    }
}

/// A pointer button by name, the left one when left out.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn egui(self) -> egui::PointerButton {
        match self {
            Button::Left => egui::PointerButton::Primary,
            Button::Right => egui::PointerButton::Secondary,
            Button::Middle => egui::PointerButton::Middle,
        }
    }
}

/// The modifier keys a request holds down, none when left out.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Mods {
    shift: bool,
    ctrl: bool,
    alt: bool,
    cmd: bool,
}

impl Mods {
    /// Held while the pointer moves (touchpad navigation reads them).
    pub fn held(self) -> egui::Modifiers {
        egui::Modifiers { shift: self.shift, alt: self.alt, ctrl: self.ctrl, command: self.ctrl || self.cmd, mac_cmd: self.cmd }
    }
    /// Held through a click or a drag's press.
    pub fn press(self) -> egui::Modifiers {
        egui::Modifiers { shift: self.shift, ctrl: self.ctrl, command: self.ctrl, alt: self.alt, ..Default::default() }
    }
    /// Held through a key press: `cmd` is the platform's command key.
    pub fn key(self) -> egui::Modifiers {
        egui::Modifiers { shift: self.shift, command: self.cmd, ctrl: self.cmd || self.ctrl, ..Default::default() }
    }
}

/// A key by egui's name or a common alias (`enter`, `esc`, `delete`, `space`, `tab`).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct Key(pub egui::Key);

impl TryFrom<String> for Key {
    type Error = String;
    fn try_from(name: String) -> Result<Self, String> {
        let alias = match name.to_ascii_lowercase().as_str() {
            "enter" | "return" => Some(egui::Key::Enter),
            "esc" | "escape" => Some(egui::Key::Escape),
            "delete" => Some(egui::Key::Delete),
            "space" => Some(egui::Key::Space),
            "tab" => Some(egui::Key::Tab),
            _ => None,
        };
        egui::Key::from_name(&name).or(alias).map(Key).ok_or_else(|| "unknown key".into())
    }
}

/// A feature or occurrence by id or name.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum IdOrName {
    Id(u64),
    Name(String),
}

impl IdOrName {
    pub fn key(&self) -> String {
        match self {
            IdOrName::Id(n) => n.to_string(),
            IdOrName::Name(x) => x.clone(),
        }
    }
}

/// `engine.execute {command, params?}` (`id` for `command` too).
#[derive(Deserialize)]
pub(crate) struct Execute {
    #[serde(alias = "id")]
    pub command: String,
    #[serde(default)]
    pub params: Value,
}

impl Execute {
    /// The command's params, `{}` when left out.
    pub fn params(self) -> Value {
        if self.params.is_null() { serde_json::json!({}) } else { self.params }
    }
}

/// `engine.batch {commands, rollback?, timings?, profile?}`.
#[derive(Deserialize)]
pub(crate) struct Batch {
    pub commands: Vec<Value>,
    #[serde(default = "yes")]
    pub rollback: bool,
    #[serde(default)]
    pub timings: bool,
    #[serde(default)]
    pub profile: bool,
}

/// `ui.move {x, y, shift?, ctrl?, alt?, cmd?}`.
#[derive(Deserialize)]
pub(crate) struct Move {
    #[serde(flatten)]
    pub at: Xy,
    #[serde(flatten)]
    pub mods: Mods,
}

/// `ui.click {x, y, button?, shift?, ctrl?, alt?, double?}`.
#[derive(Deserialize)]
pub(crate) struct Click {
    #[serde(flatten)]
    pub at: Xy,
    #[serde(default)]
    pub button: Button,
    #[serde(flatten)]
    pub mods: Mods,
    #[serde(default)]
    pub double: bool,
}

/// `ui.drag {x0, y0, x1, y1, button?, shift?, ctrl?, alt?, steps?, hold?, hold_end?}`.
#[derive(Deserialize)]
pub(crate) struct Drag {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    #[serde(default)]
    pub button: Button,
    #[serde(flatten)]
    pub mods: Mods,
    #[serde(default = "Drag::default_steps")]
    pub steps: f64,
    #[serde(default)]
    pub hold: f64,
    #[serde(default)]
    pub hold_end: f64,
}

impl Drag {
    fn default_steps() -> f64 {
        10.0
    }
}

/// `ui.scroll {x, y, delta?}`.
#[derive(Deserialize)]
pub(crate) struct Scroll {
    #[serde(flatten)]
    pub at: Xy,
    #[serde(default = "Scroll::default_delta")]
    pub delta: f64,
}

impl Scroll {
    fn default_delta() -> f64 {
        -100.0
    }
}

/// `ui.key {key, shift?, ctrl?, cmd?}`.
#[derive(Deserialize)]
pub(crate) struct KeyPress {
    pub key: Key,
    #[serde(flatten)]
    pub mods: Mods,
}

/// `ui.text {text}`.
#[derive(Deserialize)]
pub(crate) struct Text {
    #[serde(default)]
    pub text: String,
}

/// `ui.file {path, insert?}`.
#[derive(Deserialize)]
pub(crate) struct File {
    pub path: String,
    #[serde(default)]
    pub insert: bool,
}

/// `ui.handles {prefix?}`.
#[derive(Deserialize)]
pub(crate) struct Handles {
    #[serde(default)]
    pub prefix: String,
}

/// `ui.editFeature {feature}`.
#[derive(Deserialize)]
pub(crate) struct EditFeature {
    pub feature: IdOrName,
}

/// `ui.dialogInput {index?}`.
#[derive(Deserialize)]
pub(crate) struct DialogInput {
    #[serde(default)]
    pub index: usize,
}

/// `ui.confirm {accept?}`: without `accept`, what the confirmation lists.
#[derive(Deserialize)]
pub(crate) struct Confirm {
    pub accept: Option<bool>,
}

/// `ui.shortcut {command, key?, replace?}`: without `key`, the command's key.
#[derive(Deserialize)]
pub(crate) struct Shortcut {
    pub command: String,
    pub key: Option<String>,
    #[serde(default)]
    pub replace: bool,
}

/// The two workspaces.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Workspace {
    Design,
    Render,
}

/// `ui.workspace {workspace?}`: without one, the workspace shown.
#[derive(Deserialize)]
pub(crate) struct WorkspaceParams {
    pub workspace: Option<Workspace>,
}

/// `ui.home {open?, sample?}`.
#[derive(Deserialize)]
pub(crate) struct Home {
    pub open: Option<bool>,
    pub sample: Option<usize>,
}

/// `ui.view {view?, animate?}`.
#[derive(Deserialize)]
pub(crate) struct View {
    #[serde(default = "View::home")]
    pub view: String,
    #[serde(default)]
    pub animate: bool,
}

impl View {
    fn home() -> String {
        "home".into()
    }
}

/// `ui.start {command, params?}`.
#[derive(Deserialize)]
pub(crate) struct Start {
    pub command: String,
    #[serde(default)]
    pub params: Value,
}

/// `ui.menu`: close the menu, open one at a point, or say what is open.
pub(crate) enum Menu {
    Close,
    Open { at: Xy, target: crate::context_menu::Target },
    Describe,
}

impl Menu {
    pub fn parse(p: &Value) -> Result<Menu, String> {
        if field::<bool>(p, "close")?.unwrap_or(false) {
            return Ok(Menu::Close);
        }
        if p.get("x").is_none() && p.get("y").is_none() {
            return Ok(Menu::Describe);
        }
        let at = parse::<Xy>(p)?;
        Ok(Menu::Open { at, target: field(p, "target")?.unwrap_or_default() })
    }
}

/// A Help menu item.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HelpItem {
    About,
    Shortcuts,
    Report,
    Crashes,
}

impl HelpItem {
    pub fn name(self) -> &'static str {
        match self {
            HelpItem::About => "about",
            HelpItem::Shortcuts => "shortcuts",
            HelpItem::Report => "report",
            HelpItem::Crashes => "crashes",
        }
    }
}

/// `ui.help {item?}`.
#[derive(Deserialize)]
pub(crate) struct Help {
    pub item: Option<HelpItem>,
}

/// `ui.render {path?, width?, height?, grid?, axes?}`: `grid` and `axes` as the window shows
/// them when left out.
#[derive(Deserialize)]
pub(crate) struct Render {
    pub path: Option<String>,
    #[serde(default = "Render::default_width")]
    pub width: f64,
    #[serde(default = "Render::default_height")]
    pub height: f64,
    pub grid: Option<bool>,
    pub axes: Option<bool>,
}

impl Render {
    fn default_width() -> f64 {
        1280.0
    }
    fn default_height() -> f64 {
        800.0
    }
}

/// `ui.screenshot {path?}`.
#[derive(Deserialize)]
pub(crate) struct Screenshot {
    pub path: Option<String>,
}

/// `ui.resize {width, height}`.
#[derive(Deserialize)]
pub(crate) struct Resize {
    pub width: f64,
    pub height: f64,
}

/// `ui.menuPick {item}`.
#[derive(Deserialize)]
pub(crate) struct MenuPick {
    pub item: String,
}

/// `ui.rename {text?, commit?}`.
#[derive(Deserialize)]
pub(crate) struct Rename {
    pub text: Option<String>,
    #[serde(default = "yes")]
    pub commit: bool,
}

/// `ui.window {action}` and `ui.capture {action?}`.
#[derive(Deserialize)]
pub(crate) struct Action {
    pub action: Option<String>,
}

/// `app.quit {force?}`.
#[derive(Deserialize)]
pub(crate) struct Quit {
    #[serde(default)]
    pub force: bool,
}

/// `ui.worldToScreen {points}`, `ui.sketchToScreen {points}`: the first 1000 points.
#[derive(Deserialize)]
#[serde(bound = "T: DeserializeOwned")]
pub(crate) struct Points<T> {
    #[serde(default, deserialize_with = "first_points")]
    pub points: Vec<T>,
}

fn first_points<'de, D: serde::Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    let all = Vec::<Value>::deserialize(d)?;
    all.into_iter().take(MAX_POINTS).map(|v| serde_json::from_value(v).map_err(serde::de::Error::custom)).collect()
}

/// What `ui.at` finds the screen point of: one of these, tried in this order.
pub(crate) enum At {
    /// `{occurrence: [ids], control?}`: a browser occurrence row (or a control on it).
    Occurrence { key: String, control: String },
    /// `{handle}`: a scenario handle drawn last frame.
    Handle(String),
    /// `{browser: label}` or `{chevron: label}`: a browser row, or its fold arrow.
    Browser { label: String, chevron: bool },
    /// `{dimension}`: a sketch dimension's text.
    Dimension(String),
    /// `{world: [x, y, z]}`.
    World(Vec3),
    /// `{sketch: [x, y]}`: in the active sketch.
    Sketch(Vec2),
    /// `{plane: "XY"}`: the middle of an origin plane's square.
    Plane(String),
    /// `{axis}`: along an origin axis, on its solid part.
    Axis(String),
    /// `{gizmo, by?, turn?}`: a move-triad handle ("x", "xy", "rz", "ball"), moved `by`
    /// [x, y, z] mm or turned `turn` degrees (rings).
    Gizmo { name: String, by: Vec3, turn: f64 },
}

impl At {
    pub fn parse(p: &Value) -> Result<At, String> {
        if let Some(ids) = field::<Vec<u64>>(p, "occurrence").map_err(|_| "occurrence path must contain integer ids".to_string())? {
            let key = ids.iter().map(u64::to_string).collect::<Vec<_>>().join("/");
            return Ok(At::Occurrence { key, control: field(p, "control")?.unwrap_or_else(|| "row".into()) });
        }
        if let Some(h) = field(p, "handle")? {
            return Ok(At::Handle(h));
        }
        for (k, chevron) in [("browser", false), ("chevron", true)] {
            if let Some(label) = field(p, k)? {
                return Ok(At::Browser { label, chevron });
            }
        }
        if let Some(d) = field(p, "dimension")? {
            return Ok(At::Dimension(d));
        }
        if let Some([x, y, z]) = field::<[f64; 3]>(p, "world")? {
            return Ok(At::World(Vec3::new(x, y, z)));
        }
        if let Some([x, y]) = field::<[f64; 2]>(p, "sketch")? {
            return Ok(At::Sketch(Vec2::new(x, y)));
        }
        if let Some(n) = field(p, "plane")? {
            return Ok(At::Plane(n));
        }
        if let Some(n) = field(p, "axis")? {
            return Ok(At::Axis(n));
        }
        if let Some(name) = field(p, "gizmo")? {
            let by = field::<[f64; 3]>(p, "by")?.map_or(Vec3::default(), |[x, y, z]| Vec3::new(x, y, z));
            return Ok(At::Gizmo { name, by, turn: field(p, "turn")?.unwrap_or(0.0) });
        }
        Err(format!("ui.at needs one of occurrence, handle, browser, chevron, dimension, world, sketch, plane, axis, gizmo: {p}"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn left_out_fields_are_filled_in_once() {
        let d: Drag = parse(&json!({"x0": 1, "y0": 2, "x1": 3, "y1": 4})).unwrap();
        assert!(matches!(d.button, Button::Left));
        assert_eq!((d.steps, d.hold, d.hold_end), (10.0, 0.0, 0.0));
        assert!(d.mods.press().is_none());
        let v: View = parse(&Value::Null).unwrap();
        assert_eq!((v.view.as_str(), v.animate), ("home", false));
        let r: Rename = parse(&json!({})).unwrap();
        assert!(r.commit && r.text.is_none());
        let e: Execute = parse(&json!({"id": "solid.extrude", "params": null})).unwrap();
        assert_eq!(e.command, "solid.extrude");
        assert_eq!(e.params(), json!({}));
    }

    #[test]
    fn missing_or_wrong_fields_are_refused() {
        assert!(parse::<Click>(&json!({"x": 1})).is_err());
        assert!(parse::<Click>(&json!({"x": 1, "y": 2, "button": "fourth"})).is_err());
        assert!(parse::<Click>(&json!({"x": 1, "y": 2, "shift": "yes"})).is_err());
        let far = parse::<Move>(&json!({"x": 2e6, "y": 0})).err().unwrap_or_default();
        assert!(far.contains("±1000000"), "{far}");
        assert!(parse::<KeyPress>(&json!({"key": "no such key"})).is_err());
        assert!(parse::<KeyPress>(&json!({"key": "esc"})).is_ok_and(|k| k.key.0 == egui::Key::Escape));
    }

    #[test]
    fn at_takes_the_first_target_it_names() {
        let at = At::parse(&json!({"handle": "toolbar:solid.extrude", "world": [0, 0, 0]})).unwrap();
        assert!(matches!(at, At::Handle(h) if h == "toolbar:solid.extrude"));
        assert!(matches!(At::parse(&json!({"occurrence": [1, 2]})).unwrap(), At::Occurrence { key, control } if key == "1/2" && control == "row"));
        assert!(matches!(At::parse(&json!({"chevron": "Bodies"})).unwrap(), At::Browser { chevron: true, .. }));
        assert!(matches!(At::parse(&json!({"gizmo": "x"})).unwrap(), At::Gizmo { turn, .. } if turn == 0.0));
        assert!(At::parse(&json!({"occurrence": ["a"]})).is_err());
        assert!(At::parse(&json!({"world": [1, 2]})).is_err());
        assert!(At::parse(&json!({})).is_err());
    }

    #[test]
    fn menu_opens_only_at_a_point() {
        assert!(matches!(Menu::parse(&json!({"close": true, "x": 1, "y": 2})).unwrap(), Menu::Close));
        assert!(matches!(Menu::parse(&json!({})).unwrap(), Menu::Describe));
        assert!(matches!(Menu::parse(&json!({"x": 1, "y": 2})).unwrap(), Menu::Open { .. }));
        assert!(Menu::parse(&json!({"x": 1})).is_err());
    }

    #[test]
    fn points_stop_at_the_cap() {
        let many: Vec<[f64; 2]> = vec![[0.0, 0.0]; MAX_POINTS + 5];
        let q: Points<[f64; 2]> = parse(&json!({ "points": many })).unwrap();
        assert_eq!(q.points.len(), MAX_POINTS);
        assert!(parse::<Points<[f64; 3]>>(&json!({"points": [[1, 2]]})).is_err());
    }
}
