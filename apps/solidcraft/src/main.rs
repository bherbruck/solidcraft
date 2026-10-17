//! SolidCraft desktop app.
//!
//! Usage: `solidcraft [--control <port>] [--sample] [--crash-log] [--safe-mode] [design.solidcraft | part.step]`
//!
//! `--safe-mode` (or `SOLIDCRAFT_SAFE_MODE=1`) starts with default settings, no recovery offer and
//! the software renderer; see `startup` and docs/book/src/guide/troubleshooting.md.
//!
//! A `.step`/`.stp` file opens as a new design holding the file's bodies (an Import feature); a
//! `.3mf`/`.stl`/`.obj`/`.glb`/`.gltf` file as a new design holding mesh bodies.
//!
//! `--control <port>` (or `SOLIDCRAFT_CONTROL_PORT`) starts a localhost JSON-lines control
//! server: `{"id":1,"method":"engine.execute","params":{"command":"solid.box","params":{…}}}`.
//! See `solidcraft_ui_egui::control` and docs/control-protocol.md.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![deny(unsafe_code)]

#[cfg(not(target_os = "macos"))]
#[path = "../../../assets/icons/app/shared/source/app_icon.rs"]
mod app_icon;
mod control_server;
#[cfg(target_os = "windows")]
mod native_crash;
#[cfg(target_os = "windows")]
mod native_frame;
#[cfg(target_os = "macos")]
mod open_documents;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod space_mouse;
mod startup;

use solidcraft_engine::{Session, io};
use solidcraft_ui_egui::{Services, SolveApp};

struct App {
    solve: SolveApp,
    /// The settings folder whose start marker the first frame clears (`startup::started`).
    starting: Option<std::path::PathBuf>,
    /// Frames begun; the second one means the first was drawn (startup is over).
    frames: u32,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    space_mouse: space_mouse::SpaceMouse,
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.frames < 2 {
            self.frames += 1;
            // A second frame right away, which means the first was presented: the app is up.
            ctx.request_repaint();
            if self.frames == 2 {
                solidcraft_engine::crash::phase(solidcraft_engine::crash::RUNNING);
                if let Some(dir) = self.starting.take() {
                    startup::started(&dir);
                }
            }
        }
        // The design on screen is the one whose commands a crash report replays.
        self.solve.session.crash_journal = solidcraft_engine::crash::enabled();
        // The OS closing the window (Alt+F4, Cmd+Q, the window manager): ask about unsaved
        // designs first; the window closes once they are answered (`quit_requested`).
        if ctx.input(|i| i.viewport().close_requested()) && !self.solve.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            solidcraft_ui_egui::documents::request_quit(&mut self.solve);
        }
        #[cfg(target_os = "macos")]
        for f in if self.solve.file_picker_busy() { Vec::new() } else { open_documents::take() } {
            self.solve.open_path_async(&f);
        }
        self.solve.logic(ctx);
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if !self.solve.file_picker_busy() {
            self.space_mouse.tick(&mut self.solve, ctx);
        }
        if self.solve.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Before the app's own hook, which adds the control channel's events.
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        self.space_mouse.raw_input_hook(self.solve.preferences.space_mouse_enabled, raw);
        self.solve.raw_input_hook(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.solve.ui(ui);
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(solidcraft_ui_egui::prefs::STORAGE_KEY, self.solve.prefs());
    }
    fn on_exit(&mut self) {
        // The agent chat's agent and everything it started end with the app.
        solidcraft_ui_egui::agent_chat::shutdown(&mut self.solve);
        solidcraft_ui_egui::recovery_ui::close(&mut self.solve);
        solidcraft_engine::crash::end_session();
        // An update asked to restart into the new version or to run its installer.
        if let Some((program, args)) = solidcraft_ui_egui::update::take_launch(&mut self.solve) {
            let mut command = std::process::Command::new(&program);
            command.args(&args);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // Not tied to this process: its own process group, and no console (the PowerShell
                // that waits for an installer is hidden; the app itself is detached).
                let hidden_shell = program.file_stem().is_some_and(|n| n.eq_ignore_ascii_case("powershell"));
                command.creation_flags(0x0000_0200 | if hidden_shell { 0x0800_0000 } else { 0x0000_0008 });
            }
            if let Err(e) = command.spawn() {
                log::error!("update: starting {}: {e}", program.display());
            }
        }
    }
}

fn services(cc: &eframe::CreationContext<'_>) -> Services {
    let parent = rfd::AsyncFileDialog::new().set_parent(cc);
    let open_parent = parent.clone();
    let program_parent = parent.clone();
    Services {
        pick_open: Some(Box::new(move |first: &[&str]| {
            // The file type the command reads comes first, so the picker opens on it (#1801).
            let mut dialog = open_parent.clone();
            if !first.is_empty() {
                let label = first.iter().map(|e| e.to_uppercase()).collect::<Vec<_>>().join(", ");
                let both: Vec<String> = first.iter().flat_map(|e| [e.to_lowercase(), e.to_uppercase()]).collect();
                dialog = dialog.add_filter(label, &both);
            }
            let future = dialog
                .add_filter(
                    "Designs, CAD files and meshes",
                    &[
                        io::DESIGN_EXT,
                        io::LEGACY_DESIGN_EXT,
                        "json",
                        "step",
                        "stp",
                        "STEP",
                        "STP",
                        "igs",
                        "iges",
                        "IGS",
                        "IGES",
                        "FCStd",
                        "fcstd",
                        "3dm",
                        "3DM",
                        "3mf",
                        "3MF",
                        "stl",
                        "STL",
                        "obj",
                        "OBJ",
                        "glb",
                        "GLB",
                        "gltf",
                        "GLTF",
                    ],
                )
                .add_filter("SolidCraft design (container or JSON)", &[io::DESIGN_EXT, io::LEGACY_DESIGN_EXT, "json"])
                .add_filter("STEP", &["step", "stp", "STEP", "STP"])
                .add_filter("IGES", &["igs", "iges", "IGS", "IGES"])
                .add_filter("FreeCAD and Rhino", &["FCStd", "fcstd", "3dm", "3DM"])
                .add_filter("Mesh (3MF, STL, OBJ, glTF)", &["3mf", "3MF", "stl", "STL", "obj", "OBJ", "glb", "GLB", "gltf", "GLTF"])
                .add_filter("Sketch drawings (DXF, SVG)", &["dxf", "DXF", "svg", "SVG"])
                .add_filter("All files", &["*"])
                .pick_file();
            Box::pin(async move { future.await.map(|p| p.path().to_string_lossy().into_owned()) })
        })),
        pick_program: Some(Box::new(move || {
            let dialog = program_parent.clone().set_title("Choose your slicer");
            let dialog = if cfg!(target_os = "windows") { dialog.add_filter("Programs", &["exe"]) } else { dialog };
            let future = dialog.pick_file();
            Box::pin(async move { future.await.map(|p| p.path().to_string_lossy().into_owned()) })
        })),
        pick_save: Some(Box::new(move |name: &str, exts: &[&str]| {
            let dialog = parent.clone().set_file_name(name);
            let dialog = if exts == [io::DESIGN_EXT, "json"] {
                if name.ends_with(".json") {
                    dialog.add_filter("Plain JSON design", &["json"]).add_filter("SolidCraft container", &[io::DESIGN_EXT])
                } else {
                    dialog.add_filter("SolidCraft container", &[io::DESIGN_EXT]).add_filter("Plain JSON design", &["json"])
                }
            } else {
                dialog.add_filter("File", exts)
            };
            let future = dialog.save_file();
            Box::pin(async move { future.await.map(|p| p.path().to_string_lossy().into_owned()) })
        })),
        graphics_path: graphics_path().map(|p| p.to_string_lossy().to_string()),
        http_get: Some(std::sync::Arc::new(http_get)),
        http_download: Some(std::sync::Arc::new(http_download)),
        // Already an installed app: only the browser build offers to install.
        install_app: None,
        agent_http: Some(std::sync::Arc::new(agent_http)),
        secrets: Some(std::sync::Arc::new(Keychain)),
        show_in_folder: Some(Box::new(|path| {
            let folder = std::path::Path::new(path).parent().unwrap_or_else(|| std::path::Path::new("."));
            let mut command = std::process::Command::new(if cfg!(target_os = "windows") {
                "explorer"
            } else if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            });
            command.arg(folder);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            let _ = command.spawn();
        })),
    }
}

/// HTTPS GET for the update check (docs/updates.md): certificates from the operating system, a
/// 15 s limit, at most 1 MB, and no headers beyond what GitHub requires (a User-Agent without
/// the version or anything about this machine, and the API's media type).
fn http_get(url: &str) -> Result<Vec<u8>, solidcraft_ui_egui::update::FetchError> {
    use solidcraft_ui_egui::update::FetchError;
    let mut response = http_call(url, std::time::Duration::from_secs(15))?;
    response.body_mut().with_config().limit(1 << 20).read_to_vec().map_err(|e| FetchError::Failed(e.to_string()))
}

/// HTTPS download of an update package to `to` (docs/updates.md), reporting progress; stops when
/// `progress` returns false. The caller verifies the file.
fn http_download(url: &str, to: &std::path::Path, progress: &dyn Fn(u64, Option<u64>) -> bool) -> Result<(), solidcraft_ui_egui::update::FetchError> {
    use solidcraft_ui_egui::update::FetchError;
    use std::io::{Read, Write};
    let fail = |e: String| FetchError::Failed(e);
    let mut response = http_call(url, std::time::Duration::from_secs(30 * 60))?;
    let total = response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let mut body = response.body_mut().with_config().limit(solidcraft_ui_egui::update::install::MAX_PACKAGE).reader();
    let mut file = std::fs::File::create(to).map_err(|e| fail(format!("{}: {e}", to.display())))?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| fail(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(buf.get(..n).unwrap_or_default()).map_err(|e| fail(format!("{}: {e}", to.display())))?;
        done += n as u64;
        if !progress(done, total) {
            return Err(fail("cancelled".into()));
        }
    }
    file.sync_all().map_err(|e| fail(format!("{}: {e}", to.display())))?;
    Ok(())
}

/// HTTP for the agent chat's API agents (OpenAI-compatible endpoints, docs/agent-chat.md): to the
/// URL the user configured, which may be plain HTTP on this computer (Ollama, LM Studio). No
/// redirects (a key must not follow one elsewhere), at most 16 MB back.
fn agent_http(r: &solidcraft_ui_egui::agent_chat::backend::HttpRequest) -> Result<(u16, Vec<u8>), String> {
    use std::time::Duration;
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(r.timeout_s.clamp(1, 900))))
        .timeout_connect(Some(Duration::from_secs(10)))
        .user_agent("SolidCraft")
        .max_redirects(0)
        .build()
        .into();
    let mut response = if r.method == "POST" {
        let mut req = agent.post(&r.url);
        for (k, v) in &r.headers {
            req = req.header(k, v);
        }
        req.send(r.body.as_deref().unwrap_or_default())
    } else {
        let mut req = agent.get(&r.url);
        for (k, v) in &r.headers {
            req = req.header(k, v);
        }
        req.call()
    }
    .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let body = response.body_mut().with_config().limit(16 << 20).read_to_vec().map_err(|e| e.to_string())?;
    Ok((status, body))
}

/// API keys in the system keychain (service "SolidCraft agent chat", one entry per agent).
struct Keychain;

impl Keychain {
    fn entry(id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new("SolidCraft agent chat", id).map_err(|e| e.to_string())
    }
}

impl solidcraft_ui_egui::agent_chat::backend::Secrets for Keychain {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match Self::entry(id)?.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), String> {
        Self::entry(id)?.set_password(secret).map_err(|e| e.to_string())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// One GET to GitHub: a 200 answer, or why not (rate limits included).
fn http_call(url: &str, timeout: std::time::Duration) -> Result<ureq::http::Response<ureq::Body>, solidcraft_ui_egui::update::FetchError> {
    use solidcraft_ui_egui::update::FetchError;
    use std::time::Duration;
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .https_only(true)
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .timeout_connect(Some(Duration::from_secs(5)))
        .user_agent("SolidCraft-update-check")
        .max_redirects(3)
        .build()
        .into();
    let response = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| FetchError::Failed(e.to_string()))?;
    let status = response.status().as_u16();
    let header = |k: &str| response.headers().get(k).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
    if status == 429 || (status == 403 && (header("x-ratelimit-remaining") == Some(0) || header("retry-after").is_some())) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let retry_after_s = header("retry-after").or_else(|| header("x-ratelimit-reset").map(|reset| reset.saturating_sub(now))).unwrap_or(3600);
        return Err(FetchError::RateLimited { retry_after_s });
    }
    if status != 200 {
        return Err(FetchError::Failed(format!("GitHub answered HTTP {status}")));
    }
    Ok(response)
}

/// The per-user settings folder: eframe's storage (`app.ron`) and the anti-aliasing preference.
fn settings_dir() -> Option<std::path::PathBuf> {
    eframe::storage_dir("SolidCraft")
}

/// The anti-aliasing preference, kept next to eframe's storage (it is needed before the
/// window opens).
fn graphics_path() -> Option<std::path::PathBuf> {
    settings_dir().map(|d| d.join("graphics.json"))
}

/// Compatibility shim (docs/compatibility.md): on the first launch after the rename, copy the
/// settings of earlier releases into the new folder. They kept `graphics.json` (and, on Linux
/// and macOS, the recovery folder) under the old product name, and eframe's `app.ron` under the
/// window's old app id. In the Flatpak sandbox, releases before 0.8 kept everything under the
/// old app id's `~/.var/app` folder; that copy comes first, as it is the newest. The old folders
/// are never changed.
fn adopt_legacy_settings() {
    let (Some(current), Some(legacy)) = (settings_dir(), eframe::storage_dir("SolveCraft")) else { return };
    let legacy_eframe = eframe::storage_dir(solidcraft_engine::io::LEGACY_APP_ID);
    let old_flatpak = solidcraft_engine::io::legacy_flatpak_path(&current);
    if let Err(e) = adopt_settings(&current, old_flatpak.as_deref(), &legacy, legacy_eframe.as_deref()) {
        log::warn!("settings: could not copy the previous release's settings into {}: {e}", current.display());
    }
}

fn adopt_settings(
    current: &std::path::Path,
    old_flatpak: Option<&std::path::Path>,
    legacy: &std::path::Path,
    legacy_eframe: Option<&std::path::Path>,
) -> std::io::Result<()> {
    let fresh = !current.exists();
    if let Some(old) = old_flatpak {
        solidcraft_engine::io::adopt_legacy_dir(current, old)?;
    }
    solidcraft_engine::io::adopt_legacy_dir(current, legacy)?;
    if let Some(old) = legacy_eframe.map(|d| d.join("app.ron")).filter(|p| fresh && p.is_file()) {
        std::fs::create_dir_all(current)?;
        let new = current.join("app.ron");
        if !new.exists() {
            std::fs::copy(old, new)?;
        }
    }
    Ok(())
}

/// `graphics.json`, read before the window opens.
fn graphics() -> Option<serde_json::Value> {
    graphics_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
}

/// The graphics backend preference (`graphics.json`'s `backend`: `auto`, `dx12`, `vulkan`, `gl`).
fn backend_preference() -> Option<String> {
    graphics().and_then(|v| v.get("backend").and_then(serde_json::Value::as_str).map(str::to_string))
}

/// Anti-aliasing samples from Preferences (4 when unset).
fn msaa() -> u16 {
    graphics()
        .and_then(|v| v.get("msaa").and_then(serde_json::Value::as_u64))
        .and_then(|m| u16::try_from(m).ok())
        .filter(|m| [1, 2, 4, 8].contains(m))
        .unwrap_or(MSAA)
}

/// Windows and Linux: no OS title bar; the application bar is the title bar
/// (`solidcraft_ui_egui::titlebar`). macOS keeps its traffic lights over the integrated bar.
const CUSTOM_TITLEBAR: bool = !cfg!(target_os = "macos");

const DEPTH_BITS: u8 = 24;
/// A 32-bit float depth buffer, drawn with reversed depth (1 near, 0 far) so its precision follows
/// the distance from the eye (#1434). Only where every adapter offers it with a stencil
/// ([`startup::float_depth_everywhere`]); else [`DEPTH_BITS`]. Not in safe mode.
const FLOAT_DEPTH_BITS: u8 = 32;
/// The section cap marks where it shows in the stencil.
const STENCIL_BITS: u8 = 8;
const MSAA: u16 = 4;

fn app_viewport() -> Result<egui::ViewportBuilder, eframe::Error> {
    // Let macOS use the bundle's layered icon; eframe's default bitmap would replace it.
    #[cfg(target_os = "macos")]
    let icon = egui::IconData::default();
    #[cfg(not(target_os = "macos"))]
    let icon =
        egui::IconData { rgba: app_icon::rgba(64).map_err(|e| eframe::Error::AppCreation(std::io::Error::other(e).into()))?, width: 64, height: 64 };
    Ok(egui::ViewportBuilder::default()
        .with_title("SolidCraft")
        .with_icon(icon)
        .with_inner_size([1600.0, 1000.0])
        .with_min_inner_size([960.0, 600.0])
        .with_drag_and_drop(true)
        .with_decorations(!CUSTOM_TITLEBAR)
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_app_id(APP_ID))
}

/// The window's app id: it names the desktop entry, so it is the packages' reverse-DNS id.
const APP_ID: &str = solidcraft_engine::io::APP_ID;

fn main() -> eframe::Result {
    adopt_legacy_settings();
    let mut safe_mode = startup::env_on(std::env::var(startup::SAFE_MODE_ENV).ok().as_deref());
    let mut control_port: Option<u16> = solidcraft_engine::geom::env::var("CONTROL_PORT").and_then(|p| p.parse().ok());
    let mut files = Vec::new();
    let mut sample = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--control" => control_port = args.next().and_then(|p| p.parse().ok()),
            "--sample" => sample = true,
            "--safe-mode" => safe_mode = true,
            "--version" | "-V" => {
                println!("solidcraft {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--crash-log" => {
                match solidcraft_engine::crash::default_dir() {
                    Some(dir) => {
                        println!("crash reports: {}", dir.display());
                        for r in solidcraft_engine::crash::list(&dir).iter().take(5) {
                            println!("  {}  {}  {}", r.id, r.kind, r.message);
                        }
                        println!("more: solidcraft-cli crashes");
                    }
                    None => println!("no crash folder on this system"),
                }
                return Ok(());
            }
            "--help" | "-h" => {
                println!("usage: solidcraft [--control PORT] [--sample] [--crash-log] [--safe-mode] [design.solidcraft | part.step]");
                return Ok(());
            }
            _ => files.push(a),
        }
    }
    // Settings that don't read are moved aside; launches that never got a window are counted,
    // and after a few in a row this one starts in safe mode (docs/book/src/guide/troubleshooting.md).
    let settings = settings_dir();
    let mut notes: Vec<String> = Vec::new();
    let mut failed_starts = 0;
    if let Some(dir) = &settings {
        notes.extend(startup::check_settings(dir, &[solidcraft_ui_egui::prefs::STORAGE_KEY, solidcraft_ui_egui::prefs::LEGACY_STORAGE_KEY]));
        failed_starts = startup::begin(dir);
    }
    if failed_starts >= startup::FAILED_STARTS_FOR_SAFE_MODE && !safe_mode {
        safe_mode = true;
        notes.push(format!("The last {failed_starts} launches stopped before the window opened, so this one is in safe mode."));
    }
    let cpu_renderer = safe_mode || startup::cpu_renderer_env(std::env::var(startup::RENDERER_ENV).ok().as_deref());
    if safe_mode {
        notes.push("Safe mode: default settings (yours are kept for the next normal start), no recovery offer, software rendering.".into());
    }
    let samples = if safe_mode { MSAA } else { msaa() };
    let wgpu_options = wgpu_options(cpu_renderer, if safe_mode { None } else { backend_preference() }.as_deref());
    let depth_bits = if !safe_mode && startup::float_depth_everywhere(&wgpu_options) { FLOAT_DEPTH_BITS } else { DEPTH_BITS };
    // Crashes leave a report in the crash folder (solidcraft_engine::crash, docs/crash-reports.md).
    let crashed = solidcraft_engine::crash::default_dir()
        .map(|dir| {
            let build = solidcraft_engine::crash::Build {
                version: solidcraft_ui_egui::help::VERSION,
                commit: solidcraft_ui_egui::help::COMMIT,
                built: solidcraft_ui_egui::help::BUILD_DATE,
            };
            solidcraft_engine::crash::install(dir, build)
        })
        .unwrap_or_default();
    // A crash in native code (a graphics driver) says where it happened: the exception and the
    // module on Windows, and on every system the startup step the app was in.
    #[cfg(target_os = "windows")]
    native_crash::install();
    eframe::egui_wgpu::set_startup_hook(startup_step);
    solidcraft_engine::crash::phase("window");
    let options = eframe::NativeOptions {
        viewport: app_viewport()?,
        depth_buffer: depth_bits,
        stencil_buffer: STENCIL_BITS,
        multisampling: samples,
        // Kept with the other settings, not in a folder named after the app id. Safe mode reads
        // and writes a scratch copy, leaving the user's settings as they were.
        persistence_path: if safe_mode {
            let scratch = std::env::temp_dir().join(format!("solidcraft-safe-mode-{}.ron", std::process::id()));
            let _ = std::fs::remove_file(&scratch);
            Some(scratch)
        } else {
            settings.as_ref().map(|d| d.join("app.ron"))
        },
        persist_window: !safe_mode,
        wgpu_options,
        ..Default::default()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut options = options;
    #[cfg(all(unix, not(target_os = "macos")))]
    if prefer_x11(
        std::env::var_os("DISPLAY").is_some(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        solidcraft_engine::geom::env::var("WAYLAND").as_deref(),
    ) {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            b.with_x11();
        }));
    }
    // Files opened from Finder and the Dock arrive as events, not arguments.
    #[cfg(target_os = "macos")]
    open_documents::install();
    let created = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let created_in_app = created.clone();
    let starting = settings.clone();
    let result = eframe::run_native(
        "SolidCraft",
        options,
        Box::new(move |cc| {
            created_in_app.store(true, std::sync::atomic::Ordering::SeqCst);
            solidcraft_engine::crash::phase("app setup");
            solidcraft_ui_egui::fonts::setup_fonts(&cc.egui_ctx);
            let mut app = SolveApp::new(Session::default(), services(cc));
            app.apply_off_thread = Some(solidcraft_ui_egui::dialogs::APPLY_AT_ONCE);
            app.custom_titlebar = CUSTOM_TITLEBAR;
            // Windows: the OS hit test offers the resize border, at the window's DPI, from the
            // first message (#1381).
            #[cfg(target_os = "windows")]
            {
                app.native_resize_border = CUSTOM_TITLEBAR && native_frame::install(cc);
                if CUSTOM_TITLEBAR && !app.native_resize_border {
                    eprintln!("SolidCraft window: no native resize border; egui edge zones instead");
                }
            }
            app.integrated_titlebar = cfg!(target_os = "macos");
            app.cpu_meshing = safe_mode;
            if let Some(p) = cc.storage.and_then(|s| solidcraft_ui_egui::prefs::stored(|k| s.get_string(k))).filter(|_| !safe_mode) {
                app.load_prefs(&p);
            }
            if let Some(rs) = &cc.wgpu_render_state {
                eprintln!("SolidCraft graphics: {:?}, MSAA {}x (requested {}x)", rs.adapter.get_info().backend, rs.msaa_samples, samples);
                solidcraft_engine::crash::phase("viewport pipelines (shader compile)");
                app.set_wgpu(rs, depth_bits, STENCIL_BITS, rs.msaa_samples);
                solidcraft_ui_egui::gpu::install_picture_renderer(rs);
                // The graphics driver is loaded now; it may have put its own exception filter
                // in front of ours.
                #[cfg(target_os = "windows")]
                native_crash::reinstall();
                // A saved anti-aliasing the device can't do: back to the default, with a notice.
                solidcraft_ui_egui::prefs::check_msaa(&mut app, u32::from(samples));
                let info = rs.adapter.get_info();
                solidcraft_engine::crash::set_environment(&format!(
                    "Renderer: {:?} {} ({}), MSAA {}x",
                    info.backend, info.name, info.driver, rs.msaa_samples
                ));
            }
            if let Some(port) = control_port {
                let rx = control_server::start(port, cc.egui_ctx.clone());
                app = app.with_control(rx);
            }
            // The agent chat's MCP bridge reaches the app through an endpoint of its own, started
            // on a free port the first time the chat connects (docs/agent-chat.md).
            let ctx = cc.egui_ctx.clone();
            app.agent_chat.control_starter = Some(Box::new(move |tx| control_server::listen_agent(tx, ctx.clone())));
            if sample {
                if let Err(e) = app.session.run_script(&solidcraft_engine::sample::script()) {
                    app.set_status(format!("sample: {e}"), true);
                }
                app.session.doc_mut().name = "Sample Plate".into();
                app.session.mark_saved();
                app.session.undo.clear();
            }
            // A launch from Finder: its files arrived before the window.
            #[cfg(target_os = "macos")]
            let files = {
                open_documents::set_ui(&cc.egui_ctx);
                [files, open_documents::take()].concat()
            };
            for f in &files {
                app.open_path_async(f);
            }
            // No file and no sample: the empty design takes Units of new designs (#1205).
            // An opened file and the sample part keep the units they were saved or built with.
            if files.is_empty() && !sample {
                solidcraft_ui_egui::prefs::apply_new_design(&mut app);
            }
            // No design given: the start page (not when driven over the control channel, whose
            // clients expect the design view; `ui.home` shows it there).
            if files.is_empty() && !sample && control_port.is_none() {
                let rev = app.session.revision;
                app.home.show(rev);
            }
            // Autosave, and offer what crashed sessions left behind.
            if let Some(dir) = solidcraft_engine::recovery::default_dir() {
                let every = if app.autosave_minutes > 0.0 { app.autosave_minutes * 60.0 } else { f64::MAX / 4.0 };
                solidcraft_ui_egui::recovery_ui::start(&mut app, &dir, std::time::Duration::from_secs_f64(every.min(1e9)));
                // Safe mode keeps autosaving but offers nothing: the entries wait for a normal start.
                if safe_mode {
                    app.recoverable.clear();
                }
            }
            // Tell the user about crashes since the last start (after the recovery offer is set up).
            solidcraft_ui_egui::crash_ui::announce(&mut app, &crashed);
            if !notes.is_empty() {
                app.set_status(notes.join(" "), true);
            }
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            let space_mouse = space_mouse::SpaceMouse::start(&cc.egui_ctx);
            solidcraft_engine::crash::phase("first frame");
            Ok(Box::new(App {
                solve: app,
                starting,
                frames: 0,
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                space_mouse,
            }))
        }),
    );
    control_server::remove_token_file();
    match result {
        Err(e) if !created.load(std::sync::atomic::Ordering::SeqCst) => no_window(e, cpu_renderer, settings.as_deref()),
        r => r,
    }
}

/// egui-wgpu is about to start a step of getting the graphics device and the window's surface up:
/// the crash journal keeps it, and the adapter once it is picked.
fn startup_step(step: &'static str, detail: &str) {
    if step == eframe::egui_wgpu::STEP_ADAPTER_CHOSEN {
        solidcraft_engine::crash::set_environment(&format!("Renderer: {detail}"));
    }
    solidcraft_engine::crash::phase(step);
}

/// The renderer settings: with `cpu`, the software adapter (WARP, lavapipe, llvmpipe). The
/// backends come from `startup::choose_backends` (on Windows D3D12 first, and only the backend
/// in use is started); `preference` is `graphics.json`'s `backend`.
fn wgpu_options(cpu: bool, preference: Option<&str>) -> eframe::egui_wgpu::WgpuConfiguration {
    let mut options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup {
        let env_set = std::env::var(startup::WGPU_BACKEND_ENV).is_ok_and(|v| !v.trim().is_empty());
        if let Some(backends) = startup::choose_backends(cfg!(target_os = "windows"), env_set, cpu, preference, startup::has_hardware_adapter) {
            eprintln!("SolidCraft graphics: starting {backends:?}");
            setup.instance_descriptor.backends = backends;
        }
        if cpu {
            setup.native_adapter_selector = Some(std::sync::Arc::new(startup::pick_cpu_adapter));
        }
        // A float depth buffer with a stencil, wherever the adapter has it (see `FLOAT_DEPTH_BITS`).
        let base = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut d = base(adapter);
            d.required_features |= adapter.features() & startup::FLOAT_DEPTH_STENCIL;
            d
        });
    }
    options
}

/// The window never opened (no graphics adapter, a device that wouldn't start): once more on
/// the software renderer, else a message box saying what failed instead of a silent exit.
fn no_window(error: eframe::Error, tried_cpu: bool, settings: Option<&std::path::Path>) -> eframe::Result {
    log::error!("the window could not open: {error}");
    eprintln!("SolidCraft: the window could not open: {error}");
    if !tried_cpu && let Ok(exe) = std::env::current_exe() {
        eprintln!("SolidCraft: trying again with the software renderer ({}=cpu)", startup::RENDERER_ENV);
        match std::process::Command::new(exe).args(std::env::args_os().skip(1)).env(startup::RENDERER_ENV, "cpu").status() {
            Ok(status) if status.success() => return Ok(()),
            // The second try showed its own message.
            Ok(_) => return Err(error),
            Err(e) => log::error!("could not start again with the software renderer: {e}"),
        }
    }
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("SolidCraft could not start")
        .set_description(startup::no_window_message(&error.to_string(), settings))
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
    Err(error)
}

/// Whether to run through XWayland rather than native Wayland. winit has no file drag-and-drop
/// on Wayland (only on X11), so dropping files from the file manager showed a "no" cursor: use
/// XWayland when it's there. Except on niri, whose XWayland (xwayland-satellite) breaks window
/// management (maximize/restore), so it stays on native Wayland and gives up file drops.
/// `SOLIDCRAFT_WAYLAND=1` keeps native Wayland, `SOLIDCRAFT_WAYLAND=0` forces XWayland.
#[cfg(all(unix, not(target_os = "macos")))]
fn prefer_x11(display: bool, wayland_display: bool, desktop: Option<&str>, wayland_override: Option<&str>) -> bool {
    match wayland_override.map(str::trim) {
        Some("0") => return display,
        Some(_) => return false,
        None => {}
    }
    let niri = desktop.is_some_and(|d| d.split(':').any(|name| name.trim().eq_ignore_ascii_case("niri")));
    display && !(niri && wayland_display)
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod backend_tests {
    use super::prefer_x11;

    #[test]
    fn xwayland_by_default_native_wayland_on_niri_and_the_override_wins() {
        // XWayland whenever there's an X display, for file drag-and-drop.
        assert!(prefer_x11(true, true, Some("GNOME"), None));
        assert!(prefer_x11(true, false, None, None));
        assert!(!prefer_x11(false, true, Some("KDE"), None));
        // niri (alone or in a desktop list) under Wayland stays native.
        assert!(!prefer_x11(true, true, Some("niri"), None));
        assert!(!prefer_x11(true, true, Some("niri:GNOME"), None));
        // niri without a Wayland socket (nested X session) still uses X11.
        assert!(prefer_x11(true, false, Some("niri"), None));
        // SOLIDCRAFT_WAYLAND: anything but 0 keeps Wayland, 0 forces XWayland (when present).
        assert!(!prefer_x11(true, true, Some("GNOME"), Some("1")));
        assert!(!prefer_x11(true, true, Some("GNOME"), Some("")));
        assert!(prefer_x11(true, true, Some("niri"), Some("0")));
        assert!(!prefer_x11(false, true, Some("niri"), Some("0")));
    }
}

#[cfg(test)]
mod icon_tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_preserves_the_bundle_icon_instead_of_setting_a_runtime_bitmap() {
        let viewport = super::app_viewport().expect("viewport");
        assert_eq!(viewport.icon.as_deref(), Some(&eframe::egui::IconData::default()));
        assert_eq!(viewport.app_id.as_deref(), Some(super::APP_ID));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn desktop_viewport_supplies_the_original_mark_to_the_window() {
        let viewport = super::app_viewport().expect("viewport");
        let icon = viewport.icon.expect("running window must have an icon");
        assert_eq!((icon.width, icon.height), (64, 64));
        assert_eq!(icon.rgba, super::app_icon::rgba(64).expect("original mark"));
        assert_eq!(viewport.app_id.as_deref(), Some(super::APP_ID));
    }
}

#[cfg(test)]
mod msaa_tests {
    #[test]
    fn first_launch_adopts_the_previous_release_settings() {
        let d = std::env::temp_dir().join(format!("solidcraft-adopt-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (current, legacy, legacy_eframe) = (d.join("solidcraft"), d.join("solvecraft"), d.join("ai.storyteller.solvecraft"));
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::create_dir_all(&legacy_eframe).unwrap();
        std::fs::write(legacy.join("graphics.json"), r#"{"msaa":8}"#).unwrap();
        std::fs::write(legacy_eframe.join("app.ron"), "(prefs)").unwrap();
        super::adopt_settings(&current, None, &legacy, Some(&legacy_eframe)).unwrap();
        assert_eq!(std::fs::read_to_string(current.join("graphics.json")).unwrap(), r#"{"msaa":8}"#);
        assert_eq!(std::fs::read_to_string(current.join("app.ron")).unwrap(), "(prefs)");
        assert!(legacy.join("graphics.json").is_file() && legacy_eframe.join("app.ron").is_file());
        // Later launches leave the current settings alone.
        std::fs::write(current.join("app.ron"), "(mine)").unwrap();
        std::fs::remove_file(current.join("graphics.json")).unwrap();
        super::adopt_settings(&current, None, &legacy, Some(&legacy_eframe)).unwrap();
        assert_eq!(std::fs::read_to_string(current.join("app.ron")).unwrap(), "(mine)");
        assert!(!current.join("graphics.json").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn saved_multisampling_uses_the_selected_backend_policy() {
        use eframe::wgpu::Backend;
        for requested in [1, 2, 4, 8] {
            assert_eq!(eframe::egui_wgpu::effective_msaa_samples(Backend::Gl, requested), 1);
            for backend in [Backend::Vulkan, Backend::Dx12, Backend::Metal] {
                assert_eq!(eframe::egui_wgpu::effective_msaa_samples(backend, requested), requested);
            }
        }
    }
}
