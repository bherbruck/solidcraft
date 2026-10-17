//! Interactive file operations run off the window thread. The old design stays intact until
//! the command and display meshes are ready; failures and cancelled jobs never replace it.
//!
//! While one runs, the window keeps drawing frames: the title bar still moves, resizes,
//! minimizes and maximizes the window, and a light overlay over the design shows the file's
//! progress with Cancel. The overlay takes the pointer, shortcuts and commands wait, and
//! control-channel commands that change the design are held until the file is in. A control
//! (MCP) `doc.open` or import is answered when its file finishes loading.

use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
    mpsc::{Receiver, Sender, TryRecvError},
};

use serde_json::{Value, json};
use solidcraft_engine::{Session, doc::Document, doc::progress::Progress};

use crate::SolveApp;
use crate::control::ControlResponse;

/// What a file operation does with its file.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Open the design (or a STEP/mesh file as a new design) in a tab of its own.
    Open,
    /// Insert a STEP or mesh file's bodies into the active design.
    Insert,
    /// Open a design an earlier run autosaved before it closed.
    Recover { id: String, dir: Option<String> },
    /// Export a snapshot of the current design without adopting the worker's session.
    ExportDxf,
}

pub struct Request {
    /// The file, or for a recovery the design's name.
    pub path: String,
    pub kind: Kind,
    /// A control-channel caller waiting for the command's reply.
    pub reply: Option<Sender<ControlResponse>>,
    /// The folders that caller's agent may use: the worker reads the file inside them.
    pub scope: Option<solidcraft_engine::io::vfs::PathScope>,
    /// What that caller asked of a STEP import besides its path (`up`, `names`): the command
    /// gets them as given.
    pub options: serde_json::Map<String, Value>,
}

impl Request {
    pub fn new(path: &str, kind: Kind) -> Self {
        Request { path: path.into(), kind, reply: None, scope: None, options: Default::default() }
    }
    fn insert(&self) -> bool {
        self.kind == Kind::Insert
    }
    /// The command's parameters: the path and the caller's import options.
    fn params(&self) -> Value {
        let mut params = self.options.clone();
        params.insert("path".into(), json!(self.path));
        Value::Object(params)
    }
    fn command(&self) -> (&'static str, Value) {
        match &self.kind {
            Kind::Open => ("doc.open", self.params()),
            Kind::Insert if solidcraft_engine::io::is_mesh_path(&self.path) => ("file.insert_mesh", json!({"path": self.path})),
            Kind::Insert => ("file.insert_step", self.params()),
            Kind::Recover { id, dir } => ("doc.recover", json!({"id": id, "dir": dir})),
            Kind::ExportDxf => ("file.export_view_dxf", Value::Object(self.options.clone())),
        }
    }
    fn answer(&self, r: Result<Value, String>) {
        if let Some(reply) = &self.reply {
            let _ = reply.send(match r {
                Ok(v) => json!({"ok": true, "result": v}),
                Err(e) => json!({"ok": false, "error": e}),
            });
        }
    }
}

struct Made {
    session: Session,
    result: Value,
    image: Option<egui::ColorImage>,
}
type Job = Result<Made, String>;
struct Running {
    request: Request,
    base: Arc<Document>,
    base_revision: u64,
    before: Vec<String>,
    rx: Receiver<Job>,
    phase: Arc<AtomicU8>,
    progress: Arc<Progress>,
    preview_rx: Option<Receiver<egui::ColorImage>>,
    preview: Option<egui::TextureHandle>,
    /// Pictures of the model as far as it is built (the picture, features built).
    partial_rx: Option<Receiver<(egui::ColorImage, usize)>>,
    /// The picture shown is of the first this many features (`None`: the saved preview).
    built: Option<usize>,
    /// A STEP file read a piece per frame on this thread (the web app), instead of on a worker.
    staged: Option<Staged>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.request.kind == Kind::ExportDxf {
            self.progress.cancel();
        }
    }
}

/// A STEP import done a piece per frame, for the web app: it has one thread, which must keep
/// drawing (and offer Cancel) while a large file comes in. The file is parsed in one frame,
/// then its solids are converted a few at a time, each frame for about [`STAGED_FRAME_MS`];
/// the import is then cached, and the command that inserts or opens the file finds it read.
struct Staged {
    tx: Sender<Job>,
    session: Option<Session>,
    command: &'static str,
    params: Value,
    reading: Option<solidcraft_engine::kernel::StepReading>,
    /// The command's result once it has run; then the bodies' meshes are made, a few a frame,
    /// so the first frame that draws the design doesn't make them all at once.
    ran: Option<Value>,
    /// Bodies meshed so far, and their triangles.
    warmed: usize,
    triangles: usize,
    /// The overlay has been drawn once (the first frame does no work, so it shows at once).
    shown: bool,
}

/// Designs up to this many triangles get a thumbnail (a picture of more takes a while).
const STAGED_THUMBNAIL_TRIANGLES: usize = 1_000_000;

/// How long a frame spends on a staged STEP import.
const STAGED_FRAME_MS: f64 = 30.0;

/// Files read in stages: STEP (not IGES, FreeCAD or Rhino, which are first restated as STEP) to
/// insert or open.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn staged_kind(request: &Request) -> bool {
    let p = &request.path;
    matches!(request.kind, Kind::Insert | Kind::Open)
        && solidcraft_engine::io::is_step_path(p)
        && !solidcraft_engine::io::is_iges_path(p)
        && !solidcraft_engine::io::is_native_cad_path(p)
}

impl Running {
    fn matches(&self, session: &Session) -> bool {
        Arc::ptr_eq(&session.doc, &self.base) && session.revision == self.base_revision
    }
    fn cancelled(&self) -> bool {
        self.progress.is_cancelled()
    }
}

#[derive(Default)]
pub struct FileLoad {
    queued: VecDeque<Request>,
    running: Option<Running>,
    /// Tests: the next load waits before this feature until cancelled.
    #[cfg(test)]
    pub hold_at: Option<usize>,
}

impl FileLoad {
    /// A file operation is under way: the design waits for it.
    pub fn busy(&self) -> bool {
        self.running.as_ref().is_some_and(|r| !r.cancelled()) || !self.queued.is_empty()
    }
    /// The running load's progress: (features rebuilt, timeline length).
    pub fn features(&self) -> Option<(usize, usize)> {
        self.running.as_ref().map(|r| r.progress.features())
    }
    /// Nothing running or queued (a cancelled job may still be finishing in [`Self::busy`]'s eyes).
    pub fn idle(&self) -> bool {
        self.running.is_none() && self.queued.is_empty()
    }
    /// Stop the running operation at its next feature and drop the queued ones.
    pub fn cancel(&mut self) {
        if let Some(r) = &self.running {
            r.progress.cancel();
        }
        for q in self.queued.drain(..) {
            q.answer(Err("file operation cancelled".into()));
        }
    }
}

pub fn begin(app: &mut SolveApp, path: &str, insert: bool) {
    let _ = begin_request(app, Request::new(path, if insert { Kind::Insert } else { Kind::Open }));
}

/// Queue a file operation; on the web (no threads) it runs now.
pub fn begin_request(app: &mut SolveApp, request: Request) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        if staged_kind(&request) {
            app.file_load.queued.push_back(request);
            start_staged(app);
            return Ok(());
        }
        let r = match &request.kind {
            Kind::Insert => {
                app.insert_path(&request.path);
                Ok(Value::Null)
            }
            Kind::Open => {
                app.open_path(&request.path);
                Ok(Value::Null)
            }
            Kind::Recover { .. } => {
                let (command, params) = request.command();
                let r = app.run(command, params);
                if r.is_ok() {
                    recovered(app, &request);
                }
                r
            }
            Kind::ExportDxf => {
                let (command, params) = request.command();
                let r = app.run(command, params);
                if r.is_ok() && !request.path.is_empty() {
                    app.set_status(crate::i18n::format("ui.saved_to", &[("path", &request.path)]), false);
                }
                r
            }
        };
        request.answer(r);
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if app.file_load.queued.len() >= crate::documents::MAX_OPEN {
            let e = "too many queued files".to_string();
            app.set_status(e.clone(), true);
            request.answer(Err(e.clone()));
            return Err(e);
        }
        app.file_load.queued.push_back(request);
        start(app);
        Ok(())
    }
}

/// A control-channel (MCP) `doc.open` or STEP/mesh import runs like File > Open: off the window
/// thread, answered when the file is in. Other requests come back to be handled now.
pub fn control(app: &mut SolveApp, req: crate::control::ControlRequest) -> Option<crate::control::ControlRequest> {
    if cfg!(target_arch = "wasm32") || !matches!(req.method.as_str(), "engine.execute" | "command") {
        return Some(req);
    }
    let command = req.params.get("command").or(req.params.get("id")).and_then(Value::as_str);
    let kind = match command {
        Some("doc.open") => Kind::Open,
        Some("file.insert_step" | "file.insert_mesh") => Kind::Insert,
        Some("file.export_view_dxf") => Kind::ExportDxf,
        _ => return Some(req),
    };
    let params = req.params.get("params");
    let path = match params.and_then(|p| p.get("path")).and_then(Value::as_str) {
        Some(path) => path,
        None if kind == Kind::ExportDxf => "",
        None => return Some(req),
    };
    let scope = match crate::control::request_scope(app, &req.params) {
        Ok(scope) => scope,
        Err(e) => {
            let _ = req.reply.send(json!({"ok": false, "error": e}));
            return None;
        }
    };
    let options = if kind == Kind::ExportDxf {
        params.and_then(Value::as_object).cloned().unwrap_or_default()
    } else {
        ["up", "names"].iter().filter_map(|k| Some((k.to_string(), params?.get(*k).filter(|v| !v.is_null())?.clone()))).collect()
    };
    let _ = begin_request(app, Request { path: path.into(), kind, reply: Some(req.reply.clone()), scope, options });
    None
}

/// An autosave that failed to load goes back in the recovery list, so it can be discarded (or
/// left for later) instead of coming back unasked.
fn reoffer(app: &mut SolveApp, id: &str, dir: Option<&str>) {
    let dir = dir.map(std::path::PathBuf::from).or_else(|| app.autosave.as_ref().map(|a| a.dir.clone()));
    let Some(dir) = dir else { return };
    if app.recoverable.iter().any(|e| e.id == id) {
        return;
    }
    if let Some(e) = solidcraft_engine::recovery::orphans(&dir).into_iter().find(|e| e.id == id) {
        app.recoverable.insert(0, e);
    }
}

/// A recovered design is ours now: autosave it under this app's entry before the old one goes.
fn recovered(app: &mut SolveApp, request: &Request) {
    let Kind::Recover { id, dir } = &request.kind else { return };
    let kept = app.autosave.as_mut().map(|a| a.save(&app.session));
    if let Some(Ok(_)) = kept {
        let _ = app.session.execute("doc.recovery_discard", &json!({"id": id, "dir": dir}));
    }
    app.set_status(format!("recovered {}: save it to keep it", request.path), false);
}

#[cfg(not(target_arch = "wasm32"))]
fn start(app: &mut SolveApp) {
    if app.file_load.running.is_some() {
        return;
    }
    let Some(request) = app.file_load.queued.pop_front() else { return };
    if matches!(request.kind, Kind::Open | Kind::Recover { .. })
        && app.docs.count() >= crate::documents::MAX_OPEN
        && !crate::documents::active_is_blank(app)
    {
        let e = "too many open designs; close a tab before opening another";
        app.set_status(e, true);
        request.answer(Err(e.into()));
        return;
    }
    let insert = request.insert();
    let before = if insert { app.session.world_state().bodies.iter().map(|b| b.name.clone()).collect() } else { Vec::new() };
    let mut scratch = if insert || request.kind == Kind::ExportDxf { app.session.scratch() } else { Session::default() };
    if insert {
        scratch.selection = app.session.selection.clone();
    }
    let base = app.session.doc.clone();
    let base_revision = app.session.revision;
    let phase = Arc::new(AtomicU8::new(0));
    let worker_phase = phase.clone();
    let progress = Arc::new(Progress::default());
    #[cfg(test)]
    if let Some(i) = app.file_load.hold_at.take() {
        progress.hold_at(i);
    }
    let worker_progress = progress.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let (preview_tx, preview_rx) = std::sync::mpsc::channel();
    let (command, params) = request.command();
    let scope = request.scope.clone();
    let thumbnail = request.kind == Kind::Open;
    // While a design rebuilds, a picture of what is built so far (a large design takes a while).
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let partial_rx = (command == "doc.open" || command == "doc.recover").then(|| {
        progress.want_partial();
        let (partial_tx, partial_rx) = std::sync::mpsc::channel();
        let (watch, finished) = (progress.clone(), finished.clone());
        let _ = std::thread::Builder::new().name("file-load-picture".into()).spawn(move || {
            let stop = || finished.load(Ordering::Relaxed) || watch.is_cancelled();
            while !stop() {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let Some(partial) = watch.take_partial() else { continue };
                let began = std::time::Instant::now();
                let picture = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| partial_picture(&partial.world, &stop))).ok().flatten();
                if solidcraft_engine::geom::env::var_os("PROFILE_OPEN").is_some() {
                    eprintln!(
                        "SolidCraft partial picture: features={} bodies={} drawn={} ms={:.1}",
                        partial.features,
                        partial.world.bodies.len(),
                        picture.is_some(),
                        began.elapsed().as_secs_f64() * 1000.0
                    );
                }
                if let Some(picture) = picture.filter(|_| !stop()) {
                    let _ = partial_tx.send((picture, partial.features));
                }
            }
        });
        partial_rx
    });
    let worker_finished = finished;
    let spawned = std::thread::Builder::new().name("file-load".into()).spawn(move || {
        let preview_began = std::time::Instant::now();
        if command == "doc.open" {
            let progress = worker_progress.clone();
            scratch.open_preview = Some(Box::new(move |preview| {
                if !progress.is_cancelled() {
                    // Render on the file worker, never the window thread.
                    let picture = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| saved_picture(preview))).ok().flatten();
                    if let Some(picture) = picture.filter(|_| !progress.is_cancelled()) {
                        let _ = preview_tx.send(picture);
                        if solidcraft_engine::geom::env::var_os("PROFILE_OPEN").is_some() {
                            eprintln!("SolidCraft saved preview: ready_ms={:.3}", preview_began.elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                }
            }));
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compute(scratch, command, &params, scope, thumbnail, &worker_phase, &worker_progress)
        }))
        .unwrap_or_else(|_| Err("the file operation failed; the current design is unchanged".into()));
        worker_finished.store(true, Ordering::Relaxed);
        let _ = tx.send(result);
    });
    match spawned {
        Ok(_) => {
            app.file_load.running = Some(Running {
                request,
                base,
                base_revision,
                before,
                rx,
                phase,
                progress,
                preview_rx: Some(preview_rx),
                preview: None,
                partial_rx,
                built: None,
                staged: None,
            })
        }
        Err(e) => {
            let e = format!("could not start file operation: {e}");
            request.answer(Err(e.clone()));
            app.set_status(e, true);
        }
    }
}

/// Start the next queued STEP file as a [`Staged`] import (see [`step_staged`]).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn start_staged(app: &mut SolveApp) {
    if app.file_load.running.is_some() {
        return;
    }
    let Some(request) = app.file_load.queued.pop_front() else { return };
    let insert = request.insert();
    let before = if insert { app.session.world_state().bodies.iter().map(|b| b.name.clone()).collect() } else { Vec::new() };
    let mut session = if insert { app.session.scratch() } else { Session::default() };
    if insert {
        session.selection = app.session.selection.clone();
    }
    let (command, params) = request.command();
    let (tx, rx) = std::sync::mpsc::channel();
    let progress = Arc::new(Progress::default());
    #[cfg(test)]
    if let Some(i) = app.file_load.hold_at.take() {
        progress.hold_at(i);
    }
    app.file_load.running = Some(Running {
        base: app.session.doc.clone(),
        base_revision: app.session.revision,
        before,
        rx,
        phase: Arc::new(AtomicU8::new(0)),
        progress,
        preview_rx: None,
        preview: None,
        partial_rx: None,
        built: None,
        staged: Some(Staged { tx, session: Some(session), command, params, reading: None, ran: None, warmed: 0, triangles: 0, shown: false }),
        request,
    });
}

/// One frame of a staged STEP import: parse the file, or convert solids for a frame's worth of
/// time, or, once every solid is in, run the command (which finds the file read) and hand its
/// result to [`tick`] as a worker would.
fn step_staged(running: &mut Running) {
    let Some(staged) = running.staged.as_mut() else { return };
    let progress = running.progress.clone();
    let scope = running.request.scope.clone();
    let path = running.request.path.clone();
    let send = |staged: &mut Staged, job: Job| {
        let _ = staged.tx.send(job);
        staged.session = None;
        staged.reading = None;
    };
    if staged.session.is_none() {
        return;
    }
    if progress.is_cancelled() {
        send(staged, Err("file operation cancelled".into()));
        return;
    }
    if !staged.shown {
        staged.shown = true;
        return;
    }
    let began = web_time::Instant::now();
    let in_budget = || began.elapsed().as_secs_f64() * 1000.0 < STAGED_FRAME_MS;
    if staged.ran.is_some() {
        let Some(session) = staged.session.as_ref() else { return };
        let world = session.world_state();
        while staged.warmed < world.bodies.len() && in_budget() {
            if let Some(body) = world.bodies.get(staged.warmed) {
                staged.triangles += body.mesh().triangles.len();
            }
            staged.warmed += 1;
        }
        if staged.warmed < world.bodies.len() {
            return;
        }
        let (Some(session), Some(result)) = (staged.session.take(), staged.ran.take()) else { return };
        let thumbnail = staged.command == "doc.open" && staged.triangles <= STAGED_THUMBNAIL_TRIANGLES;
        let image = if thumbnail { crate::home::remember(&session) } else { None };
        send(staged, Ok(Made { session, result, image }));
        return;
    }
    let Some(reading) = staged.reading.as_mut() else {
        // Parse and walk the file. The reading keeps the watch: its steps report progress and
        // stop on Cancel.
        let read = solidcraft_engine::io::vfs::scoped(scope, || {
            let bytes = solidcraft_engine::io::vfs::read(&path, solidcraft_engine::io::MAX_STEP_BYTES).map_err(|e| format!("{path}: {e}"))?;
            // The same text the import command makes of the bytes, so it finds this reading.
            let text = String::from_utf8_lossy(&bytes).into_owned();
            solidcraft_engine::doc::progress::watch(progress.clone(), || solidcraft_engine::kernel::StepReading::new(&text))
                .map_err(|e| format!("{path}: {e}"))
        });
        match read {
            Ok(reading) => staged.reading = Some(reading),
            Err(e) => send(staged, Err(e)),
        }
        return;
    };
    while in_budget() {
        if !reading.step() {
            break;
        }
    }
    let (done, total) = reading.progress();
    if done < total && !progress.is_cancelled() {
        return;
    }
    let Some(reading) = staged.reading.take() else { return };
    // Held until the command has read the file from the cache.
    let import = reading.finish_shared();
    let import = match import {
        _ if progress.is_cancelled() => Err("file operation cancelled".to_string()),
        Ok(import) => Ok(import),
        Err(e) => Err(format!("{path}: {e}")),
    };
    let (Some(mut session), Ok(import)) = (staged.session.take(), import.clone()) else {
        send(staged, Err(import.err().unwrap_or_else(|| "file operation cancelled".into())));
        return;
    };
    let (command, params) = (staged.command, staged.params.clone());
    let result = solidcraft_engine::io::vfs::scoped(running.request.scope.clone(), || {
        solidcraft_engine::doc::progress::watch(progress.clone(), || session.execute(command, &params))
    });
    drop(import);
    match result {
        Ok(result) => {
            // Next frames: the meshes ("Preparing the view").
            running.phase.store(1, Ordering::Relaxed);
            staged.session = Some(session);
            staged.ran = Some(result);
        }
        Err(e) => send(staged, Err(e.to_string())),
    }
}

/// A small picture of the bodies built so far (their display meshes, made here and kept for
/// the finished model), or `None` when there is nothing to show yet, too much to draw, or the
/// load ended meanwhile (`stop`).
#[cfg(not(target_arch = "wasm32"))]
fn partial_picture(world: &solidcraft_engine::doc::ModelState, stop: &dyn Fn() -> bool) -> Option<egui::ColorImage> {
    use solidcraft_engine::{
        geom::Aabb3,
        render::{Camera, Rgb, Scene, SceneMesh, StandardView},
    };
    let mut scene = Scene::default();
    let mut bounds = Aabb3::EMPTY;
    let mut triangles = 0usize;
    for body in &world.bodies {
        if stop() {
            return None;
        }
        let mesh = body.mesh();
        triangles += mesh.triangles.len();
        if triangles > 4_000_000 {
            return None;
        }
        bounds = bounds.union(&mesh.bounds());
        scene.meshes.push(SceneMesh { mesh, color: Rgb(176, 186, 199), face_colors: Vec::new() });
    }
    if bounds.is_empty() {
        return None;
    }
    let mut camera = Camera::default();
    camera.set_view(StandardView::Iso);
    camera.fit(&bounds);
    scene.radius = bounds.diagonal();
    if stop() || !preview_work_fits(&scene, &camera) {
        return None;
    }
    preview_image(&scene, &camera)
}

#[cfg(not(target_arch = "wasm32"))]
fn saved_picture(preview: solidcraft_engine::io::DesignPreview) -> Option<egui::ColorImage> {
    use solidcraft_engine::{
        geom::{Aabb3, Mesh, Vec3},
        render::{Camera, Rgb, Scene, SceneMesh, StandardView},
    };
    let mut scene = Scene::default();
    let mut bounds = Aabb3::EMPTY;
    for cached in preview.meshes {
        let mut mesh = Mesh {
            positions: cached.positions.as_chunks::<3>().0.iter().map(|p| Vec3::new(p[0], p[1], p[2])).collect(),
            triangles: cached.triangles.as_chunks::<3>().0.to_vec(),
            ..Default::default()
        };
        mesh.normals.resize(mesh.positions.len(), Vec3::ZERO);
        for triangle in &mesh.triangles {
            let a = *mesh.positions.get(*triangle.first()? as usize)?;
            let b = *mesh.positions.get(*triangle.get(1)? as usize)?;
            let c = *mesh.positions.get(*triangle.get(2)? as usize)?;
            let normal = (b - a).cross(c - a).normalized().unwrap_or(Vec3::Z);
            for index in triangle {
                if let Some(n) = mesh.normals.get_mut(*index as usize) {
                    *n += normal;
                }
            }
        }
        for normal in &mut mesh.normals {
            *normal = normal.normalized().unwrap_or(Vec3::Z);
        }
        bounds = bounds.union(&mesh.bounds());
        let [r, g, b] = cached.color.unwrap_or([0.69, 0.73, 0.78]).map(|c| (c * 255.0).round() as u8);
        scene.meshes.push(SceneMesh { mesh: Arc::new(mesh), color: Rgb(r, g, b), face_colors: Vec::new() });
    }
    if bounds.is_empty() {
        return None;
    }
    let mut camera = Camera::default();
    camera.set_view(StandardView::Iso);
    camera.fit(&bounds);
    scene.radius = bounds.diagonal();
    if !preview_work_fits(&scene, &camera) {
        return None;
    }
    preview_image(&scene, &camera)
}

/// The preview picture, drawn on the GPU by the renderer the app installs (none: no picture).
#[cfg(not(target_arch = "wasm32"))]
fn preview_image(scene: &solidcraft_engine::render::Scene, camera: &solidcraft_engine::render::Camera) -> Option<egui::ColorImage> {
    match solidcraft_engine::capture::rgba(scene, camera, 320, 200) {
        Ok(picture) => Some(egui::ColorImage::from_rgba_unmultiplied([320, 200], &picture.rgba)),
        Err(e) => {
            log::warn!("no preview picture: {e}");
            None
        }
    }
}

/// Bound drawing work as well as file size: repeated full-screen triangles (a hostile file) must
/// not monopolize the file worker or a software GPU. Counts each triangle's pixel box at twice the
/// picture's size.
#[cfg(not(target_arch = "wasm32"))]
fn preview_work_fits(scene: &solidcraft_engine::render::Scene, camera: &solidcraft_engine::render::Camera) -> bool {
    let mut pixels = 0.0;
    let projection = camera.view_proj(640.0 / 400.0, scene.radius.max(1.0));
    for body in &scene.meshes {
        let points: Vec<_> = body
            .mesh
            .positions
            .iter()
            .map(|p| {
                let q = projection.apply(*p);
                (q[3] > 1e-9).then(|| ((q[0] / q[3] + 1.0) * 320.0, (1.0 - q[1] / q[3]) * 200.0, q[2] / q[3]))
            })
            .collect();
        for triangle in &body.mesh.triangles {
            let mut lo = [f64::INFINITY; 2];
            let mut hi = [f64::NEG_INFINITY; 2];
            let mut visible = true;
            for index in triangle {
                if let Some(Some((x, y, _))) = points.get(*index as usize) {
                    lo = [lo[0].min(*x), lo[1].min(*y)];
                    hi = [hi[0].max(*x), hi[1].max(*y)];
                } else {
                    visible = false;
                    break;
                }
            }
            if visible {
                let width = (hi[0].ceil().min(639.0) - lo[0].floor().max(0.0) + 1.0).max(0.0);
                let height = (hi[1].ceil().min(399.0) - lo[1].floor().max(0.0) + 1.0).max(0.0);
                pixels += width * height;
                if !pixels.is_finite() || pixels > 16_000_000.0 {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(not(target_arch = "wasm32"))]
fn compute(
    mut session: Session,
    command: &str,
    params: &Value,
    scope: Option<solidcraft_engine::io::vfs::PathScope>,
    thumbnail: bool,
    phase: &AtomicU8,
    progress: &Arc<Progress>,
) -> Job {
    let profile = solidcraft_engine::geom::env::var_os("PROFILE_OPEN").is_some();
    let began = std::time::Instant::now();
    let cancelled = || Err("file operation cancelled".to_string());
    if progress.is_cancelled() {
        return cancelled();
    }
    // The timeline's rebuild reports each feature and stops at the next one after Cancel. An
    // agent's scope is per thread, so it is set again here.
    let result =
        solidcraft_engine::io::vfs::scoped(scope, || solidcraft_engine::doc::progress::watch(progress.clone(), || session.execute(command, params)))
            .map_err(|e| e.to_string())?;
    if command == "file.export_view_dxf" {
        // A successful export has already published its complete file. A Cancel arriving
        // after that commit must not turn a completed save into a misleading cancellation.
        return Ok(Made { session, result, image: None });
    }
    if progress.is_cancelled() {
        return cancelled();
    }
    let geometry_ms = began.elapsed().as_secs_f64() * 1000.0;
    let view_began = std::time::Instant::now();
    phase.store(1, Ordering::Relaxed);
    let world = session.world_state();
    let placement_ms = view_began.elapsed().as_secs_f64() * 1000.0;
    let mesh_began = std::time::Instant::now();
    // Fit, picking and the first viewport frame must not trigger cold tessellation on the UI.
    world.warm_meshes(|| !progress.is_cancelled());
    if progress.is_cancelled() {
        return cancelled();
    }
    let tessellation_ms = mesh_began.elapsed().as_secs_f64() * 1000.0;
    phase.store(2, Ordering::Relaxed);
    let view_ms = view_began.elapsed().as_secs_f64() * 1000.0;
    let thumbnail_began = std::time::Instant::now();
    let image = if thumbnail { crate::home::remember(&session) } else { None };
    if profile {
        eprintln!(
            "SolidCraft open phases: geometry_ms={geometry_ms:.3} placement_ms={placement_ms:.3} tessellation_ms={tessellation_ms:.3} display_warmup_ms={view_ms:.3} thumbnail_ms={:.3} total_ms={:.3}",
            thumbnail_began.elapsed().as_secs_f64() * 1000.0,
            began.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(Made { session, result, image })
}

pub fn tick(app: &mut SolveApp, ctx: &egui::Context) {
    if let Some(running) = &mut app.file_load.running {
        step_staged(running);
    }
    if let Some(running) = &mut app.file_load.running {
        if !running.cancelled() && running.matches(&app.session) {
            if let Some(image) = running.preview_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                running.preview = Some(ctx.load_texture("opening-preview", image, egui::TextureOptions::LINEAR));
                running.built = None;
            }
            // The model as far as it is built, unless the file's saved picture (all of it) is up.
            if let Some((image, built)) = running.partial_rx.as_ref().and_then(|rx| rx.try_iter().last())
                && (running.preview.is_none() || running.built.is_some())
            {
                running.preview = Some(ctx.load_texture("opening-preview", image, egui::TextureOptions::LINEAR));
                running.built = Some(built);
            }
        } else {
            running.preview = None;
        }
    }
    let result = app.file_load.running.as_ref().and_then(|r| match r.rx.try_recv() {
        Ok(job) => Some(job),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Err("the file operation stopped; the current design is unchanged".into())),
    });
    if let Some(job) = result
        && let Some(running) = app.file_load.running.take()
    {
        let request = &running.request;
        match job {
            Ok(made) if request.kind == Kind::ExportDxf => {
                if !request.path.is_empty() {
                    app.set_status(crate::i18n::format("ui.saved_to", &[("path", &request.path)]), false);
                }
                request.answer(Ok(made.result));
            }
            _ if running.cancelled() => request.answer(Err("file operation cancelled".into())),
            Ok(made) if running.matches(&app.session) => {
                if request.insert() {
                    app.session.accept_computed(made.session);
                    app.big_operation = true;
                } else {
                    crate::documents::adopt(app, made.session);
                    if request.kind == Kind::Open {
                        app.home.add_recent(&request.path);
                        crate::home::install(&mut app.home, ctx, &request.path, made.image);
                    }
                }
                app.home.open = false;
                app.fit_view();
                // A caller that said which way is up has been answered: no offer to stand the part up.
                if (request.kind == Kind::Open || request.insert()) && !request.options.contains_key("up") {
                    crate::stand_up::offer(app, &request.path, &running.before, Some(&made.result));
                }
                let warnings = made.result.get("warnings").and_then(Value::as_array);
                if let Kind::Recover { .. } = request.kind {
                    recovered(app, request);
                } else {
                    app.set_status(
                        if let Some(w) = warnings.filter(|w| !w.is_empty()) {
                            format!("imported with {} warning(s): {}", w.len(), w.first().and_then(Value::as_str).unwrap_or(""))
                        } else {
                            format!("{} done", if request.insert() { "Insert" } else { "Open" })
                        },
                        false,
                    );
                }
                request.answer(Ok(made.result));
            }
            Ok(_) => {
                let e = "the active design changed while loading; the file operation was discarded";
                request.answer(Err(e.into()));
                app.set_status(e, true);
            }
            Err(e) => {
                request.answer(Err(e.clone()));
                if let Kind::Recover { id, dir } = &request.kind {
                    reoffer(app, id, dir.as_deref());
                    app.set_status(format!("could not recover {}: {e}. Discard it or choose Later", request.path), true);
                } else {
                    app.set_status(e, true);
                }
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    start(app);
    #[cfg(target_arch = "wasm32")]
    start_staged(app);
    if app.file_load.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

/// The light overlay over everything under the title bar while a file loads. The title bar,
/// its window buttons and the window's resize edges stay live; the design underneath takes no
/// pointer input until the file is in.
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(r) = &app.file_load.running else { return };
    let cancelled = r.cancelled();
    if cancelled && app.file_load.queued.is_empty() {
        return;
    }
    let name = match &r.request.kind {
        Kind::Recover { .. } => r.request.path.clone(),
        _ => std::path::Path::new(&r.request.path).file_name().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
    };
    let phase = r.phase.load(Ordering::Relaxed);
    let (done, total) = r.progress.features();
    // Long work inside one feature (a large STEP file) has progress of its own.
    let export = r.request.kind == Kind::ExportDxf;
    let work = r.progress.work().filter(|_| export || phase == 0);
    let screen = ctx.content_rect();
    // Leave the title bar to the window. The resize edges are a foreground layer above this.
    let cover = egui::Rect::from_min_max(egui::pos2(screen.left(), screen.top() + crate::toolbar::APP_BAR_HEIGHT), screen.max);
    if cover.width() <= 0.0 || cover.height() <= 0.0 {
        return;
    }
    let centre = app.viewport.rect.filter(|v| cover.contains_rect(*v)).unwrap_or(cover).center();
    // Typing goes nowhere until the design is in.
    ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    let t = crate::theme::Tokens::get();
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let id = egui::Id::new("file_loading");
    let card_id = id.with("card");
    // Above the dialogs over the design; menus, tooltips and the resize edges stay above it.
    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, id));
    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, card_id));
    egui::Area::new(id).order(egui::Order::Middle).fixed_pos(cover.min).interactable(true).show(ctx, |ui| {
        ui.set_min_size(cover.size());
        ui.interact(cover, egui::Id::new("file_loading_block"), egui::Sense::click_and_drag());
        ui.painter().rect_filled(cover, 0.0, egui::Color32::from_black_alpha(70));
    });
    let width = 340.0f32.min(cover.width() - 16.0).max(120.0);
    // Anchored at its top with Cancel in the heading row: the card grows downwards (the saved
    // preview, the progress bar) and Cancel never moves under the pointer.
    let top = egui::pos2(centre.x, (centre.y - 120.0).max(cover.top() + 8.0));
    egui::Area::new(card_id).order(egui::Order::Middle).pivot(egui::Align2::CENTER_TOP).fixed_pos(top).show(ctx, |child| {
        egui::Frame::popup(child.style()).fill(t.panel).show(child, |ui| {
            ui.set_width(width - 16.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(if cancelled && export {
                        crate::i18n::tr_label("dxf.cancelling")
                    } else if cancelled {
                        "Waiting for file operation"
                    } else {
                        match r.request.kind {
                            Kind::Insert => "Inserting geometry",
                            Kind::Open => "Opening design",
                            Kind::Recover { .. } => "Recovering design",
                            Kind::ExportDxf => crate::i18n::tr_label("dxf.exporting"),
                        }
                    })
                    .strong()
                    .size(14.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = ui.button(crate::i18n::tr("ui.cancel"));
                    crate::scenario::publish_handle("file_loading:cancel", b.rect.center());
                    cancel |= b.clicked();
                });
            });
            ui.label(egui::RichText::new(format!("{name}…")).weak());
            if let Some(preview) = r.preview.as_ref().filter(|_| !cancelled) {
                let w = ui.available_width().min(320.0);
                ui.image((preview.id(), egui::vec2(w, w * 200.0 / 320.0)));
                let note = match r.built {
                    Some(built) => format!("Built so far: {built} of {total} features."),
                    None => crate::i18n::tr_label("Saved preview. Geometry is being checked before editing.").to_string(),
                };
                ui.label(egui::RichText::new(note).small().weak());
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(if cancelled && export {
                    crate::i18n::tr_label("dxf.cancel_wait").to_string()
                } else if cancelled {
                    "Waiting for the previous operation to stop...".to_string()
                } else {
                    match (phase, work) {
                        (_, work) if r.request.kind == Kind::ExportDxf => crate::dialogs_view_dxf::progress_text(work),
                        (_, Some(("reading", d, t))) => format!(
                            "Reading the file... {:.0}% ({:.1} / {:.1} MB)",
                            d as f64 * 100.0 / t.max(1) as f64,
                            d as f64 / 1_000_000.0,
                            t as f64 / 1_000_000.0
                        ),
                        (_, Some(("parsing", d, t))) => format!("Parsing STEP... {:.0}%", d as f64 * 100.0 / t.max(1) as f64),
                        (_, Some((_, d, t))) if d < t => format!("Converting solid {} of {t}", d + 1),
                        (_, Some(_)) => "Preparing the assembly...".into(),
                        (1, _) => "Preparing the view...".into(),
                        (2, _) => "Preparing the thumbnail...".into(),
                        _ if total > 0 => format!("Rebuilding feature {} of {total}", (done + 1).min(total)),
                        _ => "Reading the file...".into(),
                    }
                });
            });
            // Always there, so the card keeps its height as the phases change.
            let fraction = if cancelled {
                0.0
            } else if let Some((_, d, t)) = work {
                d as f32 / t.max(1) as f32
            } else if total == 0 {
                0.0
            } else if phase == 0 {
                done as f32 / total as f32
            } else {
                1.0
            };
            ui.add(egui::ProgressBar::new(fraction).desired_height(6.0));
        });
    });
    if cancel && !cancelled {
        // Keep the running job until it exits: cancelling must not allow another large
        // import to overlap it in memory. The rebuild stops at its next feature and the
        // current design stays as it was.
        app.file_load.cancel();
        app.set_status(if export { crate::i18n::tr_label("dxf.cancelled") } else { "file operation cancelled" }, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxf_export_progress_keeps_frames_alive_and_never_adopts_a_worker_document() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
        h.frames(3);
        let base = h.app.session.doc.clone();
        let undo = h.app.session.undo.len();
        let cam = h.app.cam;
        let progress = Arc::new(Progress::default());
        solidcraft_engine::doc::progress::watch(progress.clone(), || {
            solidcraft_engine::kernel::watch::current().unwrap().progress("dxf_occlusion", 64, 128);
        });
        let (tx, rx) = std::sync::mpsc::channel();
        h.app.file_load.running = Some(Running {
            request: Request::new("drawing.dxf", Kind::ExportDxf),
            base: base.clone(),
            base_revision: h.app.session.revision,
            before: Vec::new(),
            rx,
            phase: Arc::new(AtomicU8::new(0)),
            progress: progress.clone(),
            preview_rx: None,
            preview: None,
            partial_rx: None,
            built: None,
            staged: None,
        });
        // No worker result is available: drawing a frame must not wait for one.
        h.frames(4);
        assert!(h.app.file_load.busy());
        let at = crate::scenario::handle_at("file_loading:cancel").unwrap();
        let (request, _reply) = crate::control::ControlRequest::new("ui.click", json!({"x":at.x,"y":at.y}));
        let _ = crate::control::handle(&mut h.app, &h.ctx, &request);
        h.frames(4);
        assert!(progress.is_cancelled());
        tx.send(Err("DXF export cancelled".into())).unwrap();
        h.frames(2);
        assert!(h.app.file_load.idle());
        assert!(Arc::ptr_eq(&base, &h.app.session.doc));
        assert_eq!(h.app.session.undo.len(), undo);
        assert_eq!(h.app.cam, cam);

        // The success path is equally read-only, even if the worker has another session.
        let mut other = Session::default();
        other.execute("file.new", &json!({"name":"Worker must not replace the current design"})).unwrap();
        let mut request = Request::new("drawing.dxf", Kind::ExportDxf);
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        request.reply = Some(reply_tx);
        let (tx, rx) = std::sync::mpsc::channel();
        h.app.file_load.running = Some(Running {
            request,
            base: base.clone(),
            base_revision: h.app.session.revision,
            before: Vec::new(),
            rx,
            phase: Arc::new(AtomicU8::new(0)),
            progress: Arc::new(Progress::default()),
            preview_rx: None,
            preview: None,
            partial_rx: None,
            built: None,
            staged: None,
        });
        tx.send(Ok(Made { session: other, result: json!({"visible":12}), image: None })).unwrap();
        h.frames(2);
        assert_eq!(reply_rx.try_recv().unwrap()["result"]["visible"], 12);
        assert!(Arc::ptr_eq(&base, &h.app.session.doc));
        assert_eq!(h.app.session.undo.len(), undo);
        assert_eq!(h.app.cam, cam);
    }

    #[test]
    fn the_overlay_shows_byte_progress_and_keeps_completed_stages_until_the_next_one() {
        let mut h = crate::scenario::Harness::new();
        let (_tx, rx) = std::sync::mpsc::channel();
        let progress = Arc::new(Progress::default());
        let phase = Arc::new(AtomicU8::new(0));
        h.app.file_load.running = Some(Running {
            request: Request::new("assembly.step", Kind::Open),
            base: h.app.session.doc.clone(),
            base_revision: h.app.session.revision,
            before: Vec::new(),
            rx,
            phase: phase.clone(),
            progress: progress.clone(),
            preview_rx: None,
            preview: None,
            partial_rx: None,
            built: None,
            staged: None,
        });
        for (stage, done, total, label) in [
            ("reading", 16_000_000, 64_000_000, "Reading the file... 25% (16.0 / 64.0 MB)"),
            ("reading", 64_000_000, 64_000_000, "Reading the file... 100% (64.0 / 64.0 MB)"),
            ("parsing", 32_000_000, 64_000_000, "Parsing STEP... 50%"),
            ("parsing", 64_000_000, 64_000_000, "Parsing STEP... 100%"),
            ("converting", 3, 10, "Converting solid 4 of 10"),
            ("converting", 10, 10, "Preparing the assembly..."),
        ] {
            solidcraft_engine::doc::progress::watch(progress.clone(), || {
                solidcraft_engine::doc::progress::work_step(stage, done, total);
            });
            h.frames(3);
            assert!(h.widget(label).is_some(), "the overlay must show {label:?}");
        }
        solidcraft_engine::doc::progress::watch(progress.clone(), || {
            let mut session = Session::default();
            session.execute("solid.box", &json!({"length": 2, "width": 3, "height": 4})).unwrap();
        });
        h.frames(3);
        assert!(h.widget("Rebuilding feature 1 of 1").is_some(), "a later feature must replace the previous file's progress");
        solidcraft_engine::doc::progress::watch(progress, || {
            solidcraft_engine::doc::progress::work_step("converting", 10, 10);
        });
        phase.store(1, Ordering::Relaxed);
        h.frames(3);
        assert!(h.widget("Preparing the view...").is_some(), "view preparation replaces the import stage");
    }

    #[test]
    fn opening_result_and_preview_cannot_overwrite_a_newer_design() {
        for (insert, change_document) in [(false, true), (true, true), (false, false)] {
            let mut app = SolveApp::new(Session::default(), Default::default());
            let base = app.session.doc.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            let (preview_tx, preview_rx) = std::sync::mpsc::channel();
            preview_tx.send(egui::ColorImage::new([2, 2], vec![egui::Color32::RED; 4])).unwrap();
            app.file_load.running = Some(Running {
                request: Request::new("old.solidcraft", if insert { Kind::Insert } else { Kind::Open }),
                base,
                base_revision: app.session.revision,
                before: Vec::new(),
                rx,
                phase: Arc::new(AtomicU8::new(0)),
                progress: Arc::new(Progress::default()),
                preview_rx: Some(preview_rx),
                preview: None,
                partial_rx: None,
                built: None,
                staged: None,
            });
            if change_document {
                app.session.execute("file.new", &serde_json::json!({"name":"Newer design"})).unwrap();
            } else {
                // An edit and undo can return to the original Arc; the generation still changed.
                app.session.revision += 1;
            }
            let newer = app.session.doc.clone();
            tick(&mut app, &egui::Context::default());
            assert!(app.file_load.running.as_ref().unwrap().preview.is_none());
            tx.send(Ok(Made { session: Session::default(), result: Value::Null, image: None })).unwrap();
            tick(&mut app, &egui::Context::default());
            assert!(Arc::ptr_eq(&newer, &app.session.doc));
            assert!(app.status.as_ref().unwrap().0.contains("discarded"));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn provisional_picture_renders_without_adopting_cached_meshes() {
        if !crate::gpu::test_picture_renderer() {
            return;
        }
        let preview = solidcraft_engine::io::DesignPreview {
            meshes: vec![solidcraft_engine::doc::MeshData {
                name: "Saved triangle".into(),
                positions: vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0],
                triangles: vec![0, 1, 2],
                color: Some([0.8, 0.2, 0.1]),
            }],
        };
        let picture = saved_picture(preview).unwrap();
        assert_eq!(picture.size, [320, 200]);
        assert!(picture.pixels.iter().any(|p| u16::from(p.r()) > u16::from(p.g()) * 2));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn repeated_large_preview_triangles_are_a_cache_miss_before_drawing() {
        let preview = solidcraft_engine::io::DesignPreview {
            meshes: vec![solidcraft_engine::doc::MeshData {
                name: "Hostile overdraw".into(),
                positions: vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0],
                triangles: (0..10_000).flat_map(|_| [0, 1, 2]).collect(),
                color: None,
            }],
        };
        assert!(saved_picture(preview).is_none());
    }
    #[test]
    fn failed_import_keeps_the_current_document() {
        let mut app = SolveApp::new(Session::default(), Default::default());
        let old = app.session.doc.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err("bad STEP".into())).unwrap();
        app.file_load.running = Some(Running {
            request: Request::new("bad.step", Kind::Insert),
            base: old.clone(),
            base_revision: app.session.revision,
            before: Vec::new(),
            rx,
            phase: Arc::new(AtomicU8::new(0)),
            progress: Arc::new(Progress::default()),
            preview_rx: None,
            preview: None,
            partial_rx: None,
            built: None,
            staged: None,
        });
        tick(&mut app, &egui::Context::default());
        assert!(Arc::ptr_eq(&app.session.doc, &old));
        assert!(!app.file_load.busy());
        assert!(app.status.as_ref().unwrap().0.contains("bad STEP"));
    }
    #[test]
    fn cancelling_discards_a_finished_result_and_keeps_the_worker_slot_until_completion() {
        let mut app = SolveApp::new(Session::default(), Default::default());
        let old = app.session.doc.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let progress = Arc::new(Progress::default());
        progress.cancel();
        app.file_load.running = Some(Running {
            request: Request::new("cancelled.step", Kind::Insert),
            base: old.clone(),
            base_revision: app.session.revision,
            before: Vec::new(),
            rx,
            phase: Arc::new(AtomicU8::new(0)),
            progress,
            preview_rx: None,
            preview: None,
            partial_rx: None,
            built: None,
            staged: None,
        });
        assert!(!app.file_load.busy());
        assert!(app.file_load.running.is_some());
        let mut computed = Session::default();
        computed.execute("solid.box", &serde_json::json!({"length":10,"width":10,"height":10})).unwrap();
        tx.send(Ok(Made { session: computed, result: Value::Null, image: None })).unwrap();
        tick(&mut app, &egui::Context::default());
        assert!(app.file_load.running.is_none());
        assert!(Arc::ptr_eq(&old, &app.session.doc));
    }

    /// #1376: the web app reads a STEP file a piece per frame. The first frame only shows the
    /// overlay, the next parses, and the solids follow, frame by frame, until the insert
    /// lands as a worker's would; Cancel between frames leaves the design as it was.
    #[test]
    fn a_staged_step_import_lands_over_frames_and_cancels_between_them() {
        let dir = std::env::temp_dir().join(format!("solidcraft-staged-step-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("boxes.step").to_string_lossy().into_owned();
        let mut part = Session::default();
        for i in 0..6 {
            part.execute("solid.box", &serde_json::json!({"length": 10, "width": 10, "height": 10, "corner": [i * 20, 0, 0]})).unwrap();
        }
        part.execute("file.export", &serde_json::json!({"path": path})).unwrap();
        let ctx = egui::Context::default();
        let mut app = SolveApp::new(Session::default(), Default::default());
        let bodies = |app: &SolveApp| app.session.world_state().bodies.len();
        assert!(staged_kind(&Request::new(&path, Kind::Insert)) && staged_kind(&Request::new(&path, Kind::Open)));
        assert!(!staged_kind(&Request::new("part.igs", Kind::Insert)) && !staged_kind(&Request::new("part.solidcraft", Kind::Open)));
        app.file_load.queued.push_back(Request::new(&path, Kind::Insert));
        start_staged(&mut app);
        tick(&mut app, &ctx);
        assert!(app.file_load.busy() && bodies(&app) == 0, "the first frame only shows the overlay");
        tick(&mut app, &ctx);
        let reading = app.file_load.running.as_ref().and_then(|r| r.staged.as_ref()).is_some_and(|s| s.reading.is_some());
        assert!(reading, "the next frame parses the file");
        let mut frames = 2;
        while app.file_load.running.is_some() && frames < 1000 {
            tick(&mut app, &ctx);
            frames += 1;
        }
        assert_eq!(bodies(&app), 6, "{:?}", app.status);
        assert!(app.status.as_ref().is_some_and(|s| s.0.contains("Insert done")), "{:?}", app.status);
        // Cancelled after the parse: nothing is inserted, and nothing waits.
        let before = app.session.doc.clone();
        app.file_load.queued.push_back(Request::new(&path, Kind::Insert));
        start_staged(&mut app);
        tick(&mut app, &ctx);
        tick(&mut app, &ctx);
        app.file_load.cancel();
        tick(&mut app, &ctx);
        assert!(app.file_load.running.is_none() && !app.file_load.busy());
        assert!(Arc::ptr_eq(&before, &app.session.doc));
        assert_eq!(bodies(&app), 6);
        // A file that isn't STEP fails like the worker's would, and the design stays.
        let junk = dir.join("junk.step").to_string_lossy().into_owned();
        std::fs::write(&junk, "not a step file").unwrap();
        app.file_load.queued.push_back(Request::new(&junk, Kind::Insert));
        start_staged(&mut app);
        for _ in 0..4 {
            tick(&mut app, &ctx);
        }
        assert!(app.file_load.running.is_none());
        assert!(app.status.as_ref().is_some_and(|s| s.2 && s.0.contains("junk.step")), "{:?}", app.status);
        assert!(Arc::ptr_eq(&before, &app.session.doc));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
