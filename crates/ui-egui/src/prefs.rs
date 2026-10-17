//! Preferences, one dialog in the spirit of Fusion's, opened from the gear at the top right of the
//! window, File › Preferences or Ctrl+, (Cmd+, on macOS): General (theme, UI scale, units of new
//! designs, autosave, display precision, timeline layout, the agent cursor), Navigation (mouse presets, zoom
//! direction, zoom to the cursor, orbit centre, Look At animation), Keyboard Shortcuts (search,
//! rebind, conflicts; `keymap`), Design (default modelling orientation, Y up or Z up), Graphics
//! (visual style, grid, shadow, perspective, anti-aliasing, colour vision), Sketch and Selection.
//! Changes apply at once; Cancel puts back what was there when the dialog opened, OK keeps them,
//! Apply keeps them and stays open, and Restore Defaults resets the page shown. Everything is
//! kept between runs, except anti-aliasing, which needs a restart.

use egui::{Modifiers, Pos2, RichText};
use serde::{Deserialize, Serialize};
use serde_json::json;
use solidcraft_engine::geom::Vec3;

/// The key the apps keep the preferences under in eframe's storage.
pub const STORAGE_KEY: &str = "solidcraft.prefs";
/// The key earlier releases used. Compatibility shim (docs/compatibility.md): read when the
/// current key is missing, never written.
pub const LEGACY_STORAGE_KEY: &str = "solvecraft.prefs";

/// The stored preferences, from the current key or else the legacy one.
pub fn stored(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    get(STORAGE_KEY).or_else(|| get(LEGACY_STORAGE_KEY))
}

use crate::SolveApp;
use crate::theme::Tokens;
use crate::viewport::NavMode;

pub const UNITS: [&str; 5] = ["mm", "cm", "m", "in", "ft"];
pub const OPENSCAD_NAV: &str = "openscad";
pub const TOUCHPAD_NAV: &str = "touchpad";
/// A mouse button that can drag the camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavButton {
    Left,
    Middle,
    Right,
}

/// Modifier keys a binding needs held (others may be held too).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct NavKeys {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl NavKeys {
    const NONE: NavKeys = NavKeys { shift: false, ctrl: false, alt: false };
    const SHIFT: NavKeys = NavKeys { shift: true, ctrl: false, alt: false };
    const CTRL: NavKeys = NavKeys { shift: false, ctrl: true, alt: false };
    const SHIFT_ALT: NavKeys = NavKeys { shift: true, ctrl: false, alt: true };

    /// All of these keys are held (Ctrl is Cmd on macOS).
    fn held(self, m: Modifiers) -> bool {
        (!self.shift || m.shift) && (!self.ctrl || m.ctrl || m.command) && (!self.alt || m.alt)
    }
}

/// One camera binding: dragging `button` with `keys` held does `mode`.
#[derive(Clone, Copy, Debug)]
pub struct NavBinding {
    pub button: NavButton,
    pub keys: NavKeys,
    pub mode: NavMode,
}

const fn bind(button: NavButton, keys: NavKeys, mode: NavMode) -> NavBinding {
    NavBinding { button, keys, mode }
}

/// A mouse mapping: its bindings, most specific first (the first one whose button is dragged
/// with all its keys held wins).
pub struct NavPreset {
    pub id: &'static str,
    pub label: &'static str,
    /// What the mouse does, for the Preferences page.
    pub help: &'static str,
    /// Choosing the preset turns the wheel the other way.
    pub reverse_zoom: bool,
    pub bindings: &'static [NavBinding],
}

impl NavPreset {
    /// What dragging `button` with `mods` held does, if anything.
    pub fn mode(&self, button: NavButton, mods: Modifiers) -> Option<NavMode> {
        self.bindings.iter().find(|b| b.button == button && b.keys.held(mods)).map(|b| b.mode)
    }
}

use NavButton::{Left, Middle, Right};
use NavMode::{Orbit, Pan, Zoom};

/// The mouse mappings. The first is the default.
pub const NAV_PRESETS: [NavPreset; 8] = [
    NavPreset {
        id: "fusion",
        label: "Fusion",
        help: "Middle drag pans, Shift+middle orbits, right drag orbits",
        reverse_zoom: false,
        bindings: &[bind(Middle, NavKeys::SHIFT, Orbit), bind(Middle, NavKeys::NONE, Pan), bind(Right, NavKeys::NONE, Orbit)],
    },
    NavPreset {
        id: "solidworks",
        label: "SolidWorks",
        help: "Middle drag orbits, Ctrl+middle pans, Shift+middle zooms, right drag orbits",
        reverse_zoom: true,
        bindings: &[
            bind(Middle, NavKeys::CTRL, Pan),
            bind(Middle, NavKeys::SHIFT, Zoom),
            bind(Middle, NavKeys::NONE, Orbit),
            bind(Right, NavKeys::NONE, Orbit),
        ],
    },
    NavPreset {
        id: "inventor",
        label: "Inventor",
        help: "Middle drag pans, Shift+middle orbits, Ctrl+middle zooms, right drag orbits",
        reverse_zoom: false,
        bindings: &[
            bind(Middle, NavKeys::SHIFT, Orbit),
            bind(Middle, NavKeys::CTRL, Zoom),
            bind(Middle, NavKeys::NONE, Pan),
            bind(Right, NavKeys::NONE, Orbit),
        ],
    },
    NavPreset {
        id: "alias",
        label: "Alias",
        help: "Shift+Alt+left drag orbits, Shift+Alt+middle pans, Shift+Alt+right zooms; middle drag also pans and right drag orbits",
        reverse_zoom: false,
        bindings: &[
            bind(Left, NavKeys::SHIFT_ALT, Orbit),
            bind(Middle, NavKeys::SHIFT_ALT, Pan),
            bind(Right, NavKeys::SHIFT_ALT, Zoom),
            bind(Middle, NavKeys::CTRL, Zoom),
            bind(Middle, NavKeys::NONE, Pan),
            bind(Right, NavKeys::NONE, Orbit),
        ],
    },
    NavPreset {
        id: "blender",
        label: "Blender",
        help: "Middle drag orbits, Shift+middle pans, Ctrl+middle zooms; right drag leaves the camera alone",
        reverse_zoom: false,
        bindings: &[bind(Middle, NavKeys::SHIFT, Pan), bind(Middle, NavKeys::CTRL, Zoom), bind(Middle, NavKeys::NONE, Orbit)],
    },
    NavPreset {
        id: "tinkercad",
        label: "Tinkercad",
        help: "Right drag orbits, middle or Shift+right drag pans",
        reverse_zoom: false,
        bindings: &[bind(Right, NavKeys::SHIFT, Pan), bind(Right, NavKeys::NONE, Orbit), bind(Middle, NavKeys::NONE, Pan)],
    },
    NavPreset {
        id: OPENSCAD_NAV,
        label: "OpenSCAD",
        help: "Left drag on empty space orbits, Shift+left drag box-selects, sketch geometry drags edit; right or middle drag pans, Shift+right or Shift+middle drag zooms",
        reverse_zoom: false,
        bindings: &[
            bind(Left, NavKeys::NONE, Orbit),
            bind(Right, NavKeys::SHIFT, Zoom),
            bind(Middle, NavKeys::SHIFT, Zoom),
            bind(Right, NavKeys::NONE, Pan),
            bind(Middle, NavKeys::NONE, Pan),
        ],
    },
    NavPreset {
        id: TOUCHPAD_NAV,
        label: "Touchpad",
        help: "Hold Shift and move the pointer to pan; hold Alt/Option and move it to orbit. No mouse button is needed. Vertical scroll zooms.",
        reverse_zoom: false,
        bindings: &[bind(Middle, NavKeys::NONE, Pan), bind(Right, NavKeys::NONE, Orbit)],
    },
];

impl NavPreset {
    /// Button-free motion has exact modifiers, unlike the additive drag bindings.
    pub fn motion_mode(&self, mods: Modifiers) -> Option<NavMode> {
        if self.id != TOUCHPAD_NAV || mods.ctrl || mods.command || mods.mac_cmd {
            return None;
        }
        match (mods.shift, mods.alt) {
            (true, false) => Some(Pan),
            (false, true) => Some(Orbit),
            _ => None,
        }
    }

    fn translated_label(&self) -> &str {
        if self.id == TOUCHPAD_NAV { crate::i18n::tr("nav.touchpad.label") } else { crate::i18n::tr_label(self.label) }
    }
}

impl Prefs {
    /// Touchpad scroll zooms without overwriting the user's preference for other presets.
    pub fn trackpad_pans(&self) -> bool {
        self.nav != TOUCHPAD_NAV && self.trackpad_pan
    }
}

/// The preset with this id (the default for an unknown one).
pub fn nav_preset(id: &str) -> &'static NavPreset {
    NAV_PRESETS.iter().find(|p| p.id == id).unwrap_or(&NAV_PRESETS[0])
}
/// The dialog's pages, in order.
pub const PAGES: [&str; 8] = ["General", "Navigation", "Keyboard Shortcuts", "Design", "Graphics", "Sketch", "Selection", "AI"];
pub const PAGE_AI: usize = 7;
pub const PAGE_SHORTCUTS: usize = 2;
pub const PAGE_DESIGN: usize = 3;
pub const PAGE_GRAPHICS: usize = 4;
pub const THEMES: [(&str, &str); 3] = [("dark", "Dark"), ("light", "Light"), ("system", "System")];
pub const MSAA: [u32; 4] = [1, 2, 4, 8];
/// Supersampling factors (#1288): off, 1.5× and 2× the viewport's pixels each way.
pub const SUPERSAMPLING: [f32; 3] = [1.0, 1.5, 2.0];

/// Original view navigators, independent of the mouse navigation preset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigatorStyle {
    /// Also what a navigator this version doesn't know reads as (`lenient`).
    #[default]
    Axes,
    Cube,
}

impl NavigatorStyle {
    pub const ALL: [Self; 2] = [Self::Axes, Self::Cube];

    pub fn label(self) -> &'static str {
        match self {
            Self::Axes => "Signed axes",
            Self::Cube => "Cube",
        }
    }
}

/// What an orbit turns around (Preferences: "Orbit around").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrbitAround {
    /// The world origin: the view turns about it and keeps it still on screen.
    Origin,
    /// The view's centre: what Pan moves and Fit frames (the default, as it always was).
    #[default]
    ViewCentre,
    /// The model point under the cursor when the drag began.
    Cursor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Units of new designs.
    pub default_units: String,
    /// Mouse mapping: `fusion`, `solidworks` or `inventor`.
    pub nav: String,
    /// View orientation widget: original signed axes (default) or compact cube.
    pub navigator_style: NavigatorStyle,
    /// Show the radial ring and enable hold/flick gestures for viewport context menus.
    pub radial_context_menu: bool,
    /// The wheel zooms out when turned forward.
    pub zoom_reverse: bool,
    /// What an orbit turns around.
    pub orbit_around: OrbitAround,
    /// The cursor preference as releases before the origin option saved it (`true` = the point
    /// under the cursor). [`migrate`] folds it into `orbit_around` when saved preferences are
    /// read, so an existing choice survives the upgrade; this version only writes `false`.
    pub orbit_cursor: bool,
    /// Whether a connected 3D mouse moves the view.
    pub space_mouse_enabled: bool,
    /// Speed multiplier for 3D mouse navigation.
    pub space_mouse_speed: f32,
    /// Per physical axis of the cap (push X, Y, Z, then tilt X, Y, Z), whether it moves the view
    /// the other way.
    pub space_mouse_reverse: [bool; 6],
    /// Per physical axis of the cap, a multiplier on top of `space_mouse_speed`; 0 switches it off.
    pub space_mouse_axis_speed: [f32; 6],
    /// Anti-aliasing samples (applies on restart).
    pub msaa: u32,
    /// The grid turns to the upright origin plane facing a side view (#1470). Off by default: the
    /// grid stays on the ground plane (XY) whatever the view.
    pub grid_follows_view: bool,
    /// How finely the viewport tessellates curved faces and edges: `coarse`, `normal`, `fine` or
    /// `ultra` (#1287). Only the display plan the viewport draws; never the design or exports.
    pub display_quality: String,
    /// Supersampling (#1288): the view drawn at this many times its pixels each way and filtered
    /// down (1, 1.5 or 2; 1 is off).
    pub supersampling: f32,
    /// Supersample only while the view holds still (full speed while orbiting).
    pub supersampling_adaptive: bool,
    /// Show the frame rate in the view's corner (the view then redraws every frame).
    pub show_frame_rate: bool,
    /// Show world X/Y/Z component dimensions beside the total distance in Measure.
    pub measure_axis_distances: bool,
    /// Decimals shown for lengths and angles.
    pub length_decimals: u8,
    pub angle_decimals: u8,
    /// `dark`, `light` or `system` (follow the operating system).
    pub theme: String,
    /// Interface language (a registered locale ID).
    pub language: String,
    /// Interface scale (1 = 100 %).
    pub ui_scale: f32,
    /// Interface font: an installed font family (or a font file), or empty for the built-in one
    /// (#1290). Menus, dialogs and labels use it; glyphs it lacks fall back as before.
    pub ui_font: String,
    /// Put the feature history above the parts browser in the left sidebar instead of along the
    /// bottom of the viewport.
    pub timeline_left: bool,
    /// Height of the feature-history section when it shares the left sidebar with the browser.
    pub timeline_left_height: f32,
    /// Whether the browser below the left feature history is collapsed.
    pub timeline_left_browser_collapsed: bool,
    /// The wheel zooms toward the point under the cursor (else toward the view's centre).
    pub zoom_to_cursor: bool,
    /// Spread discrete wheel input across frames (off: apply each event immediately).
    pub smooth_wheel_zoom: bool,
    /// Two-finger trackpad scroll pans (off: it zooms like the wheel); pinch always zooms.
    pub trackpad_pan: bool,
    /// View changes (axis view, Look At, Home) animate; `look_at_speed` scales how fast.
    pub look_at_anim: bool,
    pub look_at_speed: f32,
    /// Default modelling orientation: `z` (Z up) or `y` (Y up) (#3).
    pub up_axis: String,
    /// After a STEP or IGES file opens, offer to stand a Y-up part up (`stand_up`).
    pub offer_stand_up: bool,
    /// What a middle-button double-click in the view does: `home`, `fit` or `off`.
    pub middle_double_click: String,
    /// Colour vision mode (#778): `off`, `protan`, `deutan` or `tritan`.
    pub color_vision: crate::theme::ColorVision,
    /// Check for updates at start and once a day (docs/updates.md).
    pub check_updates: bool,
    /// When the last update check ran, and the versions skipped or put off (`update::Memo`).
    pub update: crate::update::Memo,
    /// Save as mesh: the last format (`stl` or `3mf`), whether it was sent to a slicer, which one
    /// (an installed slicer's id or a program path), and the programs the user added (newest first).
    pub save_mesh_format: String,
    /// Hidden edges in the current-view DXF export (false: omit, true: dashed).
    pub view_dxf_dashed: bool,
    /// Model-space chord tolerance for current-view DXF curves, in millimetres.
    pub view_dxf_tolerance: f64,
    pub save_mesh_send: bool,
    pub save_mesh_slicer: String,
    pub custom_slicers: Vec<String>,
    /// AI features (docs/agent-chat.md): the master switch. Off: no AI button, shortcut, menu
    /// entry or panel, and nothing is looked for or started.
    pub ai_features: bool,
    /// The AI button at the toolbar row's right end (the shortcut and View menu entry stay).
    pub ai_toolbar_button: bool,
    /// Agents the user added (the installed ones are found by themselves). API keys are never
    /// here: they live in the system keychain.
    pub agents: Vec<crate::agent_chat::agents::AgentConfig>,
    /// The agent a new conversation starts with (an agent's id; empty: the first one available).
    pub agent_default: String,
    /// How much the agent may do without asking (`ask`, `edits`, `all`), and per agent.
    pub ai_permissions: crate::agent_chat::agents::Mode,
    pub agent_permissions: std::collections::BTreeMap<String, crate::agent_chat::agents::Mode>,
    /// The model last chosen in the chat, per agent: put back when the agent offers it again.
    pub agent_models: std::collections::BTreeMap<String, String>,
    /// "Always allow" answers: per agent, the tools it may use without asking.
    pub agent_always_allow: std::collections::BTreeMap<String, Vec<String>>,
    /// The side the panel docks on (`left` or `right`), its width, and whether it is folded to a strip.
    pub agent_chat_side: String,
    pub agent_chat_width: f32,
    pub agent_chat_minimized: bool,
    /// Changes the agent may make for one message.
    pub agent_chat_turn_limit: u32,
}

/// What a middle-button double-click in the view does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiddleDoubleClick {
    /// Go Home (the iso view, framing what is shown).
    Home,
    /// Frame what is shown, keeping the orientation.
    Fit,
    Off,
}

impl MiddleDoubleClick {
    pub const ALL: [(MiddleDoubleClick, &'static str, &'static str); 3] =
        [(MiddleDoubleClick::Home, "home", "Home"), (MiddleDoubleClick::Fit, "fit", "Fit"), (MiddleDoubleClick::Off, "off", "Off")];

    /// The preference's setting (Home for an unknown one).
    pub fn of(app: &SolveApp) -> MiddleDoubleClick {
        Self::parse(&app.preferences.middle_double_click).unwrap_or(MiddleDoubleClick::Home)
    }

    pub fn parse(id: &str) -> Option<MiddleDoubleClick> {
        Self::ALL.iter().find(|(_, i, _)| *i == id).map(|(m, _, _)| *m)
    }

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(m, _, _)| *m == self).map_or("Home", |(_, _, l)| l)
    }
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            default_units: "mm".into(),
            nav: "fusion".into(),
            navigator_style: NavigatorStyle::default(),
            radial_context_menu: false,
            zoom_reverse: false,
            orbit_around: OrbitAround::default(),
            orbit_cursor: false,
            space_mouse_enabled: true,
            space_mouse_speed: 1.0,
            space_mouse_reverse: [false; 6],
            space_mouse_axis_speed: [1.0; 6],
            msaa: 4,
            grid_follows_view: false,
            display_quality: "normal".into(),
            supersampling: 1.0,
            supersampling_adaptive: true,
            show_frame_rate: false,
            measure_axis_distances: true,
            length_decimals: 2,
            angle_decimals: 1,
            theme: "dark".into(),
            language: "en".into(),
            ui_scale: 1.0,
            ui_font: String::new(),
            timeline_left: false,
            timeline_left_height: 360.0,
            timeline_left_browser_collapsed: false,
            zoom_to_cursor: true,
            smooth_wheel_zoom: true,
            trackpad_pan: true,
            look_at_anim: true,
            look_at_speed: 1.0,
            up_axis: "z".into(),
            offer_stand_up: true,
            middle_double_click: "home".into(),
            color_vision: crate::theme::ColorVision::Off,
            check_updates: true,
            update: crate::update::Memo::default(),
            save_mesh_format: "stl".into(),
            view_dxf_dashed: false,
            view_dxf_tolerance: 0.01,
            save_mesh_send: false,
            save_mesh_slicer: String::new(),
            custom_slicers: Vec::new(),
            ai_features: true,
            ai_toolbar_button: true,
            agents: Vec::new(),
            agent_default: String::new(),
            ai_permissions: crate::agent_chat::agents::Mode::Edits,
            agent_permissions: Default::default(),
            agent_models: Default::default(),
            agent_always_allow: Default::default(),
            agent_chat_side: "right".into(),
            agent_chat_width: 380.0,
            agent_chat_minimized: false,
            agent_chat_turn_limit: crate::agent_chat::gate::DEFAULT_TURN_LIMIT,
        }
    }
}

/// The preferences window.
#[derive(Default)]
pub struct PrefsWindow {
    pub open: bool,
    /// The page shown (index into [`PAGES`]).
    pub section: usize,
    /// Everything as it was when the dialog opened (or at the last Apply), for Cancel.
    snapshot: Option<Snapshot>,
    /// The up axis preference the view follows now (applied when it changes, and at startup).
    applied_up: Option<String>,
    /// The search text of the interface font list.
    font_filter: String,
}

/// Open the dialog at a page.
pub fn open_at(app: &mut SolveApp, page: usize) {
    app.prefs_window.open = true;
    app.prefs_window.section = page.min(PAGES.len() - 1);
}

/// Every setting the dialog changes, for Cancel and Restore Defaults.
#[derive(Clone)]
struct Snapshot {
    prefs: Prefs,
    dark: bool,
    grid: bool,
    shadow: bool,
    perspective: bool,
    style: u8,
    pick_bodies: bool,
    agent: crate::agent_cursor::Settings,
    autosave: f64,
    auto_project: bool,
    keys: std::collections::BTreeMap<String, String>,
}

impl Snapshot {
    fn take(app: &SolveApp) -> Snapshot {
        Snapshot {
            prefs: app.preferences.clone(),
            dark: app.ui.dark,
            grid: app.ui.show_grid,
            shadow: app.ui.ground_shadow,
            perspective: app.ui.perspective,
            style: app.ui.visual_style,
            pick_bodies: app.ui.pick_bodies,
            agent: app.ui.agent_cursor,
            autosave: app.autosave_minutes,
            auto_project: app.session.auto_project,
            keys: app.keymap.custom.clone(),
        }
    }
    fn restore(self, app: &mut SolveApp) {
        let msaa = app.preferences.msaa;
        // Update bookkeeping isn't a setting: a check that finished while the dialog was open stays.
        let update = std::mem::take(&mut app.preferences.update);
        app.preferences = self.prefs;
        app.preferences.update = update;
        app.ui.dark = self.dark;
        app.ui.show_grid = self.grid;
        app.ui.ground_shadow = self.shadow;
        app.ui.perspective = self.perspective;
        app.ui.visual_style = self.style;
        app.ui.pick_bodies = self.pick_bodies;
        app.ui.agent_cursor = self.agent;
        app.autosave_minutes = self.autosave;
        set_autosave(app);
        app.session.auto_project = self.auto_project;
        app.keymap.custom = self.keys;
        if app.preferences.msaa != msaa {
            save_graphics(app);
        }
    }
}

/// The interface font (#1290): the built-in font first, then the installed families, with a
/// search box. The choice applies at once, like the other preferences.
fn ui_font_row(app: &mut SolveApp, ui: &mut egui::Ui) {
    let builtin = app.tr_label("Built-in").to_string();
    let current = if app.preferences.ui_font.trim().is_empty() { builtin.clone() } else { app.preferences.ui_font.clone() };
    let families = crate::sketch_text::families(ui.ctx());
    let mut chosen: Option<String> = None;
    let combo = egui::ComboBox::from_id_salt("pref_ui_font")
        .width(220.0)
        .height(340.0)
        .selected_text(current)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let search = egui::TextEdit::singleline(&mut app.prefs_window.font_filter).hint_text(crate::i18n::tr("sketch_text.font_search"));
            ui.add(search.desired_width(f32::INFINITY));
            let filter = app.prefs_window.font_filter.trim().to_lowercase();
            egui::ScrollArea::vertical().max_height(280.0).min_scrolled_height(280.0).auto_shrink([false, false]).show(ui, |ui| {
                if builtin.to_lowercase().contains(&filter) && ui.selectable_label(app.preferences.ui_font.trim().is_empty(), &builtin).clicked() {
                    chosen = Some(String::new());
                }
                match &families {
                    Some(list) => {
                        for family in list.iter().filter(|f| f.to_lowercase().contains(&filter)) {
                            let item = ui.selectable_label(app.preferences.ui_font == *family, family);
                            crate::scenario::publish_handle(&format!("prefs:font:{family}"), item.rect.center());
                            if item.clicked() {
                                chosen = Some(family.clone());
                            }
                        }
                    }
                    None => {
                        ui.label(crate::i18n::tr_label("Listing the installed fonts…"));
                    }
                }
            });
        });
    crate::scenario::publish_handle("prefs:ui_font", combo.response.rect.center());
    if let Some(name) = chosen {
        if name.is_empty() || crate::fonts::ui_font(&name).is_some() {
            app.preferences.ui_font = name;
        } else {
            app.set_status(format!("The font `{name}` could not be read"), true);
        }
        app.prefs_window.font_filter.clear();
        ui.close();
    }
}

/// Restore Defaults for one page.
fn defaults(app: &mut SolveApp, page: usize) {
    let d = Prefs::default();
    let p = &mut app.preferences;
    match page {
        0 => {
            p.theme = d.theme;
            p.language = d.language;
            p.ui_scale = d.ui_scale;
            p.ui_font = d.ui_font;
            p.timeline_left = d.timeline_left;
            p.timeline_left_height = d.timeline_left_height;
            p.timeline_left_browser_collapsed = d.timeline_left_browser_collapsed;
            p.default_units = d.default_units;
            p.length_decimals = d.length_decimals;
            p.angle_decimals = d.angle_decimals;
            p.check_updates = d.check_updates;
            app.ui.dark = true;
            app.ui.agent_cursor = Default::default();
            app.autosave_minutes = 5.0;
            set_autosave(app);
        }
        PAGE_AI => {
            // The agents the user added and their "always allow" answers stay.
            p.ai_features = d.ai_features;
            p.ai_toolbar_button = d.ai_toolbar_button;
            p.ai_permissions = d.ai_permissions;
            p.agent_permissions = d.agent_permissions;
            p.agent_chat_side = d.agent_chat_side;
            p.agent_chat_width = d.agent_chat_width;
            p.agent_chat_minimized = d.agent_chat_minimized;
            p.agent_chat_turn_limit = d.agent_chat_turn_limit;
        }
        1 => {
            p.nav = d.nav;
            p.navigator_style = d.navigator_style;
            p.radial_context_menu = d.radial_context_menu;
            p.zoom_reverse = d.zoom_reverse;
            p.zoom_to_cursor = d.zoom_to_cursor;
            p.smooth_wheel_zoom = d.smooth_wheel_zoom;
            p.trackpad_pan = d.trackpad_pan;
            p.orbit_around = d.orbit_around;
            p.orbit_cursor = d.orbit_cursor;
            p.space_mouse_enabled = d.space_mouse_enabled;
            p.space_mouse_speed = d.space_mouse_speed;
            p.space_mouse_reverse = d.space_mouse_reverse;
            p.space_mouse_axis_speed = d.space_mouse_axis_speed;
            p.look_at_anim = d.look_at_anim;
            p.look_at_speed = d.look_at_speed;
            p.middle_double_click = d.middle_double_click;
        }
        PAGE_SHORTCUTS => crate::keymap::reset(app, None),
        PAGE_DESIGN => {
            p.up_axis = d.up_axis;
            p.offer_stand_up = d.offer_stand_up;
        }
        PAGE_GRAPHICS => {
            let msaa = p.msaa != d.msaa;
            p.msaa = d.msaa;
            p.display_quality = d.display_quality;
            p.supersampling = d.supersampling;
            p.supersampling_adaptive = d.supersampling_adaptive;
            p.show_frame_rate = d.show_frame_rate;
            p.measure_axis_distances = d.measure_axis_distances;
            p.color_vision = d.color_vision;
            app.ui.visual_style = 0;
            app.ui.show_grid = true;
            app.ui.ground_shadow = true;
            app.ui.perspective = false;
            if msaa {
                save_graphics(app);
            }
        }
        5 => app.session.auto_project = true,
        _ => app.ui.pick_bodies = false,
    }
}

/// Settings that follow the preferences every frame: the theme when it follows the system, and
/// the interface scale.
pub fn apply(app: &mut SolveApp, ctx: &egui::Context) {
    // The viewport's display quality (an unknown saved value draws at Normal).
    solidcraft_engine::doc::display_quality::set_current(
        solidcraft_engine::doc::display_quality::DisplayQuality::parse(&app.preferences.display_quality).unwrap_or_default(),
    );
    // The view's up axis follows the preference (set in the dialog, by ui.prefs or loaded).
    if app.prefs_window.applied_up.as_deref() != Some(app.preferences.up_axis.as_str()) {
        up_axis_changed(app);
    }
    if app.preferences.theme == "system"
        && let Some(t) = ctx.system_theme()
    {
        app.ui.dark = t == egui::Theme::Dark;
    }
    // egui's end-of-frame shortcuts only change its context; the saved preference would
    // overwrite that change next frame. Consume the desktop shortcuts into the preference.
    if ctx.options(|o| o.zoom_with_keyboard) && !crate::keymap::capturing(app) {
        use egui::gui_zoom::kb_shortcuts::{ZOOM_IN, ZOOM_IN_SECONDARY, ZOOM_OUT, ZOOM_RESET};
        if ctx.input_mut(|i| i.consume_shortcut(&ZOOM_RESET)) {
            app.preferences.ui_scale = 1.0;
        } else {
            let zoom_in = ctx.input_mut(|i| i.consume_shortcut(&ZOOM_IN) || i.consume_shortcut(&ZOOM_IN_SECONDARY));
            let zoom_out = ctx.input_mut(|i| i.consume_shortcut(&ZOOM_OUT));
            if zoom_in != zoom_out {
                app.preferences.ui_scale = (app.preferences.ui_scale + if zoom_in { 0.1 } else { -0.1 }).clamp(0.5, 2.5);
            }
        }
    }
    // Scaling while the pointer is held moves the slider under the pointer, feeding a
    // different value back into it. Commit the layout change after the drag ends.
    let z = app.preferences.ui_scale.clamp(0.5, 2.5);
    if z.is_finite() && !ctx.input(|i| i.pointer.any_down()) && (ctx.zoom_factor() - z).abs() > 1e-3 {
        ctx.set_zoom_factor(z);
    }
}

/// A new, empty design takes the preferred units (not an undo step, not a change to save).
/// Callers: a launch with no file, New on a design that is already empty, `file.new`, and the
/// blank design left when the last tab closes. A design that already has features is unchanged.
pub fn apply_new_design(app: &mut SolveApp) {
    if app.session.doc.features.is_empty() && app.session.doc.units != app.preferences.default_units {
        let saved = !app.session.is_dirty();
        if app.session.execute("document.units", &json!({ "units": app.preferences.default_units })).is_ok() {
            app.session.undo.pop();
            if saved {
                app.session.mark_saved();
            }
        }
    }
}

/// The autosave interval follows `app.autosave_minutes` (0: off).
pub fn set_autosave(app: &mut SolveApp) {
    let m = app.autosave_minutes;
    if let Some(a) = app.autosave.as_mut() {
        a.interval = if m > 0.0 { std::time::Duration::from_secs_f64(m * 60.0) } else { std::time::Duration::from_secs(u64::MAX / 4) };
    }
}

/// Which navigation dragging `button` with `mods` held does under the preset.
pub fn nav_mode(app: &SolveApp, button: NavButton, mods: Modifiers) -> Option<NavMode> {
    nav_preset(&app.preferences.nav).mode(button, mods)
}

/// +1, or −1 when the wheel zooms the other way.
pub fn zoom_sign(app: &SolveApp) -> f64 {
    if app.preferences.zoom_reverse { -1.0 } else { 1.0 }
}

/// The model point under a screen position. Over empty space: the centre of the visible bodies
/// and sketches (what Fit frames), or the view's centre when nothing is visible.
pub(crate) fn pivot_under(app: &SolveApp, proj: &crate::viewport::Proj, at: Pos2) -> Vec3 {
    use crate::viewport::Hit;
    for h in crate::viewport::pick(app, proj, at) {
        match h {
            Hit::Face { point, .. } | Hit::Vertex { point, .. } => return point,
            Hit::Edge { mid, .. } => return mid,
            _ => {}
        }
    }
    let b = crate::viewport::view_bounds(app);
    if b.is_empty() { app.cam.target } else { b.center() }
}

/// Units per millimetre and the unit's name.
pub fn unit_scale(units: &str) -> (f64, &'static str) {
    match units {
        "cm" => (0.1, "cm"),
        "m" => (0.001, "m"),
        "in" => (1.0 / 25.4, "in"),
        "ft" => (1.0 / 304.8, "ft"),
        _ => (1.0, "mm"),
    }
}

/// Inch labels retain a meaningful fourth digit without promoting floating-point noise.
pub fn length_precision(value: f64, units: &str, decimals: u8) -> usize {
    let base = usize::from(decimals.min(8));
    if units != "in" {
        return base;
    }
    let needs_four = ((value * 10_000.0).round() - (value * 1000.0).round() * 10.0).abs() >= 0.5;
    base.max(if needs_four { 4 } else { 3 })
}

pub fn length_number(mm: f64, units: &str, decimals: u8) -> String {
    let value = mm * unit_scale(units).0;
    format!("{:.*}", length_precision(value, units, decimals), value)
}

/// A plain number typed into a field, with a decimal point or a decimal comma (#1347): "2.5"
/// and "2,5" are both 2.5, as a numeric keypad types either depending on the locale. Spaces are
/// ignored and "−" is a minus, as in egui's own parser. A comma beside a point ("1.234,5") or a
/// second comma is not guessed at: the text is not a number.
pub fn parse_number(text: &str) -> Option<f64> {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).map(|c| if c == '\u{2212}' { '-' } else { c }).collect();
    let t = if !t.contains('.') && t.matches(',').count() == 1 { t.replacen(',', ".", 1) } else { t };
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Edit a millimetre value through a control expressed in the current design units.
/// Keep the stored value untouched unless the user actually changes the field.
pub fn length_drag<'a>(value: &'a mut f64, units: &str, decimals: u8) -> egui::DragValue<'a> {
    let (scale, unit) = unit_scale(units);
    egui::DragValue::new(value)
        .speed(0.5)
        .suffix(format!(" {unit}"))
        .custom_formatter(move |mm, _| length_number(mm, unit, decimals))
        .custom_parser(move |text| parse_length_mm(text, scale))
}

/// A length typed into a field, in millimetres: a bare number is in the design's units (`scale`
/// units per millimetre); an explicit unit ("10 mm", "0.5 in", "2 cm + 3 mm") is honoured.
pub fn parse_length_mm(text: &str, scale: f64) -> Option<f64> {
    use solidcraft_engine::doc::expr;
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let defaults = expr::Defaults { len: 1.0 / scale, ..expr::Defaults::default() };
    let no_params = |n: &str| Err(solidcraft_engine::doc::DocError::Expr(format!("unknown parameter `{n}`")));
    let value = expr::eval_with_defaults(text.trim(), &no_params, defaults).ok()?.to_kind_in(expr::Kind::Length, defaults).ok()?;
    value.is_finite().then_some(value)
}

/// A length in millimetres in `units`, with `decimals` (for display).
pub fn format_length(mm: f64, units: &str, decimals: u8) -> String {
    let (_, u) = unit_scale(units);
    format!("{} {u}", length_number(mm, units, decimals))
}

/// A length, area (`power` 2) or volume (3) given in millimetres, shown in the design's units
/// with the length precision.
pub fn show_mm(app: &SolveApp, v: f64, power: i32) -> String {
    let (k, u) = unit_scale(&app.session.doc.units);
    let sup = match power {
        2 => "²",
        3 => "³",
        _ => "",
    };
    let value = v * k.powi(power);
    format!("{:.*} {u}{sup}", length_precision(value, &app.session.doc.units, app.preferences.length_decimals), value)
}

/// Numbers in millimetres, in the design's units without the unit name.
pub fn show_mm_bare(app: &SolveApp, v: f64) -> String {
    length_number(v, &app.session.doc.units, app.preferences.length_decimals)
}

/// Write the anti-aliasing setting where the host reads it at start (the window's samples can't
/// change while it is open).
fn save_graphics(app: &SolveApp) {
    if let Some(path) = &app.services.graphics_path {
        // Other settings in the file (the graphics `backend`) stay as they are.
        let mut graphics = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| json!({}));
        graphics["msaa"] = json!(app.preferences.msaa);
        let _ = std::fs::write(path, graphics.to_string());
    }
}

/// At start, with the graphics device known: the saved anti-aliasing (`requested` samples, as
/// the host read it) is more than the device supports. The window would not have opened with it,
/// so the renderer fell back; put the setting back to its default (or the largest supported
/// count below it) and tell the user what changed.
pub fn check_msaa(app: &mut SolveApp, requested: u32) {
    let supported = &app.msaa_supported;
    if requested <= 1 || supported.is_empty() || supported.contains(&requested) {
        return;
    }
    let default = Prefs::default().msaa;
    let reset = supported.iter().copied().filter(|&n| n <= default).max().unwrap_or(1);
    let supported = supported.clone();
    app.preferences.msaa = reset;
    save_graphics(app);
    app.settings_reset.push(SettingReset::Msaa { requested, reset, supported });
}

/// A setting put back at start because it would keep SolidCraft from starting.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingReset {
    /// Anti-aliasing samples the graphics device can't render with.
    Msaa { requested: u32, reset: u32, supported: Vec<u32> },
}

impl SettingReset {
    /// The line the notice shows, in the current interface language.
    pub fn message(&self) -> String {
        match self {
            SettingReset::Msaa { requested, reset, supported } => {
                let setting = format!("{} › {}", crate::i18n::tr("ui.graphics"), crate::i18n::tr("ui.anti_aliasing"));
                let supported = supported.iter().map(|n| format!("{n}")).collect::<Vec<_>>().join("×, ") + "×";
                crate::i18n::format(
                    "settings.msaa_reset",
                    &[("setting", &setting), ("requested", &requested.to_string()), ("reset", &reset.to_string()), ("supported", &supported)],
                )
            }
        }
    }
}

/// Settings put back at start (`SolveApp::settings_reset`), until dismissed.
pub fn show_reset_notice(app: &mut SolveApp, ctx: &egui::Context) {
    if app.settings_reset.is_empty() {
        return;
    }
    let mut ok = false;
    let mut prefs = false;
    crate::frame::window(ctx, app.tr("settings.reset_title"), crate::frame::Width::Normal)
        .id(egui::Id::new("sc_settings_reset"))
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(app.tr("settings.reset_intro"));
            ui.add_space(4.0);
            for reset in &app.settings_reset {
                ui.label(format!("• {}", reset.message()));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ok = ui.button(app.tr("ui.heading_ok")).clicked();
                prefs = ui.button(app.tr("ui.open_preferences")).clicked();
            });
        });
    if ok || prefs {
        app.settings_reset.clear();
    }
    if prefs {
        open_at(app, PAGE_GRAPHICS);
    }
}

pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    // File › Keyboard Shortcuts and Help › Keyboard Shortcuts open this dialog at that page.
    if app.keymap.open {
        app.keymap.open = false;
        open_at(app, PAGE_SHORTCUTS);
    }
    if !app.prefs_window.open {
        app.prefs_window.snapshot = None;
        return;
    }
    if app.prefs_window.snapshot.is_none() {
        app.prefs_window.snapshot = Some(Snapshot::take(app));
    }
    let t = Tokens::get();
    let mut open = true;
    let mut button: Option<&str> = None;
    let before = app.preferences.clone();
    let shown = crate::frame::window(ctx, app.tr("ui.preferences"), crate::frame::Width::Wide)
        .id(egui::Id::new("sc_prefs"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            // One fixed size for every page (#290): the page is drawn into a clipped child of
            // that size and only that size is allocated, so no page (or translation) can widen
            // the window; descriptions wrap and tall pages scroll inside it.
            ui.set_max_width(PREFS_W);
            let (body, _) = ui.allocate_exact_size(egui::vec2(PREFS_W, PREFS_H), egui::Sense::hover());
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::left_to_right(egui::Align::Min)));
            child.shrink_clip_rect(body);
            let ui_body = &mut child;
            ui_body.vertical(|ui| {
                ui.set_width(PREFS_NAV_W);
                for (i, s) in PAGES.iter().enumerate() {
                    if ui.selectable_label(app.prefs_window.section == i, app.tr_label(s)).clicked() {
                        app.prefs_window.section = i;
                        app.keymap.read_only = false;
                    }
                }
            });
            ui_body.separator();
            ui_body.vertical(|ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(app.tr_label(PAGES.get(app.prefs_window.section).copied().unwrap_or_default())).strong().size(14.0));
                ui.add_space(6.0);
                if app.prefs_window.section == PAGE_SHORTCUTS {
                    // The shortcuts table scrolls by itself.
                    page(app, ui, &t);
                } else {
                    egui::ScrollArea::vertical().id_salt("sc_prefs_page").auto_shrink([false, false]).show(ui, |ui| page(app, ui, &t));
                }
            });
            ui.separator();
            // Restore Defaults at the left; OK, Cancel and Apply flush right, as in every dialog.
            ui.horizontal(|ui| {
                ui.set_width(PREFS_W);
                if ui.button(app.tr("ui.restore_defaults")).on_hover_text(app.tr("ui.reset_the_settings_on_this_page")).clicked() {
                    button = Some("defaults");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Right to left: Apply is the rightmost.
                    for (key, id) in [("ui.apply", "apply"), ("ui.cancel", "cancel"), ("ui.heading_ok", "ok")] {
                        let r = ui.button(app.tr(key));
                        crate::scenario::publish_rect(&format!("prefs:{id}"), r.rect);
                        if r.clicked() {
                            button = Some(id);
                        }
                    }
                });
                crate::scenario::publish_rect("prefs:footer", ui.max_rect());
            });
        });
    if let Some(w) = &shown {
        crate::scenario::publish_rect("prefs:window", w.response.rect);
    }
    if app.preferences.msaa != before.msaa {
        save_graphics(app);
    }
    if app.preferences.up_axis != before.up_axis {
        up_axis_changed(app);
    }
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !crate::keymap::capturing(app);
    match button {
        Some("defaults") => defaults(app, app.prefs_window.section),
        Some("apply") => app.prefs_window.snapshot = Some(Snapshot::take(app)),
        Some("ok") => close(app, false),
        Some("cancel") => close(app, true),
        _ if !open || esc => close(app, !open || esc),
        _ => {}
    }
}

/// Close the dialog; `revert`: put back what was there when it opened (Cancel, Esc, ×).
fn close(app: &mut SolveApp, revert: bool) {
    if let Some(s) = app.prefs_window.snapshot.take()
        && revert
    {
        let up = app.preferences.up_axis.clone();
        s.restore(app);
        if app.preferences.up_axis != up {
            up_axis_changed(app);
        }
    }
    app.prefs_window.open = false;
    app.keymap.read_only = false;
    crate::keymap::stop_capture(app);
}

/// The default orientation changed: the view turns to match.
fn up_axis_changed(app: &mut SolveApp) {
    use solidcraft_engine::render::UpAxis;
    app.prefs_window.applied_up = Some(app.preferences.up_axis.clone());
    let up = if app.preferences.up_axis.eq_ignore_ascii_case("y") { UpAxis::Y } else { UpAxis::Z };
    if app.ui.up_axis != up || app.cam.up != up {
        app.set_up_axis(up);
    }
}

/// The Preferences window's content size: the same on every page.
const PREFS_W: f32 = 744.0;
const PREFS_H: f32 = 400.0;
/// The page list at its left.
const PREFS_NAV_W: f32 = 128.0;

/// One page of the dialog.
fn page(app: &mut SolveApp, ui: &mut egui::Ui, t: &Tokens) {
    let note = |ui: &mut egui::Ui, s: &str| {
        ui.label("");
        // Descriptions wrap within the page instead of widening it.
        ui.add(egui::Label::new(RichText::new(s).size(11.0).color(t.text_dim)).wrap());
        ui.end_row();
    };
    if app.prefs_window.section == PAGE_SHORTCUTS {
        crate::keymap::page(app, ui);
        return;
    }
    if app.prefs_window.section == PAGE_AI {
        crate::agent_chat::settings::page(app, ui);
        return;
    }
    egui::Grid::new(("sc_prefs_grid", app.prefs_window.section)).num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| match app.prefs_window.section {
        0 => {
            ui.label(app.tr("ui.language"));
            egui::ComboBox::from_id_salt("pref_language").selected_text(crate::i18n::Locale::for_language(&app.preferences.language).name()).show_ui(
                ui,
                |ui| {
                    for locale in crate::i18n::LOCALES {
                        let (id, label) = (locale.id, locale.name);
                        ui.selectable_value(&mut app.preferences.language, id.to_string(), label);
                    }
                },
            );
            ui.end_row();
            ui.label(app.tr("ui.theme"));
            ui.horizontal(|ui| {
                for (id, label) in THEMES {
                    if ui.radio(app.preferences.theme == id, app.tr_label(label)).clicked() {
                        app.preferences.theme = id.to_string();
                        if id != "system" {
                            app.ui.dark = id == "dark";
                        }
                    }
                }
            });
            ui.end_row();
            ui.label(app.tr("ui.ui_scale"));
            let scale = ui.add(
                egui::Slider::new(&mut app.preferences.ui_scale, 0.5..=2.5).step_by(0.05).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)),
            );
            crate::scenario::publish_rect("prefs:scale", scale.rect);
            ui.end_row();
            if !cfg!(target_arch = "wasm32") {
                ui.label(app.tr_label("Interface font"));
                ui_font_row(app, ui);
                ui.end_row();
            }
            ui.label(app.tr("ui.feature_history"));
            let timeline_bottom = app.tr("ui.timeline_bottom").to_string();
            let timeline_left = app.tr("ui.timeline_left_sidebar").to_string();
            ui.horizontal(|ui| {
                ui.radio_value(&mut app.preferences.timeline_left, false, timeline_bottom);
                ui.radio_value(&mut app.preferences.timeline_left, true, timeline_left);
            });
            ui.end_row();
            note(ui, app.tr("ui.timeline_left_note"));
            ui.label(app.tr("ui.units_of_new_designs"));
            egui::ComboBox::from_id_salt("pref_units").selected_text(app.preferences.default_units.clone()).show_ui(ui, |ui| {
                for u in UNITS {
                    ui.selectable_value(&mut app.preferences.default_units, u.to_string(), u);
                }
            });
            ui.end_row();
            ui.label(app.tr("ui.autosave_every"));
            if ui.add(egui::DragValue::new(&mut app.autosave_minutes).range(0.0..=120.0).speed(0.5).suffix(" min").custom_parser(parse_number)).changed() {
                set_autosave(app);
            }
            ui.end_row();
            note(ui, crate::i18n::tr_label("0 turns it off"));
            ui.label(app.tr("ui.length_decimals"));
            ui.add(egui::DragValue::new(&mut app.preferences.length_decimals).range(0..=6));
            ui.end_row();
            ui.label(app.tr("ui.angle_decimals"));
            ui.add(egui::DragValue::new(&mut app.preferences.angle_decimals).range(0..=6));
            ui.end_row();
            ui.label(app.tr("ui.agent_cursor"));
            let speed_label = app.tr("ui.agent_cursor_speed");
            let s = &mut app.ui.agent_cursor;
            ui.checkbox(&mut s.show, crate::i18n::tr_label("Show the pointer an AI agent moves (#35)"));
            ui.end_row();
            ui.label(speed_label);
            ui.add_enabled_ui(s.show, |ui| {
                crate::agent_cursor::speed_slider(ui, &mut s.speed);
            });
            ui.end_row();
            ui.label("");
            ui.add_enabled(s.show, egui::Checkbox::new(&mut s.follow_camera, crate::i18n::tr_label("Follow the camera (Look At sketches)")));
            ui.end_row();
            crate::update::prefs_rows(app, ui, t);
        }
        1 => {
            ui.label(app.tr_label("Pan, zoom, orbit"));
            let current_preset_label = nav_preset(&app.preferences.nav).translated_label();
            egui::ComboBox::from_id_salt("pref_nav").selected_text(current_preset_label).show_ui(ui, |ui| {
                for p in &NAV_PRESETS {
                    if ui.selectable_value(&mut app.preferences.nav, p.id.to_string(), p.translated_label()).clicked() {
                        app.preferences.zoom_reverse = p.reverse_zoom;
                    }
                }
            });
            ui.end_row();
            if app.preferences.nav == TOUCHPAD_NAV {
                note(ui, crate::i18n::tr("nav.touchpad.help"));
            } else {
                let snap_help = if app.preferences.nav == OPENSCAD_NAV { "" } else { "; hold Shift during orbit to snap" };
                let raw_help = format!("{}; Ctrl+left drag on empty space orbits{snap_help}; hold Space to pan; the wheel zooms", nav_preset(&app.preferences.nav).help);
                note(ui, crate::i18n::tr_label(&raw_help));
            }
            ui.label(crate::i18n::tr_label("Context menu"));
            ui.checkbox(&mut app.preferences.radial_context_menu, crate::i18n::tr_label("Radial context menu"));
            ui.end_row();
            ui.label(crate::i18n::tr_label("View navigator"));
            let picker =
                egui::ComboBox::from_id_salt("pref_navigator_style").selected_text(crate::i18n::tr_label(app.preferences.navigator_style.label())).show_ui(ui, |ui| {
                    for style in NavigatorStyle::ALL {
                        let response = ui.selectable_value(&mut app.preferences.navigator_style, style, crate::i18n::tr_label(style.label()));
                        crate::scenario::publish_rect(&format!("prefs:navigator:{}", style.label()), response.rect);
                        crate::scenario::publish_handle(&format!("prefs:navigator:{}", style.label()), response.rect.center());
                    }
                });
            crate::scenario::publish_rect("prefs:navigator", picker.response.rect);
            crate::scenario::publish_handle("prefs:navigator", picker.response.rect.center());
            ui.end_row();
            ui.label(crate::i18n::tr_label("Reverse zoom direction"));
            ui.checkbox(&mut app.preferences.zoom_reverse, "");
            ui.end_row();
            ui.label(crate::i18n::tr_label("Zoom to the cursor"));
            ui.checkbox(&mut app.preferences.zoom_to_cursor, "")
                .on_hover_text(crate::i18n::tr_label("The wheel zooms toward the point under the cursor (off: toward the view's centre)"));
            ui.end_row();
            ui.label(crate::i18n::tr_label("Smooth wheel zoom"));
            ui.checkbox(&mut app.preferences.smooth_wheel_zoom, "")
                .on_hover_text(crate::i18n::tr_label("Turn off to apply each wheel event immediately, without a smoothing tail"));
            ui.end_row();
            ui.label(crate::i18n::tr_label("Two-finger scroll pans"));
            let touchpad = app.preferences.nav == TOUCHPAD_NAV;
            ui.add_enabled(!touchpad, egui::Checkbox::new(&mut app.preferences.trackpad_pan, "")).on_hover_text(if touchpad {
                crate::i18n::tr("nav.touchpad.scroll")
            } else {
                crate::i18n::tr_label("Trackpad gestures pan (off: they zoom like the wheel); mouse wheel zooms, thumb wheel pans sideways, pinch always zooms")
            });
            ui.end_row();
            ui.label(crate::i18n::tr_label("Orbit around"));
            // Wrapped: three choices, and German or Russian labels are long.
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(&mut app.preferences.orbit_around, OrbitAround::Origin, crate::i18n::tr_label("Origin"));
                ui.radio_value(&mut app.preferences.orbit_around, OrbitAround::ViewCentre, crate::i18n::tr_label("View centre"));
                ui.radio_value(&mut app.preferences.orbit_around, OrbitAround::Cursor, crate::i18n::tr_label("Point under the cursor"));
            });
            ui.end_row();
            ui.label(crate::i18n::tr_label("Middle double-click"));
            let current = MiddleDoubleClick::of(app);
            egui::ComboBox::from_id_salt("pref_middle_double_click").selected_text(crate::i18n::tr_label(current.label())).show_ui(ui, |ui| {
                for (m, id, label) in MiddleDoubleClick::ALL {
                    if ui.selectable_label(current == m, crate::i18n::tr_label(label)).clicked() {
                        app.preferences.middle_double_click = id.into();
                    }
                }
            })
            .response
            .on_hover_text(crate::i18n::tr_label("Double-clicking the middle button in the view: Home (the home view, framing what is shown), Fit (frame what is shown, keeping the orientation) or nothing"));
            ui.end_row();
            ui.label(crate::i18n::tr_label("Animate view changes"));
            ui.checkbox(&mut app.preferences.look_at_anim, crate::i18n::tr_label("Axis view, Look At, Home and Fit"));
            ui.end_row();
            ui.label(crate::i18n::tr_label("Animation speed"));
            ui.add_enabled(
                app.preferences.look_at_anim,
                egui::Slider::new(&mut app.preferences.look_at_speed, 0.5..=3.0).step_by(0.25).custom_formatter(|v, _| format!("{v:.2}×")),
            );
            ui.end_row();
            ui.label(crate::i18n::tr_label("3D mouse"));
            ui.checkbox(&mut app.preferences.space_mouse_enabled, crate::i18n::tr_label("Navigate with SpaceNavigator / SpaceMouse"));
            ui.end_row();
            ui.label(crate::i18n::tr_label("3D mouse speed"));
            ui.add_enabled(
                app.preferences.space_mouse_enabled,
                egui::Slider::new(&mut app.preferences.space_mouse_speed, 0.25..=3.0).step_by(0.05).custom_formatter(|v, _| format!("{v:.2}×")),
            );
            ui.end_row();
            for (axis, name) in SPACE_MOUSE_AXES.iter().enumerate() {
                ui.label(crate::i18n::tr_label(name));
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(app.preferences.space_mouse_enabled, |ui| {
                        if let Some(reverse) = app.preferences.space_mouse_reverse.get_mut(axis) {
                            ui.checkbox(reverse, crate::i18n::tr_label("Invert"));
                        }
                        if let Some(speed) = app.preferences.space_mouse_axis_speed.get_mut(axis) {
                            ui.add(egui::Slider::new(speed, 0.0..=3.0).step_by(0.05).custom_formatter(|v, _| format!("{v:.2}×")));
                        }
                    });
                });
                ui.end_row();
            }
            note(ui, crate::i18n::tr_label("Move and rotate the view together; sketches use pan and zoom only."));
        }
        PAGE_DESIGN => {
            ui.label(crate::i18n::tr_label("Default modelling orientation"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut app.preferences.up_axis, "z".to_string(), crate::i18n::tr_label("Z up"));
                ui.radio_value(&mut app.preferences.up_axis, "y".to_string(), crate::i18n::tr_label("Y up"));
            });
            ui.end_row();
            note(ui, crate::i18n::tr_label("Which axis points up in the views (axis view, Home, Look At). Designs are unchanged."));
            ui.label(crate::i18n::tr_label("Offer to stand up Y-up files"));
            ui.checkbox(&mut app.preferences.offer_stand_up, "")
                .on_hover_text(crate::i18n::tr_label("After a STEP or IGES file opens with Z up, offer a quarter turn about X"));
            ui.end_row();
        }
        4 => {
            ui.label(crate::i18n::tr_label("Visual style"));
            let styles = ["Shaded with edges", "Shaded", "Wireframe", "Shaded with hidden edges"];
            let selected_label = crate::i18n::tr_label(styles[usize::from(app.ui.visual_style.min(3))]);
            egui::ComboBox::from_id_salt("pref_style").selected_text(selected_label).show_ui(ui, |ui| {
                for (i, s) in styles.iter().enumerate() {
                    ui.selectable_value(&mut app.ui.visual_style, i as u8, crate::i18n::tr_label(s));
                }
            });
            ui.end_row();
            ui.label(crate::i18n::tr_label("Grid"));
            ui.checkbox(&mut app.ui.show_grid, "");
            ui.end_row();
            ui.label(crate::i18n::tr_label("Grid follows the view"));
            ui.add_enabled(app.ui.show_grid, egui::Checkbox::without_text(&mut app.preferences.grid_follows_view))
                .on_hover_text(crate::i18n::tr_label("Side views show the grid on the upright plane facing them instead of the ground"));
            ui.end_row();
            ui.label(crate::i18n::tr_label("Ground shadow"));
            ui.checkbox(&mut app.ui.ground_shadow, "");
            ui.end_row();
            ui.label(crate::i18n::tr_label("Perspective view"));
            ui.checkbox(&mut app.ui.perspective, "");
            ui.end_row();
            ui.label(app.tr("ui.anti_aliasing"));
            egui::ComboBox::from_id_salt("pref_msaa").selected_text(format!("{}×", app.preferences.msaa)).show_ui(ui, |ui| {
                for m in MSAA {
                    // Counts the graphics device can't do would keep the window from opening.
                    let ok = app.msaa_supported.is_empty() || app.msaa_supported.contains(&m);
                    ui.add_enabled_ui(ok, |ui| ui.selectable_value(&mut app.preferences.msaa, m, format!("{m}×")))
                        .inner
                        .on_disabled_hover_text(app.tr("settings.msaa_unsupported"));
                }
            });
            ui.end_row();
            note(ui, app.tr("settings.msaa_applies_on_restart"));
            ui.label(app.tr("ui.display_quality"));
            {
                use solidcraft_engine::doc::display_quality::DisplayQuality;
                let label = |q: DisplayQuality| crate::i18n::tr(&format!("ui.display_quality_{}", q.id())).to_string();
                let current = DisplayQuality::parse(&app.preferences.display_quality).unwrap_or_default();
                egui::ComboBox::from_id_salt("pref_display_quality").selected_text(label(current)).show_ui(ui, |ui| {
                    for q in DisplayQuality::ALL {
                        if ui.selectable_label(q == current, label(q)).clicked() {
                            app.preferences.display_quality = q.id().to_string();
                        }
                    }
                });
            }
            ui.end_row();
            note(ui, app.tr("settings.display_quality_note"));
            ui.label(app.tr("ui.supersampling"));
            let ss_label = |f: f32| if f > 1.0 { format!("{f}×") } else { crate::i18n::tr("ui.supersampling_off").to_string() };
            let adaptive = app.tr("ui.supersampling_adaptive").to_string();
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("pref_ssaa").selected_text(ss_label(app.preferences.supersampling)).show_ui(ui, |ui| {
                    for f in SUPERSAMPLING {
                        ui.selectable_value(&mut app.preferences.supersampling, f, ss_label(f));
                    }
                });
                ui.add_enabled(app.preferences.supersampling > 1.0, egui::Checkbox::new(&mut app.preferences.supersampling_adaptive, adaptive));
            });
            ui.end_row();
            note(ui, app.tr("settings.supersampling_note"));
            ui.label(crate::i18n::tr_label("Renderer"));
            ui.label(match (&app.viewport.gpu, &app.help.gpu) {
                (Some(_), Some(g)) => format!("GPU: {g}"),
                (Some(_), None) => "GPU".to_string(),
                _ => "CPU (software)".to_string(),
            });
            ui.end_row();
            ui.label(app.tr("settings.measure_axis_distances"));
            let response = ui.checkbox(&mut app.preferences.measure_axis_distances, "");
            crate::scenario::publish_rect("prefs:measure_axis_distances", response.rect);
            response.on_hover_text(app.tr("settings.measure_axis_distances_help"));
            ui.end_row();
            // Colour vision (#778): its own section at the end of the page.
            ui.label(RichText::new(app.tr("ui.colour_vision")).strong());
            ui.label("");
            ui.end_row();
            ui.label(app.tr("ui.colour_vision_mode"));
            let mode = app.preferences.color_vision;
            egui::ComboBox::from_id_salt("pref_color_vision").selected_text(app.tr(mode.label_id())).show_ui(ui, |ui| {
                for m in crate::theme::ColorVision::ALL {
                    ui.selectable_value(&mut app.preferences.color_vision, m, crate::i18n::tr(m.label_id())).on_hover_text(crate::i18n::tr(m.help_id()));
                }
            });
            if app.preferences.color_vision != mode {
                // At once, like the theme: the next frame draws in the new colours.
                ui.ctx().request_repaint();
            }
            ui.end_row();
            ui.label(app.tr("ui.colour_vision_sample"));
            colour_vision_sample(ui, t);
            ui.end_row();
            note(ui, app.tr("ui.colour_vision_note"));
            // Last, so the colour vision section above keeps its place on the page.
            ui.label("");
            ui.end_row();
            ui.label(app.tr("ui.frame_rate"));
            ui.checkbox(&mut app.preferences.show_frame_rate, "");
            ui.end_row();
            note(ui, app.tr("settings.frame_rate_note"));
        }
        5 => {
            ui.label(crate::i18n::tr_label("Project face edges into new sketches"));
            let mut auto = app.session.auto_project;
            if ui.checkbox(&mut auto, "").changed() {
                let _ = app.run("sketch.auto_project", json!({ "value": auto }));
            }
            ui.end_row();
        }
        _ => {
            ui.label(crate::i18n::tr_label("Click selects whole bodies"));
            ui.checkbox(&mut app.ui.pick_bodies, "");
            ui.end_row();
        }
    });
}

/// The names under the Colour vision sample (message ids).
pub(crate) const COLOUR_VISION_SAMPLE: [&str; 6] =
    ["ui.colour_vision_fixed", "ui.colour_vision_projected", "ui.axes", "Bends", "ui.draft", "ui.colour_vision_failed"];

/// The Colour vision sample (#778): fixed and projected sketch lines, the X and Y axes, bends up
/// and down, Draft Analysis and a failed feature, each drawn as it is where it appears, in the
/// colours on now. One fixed-size area, so the grid row is as tall as what it draws.
fn colour_vision_sample(ui: &mut egui::Ui, t: &Tokens) {
    const W: f32 = 72.0;
    const LINE: f32 = 4.0;
    let (area, _) = ui.allocate_exact_size(egui::vec2(W * COLOUR_VISION_SAMPLE.len() as f32, 50.0), egui::Sense::hover());
    let p = ui.painter_at(area);
    let line = |c| egui::Stroke::new(LINE, c);
    for (i, name) in COLOUR_VISION_SAMPLE.iter().enumerate() {
        let cell = egui::Rect::from_min_size(area.min + egui::vec2(W * i as f32, 0.0), egui::vec2(W - 8.0, 32.0));
        let (a, b) = (cell.left_center() + egui::vec2(8.0, 0.0), cell.right_center() - egui::vec2(8.0, 0.0));
        match i {
            0 | 1 => {
                p.rect_filled(cell, 3.0, t.viewport_top);
                p.line_segment([a, b], line(if i == 0 { t.sketch_locked } else { t.sketch_projected }));
            }
            2 => {
                // X along and Y up from one corner, as the view triad draws them.
                p.rect_filled(cell, 3.0, t.viewport_top);
                let o = cell.left_bottom() + egui::vec2(12.0, -7.0);
                p.line_segment([o, egui::pos2(cell.right() - 10.0, o.y)], line(t.triad[0]));
                p.line_segment([o, egui::pos2(o.x, cell.top() + 6.0)], line(t.triad[1]));
            }
            3 => {
                // Dashed, as the flat pattern draws them.
                p.rect_filled(cell, 3.0, t.viewport_top);
                for (dy, c) in [(-7.0, t.bend_up), (7.0, t.bend_down)] {
                    p.extend(egui::Shape::dashed_line(&[a + egui::vec2(0.0, dy), b + egui::vec2(0.0, dy)], line(c), 8.0, 5.0));
                }
            }
            4 => {
                // The three bands of Draft Analysis; in a mode the negative one is striped.
                let w = cell.width() / 3.0;
                for (k, c) in t.analysis.draft.iter().enumerate() {
                    let band = egui::Rect::from_min_size(cell.min + egui::vec2(w * k as f32, 0.0), egui::vec2(w, cell.height()));
                    p.rect_filled(band, 0.0, crate::theme::srgb(*c));
                    if k == 2 && t.analysis.hatch {
                        let stripes = p.with_clip_rect(band);
                        let dark = crate::theme::srgb(c.map(|x| x * 0.6));
                        let mut x = band.left() - band.height();
                        while x < band.right() {
                            stripes.line_segment(
                                [egui::pos2(x, band.bottom()), egui::pos2(x + band.height(), band.top())],
                                egui::Stroke::new(2.0, dark),
                            );
                            x += 9.0;
                        }
                    }
                }
            }
            _ => {
                // A failed feature on the timeline.
                p.rect(cell.shrink2(egui::vec2(10.0, 3.0)), 3.0, t.timeline_item_error, egui::Stroke::new(1.0, t.error), egui::StrokeKind::Inside);
            }
        }
        p.text(
            cell.center_bottom() + egui::vec2(0.0, 3.0),
            egui::Align2::CENTER_TOP,
            crate::i18n::tr(name),
            egui::FontId::proportional(11.0),
            t.text_dim,
        );
    }
}

/// The control channel's `ui.prefs`: `open` (a page name) opens the dialog there, `button`
/// presses OK, Cancel, Apply or Restore Defaults, `stand_up: true` answers the stand-up offer
/// (`false` leaves the file as it is, like "Leave it"); any other keys set preferences. Returns
/// them all, the open page and the offer.
pub fn control(app: &mut SolveApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut rest = p.as_object().cloned().unwrap_or_default();
    if let Some(page) = rest.remove("open") {
        let name = page.as_str().unwrap_or("General");
        let i = PAGES.iter().position(|x| x.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no page `{name}` ({})", PAGES.join(", ")))?;
        open_at(app, i);
        if app.prefs_window.snapshot.is_none() {
            app.prefs_window.snapshot = Some(Snapshot::take(app));
        }
    }
    match rest.remove("stand_up").and_then(|v| v.as_bool()) {
        Some(true) => crate::stand_up::stand_up(app)?,
        Some(false) => crate::stand_up::leave(app),
        None => {}
    }
    let button = rest.remove("button");
    let mut out = set(app, &serde_json::Value::Object(rest))?;
    match button.as_ref().and_then(|b| b.as_str()) {
        Some("ok") => close(app, false),
        Some("cancel") => close(app, true),
        Some("apply") => app.prefs_window.snapshot = Some(Snapshot::take(app)),
        Some("defaults") => defaults(app, app.prefs_window.section),
        Some(b) => return Err(format!("unknown button `{b}` (ok, cancel, apply, defaults)")),
        None => {}
    }
    if button.is_some() {
        out = serde_json::to_value(&app.preferences).map_err(|e| e.to_string())?;
    }
    if let Some(o) = out.as_object_mut() {
        o.insert("dialog".into(), json!(app.prefs_window.open.then(|| PAGES.get(app.prefs_window.section).copied().unwrap_or_default())));
        o.insert("stand_up_offer".into(), json!(app.stand_up));
        o.insert("dark".into(), json!(app.ui.dark));
    }
    Ok(out)
}

/// Fold a setting an older release kept in its own field into the one this version reads.
/// `orbit_cursor` (`true` = the point under the cursor) became `orbit_around`.
/// The cap's physical axes, in the order of `Prefs::space_mouse_reverse`.
const SPACE_MOUSE_AXES: [&str; 6] = ["Push left / right", "Push forward / back", "Lift / press", "Tilt forward / back", "Tilt left / right", "Twist"];

pub fn migrate(mut p: Prefs) -> Prefs {
    if p.orbit_cursor {
        p.orbit_around = OrbitAround::Cursor;
        p.orbit_cursor = false;
    }
    p
}

/// Saved preferences, keeping every one that still reads. A value this version doesn't
/// understand (a setting a newer or older release saved differently, a hand edit) falls back to
/// its default on its own instead of discarding all of them.
pub fn lenient(saved: &serde_json::Value) -> Prefs {
    if let Ok(p) = serde_json::from_value::<Prefs>(saved.clone()) {
        return migrate(p);
    }
    let Ok(mut cur) = serde_json::to_value(Prefs::default()) else { return Prefs::default() };
    if let (Some(o), Some(src)) = (cur.as_object_mut(), saved.as_object()) {
        for (k, v) in src.iter().take(1000) {
            // Keys this version doesn't have are ignored anyway.
            let Some(old) = o.get(k).cloned() else { continue };
            o.insert(k.clone(), v.clone());
            if serde_json::from_value::<Prefs>(serde_json::Value::Object(o.clone())).is_err() {
                log::warn!("preferences: `{k}` is not understood by this version; using its default");
                o.insert(k.clone(), old);
            }
        }
    }
    serde_json::from_value(cur).map(migrate).unwrap_or_default()
}

/// Set preferences from JSON (control channel); returns them all.
pub fn set(app: &mut SolveApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut cur = serde_json::to_value(&app.preferences).map_err(|e| e.to_string())?;
    if let (Some(o), Some(src)) = (cur.as_object_mut(), p.as_object()) {
        for (k, v) in src {
            if !o.contains_key(k) {
                return Err(format!("unknown preference `{k}`"));
            }
            o.insert(k.clone(), v.clone());
        }
    }
    // Saved preferences read an unknown mode as Off; asked for here, it is an error.
    if let Some(v) = p.get("color_vision")
        && v.as_str().and_then(crate::theme::ColorVision::from_id).is_none()
    {
        return Err("`color_vision` must be off, protan, deutan or tritan".into());
    }
    let new: Prefs = migrate(serde_json::from_value(cur).map_err(|e| e.to_string())?);
    if !UNITS.contains(&new.default_units.as_str()) {
        return Err(format!("units must be one of {}", UNITS.join(", ")));
    }
    if !NAV_PRESETS.iter().any(|p| p.id == new.nav) || !MSAA.contains(&new.msaa) {
        return Err(format!("`nav` must be one of {} and `msaa` 1, 2, 4 or 8", NAV_PRESETS.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")));
    }
    if new.msaa != app.preferences.msaa && !app.msaa_supported.is_empty() && !app.msaa_supported.contains(&new.msaa) {
        return Err(format!("this graphics device doesn't support `msaa` {}; it supports {:?}", new.msaa, app.msaa_supported));
    }
    if !THEMES.iter().any(|(id, _)| *id == new.theme) || !["z", "y"].contains(&new.up_axis.as_str()) {
        return Err("`theme` must be dark, light or system and `up_axis` z or y".into());
    }
    if !crate::i18n::is_supported(&new.language) {
        return Err(format!("`language` must be one of: {}", crate::i18n::LOCALES.iter().map(|locale| locale.id).collect::<Vec<_>>().join(", ")));
    }
    if new.ui_font != app.preferences.ui_font && !new.ui_font.trim().is_empty() && crate::fonts::ui_font(&new.ui_font).is_none() {
        return Err(format!("`ui_font`: `{}` is not an installed font family or a readable font file", new.ui_font.trim()));
    }
    if !(0.5..=2.5).contains(&new.ui_scale) || !(0.25..=4.0).contains(&new.look_at_speed) {
        return Err("`ui_scale` must be 0.5…2.5 and `look_at_speed` 0.25…4".into());
    }
    if solidcraft_engine::doc::display_quality::DisplayQuality::parse(&new.display_quality).is_none() {
        return Err("`display_quality` must be coarse, normal, fine or ultra".into());
    }
    if !SUPERSAMPLING.contains(&new.supersampling) {
        return Err("`supersampling` must be 1 (off), 1.5 or 2".into());
    }
    if MiddleDoubleClick::parse(&new.middle_double_click).is_none() {
        return Err("`middle_double_click` must be home, fit or off".into());
    }
    if new.theme != app.preferences.theme && new.theme != "system" {
        app.ui.dark = new.theme == "dark";
    }
    let up_changed = new.up_axis != app.preferences.up_axis;
    let msaa_changed = new.msaa != app.preferences.msaa;
    app.preferences = new;
    if msaa_changed {
        save_graphics(app);
    }
    if up_changed {
        up_axis_changed(app);
    }
    serde_json::to_value(&app.preferences).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    /// The interface font (#1290): a font that can be read, or empty for the built-in one; saved
    /// with the others and reset by Restore Defaults.
    #[test]
    fn ui_font_is_checked_saved_and_restored() {
        use serde_json::json;
        let mut app = crate::SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        assert_eq!(app.preferences.ui_font, "");
        let family = solidcraft_engine::sketch::DEFAULT_FONT_FAMILY;
        let out = super::set(&mut app, &json!({ "ui_font": family })).unwrap();
        assert_eq!(out["ui_font"], family);
        let e = super::set(&mut app, &json!({"ui_font": "No Such Font 1290"})).unwrap_err();
        assert!(e.contains("ui_font"), "{e}");
        assert_eq!(app.preferences.ui_font, family, "a refused font leaves the choice as it was");
        let mut other = crate::SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        other.load_prefs(&app.prefs());
        assert_eq!(other.preferences.ui_font, family);
        super::defaults(&mut app, 0);
        assert_eq!(app.preferences.ui_font, "");
    }

    #[test]
    fn number_fields_take_a_decimal_point_or_a_decimal_comma() {
        // #1347
        assert_eq!(super::parse_number("2.5"), Some(2.5));
        assert_eq!(super::parse_number("2,5"), Some(2.5));
        assert_eq!(super::parse_number(" -0,25 "), Some(-0.25));
        assert_eq!(super::parse_number("\u{2212}1,5"), Some(-1.5));
        assert_eq!(super::parse_number("12"), Some(12.0));
        for bad in ["1.234,5", "1,2,3", ",", "abc", "", "inf", "NaN"] {
            assert_eq!(super::parse_number(bad), None, "{bad}");
        }
    }

    /// Display quality (#1287) is a preference like any other: set by name, checked, saved with
    /// the others, back to Normal with Restore Defaults.
    #[test]
    fn display_quality_is_set_checked_and_restored() {
        use serde_json::json;
        let mut app = crate::SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        assert_eq!(app.preferences.display_quality, "normal");
        let out = super::set(&mut app, &json!({"display_quality": "fine"})).unwrap();
        assert_eq!(out["display_quality"], "fine");
        assert!(super::set(&mut app, &json!({"display_quality": "silky"})).unwrap_err().contains("coarse, normal, fine or ultra"));
        assert_eq!(app.preferences.display_quality, "fine");
        assert_eq!(super::lenient(&json!({"display_quality": "ultra"})).display_quality, "ultra");
        super::defaults(&mut app, super::PAGE_GRAPHICS);
        assert_eq!(app.preferences.display_quality, "normal");
    }

    /// Supersampling (#1288): off by default, adaptive, set by factor and checked.
    #[test]
    fn supersampling_is_off_by_default_and_checked() {
        use serde_json::json;
        let mut app = crate::SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        assert_eq!((app.preferences.supersampling, app.preferences.supersampling_adaptive), (1.0, true));
        super::set(&mut app, &json!({"supersampling": 2.0})).unwrap();
        assert_eq!(app.preferences.supersampling, 2.0);
        assert!(super::set(&mut app, &json!({"supersampling": 3.0})).unwrap_err().contains("1 (off), 1.5 or 2"));
        super::set(&mut app, &json!({"supersampling": 1.5, "supersampling_adaptive": false})).unwrap();
        super::defaults(&mut app, super::PAGE_GRAPHICS);
        assert_eq!((app.preferences.supersampling, app.preferences.supersampling_adaptive), (1.0, true));
    }

    #[test]
    fn a_value_this_version_does_not_understand_keeps_the_other_preferences() {
        use serde_json::json;
        // A navigator, a field of the wrong type and a key from another release.
        let saved = json!({"nav": "solidworks", "navigator_style": "hologram", "ui_scale": "big", "zoom_reverse": true, "from_the_future": 3});
        let p = super::lenient(&saved);
        assert_eq!(p.nav, "solidworks");
        assert!(p.zoom_reverse);
        assert_eq!(p.navigator_style, super::NavigatorStyle::Axes);
        assert_eq!(p.ui_scale, super::Prefs::default().ui_scale);
        assert_eq!(super::lenient(&json!("garbage")), super::Prefs::default());
        // Whole-app load: the other saved state is kept too, and nothing panics.
        let mut app = crate::SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        app.load_prefs(&json!({"grid": false, "preferences": saved}).to_string());
        assert!(!app.ui.show_grid && app.preferences.nav == "solidworks" && app.preferences.zoom_reverse);
        for junk in ["", "{", "[]", "null", "{\"preferences\": 7, \"recent\": {}, \"shortcuts\": 1, \"toolbar\": \"x\"}"] {
            crate::SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default()).load_prefs(junk);
        }
    }

    #[test]
    fn stored_prefs_fall_back_to_the_legacy_key() {
        let only_old = |k: &str| (k == super::LEGACY_STORAGE_KEY).then(|| "old".to_string());
        assert_eq!(super::stored(only_old).as_deref(), Some("old"));
        let both = |k: &str| Some(if k == super::STORAGE_KEY { "new" } else { "old" }.to_string());
        assert_eq!(super::stored(both).as_deref(), Some("new"));
        assert_eq!(super::stored(|_| None), None);
    }

    use super::*;
    use crate::navigation::orbit;

    #[test]
    fn unsupported_saved_msaa_is_reset_to_the_default_with_a_notice() {
        let dir = std::env::temp_dir().join(format!("sc_msaa_reset_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("graphics.json");
        let services = crate::Services { graphics_path: Some(path.to_string_lossy().to_string()), ..Default::default() };
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), services);
        // WebGPU's guarantee for the surface format: 8x saved, the device can only do 1x and 4x.
        app.msaa_supported = vec![1, 4];
        app.preferences.msaa = 8;
        std::fs::write(&path, r#"{"msaa":8,"backend":"vulkan"}"#).unwrap();
        check_msaa(&mut app, 8);
        assert_eq!(app.preferences.msaa, 4);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default()).unwrap_or_default();
        assert_eq!(saved["msaa"], 4);
        assert_eq!(saved["backend"], "vulkan", "the backend preference is kept");
        assert_eq!(app.settings_reset, [SettingReset::Msaa { requested: 8, reset: 4, supported: vec![1, 4] }]);
        assert_eq!(
            app.settings_reset[0].message(),
            "Graphics › Anti-aliasing: 8× isn't supported by this graphics device; it is now 4× (supported: 1×, 4×)."
        );
        {
            let _de = crate::i18n::scope("de");
            assert_eq!(
                app.settings_reset[0].message(),
                "Grafik › Kantenglättung: 8× wird von diesem Grafikgerät nicht unterstützt; jetzt 4× (unterstützt: 1×, 4×)."
            );
        }
        // Below the default when the device can't do that either.
        app.settings_reset.clear();
        app.msaa_supported = vec![1, 2];
        check_msaa(&mut app, 8);
        assert_eq!(app.preferences.msaa, 2);
        // Supported, off, or no GPU: nothing changes (OpenGL's 1x policy isn't a reset either).
        for (supported, requested) in [(vec![1, 2, 4, 8], 8), (vec![1, 4], 1), (vec![], 8), (vec![1, 4], 4)] {
            app.settings_reset.clear();
            app.preferences.msaa = requested;
            app.msaa_supported = supported;
            check_msaa(&mut app, requested);
            assert!(app.settings_reset.is_empty());
            assert_eq!(app.preferences.msaa, requested);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn control_channel_rejects_msaa_the_device_cannot_do_and_bad_saved_msaa_loads_as_default() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        app.msaa_supported = vec![1, 4];
        assert!(set(&mut app, &serde_json::json!({ "msaa": 8 })).is_err());
        assert!(set(&mut app, &serde_json::json!({ "msaa": 1 })).is_ok());
        app.load_prefs(&serde_json::json!({ "preferences": { "msaa": 16 } }).to_string());
        assert_eq!(app.preferences.msaa, Prefs::default().msaa);
    }

    #[test]
    fn length_fields_read_bare_numbers_in_design_units_and_honour_explicit_units() {
        let inch = unit_scale("in").0;
        let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-9);
        assert!(close(parse_length_mm("2", inch), 50.8));
        assert!(close(parse_length_mm(" 10 mm ", inch), 10.0));
        assert!(close(parse_length_mm("0.5 in", 1.0), 12.7));
        assert!(close(parse_length_mm("1 in + 5 mm", unit_scale("cm").0), 30.4));
        assert!(close(parse_length_mm("3", unit_scale("cm").0), 30.0));
        assert!(close(parse_length_mm("3", 1.0), 3.0));
        assert_eq!(parse_length_mm("10 deg", inch), None);
        assert_eq!(parse_length_mm("abc", inch), None);
        assert_eq!(parse_length_mm("", inch), None);
        assert_eq!(parse_length_mm("1e400", 1.0), None);
    }

    #[test]
    fn navigator_picker_preserves_axes_and_follows_preferences_transactions() {
        let old: Prefs = serde_json::from_value(json!({"nav":"solidworks"})).unwrap();
        assert_eq!(old.navigator_style, NavigatorStyle::Axes);
        let mut h = crate::scenario::Harness::new();
        assert!(crate::scenario::handle_at("axis_view:z").is_some());
        assert!(crate::scenario::handle_at("navigation_cube:front").is_none());
        let camera = h.app.cam;
        let design = h.app.session.doc.to_json();
        let nav = h.app.preferences.nav.clone();
        h.call("ui.prefs", json!({"open":"Navigation"}));
        let picker = crate::scenario::rect_of("prefs:navigator").unwrap().center();
        h.call("ui.click", json!({"x":picker.x,"y":picker.y}));
        let cube = crate::scenario::rect_of("prefs:navigator:Cube").unwrap().center();
        assert!(crate::scenario::rect_of("prefs:navigator:Signed axes").is_some());
        h.call("ui.click", json!({"x":cube.x,"y":cube.y}));
        assert_eq!(h.app.preferences.navigator_style, NavigatorStyle::Cube);
        assert!(crate::scenario::handle_at("navigation_cube:front").is_some());
        assert_eq!(h.app.preferences.nav, nav);
        assert_eq!(h.app.cam, camera);
        assert_eq!(h.app.session.doc.to_json(), design);
        let mut restored = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        restored.load_prefs(&h.app.prefs());
        assert_eq!(restored.preferences.navigator_style, NavigatorStyle::Cube);
        let before = h.app.preferences.clone();
        assert!(control(&mut h.app, &json!({"navigator_style":"unknown", "nav":"solidworks"})).is_err());
        assert_eq!(h.app.preferences, before, "invalid settings must fail atomically");
        h.call("ui.prefs", json!({"button":"cancel"}));
        assert_eq!(h.app.preferences.navigator_style, NavigatorStyle::Axes);
        assert!(crate::scenario::handle_at("navigation_cube:front").is_none());
        h.call("ui.prefs", json!({"open":"Navigation","navigator_style":"cube"}));
        h.call("ui.prefs", json!({"button":"apply"}));
        h.call("ui.prefs", json!({"button":"defaults"}));
        assert_eq!(h.app.preferences.navigator_style, NavigatorStyle::Axes);
        h.call("ui.prefs", json!({"button":"cancel"}));
        assert_eq!(h.app.preferences.navigator_style, NavigatorStyle::Cube);
        h.call("ui.prefs", json!({"navigator_style":"axes", "look_at_anim":false}));
        h.call("ui.view", json!({"view":"front"}));
        let axis = crate::scenario::handle_at("axis_view:-y").unwrap();
        h.call("ui.click", json!({"x":axis.x,"y":axis.y}));
        assert!(h.app.cam.back().dist(Vec3::Y) < 1e-6, "original axes retain click-again reversal");
        assert_eq!(h.app.session.doc.to_json(), design);
    }

    #[test]
    fn radial_context_menu_defaults_off_and_follows_preferences_transactions() {
        assert!(!Prefs::default().radial_context_menu);
        let old: Prefs = serde_json::from_value(json!({"nav":"solidworks","zoom_reverse":true})).unwrap();
        assert!(!old.radial_context_menu && old.zoom_reverse && old.nav == "solidworks");
        let mut h = crate::scenario::Harness::new();
        h.call("ui.prefs", json!({"open":"Navigation"}));
        h.step(&json!({"widget":"Radial context menu"})).unwrap();
        assert!(h.app.preferences.radial_context_menu, "the actual checkbox enables radial menus");
        let mut restored = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        restored.load_prefs(&h.app.prefs());
        assert!(restored.preferences.radial_context_menu, "enabled preference survives save/load");
        h.call("ui.prefs", json!({"button":"cancel"}));
        assert!(!h.app.preferences.radial_context_menu, "Cancel restores the default-off setting");
        h.call("ui.prefs", json!({"open":"Navigation","radial_context_menu":true}));
        h.call("ui.prefs", json!({"button":"apply"}));
        h.call("ui.prefs", json!({"button":"defaults"}));
        assert!(!h.app.preferences.radial_context_menu, "Navigation defaults disable the ring");
        h.call("ui.prefs", json!({"button":"cancel"}));
        assert!(h.app.preferences.radial_context_menu, "Cancel restores the last Apply");
    }

    #[test]
    fn wheel_smoothing_persists_and_follows_dialog_buttons() {
        assert!(serde_json::from_value::<Prefs>(json!({})).unwrap().smooth_wheel_zoom);
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        control(&mut app, &json!({"open":"Navigation", "smooth_wheel_zoom":false})).unwrap();
        control(&mut app, &json!({"button":"cancel"})).unwrap();
        assert!(app.preferences.smooth_wheel_zoom);
        control(&mut app, &json!({"open":"Navigation", "smooth_wheel_zoom":false})).unwrap();
        control(&mut app, &json!({"button":"apply"})).unwrap();
        control(&mut app, &json!({"smooth_wheel_zoom":true, "button":"cancel"})).unwrap();
        assert!(!app.preferences.smooth_wheel_zoom);
        let mut other = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        other.load_prefs(&app.prefs());
        assert!(!other.preferences.smooth_wheel_zoom);
        control(&mut app, &json!({"open":"Navigation", "button":"defaults"})).unwrap();
        assert!(app.preferences.smooth_wheel_zoom);
        control(&mut app, &json!({"button":"cancel"})).unwrap();
        assert!(!app.preferences.smooth_wheel_zoom);
    }

    #[test]
    fn touchpad_preset_persists_and_obeys_preference_transactions() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        control(&mut app, &json!({"open":"Navigation", "nav":"touchpad"})).unwrap();
        assert_eq!(app.preferences.nav, "touchpad");
        assert!(app.preferences.trackpad_pan, "choosing Touchpad must not overwrite the saved scroll preference");
        control(&mut app, &json!({"button":"cancel"})).unwrap();
        assert_eq!(app.preferences.nav, "fusion");
        control(&mut app, &json!({"open":"Navigation", "nav":"touchpad", "button":"apply"})).unwrap();
        let mut other = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        other.load_prefs(&app.prefs());
        assert_eq!(other.preferences.nav, "touchpad");
        control(&mut app, &json!({"button":"defaults"})).unwrap();
        assert_eq!(app.preferences.nav, "fusion");
        control(&mut app, &json!({"button":"cancel"})).unwrap();
        assert_eq!(app.preferences.nav, "touchpad");
    }

    #[test]
    fn openscad_preset_maps_navigation_and_persists() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        set(&mut app, &json!({"nav": OPENSCAD_NAV})).unwrap();
        for button in [NavButton::Middle, NavButton::Right] {
            assert_eq!(nav_mode(&app, button, Modifiers::default()), Some(NavMode::Pan));
            assert_eq!(nav_mode(&app, button, Modifiers { shift: true, ..Default::default() }), Some(NavMode::Zoom));
        }
        assert_eq!(nav_mode(&app, NavButton::Left, Modifiers::default()), Some(NavMode::Orbit));
        let mut other = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        other.load_prefs(&app.prefs());
        assert_eq!(other.preferences.nav, OPENSCAD_NAV);
    }

    #[test]
    fn trackpad_pan_defaults_on_persists_and_follows_dialog_buttons() {
        assert!(serde_json::from_value::<Prefs>(json!({})).unwrap().trackpad_pan);
        assert!(Prefs::default().trackpad_pan);
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        control(&mut app, &json!({"open":"Navigation", "trackpad_pan":false})).unwrap();
        control(&mut app, &json!({"button":"cancel"})).unwrap();
        assert!(app.preferences.trackpad_pan);
        control(&mut app, &json!({"open":"Navigation", "trackpad_pan":false})).unwrap();
        control(&mut app, &json!({"button":"apply"})).unwrap();
        assert!(!app.preferences.trackpad_pan);
        let mut other = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        other.load_prefs(&app.prefs());
        assert!(!other.preferences.trackpad_pan);
        control(&mut app, &json!({"open":"Navigation", "button":"defaults"})).unwrap();
        assert!(app.preferences.trackpad_pan);
        control(&mut app, &json!({"button":"cancel"})).unwrap();
        assert!(!app.preferences.trackpad_pan);
    }

    #[test]
    fn blender_preset_moves_the_camera_with_the_middle_button_only() {
        // #1090: orbit, pan and zoom all on the middle button; no left or right drag binding, so
        // those stay selection and the context menu (Ctrl+left on empty space orbits in every
        // preset, outside the bindings).
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        set(&mut app, &json!({"nav": "blender"})).unwrap();
        let none = Modifiers::default();
        let shift = Modifiers { shift: true, ..Default::default() };
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(nav_mode(&app, NavButton::Middle, none), Some(NavMode::Orbit));
        assert_eq!(nav_mode(&app, NavButton::Middle, shift), Some(NavMode::Pan));
        assert_eq!(nav_mode(&app, NavButton::Middle, ctrl), Some(NavMode::Zoom));
        for mods in [none, shift, ctrl] {
            assert_eq!(nav_mode(&app, NavButton::Left, mods), None);
            assert_eq!(nav_mode(&app, NavButton::Right, mods), None);
        }
        assert_eq!(zoom_sign(&app), 1.0, "the wheel turns the usual way");
        let mut other = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        other.load_prefs(&app.prefs());
        assert_eq!(other.preferences.nav, "blender");
    }

    #[test]
    fn presets_map_the_mouse_and_prefs_round_trip() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        let shift = Modifiers { shift: true, ..Default::default() };
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        let none = Modifiers::default();
        assert!(matches!(nav_mode(&app, NavButton::Middle, none), Some(NavMode::Pan)));
        assert!(matches!(nav_mode(&app, NavButton::Middle, shift), Some(NavMode::Orbit)));
        set(&mut app, &json!({"nav": "solidworks"})).unwrap();
        assert!(matches!(nav_mode(&app, NavButton::Middle, none), Some(NavMode::Orbit)));
        assert!(matches!(nav_mode(&app, NavButton::Middle, ctrl), Some(NavMode::Pan)));
        set(&mut app, &json!({"nav": "inventor", "zoom_reverse": true})).unwrap();
        assert!(matches!(nav_mode(&app, NavButton::Middle, ctrl), Some(NavMode::Zoom)));
        assert_eq!(zoom_sign(&app), -1.0);
        assert!(matches!(nav_mode(&app, NavButton::Right, none), Some(NavMode::Orbit)), "right drag orbits everywhere");
        assert_eq!(nav_mode(&app, NavButton::Left, none), None, "left drag selects");
        let shift_alt = Modifiers { shift: true, alt: true, ..Default::default() };
        assert_eq!(nav_mode(&app, NavButton::Left, shift_alt), None, "only Alias navigates with the left button");
        set(&mut app, &json!({"nav": "alias"})).unwrap();
        assert_eq!(nav_mode(&app, NavButton::Left, shift_alt), Some(NavMode::Orbit));
        assert_eq!(nav_mode(&app, NavButton::Middle, shift_alt), Some(NavMode::Pan));
        assert_eq!(nav_mode(&app, NavButton::Right, shift_alt), Some(NavMode::Zoom));
        assert_eq!(nav_mode(&app, NavButton::Left, none), None, "a plain left drag still selects");
        assert_eq!(nav_mode(&app, NavButton::Left, shift), None);
        assert_eq!(nav_mode(&app, NavButton::Right, none), Some(NavMode::Orbit));
        assert!(set(&mut app, &json!({"nav": "no-such-preset"})).is_err());
        assert!(set(&mut app, &json!({"default_units": "furlong"})).is_err());
        assert!(set(&mut app, &json!({"nope": 1})).is_err());
        // Kept between runs.
        let mut other = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        other.load_prefs(&app.prefs());
        assert_eq!(other.preferences, app.preferences);
        assert_eq!(format_length(25.4, "in", 3), "1.000 in");
    }

    /// The dialog: changes apply at once, Cancel puts back what was there when it opened, Apply
    /// keeps them, Restore Defaults resets one page, and the shortcuts menus open its page.
    #[test]
    fn preferences_dialog_cancel_apply_and_defaults() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        control(&mut app, &json!({"open": "general"})).unwrap();
        assert!(app.prefs_window.open && app.prefs_window.section == 0);
        control(&mut app, &json!({"theme": "light", "ui_scale": 1.25, "zoom_to_cursor": false})).unwrap();
        assert!(!app.ui.dark, "light applies at once");
        app.keymap.custom.insert("solid.extrude".into(), "Shift+E".into());
        let v = control(&mut app, &json!({"button": "cancel"})).unwrap();
        assert_eq!(v["dialog"], serde_json::Value::Null);
        assert!(app.ui.dark && app.preferences.theme == "dark" && app.preferences.ui_scale == 1.0 && app.preferences.zoom_to_cursor);
        assert!(app.keymap.custom.is_empty(), "a rebinding made while the dialog was open is undone too");

        control(&mut app, &json!({"open": "Navigation", "nav": "alias", "look_at_anim": false})).unwrap();
        control(&mut app, &json!({"button": "apply"})).unwrap();
        control(&mut app, &json!({"nav": "tinkercad"})).unwrap();
        control(&mut app, &json!({"button": "cancel"})).unwrap();
        assert_eq!(app.preferences.nav, "alias", "Cancel goes back to the last Apply");
        let to = {
            let mut c = app.cam;
            c.set_view(solidcraft_engine::render::StandardView::Top);
            c
        };
        app.animate_to(to);
        assert!(app.cam_anim.is_none() && app.cam == to, "no animation: the view jumps");

        control(&mut app, &json!({"open": "Navigation"})).unwrap();
        control(&mut app, &json!({"button": "defaults"})).unwrap();
        assert_eq!(app.preferences.nav, "fusion");
        assert!(app.preferences.look_at_anim);
        control(&mut app, &json!({"button": "ok"})).unwrap();
        assert!(!app.prefs_window.open);

        // File and Help › Keyboard Shortcuts open the same dialog at its page.
        app.keymap.open = true;
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |ui| show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.prefs_window.open && app.prefs_window.section == PAGE_SHORTCUTS && !app.keymap.open);
        assert!(control(&mut app, &json!({"open": "Plugins"})).is_err());
        assert!(set(&mut app, &json!({"up_axis": "x"})).is_err());
        assert!(set(&mut app, &json!({"theme": "sepia"})).is_err());
        assert!(set(&mut app, &json!({"language": "xx"})).is_err());
    }

    #[test]
    fn language_preference_switches_and_persists() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        assert_eq!(app.preferences.language, "en");
        assert_eq!(app.tr("ui.preferences"), "Preferences");
        assert_eq!(app.tr("ui.extrude"), "Extrude");

        // Switch language to Japanese
        set(&mut app, &json!({"language": "ja", "timeline_left": true})).unwrap();
        app.preferences.timeline_left_height = 275.0;
        app.preferences.timeline_left_browser_collapsed = true;
        assert_eq!(app.preferences.language, "ja");
        assert!(app.preferences.timeline_left);
        assert_eq!(app.tr("ui.preferences"), "環境設定");
        assert_eq!(app.tr("ui.extrude"), "押し出し");

        // Preferences round-trip
        let mut other = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        other.load_prefs(&app.prefs());
        assert_eq!(other.preferences.language, "ja");
        assert!(other.preferences.timeline_left);
        assert_eq!(other.preferences.timeline_left_height, 275.0);
        assert!(other.preferences.timeline_left_browser_collapsed);
        assert_eq!(other.tr("ui.preferences"), "環境設定");
    }

    /// A STEP file opened Z-up is offered to be stood up: a quarter turn about X, undoable.
    #[test]
    fn a_y_up_step_file_can_be_stood_up() {
        let dir = std::env::temp_dir().join(format!("solidcraft-standup-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tall_in_y.step").to_string_lossy().to_string();
        let mut src = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        src.run("solid.box", json!({"length": 10, "width": 40, "height": 5})).unwrap();
        src.run("file.export", json!({"path": path})).unwrap();

        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        app.open_path(&path);
        let offered = app.stand_up.clone().unwrap_or_default();
        assert_eq!(offered.len(), 1, "the file's body is offered");
        let size = |app: &SolveApp| solidcraft_engine::view::bounds(&app.session);
        assert!((size(&app).max.y - size(&app).min.y - 40.0).abs() < 1e-6);
        control(&mut app, &json!({"stand_up": true})).unwrap();
        let b = size(&app);
        assert!((b.max.z - b.min.z - 40.0).abs() < 1e-6, "Y became Z: {b:?}");
        assert!(app.stand_up.is_none());
        app.run("edit.undo", json!({})).unwrap();
        assert!((size(&app).max.y - size(&app).min.y - 40.0).abs() < 1e-6, "one undo step");

        // Not offered when turned off, or with Y up as the default orientation.
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        app.preferences.offer_stand_up = false;
        app.open_path(&path);
        assert!(app.stand_up.is_none());
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        app.preferences.up_axis = "y".into();
        app.open_path(&path);
        assert!(app.stand_up.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Orbiting about the world origin keeps the origin where it was drawn, even when Pan has
    /// moved it off to the side.
    #[test]
    fn orbiting_around_the_origin_keeps_it_still() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        app.run("solid.box", json!({"length": 40, "width": 40, "height": 40})).unwrap();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        app.fit_view();
        assert_eq!(app.preferences.orbit_around, OrbitAround::ViewCentre, "the view centre stays the default");
        app.preferences.orbit_around = OrbitAround::Origin;
        app.cam.pan(60.0, -25.0, 600.0);
        let was = (app.cam.yaw, app.cam.pitch);
        let before = crate::viewport::projection(&app, rect).to_screen(Vec3::ZERO).unwrap();
        orbit(&mut app, rect, 40.0, 25.0, None);
        let after = crate::viewport::projection(&app, rect).to_screen(Vec3::ZERO).unwrap();
        assert!(after.distance(before) < 2.0, "{before:?} -> {after:?}");
        assert_ne!((app.cam.yaw, app.cam.pitch), was, "the view must turn");
    }

    /// Preferences saved before the origin option had `orbit_cursor`: that choice survives, and
    /// the control channel still accepts the old key.
    #[test]
    fn saved_cursor_preference_migrates_to_the_origin_option() {
        assert_eq!(lenient(&json!({"orbit_cursor": true})).orbit_around, OrbitAround::Cursor);
        assert_eq!(lenient(&json!({"orbit_cursor": false})).orbit_around, OrbitAround::ViewCentre);
        assert_eq!(lenient(&json!({})).orbit_around, OrbitAround::ViewCentre);
        assert_eq!(lenient(&json!({"orbit_around": "origin"})).orbit_around, OrbitAround::Origin);

        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        set(&mut app, &json!({"orbit_around": "origin"})).unwrap();
        assert_eq!(app.preferences.orbit_around, OrbitAround::Origin);
        set(&mut app, &json!({"orbit_cursor": true})).unwrap();
        assert_eq!(app.preferences.orbit_around, OrbitAround::Cursor, "the old key still moves the choice");
        assert!(!app.preferences.orbit_cursor, "and is folded away");
    }

    #[test]
    fn orbiting_around_the_cursor_keeps_that_point_still() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        app.run("solid.box", json!({"length": 40, "width": 40, "height": 40})).unwrap();
        app.preferences.orbit_around = OrbitAround::Cursor;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        app.fit_view();
        let at = egui::pos2(500.0, 250.0);
        let pivot = pivot_under(&app, &crate::viewport::projection(&app, rect), at);
        orbit(&mut app, rect, 40.0, 25.0, Some(at));
        let s = crate::viewport::projection(&app, rect).to_screen(pivot).unwrap();
        assert!(s.distance(at) < 2.0, "{s:?}");
    }

    /// A drag that starts over empty space turns around the visible geometry's centre, not a
    /// point far off to the side; with nothing visible it turns around the view's centre.
    #[test]
    fn orbiting_from_empty_space_turns_around_the_visible_geometry() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        app.run("solid.box", json!({"length": 40, "width": 40, "height": 40})).unwrap();
        app.preferences.orbit_around = OrbitAround::Cursor;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        app.fit_view();
        let corner = egui::pos2(10.0, 10.0);
        let proj = crate::viewport::projection(&app, rect);
        let hits = crate::viewport::pick(&app, &proj, corner);
        use crate::viewport::Hit;
        assert!(!hits.iter().any(|h| matches!(h, Hit::Face { .. } | Hit::Edge { .. } | Hit::Vertex { .. })), "the corner is empty space");
        let centre = crate::viewport::view_bounds(&app).center();
        orbit(&mut app, rect, 40.0, 25.0, Some(corner));
        assert_eq!(app.viewport.orbit_pivot, Some(centre));

        let mut app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        app.preferences.orbit_around = OrbitAround::Cursor;
        app.cam.target = solidcraft_engine::geom::Vec3::new(5.0, -3.0, 2.0);
        orbit(&mut app, rect, 40.0, 25.0, Some(corner));
        assert_eq!(app.viewport.orbit_pivot, Some(solidcraft_engine::geom::Vec3::new(5.0, -3.0, 2.0)));
    }

    /// The Y up preference turns the view: the camera's up axis follows it, startup included.
    #[test]
    fn up_axis_preference_turns_the_view() {
        use solidcraft_engine::render::UpAxis;
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        let ctx = egui::Context::default();
        apply(&mut app, &ctx);
        assert_eq!(app.cam.up, UpAxis::Z);
        app.preferences.up_axis = "y".into();
        apply(&mut app, &ctx);
        assert_eq!((app.ui.up_axis, app.cam.up), (UpAxis::Y, UpAxis::Y));
        // Home looks at the model with Y up: the eye is above the XZ ground.
        assert!(app.cam.back().y > 0.3, "{:?}", app.cam.back());
        app.preferences.up_axis = "z".into();
        apply(&mut app, &ctx);
        assert_eq!(app.cam.up, UpAxis::Z);
    }
}

#[cfg(test)]
mod ui_scale_regression_tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, pos2};

    #[test]
    fn desktop_zoom_shortcuts_update_and_persist_the_scale_preference() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.key", json!({"key":"Minus","cmd":true}));
        assert!((h.app.preferences.ui_scale - 0.9).abs() < 1e-4);
        assert!((h.ctx.zoom_factor() - 0.9).abs() < 1e-4);
        h.frames(12);
        assert!((h.ctx.zoom_factor() - 0.9).abs() < 1e-4, "must not flash back to the old preference");
        h.step(&json!({"restart":"close"})).unwrap();
        assert!((h.app.preferences.ui_scale - 0.9).abs() < 1e-4);
        h.call("ui.key", json!({"key":"Equals","cmd":true}));
        assert!((h.ctx.zoom_factor() - 1.0).abs() < 1e-4);
        h.call("ui.key", json!({"key":"Plus","cmd":true}));
        assert!((h.ctx.zoom_factor() - 1.1).abs() < 1e-4);
        h.call("ui.key", json!({"key":"0","cmd":true}));
        assert!((h.ctx.zoom_factor() - 1.0).abs() < 1e-4);
        h.call("ui.prefs", json!({"ui_scale":0.5}));
        h.call("ui.key", json!({"key":"Minus","cmd":true}));
        assert_eq!(h.app.preferences.ui_scale, 0.5);
        h.call("ui.prefs", json!({"ui_scale":2.5}));
        h.call("ui.key", json!({"key":"Plus","cmd":true}));
        assert_eq!(h.app.preferences.ui_scale, 2.5);
    }

    #[test]
    fn dragging_the_scale_slider_keeps_its_window_still_until_release() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.prefs", json!({"open":"General"}));
        let window = crate::scenario::rect_of("prefs:window").unwrap();
        let slider = crate::scenario::rect_of("prefs:scale").unwrap();
        let a = pos2(slider.left() + 20.0, slider.center().y);
        let b = pos2(slider.left() + 90.0, slider.center().y);
        h.app.synthetic.push(Event::PointerMoved(a));
        h.app.synthetic.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.frames(3);
        h.app.synthetic.push(Event::PointerMoved(b));
        h.frames(4);
        assert_ne!(h.app.preferences.ui_scale, 1.0, "the slider changes its value during the drag");
        assert_eq!(h.ctx.zoom_factor(), 1.0, "layout must remain stable until release");
        assert_eq!(crate::scenario::rect_of("prefs:window").unwrap(), window);
        h.app.synthetic.push(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.frames(5);
        assert!((h.ctx.zoom_factor() - h.app.preferences.ui_scale).abs() < 1e-4);
        h.call("ui.prefs", json!({"button":"cancel"}));
        assert_eq!(h.app.preferences.ui_scale, 1.0, "Cancel restores the original scale");
        assert_eq!(h.ctx.zoom_factor(), 1.0);
    }
}
