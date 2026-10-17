//! SolidCraft in the browser.
//!
//! Runs the same [`solidcraft_ui_egui::SolveApp`] as the desktop app through eframe's web runner
//! (wgpu: WebGPU where available, WebGL2 otherwise). Build with `trunk build --release` from this
//! directory (output in `dist/web`). URL flags: `?webgl` forces WebGL2 (remembered until
//! `?webgpu`), `?sample` opens the sample part.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod files;
#[cfg(target_arch = "wasm32")]
mod install;
#[cfg(any(target_arch = "wasm32", test))]
mod space_mouse;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::rc::Rc;

    use solidcraft_engine::Session;
    use solidcraft_engine::io::vfs;
    use solidcraft_ui_egui::{Services, SolveApp};
    use wasm_bindgen::JsCast as _;

    use crate::api::Api;
    use crate::files::{self, Files, Intent};
    use crate::space_mouse::SpaceMouse;

    const CANVAS_ID: &str = "solidcraft_canvas";
    const LOADING_ID: &str = "solidcraft_loading";
    /// A 32-bit float depth buffer (no stencil): WebGPU and WebGL2 both have it, and the viewport
    /// draws it with reversed depth, whose precision follows the distance from the eye (#1434).
    const DEPTH_BITS: u8 = 32;

    /// The loading animation (assets/styles/web/loading-logo.svg) settles into the finished mark
    /// about 3.2 s after the page starts; this adds a short hold. The app doesn't land before it.
    const LOADING_MIN_MS: f64 = 3600.0;
    /// The fade from the loading screen into the app.
    const LOADING_FADE_MS: i32 = 300;

    /// Take the loading screen away once the animation has played to the finished mark: wait out
    /// what is left of `LOADING_MIN_MS`, fade it, then remove it.
    fn dismiss_loading(el: web_sys::Element) {
        let Some(window) = web_sys::window() else {
            el.remove();
            return;
        };
        let now = window.performance().map_or(LOADING_MIN_MS, |p| p.now());
        let wait = (LOADING_MIN_MS - now).max(0.0) as i32;
        let fade = wasm_bindgen::closure::Closure::once_into_js(move || {
            let _ = el.set_attribute("class", "done");
            let gone = wasm_bindgen::closure::Closure::once_into_js(move || el.remove());
            if let Some(window) = web_sys::window() {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(gone.unchecked_ref(), LOADING_FADE_MS);
            }
        });
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(fade.unchecked_ref(), wait);
    }

    /// Seconds between autosaves of unsaved work.
    const AUTOSAVE_S: f64 = 30.0;

    thread_local! {
        /// An open design has unsaved changes: leaving the page asks first.
        static UNSAVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// The browser's "leave site? changes you made may not be saved" prompt while a design has
    /// unsaved changes (the page can't show its own dialog when it is being closed).
    fn ask_before_unload(window: &web_sys::Window) {
        let on_unload = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(|e: web_sys::BeforeUnloadEvent| {
            if UNSAVED.with(std::cell::Cell::get) {
                e.prevent_default();
                e.set_return_value("unsaved");
            }
        });
        let _ = window.add_event_listener_with_callback("beforeunload", on_unload.as_ref().unchecked_ref());
        // The listener lives as long as the page.
        on_unload.forget();
    }

    const JAPANESE_FONT_PATH: &str = "./fonts/BIZUDPGothic-Regular.ttf";

    type FontLoadResult = Rc<std::cell::RefCell<Option<Result<Vec<u8>, String>>>>;

    struct Shell {
        app: SolveApp,
        api: Api,
        files: Rc<Files>,
        /// When the last autosave was (seconds), and the revision it holds (none stored: `None`).
        autosaved_at: f64,
        autosaved: Option<u64>,
        /// A stored design being renamed: (its name, the new name).
        renaming: Option<(String, String)>,
        /// The rename field takes the keyboard when it appears.
        rename_focus: bool,
        /// The start page's recent designs were filled from the stored ones.
        recents_synced: bool,
        /// On-demand CJK font: fetched only when a CJK language is selected.
        font: solidcraft_ui_egui::fonts::CjkFontLoader,
        font_result: FontLoadResult,
        space_mouse: SpaceMouse,
    }

    impl Shell {
        /// Files from the computer, writes to store, messages and the autosave.
        fn files_tick(&mut self, ctx: &egui::Context) {
            if self.files.ctx.borrow().is_none() {
                *self.files.ctx.borrow_mut() = Some(ctx.clone());
            }
            // A ready picker answer is consumed in app.logic later in this frame. Keep
            // incoming files queued until then so they cannot replace its target design.
            if self.app.file_picker_busy() {
                ctx.request_repaint();
                return;
            }
            files::flush(&self.files);
            for (text, error) in self.files.messages.borrow_mut().drain(..) {
                self.app.set_status(text, error);
            }
            let mut incoming: Vec<_> = self.files.incoming.borrow_mut().drain(..).collect();
            // A glTF file's buffers picked with it are kept next to it, not opened.
            incoming.retain_mut(|(name, bytes, _)| {
                let companion =
                    std::path::Path::new(name.as_str()).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("bin"));
                if companion {
                    vfs::insert(&format!("{}{name}", files::UPLOAD), std::mem::take(bytes));
                }
                !companion
            });
            for (name, bytes, intent) in incoming {
                let design = solidcraft_engine::io::is_design_path(&name);
                let path = if design && intent == Intent::Open {
                    // A design from the computer is kept in the browser (a copy), so Save works.
                    let path = files::free_name(&name);
                    if let Err(e) = vfs::write(&path, &bytes) {
                        self.app.set_status(e.to_string(), true);
                    }
                    files::flush(&self.files);
                    path
                } else {
                    let path = format!("{}{name}", files::UPLOAD);
                    vfs::insert(&path, bytes);
                    path
                };
                match intent {
                    Intent::Open => self.app.open_path(&path),
                    Intent::Insert => self.app.drop_path(&path),
                }
                self.files.panel.set(false);
            }
            if self.files.ready.get() && !self.recents_synced {
                self.recents_synced = true;
                self.sync_recents();
            }
            let now = ctx.input(|i| i.time);
            // Unsaved work is kept (at most every AUTOSAVE_S); saved work needs no autosave.
            if self.files.ready.get() && self.files.recovery.borrow().is_none() {
                let s = &self.app.session;
                let changed = s.is_dirty() && self.autosaved != Some(s.revision);
                let wait = AUTOSAVE_S - (now - self.autosaved_at);
                if changed && wait <= 0.0 {
                    files::autosave(&self.files, Some((&s.design(), s.path.as_deref())));
                    self.autosaved = Some(s.revision);
                    self.autosaved_at = now;
                } else if changed {
                    // Come back for it even when nothing else happens.
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
                } else if !s.is_dirty() && self.autosaved.is_some() {
                    files::autosave(&self.files, None);
                    self.autosaved = None;
                }
            }
            if !self.files.ready.get() || !self.files.incoming.borrow().is_empty() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }

        /// The start page's recent designs are the stored ones: the remembered order first (those
        /// still stored), then the others, newest first.
        fn sync_recents(&mut self) {
            let mut entries = self.files.entries.borrow().clone();
            entries.sort_by(|a, b| b.modified.total_cmp(&a.modified));
            let recent = &mut self.app.home.recent;
            recent.retain(|p| vfs::exists(p));
            for e in entries.iter().filter(|e| solidcraft_engine::io::is_design_path(&e.name)) {
                let p = format!("{}{}", files::OPFS, e.name);
                if !recent.contains(&p) {
                    recent.push(p);
                }
            }
            recent.truncate(solidcraft_ui_egui::home::MAX_RECENT);
        }

        /// The stored designs: open, insert, rename, delete, download; open from the computer.
        fn file_panel(&mut self, ctx: &egui::Context) {
            if !self.files.panel.get() {
                return;
            }
            let mut open = true;
            let mut act: Option<(&'static str, String)> = None;
            egui::Window::new("Files in this browser")
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_width(520.0)
                .pivot(egui::Align2::CENTER_CENTER)
                .default_pos(ctx.content_rect().center())
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Open from computer…").clicked() {
                            files::pick_from_disk(&self.files, Intent::Open);
                        }
                        if ui.button("Insert from computer…").on_hover_text("STEP, 3MF or STL into the open design").clicked() {
                            files::pick_from_disk(&self.files, Intent::Insert);
                        }
                        if ui.button("Download open design").clicked() {
                            act = Some(("download_open", String::new()));
                        }
                    });
                    ui.separator();
                    if !self.files.ready.get() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Reading the browser's storage…");
                        });
                        return;
                    }
                    let entries = self.files.entries.borrow().clone();
                    if entries.is_empty() {
                        ui.label("Designs you save appear here. They stay in this browser.");
                    }
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        egui::Grid::new("opfs_files").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
                            for e in &entries {
                                match &mut self.renaming {
                                    Some((from, to)) if *from == e.name => {
                                        let r = ui.add(egui::TextEdit::singleline(to).desired_width(200.0));
                                        if std::mem::take(&mut self.rename_focus) {
                                            r.request_focus();
                                        }
                                        if r.lost_focus() {
                                            act = Some(if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                                ("rename", to.clone())
                                            } else {
                                                ("rename_cancel", String::new())
                                            });
                                        }
                                    }
                                    _ => {
                                        ui.label(&e.name);
                                    }
                                }
                                ui.label(format!("{:.1} KB", e.size as f64 / 1024.0));
                                ui.horizontal(|ui| {
                                    if ui.small_button("Open").clicked() {
                                        act = Some(("open", e.name.clone()));
                                    }
                                    let design = solidcraft_engine::io::is_design_path(&e.name);
                                    if !design && ui.small_button("Insert").on_hover_text("Into the open design (STEP, 3MF, STL)").clicked() {
                                        act = Some(("insert", e.name.clone()));
                                    }
                                    if ui.small_button("Rename").clicked() {
                                        self.renaming = Some((e.name.clone(), e.name.clone()));
                                        self.rename_focus = true;
                                    }
                                    if ui.small_button("Download").clicked() {
                                        act = Some(("download", e.name.clone()));
                                    }
                                    if ui.small_button("Delete").clicked() {
                                        act = Some(("delete", e.name.clone()));
                                    }
                                });
                                ui.end_row();
                            }
                        });
                    });
                });
            if !open {
                self.files.panel.set(false);
                self.renaming = None;
            }
            let Some((what, name)) = act else { return };
            let path = format!("{}{name}", files::OPFS);
            match what {
                "open" => {
                    self.app.open_path(&path);
                    self.files.panel.set(false);
                }
                "insert" => {
                    self.app.insert_path(&path);
                    self.files.panel.set(false);
                }
                "download" => match vfs::read(&path, solidcraft_engine::io::MAX_DESIGN_BYTES) {
                    Ok(b) => {
                        if let Err(e) = files::download(&name, &b) {
                            self.app.set_status(e, true);
                        }
                    }
                    Err(e) => self.app.set_status(e.to_string(), true),
                },
                "download_open" => {
                    let d = &self.app.session.doc;
                    let result = self
                        .app
                        .session
                        .design_container(solidcraft_engine::io::DesignCompression::Fast, false, false)
                        .map_err(|e| e.to_string())
                        .and_then(|bytes| files::download(&format!("{}.{}", d.name, solidcraft_engine::io::DESIGN_EXT), &bytes));
                    if let Err(e) = result {
                        self.app.set_status(e, true);
                    }
                }
                "delete" => {
                    files::delete(&self.files, &name);
                    self.app.home.recent.retain(|r| *r != path);
                }
                "rename_cancel" => self.renaming = None,
                "rename" => {
                    if let Some((from, _)) = self.renaming.take() {
                        match files::rename(&self.files, &from, &name) {
                            Ok(()) => {
                                // Recent designs follow the new name.
                                let (old, new) = (format!("{}{from}", files::OPFS), format!("{}{name}", files::OPFS));
                                for r in &mut self.app.home.recent {
                                    if *r == old {
                                        *r = new.clone();
                                    }
                                }
                            }
                            Err(e) => self.app.set_status(e, true),
                        }
                    }
                }
                _ => {}
            }
        }

        /// Offer the autosave an earlier session left.
        fn recovery_prompt(&mut self, ctx: &egui::Context) {
            let Some((name, path)) = self.files.recovery.borrow().clone() else { return };
            let mut answer = None;
            egui::Window::new("Recover unsaved work?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
                ctx,
                |ui| {
                    ui.label(format!("“{name}” had unsaved changes when this page last closed."));
                    ui.horizontal(|ui| {
                        if ui.button("Recover").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("Discard").clicked() {
                            answer = Some(false);
                        }
                    });
                },
            );
            let Some(recover) = answer else { return };
            *self.files.recovery.borrow_mut() = None;
            if recover {
                self.app.open_path(&files::recovery_path());
                // It belongs to its own file again (saving goes there); it is unsaved.
                self.app.session.path = path;
                self.app.session.doc_mut().name = name;
                self.app.session.mark_unsaved();
                self.app.home.open = false;
            }
            files::autosave(&self.files, None);
        }

        /// Fetch and install the CJK font when a CJK language is selected. If that fails, this
        /// session falls back to English (no tofu); the saved language preference is kept.
        fn tick_font(&mut self, ctx: &egui::Context) {
            let done = self.font_result.borrow_mut().take();
            if let Some(res) = done {
                let installed = match res {
                    Ok(bytes) => solidcraft_ui_egui::fonts::install_cjk_font(ctx, bytes),
                    Err(err) => {
                        log::warn!("failed to fetch CJK font from {JAPANESE_FONT_PATH}: {err}");
                        false
                    }
                };
                if installed {
                    log::info!("loaded web CJK font from {JAPANESE_FONT_PATH}");
                } else if solidcraft_ui_egui::fonts::needs_cjk_font(&self.app.preferences.language) {
                    log::warn!("CJK font unavailable; showing English for this session");
                }
                self.font.finished(installed, &mut self.app.preferences.language);
                ctx.request_repaint();
            }
            if self.font.tick(&mut self.app.preferences.language) {
                self.fetch_font(ctx);
            }
        }

        /// CJK text on screen with no CJK font yet (an English UI showing a Japanese name):
        /// fetch the font, so the text does not stay tofu (#720).
        fn watch_text_for_font(&mut self, ctx: &egui::Context) {
            if self.font.watching_text() && solidcraft_ui_egui::fonts::painted_cjk(ctx) && self.font.text_needs_font() {
                self.fetch_font(ctx);
            }
        }

        fn fetch_font(&self, ctx: &egui::Context) {
            let slot = self.font_result.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let res = fetch_font_bytes(JAPANESE_FONT_PATH).await;
                *slot.borrow_mut() = Some(res);
                ctx.request_repaint();
            });
        }
    }

    impl eframe::App for Shell {
        fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
            self.tick_font(ctx);
            self.app.logic(ctx);
            self.api.poll();
            self.space_mouse.logic(&mut self.app, ctx);
            self.files_tick(ctx);
            UNSAVED.with(|u| u.set(solidcraft_ui_egui::documents::any_dirty(&self.app)));
        }
        fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
            files::take_drops(&self.files, raw);
            // Before the app's own hook, which adds the control channel's events.
            self.space_mouse.raw_input_hook(ctx, self.app.preferences.space_mouse_enabled, raw);
            self.app.raw_input_hook(raw);
        }
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            self.app.ui(ui);
            let ctx = ui.ctx().clone();
            self.file_panel(&ctx);
            self.recovery_prompt(&ctx);
            self.watch_text_for_font(&ctx);
        }
        fn save(&mut self, storage: &mut dyn eframe::Storage) {
            // Persist the user's own language, not a session-only English fallback.
            let saved = self.font.persisted_language(&self.app.preferences.language).to_owned();
            let live = std::mem::replace(&mut self.app.preferences.language, saved);
            storage.set_string(solidcraft_ui_egui::prefs::STORAGE_KEY, self.app.prefs());
            self.app.preferences.language = live;
        }
    }

    fn query() -> String {
        web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
    }

    /// Where `?webgl` is remembered. The installed app starts from its manifest's start URL, which
    /// has no query, so the choice has to outlive the page it was made on.
    const WEBGL_KEY: &str = "solidcraft.webgl";

    /// Render with WebGL2: asked for now (`?webgl`, remembered) or before (until `?webgpu`).
    fn webgl() -> bool {
        let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
        let q = query();
        if q.contains("webgl") {
            if let Some(s) = &storage {
                let _ = s.set_item(WEBGL_KEY, "1");
            }
            return true;
        }
        if q.contains("webgpu") {
            if let Some(s) = &storage {
                let _ = s.remove_item(WEBGL_KEY);
            }
            return false;
        }
        storage.and_then(|s| s.get_item(WEBGL_KEY).ok().flatten()).is_some()
    }

    /// Fetch font bytes asynchronously through the browser.
    async fn fetch_font_bytes(url: &str) -> Result<Vec<u8>, String> {
        let window = web_sys::window().ok_or("no window")?;
        let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url)).await.map_err(|e| format!("{e:?}"))?;
        let resp: web_sys::Response = resp_val.dyn_into().map_err(|_| "response cast failed")?;
        if !resp.ok() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let ab_val = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(|e| format!("{e:?}"))?).await.map_err(|e| format!("{e:?}"))?;
        let uint8_arr = js_sys::Uint8Array::new(&ab_val);
        Ok(uint8_arr.to_vec())
    }

    pub fn start() {
        eframe::WebLogger::init(log::LevelFilter::Info).ok();
        wasm_bindgen_futures::spawn_local(async {
            let Some(window) = web_sys::window() else { return };
            ask_before_unload(&window);
            let Some(document) = window.document() else { return };
            let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
                log::error!("missing <canvas id=\"{CANVAS_ID}\">");
                return;
            };
            let mut options = eframe::WebOptions { depth_buffer: DEPTH_BITS, ..Default::default() };
            if webgl()
                && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
            {
                create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
            }
            let result = eframe::WebRunner::new()
                .start(
                    canvas,
                    options,
                    Box::new(move |cc| {
                        solidcraft_ui_egui::fonts::setup_fonts(&cc.egui_ctx);
                        let files = Rc::new(Files::default());
                        files::start(&files);
                        let panel = files.clone();
                        let services = Services {
                            // Open shows the browser's files (and the computer's, from there).
                            pick_open: Some(Box::new(move |_: &[&str]| {
                                panel.panel.set(true);
                                Box::pin(std::future::ready(None))
                            })),
                            // Designs are kept in the browser; everything else is downloaded.
                            pick_save: Some(Box::new(|name: &str, exts: &[&str]| {
                                let design = exts.contains(&solidcraft_engine::io::DESIGN_EXT);
                                Box::pin(std::future::ready(Some(format!("{}{name}", if design { files::OPFS } else { files::DOWNLOADS }))))
                            })),
                            install_app: Some(crate::install::service(&cc.egui_ctx)),
                            ..Default::default()
                        };
                        // Thumbnails of saves and start-page cards.
                        solidcraft_ui_egui::gpu::install_cpu_picture_renderer();
                        let (sender, receiver) = std::sync::mpsc::channel();
                        let mut app = SolveApp::new(Session::default(), services).with_control(receiver);
                        if let Some(p) = cc.storage.and_then(|s| solidcraft_ui_egui::prefs::stored(|k| s.get_string(k))) {
                            app.load_prefs(&p);
                        }
                        if let Some(rs) = &cc.wgpu_render_state {
                            // eframe's web painter has no stencil buffer.
                            app.set_wgpu(rs, DEPTH_BITS, 0, 1);
                        }
                        if query().contains("sample") {
                            if let Err(e) = app.session.run_script(&solidcraft_engine::sample::script()) {
                                app.set_status(format!("sample: {e}"), true);
                            }
                            app.session.doc_mut().name = "Sample Plate".into();
                            app.session.mark_saved();
                        }
                        if !query().contains("sample") {
                            // The empty design takes Units of new designs (#1205). The sample
                            // part is built in millimetres and is left that way.
                            solidcraft_ui_egui::prefs::apply_new_design(&mut app);
                            let rev = app.session.revision;
                            app.home.show(rev);
                        }
                        let api = Api::install(sender, cc.egui_ctx.clone()).map_err(|e| format!("SolidCraft browser API: {e:?}"))?;
                        Ok(Box::new(Shell {
                            app,
                            api,
                            files,
                            autosaved_at: f64::NEG_INFINITY,
                            autosaved: None,
                            renaming: None,
                            rename_focus: false,
                            recents_synced: false,
                            font: Default::default(),
                            font_result: Rc::new(std::cell::RefCell::new(None)),
                            space_mouse: SpaceMouse::default(),
                        }))
                    }),
                )
                .await;
            if let Some(el) = document.get_element_by_id(LOADING_ID) {
                match result {
                    Ok(()) => dismiss_loading(el),
                    Err(e) => {
                        el.set_inner_html(&format!("<p>SolidCraft failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>"))
                    }
                }
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("solidcraft-web only runs in the browser: build it with `trunk build --release` in apps/solidcraft-web");
}
