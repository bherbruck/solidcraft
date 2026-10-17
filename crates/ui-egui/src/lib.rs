//! The SolidCraft egui front end.
//!
//! A thin shell over [`solidcraft_engine::Session`]: panels read engine state and act through
//! commands ([`SolveApp::run`]). Nothing here owns design data. The layout follows the familiar
//! parametric-CAD desktop: application bar, a toolbar of workspace tabs and panels, the browser
//! on the left, the 3D viewport with an axis view and navigation bar, the command dialog on the
//! right of the viewport, the command palette and a bottom or left-sidebar feature timeline.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod access;
pub mod agent_chat;
pub mod agent_cursor;
pub mod browser;
pub mod canvas;
#[cfg(test)]
mod colour_tokens_tests;
#[cfg(test)]
mod colour_vision_tests;
pub mod context_menu;
pub mod control;
pub mod crash_ui;
pub mod deferred;
pub mod delete;
pub mod design_properties;
pub mod dialogs;
pub mod dialogs_appearance;
pub mod dialogs_assembly;
pub mod dialogs_coupling;
pub mod dialogs_library;
pub mod dialogs_motion;
pub mod dialogs_move;
pub mod dialogs_parts;
pub mod dialogs_plastic;
pub mod dialogs_save_mesh;
pub mod dialogs_sheet;
pub mod dialogs_surface;
pub mod dialogs_sweep;
mod dialogs_thread;
pub mod dialogs_view_dxf;
pub mod dim_view;
#[cfg(test)]
mod dimension_create_ui_tests;
pub mod documents;
pub mod drag_snap;
pub mod edit_clipboard;
pub mod fonts;
pub mod frame;
pub mod frame_rate;
pub mod gizmo;
pub mod gpu;
pub mod help;
#[cfg(test)]
mod hole_direction_ui_tests;
#[cfg(test)]
mod hole_placement_ui_tests;
pub mod home;
pub mod i18n;
#[cfg(test)]
mod i18n_menu_tests;
#[cfg(test)]
mod i18n_ui_tests;
pub mod icon_svg;
pub mod icons;
pub mod inference;
pub mod install;
pub mod issue_report;
pub mod joint_drag;
pub mod keymap;
mod material_card_review;
#[cfg(test)]
mod measurement_ui_tests;
#[cfg(test)]
mod menu_tests;
#[cfg(test)]
mod surface_clamp_tests;
#[cfg(test)]
mod timeline_marks_tests;
#[cfg(test)]
mod status_tests {
    #[test]
    fn the_status_bar_names_commands_by_label() {
        assert_eq!(super::command_label("solid.fillet"), "Fillet");
        assert_eq!(
            super::friendly_error("solid.shell", "invalid parameters for `solid.shell`: offset: must be positive"),
            "Shell: offset: must be positive"
        );
        assert_eq!(super::friendly_error("edit.undo", "nothing to undo"), "Undo: nothing to undo");
        let mut app = super::SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        app.run("solid.box", serde_json::json!({"length": 5, "width": 5, "height": 5})).unwrap();
        assert_eq!(app.session.log.last().map(String::as_str), Some("Box done"));
        let _ = app.run("edit.redo", serde_json::json!({}));
        assert!(app.session.log.last().is_some_and(|l| l.starts_with("Redo: ") && !l.contains("edit.redo")), "{:?}", app.session.log.last());
    }
}
#[cfg(test)]
mod apply_off_thread_tests;
mod expression_complete;
#[cfg(test)]
mod extrude_edit_tests;
mod file_load;
mod file_picker;
#[cfg(test)]
mod history_off_thread_tests;
pub use file_picker::FilePickerFuture;
#[cfg(test)]
mod blend_arrow_tests;
pub(crate) mod display_lod;
#[cfg(test)]
mod file_load_ui_tests;
#[cfg(test)]
mod fillet_spline_handle_tests;
pub(crate) mod navigation;
pub mod palette;
pub mod params_dialog;
pub(crate) mod pick_buffer;
pub(crate) mod pick_index;
pub mod prefs;
#[cfg(test)]
mod preselect_tests;
pub mod preview;
#[cfg(test)]
mod primitive_units_tests;
pub mod recovery_ui;
pub mod ref_images;
pub mod render_workspace;
#[cfg(test)]
mod revolve_edit_tests;
#[cfg(test)]
mod revolve_manipulator_tests;
pub mod scenario;
pub mod select_other;
pub mod selection;
#[cfg(test)]
mod shell_face_sketch_ui_tests;
pub mod shortcut_box;
pub mod sketch3d;
#[cfg(test)]
mod sketch3d_frame_ui_tests;
pub(crate) mod sketch_attachment_ui;
#[cfg(test)]
mod sketch_attachment_ui_tests;
pub mod sketch_dims;
pub(crate) mod sketch_display;
#[cfg(test)]
mod sketch_display_ui_tests;
#[cfg(test)]
mod sketch_drag_ui_tests;
#[cfg(test)]
mod sketch_edit_tests;
mod sketch_families;
#[cfg(test)]
mod sketch_guidance_ui_tests;
pub mod sketch_modify_ui;
#[cfg(test)]
mod sketch_modify_ui_tests;
pub mod sketch_palette;
mod sketch_pattern_tree;
#[cfg(test)]
mod sketch_side_ui_tests;
pub(crate) mod sketch_text;
mod sketch_text_handles;
mod sketch_text_move;
pub mod sketch_tools;
pub mod smart_extrude;
#[cfg(test)]
mod smart_extrude_ui_tests;
pub mod space_mouse;
pub mod stand_up;
#[cfg(test)]
mod sweep_dialog_tests;
pub mod theme;
#[cfg(test)]
mod thin_extrude_tests;
pub mod timeline;
pub mod timeline_left;
pub mod titlebar;
pub mod toolbar;
pub mod toolbar_custom;
pub mod tools;
pub mod update;
pub mod viewport;
mod viewport_layout;
pub mod workspace;

use std::sync::mpsc::Receiver;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_engine::render::{Camera, CameraAnim, StandardView};

pub use control::{ControlRequest, ControlResponse};

/// UI state that automation can read and set (`ui.set`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    /// Toolbar tab: SOLID, SURFACE, MESH, SHEET METAL, PLASTIC, UTILITIES (SKETCH while sketching).
    pub tab: String,
    pub show_browser: bool,
    pub show_timeline: bool,
    pub show_grid: bool,
    pub show_origin: bool,
    pub perspective: bool,
    pub show_sketches: bool,
    pub palette_open: bool,
    /// Dark theme (the default); a preference kept between runs.
    pub dark: bool,
    /// Selection filter: kinds a click doesn't pick (`vertices`, `edges`, `faces`, `sketch`).
    pub pick_off: Vec<String>,
    /// A click on a face picks its whole body.
    pub pick_bodies: bool,
    /// 0: shaded with edges, 1: shaded, 2: wireframe, 3: shaded with hidden edges.
    pub visual_style: u8,
    /// A soft shadow on the ground under the model (seen from above).
    pub ground_shadow: bool,
    /// The world's up axis (Z or Y; #3): the camera, axis view, home view and ground follow it.
    pub up_axis: solidcraft_engine::render::UpAxis,
    /// Sketches whose closed profiles are not shaded.
    pub hidden_profiles: Vec<u64>,
    /// Finished sketches whose dimensions are shown.
    pub shown_dims: Vec<u64>,
    /// Bodies locked in the browser (no move or delete).
    pub locked_bodies: Vec<String>,
    /// The agent cursor: shown, speed, follow camera (#35).
    pub agent_cursor: agent_cursor::Settings,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            tab: "SOLID".into(),
            show_browser: true,
            show_timeline: true,
            show_grid: true,
            show_origin: true,
            perspective: false,
            show_sketches: true,
            palette_open: false,
            dark: true,
            pick_off: Vec::new(),
            pick_bodies: false,
            visual_style: 0,
            ground_shadow: true,
            up_axis: Default::default(),
            hidden_profiles: Vec::new(),
            shown_dims: Vec::new(),
            locked_bodies: Vec::new(),
            agent_cursor: Default::default(),
        }
    }
}

/// Host services injected by the app so this crate stays portable.
#[derive(Default)]
pub struct Services {
    /// Ask for a file to open. The extensions are the file type the command reads, offered as
    /// the picker's first filter (`&[]`: designs, CAD files and meshes, #1801).
    pub pick_open: Option<Box<dyn Fn(&[&str]) -> FilePickerFuture>>,
    pub pick_save: Option<Box<dyn Fn(&str, &[&str]) -> FilePickerFuture>>,
    /// Open a dialog asking for a program (a slicer); `None` when the host has no such dialog.
    pub pick_program: Option<Box<dyn Fn() -> FilePickerFuture>>,
    pub show_in_folder: Option<Box<dyn Fn(&str)>>,
    /// Where the host reads the anti-aliasing preference at start (a small JSON file).
    pub graphics_path: Option<String>,
    /// HTTPS GET for the update check (`update`); None: never check (browser, tests).
    pub http_get: Option<update::HttpGet>,
    /// HTTPS download to a file, with progress, for one-click updates (`update::install`).
    pub http_download: Option<update::install::HttpDownload>,
    /// The browser's offer to install the web app (`install`); None: no Install button (desktop).
    pub install_app: Option<install::InstallApp>,
    /// HTTP for the agent chat's API agents (OpenAI-compatible endpoints); None: no API agents.
    pub agent_http: Option<agent_chat::backend::HttpFn>,
    /// The system keychain, for API agents' keys; None: keys can't be stored.
    pub secrets: Option<std::sync::Arc<dyn agent_chat::backend::Secrets>>,
}

pub struct SolveApp {
    pub session: Session,
    pub ui: UiState,
    pub cam: Camera,
    /// A view change in progress (axis view, Home, Fit); any mouse navigation cancels it.
    pub cam_anim: Option<CameraAnim>,
    /// Seconds since start, as of this frame.
    pub now: f64,
    /// The view before the current sketch started (Finish Sketch returns to it).
    pub pre_sketch_cam: Option<Camera>,
    /// The history marker before Edit Sketch rolled the timeline to the sketch; Finish Sketch
    /// restores it.
    pub pre_sketch_marker: Option<Option<usize>>,
    pub services: Services,
    file_picker: Option<file_picker::Pending>,
    pub viewport: viewport::ViewportState,
    pub render_workspace: render_workspace::State,
    pub tool: Option<tools::Tool>,
    pub dialog: Option<dialogs::Dialog>,
    /// The Change Parameters window, when open.
    pub params: Option<params_dialog::ParamsDialog>,
    /// The Design Properties window, when open.
    pub design_properties: Option<design_properties::DesignProperties>,
    material_card_review: Option<material_card_review::ReviewWindow>,
    pub palette: palette::Palette,
    /// Context menus, rename box and Properties window.
    pub menu: context_menu::MenuState,
    /// Browser tree state (reveal, groups being edited, occurrence moves).
    pub tree: browser::TreeState,
    /// The S box (command search at the cursor): recent and pinned commands.
    pub sbox: shortcut_box::ShortcutBox,
    /// Keyboard shortcuts the user changed.
    pub keymap: keymap::Keymap,
    /// The Help menu and the About window.
    pub help: help::HelpState,
    /// An editable local issue draft and optional captured app frame.
    pub issue_report: issue_report::ReportState,
    /// The Crash Reports window.
    pub crashes: crash_ui::CrashUi,
    /// The update check and its window (`update`).
    pub updater: update::Updater,
    /// Safari's steps for installing the web app, while their window is open (`install`).
    pub install_steps: Option<install::Steps>,
    /// Toolbar buttons the user pinned, removed or reordered.
    pub toolbar_custom: toolbar_custom::ToolbarCustom,
    pub sketch_choices: sketch_families::Choices,
    /// Preferences kept between runs, and their window.
    pub preferences: prefs::Prefs,
    pub prefs_window: prefs::PrefsWindow,
    /// The slicers the Save as mesh dialog offers.
    pub slicer_menu: dialogs_save_mesh::SlicerMenu,
    pub preview: preview::PreviewState,
    /// OK pressed on a feature that takes a while: its evaluation runs off the window thread.
    pub applying: Option<dialogs::Applying>,
    /// A command recomputing the history (the marker moved, undo) computing off the window
    /// thread, and those asked for meanwhile ([`deferred`]).
    pub deferred: Option<deferred::Deferred>,
    pub deferred_queue: Vec<(String, Value)>,
    /// How long OK waits for a feature before computing it off the window thread, saying so;
    /// `None`: it waits as long as it takes (headless use, tests). The desktop app sets it.
    pub apply_off_thread: Option<std::time::Duration>,
    /// The last command started interactively (id, label), for Repeat.
    pub last_command: Option<(String, String)>,
    /// Esc was already acted on this frame (by the shortcuts), so later handlers skip it.
    pub esc_handled: bool,
    pub status: Option<(String, f64, bool)>,
    pub quit_requested: bool,
    /// Autosave into the recovery folder (the desktop app turns it on).
    pub autosave: Option<solidcraft_engine::recovery::Autosaver>,
    /// Designs left by crashed apps, offered for recovery.
    pub recoverable: Vec<solidcraft_engine::recovery::Entry>,
    /// The bodies of a just-opened Y-up file, offered to be stood up (`stand_up`).
    pub stand_up: Option<Vec<String>>,
    /// The placement of the component that file came in as: the offer turns it.
    pub stand_up_occurrence: Option<u64>,
    /// The anti-aliasing sample counts the graphics device supports (empty without a GPU).
    pub msaa_supported: Vec<u32>,
    /// Settings put back at start because they kept SolidCraft from starting, one line each,
    /// shown until dismissed (`prefs::show_reset_notice`).
    pub settings_reset: Vec<prefs::SettingReset>,
    /// The last command was a big operation: autosave after it.
    pub big_operation: bool,
    /// Autosave interval (minutes; 0 = only after big operations). A preference.
    pub autosave_minutes: f64,
    /// The application bar is the window's title bar (Windows, Linux: no OS decorations).
    pub custom_titlebar: bool,
    /// The OS hit test already offers the resize border ([`titlebar`]; the Windows desktop app),
    /// so no egui resize zones are laid out.
    pub native_resize_border: bool,
    /// macOS: the traffic lights sit over the application bar's left end.
    pub integrated_titlebar: bool,
    /// Build bodies' display meshes on the CPU even where the GPU could (safe mode). Read by
    /// [`SolveApp::set_wgpu`].
    pub cpu_meshing: bool,
    pub frame_ms: f64,
    /// The viewport's frame-rate readout ([`frame_rate`]).
    pub frame_rate: frame_rate::FrameRate,
    pub synthetic: Vec<egui::Event>,
    /// The modifiers of the synthetic press in progress (a Ctrl-drag keeps Ctrl while it moves).
    pub synthetic_mods: egui::Modifiers,
    control_rx: Option<Receiver<ControlRequest>>,
    /// Control requests that change the design, held while a file loads.
    control_held: std::collections::VecDeque<ControlRequest>,
    /// The agent cursor acting out control requests.
    pub agent: agent_cursor::AgentCursor,
    /// The agent chat panel (docs/design/in-app-agent-chat.md).
    pub agent_chat: agent_chat::AgentChat,
    pending_shots: Vec<(u64, Option<String>, std::sync::mpsc::Sender<Value>, f64)>,
    queued_shots: Vec<(u64, f64, u32)>,
    shot_token: u64,
    styled: bool,
    fitted: bool,
    pending_fit: Option<(Camera, solidcraft_engine::geom::Aabb3, bool)>,
    /// The sheet metal flat pattern's own pan and zoom.
    pub flat_view: dialogs_sheet::FlatView,
    /// The start page.
    pub home: home::HomeState,
    pub file_load: file_load::FileLoad,
    /// The other open designs (tabs).
    pub docs: documents::Documents,
    /// The language the installed font fallback chain is ordered for (`fonts::fallback_order`);
    /// changing `preferences.language` re-installs the fonts on the next frame.
    applied_fonts_language: String,
    /// The interface font the fonts were last set up with (`Prefs::ui_font`).
    applied_ui_font: String,
}

impl SolveApp {
    pub fn new(mut session: Session, services: Services) -> Self {
        #[cfg(test)]
        let image_host = Some(solidcraft_engine::render::job::CpuHost::cooperative());
        #[cfg(not(test))]
        let image_host = solidcraft_engine::render::job::CpuHost::for_platform().map_err(|e| log::error!("render image host: {e}")).ok();
        if let Err(e) = session.set_render_job_host(image_host.clone()) {
            log::error!("render image host: {e}");
        }
        let mut render_workspace = render_workspace::State::default();
        render_workspace.image_host = image_host;
        SolveApp {
            session,
            ui: UiState::default(),
            cam: Camera::default(),
            cam_anim: None,
            now: 0.0,
            pre_sketch_cam: None,
            pre_sketch_marker: None,
            services,
            file_picker: None,
            viewport: viewport::ViewportState::default(),
            render_workspace,
            tool: None,
            dialog: None,
            params: None,
            design_properties: None,
            material_card_review: None,
            palette: palette::Palette::default(),
            menu: context_menu::MenuState::default(),
            tree: browser::TreeState::default(),
            sbox: shortcut_box::ShortcutBox::default(),
            keymap: keymap::Keymap::default(),
            help: help::HelpState::default(),
            issue_report: issue_report::ReportState::default(),
            crashes: crash_ui::CrashUi::default(),
            updater: update::Updater::default(),
            install_steps: None,
            toolbar_custom: toolbar_custom::ToolbarCustom::default(),
            sketch_choices: sketch_families::Choices::default(),
            preferences: prefs::Prefs::default(),
            prefs_window: prefs::PrefsWindow::default(),
            slicer_menu: dialogs_save_mesh::SlicerMenu::default(),
            preview: preview::PreviewState::default(),
            applying: None,
            deferred: None,
            deferred_queue: Vec::new(),
            apply_off_thread: None,
            last_command: None,
            esc_handled: false,
            status: None,
            quit_requested: false,
            autosave: None,
            recoverable: Vec::new(),
            stand_up: None,
            stand_up_occurrence: None,
            msaa_supported: Vec::new(),
            settings_reset: Vec::new(),
            big_operation: false,
            autosave_minutes: 5.0,
            custom_titlebar: false,
            native_resize_border: false,
            integrated_titlebar: false,
            cpu_meshing: false,
            frame_ms: 0.0,
            frame_rate: frame_rate::FrameRate::default(),
            synthetic: Vec::new(),
            synthetic_mods: egui::Modifiers::default(),
            control_rx: None,
            agent_chat: Default::default(),
            control_held: Default::default(),
            agent: Default::default(),
            pending_shots: Vec::new(),
            queued_shots: Vec::new(),
            shot_token: 0,
            styled: false,
            fitted: false,
            pending_fit: None,
            flat_view: Default::default(),
            home: home::HomeState::default(),
            file_load: file_load::FileLoad::default(),
            docs: documents::Documents::default(),
            applied_fonts_language: "en".into(),
            applied_ui_font: String::new(),
        }
    }

    /// Translate text using the active language preference.
    pub fn tr<'a>(&self, text: &'a str) -> &'a str {
        i18n::Locale::for_language(&self.preferences.language).text(text)
    }

    /// Translate a display label supplied by the engine, without changing its identifier.
    pub fn tr_label<'a>(&self, text: &'a str) -> &'a str {
        i18n::Locale::for_language(&self.preferences.language).label(text)
    }

    /// Draw the viewport on the GPU (eframe's wgpu render state, with the app's depth and
    /// stencil buffer bits and MSAA sample count). Without it the viewport renders on the CPU.
    pub fn set_wgpu(&mut self, rs: &egui_wgpu::RenderState, depth_bits: u8, stencil_bits: u8, samples: u32) {
        self.viewport.gpu = Some(gpu::install(rs, depth_bits, stencil_bits, samples, !self.cpu_meshing));
        render_workspace::invalidate_scene(self);
        self.render_workspace.host = match render_workspace::host_executor(
            rs.device.limits().max_texture_dimension_2d,
            self.viewport.gpu.is_some_and(|g| g.depth.is_some()),
            samples.max(1),
        ) {
            Ok(executor) => Some(executor),
            Err(error) => {
                self.set_status(error, true);
                None
            }
        };
        render_workspace::sync_backend(self);
        self.msaa_supported = rs.msaa_supported.clone();
        let info = rs.adapter.get_info();
        self.help.gpu = Some(format!("{} ({:?}, {:?})", info.name.trim(), info.backend, info.device_type));
    }

    /// Preferences kept between runs (JSON), for the host to store.
    pub fn prefs(&self) -> String {
        json!({
            "dark": self.ui.dark,
            "grid": self.ui.show_grid,
            "perspective": self.ui.perspective,
            "pick_bodies": self.ui.pick_bodies,
            "auto_project": self.session.auto_project,
            "visual_style": self.ui.visual_style,
            "browser_collapsed": self.tree.collapsed,
            "browser_expanded": self.tree.expanded,
            "recent": self.home.recent,
            "recent_opened_at": self.home.opened_at,
            "shortcut_box": self.sbox.prefs(),
            "shortcuts": self.keymap.prefs(),
            "toolbar": self.toolbar_custom.prefs(),
            "autosave_minutes": self.autosave_minutes,
            "ground_shadow": self.ui.ground_shadow,
            "agent_cursor": self.ui.agent_cursor,
            "preferences": self.preferences,
        })
        .to_string()
    }

    /// Restore preferences saved with [`SolveApp::prefs`].
    pub fn load_prefs(&mut self, prefs: &str) {
        let Ok(v) = serde_json::from_str::<Value>(prefs) else { return };
        let flag = |k: &str| v.get(k).and_then(Value::as_bool);
        self.home.opened_at = v.get("recent_opened_at").and_then(|x| serde_json::from_value(x.clone()).ok()).unwrap_or_default();
        if let Some(d) = flag("dark") {
            self.ui.dark = d;
        }
        if let Some(g) = flag("grid") {
            self.ui.show_grid = g;
        }
        if let Some(p) = flag("perspective") {
            self.ui.perspective = p;
        }
        if let Some(b) = flag("pick_bodies") {
            self.ui.pick_bodies = b;
        }
        if let Some(a) = flag("auto_project") {
            self.session.auto_project = a;
        }
        if let Some(m) = v.get("autosave_minutes").and_then(Value::as_f64).filter(|m| m.is_finite()) {
            self.autosave_minutes = m.clamp(0.0, 600.0);
        }
        if let Some(s) = v.get("visual_style").and_then(Value::as_u64) {
            self.ui.visual_style = u8::try_from(s.min(3)).unwrap_or(0);
        }
        // Browser folders the user opened or closed.
        let keys = |k: &str| -> std::collections::BTreeSet<String> {
            v.get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().take(10_000).filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default()
        };
        self.tree.collapsed = keys("browser_collapsed");
        self.tree.expanded = keys("browser_expanded");
        self.home.recent = v
            .get("recent")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).take(home::MAX_RECENT).map(str::to_string).collect())
            .unwrap_or_default();
        if let Some(s) = v.get("shortcut_box") {
            self.sbox.load(s);
        }
        if let Some(s) = v.get("shortcuts") {
            self.keymap.load(s);
        }
        if let Some(t) = v.get("toolbar") {
            self.toolbar_custom.load(t);
        }
        if let Some(mut p) = v.get("preferences").filter(|p| p.is_object()).map(prefs::lenient) {
            if !prefs::MSAA.contains(&p.msaa) {
                p.msaa = prefs::Prefs::default().msaa;
            }
            self.preferences = p;
        }
        // Saved before the theme was a preference: the Dark/Light flag says which.
        if v.get("preferences").and_then(|p| p.get("theme")).is_none() {
            self.preferences.theme = if self.ui.dark { "dark" } else { "light" }.into();
        }
        if let Some(g) = flag("ground_shadow") {
            self.ui.ground_shadow = g;
        }
        if let Some(a) = v.get("agent_cursor").and_then(|a| serde_json::from_value(a.clone()).ok()) {
            self.ui.agent_cursor = a;
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Run a command programmatically (JSON parameters, no dialogs).
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        if self.file_load.busy() || self.file_picker.is_some() {
            return Err("a file operation is in progress".into());
        }
        let t0 = now_ms();
        let r = self.session.execute(id, &params).map_err(|e| e.to_string());
        self.big_operation |= now_ms() - t0 >= recovery_ui::BIG_OPERATION_MS;
        if let Ok(v) = &r {
            if id == "view.navigate" {
                let to: Camera = serde_json::from_value(v.get("camera").cloned().unwrap_or_default()).map_err(|e| e.to_string())?;
                if v.get("continuous").and_then(Value::as_bool).unwrap_or(false) {
                    self.cancel_view_animation();
                    self.cam = to;
                } else if v.get("fit").and_then(Value::as_bool).unwrap_or(false) {
                    let bounds = solidcraft_engine::view::bounds(&self.session);
                    self.animate_to(self.fitted(to));
                    self.pending_fit = Some((to, bounds, true));
                } else {
                    self.animate_to(to);
                }
            }
            // Curves drawn in construction (or centerline) mode take that line type.
            sketch_palette::apply_mode(self, id, v);
        }
        match &r {
            Ok(v) => {
                // Programmatic callers (the control channel, scripts) run outside a render
                // language scope, so resolve the echo from the saved preference instead of the
                // thread-local scope. The command's own label is translated too, so the echo
                // reads the same way the toolbar button it came from does.
                let language = i18n::Locale::for_language(&self.preferences.language);
                let label = language.label(command_label(id));
                // Frames of a gesture (a continuous view move, a part dragged by its joints) are
                // not echoed one by one.
                if v.get("continuous").and_then(Value::as_bool) != Some(true) {
                    self.session.echo(language.format("ui.command_done", &[("command", label)]));
                }
                if id == "file.new" {
                    // `file.new` starts from millimetres. Units of new designs still apply (#1205).
                    prefs::apply_new_design(self);
                }
                if matches!(id, "file.new" | "doc.open" | "doc.recover") {
                    render_workspace::reset_document(self);
                    self.sketch_choices = sketch_families::Choices::default();
                    dim_view::cancel();
                    self.menu.props = None;
                    sketch_attachment_ui::clear(self);
                    // The old document is already replaced: discard its editor without
                    // applying cancellation hooks (such as restoring its timeline marker).
                    self.dialog = None;
                    self.tool = None;
                    self.preview.reset();
                    self.cam = Camera::default();
                    self.fit_view();
                }
                if matches!(id, "doc.open" | "file.save" | "file.save_as")
                    && let Some(p) = self.session.path.clone()
                {
                    self.home.add_recent(&p);
                    if matches!(id, "file.save" | "file.save_as") {
                        home::remember_session(&mut self.home, &self.session);
                    }
                }
            }
            Err(e) => {
                let msg = friendly_error(id, e);
                self.session.echo(msg.clone());
                self.set_status(msg, true);
            }
        }
        if self.session.active_sketch.is_some() && self.ui.tab != "SKETCH" {
            self.ui.tab = "SKETCH".into();
        } else if self.session.active_sketch.is_none() && self.ui.tab == "SKETCH" {
            self.ui.tab = "SOLID".into();
            self.tool = None;
        }
        r
    }

    /// Start a command the way a toolbar click does: interactive tools and dialogs for commands
    /// that have them, otherwise run it with defaults.
    pub fn start(&mut self, id: &str) {
        // A file is loading: the design waits for it.
        if self.file_load.busy() || self.file_picker.is_some() {
            return;
        }
        // A disabled command starts nothing, however it was reached. The toolbar greys its
        // button, but a shortcut, the command palette and the control channel all land here.
        // Its reason goes to the status line, so the refusal is never silent.
        if let Some(info) = solidcraft_engine::find_command(id).map(|c| c.info(&self.session))
            && let Some(why) = info.disabled_reason
        {
            let label = self.tr_label(info.label).to_string();
            self.set_status(format!("{label}: {why}"), true);
            return;
        }
        if id == "material.properties.validate" {
            self.material_card_review = Some(material_card_review::ReviewWindow::default());
            self.home.open = false;
            return;
        }
        // Another command ends Edit Text (its dialog is not a feature edit to keep).
        sketch_text_move::cancel_drag(self);
        if sketch_text::editing(self) || self.dialog.as_ref().is_some_and(|d| matches!(d.kind, dialogs::Kind::TextMove(_))) {
            dialogs::cancel(self);
        }
        // A component moved without Capture Position: ask first (Capture, Revert or Cancel).
        if browser::needs_capture(self, id) && !(id == "sketch.dimension" && self.session.active_sketch.is_some()) {
            self.tree.capture_prompt = Some(id.to_string());
            return;
        }
        // A line-type toggle changes the current drawing, including a partly picked shape.
        // Handle it before cancelling tools or inline dimensions.
        if id == "sketch.construction" {
            sketch_palette::toggle_construction(self);
            return;
        }
        dim_view::cancel();
        self.tool = None;
        self.home.open = false;
        if let Some(ext) = id.strip_prefix("sketch.insert_").filter(|e| matches!(*e, "dxf" | "svg")) {
            self.pick_open_command(id, json!({}), &[ext]);
            return;
        }
        if id == "component.insert_design" {
            // Another design becomes a linked component; a STEP or mesh file picked here is
            // inserted the way Insert STEP or Insert Mesh would (#1240).
            self.pick_open(|app, path| {
                let Some(path) = path else { return };
                if is_design_path(&path) {
                    if app.run("component.insert_design", json!({ "path": path })).is_ok() {
                        app.fit_view();
                    }
                } else {
                    app.insert_path_async(&path);
                }
            });
            return;
        }
        if matches!(id, "file.insert_step" | "file.insert_mesh") {
            self.pick_open(|app, path| {
                if let Some(path) = path {
                    app.insert_path_async(&path);
                }
            });
            return;
        }
        if !sketch_tools::start_hook(self, id) {
            return;
        }
        if solidcraft_engine::find_command(id).is_some_and(|c| c.info(&self.session).enabled) {
            self.sketch_choices.remember(id);
        }
        if id == "parameters.change" {
            params_dialog::open(self);
            return;
        }
        if matches!(id, "document.metadata" | "document.metadata.set") {
            design_properties::open(self);
            return;
        }
        if id == "sketch.finish" {
            self.finish_sketch();
            return;
        }
        // Press Pull: edges get a fillet, faces and profiles an extrude.
        if id == "solid.press_pull" {
            let edges = self.session.selection.iter().any(|s| matches!(s, solidcraft_engine::Sel::Edge { .. }));
            return self.start(if edges { "solid.fillet" } else { "solid.extrude" });
        }
        if let Some(spec) = solidcraft_engine::find_command(id)
            && (tools::Tool::for_command(id).is_some() || dialogs::Dialog::for_command(self, id).is_some())
            && id != "sketch.create"
        {
            self.last_command = Some((spec.id.to_string(), spec.label.to_string()));
        }
        if let Some(mut t) = tools::Tool::for_command(id) {
            if let Some(params) = tools::constraint_preselection(id, &self.session.selection) {
                if self.run(id, params).is_ok() {
                    let _ = self.session.execute("select.clear", &json!({}));
                    self.set_status("Constraint applied", false);
                }
                return;
            }
            if id == "sketch.project"
                && let Some(refs) = tools::projection_preselection(&self.session.selection)
            {
                if self.run(id, json!({"refs":refs})).is_ok() {
                    let _ = self.session.execute("select.clear", &json!({}));
                    self.set_status("Projection applied", false);
                }
                return;
            }
            // Curve-pair constraints can consume an exact preselection. Leave ambiguous
            // selections to the ordinary pick tool; never silently constrain a subset.
            if matches!(
                id,
                "sketch.constraint.parallel"
                    | "sketch.constraint.perpendicular"
                    | "sketch.constraint.equal"
                    | "sketch.constraint.tangent"
                    | "sketch.constraint.concentric"
                    | "sketch.constraint.collinear"
            ) && let [solidcraft_engine::Sel::SketchCurve { id: a }, solidcraft_engine::Sel::SketchCurve { id: b }] =
                self.session.selection.as_slice()
                && a != b
            {
                let params = json!({ "a": a, "b": b });
                if self.run(id, params).is_ok() {
                    let _ = self.session.execute("select.clear", &json!({}));
                    self.set_status("Constraint applied", false);
                }
                return;
            }
            if id == "sketch.dimension" {
                t.picks = self
                    .session
                    .selection
                    .iter()
                    .filter_map(|s| match s {
                        solidcraft_engine::Sel::SketchCurve { id } | solidcraft_engine::Sel::SketchPoint { id } => Some(id.clone()),
                        _ => None,
                    })
                    .take(2)
                    .collect();
                let _ = self.session.execute("select.clear", &json!({}));
            }
            self.tool = Some(t);
            self.set_status(tools::hint(id), false);
            return;
        }
        if let Some(d) = dialogs::Dialog::for_command(self, id) {
            // The pre-selection now belongs to the dialog's inputs.
            if d.wants_picks() && !self.session.selection.is_empty() {
                let _ = self.session.execute("select.clear", &json!({}));
            }
            self.dialog = Some(d);
            return;
        }
        // Undo and redo can recompute much of the history: off the window thread when slow.
        if matches!(id, "edit.undo" | "edit.redo") {
            let _ = self.run_heavy(id, json!({}));
            return;
        }
        let _ = self.run(id, json!({}));
    }

    /// Edit a feature the way Fusion does: sketches open in sketch mode; other features roll the
    /// timeline back to just before themselves and reopen their dialog, filled in.
    pub fn edit_feature(&mut self, id: u64) {
        use solidcraft_engine::doc::FeatureKind;
        let Some(f) = self.session.doc.feature(id) else { return };
        if matches!(f.kind, FeatureKind::Sketch { .. }) {
            self.edit_sketch(id);
            return;
        }
        if self.session.active_sketch.is_some() {
            self.finish_sketch();
        }
        let Some(idx) = self.session.doc.feature_index(id) else { return };
        let marker = self.session.doc.marker;
        self.tool = None;
        if dialogs::roll_marker(self, Some(idx)).is_err() {
            return;
        }
        match dialogs::for_feature(self, id, marker) {
            Some(d) => self.dialog = Some(d),
            None => {
                let _ = dialogs::roll_marker(self, marker);
                self.palette.text = format!("timeline.edit {{\"feature\": {id}, \"set\": {{}}}}");
                self.ui.palette_open = true;
            }
        }
    }

    /// Edit a sketch the way Fusion does: roll the timeline back to just after it (so later
    /// features don't hide it), turn the camera to face it, and remember both so Finish Sketch
    /// puts them back. Editing the sketch that is already open does nothing; editing another one
    /// finishes the current sketch first, so edit sessions never nest.
    pub fn edit_sketch(&mut self, id: u64) {
        if let solidcraft_engine::doc::FaceAttachmentStatus::Lost { message, .. } =
            self.session.placed_document().sketch_face_attachment(id, &self.session.model)
        {
            browser::redefine_sketch(self, id);
            self.set_status(message, true);
            return;
        }
        if self.session.active_sketch == Some(id) {
            return;
        }
        if self.session.active_sketch.is_some() {
            self.finish_sketch();
        }
        self.tool = None;
        self.dialog = None;
        let Some(idx) = self.session.doc.feature_index(id) else { return };
        let marker = self.session.doc.marker;
        let rolled = marker.is_none_or(|m| m > idx + 1) && self.run("timeline.roll_to", json!({ "position": idx + 1 })).is_ok();
        let before = self.cam;
        if self.run("sketch.edit", json!({ "sketch": id })).is_err() {
            if rolled {
                self.restore_marker(marker);
            }
            return;
        }
        self.pre_sketch_marker = rolled.then_some(marker);
        self.pre_sketch_cam = Some(before);
        dialogs::look_at_sketch(self);
    }

    /// Finish the active sketch: stop editing, restore the history marker Edit Sketch moved and
    /// return to the view from before the sketch.
    pub fn finish_sketch(&mut self) {
        sketch_text_move::cancel_drag(self);
        if sketch_text::editing(self) || self.dialog.as_ref().is_some_and(|d| matches!(d.kind, dialogs::Kind::TextMove(_))) {
            dialogs::cancel(self);
        }
        dim_view::cancel();
        self.tool = None;
        let _ = self.run("sketch.finish", json!({}));
        if let Some(marker) = self.pre_sketch_marker.take() {
            self.restore_marker(marker);
        }
        if let Some(c) = self.pre_sketch_cam.take() {
            self.animate_to(c);
        }
    }

    fn restore_marker(&mut self, marker: Option<usize>) {
        // Rolling forward past the features after the sketch recomputes them (an edited sketch
        // under a text cut): off the window thread when it takes a while.
        let _ = self.run_heavy("timeline.roll_to", marker.map(|m| json!({ "position": m })).unwrap_or_else(|| json!({})));
    }

    /// Everything shown as selected: the selection plus the open dialog's inputs.
    pub fn highlighted(&self) -> Vec<solidcraft_engine::Sel> {
        let mut v = self.session.selection.clone();
        if let Some(d) = &self.dialog {
            v.extend(d.items());
        }
        v.extend(sketch_text::highlighted(self));
        v.extend(sketch_text_move::highlighted(self));
        v
    }

    /// Is the origin widget shown? Origin planes and axes are drawn and pickable. While Create
    /// Sketch picks its plane the Origin is always shown, drawn through the model (a plane
    /// inside a body stays visible), even when the browser hides it.
    pub fn origin_visible(&self) -> bool {
        self.ui.show_origin || self.picking_sketch_plane()
    }

    /// Is Create Sketch open, waiting for its plane?
    pub fn picking_sketch_plane(&self) -> bool {
        self.dialog.as_ref().is_some_and(|d| matches!(d.kind, dialogs::Kind::Sketch))
    }

    /// Is the view drawn in perspective? The perspective preference, except while a sketch is
    /// open: a sketch is drawn and measured on its plane, so the view is parallel (no foreshortening).
    pub fn perspective_view(&self) -> bool {
        self.ui.perspective && self.session.active_sketch.is_none()
    }

    pub fn set_status(&mut self, s: impl Into<String>, error: bool) {
        self.status = Some((s.into(), now_ms(), error));
    }

    pub fn open_path(&mut self, path: &str) {
        self.home.open = false;
        documents::prepare_open(self);
        if let Ok(r) = self.run("doc.open", json!({ "path": path })) {
            self.fit_view();
            stand_up::offer(self, path, &[], Some(&r));
        }
    }

    pub fn open_path_async(&mut self, path: &str) {
        file_load::begin(self, path, false);
    }
    pub fn insert_path_async(&mut self, path: &str) {
        file_load::begin(self, path, true);
    }

    /// Insert a STEP file's bodies (an Import base feature) or a mesh file's meshes into the
    /// current design.
    pub fn insert_path(&mut self, path: &str) {
        let cmd = if solidcraft_engine::io::is_mesh_path(path) { "file.insert_mesh" } else { "file.insert_step" };
        let before: Vec<String> = self.session.world_state().bodies.iter().map(|b| b.name.clone()).collect();
        if let Ok(r) = self.run(cmd, json!({ "path": path })) {
            self.fit_view();
            stand_up::offer(self, path, &before, Some(&r));
            if let Some(w) = r["warnings"].as_array().filter(|w| !w.is_empty()) {
                self.set_status(format!("imported with {} warning(s): {}", w.len(), w.first().and_then(Value::as_str).unwrap_or("")), false);
            }
        }
    }

    /// A dropped file: a STEP or mesh file (3MF, STL, OBJ, glTF) joins a design that has features, anything else
    /// opens.
    pub fn drop_path(&mut self, path: &str) {
        let import = solidcraft_engine::io::is_step_path(path) || solidcraft_engine::io::is_mesh_path(path);
        if import && !self.session.doc.features.is_empty() {
            self.insert_path_async(path);
        } else {
            self.open_path_async(path);
        }
    }

    /// Frame the model at once (programmatic use; the toolbar animates).
    /// Make `up` the world's up axis: the camera turns to the same standard view in that world
    /// (home) and frames the model.
    pub fn set_up_axis(&mut self, up: solidcraft_engine::render::UpAxis) {
        self.ui.up_axis = up;
        self.cam_anim = None;
        self.cam.up = up;
        self.cam.set_view(StandardView::Iso);
        self.fit_view();
    }

    pub fn fit_view(&mut self) {
        if dialogs_sheet::flat_active(self) {
            self.flat_view.fitted = false;
            return;
        }
        if render_workspace::active(self) {
            self.cancel_view_animation();
            match solidcraft_engine::product_scene::extract(&self.session) {
                Ok(scene) if !scene.bounds.is_empty() => self.cam.fit(&scene.bounds),
                Err(e) => self.set_status(e.to_string(), true),
                _ => {}
            }
            return;
        }
        self.cam_anim = None;
        self.viewport.orbit_snap = None;
        self.viewport.orbit_free = None;
        let bounds = viewport::view_bounds(self);
        self.pending_fit = Some((self.cam, bounds, false));
        self.cam = self.fitted(self.cam);
    }

    /// Frame `bounds` at once, keeping the view direction (programmatic use).
    pub fn fit_bounds(&mut self, bounds: solidcraft_engine::geom::Aabb3) {
        self.cancel_view_animation();
        self.viewport.orbit_snap = None;
        self.viewport.orbit_free = None;
        self.pending_fit = Some((self.cam, bounds, false));
        self.cam = self.fitted_bounds(self.cam, &bounds);
    }

    /// `cam` moved to frame the whole model.
    pub fn fitted(&self, mut cam: Camera) -> Camera {
        let b = viewport::view_bounds(self);
        if b.is_empty() {
            cam.target = solidcraft_engine::geom::Vec3::ZERO;
            cam.distance = 150.0;
        } else {
            cam = self.fitted_bounds(cam, &b);
        }
        cam
    }

    fn fitted_bounds(&self, mut cam: Camera, bounds: &solidcraft_engine::geom::Aabb3) -> Camera {
        if let Some(layout) = &self.viewport.layout {
            return viewport_layout::fit_bounds(cam, bounds, layout, self.perspective_view()).unwrap_or(cam);
        }
        let previous = cam;
        cam.fit(bounds);
        if cam.is_valid() { cam } else { previous }
    }

    fn resolve_fit(&mut self, ctx: &egui::Context) {
        viewport::refresh_layout(self, ctx.pixels_per_point());
        if self.viewport.layout.is_some()
            && let Some((cam, bounds, animate)) = self.pending_fit.take()
        {
            let to = if bounds.is_empty() { self.fitted(cam) } else { self.fitted_bounds(cam, &bounds) };
            if animate {
                self.animate_to(to);
            } else {
                self.cam = to;
                self.cam_anim = None;
            }
            ctx.request_repaint();
        }
    }

    /// Move the camera to `to` over half a second (ease in-out, orientation slerped).
    pub fn animate_to(&mut self, to: Camera) {
        self.pending_fit = None;
        self.viewport.orbit_snap = None;
        self.viewport.orbit_free = None;
        if !to.is_valid() {
            return;
        }
        // Preferences › Navigation: animated (at a chosen speed) or immediate.
        if !self.preferences.look_at_anim {
            self.cam = to;
            self.cam_anim = None;
            return;
        }
        let mut anim = CameraAnim::new(self.cam, to, self.now);
        let speed = f64::from(self.preferences.look_at_speed);
        if speed.is_finite() && speed > 0.0 {
            anim.duration /= speed;
        }
        self.cam_anim = Some(anim);
    }

    /// Frame everything shown, keeping the orientation (animated; the selection is ignored).
    pub fn animate_fit_all(&mut self) {
        if dialogs_sheet::flat_active(self) {
            self.animate_view("fit");
            return;
        }
        if render_workspace::active(self) {
            // The Render preview frames its product scene (see `fit_view`).
            self.fit_view();
            return;
        }
        self.pending_fit = None;
        let request = Some((self.cam, viewport::view_bounds(self), true));
        let to = self.fitted(self.cam);
        self.animate_to(to);
        self.pending_fit = request;
    }

    /// Animated view changes for the axis view and navigation bar.
    pub fn animate_view(&mut self, v: &str) {
        // The flat pattern is a 2D view: Fit and Home frame it, and the Design camera stays put.
        if dialogs_sheet::flat_active(self) && matches!(v, "fit" | "home") {
            self.flat_view.fitted = false;
            return;
        }
        if render_workspace::active(self) && matches!(v, "fit" | "home") {
            if v == "home" {
                self.cam.set_view(StandardView::Iso);
            }
            self.fit_view();
            return;
        }
        self.pending_fit = None;
        let mut to = self.cam;
        match v {
            // With something selected, Fit frames the selection.
            "fit" => match viewport::selection_bounds(self) {
                Some(b) => {
                    self.pending_fit = Some((to, b, true));
                    to = self.fitted_bounds(to, &b);
                }
                None => {
                    self.pending_fit = Some((to, viewport::view_bounds(self), true));
                    to = self.fitted(to);
                }
            },
            "home" => {
                to.set_view(StandardView::Iso);
                self.pending_fit = Some((to, viewport::view_bounds(self), true));
                to = self.fitted(to);
            }
            v => match StandardView::parse(v) {
                Some(sv) => to.set_view(sv),
                None => return,
            },
        }
        let request = self.pending_fit.take();
        self.animate_to(to);
        self.pending_fit = request;
    }

    /// Move the viewport to the camera a command asked for (`view.apply`): animated like a view
    /// change, or at once (control requests, so a screenshot right after shows it).
    pub fn follow_view_request(&mut self, animate: bool) {
        let Some(mut to) = self.session.camera.take() else { return };
        if render_workspace::active(self) {
            self.ui.up_axis = to.up;
        } else {
            to.up = self.cam.up;
        }
        if animate {
            self.animate_to(to);
        } else if to.is_valid() {
            self.cancel_view_animation();
            self.viewport.orbit_snap = None;
            self.viewport.orbit_free = None;
            self.cam = to;
        }
    }

    /// Stop any view animation where it is (the user took over the camera).
    pub fn cancel_view_animation(&mut self) {
        self.pending_fit = None;
        self.cam_anim = None;
    }

    /// Hand the view to an external navigation device without a pending mouse orbit or spin
    /// immediately taking it back on the next frame.
    pub fn begin_external_navigation(&mut self) {
        self.cancel_view_animation();
        self.viewport.orbit_snap = None;
        self.viewport.orbit_free = None;
        self.viewport.spin = false;
    }

    fn step_view_animation(&mut self, ctx: &egui::Context) {
        let Some(anim) = self.cam_anim else { return };
        let (cam, done) = anim.sample(self.now);
        self.cam = if cam.is_valid() { cam } else { anim.to };
        if done {
            self.cam_anim = None;
        } else {
            ctx.request_repaint();
        }
    }

    /// Per-frame logic before layout.
    pub fn logic(&mut self, ctx: &egui::Context) {
        file_picker::tick(self, ctx);
        render_workspace::sync_backend(self);
        render_workspace::tick(self, ctx);
        file_load::tick(self, ctx);
        self.esc_handled = false;
        prefs::apply(self, ctx);
        if !self.styled || theme::is_dark() != self.ui.dark || theme::vision() != self.preferences.color_vision {
            theme::set_dark(self.ui.dark);
            theme::set_vision(self.preferences.color_vision);
            theme::apply(ctx);
            self.styled = true;
        }
        if !self.fitted {
            self.fit_view();
            self.fitted = true;
        }
        self.now = ctx.input(|i| i.time);
        if self.cam.up != self.ui.up_axis {
            self.set_up_axis(self.ui.up_axis);
        }
        self.step_view_animation(ctx);
        self.drain_control(ctx);
        self.follow_view_request(true);
        agent_cursor::step(self, ctx);
        if !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
        issue_report::tick(self, ctx);
        self.collect_screenshots(ctx);
        self.issue_screenshots(ctx);
        if self.issue_report.draft.is_none() && self.file_picker.is_none() {
            toolbar::shortcuts(self, ctx);
        }
        recovery_ui::tick(self);
        update::tick(self, ctx);
        if self.issue_report.draft.is_none() && self.file_picker.is_none() {
            for f in ctx.input(|i| i.raw.dropped_files.clone()) {
                let p = f.path().to_string_lossy().to_string();
                if !p.is_empty() {
                    self.drop_path(&p);
                }
            }
        }
    }

    /// Inject synthetic events (one pointer event per frame).
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        if self.synthetic.is_empty() {
            return;
        }
        // Text and modifier prefixes may precede a pointer event. Never drain past that event.
        let n = self
            .synthetic
            .iter()
            .position(|e| {
                matches!(
                    e,
                    egui::Event::PointerMoved(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::PointerGone
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Key { pressed: false, .. }
                )
            })
            .map_or(self.synthetic.len(), |i| i + 1);
        for event in self.synthetic.drain(..n) {
            match &event {
                egui::Event::ModifiersChanged(modifiers) => self.synthetic_mods = *modifiers,
                egui::Event::PointerButton { pos, modifiers, pressed, .. } => {
                    raw.events.push(egui::Event::PointerMoved(*pos));
                    raw.events.push(egui::Event::ModifiersChanged(*modifiers));
                    // A drag keeps its press's modifiers until the release.
                    self.synthetic_mods = if *pressed { *modifiers } else { egui::Modifiers::NONE };
                }
                egui::Event::Key { modifiers, .. } => raw.events.push(egui::Event::ModifiersChanged(*modifiers)),
                egui::Event::PointerMoved(_) => raw.events.push(egui::Event::ModifiersChanged(self.synthetic_mods)),
                _ => {}
            }
            raw.events.push(event);
        }
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        expression_complete::begin_frame(ui.ctx());
        let t0 = now_ms();
        if self.preferences.language != self.applied_fonts_language || self.preferences.ui_font != self.applied_ui_font {
            self.applied_fonts_language.clone_from(&self.preferences.language);
            self.applied_ui_font.clone_from(&self.preferences.ui_font);
            crate::fonts::setup_fonts_for(ui.ctx(), &self.preferences.language, &self.preferences.ui_font);
        }
        let _language = crate::i18n::scope(&self.preferences.language);
        let mut report_input = None;
        issue_report::CadInput::block(self, ui, &mut report_input);
        self.cad_ui(ui, &mut report_input);
        drop(report_input);
        // After everything else, on every page (the start page too, which returns early from
        // `cad_ui`): the window's edges resize it (#1381).
        if self.custom_titlebar && !self.native_resize_border {
            titlebar::resize_zones(ui);
        }
        if self.file_picker.is_none() {
            issue_report::show(self, ui.ctx());
        }
        if !self.home.open {
            agent_cursor::paint(self, ui.ctx());
        }
        if self.file_picker_busy() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.frame_ms = now_ms() - t0;
    }

    fn cad_ui(&mut self, ui: &mut egui::Ui, report_input: &mut Option<issue_report::CadInput>) {
        scenario::clear_handles();
        self.poll_deferred(ui.ctx());
        toolbar::app_bar(self, ui);
        issue_report::CadInput::block(self, ui, report_input);
        file_load::show(self, ui.ctx());
        documents::prompt(self, ui.ctx());
        update::show(self, ui.ctx());
        install::show(self, ui.ctx());
        if self.home.open {
            home::show(self, ui);
            if self.home.open {
                // Preferences and Keyboard Shortcuts open from Home too (its links, the gear, Ctrl+,).
                prefs::show(self, ui.ctx());
                return;
            }
        }
        toolbar::toolbar(self, ui);
        let show_timeline = self.ui.show_timeline && !render_workspace::active(self);
        let left_timeline = show_timeline && self.preferences.timeline_left;
        if show_timeline && !left_timeline {
            timeline::timeline(self, ui);
        }
        palette::palette_bar(self, ui);
        if self.ui.show_browser {
            if left_timeline {
                browser::browser_below_timeline(self, ui);
            } else {
                browser::browser(self, ui);
            }
        } else if left_timeline {
            timeline_left::left_panel(self, ui);
        }
        if render_workspace::active(self) {
            render_workspace::panel(self, ui);
        }
        agent_chat::show(self, ui);
        preview::update(self, ui.ctx());
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            if render_workspace::active(self) {
                render_workspace::show(self, ui);
            } else {
                viewport::show(self, ui);
            }
        });
        dialogs::show(self, ui.ctx());
        issue_report::CadInput::block(self, ui, report_input);
        sketch_palette::show(self, ui.ctx());
        self.resolve_fit(ui.ctx());
        params_dialog::show(self, ui.ctx());
        design_properties::show(self, ui.ctx());
        material_card_review::show(self, ui.ctx());
        recovery_ui::show(self, ui.ctx());
        stand_up::show(self, ui.ctx());
        prefs::show_reset_notice(self, ui.ctx());
        context_menu::show(self, ui.ctx());
        delete::show(self, ui.ctx());
        shortcut_box::show(self, ui.ctx());
        prefs::show(self, ui.ctx());
        toolbar_custom::window(self, ui.ctx());
    }

    fn drain_control(&mut self, ctx: &egui::Context) {
        /// Requests run on this thread, so a flood would starve rendering. The rest stay
        /// queued for the next frame.
        const PER_FRAME: usize = 64;

        /// Held requests beyond this are refused rather than kept.
        const MAX_HELD: usize = 256;

        // Held requests (and the agent chat's, which it hands over here) run even when the app
        // has no control channel of its own.
        if self.control_rx.is_none() && self.control_held.is_empty() {
            return;
        }
        let rx = self.control_rx.take();
        // While the agent cursor acts out a request, later ones wait their turn.
        for _ in 0..PER_FRAME {
            if self.agent.busy() {
                break;
            }
            // Held requests go first, in order, once the file is in.
            let held = if self.file_load.busy() || self.file_picker.is_some() { None } else { self.control_held.pop_front() };
            let Some(req) = held.or_else(|| rx.as_ref().and_then(|rx| rx.try_recv().ok())) else { break };
            // A file is loading: commands that change the design wait for it (the window,
            // inspection and screenshots still answer).
            if ((self.file_load.busy() || self.file_picker.is_some())
                && matches!(req.method.as_str(), "engine.execute" | "command" | "engine.script" | "engine.batch"))
                || (self.file_picker_busy() && matches!(req.method.as_str(), "ui.file" | "ui.home"))
            {
                if self.control_held.len() < MAX_HELD {
                    self.control_held.push_back(req);
                } else {
                    let _ = req.reply.send(json!({"ok": false, "error": "a file operation is in progress"}));
                }
                continue;
            }
            // Opening or importing a file: answered when the file is in, without freezing the window.
            let Some(req) = file_load::control(self, req) else {
                ctx.request_repaint();
                continue;
            };
            let reply = req.reply.clone();
            if agent_cursor::intercept(self, &req.method, &req.params, &reply) {
                ctx.request_repaint();
                continue;
            }
            match control::handle(self, ctx, &req) {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                // A window that is minimised or covered (another macOS Space) presents no frame:
                // say so at once rather than after the deadline (#1677).
                control::Outcome::Screenshot { .. } if window_hidden(ctx) => {
                    let _ = reply.send(json!({"ok": false, "hidden": true, "error": "no frame was presented (window hidden?); use ui.render"}));
                }
                control::Outcome::Screenshot { path } => {
                    self.shot_token += 1;
                    let token = self.shot_token;
                    self.queued_shots.push((token, now_ms() + 150.0, 0));
                    self.pending_shots.push((token, path, reply, now_ms() + 8000.0));
                }
            }
            ctx.request_repaint();
        }
        self.control_rx = rx;
    }

    fn issue_screenshots(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        self.queued_shots.retain_mut(|(token, at, frames)| {
            *frames += 1;
            if now >= *at && *frames >= 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(*token)));
                false
            } else {
                true
            }
        });
        if !self.queued_shots.is_empty() || !self.pending_shots.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn collect_screenshots(&mut self, ctx: &egui::Context) {
        if self.pending_shots.is_empty() && !issue_report::pending(self) {
            return;
        }
        let events: Vec<_> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        let token = user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).copied()?;
                        Some((token, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        for (token, image) in events {
            issue_report::receive(self, token, image.clone());
            if let Some(i) = self.pending_shots.iter().position(|(t, ..)| *t == token) {
                let (_, path, reply, _) = self.pending_shots.remove(i);
                let _ = reply.send(control::save_screenshot(&image, path.as_deref()));
            }
        }
        let now = now_ms();
        // Hidden while waiting: no frame is coming.
        let hidden = window_hidden(ctx);
        self.pending_shots.retain(|(_, _, reply, deadline)| {
            if now < *deadline && !hidden {
                return true;
            }
            let _ = reply.send(json!({"ok": false, "error": "no frame was presented (window hidden?); use ui.render"}));
            false
        });
    }
}

/// The window is minimised or fully covered, so it presents no frames (#1677). Unknown counts as
/// shown: the screenshot deadline still ends a wait for a frame that never comes.
fn window_hidden(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().visible()) == Some(false)
}

/// A command's name as people see it (its label), for the status bar.
pub fn command_label(id: &str) -> &str {
    solidcraft_engine::find_command(id).map_or(id, |c| c.label)
}

/// An engine error with the command's label instead of its id: "Shell: offset must be …".
pub fn friendly_error(id: &str, e: &str) -> String {
    let label = command_label(id);
    let msg = e.replace(&format!("invalid parameters for `{id}`: "), "").replace(&format!("{id}: "), "").replace(&format!("`{id}`"), label);
    if msg.starts_with(label) { msg } else { format!("{label}: {msg}") }
}

pub fn now_ms() -> f64 {
    use web_time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

#[cfg(test)]
mod picker_test_support;

/// A SolidCraft design file (`.solidcraft`, or a design saved as JSON), as opposed to a STEP or
/// mesh file.
fn is_design_path(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "solidcraft" | "solvecraft" | "json")
}
