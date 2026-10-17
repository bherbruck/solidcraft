//! The SolidCraft engine.
//!
//! Every user-visible action is a command with a stable, dotted, lower-case id
//! (`solid.extrude`, `sketch.create`, `solid.fillet`, `timeline.rollback`). Commands take JSON
//! parameters and never open dialogs; the UI, the command palette, scripts, the CLI and the
//! control channel all run the same commands through [`Session::execute`].
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod capture;
pub mod cmd;
pub mod control_auth;
pub mod crash;
pub mod frames;
pub mod fuzz;
pub mod licences;
pub mod occurrences;
pub mod params;
pub mod product_scene;
pub mod recovery;
pub mod render_jobs;
pub mod sample;
mod slicer_icon;
pub mod slicers;
pub mod timing;
pub mod view;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solidcraft_doc::{Document, Model, ModelState};
use solidcraft_geom::Vec3;

pub use cmd::{CommandInfo, CommandSpec, auto_operation, command_specs, extrude_reach, find_command};
pub use solidcraft_doc as doc;
pub use solidcraft_doc::Visibility;
pub use solidcraft_geom as geom;
pub use solidcraft_io as io;
pub use solidcraft_kernel as kernel;
pub use solidcraft_render as render;
pub use solidcraft_sketch as sketch;

/// Longest accepted request line on the JSON-lines channels (MCP over stdio and the control
/// server over loopback), so both refuse runaway input at the same size.
pub const MAX_LINE: usize = 16 << 20;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`{hint}", hint = cmd::did_you_mean(.0))]
    UnknownCommand(String),
    #[error("`{0}` is not available right now: {1}")]
    Disabled(String, String),
    #[error("invalid parameters for `{cmd}`: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("{0}")]
    Other(String),
    /// A command panicked; the document was kept as it was (a bug: please report it).
    #[error("internal error in `{0}` (the design was kept as it was): {1}")]
    Internal(String, String),
}

impl From<solidcraft_doc::DocError> for EngineError {
    fn from(e: solidcraft_doc::DocError) -> Self {
        EngineError::Other(e.to_string())
    }
}
impl From<solidcraft_sketch::SketchError> for EngineError {
    fn from(e: solidcraft_sketch::SketchError) -> Self {
        EngineError::Other(e.to_string())
    }
}
impl From<solidcraft_kernel::KernelError> for EngineError {
    fn from(e: solidcraft_kernel::KernelError) -> Self {
        EngineError::Other(e.to_string())
    }
}
impl From<solidcraft_io::IoError> for EngineError {
    fn from(e: solidcraft_io::IoError) -> Self {
        EngineError::Other(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Something selected in the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Sel {
    /// An exact placement, including its containing occurrence placements.
    Occurrence {
        path: Vec<u64>,
    },
    Body {
        name: String,
    },
    /// An edge, identified by a point on it.
    Edge {
        body: String,
        index: usize,
        point: Vec3,
    },
    Face {
        body: String,
        index: usize,
        point: Vec3,
    },
    Feature {
        id: u64,
    },
    /// A sketch curve or point (by id) of the active sketch.
    SketchCurve {
        id: String,
    },
    SketchPoint {
        id: String,
    },
    /// Geometry of a different sketch, retained for projection preselection.
    ExternalSketchCurve {
        sketch: u64,
        id: String,
    },
    ExternalSketchPoint {
        sketch: u64,
        id: String,
    },
    /// A constraint or dimension (by id) of the active sketch.
    SketchConstraint {
        id: String,
    },
    /// A closed profile of a sketch (by index).
    Profile {
        sketch: u64,
        index: usize,
    },
    /// An origin plane (XY, XZ, YZ) or a construction plane (by feature name).
    Plane {
        name: String,
    },
    /// An origin axis (X, Y, Z).
    Axis {
        name: String,
    },
    /// A body vertex at a point.
    Vertex {
        body: String,
        point: Vec3,
    },
}

/// One undo step.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub label: String,
    pub doc: Arc<Document>,
    pub active_sketch: Option<u64>,
    pub pending_moves: std::collections::BTreeMap<u64, solidcraft_doc::Mat>,
}

/// A copy of a session that evaluates commands ahead of it ([`Session::ahead`]).
pub struct Ahead(Session);

impl Ahead {
    /// Run `commands` (their results stay in the shared evaluation cache).
    pub fn evaluate(self, commands: &[(String, Value)]) -> Result<()> {
        self.0.preview_in_place(commands).map(|_| ())
    }
}

/// What some commands would make, evaluated on a scratch copy of a session
/// ([`Session::preview`]). The session itself is not changed.
#[derive(Clone, Debug)]
pub struct Preview {
    /// The model now.
    pub before: Arc<ModelState>,
    /// The model after the commands.
    pub after: Arc<ModelState>,
    /// What each command replied (as `execute` returns it).
    pub replies: Vec<Value>,
}

/// Prepared world geometry depends on design/model identity and placement context, not
/// selection or the view revision. Retained Arcs prevent pointer reuse after a design closes.
#[derive(Clone)]
struct WorldCache {
    doc: Arc<Document>,
    model: Arc<solidcraft_doc::ModelState>,
    pending_moves: std::collections::BTreeMap<u64, solidcraft_doc::Mat>,
    active_component: u64,
    active_occurrence_path: Option<Vec<u64>>,
    world: Arc<solidcraft_doc::ModelState>,
    /// The moved components' bodies as placed for `world`, reused by the next world state.
    placed: Arc<std::sync::Mutex<solidcraft_doc::PlacedBodies>>,
}

impl WorldCache {
    fn matches(&self, session: &Session, model: &Arc<solidcraft_doc::ModelState>) -> bool {
        Arc::ptr_eq(&self.doc, &session.doc)
            && Arc::ptr_eq(&self.model, model)
            && self.pending_moves == session.pending_moves
            && self.active_component == session.active_component
            && self.active_occurrence_path == session.active_occurrence_path
    }
}

/// An editing session on one design.
pub struct Session {
    pub doc: Arc<Document>,
    pub model: Model,
    pub undo: Vec<Snapshot>,
    pub redo: Vec<Snapshot>,
    /// Sketch feature being edited (sketch mode).
    pub active_sketch: Option<u64>,
    pub selection: Vec<Sel>,
    pub path: Option<String>,
    /// One-shot display-only notification during doc.open, before authoritative evaluation.
    /// Interactive loaders may show it provisionally; it never becomes model state.
    pub open_preview: Option<Box<dyn Fn(solidcraft_io::DesignPreview) + Send + Sync>>,
    saved: Arc<Document>,
    /// Messages for the command line / log.
    pub log: Vec<String>,
    /// Increments on every document change (views use it to refresh).
    pub revision: u64,
    /// Project a face's edges into a sketch created on it (Fusion's default; `sketch.auto_project`).
    pub auto_project: bool,
    /// The component new sketches and features go into (0 = the root).
    pub active_component: u64,
    /// The placement used when authoring shared component geometry.
    pub active_occurrence_path: Option<Vec<u64>>,
    /// Occurrence moves not captured yet (Capture Position keeps them, a recompute drops them).
    pub pending_moves: std::collections::BTreeMap<u64, solidcraft_doc::Mat>,
    world_cache: std::sync::Mutex<Option<WorldCache>>,
    occurrence_visibility_cache: std::sync::Mutex<Option<occurrences::VisibilityCache>>,
    removed_later_cache: std::sync::Mutex<Option<occurrences::RemovedLaterCache>>,
    /// Features copied with `timeline.copy` (pasted by `timeline.paste`).
    pub clipboard: Vec<solidcraft_doc::Feature>,
    /// Sketch geometry copied with `sketch.copy` (pasted by `sketch.paste`).
    pub sketch_clipboard: Option<cmd::sketch_clipboard::SketchClip>,
    /// Section Analysis: a view cut by a plane (origin, unit normal; the normal side is hidden).
    pub section: Option<(Vec3, Vec3)>,
    /// Surface analysis shading the model (zebra, draft, curvature map).
    pub analysis: Option<SurfaceAnalysis>,
    /// The seams `inspect.continuity` last measured, for the viewport to shade. View state: not
    /// part of the design and not an undo step.
    pub continuity: Vec<ContinuitySeam>,
    /// What is hidden or shown in the view (`browser.visibility`): saved with the design, not an
    /// undo step. A design brings its saved visibility when it opens (`Session::new`).
    pub visibility: Visibility,
    /// The camera `view.apply` asked the view to use (view state, not saved). The desktop app
    /// moves its viewport there and takes it; headless renders keep using it until a standard
    /// view is chosen.
    pub camera: Option<render::Camera>,
    /// Host-owned viewport metadata, never saved or included in history.
    render_viewport: Option<Arc<render::ExecutorDescriptor>>,
    render_jobs: render_jobs::Jobs,
    /// The visibility the design file holds (what `is_dirty` compares with).
    saved_visibility: Visibility,
    /// States to go back to (`edit.checkpoint`, `edit.restore_checkpoint`): how batches of
    /// commands are undone however long the undo history is.
    pub(crate) checkpoints: Vec<Checkpoint>,
    next_checkpoint: u64,
    /// Record this session's top-level commands in the crash journal (the app's own design;
    /// never previews or other scratch sessions). See [`crash`].
    pub crash_journal: bool,
    /// Full model evaluations (`refresh`), for tests.
    #[cfg(test)]
    pub(crate) evaluations: usize,
}

/// A sketch changed by one frame of an interactive gesture (`live: true`).
#[derive(Clone, Debug)]
pub struct LiveEdit {
    pub sketch: u64,
    /// What the frame's solve found: a full solve's report, or, when the sketch was solved
    /// without a rank analysis, its last report's degrees of freedom and determined flags (a
    /// drag does not change them; the evaluation at the gesture's end brings them up to date).
    pub report: solidcraft_sketch::SolveReport,
}

/// The design, active sketch, pending positions and undo history at an `edit.checkpoint`.
#[derive(Clone, Debug)]
pub(crate) struct Checkpoint {
    pub id: u64,
    pub doc: Arc<Document>,
    pub active_sketch: Option<u64>,
    pub pending_moves: std::collections::BTreeMap<u64, solidcraft_doc::Mat>,
    pub undo: Vec<Snapshot>,
    pub redo: Vec<Snapshot>,
}

/// A way of shading the model's faces to judge their shape (view state, not part of the design).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SurfaceAnalysis {
    /// Reflected stripes: `stripes` across the view.
    Zebra { stripes: f64 },
    /// Faces by draft angle to `pull`: past +`angle` green, past −`angle` red, between yellow.
    Draft { pull: Vec3, angle: f64 },
    /// Colour by curvature: blue flat, through green, to red at 1 / `radius`.
    Curvature { radius: f64 },
    /// Mirror finish reflecting a studio (sky, horizon, floor, light panels).
    Environment,
    /// Green where a tool coming along `dir` reaches the face (facing it, nothing above), red
    /// elsewhere.
    Access { dir: Vec3 },
}

/// One seam measured by `inspect.continuity`, where the viewport shades it: the stations it was
/// sampled at, in world coordinates, and how it reads. View state only: the design is untouched.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ContinuitySeam {
    /// The body the seam is an edge of.
    pub body: String,
    /// The edge's index in the body's display mesh edge order.
    pub edge: usize,
    /// Sample points along the seam, in world coordinates.
    pub points: Vec<Vec3>,
    /// The seam's worst verdict along its length.
    pub verdict: kernel::SeamVerdict,
}

impl SurfaceAnalysis {
    /// How a picture shades the faces for this analysis.
    pub fn mode(self) -> render::AnalysisMode {
        use render::AnalysisMode as M;
        match self {
            SurfaceAnalysis::Zebra { stripes } => M::Zebra { stripes },
            SurfaceAnalysis::Draft { pull, angle } => M::Draft { pull, angle },
            SurfaceAnalysis::Curvature { radius } => M::Curvature { radius },
            SurfaceAnalysis::Environment => M::Environment,
            SurfaceAnalysis::Access { dir } => M::Access { dir },
        }
    }
}

const MAX_UNDO: usize = 200;
/// Checkpoints kept (the oldest go first).
const MAX_CHECKPOINTS: usize = 16;
const MAX_LOG: usize = 2000;

impl Default for Session {
    fn default() -> Self {
        Session::new(Document::new("Untitled"))
    }
}

impl Session {
    pub fn new(mut doc: Document) -> Self {
        // The session keeps what is hidden (not an undo step); the design file carries it.
        let visibility = std::mem::take(&mut doc.visibility);
        let doc = Arc::new(doc);
        let mut model = Model::new();
        model.evaluate(&doc);
        Session {
            saved: doc.clone(),
            doc,
            model,
            undo: Vec::new(),
            redo: Vec::new(),
            active_sketch: None,
            selection: Vec::new(),
            path: None,
            open_preview: None,
            log: Vec::new(),
            revision: 1,
            auto_project: true,
            active_component: 0,
            active_occurrence_path: None,
            pending_moves: Default::default(),
            world_cache: Default::default(),
            occurrence_visibility_cache: Default::default(),
            removed_later_cache: Default::default(),
            clipboard: Vec::new(),
            sketch_clipboard: None,
            section: None,
            analysis: None,
            continuity: Vec::new(),
            checkpoints: Vec::new(),
            next_checkpoint: 1,
            saved_visibility: visibility.clone(),
            visibility,
            camera: None,
            render_viewport: None,
            render_jobs: Default::default(),
            #[cfg(test)]
            evaluations: 0,
            crash_journal: false,
        }
    }

    /// Become `fresh` (another design opened, recovered or started), keeping the settings that
    /// are the user's rather than the design's: `auto_project` is a preference (#680).
    pub fn replace_design(&mut self, fresh: Session) {
        let auto_project = self.auto_project;
        let render_viewport = self.render_viewport.clone();
        let render_jobs = self.render_jobs.scratch();
        *self = fresh;
        self.auto_project = auto_project;
        self.render_viewport = render_viewport;
        self.render_jobs = render_jobs;
    }

    /// The design has changes not in its file: in the document, or in what is hidden.
    pub fn is_dirty(&self) -> bool {
        (!Arc::ptr_eq(&self.doc, &self.saved) && *self.doc != *self.saved) || self.visibility != self.saved_visibility
    }
    pub fn mark_saved(&mut self) {
        self.saved = self.doc.clone();
        self.saved_visibility = self.visibility.clone();
    }

    pub fn render_backend(&self) -> render::BackendDescriptor {
        let mut backend = render::builtin_backend();
        if let Some(viewport) = &self.render_viewport {
            backend.executors.push((**viewport).clone());
        }
        if let Some(executor) = &self.render_jobs.descriptor {
            let mut executor = executor.clone();
            if self.render_jobs.host.as_ref().is_none_or(|host| !host.is_available()) {
                executor.availability = render::Availability::Unavailable { reason: "CPU image host is unavailable".into() };
            }
            backend.executors.push(executor);
        }
        backend
    }

    pub fn render_viewport(&self) -> Option<&render::ExecutorDescriptor> {
        self.render_viewport.as_deref()
    }

    /// Installed hosts supply pure metadata; document commands cannot invent a device.
    pub fn set_render_viewport(&mut self, viewport: Option<render::ExecutorDescriptor>) -> Result<()> {
        if self.render_viewport.as_deref() == viewport.as_ref() {
            return Ok(());
        }
        if let Some(executor) = &viewport {
            if executor.profiles.is_empty() || executor.profiles.iter().any(|p| p.operation != render::RenderOperation::Viewport) {
                return Err(EngineError::Other("host viewport executor must contain only viewport profiles".into()));
            }
            let mut backend = render::builtin_backend();
            if let Some(executor) = &self.render_jobs.descriptor {
                backend.executors.push(executor.clone());
            }
            backend.executors.push(executor.clone());
            // Resolution validates the whole registry, including duplicate executor IDs.
            let request = render::RenderRequest {
                backend: None,
                mode: render::RenderMode::Preview,
                operation: render::RenderOperation::Image,
                width: 1,
                height: 1,
                format: Some(render::RenderFormat::Png),
                features: Vec::new(),
                policy: Default::default(),
            };
            render::resolve_request(&[backend], &request).map_err(|e| EngineError::Other(e.to_string()))?;
        }
        self.render_viewport = viewport.map(Arc::new);
        Ok(())
    }

    /// The design as a file holds it: the document with what is hidden in it.
    pub fn design(&self) -> std::borrow::Cow<'_, Document> {
        if self.visibility.is_empty() {
            return std::borrow::Cow::Borrowed(&*self.doc);
        }
        let mut doc = (*self.doc).clone();
        doc.visibility = self.visibility.clone();
        std::borrow::Cow::Owned(doc)
    }
    /// The design has changes not in its file (a recovered design).
    pub fn mark_unsaved(&mut self) {
        self.saved = Arc::new(Document::new(""));
    }

    pub fn echo(&mut self, s: impl Into<String>) {
        self.log.push(s.into());
        if self.log.len() > MAX_LOG {
            let n = self.log.len() - MAX_LOG;
            self.log.drain(..n);
        }
    }

    /// Mutable document (copy-on-write).
    pub fn doc_mut(&mut self) -> &mut Document {
        Arc::make_mut(&mut self.doc)
    }

    /// Prepared placements only: optional save caches must not compute a cold world.
    pub(crate) fn cached_world_state(&self) -> Option<Arc<solidcraft_doc::ModelState>> {
        let state = self.model.state();
        if self.doc.occurrences.is_empty() {
            return Some(state);
        }
        self.world_cache.lock().ok()?.as_ref().filter(|cache| cache.matches(self, &state)).map(|cache| cache.world.clone())
    }

    /// Re-evaluate the model after a document change.
    /// The model placed in the world through component occurrences (and uncaptured moves): what
    /// to display, measure, pick and export. The same as `model.state()` without components.
    pub fn world_state(&self) -> Arc<solidcraft_doc::ModelState> {
        let st = self.model.state();
        if self.doc.occurrences.is_empty() {
            return st;
        }
        if let Ok(c) = self.world_cache.lock()
            && let Some(cached) = c.as_ref()
            && cached.matches(self, &st)
        {
            return cached.world.clone();
        }
        // The bodies the last world state placed: unchanged ones are not transformed and meshed
        // again (a design built command by command changed one body at a time).
        let placed = self.world_cache.lock().ok().and_then(|c| c.as_ref().map(|c| c.placed.clone())).unwrap_or_default();
        // Taken out while placing, so a scratch copy of the session sharing it never waits.
        let mut reuse = placed.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default();
        let mut w = {
            if self.pending_moves.is_empty() {
                solidcraft_doc::world_state_reusing(&self.doc, &st, &mut reuse)
            } else {
                let mut d = (*self.doc).clone();
                for o in &mut d.occurrences {
                    if let Some(m) = self.pending_moves.get(&o.id) {
                        o.transform = *m;
                    }
                }
                solidcraft_doc::world_state_reusing(&d, &st, &mut reuse)
            }
        };
        if let Ok(mut p) = placed.lock() {
            *p = reuse;
        }
        // Sketch definitions are displayed once, at the placement currently being authored.
        if self.active_occurrence_path.is_some() && self.active_component != 0 {
            let first = self.occurrence_placement(&self.doc.first_path(self.active_component)).map(|p| p.world).unwrap_or(solidcraft_doc::IDENTITY);
            if let Some(inverse) = solidcraft_doc::mat_inverse(&first) {
                let delta = solidcraft_doc::mat_mul(&self.active_component_frame(), &inverse);
                for sketch in w.sketches.iter_mut().filter(|sk| self.doc.feature(sk.feature).is_some_and(|f| f.component == self.active_component)) {
                    let sketch = Arc::make_mut(sketch);
                    let plane = sketch.plane;
                    if let Some(mapped) = solidcraft_geom::Plane::new(
                        solidcraft_doc::apply_point(&delta, plane.origin),
                        solidcraft_doc::apply_vector(&delta, plane.x),
                        solidcraft_doc::apply_vector(&delta, plane.y),
                    ) {
                        sketch.plane = mapped;
                    }
                    for wire in &mut sketch.sketch.wires {
                        if wire.space == solidcraft_doc::sketch::WireSpace::Component {
                            for point in wire.pts.iter_mut().chain(&mut wire.fit) {
                                *point = solidcraft_doc::apply_point(&delta, *point);
                            }
                        }
                    }
                }
            }
        }
        let w = Arc::new(w);
        if let Ok(mut c) = self.world_cache.lock() {
            *c = Some(WorldCache {
                doc: self.doc.clone(),
                model: st,
                pending_moves: self.pending_moves.clone(),
                active_component: self.active_component,
                active_occurrence_path: self.active_occurrence_path.clone(),
                world: w.clone(),
                placed,
            });
        }
        w
    }

    pub fn refresh(&mut self) {
        self.model.evaluate(&self.doc);
        #[cfg(test)]
        {
            self.evaluations += 1;
        }
        self.revision += 1;
        self.prune_occurrence_state();
        // Joints follow the geometry their origins snap to.
        if !self.doc.assembly.joints.is_empty() {
            cmd::joints::resolve(self);
        }
        if let Some(id) = self.active_sketch
            && self.doc.feature(id).is_none()
        {
            self.active_sketch = None;
        }
    }

    /// Only persistent render intent changed. Never evaluate or clear a stale model here.
    fn render_only_change(&self, before: &Snapshot) -> bool {
        self.active_sketch == before.active_sketch && self.pending_moves == before.pending_moves && self.doc.differs_only_in_render(&before.doc)
    }

    fn refresh_render(&mut self, previous: &Arc<Document>) {
        self.revision += 1;
        // Geometry/placements have been proved identical. Retarget the cache's document key
        // instead of discarding prepared meshes merely because the document Arc changed.
        if let Ok(mut cache) = self.world_cache.lock()
            && let Some(cache) = cache.as_mut()
            && Arc::ptr_eq(&cache.doc, previous)
        {
            cache.doc = self.doc.clone();
        }
    }

    /// Show the sketch a live frame changed, leaving the features after it as they were last
    /// evaluated (marked stale); a full `refresh` when the model cannot show it that way.
    fn refresh_live(&mut self, edit: LiveEdit) {
        let sketch = self.doc.sketch(edit.sketch).ok().cloned();
        if sketch.is_some_and(|sketch| self.model.show_live_sketch(edit.sketch, sketch, edit.report)) {
            self.revision += 1;
        } else {
            self.refresh();
        }
    }

    /// The sketches whose last evaluation solved, each with its own parameter-driven values
    /// (taken before a value edit, for [`Session::settle_value_edit`]).
    pub(crate) fn solved_sketches(&self) -> SketchValues {
        self.model
            .results
            .iter()
            .filter_map(|r| r.state.sketch(r.id).filter(|k| k.report.keeps_orientation()).map(|k| (r.id, own_values(&k.sketch))))
            .collect()
    }

    /// After the document's values changed (parameters, a configuration, the units): evaluate,
    /// and refuse the edit when a sketch that solved before no longer does, or turns over,
    /// because its own parameter-driven values changed (the command fails, so `execute`
    /// restores the document's values and geometry). A sketch broken by a change upstream
    /// (a projected or linked edge moved) is not the edit's to refuse: it carries a warning,
    /// as before. Otherwise store each sketch whose values changed as solved, so the next edit
    /// continues from it (see `Model::adopt_solved_sketches`).
    pub(crate) fn settle_value_edit(&mut self, solved_before: &SketchValues) -> Result<()> {
        self.refresh();
        for r in &self.model.results {
            let (Some(before), Some(k)) = (solved_before.get(&r.id), r.state.sketch(r.id)) else { continue };
            if !k.report.keeps_orientation() && own_values(&k.sketch) != *before {
                let why = k.report.failure(|f| format!("its constraints would conflict ({f})"));
                return Err(EngineError::Other(format!("sketch `{}`: {why}; the values were not changed", r.name)));
            }
        }
        // Checked first: storing makes the document unique (a copy when the undo step shares it).
        if self.model.edited_sketches(&self.doc).next().is_some() {
            self.model.adopt_solved_sketches(Arc::make_mut(&mut self.doc));
        }
        Ok(())
    }

    /// Run a command with JSON parameters (no dialogs). Undoable commands record an undo step
    /// when they change the document; a failing or panicking command leaves it unchanged.
    ///
    /// A command that takes `live` ([`CommandSpec::live`]) run with `live: true` is one frame of
    /// an interactive gesture: the model shows its sketch's new solution and keeps every later
    /// feature's last result (stale) instead of evaluating them. Any other command that reads
    /// the model ([`CommandSpec::reads_model`]) first evaluates a stale model in full, so the
    /// gesture's end (`sketch.drag_end`, or whatever runs next after an abandoned gesture) brings
    /// the model up to date before anything reads it.
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value> {
        let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        let host_viewport = self.render_viewport.clone();
        let host_jobs = self.render_jobs.host.clone();
        let live = spec.live.filter(|_| params.get("live").and_then(Value::as_bool) == Some(true));
        if live.is_none() && spec.reads_model && self.model.is_stale() {
            self.refresh();
        }
        (spec.enabled)(self).map_err(|why| EngineError::Disabled(spec.id.to_string(), why))?;
        let before = Snapshot {
            label: spec.label.to_string(),
            doc: self.doc.clone(),
            active_sketch: self.active_sketch,
            pending_moves: self.pending_moves.clone(),
        };
        let null = Value::Object(Default::default());
        let params = if params.is_null() { &null } else { params };
        let given = params;
        // Picks in the world go into their component's frame (once: commands run by commands
        // already have them there).
        let (_nested, outermost) = frames::Nested::enter(spec.id);
        let mapped = if outermost { frames::to_local(self, spec.id, params) } else { None };
        let params = mapped.as_ref().unwrap_or(params);
        if outermost {
            crash::command_begin(self, spec.id, given);
        }
        let scope = crash::Scope::enter();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match live {
            Some(run) => run(self, params),
            None => (spec.run)(self, params).map(|v| (v, None)),
        }));
        drop(scope);
        let r = match r {
            Ok(r) => r,
            Err(p) => {
                let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                Err(EngineError::Internal(spec.id.to_string(), msg))
            }
        };
        // New/open/recover can replace the Session. Device ownership belongs to the host,
        // so its capability metadata survives those document lifecycle commands.
        self.render_viewport = host_viewport;
        if self.render_jobs.host.is_none() && host_jobs.is_some() {
            self.set_render_job_host(host_jobs)?;
        }
        if outermost && crash::enabled() {
            // A failed command leaves the design as it was (restored below).
            let failed = r.as_ref().err().map(ToString::to_string);
            let after = if failed.is_some() { &before.doc } else { &self.doc };
            // Commands that only read leave no trace (the journal keeps what a replay needs).
            let changed = !Arc::ptr_eq(&self.doc, &before.doc)
                || self.active_sketch != before.active_sketch
                || self.pending_moves != before.pending_moves
                || spec.id.starts_with("select.");
            crash::command_done(self.crash_journal, changed, &before.doc, after, spec.id, given, failed.map_or(Ok(()), Err));
        }
        let changed = !Arc::ptr_eq(&self.doc, &before.doc);
        match r {
            Ok((v, live_edit)) => {
                let v = if changed { self.with_created(spec.id, params, &before, v) } else { v };
                if changed {
                    let render_only = self.render_only_change(&before);
                    let previous_doc = before.doc.clone();
                    if spec.undoable {
                        self.undo.push(before);
                        if self.undo.len() > MAX_UNDO {
                            self.undo.remove(0);
                        }
                        self.redo.clear();
                    }
                    match live_edit {
                        Some(edit) => self.refresh_live(edit),
                        None if render_only => self.refresh_render(&previous_doc),
                        None => self.refresh(),
                    }
                }
                Ok(v)
            }
            Err(e) => {
                if changed || self.pending_moves != before.pending_moves {
                    self.doc = before.doc;
                    self.active_sketch = before.active_sketch;
                    self.pending_moves = before.pending_moves;
                    self.refresh();
                }
                Err(e)
            }
        }
    }

    /// A sketch command's result gets `created`: the ids of the points, curves, constraints and
    /// dimensions it added to its sketch (the `sketch` param, else the active sketch), so every
    /// drawing and modify command reports them the same way (see `cmd::sketch_created`). A command
    /// that runs another one (`sketch.draw.dimensioned`) gets the whole diff, replacing the nested
    /// command's narrower `created`.
    fn with_created(&self, id: &str, params: &Value, before: &Snapshot, mut v: Value) -> Value {
        if !id.starts_with("sketch.") {
            return v;
        }
        let Some(out) = v.as_object_mut() else { return v };
        let named = match params.get("sketch") {
            Some(Value::Number(n)) => self.doc.find_feature(&n.to_string()).map(|f| f.id),
            Some(Value::String(x)) => self.doc.find_feature(x).map(|f| f.id),
            _ => None,
        };
        let Some(sk) = named.or(self.active_sketch).or(before.active_sketch) else { return v };
        let Ok(after) = self.doc.sketch(sk) else { return v };
        out.insert("created".into(), cmd::sketch_created(before.doc.sketch(sk).ok(), after));
        v
    }

    /// Remember the design, active sketch, pending positions and undo history; returns the checkpoint's id.
    pub fn checkpoint(&mut self) -> u64 {
        let id = self.next_checkpoint;
        self.next_checkpoint += 1;
        self.checkpoints.push(Checkpoint {
            id,
            doc: self.doc.clone(),
            active_sketch: self.active_sketch,
            undo: self.undo.clone(),
            pending_moves: self.pending_moves.clone(),
            redo: self.redo.clone(),
        });
        if self.checkpoints.len() > MAX_CHECKPOINTS {
            self.checkpoints.remove(0);
        }
        id
    }

    /// Go back to checkpoint `id` as if nothing since had happened (the undo history too). It
    /// and later checkpoints are used up.
    pub fn restore_checkpoint(&mut self, id: u64) -> Result<()> {
        let i = self.checkpoints.iter().position(|c| c.id == id).ok_or_else(|| EngineError::Other(format!("no checkpoint {id}")))?;
        let Some(c) = self.checkpoints.drain(i..).next() else { return Err(EngineError::Other(format!("no checkpoint {id}"))) };
        let before =
            Snapshot { label: String::new(), doc: self.doc.clone(), active_sketch: self.active_sketch, pending_moves: self.pending_moves.clone() };
        self.doc = c.doc;
        self.active_sketch = c.active_sketch;
        self.pending_moves = c.pending_moves;
        self.undo = c.undo;
        self.redo = c.redo;
        self.selection.clear();
        if self.render_only_change(&before) {
            self.refresh_render(&before.doc);
        } else if !Arc::ptr_eq(&self.doc, &before.doc) || self.active_sketch != before.active_sketch || self.pending_moves != before.pending_moves {
            self.refresh();
        }
        Ok(())
    }

    /// Forget checkpoint `id` (and later ones) without going back.
    pub fn drop_checkpoint(&mut self, id: u64) {
        if let Some(i) = self.checkpoints.iter().position(|c| c.id == id) {
            self.checkpoints.truncate(i);
        }
    }

    /// A scratch copy of the session (same design and model, no history or log) to try
    /// commands on without touching this one.
    pub fn scratch(&self) -> Session {
        Session {
            doc: self.doc.clone(),
            model: self.model.clone(),
            undo: Vec::new(),
            redo: Vec::new(),
            active_sketch: self.active_sketch,
            selection: self.selection.clone(),
            path: None,
            open_preview: None,
            saved: self.saved.clone(),
            log: Vec::new(),
            revision: self.revision,
            auto_project: self.auto_project,
            active_component: self.active_component,
            active_occurrence_path: self.active_occurrence_path.clone(),
            pending_moves: self.pending_moves.clone(),
            world_cache: std::sync::Mutex::new(self.world_cache.lock().ok().and_then(|cache| cache.clone())),
            occurrence_visibility_cache: Default::default(),
            removed_later_cache: Default::default(),
            clipboard: self.clipboard.clone(),
            sketch_clipboard: self.sketch_clipboard.clone(),
            section: None,
            analysis: None,
            continuity: Vec::new(),
            checkpoints: Vec::new(),
            next_checkpoint: 1,
            visibility: self.visibility.clone(),
            camera: self.camera,
            render_viewport: self.render_viewport.clone(),
            render_jobs: self.render_jobs.scratch(),
            saved_visibility: self.saved_visibility.clone(),
            #[cfg(test)]
            evaluations: 0,
            crash_journal: false,
        }
    }

    /// Refresh UI caches when adopting a session or switching tabs, without discarding its
    /// already prepared world-space geometry. This does not change the document.
    pub fn invalidate_view(&mut self, previous_revision: u64) {
        let old = self.revision;
        self.revision = old.max(previous_revision) + 1;
    }

    /// Commit commands evaluated on a scratch copy without losing this session's file,
    /// saved state, view state, checkpoints or earlier undo steps.
    pub fn accept_computed(&mut self, mut computed: Session) {
        let previous_revision = self.revision;
        self.doc = computed.doc;
        self.model = computed.model;
        self.world_cache = computed.world_cache;
        self.undo.append(&mut computed.undo);
        if self.undo.len() > MAX_UNDO {
            self.undo.drain(..self.undo.len() - MAX_UNDO);
        }
        self.redo = computed.redo;
        self.active_sketch = computed.active_sketch;
        self.selection = computed.selection;
        self.revision = computed.revision;
        self.invalidate_view(previous_revision);
        self.active_component = computed.active_component;
        self.active_occurrence_path = computed.active_occurrence_path;
        self.pending_moves = computed.pending_moves;
        // Joints the computation hid (an import's rigid group) stay hidden.
        for j in computed.visibility.hidden_joints {
            if !self.visibility.hidden_joints.contains(&j) {
                self.visibility.hidden_joints.push(j);
            }
        }
        for message in computed.log {
            self.echo(message);
        }
    }

    /// Live preview: run `commands` (id, parameters) on a scratch copy and return the model
    /// before and after. Nothing in this session changes (document, undo history, selection).
    /// A `timeline.redefine` is shown even while the timeline is rolled back to the feature
    /// being edited.
    pub fn preview(&self, commands: &[(String, Value)]) -> Result<Preview> {
        self.scratch().preview_in_place(commands).map(|(after, replies)| Preview { before: self.model.state(), after: after.model.state(), replies })
    }

    /// The same evaluated preview in occurrence coordinates, including uncaptured moves.
    pub fn preview_world(&self, commands: &[(String, Value)]) -> Result<Preview> {
        self.scratch().preview_in_place(commands).map(|(after, replies)| Preview { before: self.world_state(), after: after.world_state(), replies })
    }

    /// World preview with each session's visibility applied, including new body placements.
    pub fn preview_visible_world(&self, commands: &[(String, Value)]) -> Result<Preview> {
        let visible = |session: &Session| {
            let world = session.world_state();
            let mut shown = (*world).clone();
            shown.bodies.retain(|body| session.body_visible(&body.name));
            Arc::new(shown)
        };
        self.scratch().preview_in_place(commands).map(|(after, replies)| Preview { before: visible(self), after: visible(&after), replies })
    }

    /// Run `commands` on a [`Session::scratch`] copy that keeps the undo and redo stacks: what
    /// they evaluate lands in the evaluation cache the copy shares with this session, so running
    /// them here afterwards finds it there. For a front end that computes a command which may
    /// take a while (undo, the history marker moved) off its window thread first.
    pub fn ahead(&self) -> Ahead {
        let mut copy = self.scratch();
        copy.undo.clone_from(&self.undo);
        copy.redo.clone_from(&self.redo);
        Ahead(copy)
    }

    fn preview_in_place(mut self, commands: &[(String, Value)]) -> Result<(Self, Vec<Value>)> {
        if commands.is_empty() {
            return Err(EngineError::Other("nothing to preview".into()));
        }
        let mut replies = Vec::with_capacity(commands.len());
        for (id, params) in commands {
            let r = self.execute(id, params)?;
            replies.push(r.clone());
            if id == "timeline.redefine"
                && let Some(fid) = r.get("feature").and_then(Value::as_u64)
                && let Some(idx) = self.doc.feature_index(fid)
                && self.doc.marker.is_some_and(|m| m <= idx)
            {
                self.doc_mut().marker = Some(idx + 1);
                self.refresh();
                if let Some(e) = self.model.result(fid).and_then(|r| r.error.clone()) {
                    return Err(EngineError::Other(e));
                }
            }
        }
        Ok((self, replies))
    }

    /// Measure one or two selections (see `inspect.measure` with `items`); errors come back as
    /// `{"error": …}`.
    pub fn measure_items(&self, items: &[Sel]) -> Value {
        let v = serde_json::to_value(items).unwrap_or(Value::Null);
        cmd::measure_items(self, &v).unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }))
    }

    /// Run a script: a JSON array of `{"command": id, "params": {...}}` (or an object with a
    /// `commands` array). Stops at the first error. Returns each command's result.
    pub fn run_script(&mut self, script: &Value) -> Result<Vec<Value>> {
        Ok(self.run_script_with(script, None)?.0)
    }

    /// [`Session::run_script`], timing each command (#1833): the results and
    /// `{"timings": [{"index", "command", "ms", "profile"?}], "total_ms"}`, with `profile` the
    /// kernel's share of each command ([`timing`]).
    pub fn run_script_timed(&mut self, script: &Value, profile: bool) -> Result<(Vec<Value>, Value)> {
        let (results, timings) = self.run_script_with(script, Some(profile))?;
        Ok((results, timings.unwrap_or(Value::Null)))
    }

    fn run_script_with(&mut self, script: &Value, profile: Option<bool>) -> Result<(Vec<Value>, Option<Value>)> {
        let list = match script {
            Value::Array(a) => a.clone(),
            Value::Object(o) => {
                o.get("commands").and_then(Value::as_array).cloned().ok_or_else(|| EngineError::Other("script: missing `commands` array".into()))?
            }
            _ => return Err(EngineError::Other("script must be a JSON array or {\"commands\": [...]}".into())),
        };
        if list.len() > 100_000 {
            return Err(EngineError::Other("script too long".into()));
        }
        let mut out = Vec::new();
        let mut steps = Vec::new();
        let clock = timing::Clock::start();
        for (i, c) in list.iter().enumerate() {
            let id = c
                .get("command")
                .or_else(|| c.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| EngineError::Other(format!("script step {}: missing `command`", i + 1)))?;
            let p = c.get("params").cloned().unwrap_or(Value::Null);
            let r = match profile {
                Some(profile) => {
                    let (r, step) = timing::time(profile, || self.execute(id, &p));
                    steps.push(step.to_json(i, id));
                    r
                }
                None => self.execute(id, &p),
            };
            out.push(r.map_err(|e| EngineError::Other(format!("step {} ({id}): {e}", i + 1)))?);
        }
        let timings = profile.map(|_| serde_json::json!({"timings": steps, "total_ms": timing::round_ms(clock.ms())}));
        Ok((out, timings))
    }
}

#[cfg(test)]
mod boolean_regression_tests;
#[cfg(test)]
mod construct_tests;
#[cfg(test)]
mod delete_face_tests;
#[cfg(test)]
mod derived_units_tests;
#[cfg(test)]
mod display_mesh_tests;
#[cfg(test)]
mod exact_tool_tests;
#[cfg(test)]
mod extent_tests;
#[cfg(test)]
mod face_name_tests;
#[cfg(test)]
mod hole_face_tests;
#[cfg(test)]
mod hole_matrix_tests;
#[cfg(test)]
mod large_assembly_tests;
#[cfg(test)]
mod measure_centre_tests;
#[cfg(test)]
mod naming_history_tests;
#[cfg(test)]
mod pattern_suppress_tests;
#[cfg(test)]
mod profile_curves_tests;
#[cfg(test)]
mod profile_refs_tests;
#[cfg(test)]
mod sketch_edit_removed_tests;
#[cfg(test)]
mod split_face_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod thread_tests;
#[cfg(test)]
mod view_bounds_tests;
#[cfg(test)]
mod world_placement_cache_tests;

/// Per sketch feature: its parameter-driven dimensions' values (see [`own_values`]).
pub(crate) type SketchValues = std::collections::BTreeMap<u64, std::collections::BTreeMap<String, f64>>;

/// The values of a sketch's driving dimensions that come from parameters: what a value edit
/// changes in it (as opposed to geometry it projects or links from upstream).
fn own_values(sk: &solidcraft_sketch::Sketch) -> std::collections::BTreeMap<String, f64> {
    sk.constraints.iter().filter(|c| c.param.is_some() && !c.driven).filter_map(|c| Some((c.id.clone(), c.kind.value()?))).collect()
}
