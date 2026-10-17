//! Live operation previews. While a feature dialog is open, its commands are evaluated on a
//! scratch copy of the session ([`solidcraft_engine::Session::preview`]), off the UI thread
//! where threads exist, and the result is drawn in place of the bodies it changes: new surface
//! tinted, removed material as a translucent ghost. The last good preview stays up while the
//! next one computes or when an input makes the feature fail (the error shows in the dialog).
//! Nothing here touches the document or the undo history.
//!
//! A feature that adds or removes a tool body (join, cut, intersect) shows only its tool. That
//! is all its preview builds: its boolean, which can take seconds (a text joined onto a part),
//! runs once, on OK, and an error it finds shows then, the dialog staying open.

use std::hash::{Hash, Hasher};
use std::sync::mpsc::{Receiver, TryRecvError};

use serde_json::Value;
use solidcraft_engine::Session;
use solidcraft_engine::doc::ModelState;
use solidcraft_engine::geom::{Aabb3, Mesh, Vec3};

use crate::SolveApp;
use crate::gpu::{GpuScene, SceneSlot};
use crate::theme::Tokens;

/// Colours a preview is drawn with (straight sRGBA).
#[derive(Clone, Copy, Debug)]
pub struct Colors {
    pub body: [u8; 4],
    pub added: [u8; 4],
    /// A body the preview creates (New Body), so it doesn't look like a join.
    pub new_body: [u8; 4],
    pub removed: [u8; 4],
    pub edge: [u8; 4],
}

impl Colors {
    pub fn from_tokens(t: &Tokens) -> Colors {
        let c = |c: egui::Color32| c.to_srgba_unmultiplied();
        Colors { body: c(t.body), added: c(t.preview_add), new_body: c(t.preview_new), removed: c(t.preview_cut), edge: c(t.body_edge) }
    }
}

pub struct SketchPreview {
    pub sketch: std::sync::Arc<solidcraft_engine::doc::SolvedSketch>,
    pub curves: Vec<usize>,
    pub points: Vec<usize>,
}

/// A computed preview: the scene to draw and the bodies it stands in for.
pub struct Built {
    pub scene: GpuScene,
    pub sketches: Vec<SketchPreview>,
    pub replaced: Vec<String>,
    /// Why the feature failed, when the scene only shows its tool body.
    pub error: Option<String>,
    /// Extent of what the preview shows (the view's depth range must cover it).
    pub bounds: Aabb3,
    /// The smallest and largest radius a fillet used when it reduced one too large for its edges
    /// (`fit: reduce`).
    pub applied: Option<(f64, f64)>,
    /// The radius spline of each edge a fillet tapered (`fit: taper`), with a point on the edge.
    pub tapered: Vec<(Vec3, solidcraft_engine::kernel::Taper)>,
    /// The reply of a previewed drive that stopped short of the asked values (the dialog says
    /// where and why, in the UI's language).
    pub stopped: Option<serde_json::Value>,
}

type Job = Result<Built, String>;

#[derive(Default)]
pub struct PreviewState {
    /// Key of the commands whose result (or error) is shown.
    shown_key: Option<u64>,
    running: Option<(u64, Receiver<Job>)>,
    /// The running job builds a tool alone (for a join, cut or intersect).
    running_tool: bool,
    /// Inputs of a joint or distance preview, excluding only its driven values.
    running_stream_key: Option<u64>,
    /// Bodies the preview stands in for (hidden in the model scene).
    pub replaced: Vec<String>,
    /// The preview scene for the GPU and its key (changes whenever the scene does).
    pub slot: SceneSlot,
    pub key: u64,
    /// Is a preview scene up?
    pub active: bool,
    pub sketches: Vec<SketchPreview>,
    /// Why the current inputs don't make a feature (the last good preview stays up).
    pub error: Option<String>,
    /// A preview is being computed.
    pub busy: bool,
    /// Extent of the preview scene.
    pub bounds: Aabb3,
    /// Time the last preview took (ms), for `ui.inspect`.
    pub ms: f64,
    started: f64,
    /// The smallest and largest radius the previewed fillet used when it reduced the one asked for.
    pub applied: Option<(f64, f64)>,
    /// The reply of a previewed drive that stopped short (see [`Built::stopped`]).
    pub stopped: Option<serde_json::Value>,
    /// The radius splines the previewed fillet tapered its edges with.
    pub tapered: Vec<(Vec3, solidcraft_engine::kernel::Taper)>,
    straight: crate::extrude_preview::StraightPreview,
}

impl PreviewState {
    fn clear(&mut self) {
        self.straight.clear();
        self.running = None;
        self.running_tool = false;
        self.running_stream_key = None;
        self.shown_key = None;
        self.error = None;
        self.applied = None;
        self.stopped = None;
        self.tapered.clear();
        self.sketches.clear();
        self.busy = false;
        if self.active || !self.replaced.is_empty() {
            self.active = false;
            self.bounds = Aabb3::EMPTY;
            self.replaced.clear();
            self.set_scene(GpuScene::default());
        }
    }

    /// Drop the preview of a design that leaves the window. Its scene goes too: the viewport
    /// keeps drawing the last scene it took until the slot it reads hands it another, so a fresh
    /// state (another slot, empty) would leave the old preview drawn over the next design.
    pub fn reset(&mut self) {
        let (slot, key) = (std::mem::take(&mut self.slot), self.key);
        *self = PreviewState { slot, key, ..PreviewState::default() };
        self.set_scene(GpuScene::default());
    }

    fn set_scene(&mut self, sc: GpuScene) {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(sc);
        }
        self.key = self.key.wrapping_add(1);
    }

    fn apply(&mut self, key: u64, job: Job) {
        self.shown_key = Some(key);
        self.ms = crate::now_ms() - self.started;
        match job {
            Ok(b) => {
                self.sketches = b.sketches;
                self.replaced = b.replaced;
                self.bounds = b.bounds;
                self.active = true;
                self.error = b.error;
                self.applied = b.applied;
                self.stopped = b.stopped;
                self.tapered = b.tapered;
                self.set_scene(b.scene);
            }
            Err(e) => {
                self.sketches.clear();
                self.error = Some(e);
                self.applied = None;
                self.stopped = None;
                self.tapered.clear();
            }
        }
    }

    /// Errors validate only the current inputs. Keep the last good geometry while
    /// computing new inputs, but never let an obsolete result block their Apply.
    fn poll(&mut self, key: u64, stream_key: Option<u64>) {
        if self.shown_key != Some(key) {
            self.sketches.clear();
            self.error = None;
        }
        if let Some((k, rx)) = &self.running {
            let k = *k;
            let job = match rx.try_recv() {
                Ok(job) => Some(job),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("the preview failed".into())),
            };
            if let Some(job) = job {
                self.running = None;
                let good = matches!(&job, Ok(b) if b.error.is_none());
                let intermediate = good && (self.running_tool || stream_key.is_some() && self.running_stream_key == stream_key);
                self.running_stream_key = None;
                self.running_tool = false;
                // A held slider, a dragged arrow or an animation may advance faster than the
                // worker. Show completed joint/distance results from the same setup, and tools built for
                // inputs since changed, then compute the latest. Errors still require an exact
                // key.
                if k == key || intermediate {
                    self.apply(k, job);
                }
            }
        }
    }
}

/// Joint motion can stream successful intermediate poses while values change. Include
/// every other input, the document revision and colours so a different joint, changed
/// limits, feature dialog or document cannot reuse an obsolete pose.
fn motion_key(cmds: &[(String, Value)], revision: u64, colors: Colors) -> Option<u64> {
    if !cmds.iter().any(|(c, p)| c == "joint.drive" || c == "joint.create" && p.get("values").is_some())
        || !cmds.iter().all(|(c, _)| {
            matches!(
                c.as_str(),
                "joint.drive"
                    | "joint.create"
                    | "joint.as_built"
                    | "joint.edit"
                    | "joint.limits"
                    | "joint.gear"
                    | "joint.rack_pinion"
                    | "joint.screw"
                    | "joint.cam"
            )
        })
    {
        return None;
    }
    let mut setup = cmds.to_vec();
    for (command, params) in &mut setup {
        if matches!(command.as_str(), "joint.drive" | "joint.create")
            && let Some(p) = params.as_object_mut()
        {
            p.remove("values");
            if command == "joint.drive" {
                p.remove("value");
            }
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&setup).ok()?.hash(&mut h);
    revision.hash(&mut h);
    format!("{colors:?}").hash(&mut h);
    Some(h.finish())
}

/// Tool previews already stream in `poll`. New-body extrusions and face offsets instead
/// build their full result; allow successful intermediate distances only within the same
/// setup. Preserve selections, feature identity, extent mode, revision and colours. The
/// exact job key still ensures the latest distance is computed and errors never go stale.
fn stream_key(cmds: &[(String, Value)], revision: u64, colors: Colors) -> Option<u64> {
    if let Some(key) = motion_key(cmds, revision, colors) {
        return Some(key);
    }
    if cmds.is_empty() || as_new_body(cmds).is_some() {
        return None;
    }
    let mut setup = cmds.to_vec();
    for (command, params) in &mut setup {
        let (command, params) = if command == "timeline.redefine" {
            let command = params.get("command")?.as_str()?.to_string();
            (command, params.get_mut("params")?)
        } else {
            (command.clone(), params)
        };
        let fields: &[&str] = match command.as_str() {
            "solid.extrude" => &["distance", "distance2", "to_offset"],
            "solid.offset_face" => &["distance"],
            _ => return None,
        };
        let params = params.as_object_mut()?;
        for field in fields {
            // Keep presence: changing extent mode must not reuse the old result.
            if let Some(value) = params.get_mut(*field) {
                *value = Value::Null;
            }
        }
    }
    Some(job_key(&setup, revision, colors))
}

/// Keep the preview in step with the open dialog: start a computation when its inputs changed,
/// take finished results. Called once per frame.
pub fn update(app: &mut SolveApp, ctx: &egui::Context) {
    let cmds = match app.dialog.as_ref().filter(|d| d.previews()).map(|d| crate::dialogs::preview_commands(app, d)) {
        Some(Ok(c)) if !c.is_empty() => c,
        _ if crate::sketch_attachment_ui::commands(app).is_some() => crate::sketch_attachment_ui::commands(app).unwrap_or_default(),
        // A parameter being edited in Change Parameters.
        _ if app.params.as_ref().and_then(|p| p.preview_commands()).is_some() => {
            app.params.as_ref().and_then(|p| p.preview_commands()).unwrap_or_default()
        }
        // No dialog, or inputs still missing: nothing to preview.
        _ => {
            app.preview.clear();
            return;
        }
    };
    let colors = Colors::from_tokens(&Tokens::get());
    let key = job_key(&cmds, app.session.revision, colors);
    let stream_key = stream_key(&cmds, app.session.revision, colors);
    let tool = as_new_body(&cmds).is_some();
    let pv = &mut app.preview;
    pv.poll(key, stream_key);
    if pv.running.is_none() && pv.shown_key != Some(key) {
        pv.started = crate::now_ms();
        pv.running_stream_key = stream_key;
        let mut straight = std::mem::take(&mut app.preview.straight);
        let job = straight.compute(app, &cmds, colors);
        app.preview.straight = straight;
        if let Some(job) = job {
            app.preview.apply(key, job);
            app.preview.busy = false;
            return;
        }
        let scratch = app.session.scratch();
        spawn(&mut app.preview, key, scratch, cmds, colors, tool);
    }
    app.preview.busy = app.preview.running.is_some();
    if app.preview.busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

/// What tells one preview computation from another: its commands, the document revision they
/// run on and the colours.
fn job_key(cmds: &[(String, Value)], revision: u64, colors: Colors) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(cmds).unwrap_or_default().hash(&mut h);
    revision.hash(&mut h);
    format!("{colors:?}").hash(&mut h);
    h.finish()
}

/// Start computing the preview of `cmds`: for a tool feature (`tool`), only its tool.
#[cfg(not(target_arch = "wasm32"))]
fn spawn(pv: &mut PreviewState, key: u64, scratch: Session, cmds: Vec<(String, Value)>, colors: Colors, tool: bool) {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("preview".into()).spawn(move || {
        let _ = tx.send(if tool { compute_tool(&scratch, &cmds, colors) } else { compute(&scratch, &cmds, colors) });
    });
    match spawned {
        Ok(_) => {
            pv.running = Some((key, rx));
            pv.running_tool = tool;
        }
        Err(e) => pv.apply(key, Err(e.to_string())),
    }
}

/// No threads on the web: compute right away.
#[cfg(target_arch = "wasm32")]
fn spawn(pv: &mut PreviewState, key: u64, scratch: Session, cmds: Vec<(String, Value)>, colors: Colors, tool: bool) {
    pv.apply(key, if tool { compute_tool(&scratch, &cmds, colors) } else { compute(&scratch, &cmds, colors) });
}

/// Evaluate the commands on the scratch session and build the preview scene.
/// Features that add or remove a tool body (extrude, revolve, primitives with join, cut or
/// intersect) show the tool itself: tinted for join, see-through red for cut. Other features
/// show their result in place of the bodies they change. When the feature fails (a boolean the
/// kernel can't do yet), the tool body is still shown, with the error.
pub fn compute(s: &Session, cmds: &[(String, Value)], colors: Colors) -> Job {
    if cmds.iter().any(|(command, params)| {
        matches!(command.as_str(), "sketch.redefine" | "sketch.mirror" | "sketch.pattern.rectangular" | "sketch.pattern.circular" | "sketch.offset")
            || command == "construct.plane.along_path"
            || params.get("command").and_then(Value::as_str) == Some("construct.plane.along_path")
    }) {
        let preview = s.preview_visible_world(cmds).map_err(|error| error.to_string())?;
        return std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut built = build(&preview.before, &preview.after, colors);
            built.bounds = state_bounds(&preview.after);
            built
        }))
        .map_err(|_| "the preview failed".to_string());
    }
    // Joints and drives move occurrences: shown on the placed model.
    if let Some(job) = crate::dialogs_assembly::world_preview(s, cmds, colors) {
        return job;
    }
    match as_new_body(cmds) {
        Some(_) => {
            let mut b = compute_tool(s, cmds, colors)?;
            b.error = s.preview(cmds).err().map(|e| e.to_string());
            Ok(b)
        }
        None => {
            let p = s.preview(cmds).map_err(|e| e.to_string())?;
            let guard =
                |f: &dyn Fn() -> Built| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| "the preview failed".to_string());
            let mut b = guard(&|| build(&p.before, &p.after, colors))?;
            b.bounds = state_bounds(&p.after);
            // The radius a previewed fillet used when it reduced the one asked for (a new
            // fillet or an edited one: the feature the replies name).
            let features = || p.replies.iter().filter_map(|r| r.get("feature").and_then(Value::as_u64));
            b.applied = features().find_map(|id| p.after.fitted_range(id));
            b.tapered = features().find_map(|id| p.after.tapered(id)).map(<[_]>::to_vec).unwrap_or_default();
            Ok(b)
        }
    }
}

/// The tool body of a feature that adds or removes one, built on its own (as a new body): what
/// its live preview shows, without the boolean. When even the tool fails, the feature's own
/// error. Other features: their whole preview ([`compute`]).
fn compute_tool(s: &Session, cmds: &[(String, Value)], colors: Colors) -> Job {
    let Some((alt, op)) = as_new_body(cmds) else { return compute(s, cmds, colors) };
    let tool = match s.preview(&alt) {
        Ok(t) => t,
        Err(e) => return Err(s.preview(cmds).err().map_or_else(|| e.to_string(), |r| r.to_string())),
    };
    let mut b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_tool(&tool.before, &tool.after, colors, &op)))
        .map_err(|_| "the preview failed".to_string())?;
    b.bounds = state_bounds(&tool.after);
    Ok(b)
}

fn extrusion_params<'a>(id: &str, params: &'a Value) -> Option<&'a Value> {
    match id {
        "solid.extrude" => Some(params),
        "timeline.redefine" if params.get("command").and_then(Value::as_str) == Some("solid.extrude") => params.get("params"),
        _ => None,
    }
}

/// Check references without intersecting the tool with the target's geometry.
pub(crate) fn extrusion_target_error(s: &Session, cmds: &[(String, Value)], before: &ModelState) -> Option<String> {
    for (id, params) in cmds {
        let params = extrusion_params(id, params)?;
        let op = params.get("operation").and_then(Value::as_str).unwrap_or("new");
        if op == "new" {
            continue;
        }
        let targets = params
            .get("targets")
            .and_then(Value::as_array)
            .filter(|targets| !targets.is_empty())
            .or_else(|| params.get("participants").and_then(Value::as_array));
        let mut found = false;
        if let Some(targets) = targets.filter(|targets| !targets.is_empty()) {
            for target in targets.iter().filter_map(Value::as_str) {
                if target == "*" {
                    found |= !before.bodies.is_empty();
                } else if before.body(target).is_some() {
                    found = true;
                } else {
                    return Some(format!("unknown body `{target}`"));
                }
            }
        } else {
            found = params.get("faces").and_then(|_| params.get("face_body")).and_then(Value::as_str).map_or_else(
                || before.bodies.iter().any(|body| s.doc.model_body_component(body) == s.active_component),
                |name| s.world_state().body(name).is_some(),
            );
        }
        if !found && matches!(op, "cut" | "intersect") {
            return Some("there is no body to cut or intersect".into());
        }
    }
    None
}

fn state_bounds(st: &ModelState) -> Aabb3 {
    st.bodies.iter().fold(Aabb3::EMPTY, |b, x| b.union(&x.mesh().bounds()))
}

/// The tool bodies a join/cut/intersect adds: opaque and tinted for join, see-through for cut
/// (red) and intersect.
fn build_tool(before: &ModelState, after: &ModelState, c: Colors, op: &str) -> Built {
    let mut sc = GpuScene::default();
    for a in after.bodies.iter().filter(|a| before.body(&a.name).is_none()) {
        let m = a.mesh();
        for t in &m.triangles {
            for k in t {
                let i = *k as usize;
                if let (Some(p), Some(n)) = (m.positions.get(i), m.normals.get(i)) {
                    match op {
                        "cut" => sc.xray_tri(p.to_f32(), n.to_f32(), c.removed),
                        "intersect" => sc.xray_tri(p.to_f32(), n.to_f32(), [c.added[0], c.added[1], c.added[2], c.removed[3]]),
                        _ => sc.tri(p.to_f32(), n.to_f32(), c.added),
                    }
                }
            }
        }
        let edge = if op == "cut" { [c.removed[0], c.removed[1], c.removed[2], 255] } else { c.edge };
        for (ei, e) in m.edges.iter().enumerate() {
            if m.shaded_edge_visible(ei) {
                for w in e.windows(2) {
                    sc.line(w[0].to_f32(), w[1].to_f32(), edge, 1.3, op != "join");
                }
            }
        }
    }
    Built {
        scene: sc,
        sketches: Vec::new(),
        replaced: Vec::new(),
        error: None,
        bounds: Aabb3::EMPTY,
        applied: None,
        tapered: Vec::new(),
        stopped: None,
    }
}

/// The same commands making a new body instead of joining, cutting or intersecting, and the
/// operation they had.
fn as_new_body(cmds: &[(String, Value)]) -> Option<(Vec<(String, Value)>, String)> {
    let mut out = cmds.to_vec();
    let mut changed: Option<String> = None;
    for (command, p) in &mut out {
        let inner = p.get("command").and_then(Value::as_str).map(str::to_string);
        // A combine's operation is between bodies that already exist: it has no new-body form
        // (and no tool body of its own to show).
        if inner.as_deref().unwrap_or(command.as_str()) == "solid.combine" {
            continue;
        }
        let p = if p.get("command").is_some() { p.get_mut("params")? } else { p };
        if let Some(op) = p.get_mut("operation")
            && let Some(o) = op.as_str().filter(|o| *o != "new").map(str::to_string)
        {
            *op = Value::from("new");
            changed = Some(o);
        }
    }
    changed.map(|op| (out, op))
}

fn same(a: &Mesh, b: &Mesh) -> bool {
    a.triangles.len() == b.triangles.len() && a.positions == b.positions
}

/// The preview scene: changed and new bodies (new surface tinted) and, as a ghost, the old
/// version of changed or removed bodies.
pub fn build(before: &ModelState, after: &ModelState, c: Colors) -> Built {
    let mut sc = GpuScene::default();
    let mut replaced = Vec::new();
    for construct in &after.construct {
        if before.construct.iter().any(|old| old == construct) {
            continue;
        }
        if let solidcraft_engine::doc::construct::ConstructGeom::Plane(plane) = construct.geom {
            let (o, x, y) = (plane.origin, plane.x * 10.0, plane.y * 10.0);
            let quad = [o - x - y, o + x - y, o + x + y, o - x + y];
            let color = [c.added[0], c.added[1], c.added[2], 64];
            for triangle in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                for p in triangle {
                    sc.xray_tri(p.to_f32(), plane.normal().to_f32(), color);
                }
            }
            for (a, b) in quad.iter().zip(quad.iter().cycle().skip(1)).take(4) {
                sc.line(a.to_f32(), b.to_f32(), c.added, 1.3, true);
            }
        }
    }
    for a in &after.bodies {
        let am = a.mesh();
        let old = before.body(&a.name).map(|b| b.mesh());
        if old.as_ref().is_some_and(|bm| same(bm, &am)) {
            continue;
        }
        let nf = am.tri_face.iter().max().map_or(0, |m| *m as usize + 1);
        let added: Vec<bool> = match &old {
            Some(bm) => new_faces(&am, nf, bm),
            None => vec![true; nf],
        };
        // Material removed from a body (a hole, a pocket): its new faces also show through the
        // body in the cut colour, so the cut is seen from any side.
        let cut = old.as_ref().is_some_and(|bm| am.measure().volume < bm.measure().volume - 1e-6);
        for (t, f) in am.triangles.iter().zip(&am.tri_face) {
            let new = added.get(*f as usize).copied().unwrap_or(true);
            let col = match (new, old.is_some()) {
                (false, _) => c.body,
                (true, true) => c.added,
                (true, false) => c.new_body,
            };
            for k in t {
                let i = *k as usize;
                if let (Some(p), Some(n)) = (am.positions.get(i), am.normals.get(i)) {
                    sc.tri(p.to_f32(), n.to_f32(), col);
                    if new && cut {
                        sc.xray_tri(p.to_f32(), n.to_f32(), c.removed);
                    }
                }
            }
        }
        for (ei, e) in am.edges.iter().enumerate() {
            if !am.shaded_edge_visible(ei) {
                continue;
            }
            for w in e.windows(2) {
                sc.line(w[0].to_f32(), w[1].to_f32(), c.edge, 1.3, false);
            }
        }
        if old.is_some() {
            replaced.push(a.name.clone());
        }
    }
    for b in &before.bodies {
        let gone = after.body(&b.name).is_none();
        if !gone && !replaced.contains(&b.name) {
            continue;
        }
        let m = b.mesh();
        for t in &m.triangles {
            for k in t {
                let i = *k as usize;
                if let (Some(p), Some(n)) = (m.positions.get(i), m.normals.get(i)) {
                    sc.ghost_tri(p.to_f32(), n.to_f32(), c.removed);
                }
            }
        }
        if gone {
            replaced.push(b.name.clone());
        }
    }
    let sketches = after
        .sketches
        .iter()
        .filter_map(|sketch| {
            let old = before.sketch(sketch.feature);
            let old_curves = old.into_iter().flat_map(|s| s.sketch.curves.iter().map(|c| c.id.as_str())).collect::<std::collections::HashSet<_>>();
            let old_points = old.into_iter().flat_map(|s| s.sketch.points.iter().map(|p| p.id.as_str())).collect::<std::collections::HashSet<_>>();
            let indices = sketch
                .sketch
                .curves
                .iter()
                .enumerate()
                .filter_map(|(index, curve)| {
                    let unchanged = old_curves.contains(curve.id.as_str());
                    if unchanged { None } else { Some(index) }
                })
                .collect::<Vec<_>>();
            let points = sketch
                .sketch
                .points
                .iter()
                .enumerate()
                .filter_map(|(i, p)| if old_points.contains(p.id.as_str()) { None } else { Some(i) })
                .collect::<Vec<_>>();
            if indices.is_empty() && points.is_empty() { None } else { Some(SketchPreview { sketch: sketch.clone(), curves: indices, points }) }
        })
        .collect();
    Built { scene: sc, sketches, replaced, error: None, bounds: Aabb3::EMPTY, applied: None, tapered: Vec::new(), stopped: None }
}

/// Which of the `nf` faces of `m` are new surface, i.e. not on the old mesh? Each is decided by
/// a vote of points on it (triangle centroids spread over the face). One pass over `m`'s
/// triangles gathers the centroids, and the old mesh is indexed once, so the cost stays
/// near-linear however many faces there are (a modeled thread has thousands).
fn new_faces(m: &Mesh, nf: usize, old: &Mesh) -> Vec<bool> {
    let mut cents: Vec<Vec<Vec3>> = vec![Vec::new(); nf];
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        if let (Some(face), Some([a, b, c])) = (cents.get_mut(*f as usize), m.tri(t)) {
            face.push((a + b + c) / 3.0);
        }
    }
    let index = SurfaceIndex::new(old);
    cents
        .iter()
        .map(|cents| {
            let Some(index) = &index else { return true };
            if cents.is_empty() {
                return true;
            }
            let step = cents.len().div_ceil(9);
            let samples: Vec<&Vec3> = cents.iter().step_by(step.max(1)).collect();
            let off = samples.iter().filter(|p| !index.contains(***p)).count();
            off * 3 > samples.len()
        })
        .collect()
}

/// The triangles of a mesh in a uniform grid, to ask whether a point lies on the surface (within
/// its tessellation tolerance) by looking only at the triangles near it.
struct SurfaceIndex {
    tris: Vec<[Vec3; 3]>,
    tol: f64,
    lo: [f64; 3],
    cell: [f64; 3],
    res: usize,
    cells: Vec<Vec<u32>>,
}

impl SurfaceIndex {
    /// Most triangle-in-cell entries the grid may hold; a coarser grid is used beyond it.
    const BUDGET: usize = 4_000_000;

    /// None for a mesh with no surface.
    fn new(m: &Mesh) -> Option<SurfaceIndex> {
        let bb = m.bounds();
        if bb.is_empty() {
            return None;
        }
        let tol = (bb.diagonal() * 2e-3).max(1e-4);
        let tris: Vec<[Vec3; 3]> = m.triangles.iter().filter_map(|t| m.tri(t)).collect();
        let lo = [bb.min.x - tol, bb.min.y - tol, bb.min.z - tol];
        let ext = [bb.max.x - bb.min.x + 2.0 * tol, bb.max.y - bb.min.y + 2.0 * tol, bb.max.z - bb.min.z + 2.0 * tol];
        let mut res = ((tris.len() as f64).cbrt().round() as usize).clamp(1, 48);
        loop {
            let cell = [ext[0] / res as f64, ext[1] / res as f64, ext[2] / res as f64];
            let mut index = SurfaceIndex { tris: Vec::new(), tol, lo, cell, res, cells: vec![Vec::new(); res * res * res] };
            let mut entries = 0usize;
            'fill: for (i, t) in tris.iter().enumerate() {
                let (a, b) = index.range(t);
                for x in a[0]..=b[0] {
                    for y in a[1]..=b[1] {
                        for z in a[2]..=b[2] {
                            entries += 1;
                            if entries > Self::BUDGET && res > 1 {
                                break 'fill;
                            }
                            if let Some(c) = index.cells.get_mut((x * res + y) * res + z) {
                                c.push(i as u32);
                            }
                        }
                    }
                }
            }
            if entries <= Self::BUDGET || res == 1 {
                index.tris = tris;
                return Some(index);
            }
            res /= 2;
        }
    }

    fn cell_of(&self, v: f64, axis: usize) -> usize {
        let (lo, cell) = (self.lo.get(axis).copied().unwrap_or(0.0), self.cell.get(axis).copied().unwrap_or(1.0));
        let k = ((v - lo) / cell).floor();
        if k.is_finite() { (k.max(0.0) as usize).min(self.res - 1) } else { 0 }
    }

    /// The cells (inclusive corners) a triangle, grown by the tolerance, touches.
    fn range(&self, t: &[Vec3; 3]) -> ([usize; 3], [usize; 3]) {
        let (mut a, mut b) = ([0; 3], [0; 3]);
        for axis in 0..3 {
            let c = t.map(|p| [p.x, p.y, p.z][axis]);
            let (lo, hi) = (c[0].min(c[1]).min(c[2]) - self.tol, c[0].max(c[1]).max(c[2]) + self.tol);
            a[axis] = self.cell_of(lo, axis);
            b[axis] = self.cell_of(hi, axis);
        }
        (a, b)
    }

    /// Does `p` lie on the surface?
    fn contains(&self, p: Vec3) -> bool {
        let (x, y, z) = (self.cell_of(p.x, 0), self.cell_of(p.y, 1), self.cell_of(p.z, 2));
        let Some(cell) = self.cells.get((x * self.res + y) * self.res + z) else { return false };
        cell.iter().filter_map(|i| self.tris.get(*i as usize)).any(|[a, b, c]| point_triangle_dist(p, *a, *b, *c) < self.tol)
    }
}

pub fn point_triangle_dist(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f64 {
    let n = (b - a).cross(c - a);
    let Some(nn) = n.normalized() else { return f64::INFINITY };
    let h = (p - a).dot(nn);
    let q = p - nn * h;
    let inside = [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).cross(q - *u).dot(n) >= -1e-12);
    if inside { h.abs() } else { [(a, b), (b, c), (c, a)].iter().map(|(u, v)| p.dist_to_segment(*u, *v)).fold(f64::INFINITY, f64::min) }
}

/// The same computed sketch preview is drawn over both native GPU and CPU viewports.
pub fn paint_sketches(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    if !app.dialog.as_ref().is_some_and(|d| matches!(d.kind, crate::dialogs::Kind::SketchModify(_)) && d.previews()) {
        return;
    }
    let stroke = egui::Stroke::new(2.5, Tokens::get().preview_sketch);
    let mut budget = solidcraft_engine::render::SamplingBudget::new(32_768, 65_536);
    let mut curves = 0;
    for preview in &app.preview.sketches {
        let sketch = &preview.sketch;
        for index in &preview.curves {
            crate::tools::paint_curve(&sketch.sketch, *index, &sketch.plane, painter, proj, stroke, &mut budget);
            curves += 1;
        }
        for index in &preview.points {
            if let Some(point) = sketch.sketch.point(*index).and_then(|p| proj.to_screen(sketch.plane.to_world(p))) {
                painter.circle_filled(point, 3.0, stroke.color);
            }
        }
    }
    crate::scenario::publish_count("sketch_preview_curves", curves as f64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn colors() -> Colors {
        Colors { body: [1, 1, 1, 255], added: [2, 2, 2, 255], new_body: [4, 4, 4, 255], removed: [3, 3, 3, 100], edge: [0, 0, 0, 255] }
    }

    #[test]
    fn correcting_inputs_clears_old_errors_and_discards_late_old_jobs() {
        let mut pv = PreviewState {
            shown_key: Some(1),
            error: Some("unknown_angle".into()),
            active: true,
            replaced: vec!["Turn".into()],
            key: 9,
            ..PreviewState::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        pv.running = Some((1, rx));
        pv.poll(2, None);
        assert!(pv.error.is_none(), "the corrected inputs can Apply while their preview computes");
        assert!(pv.running.is_some());
        assert!(pv.active);
        assert_eq!(pv.replaced, ["Turn"]);
        assert_eq!(pv.key, 9, "the last good geometry stays visible");
        tx.send(Err("unknown_angle".into())).unwrap();
        pv.poll(2, None);
        assert!(pv.error.is_none(), "a late obsolete result cannot disable Apply again");
        assert!(pv.running.is_none());
        assert_eq!(pv.shown_key, Some(1));
        assert_eq!(pv.key, 9);

        let (tx, rx) = std::sync::mpsc::channel();
        pv.running = Some((2, rx));
        tx.send(Err("current invalid expression".into())).unwrap();
        pv.poll(2, None);
        assert_eq!(pv.error.as_deref(), Some("current invalid expression"));
        assert_eq!(pv.shown_key, Some(2));
        pv.poll(2, None);
        assert_eq!(pv.error.as_deref(), Some("current invalid expression"), "current errors still prevent invalid Apply");
    }

    fn pose() -> Built {
        Built {
            scene: GpuScene::default(),
            sketches: Vec::new(),
            replaced: vec!["Lid".into()],
            error: None,
            bounds: Aabb3::EMPTY,
            applied: None,
            tapered: Vec::new(),
            stopped: None,
        }
    }

    /// The worker completes while the slider is still held or Play keeps advancing.
    /// Intermediate successful poses must reach the viewport, and the final value
    /// must still be computed rather than treating that intermediate pose as current.
    #[test]
    fn joint_motion_shows_completed_poses_while_inputs_keep_advancing() {
        let mut pv = PreviewState::default();
        for (running, wanted) in [(1, 2), (2, 3), (3, 4)] {
            let (tx, rx) = std::sync::mpsc::channel();
            pv.running = Some((running, rx));
            pv.running_stream_key = Some(42);
            pv.poll(wanted, Some(42));
            assert!(pv.running.is_some());
            tx.send(Ok(pose())).unwrap();
            pv.poll(wanted, Some(42));
            assert!(pv.active, "motion should be visible before input stops");
            assert_eq!(pv.replaced, ["Lid"]);
            assert_eq!(pv.key, running, "every completed pose reaches the scene");
            assert_eq!(pv.shown_key, Some(running));
            assert_ne!(pv.shown_key, Some(wanted), "the latest pose still needs a job");
        }
        let (tx, rx) = std::sync::mpsc::channel();
        pv.running = Some((4, rx));
        pv.running_stream_key = Some(42);
        tx.send(Ok(pose())).unwrap();
        pv.poll(4, Some(42));
        assert_eq!(pv.shown_key, Some(4), "release catches up to the exact final pose");
        assert!(pv.running.is_none());
    }

    #[test]
    fn intermediate_motion_rejects_other_setups_and_obsolete_errors() {
        for (context, job) in [
            (None, Ok(pose())),
            (Some(43), Ok(pose())),
            (Some(42), Err("old invalid value".into())),
            (Some(42), Ok(Built { error: Some("old tool preview error".into()), ..pose() })),
        ] {
            let mut pv = PreviewState { running_stream_key: Some(42), ..PreviewState::default() };
            let (tx, rx) = std::sync::mpsc::channel();
            pv.running = Some((1, rx));
            tx.send(job).unwrap();
            pv.poll(2, context);
            assert!(!pv.active);
            assert!(pv.error.is_none(), "obsolete errors must not block Apply");
            assert!(pv.running.is_none());
        }
    }

    #[test]
    fn motion_context_changes_only_when_the_joint_setup_changes() {
        let commands = |angle| {
            vec![
                ("joint.edit".into(), json!({"joint": 1, "flip": true})),
                ("joint.limits".into(), json!({"joint": 1, "min": "-135 deg", "max": "0 deg"})),
                ("joint.drive".into(), json!({"joint": 1, "values": [angle]})),
            ]
        };
        let key = motion_key(&commands("-30 deg"), 7, colors());
        assert!(key.is_some());
        assert_eq!(key, motion_key(&commands("-60 deg"), 7, colors()));
        assert_ne!(key, motion_key(&commands("-60 deg"), 8, colors()));
        let mut different = commands("-60 deg");
        different[2].1["joint"] = json!(2);
        assert_ne!(key, motion_key(&different, 7, colors()));
        different = commands("-60 deg");
        different[1].1["min"] = json!("-90 deg");
        assert_ne!(key, motion_key(&different, 7, colors()));
        different = commands("-60 deg");
        different[0].1["flip"] = json!(false);
        assert_ne!(key, motion_key(&different, 7, colors()));
        let mut other_colors = colors();
        other_colors.body = [9, 9, 9, 255];
        assert_ne!(key, motion_key(&commands("-60 deg"), 7, other_colors));
        assert!(motion_key(&[("solid.extrude".into(), json!({"distance": 10}))], 7, colors()).is_none());
        for command in ["joint.create", "joint.drive"] {
            assert_eq!(
                motion_key(&[(command.into(), json!({"joint": 1, "values": [0]}))], 7, colors()),
                motion_key(&[(command.into(), json!({"joint": 1, "values": [90]}))], 7, colors()),
            );
        }
        // A gear dialog playing its driving joint streams its poses too.
        let gear = |angle: &str| {
            vec![
                ("joint.gear".to_string(), json!({"a": 1, "b": 2, "ra": 10, "rb": 20})),
                ("joint.drive".to_string(), json!({"joint": 1, "values": [angle]})),
            ]
        };
        assert!(motion_key(&gear("0 deg"), 7, colors()).is_some());
        assert_eq!(motion_key(&gear("0 deg"), 7, colors()), motion_key(&gear("90 deg"), 7, colors()));
        // So does a cam dialog turning its cam.
        let cam = |angle: &str| {
            vec![
                ("joint.cam".to_string(), json!({"a": 1, "b": 2, "profile": {"sketch": 3}})),
                ("joint.drive".to_string(), json!({"joint": 1, "values": [angle]})),
            ]
        };
        assert!(motion_key(&cam("0 deg"), 7, colors()).is_some());
        assert_eq!(motion_key(&cam("0 deg"), 7, colors()), motion_key(&cam("90 deg"), 7, colors()));
    }

    /// Combine previews its result (cut, join or intersect between existing bodies) without an
    /// error, so its dialog's OK is enabled. Before, the preview also tried the command as a new
    /// body, which a combine has no form of, and always reported "operation must be join, cut or
    /// intersect".
    #[test]
    fn combine_previews_its_result_without_an_error() {
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 20, "width": 20, "height": 20, "body_name": "Cube"})).unwrap();
        s.execute("solid.cylinder", &json!({"base": [10, 10, -5], "diameter": 6, "height": 30, "body_name": "Pin"})).unwrap();
        for op in ["cut", "join", "intersect"] {
            let cmd = ("solid.combine".to_string(), json!({"target": "Cube", "tools": ["Pin"], "operation": op, "keep_tools": false}));
            let built = compute(&s, &[cmd], colors()).unwrap_or_else(|e| panic!("{op}: {e}"));
            assert!(built.error.is_none(), "{op}: {:?}", built.error);
        }
    }

    fn count(bytes: &[u8], stride: usize, col: [u8; 4]) -> usize {
        bytes.chunks(stride).filter(|v| v.get(24..28) == Some(&col[..])).count()
    }

    #[test]
    fn a_hole_shows_its_cut_through_the_body() {
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let p = s.preview(&[("solid.hole".to_string(), json!({"position": [20, 15, 20], "diameter": 5, "depth": 10}))]).unwrap();
        let b = build(&p.before, &p.after, colors());
        // The hole's walls are drawn see-through in the cut colour (seen from any side).
        assert!(count(&b.scene.xray, 28, colors().removed) > 0);
        // A feature that only adds material shows no cut.
        let p = s.preview(&[("solid.box".to_string(), json!({"length": 10, "width": 10, "height": 30, "operation": "join"}))]).unwrap();
        assert_eq!(count(&build(&p.before, &p.after, colors()).scene.xray, 28, colors().removed), 0);
    }

    fn plate() -> Session {
        let mut s = Session::default();
        for (id, p) in [
            ("sketch.create", json!({"plane": "XY"})),
            ("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]})),
            ("sketch.finish", json!({})),
            ("solid.extrude", json!({"distance": 20})),
        ] {
            s.execute(id, &p).unwrap();
        }
        s
    }

    /// The surface test the preview used to run per face against every triangle.
    fn brute_force_on_surface(m: &Mesh, p: Vec3) -> bool {
        let bb = m.bounds();
        let tol = (bb.diagonal() * 2e-3).max(1e-4);
        m.triangles.iter().filter_map(|t| m.tri(t)).any(|[a, b, c]| point_triangle_dist(p, a, b, c) < tol)
    }

    /// A flat sheet of `n × n` squares, two triangles and one face each, lifted by `z`.
    fn sheet(n: usize, z: f64) -> Mesh {
        let mut m = Mesh::default();
        for i in 0..=n {
            for j in 0..=n {
                m.positions.push(Vec3::new(i as f64, j as f64, z));
                m.normals.push(Vec3::Z);
            }
        }
        for i in 0..n {
            for j in 0..n {
                let a = (i * (n + 1) + j) as u32;
                let (b, c, d) = (a + 1, a + n as u32 + 1, a + n as u32 + 2);
                m.triangles.extend([[a, c, d], [a, d, b]]);
                m.tri_face.extend([(i * n + j) as u32; 2]);
            }
        }
        m
    }

    /// The grid answers as a scan of every triangle does, inside, beside and far from the mesh.
    #[test]
    fn surface_index_matches_a_scan() {
        let m = sheet(12, 0.0);
        let index = SurfaceIndex::new(&m).unwrap();
        for k in 0..2000 {
            let f = k as f64;
            let p = Vec3::new((f * 0.37).rem_euclid(15.0) - 1.5, (f * 0.61).rem_euclid(15.0) - 1.5, ((f * 0.13).rem_euclid(1.0) - 0.5) * 0.05);
            assert_eq!(index.contains(p), brute_force_on_surface(&m, p), "{p:?}");
        }
        assert!(SurfaceIndex::new(&Mesh::default()).is_none());
    }

    /// Thousands of faces (a modeled thread's) are classified at once, not face by face against
    /// every triangle: filleting beside a thread froze the app for minutes.
    #[test]
    fn many_faces_are_classified_quickly() {
        let started = std::time::Instant::now();
        let n = 120;
        let (old, same, lifted) = (sheet(n, 0.0), sheet(n, 0.0), sheet(n, 5.0));
        let nf = n * n;
        assert!(new_faces(&same, nf, &old).iter().all(|new| !new), "faces on the old surface are not new");
        assert!(new_faces(&lifted, nf, &old).iter().all(|new| *new), "faces off the old surface are new");
        assert!(started.elapsed().as_secs() < 10, "took {:?}", started.elapsed());
    }

    /// A fillet preview stands in for the body: the round face is new, the rest is kept, and the
    /// old body is the ghost. The session is unchanged.
    #[test]
    fn fillet_preview_scene() {
        let s = plate();
        let (rev, undo) = (s.revision, s.undo.len());
        let b = compute(&s, &[("solid.fillet".into(), json!({"edges": [[0, 0, 10]], "radius": 3}))], colors()).unwrap();
        assert_eq!(b.replaced, vec!["Body1".to_string()]);
        let added = count(&b.scene.tris, crate::gpu::TRI_SIZE, colors().added);
        let kept = count(&b.scene.tris, crate::gpu::TRI_SIZE, colors().body);
        assert!(added > 0 && kept > 0, "added {added} kept {kept}");
        assert!(!b.scene.ghost.is_empty());
        assert_eq!(s.revision, rev);
        assert_eq!(s.undo.len(), undo);
    }

    /// A cut shows its tool see-through and replaces nothing.
    #[test]
    fn cut_preview_scene() {
        let mut s = plate();
        s.execute("sketch.create", &json!({"plane": "XY"})).unwrap();
        s.execute("sketch.circle.center", &json!({"center": [20, 15], "radius": 5})).unwrap();
        s.execute("sketch.finish", &json!({})).unwrap();
        let b = compute(&s, &[("solid.extrude".into(), json!({"distance": 20, "operation": "cut"}))], colors()).unwrap();
        assert!(b.replaced.is_empty());
        assert!(b.scene.tris.is_empty() && !b.scene.xray.is_empty());
        assert!(b.error.is_none());
    }

    /// A new body is all new surface and replaces nothing; a bad value is an error.
    #[test]
    fn extrude_preview_scene() {
        let mut s = Session::default();
        s.execute("sketch.create", &json!({"plane": "XY"})).unwrap();
        s.execute("sketch.circle.center", &json!({"center": [0, 0], "radius": 5})).unwrap();
        s.execute("sketch.finish", &json!({})).unwrap();
        let b = compute(&s, &[("solid.extrude".into(), json!({"distance": "15 mm"}))], colors()).unwrap();
        assert!(b.replaced.is_empty());
        assert_eq!(count(&b.scene.tris, crate::gpu::TRI_SIZE, colors().body), 0);
        // #1805: a new body has its own colour, not a join's.
        assert!(count(&b.scene.tris, crate::gpu::TRI_SIZE, colors().new_body) > 0);
        assert_eq!(count(&b.scene.tris, crate::gpu::TRI_SIZE, colors().added), 0);
        assert!(b.scene.ghost.is_empty());
        assert!(compute(&s, &[("solid.extrude".into(), json!({"distance": "1 +"}))], colors()).is_err());
        assert!(s.model.state().bodies.is_empty());
    }

    fn session_of(cmds: &[(&str, Value)]) -> Session {
        let mut s = Session::default();
        for (id, p) in cmds {
            s.execute(id, p).unwrap();
        }
        s
    }

    /// A block and a two-letter text to cut into it.
    fn text_on_block() -> (Session, Vec<(String, Value)>) {
        let s = session_of(&[
            ("sketch.create", json!({"plane": "XY"})),
            ("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [60, 20]})),
            ("sketch.finish", json!({})),
            ("solid.extrude", json!({"distance": 10})),
            ("sketch.create", json!({"plane": "XY"})),
            ("sketch.text", json!({"text": "AB", "at": [5, 5], "height": 10})),
            ("sketch.finish", json!({})),
        ]);
        (s, vec![("solid.extrude".to_string(), json!({"distance": 2, "operation": "cut"}))])
    }

    /// A tool feature's live preview builds its tool without the boolean: a cut into nothing
    /// shows its tool with no error (OK will report it), while the whole preview has the error.
    #[test]
    fn a_tool_preview_builds_the_tool_without_the_boolean() {
        let s = session_of(&[
            ("sketch.create", json!({"plane": "XY"})),
            ("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [10, 10]})),
            ("sketch.finish", json!({})),
        ]);
        let cmds = vec![("solid.extrude".to_string(), json!({"distance": 2, "operation": "cut"}))];
        let tool = compute_tool(&s, &cmds, colors()).unwrap();
        assert!(tool.error.is_none() && !tool.scene.xray.is_empty(), "{:?}", tool.error);
        let whole = compute(&s, &cmds, colors()).unwrap();
        assert!(whole.error.as_deref().is_some_and(|e| e.contains("no body to cut")), "{:?}", whole.error);
        assert_eq!(whole.scene.xray.len(), tool.scene.xray.len());
    }

    /// While an arrow is dragged the inputs change faster than a tool is built: a tool built
    /// for inputs since changed is shown anyway (then the latest computed), an error only for
    /// the inputs it belongs to.
    #[test]
    fn a_tool_for_inputs_since_changed_still_shows() {
        let (s, cmds) = text_on_block();
        let tool = compute_tool(&s, &cmds, colors()).unwrap();
        let mut pv = PreviewState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        pv.running = Some((1, rx));
        pv.running_tool = true;
        tx.send(Ok(tool)).unwrap();
        pv.poll(2, None);
        assert!(pv.running.is_none() && pv.active && pv.shown_key == Some(1), "shown, and key 2 still to compute");
        let (tx, rx) = std::sync::mpsc::channel();
        pv.running = Some((3, rx));
        pv.running_tool = true;
        tx.send(Err("too far".into())).unwrap();
        pv.poll(4, None);
        assert!(pv.error.is_none() && pv.shown_key == Some(1), "an old error is dropped");
    }

    /// A design leaving the window (another tab) takes its preview off the viewport: an empty
    /// scene in the slot the viewport reads, under a new key. A fresh slot left the fillet
    /// preview of the old design drawn over the new tab's empty design.
    #[test]
    fn reset_hands_the_viewport_an_empty_scene() {
        let mut pv = PreviewState::default();
        let mut sc = GpuScene::default();
        sc.xray_tri([0.0; 3], [0.0, 0.0, 1.0], [255; 4]);
        pv.set_scene(sc);
        pv.active = true;
        pv.replaced = vec!["Body1".into()];
        let slot = pv.slot.clone();
        let key = pv.key;
        // The viewport took the scene.
        assert!(slot.lock().unwrap().take().is_some_and(|s| !s.xray.is_empty()));
        pv.reset();
        assert!(std::sync::Arc::ptr_eq(&slot, &pv.slot), "the slot the viewport reads");
        assert_ne!(pv.key, key, "a new key: the viewport takes the slot again");
        assert!(slot.lock().unwrap().as_ref().is_some_and(|s| s.xray.is_empty() && s.blocks.is_empty()));
        assert!(!pv.active && pv.replaced.is_empty());
    }
}

#[cfg(test)]
#[path = "preview_drag_tests.rs"]
mod drag_tests;
