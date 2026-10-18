//! Getting the window open, whatever state the settings folder or the graphics device is in
//! (docs/book/src/guide/troubleshooting.md).
//!
//! - **Settings that don't read** (`app.ron`, `graphics.json`): moved aside as
//!   `<name>.bad-<time>` and the app starts with defaults, instead of a newer or damaged file
//!   being dropped silently or stopping the start.
//! - **Launches that die before the window is up**: a marker file counts them; after two in a
//!   row the next launch starts in safe mode by itself.
//! - **Safe mode** (`--safe-mode` or `SOLIDCRAFT_SAFE_MODE=1`): default settings (the saved ones
//!   are left untouched), no recovery offer, the software renderer and CPU meshing.
//! - **Graphics failure during startup**: one retry in safe mode on the
//!   software renderer, then a message box saying what failed and what to try.

use std::path::{Path, PathBuf};

/// A queued presentation can return before a driver reset is reported (#1789). Keep
/// recovery available for ten seconds after it, unless the user starts interacting.
pub const STARTUP_SETTLE_TIME: std::time::Duration = std::time::Duration::from_secs(10);

/// Shared with the event-loop caller so unwinding the app can veto a relaunch if it
/// contains unsaved work. Once startup has settled or user activity begins, never rearm.
#[derive(Clone)]
pub struct Recovery(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Default for Recovery {
    fn default() -> Self {
        Self::new()
    }
}

impl Recovery {
    pub fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)))
    }

    pub fn eligible(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn observe(&self, since_present: Option<std::time::Duration>, user_activity: bool) {
        if user_activity || since_present.is_some_and(|elapsed| elapsed >= STARTUP_SETTLE_TIME) {
            self.0.store(false, std::sync::atomic::Ordering::Release);
        }
    }
}

/// Start eframe under the graphics recovery boundary, including its app-creation callback.
pub fn run_native(name: &str, options: eframe::NativeOptions, create_app: eframe::AppCreator<'_>) -> eframe::Result {
    run_graphics(|| eframe::run_native(name, options, create_app))
}

/// Keep graphics setup and the first paint inside the same recovery boundary. In particular,
/// wgpu's surface acquisition can panic after eframe has already constructed the app (#1789).
/// The installed panic hook still records the original failure before it is caught here.
pub fn run_graphics(run: impl FnOnce() -> eframe::Result) -> eframe::Result {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(result) => result,
        Err(payload) => {
            let message = payload.downcast_ref::<String>().map(String::as_str).or_else(|| payload.downcast_ref::<&str>().copied());
            let message: String = message.unwrap_or("non-text panic").chars().take(2000).collect();
            Err(eframe::Error::AppCreation(std::io::Error::other(format!("graphics event loop panicked: {message}")).into()))
        }
    }
}

/// A fresh process avoids reusing a failed device or winit event loop. Safe mode also keeps
/// saved settings untouched, disables GPU meshing, and ignores a saved backend preference.
pub fn software_retry(exe: &Path, args: impl IntoIterator<Item = std::ffi::OsString>) -> std::process::Command {
    let mut command = std::process::Command::new(exe);
    command.args(args).arg("--safe-mode").env(RENDERER_ENV, "cpu").env_remove(WGPU_BACKEND_ENV);
    command
}

/// The environment variable that starts safe mode (any value but `0` or empty).
pub const SAFE_MODE_ENV: &str = "SOLIDCRAFT_SAFE_MODE";
/// `SOLIDCRAFT_RENDERER=cpu` picks the software renderer (WARP, lavapipe, llvmpipe).
pub const RENDERER_ENV: &str = "SOLIDCRAFT_RENDERER";
/// Consecutive launches that died before the window was up, after which safe mode starts.
pub const FAILED_STARTS_FOR_SAFE_MODE: u32 = 2;
const MARKER: &str = "starting";

/// Whether an environment value switches something on.
pub fn env_on(v: Option<&str>) -> bool {
    v.map(str::trim).is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}

/// Whether `SOLIDCRAFT_RENDERER` asks for the software renderer.
pub fn cpu_renderer_env(v: Option<&str>) -> bool {
    v.map(str::trim).is_some_and(|v| v.eq_ignore_ascii_case("cpu") || v.eq_ignore_ascii_case("software"))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Move `path` aside as `<name>.bad-<time>` (kept for a bug report); returns where it went.
pub fn back_up(path: &Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "settings".into());
    let mut to = path.with_file_name(format!("{name}.bad-{}", unix_now()));
    let mut n = 1;
    while to.exists() && n < 100 {
        to = path.with_file_name(format!("{name}.bad-{}-{n}", unix_now()));
        n += 1;
    }
    std::fs::rename(path, &to)?;
    Ok(to)
}

/// Whether eframe's `app.ron` reads, with the preferences inside it.
fn app_ron_reads(text: &str, prefs_keys: &[&str]) -> bool {
    let Ok(kv) = ron::from_str::<std::collections::HashMap<String, String>>(text) else { return false };
    prefs_keys.iter().filter_map(|k| kv.get(*k)).all(|p| serde_json::from_str::<serde_json::Value>(p).is_ok_and(|v| v.is_object()))
}

/// Check the settings files in `dir` before anything reads them: one that doesn't read is moved
/// aside (the app then starts with defaults for it). Returns a line per file moved.
pub fn check_settings(dir: &Path, prefs_keys: &[&str]) -> Vec<String> {
    let mut notes = Vec::new();
    let mut check = |name: &str, reads: &dyn Fn(&str) -> bool| {
        let path = dir.join(name);
        let Ok(bytes) = std::fs::read(&path) else { return };
        let ok = std::str::from_utf8(&bytes).is_ok_and(|t| t.trim().is_empty() || reads(t));
        if ok {
            return;
        }
        match back_up(&path) {
            Ok(to) => {
                log::warn!("settings: {} did not read; moved to {}", path.display(), to.display());
                notes.push(format!("{name} could not be read and was moved to {}; defaults are used.", to.display()));
            }
            Err(e) => log::warn!("settings: {} did not read and could not be moved: {e}", path.display()),
        }
    };
    check("app.ron", &|t| app_ron_reads(t, prefs_keys));
    check("graphics.json", &|t| serde_json::from_str::<serde_json::Value>(t).is_ok_and(|v| v.is_object()));
    notes
}

/// A launch begins: count it in the marker in `dir`. Returns how many launches before this one
/// died before their window was up.
pub fn begin(dir: &Path) -> u32 {
    let marker = dir.join(MARKER);
    let before = std::fs::read_to_string(&marker).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0).min(1000);
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(&marker, (before + 1).to_string());
    }
    before
}

/// The window is up: this launch did not fail to start.
pub fn started(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(MARKER));
}

/// The software adapter among `adapters` (WARP on Windows, lavapipe or llvmpipe elsewhere), else
/// the first one that can draw to `surface`.
pub fn pick_cpu_adapter(adapters: &[eframe::wgpu::Adapter], surface: Option<&eframe::wgpu::Surface<'_>>) -> Result<eframe::wgpu::Adapter, String> {
    let fits = |a: &&eframe::wgpu::Adapter| surface.is_none_or(|s| a.is_surface_supported(s));
    adapters
        .iter()
        .filter(fits)
        .find(|a| a.get_info().device_type == eframe::wgpu::DeviceType::Cpu)
        .or_else(|| adapters.iter().find(fits))
        .cloned()
        .ok_or_else(|| {
            let names: Vec<String> = adapters.iter().map(|a| format!("{} ({:?})", a.get_info().name, a.get_info().backend)).collect();
            format!("no graphics adapter can draw the window (found: {})", if names.is_empty() { "none".into() } else { names.join(", ") })
        })
}

/// `WGPU_BACKEND`: when set, wgpu picks the backends from it and the order below is not used.
pub const WGPU_BACKEND_ENV: &str = "WGPU_BACKEND";

/// The graphics backends to start with, or `None` to leave it to wgpu (`WGPU_BACKEND`, or all
/// the backends it was built with).
///
/// On Windows, D3D12 comes first, then Vulkan, then OpenGL, and only the first one with a
/// hardware adapter is started at all. wgpu would otherwise start Vulkan, D3D12 and OpenGL
/// together and prefer Vulkan, and starting Vulkan loads every installed Vulkan driver: Intel's
/// (`igvk64.dll`) crashed SolidCraft at startup that way, inside the driver, before anything
/// could be reported. D3D12 is Windows' own graphics API, the one its drivers are tested with.
/// `has_hardware(backends)` says whether those backends have an adapter that isn't software
/// (it is only asked about the backends being tried, one at a time).
///
/// - `WGPU_BACKEND` set (`env_set`): `None`, whatever it says wins.
/// - The software renderer (`cpu`, safe mode): D3D12 alone on Windows (WARP); `None` elsewhere.
/// - `preference` (`graphics.json`'s `backend`: `dx12`, `vulkan`, `gl` or a comma list; `auto`
///   or empty for the order above): those backends, on any system.
/// - Elsewhere than Windows: `None`.
/// - No hardware adapter at all: D3D12 (WARP, Windows' software renderer).
pub fn choose_backends(
    windows: bool,
    env_set: bool,
    cpu: bool,
    preference: Option<&str>,
    mut has_hardware: impl FnMut(eframe::wgpu::Backends) -> bool,
) -> Option<eframe::wgpu::Backends> {
    use eframe::wgpu::Backends;
    if env_set {
        return None;
    }
    if cpu {
        return windows.then_some(Backends::DX12);
    }
    if let Some(p) = preference.map(str::trim).filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case("auto")) {
        let chosen = Backends::from_comma_list(p);
        if !chosen.is_empty() {
            return Some(chosen);
        }
        log::warn!("graphics: unknown backend preference `{p}`; using the default order");
    }
    if !windows {
        return None;
    }
    Some(WINDOWS_ORDER.into_iter().find(|b| has_hardware(*b)).unwrap_or(Backends::DX12))
}

/// The device feature for a 32-bit float depth buffer with a stencil (reversed depth, #1434).
pub const FLOAT_DEPTH_STENCIL: eframe::wgpu::Features = eframe::wgpu::Features::DEPTH32FLOAT_STENCIL8;

/// Whether every adapter `options` could start offers a float depth buffer with a stencil. eframe
/// makes the window's depth buffer from the depth bits before it picks the adapter, so the window
/// asks for a float one only when whichever it picks has it.
pub fn float_depth_everywhere(options: &eframe::egui_wgpu::WgpuConfiguration) -> bool {
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &options.wgpu_setup else { return false };
    let mut desc = eframe::wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    desc.backends = setup.instance_descriptor.backends;
    let instance = eframe::wgpu::Instance::new(desc);
    let adapters = pollster::block_on(instance.enumerate_adapters(setup.instance_descriptor.backends));
    let all = !adapters.is_empty() && adapters.iter().all(|a| a.features().contains(FLOAT_DEPTH_STENCIL));
    log::info!("graphics: float depth with stencil on every adapter: {all}");
    all
}

/// The order backends are tried in on Windows.
pub const WINDOWS_ORDER: [eframe::wgpu::Backends; 3] = [eframe::wgpu::Backends::DX12, eframe::wgpu::Backends::VULKAN, eframe::wgpu::Backends::GL];

/// Whether `backends` (started on their own, nothing else) have a hardware adapter.
pub fn has_hardware_adapter(backends: eframe::wgpu::Backends) -> bool {
    let mut desc = eframe::wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    desc.flags = eframe::wgpu::InstanceFlags::from_build_config().with_env();
    let instance = eframe::wgpu::Instance::new(desc);
    let adapters = pollster::block_on(instance.enumerate_adapters(backends));
    let found: Vec<String> = adapters.iter().map(|a| format!("{} ({:?})", a.get_info().name, a.get_info().device_type)).collect();
    log::info!("graphics: {backends:?} adapters: {found:?}");
    adapters.iter().any(|a| a.get_info().device_type != eframe::wgpu::DeviceType::Cpu)
}

/// The message shown when no window could be opened at all.
pub fn no_window_message(error: &str, settings: Option<&Path>) -> String {
    let folder = settings.map(|d| d.display().to_string()).unwrap_or_else(|| "the SolidCraft settings folder".into());
    format!(
        "SolidCraft could not open its window.\n\n{error}\n\n\
         The software renderer was tried too. Things to try:\n\
         - Install or update the graphics driver (on Windows, from the GPU maker's site rather than Windows Update).\n\
         - Over Remote Desktop or in a virtual machine, enable 3D acceleration or run it on the machine itself.\n\
         - Start it once with `solidcraft --safe-mode`.\n\
         - Rename the settings folder: {folder}\n\n\
         More: https://docs.solidcraft.io/guide/troubleshooting.html"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_text_graphics_panic_is_an_error_too() {
        let error = run_graphics(|| std::panic::panic_any(17usize)).unwrap_err();
        assert!(error.to_string().contains("non-text panic"));
    }

    #[test]
    fn a_post_present_failure_can_retry_but_interaction_or_unsaved_work_vetoes_it() {
        use std::time::Duration;
        let recovery = Recovery::new();
        let app = recovery.clone();
        app.observe(None, false);
        assert!(recovery.eligible());
        // The reporter's normal launch fails at 7 s and safe mode at 2 s, after
        // Queue::present has returned. Neither should be mistaken for stable startup.
        for seconds in [2, 7] {
            app.observe(Some(Duration::from_secs(seconds)), false);
            assert!(recovery.eligible());
        }
        let error = run_graphics(|| panic!("Surface is not configured for presentation"));
        assert!(error.is_err() && recovery.eligible());
        // App::drop observes dirty documents even if a paint panics before the next logic.
        app.observe(None, true);
        assert!(!recovery.eligible());
        app.observe(Some(Duration::ZERO), false);
        assert!(!recovery.eligible(), "saving or becoming idle must not rearm recovery");

        let recovery = Recovery::new();
        recovery.observe(Some(STARTUP_SETTLE_TIME), false);
        assert!(!recovery.eligible(), "later runtime failures must not relaunch");
    }

    #[test]
    fn software_retry_preserves_files_and_control_args_but_clears_the_backend_override() {
        let args = ["--control", "47812", "a design.solidcraft"].map(std::ffi::OsString::from);
        let command = software_retry(Path::new("solidcraft.exe"), args);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["--control", "47812", "a design.solidcraft", "--safe-mode"]);
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(env.get(std::ffi::OsStr::new(RENDERER_ENV)), Some(&Some(std::ffi::OsStr::new("cpu"))));
        assert_eq!(env.get(std::ffi::OsStr::new(WGPU_BACKEND_ENV)), Some(&None));
    }

    /// #1789's exact validation panic, on WARP, without a driver reset. A failed
    /// reconfiguration can remove wgpu's presentation state while leaving a surface object.
    #[cfg(target_os = "windows")]
    #[test]
    fn an_unconfigured_surface_after_setup_reaches_graphics_recovery() {
        use eframe::wgpu;
        use winit::{event_loop::EventLoop, platform::windows::EventLoopBuilderExtWindows, window::Window};

        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        let event_loop = builder.build().unwrap();
        #[allow(deprecated)]
        let window = std::sync::Arc::new(event_loop.create_window(Window::default_attributes().with_visible(false)).unwrap());
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::DX12;
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance.create_surface(window).unwrap();
        let adapter = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12))
            .into_iter()
            .find(|a| a.get_info().device_type == wgpu::DeviceType::Cpu && a.is_surface_supported(&surface));
        let Some(adapter) = adapter else {
            assert!(std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none(), "WARP is required for the surface regression");
            eprintln!("skipping the surface regression: no WARP adapter");
            return;
        };
        // The reporter's second failure is a surface reconfigure racing thumbnails on
        // the very same queue. Exercise the actual painter resize and capture paths.
        #[allow(deprecated)]
        let capture_window = std::sync::Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_visible(false).with_inner_size(winit::dpi::PhysicalSize::new(64, 64)))
                .unwrap(),
        );
        captures_during_surface_resize(capture_window);
        let (device, _queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let config = surface.get_default_config(&adapter, 64, 64).unwrap();
        surface.configure(&device, &config);
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            other => panic!("WARP must acquire the initial frame: {other:?}"),
        };
        // Capturing this deliberate configuration failure lets the next acquire reproduce
        // the report's secondary panic. Drop the old frame inside the same error scope.
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(&device, &config);
        drop(frame);
        assert!(pollster::block_on(validation.pop()).is_some());
        let result = run_graphics(|| {
            let _frame = surface.get_current_texture();
            Ok(())
        });
        assert!(result.unwrap_err().to_string().contains("Surface is not configured for presentation"));
    }

    #[cfg(target_os = "windows")]
    fn captures_during_surface_resize(window: std::sync::Arc<winit::window::Window>) {
        use eframe::egui_wgpu;
        use std::num::NonZeroU32;
        use std::sync::Arc;
        let mut painter = pollster::block_on(egui_wgpu::winit::Painter::new(
            egui::Context::default(),
            crate::wgpu_options(true, None),
            false,
            egui_wgpu::RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(egui::ViewportId::ROOT, Some(window))).unwrap();
        let rs = painter.render_state().unwrap();
        assert_eq!(rs.adapter.get_info().device_type, eframe::wgpu::DeviceType::Cpu);
        let capturer = solidcraft_gpu::capture::Capturer::new(&rs.adapter, rs.device.clone(), rs.queue.clone())
            .unwrap()
            .with_queue_access(rs.queue_access.clone());
        let barrier = Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            let capture_barrier = barrier.clone();
            scope.spawn(move || {
                let scene = solidcraft_engine::render::Scene::default();
                let camera = solidcraft_engine::render::Camera::default();
                capture_barrier.wait();
                for _ in 0..64 {
                    let rgba = capturer.rgba(&scene, &camera, 64, 64).unwrap();
                    assert_eq!(rgba.len(), 64 * 64 * 4);
                }
            });
            barrier.wait();
            for i in 0..256 {
                painter.on_window_resized(egui::ViewportId::ROOT, NonZeroU32::new(64 + i % 2).unwrap(), NonZeroU32::new(64).unwrap());
            }
        });
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("solidcraft-startup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn switches_read_from_the_environment() {
        assert!(env_on(Some("1")) && env_on(Some("yes")) && !env_on(Some("0")) && !env_on(Some(" ")) && !env_on(None) && !env_on(Some("false")));
        assert!(cpu_renderer_env(Some("cpu")) && cpu_renderer_env(Some("Software")) && !cpu_renderer_env(Some("gpu")) && !cpu_renderer_env(None));
    }

    #[test]
    fn settings_that_do_not_read_are_moved_aside_and_good_ones_stay() {
        let keys = ["solidcraft.prefs", "solvecraft.prefs"];
        let d = scratch("check");
        // Good files (and an empty one) stay where they are.
        std::fs::write(d.join("app.ron"), r#"{"solidcraft.prefs": "{\"grid\":true}", "egui": "(anything)"}"#).unwrap();
        std::fs::write(d.join("graphics.json"), r#"{"msaa":4}"#).unwrap();
        assert!(check_settings(&d, &keys).is_empty());
        assert!(d.join("app.ron").is_file() && d.join("graphics.json").is_file());
        // Damaged RON, preferences that aren't JSON, a truncated graphics.json, non-UTF-8 bytes.
        for (name, text) in [
            ("app.ron", b"{\"solidcraft.prefs\": \"{\\\"gr".as_slice()),
            ("app.ron", br#"{"solvecraft.prefs": "not json"}"#.as_slice()),
            ("graphics.json", br#"{"msaa":"#.as_slice()),
            ("graphics.json", b"\xff\xfe\x00".as_slice()),
        ] {
            std::fs::write(d.join(name), text).unwrap();
            let notes = check_settings(&d, &keys);
            assert_eq!(notes.len(), 1, "{name}: {notes:?}");
            assert!(!d.join(name).exists(), "{name} moved");
            let kept: Vec<_> =
                std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{name}.bad-"))).collect();
            assert!(kept.iter().any(|e| std::fs::read(e.path()).unwrap() == text), "{name}: the bad file is kept");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn launches_that_never_got_a_window_are_counted_until_one_does() {
        let d = scratch("marker");
        assert_eq!(begin(&d), 0);
        assert_eq!(begin(&d), 1);
        assert_eq!(begin(&d), FAILED_STARTS_FOR_SAFE_MODE);
        started(&d);
        assert_eq!(begin(&d), 0);
        // A damaged marker counts as none.
        std::fs::write(d.join(MARKER), "zz").unwrap();
        assert_eq!(begin(&d), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn windows_starts_d3d12_first_and_vulkan_only_when_d3d12_has_no_gpu() {
        use eframe::wgpu::Backends;
        let probe = |with: &'static [Backends]| {
            move |b: Backends| {
                assert!(b == Backends::DX12 || b == Backends::VULKAN || b == Backends::GL, "one backend at a time: {b:?}");
                with.contains(&b)
            }
        };
        // D3D12 works: Vulkan and OpenGL are never even asked about (starting Vulkan loads
        // its drivers, which is what crashed).
        let mut asked = Vec::new();
        let chosen = choose_backends(true, false, false, None, |b| {
            asked.push(b);
            b == Backends::DX12
        });
        assert_eq!(chosen, Some(Backends::DX12));
        assert_eq!(asked, [Backends::DX12]);
        // Everything works: still D3D12 alone.
        assert_eq!(choose_backends(true, false, false, None, probe(&[Backends::DX12, Backends::VULKAN, Backends::GL])), Some(Backends::DX12));
        // No D3D12 hardware adapter: Vulkan, else OpenGL, else D3D12's software adapter (WARP).
        assert_eq!(choose_backends(true, false, false, None, probe(&[Backends::VULKAN, Backends::GL])), Some(Backends::VULKAN));
        assert_eq!(choose_backends(true, false, false, None, probe(&[Backends::GL])), Some(Backends::GL));
        assert_eq!(choose_backends(true, false, false, None, probe(&[])), Some(Backends::DX12));
        // `auto` and blanks are the default order.
        assert_eq!(choose_backends(true, false, false, Some(" auto "), probe(&[Backends::DX12])), Some(Backends::DX12));
        assert_eq!(choose_backends(true, false, false, Some(""), probe(&[Backends::GL])), Some(Backends::GL));
    }

    #[test]
    fn wgpu_backend_the_preference_and_the_software_renderer_override_the_order() {
        use eframe::wgpu::Backends;
        let never = |b: Backends| -> bool { panic!("no probing expected, asked about {b:?}") };
        // WGPU_BACKEND: wgpu reads it itself, nothing is probed.
        assert_eq!(choose_backends(true, true, false, Some("gl"), never), None);
        assert_eq!(choose_backends(true, true, true, None, never), None);
        // The software renderer (safe mode, the retry): WARP through D3D12 alone, never Vulkan.
        assert_eq!(choose_backends(true, false, true, Some("vulkan"), never), Some(Backends::DX12));
        assert_eq!(choose_backends(false, false, true, None, never), None);
        // The preference, on any system, without probing.
        assert_eq!(choose_backends(true, false, false, Some("vulkan"), never), Some(Backends::VULKAN));
        assert_eq!(choose_backends(true, false, false, Some("GL"), never), Some(Backends::GL));
        assert_eq!(choose_backends(true, false, false, Some("dx12,vulkan"), never), Some(Backends::DX12 | Backends::VULKAN));
        assert_eq!(choose_backends(false, false, false, Some("vulkan"), never), Some(Backends::VULKAN));
        // An unknown preference is the default order.
        assert_eq!(choose_backends(true, false, false, Some("glide"), |b| b == Backends::DX12), Some(Backends::DX12));
        // Elsewhere: wgpu's own choice.
        assert_eq!(choose_backends(false, false, false, None, never), None);
        assert_eq!(choose_backends(false, false, false, Some("auto"), never), None);
    }

    #[test]
    fn probing_a_backend_alone_never_panics() {
        // Whatever this machine has (often nothing): an answer, not a crash.
        for b in WINDOWS_ORDER {
            let _ = has_hardware_adapter(b);
        }
    }

    #[test]
    fn the_message_names_the_settings_folder_and_safe_mode() {
        let m = no_window_message("no adapter", Some(Path::new("/x/SolidCraft")));
        assert!(m.contains("no adapter") && m.contains("/x/SolidCraft") && m.contains("--safe-mode"));
    }

    #[test]
    fn the_software_adapter_is_preferred_and_its_absence_is_an_error_not_a_panic() {
        assert!(pick_cpu_adapter(&[], None).unwrap_err().contains("none"));
        let instance = eframe::wgpu::Instance::new(eframe::wgpu::InstanceDescriptor::new_without_display_handle());
        let adapters = pollster::block_on(instance.enumerate_adapters(eframe::wgpu::Backends::all()));
        if adapters.is_empty() {
            return;
        }
        let picked = pick_cpu_adapter(&adapters, None).unwrap();
        if adapters.iter().any(|a| a.get_info().device_type == eframe::wgpu::DeviceType::Cpu) {
            assert_eq!(picked.get_info().device_type, eframe::wgpu::DeviceType::Cpu);
        }
    }
}
