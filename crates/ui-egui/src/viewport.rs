//! The 3D viewport: navigation, GPU (or CPU fallback) rendering, picking, overlays, the axis
//! view and the navigation bar.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use serde_json::json;
use solidcraft_engine::Sel;
use solidcraft_engine::geom::{Vec2, Vec3};
use solidcraft_engine::render::{Camera, Mat4, Rgb};
use solidcraft_engine::sketch::CurveKind;
use solidcraft_engine::view::{GridPlane, colors, grid_plane, grid_step, sketch_lines_with_sampler};
use std::hash::{Hash, Hasher};

use crate::gpu::{Frame, GpuScene, GpuTarget, SceneSlot, ViewportCallback};
use crate::navigation::{CameraInput, NavCaps, OrbitSnap, orbit_drag};
use crate::selection::{BoxSel, origin_quads, origin_size, ray_quad};
use crate::theme::Tokens;
use crate::{SolveApp, icons};

#[cfg(test)]
#[path = "viewport_appearance_tests.rs"]
mod appearance_tests;

#[cfg(test)]
#[path = "viewport_removed_later_tests.rs"]
mod removed_later_tests;

#[cfg(test)]
#[path = "viewport_plane_target_tests.rs"]
mod plane_target_tests;

#[path = "navigation_cube.rs"]
mod navigation_cube;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavMode {
    Orbit,
    Pan,
    Zoom,
}

#[derive(Default)]
pub struct ViewportState {
    /// A two-point drawing tool's first point was placed by a press that became a drag: the
    /// release places the second (#1630).
    pub(crate) draw_drag: bool,
    /// Per-body boxes of one world state, held (not just its address, which a later state can
    /// reuse once the old one is freed, so stale boxes would come back).
    pick_bounds: std::sync::Mutex<Option<BodyBounds>>,
    /// The model scene's blocks of bodies by key, from the last scene built.
    pub(crate) blocks: std::sync::Mutex<std::collections::HashMap<u64, CachedBlock>>,
    /// View-dependent display refinement (#1320): which bodies are drawn finer than the chosen
    /// display quality, and their refined plans.
    pub(crate) lod: std::sync::Mutex<crate::display_lod::Lod>,
    /// The view and model the refinement was last chosen for.
    lod_view: u64,
    /// Each body mesh's box (the mesh held, so its address stays its own), from the last state.
    mesh_bounds: std::sync::Mutex<std::collections::HashMap<usize, (std::sync::Arc<solidcraft_engine::geom::Mesh>, solidcraft_engine::geom::Aabb3)>>,
    /// Each body mesh's picking index (made the first time a pick reaches the body), kept while
    /// the mesh is part of the world state.
    pub(crate) pick_index: crate::pick_index::PickIndexes,
    /// The GPU pick buffer's scene, read-backs and requests.
    pub(crate) pick_buffer: crate::pick_buffer::State,
    /// The last click in the view was the middle button's (for the middle double-click).
    pub(crate) middle_clicked: bool,
    /// The last pick (its key and hits): reused while nothing it depends on changed.
    last_pick: Option<(u64, Vec<Hit>)>,
    pub rect: Option<Rect>,
    pub(crate) layout: Option<crate::viewport_layout::Layout>,
    pub(crate) palette_rect: Option<Rect>,
    pub(crate) dialog_rect: Option<Rect>,
    pub gpu: Option<GpuTarget>,
    slot: SceneSlot,
    key: u64,
    sketch_slot: SceneSlot,
    sketch_key: u64,
    physical_scale: f32,
    pub(crate) display: std::sync::Mutex<crate::sketch_display::DisplayCache>,
    hl_slot: SceneSlot,
    hl_key: u64,
    /// Browser/timeline body feedback, independent of the camera and reference widget scale.
    hover_block: std::sync::Mutex<Option<HoverBlock>>,
    pub nav: Option<NavMode>,
    /// The preset's navigation for the left-button drag under way (Alias: Shift+Alt), taken
    /// when the button went down, so the drag neither selects nor moves sketch geometry.
    pub primary_nav: Option<NavMode>,
    /// Button-free gesture and its last owned pointer position, kept through pauses.
    pub(crate) modifier_nav: Option<(NavMode, Pos2)>,
    /// What a click would pick now (after the active command's filter).
    pub hover: Option<Hit>,
    pub mouse: Option<Pos2>,
    /// Box selection being dragged.
    pub boxsel: Option<BoxSel>,
    /// Timeline item under the cursor: its bodies or resulting faces are highlighted.
    pub hover_feature: Option<u64>,
    /// Bodies whose browser row is under the cursor: highlighted the same way.
    pub hover_bodies: Vec<String>,
    /// The model point an orbit turns around (Preferences: orbit around the cursor).
    pub orbit_pivot: Option<Vec3>,
    /// Continuous orbit: the view turns about the vertical until the next click or key.
    pub spin: bool,
    /// The 3D mouse's motion this frame, for the page showing the camera to apply with what its
    /// camera may do (`navigation::device`).
    pub device_motion: Option<crate::space_mouse::DeviceMotion>,
    /// After immediate zoom, ignore egui's outstanding smoothing until the gesture ends.
    pub(crate) wheel_zoom_raw_until_idle: bool,
    /// While trackpad scrolling runs, ignore egui's smoothing tail for zoom so
    /// two-finger input is not replayed as zoom after it pans.
    pub(crate) trackpad_until_idle: bool,
    /// Inside a bracketed trackpad scroll (`Start` without its `End` yet):
    /// point moves pan, bare moves are a mouse.
    pub(crate) trackpad_gesture: bool,
    /// A primary press on the axis gizmo: the pointer position used by its last orbit step.
    pub axis_drag: Option<Pos2>,
    /// Unsnapped camera while Shift-orbit previews a standard or diagonal view.
    pub orbit_free: Option<Camera>,
    /// Spring toward the chosen orientation, including after the drag ends.
    pub(crate) orbit_snap: Option<OrbitSnap>,
    pub build_ms: f64,
    /// Adaptive supersampling (#1288): the view drawn last (its view-projection and pixels) and
    /// since when it has held still (app time, s).
    pub(crate) ss_view: (u64, f64),
    /// The right-click menu, open at this screen position.
    pub context_menu: Option<Pos2>,
    /// Select Other: its list, a press being held, and click cycling.
    pub select_other: crate::select_other::State,
    /// A sketch point or curve being dragged (its id, the undo depth when the drag began: the
    /// whole drag is one undo step, and for a curve where it was last under the cursor).
    pub point_drag: Option<(String, usize, Option<Vec2>)>,
    /// Checkpoint and source for a cancellable whole-text drag.
    pub(crate) text_drag: Option<(u64, String)>,
    /// A part that joints place, being dragged.
    pub(crate) joint_drag: Option<crate::joint_drag::Drag>,
    /// The plane the grid was last drawn on while Preferences ▸ Grid follows the view is on
    /// (#1470), kept so the choice is hysteretic near the thresholds (#1993).
    pub(crate) grid_plane: std::sync::Mutex<GridPlane>,
}

/// Something under the cursor.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    SketchPoint {
        sketch: u64,
        id: String,
        at: Vec2,
    },
    /// A sketch curve; `straight` for lines (usable as an axis).
    SketchCurve {
        sketch: u64,
        id: String,
        straight: bool,
    },
    Vertex {
        body: String,
        point: Vec3,
    },
    Edge {
        body: String,
        index: usize,
        mid: Vec3,
    },
    Profile {
        sketch: u64,
        index: usize,
    },
    Face {
        body: String,
        index: usize,
        point: Vec3,
    },
    /// An origin plane (XY, XZ, YZ) or a construction plane (by name).
    Plane {
        name: String,
        point: Vec3,
    },
    /// An origin axis (X, Y, Z).
    Axis {
        name: String,
    },
}

impl ViewportState {
    pub fn pixels_per_point(&self) -> f32 {
        if self.physical_scale > 0.0 { self.physical_scale } else { 1.0 }
    }
    #[cfg(test)]
    pub(crate) fn set_pixels_per_point(&mut self, scale: f32) {
        self.physical_scale = scale;
    }
}

fn display_proj(app: &SolveApp) -> Proj {
    projection(app, app.viewport.rect.unwrap_or_else(|| Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))))
}

/// World ↔ screen mapping for the current frame.
#[derive(Clone, Copy)]
pub struct Proj {
    pub cam: Camera,
    pub vp: Mat4,
    pub rect: Rect,
}

impl Proj {
    pub fn to_screen(&self, p: Vec3) -> Option<Pos2> {
        let c = self.vp.apply(p);
        if c[3] <= 1e-9 {
            return None;
        }
        let (x, y) = (c[0] / c[3], c[1] / c[3]);
        Some(pos2(
            self.rect.left() + ((x + 1.0) * 0.5 * self.rect.width() as f64) as f32,
            self.rect.top() + ((1.0 - y) * 0.5 * self.rect.height() as f64) as f32,
        ))
    }
    pub fn ray(&self, p: Pos2) -> (Vec3, Vec3) {
        self.cam.ray((p.x - self.rect.left()) as f64, (p.y - self.rect.top()) as f64, self.rect.width() as f64, self.rect.height() as f64)
    }
}

/// A projected 3D face (convex): an unfeathered triangle fan, outlined one edge at a time (no
/// joins between the edges). Unlike `Shape::convex_polygon`, it stays inside its outline when
/// the face turns edge-on.
pub(crate) fn projected_polygon(pts: &[Pos2], fill: Color32, stroke: Stroke) -> Vec<Shape> {
    let mut mesh = egui::Mesh::default();
    for (i, p) in (0u32..).zip(pts) {
        mesh.colored_vertex(*p, fill);
        if i >= 2 {
            mesh.add_triangle(0, i - 1, i);
        }
    }
    let mut out = vec![Shape::mesh(mesh)];
    out.extend(pts.iter().zip(pts.iter().skip(1).chain(pts.first())).map(|(a, b)| Shape::line_segment([*a, *b], stroke)));
    out
}

/// View directions all round, a few degrees apart, none exactly along an axis.
#[cfg(test)]
pub(crate) fn orbit_directions() -> impl Iterator<Item = Vec3> {
    (0..90).flat_map(|i| {
        (0..45).map(move |j| {
            let (yaw, pitch) = (f64::from(i) * 4.0f64.to_radians() + 0.003, f64::from(j) * 4.0f64.to_radians() - 1.5);
            Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin())
        })
    })
}

/// Every vertex egui draws for `shapes`.
#[cfg(test)]
pub(crate) fn drawn_points(shapes: Vec<Shape>) -> Vec<Pos2> {
    let mut tess = egui::epaint::Tessellator::new(1.0, egui::epaint::TessellationOptions::default(), [1, 1], vec![]);
    let mut mesh = egui::Mesh::default();
    for shape in shapes {
        tess.tessellate_shape(shape, &mut mesh);
    }
    mesh.vertices.iter().map(|v| v.pos).collect()
}

fn rgba(c: Rgb) -> [u8; 4] {
    [c.0, c.1, c.2, 255]
}

pub fn scene_radius(app: &SolveApp) -> f64 {
    let mut b = model_bounds(app);
    if !app.preview.bounds.is_empty() {
        b = b.union(&app.preview.bounds);
    }
    let r = if b.is_empty() { 50.0 } else { b.diagonal() * 0.5 + b.center().dist(app.cam.target) };
    r.max(app.cam.half_height() * 2.0).max(10.0)
}

pub fn projection(app: &SolveApp, rect: Rect) -> Proj {
    let mut cam = app.cam;
    cam.fov = if app.perspective_view() { 0.6 } else { 0.0 };
    let aspect = rect.width() as f64 / rect.height().max(1.0) as f64;
    Proj { cam, vp: cam.view_proj(aspect, scene_radius(app)), rect }
}

/// Sketches drawn in the viewport: the active one, and finished ones not yet used by a feature.
fn visible_sketches(app: &SolveApp) -> Vec<u64> {
    let st = app.session.world_state();
    st.sketches.iter().filter(|s| solidcraft_engine::view::sketch_drawn(&app.session, s.feature, app.ui.show_sketches)).map(|s| s.feature).collect()
}

/// What Home, Fit and the orbit pivot frame: the bodies and sketches shown now, where they are
/// now (see [`solidcraft_engine::view::visible_bounds`]).
pub fn view_bounds(app: &SolveApp) -> solidcraft_engine::geom::Aabb3 {
    solidcraft_engine::view::visible_bounds(&app.session, app.ui.show_sketches)
}

fn scene_key(app: &SolveApp) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    app.session.revision.hash(&mut h);
    (std::sync::Arc::as_ptr(&app.session.world_state()) as usize).hash(&mut h);
    app.ui.show_grid.hash(&mut h);
    app.ui.visual_style.hash(&mut h);
    app.ui.ground_shadow.hash(&mut h);
    solidcraft_engine::doc::display_quality::current().hash(&mut h);
    lod_generation(app).hash(&mut h);
    // The section plane (its cut outlines are part of the model scene).
    section_plane(app).map(|(o, n)| [o.x, o.y, o.z, n.x, n.y, n.z].map(f64::to_bits)).hash(&mut h);
    app.cam.up.hash(&mut h);
    (app.cam.pitch > 0.0).hash(&mut h);
    app.ui.show_sketches.hash(&mut h);
    app.session.visibility.hidden_bodies.hash(&mut h);
    app.session.visibility.hidden_occurrences.hash(&mut h);
    app.session.visibility.hidden_sketches.hash(&mut h);
    app.session.visibility.shown_sketches.hash(&mut h);
    app.ui.hidden_profiles.hash(&mut h);
    app.session.active_component.hash(&mut h);
    app.session.active_occurrence_path.hash(&mut h);
    app.preview.replaced.hash(&mut h);
    app.tree.redefine.hash(&mut h);
    crate::theme::is_dark().hash(&mut h);
    crate::theme::vision().hash(&mut h);
    app.session.active_sketch.hash(&mut h);
    h.finish()
}

fn sketch_scene_key(app: &SolveApp, proj: &Proj) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    scene_key(app).hash(&mut h);
    // The grid this scene draws follows the view when the preference says so (#1470): turning it
    // on or off redraws even while the camera holds still.
    app.preferences.grid_follows_view.hash(&mut h);
    (std::sync::Arc::as_ptr(&app.session.world_state()) as usize).hash(&mut h);
    crate::sketch_display::projection_key(proj, app.viewport.pixels_per_point()).hash(&mut h);
    h.finish()
}

#[cfg(test)]
pub(crate) fn display_scene_keys(app: &SolveApp, proj: &Proj) -> (u64, u64) {
    (scene_key(app), sketch_scene_key(app, proj))
}

/// A sketch curve colour from the engine, in the current theme and colour vision mode.
fn sketch_color(t: &Tokens, c: Rgb) -> [u8; 4] {
    if c == colors::SKETCH {
        c4(t.sketch)
    } else if c == colors::SKETCH_DONE || c == colors::SKETCH_FIXED {
        c4(t.sketch_fixed)
    } else if c == colors::SKETCH_LOCKED {
        c4(t.sketch_locked)
    } else if c == colors::SKETCH_PROJECTED {
        c4(t.sketch_projected)
    } else if c == colors::SKETCH_LOST {
        c4(t.sketch_lost)
    } else if c == colors::SKETCH_CONSTRUCTION {
        c4(t.sketch_construction)
    } else if c == colors::SKETCH_CENTERLINE {
        c4(t.sketch_centerline)
    } else {
        rgba(c)
    }
}

/// With a component active, bodies outside it are drawn faded.
fn body_faded(app: &SolveApp, b: &solidcraft_engine::doc::ModelBody, active_bodies: &Option<Vec<String>>) -> bool {
    let s = &app.session;
    if let Some(names) = active_bodies {
        !names.contains(&b.name)
    } else {
        s.active_component != 0 && !s.doc.component_within(b.component, s.active_component)
    }
}

/// How many times the viewport's pixels to draw this frame each way (#1288; 1: off): the
/// preference, and with adaptive supersampling only once the view has held still for a moment,
/// so orbiting and zooming stay as quick as without it.
fn supersampling(app: &mut SolveApp, ctx: &egui::Context, view_proj: [[f32; 4]; 4], size_px: [f32; 2]) -> f32 {
    let factor = app.preferences.supersampling;
    if !(factor > 1.0) {
        return 1.0;
    }
    if !app.preferences.supersampling_adaptive {
        return factor;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    view_proj.iter().flatten().chain(&size_px).for_each(|x| x.to_bits().hash(&mut h));
    let view = h.finish();
    let now = ctx.input(|i| i.time);
    if view != app.viewport.ss_view.0 {
        app.viewport.ss_view = (view, now);
    }
    let still = now - app.viewport.ss_view.1;
    if still >= SS_SETTLE {
        factor
    } else {
        // Draw it supersampled once the view settles.
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(SS_SETTLE - still + 0.01));
        1.0
    }
}

/// How long the view holds still before adaptive supersampling draws it at full quality (s).
const SS_SETTLE: f64 = 0.25;

/// The display plan the model scene draws `b` from on the GPU, or `None` when its faces are built
/// here from the reference mesh. Opaque shaded bodies (`opaque`: not faded, and no see-through
/// body or face) are drawn from their plans where the device evaluates them; faded and
/// see-through ones (sorted and blended per triangle) are built here. A body seen up close is
/// drawn from its refined plan ([`crate::display_lod`]).
fn drawn_plan(
    app: &SolveApp,
    b: &solidcraft_engine::doc::ModelBody,
    opaque: bool,
) -> Option<std::sync::Arc<solidcraft_engine::geom::tess::TessPlan>> {
    let shaded = app.ui.visual_style != 2;
    (shaded && opaque && app.viewport.gpu.is_some_and(|g| g.tess))
        .then(|| refined(app, b).map(|(plan, _)| plan).or_else(|| b.display_plan()))
        .flatten()
        .filter(|plan| plan.vertex_count() <= solidcraft_gpu::MAX_BATCH)
}

/// The refined plan and mesh `b` is drawn from, when the view has it drawn finer than the chosen
/// display quality (#1320).
fn refined(app: &SolveApp, b: &solidcraft_engine::doc::ModelBody) -> crate::display_lod::Refined {
    app.viewport.lod.lock().ok()?.refined(b, solidcraft_engine::doc::display_quality::current())
}

/// Changes whenever a body's refinement changes (the scene and highlight keys).
fn lod_generation(app: &SolveApp) -> u64 {
    app.viewport.lod.lock().map_or(0, |l| l.generation)
}

/// The mesh whose triangles the model scene shows for `b`'s faces: the plan's (evaluated on the
/// CPU, the same triangles the GPU draws) or [`drawn_mesh`]. A face highlighted over the model
/// must use the same triangles: another triangulation of the same face fights it at equal depth
/// and shimmers.
fn shown_mesh(
    app: &SolveApp,
    b: &solidcraft_engine::doc::ModelBody,
    active_bodies: &Option<Vec<String>>,
) -> std::sync::Arc<solidcraft_engine::geom::Mesh> {
    if let Some((_, mesh)) = refined(app, b) {
        // Refined: faces drawn from the refined plan on the GPU or built from its mesh here.
        return mesh;
    }
    let doc = &app.session.doc;
    let opaque =
        || !body_faded(app, b, active_bodies) && doc.model_body_opacity(b) >= 0.999 && doc.face_colors(b).iter().all(|(_, l)| l.opacity >= 0.999);
    (app.viewport.gpu.is_some_and(|g| g.tess) && opaque())
        .then(|| drawn_plan(app, b, true))
        .flatten()
        .and_then(|_| b.plan_mesh())
        .unwrap_or_else(|| drawn_mesh(app, b))
}

/// The mesh the model scene builds `b` from on the CPU (its edges always, its faces when they
/// are not drawn from the plan on the GPU): a refined plan's mesh when the view has one for it
/// (#1320), the reference mesh at Normal display quality, else the plan's mesh at the chosen
/// quality (finer or coarser faces and edges alike), falling back to the reference mesh when the
/// body has no plan.
fn drawn_mesh(app: &SolveApp, b: &solidcraft_engine::doc::ModelBody) -> std::sync::Arc<solidcraft_engine::geom::Mesh> {
    use solidcraft_engine::doc::display_quality::{DisplayQuality, current};
    if let Some((_, mesh)) = refined(app, b) {
        return mesh;
    }
    if current() == DisplayQuality::Normal { b.mesh() } else { b.plan_mesh().unwrap_or_else(|| b.mesh()) }
}

/// Choose which bodies to draw finer than the chosen display quality for this view (#1320):
/// each shown body's screen-space error from its box on screen, refinements planned off the UI
/// thread and swapped in when ready. Keeps repainting while refinements are being planned.
fn update_refinement(app: &mut SolveApp, ctx: &egui::Context, proj: &Proj) {
    if app.viewport.gpu.is_none() || !crate::display_lod::enabled() {
        return; // The CPU picture draws the reference mesh.
    }
    let q = solidcraft_engine::doc::display_quality::current();
    // Nothing to choose again while the view, the model and the planning stand still.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    scene_key(app).hash(&mut h);
    crate::sketch_display::projection_key(proj, app.viewport.pixels_per_point()).hash(&mut h);
    let view = h.finish();
    // While refinements are being planned, the view standing still: only collect finished ones.
    if view == app.viewport.lod_view {
        let (busy, finished) = app.viewport.lod.lock().map_or((false, false), |mut l| {
            let finished = l.collect();
            (l.busy(), finished)
        });
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        if !finished {
            return;
        }
    }
    let t0 = crate::now_ms();
    app.viewport.lod_view = view;
    let st = app.session.world_state();
    let ppp = f64::from(app.viewport.pixels_per_point());
    let lens = crate::display_lod::Lens::new(&proj.cam, f64::from(proj.rect.height()) * ppp);
    let bounds = body_bounds(app, &st);
    let mut live = std::collections::HashSet::with_capacity(st.bodies.len());
    let mut candidates = Vec::new();
    for (b, (name, bx)) in st.bodies.iter().zip(bounds.iter()) {
        let id = crate::display_lod::body_id(b).0;
        live.insert(id);
        if b.name != *name || b.body.is_mesh() {
            continue;
        }
        let shown = app.session.body_visible(&b.name) && !app.preview.replaced.contains(&b.name);
        let on_screen = match screen_box(proj, bx, 0.0).filter(|_| shown) {
            Some(r) => r.intersects(proj.rect).then(|| f64::from(r.width().max(r.height())) * ppp),
            // A corner behind the eye: the view is inside or right by the box.
            None => (shown && proj.cam.fov > 0.0 && lens.in_front(bx)).then_some(f64::INFINITY),
        };
        let err_px = crate::display_lod::error_px(q, b.body.size(), bx, on_screen, &lens);
        candidates.push(crate::display_lod::Candidate { body: b, err_px, triangles: b.mesh().triangles.len() });
    }
    let busy = match app.viewport.lod.lock() {
        Ok(mut lod) => {
            lod.update(q, candidates, &live);
            lod.stats.choose_ms = crate::now_ms() - t0;
            lod.busy()
        }
        Err(_) => false,
    };
    if busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
}

/// A model scene block kept by the viewport: its bytes and the meshes they were built from
/// (held, so no other mesh can take their addresses while the block is kept).
pub(crate) struct CachedBlock {
    bytes: std::sync::Arc<crate::gpu::BlockBytes>,
    meshes: Vec<std::sync::Arc<solidcraft_engine::geom::Mesh>>,
}

/// One body as the model scene draws it.
struct BodyDraw {
    mesh: std::sync::Arc<solidcraft_engine::geom::Mesh>,
    col: [u8; 4],
    faded: bool,
    opacity: f64,
    face_looks: std::collections::HashMap<usize, solidcraft_engine::doc::appearance::Look>,
    /// Its faces are drawn from its plan on the GPU (only its edges are built here).
    on_gpu: bool,
}

impl BodyDraw {
    /// Everything [`BodyDraw::build`] reads.
    fn key(&self, shaded: bool, edges: bool, hidden_edges: bool, edge_col: [u8; 4], hidden_col: [u8; 4]) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (std::sync::Arc::as_ptr(&self.mesh) as usize).hash(&mut h);
        (self.col, self.faded, self.opacity.to_bits(), self.on_gpu, shaded, edges, hidden_edges, edge_col, hidden_col).hash(&mut h);
        let mut looks: Vec<_> = self.face_looks.iter().map(|(f, l)| (*f, l.color, l.opacity.to_bits())).collect();
        looks.sort_unstable();
        looks.hash(&mut h);
        h.finish()
    }

    fn build(&self, sc: &mut crate::gpu::BlockBytes, shaded: bool, edges: bool, hidden_edges: bool, edge_col: [u8; 4], hidden_col: [u8; 4]) {
        let m = &self.mesh;
        let mut out = GpuScene::default();
        for (ti, t) in m.triangles.iter().enumerate().filter(|_| shaded && !self.on_gpu) {
            let (mut col, a) = match m.tri_face.get(ti).and_then(|f| self.face_looks.get(&(*f as usize))) {
                Some(l) => {
                    let c = Color32::from_rgb(l.color[0], l.color[1], l.color[2]);
                    (if self.faded { c4(c.gamma_multiply(0.35)) } else { c4(c) }, l.opacity)
                }
                None => (self.col, self.opacity),
            };
            let glass = !self.faded && a < 0.999;
            if glass {
                col[3] = (a.clamp(0.05, 1.0) * 255.0).round() as u8;
            }
            for k in t {
                let i = *k as usize;
                if let (Some(p), Some(n)) = (m.positions.get(i), m.normals.get(i)) {
                    if self.faded {
                        out.trans_tri(p.to_f32(), n.to_f32(), col);
                    } else if glass {
                        out.glass_tri(p.to_f32(), n.to_f32(), col);
                    } else {
                        out.tri(p.to_f32(), n.to_f32(), col);
                    }
                }
            }
        }
        for (ei, e) in m.edges.iter().enumerate().filter(|_| edges) {
            if !m.edge_pickable(ei) || (shaded && !m.shaded_edge_visible(ei)) {
                continue;
            }
            for w in e.windows(2) {
                out.line(w[0].to_f32(), w[1].to_f32(), edge_col, 1.3, false);
                if hidden_edges {
                    out.hidden_line(w[0].to_f32(), w[1].to_f32(), hidden_col, 1.0);
                }
            }
        }
        sc.tris.extend_from_slice(&out.tris);
        sc.trans.extend_from_slice(&out.trans);
        sc.glass.extend_from_slice(&out.glass);
        sc.lines.extend_from_slice(&out.lines);
        sc.hidden.extend_from_slice(&out.hidden);
    }
}

/// Add a block of bodies to the scene: the bytes built for the same bodies last time when
/// there were any, else built now.
fn push_block(
    sc: &mut GpuScene,
    kept: &mut std::collections::HashMap<u64, CachedBlock>,
    made: &mut std::collections::HashMap<u64, CachedBlock>,
    key: u64,
    draws: Vec<BodyDraw>,
    build: impl Fn(&BodyDraw, &mut crate::gpu::BlockBytes),
) {
    let same = |c: &CachedBlock| c.meshes.len() == draws.len() && c.meshes.iter().zip(&draws).all(|(m, d)| std::sync::Arc::ptr_eq(m, &d.mesh));
    let block = match kept.remove(&key).or_else(|| made.remove(&key)).filter(same) {
        Some(c) => c,
        None => {
            let mut bytes = crate::gpu::BlockBytes::default();
            for d in &draws {
                build(d, &mut bytes);
            }
            CachedBlock { bytes: std::sync::Arc::new(bytes), meshes: draws.iter().map(|d| d.mesh.clone()).collect() }
        }
    };
    // See-through triangles are sorted with the rest of the glass, every view.
    sc.glass.extend_from_slice(&block.bytes.glass);
    sc.blocks.push(crate::gpu::SceneBlock { key, bytes: block.bytes.clone() });
    made.insert(key, block);
}

fn build_scene(app: &SolveApp) -> GpuScene {
    let tk = Tokens::get();
    let mut sc = GpuScene::default();
    let s = &app.session;
    // Bodies where their occurrences place them.
    let st = s.world_state();
    let active_bodies = s.active_occurrence_path.as_ref().and_then(|p| s.occurrence_bodies(p, true).ok());
    let (shaded, edges) = match app.ui.visual_style {
        1 => (true, false),
        2 => (false, true),
        _ => (true, true),
    };
    let edge_col = if shaded { c4(tk.body_edge) } else { c4(tk.text) };
    // Shaded with hidden edges: edges behind the model show faintly.
    let hidden_edges = app.ui.visual_style == 3;
    let hidden_col = c4(tk.text.gamma_multiply(0.35));
    // The blocks the last scene built, reused when their bodies come back as they were.
    let mut kept = app.viewport.blocks.lock().map(|mut b| std::mem::take(&mut *b)).unwrap_or_default();
    let mut made: std::collections::HashMap<u64, CachedBlock> = std::collections::HashMap::new();
    let mut run: Vec<BodyDraw> = Vec::new();
    let mut run_key = std::collections::hash_map::DefaultHasher::new();
    for b in &st.bodies {
        if !app.session.body_visible(&b.name) || app.preview.replaced.contains(&b.name) {
            continue;
        }
        let faded = body_faded(app, b, &active_bodies);
        let base = body_color(&s.doc, b).unwrap_or(tk.body);
        let col = if faded { c4(base.gamma_multiply(0.35)) } else { c4(base) };
        // Appearances: the body's opacity, and faces with looks of their own.
        let opacity = s.doc.model_body_opacity(b);
        let face_looks: std::collections::HashMap<usize, solidcraft_engine::doc::appearance::Look> =
            if shaded { s.doc.face_colors(b).into_iter().collect() } else { Default::default() };
        let opaque = !faded && opacity >= 0.999 && face_looks.values().all(|l| l.opacity >= 0.999);
        let planned = drawn_plan(app, b, opaque);
        let on_gpu = planned.is_some();
        if let Some(plan) = planned {
            let face_colors = (0..b.body.face_count())
                .map(|f| match face_looks.get(&f) {
                    Some(l) => c4(Color32::from_rgb(l.color[0], l.color[1], l.color[2])),
                    None => col,
                })
                .collect();
            sc.planned.push(crate::gpu::PlannedBody { plan, face_colors });
        }
        let body = BodyDraw { mesh: drawn_mesh(app, b), col, faded, opacity, face_looks, on_gpu };
        let key = body.key(shaded, edges, hidden_edges, edge_col, hidden_col);
        key.hash(&mut run_key);
        run.push(body);
        // Blocks end where a body's own key says, so a body added or removed changes only its
        // own block, not every block after it.
        if key.is_multiple_of(16) || run.len() >= 64 {
            let draws = std::mem::take(&mut run);
            let block_key = std::mem::replace(&mut run_key, std::collections::hash_map::DefaultHasher::new()).finish();
            push_block(&mut sc, &mut kept, &mut made, block_key, draws, |d, out| d.build(out, shaded, edges, hidden_edges, edge_col, hidden_col));
        }
    }
    if !run.is_empty() {
        push_block(&mut sc, &mut kept, &mut made, run_key.finish(), run, |d, out| d.build(out, shaded, edges, hidden_edges, edge_col, hidden_col));
    }
    if let Ok(mut b) = app.viewport.blocks.lock() {
        *b = made;
    }
    if app.ui.ground_shadow && app.ui.visual_style != 2 && app.cam.pitch > 0.0 {
        ground_shadow(&mut sc, app, &st);
    }
    // Section: the outline of each body where the plane cuts it.
    if let Some((o, n)) = section_plane(app) {
        let col = c4(tk.text.gamma_multiply(0.9));
        for b in st.bodies.iter().filter(|b| app.session.body_visible(&b.name)) {
            for (a, c) in solidcraft_engine::view::section_segments(&b.mesh(), o, n) {
                sc.line(a.to_f32(), c.to_f32(), col, 1.8, false);
            }
        }
    }
    // Continuity analysis: each seam `inspect.continuity` measured, drawn over the model in the
    // colour of its worst verdict.
    for seam in &s.continuity {
        let col = c4(continuity_color(&tk, seam.verdict));
        for w in seam.points.windows(2) {
            if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
                sc.line(a.to_f32(), b.to_f32(), col, 2.6, true);
            }
        }
    }

    sc
}

/// The colour of a measured seam, one per verdict from the theme's `continuity` tokens: within
/// every tolerance (G2), tangent with a curvature jump (G1), a crease (G0), and faces that do
/// not meet.
fn continuity_color(t: &Tokens, verdict: solidcraft_engine::kernel::SeamVerdict) -> Color32 {
    use solidcraft_engine::kernel::SeamVerdict as V;
    let [smooth, jump, sharp, gap] = t.continuity;
    match verdict {
        V::Smooth => smooth,
        V::CurvatureJump => jump,
        V::Sharp => sharp,
        V::Gap => gap,
    }
}

/// A soft dark ellipse on the ground under the shown bodies (darkest in the middle).
fn build_sketch_scene(app: &SolveApp, proj: &Proj) -> GpuScene {
    let tk = Tokens::get();
    let mut sc = GpuScene::default();
    let s = &app.session;
    let st = s.world_state();
    let mut fill_budget = 131_072;
    if app.ui.show_grid && !active_view(app).hide_grid {
        let (minor, major) = grid_step(app.cam.half_height());
        let ext = (app.cam.half_height() * 3.0 / major).ceil() * major;
        // The ground plane: XY with Z up, XZ with Y up (laid out in the camera's Z-up frame), or
        // the upright origin plane facing the view when the ground is seen edge-on.
        let tf = app.cam.to_frame(app.cam.target);
        // On the ground (XY) unless the Grid follows the view preference is on (#1470); then on
        // the plane the last frame drew, so the choice is hysteretic near the thresholds (#1993).
        let (u, v) = if app.preferences.grid_follows_view {
            app.viewport.grid_plane.lock().map_or((Vec3::X, Vec3::Y), |mut last| {
                *last = grid_plane(app.cam.to_frame(app.cam.back()), *last);
                last.axes()
            })
        } else {
            (Vec3::X, Vec3::Y)
        };
        let snap = |a: Vec3| a * ((tf.dot(a) / major).round() * major);
        let c = snap(u) + snap(v);
        let w3 = |p: Vec3| app.cam.from_frame(p).to_f32();
        let n = ((ext / minor) as i64).clamp(1, 300);
        for i in -n..=n {
            let t = i as f64 * minor;
            let maj = (t / major).round() * major == t;
            let col = if maj { c4(tk.grid_major) } else { c4(tk.grid) };
            let w = if maj { 1.0 } else { 0.7 };
            sc.under_line(w3(c + u * t - v * ext), w3(c + u * t + v * ext), col, w);
            sc.under_line(w3(c - u * ext + v * t), w3(c + u * ext + v * t), col, w);
        }
    }
    for sid in visible_sketches(app) {
        let Some(ss) = st.sketch(sid) else { continue };
        let active = s.active_sketch == Some(sid);
        // Closed profiles are shaded (lifted off a face they may lie on).
        let lift = ss.plane.normal() * 0.01;
        let profiles = if app.ui.hidden_profiles.contains(&sid) || ss.sketch.view.hide_profile { &[][..] } else { &ss.profiles[..] };
        for (index, _) in profiles.iter().enumerate() {
            let display = crate::sketch_display::profile_region(app, sid, index, &ss.plane, proj);
            for tri in display.as_ref().as_ref().map_or_else(Vec::new, |region| crate::sketch_display::fill_triangles(region, &mut fill_budget)) {
                for q in tri {
                    sc.trans_tri((ss.plane.to_world(q) + lift).to_f32(), [0.0; 3], c4(tk.profile_fill));
                }
            }
        }
        // Construction curves are dashed, centerlines dash-dot (lengths follow the grid).
        let unit = grid_step(app.cam.half_height()).0;
        for (pts, col, cons) in sketch_lines_with_sampler(&ss.sketch, &ss.plane, active, &ss.report.curve_determined, |_, index, plane| {
            crate::sketch_display::curve_sample(app, sid, index, plane, proj).parts.clone()
        }) {
            let segs = sketch_segments(
                &pts,
                if !cons {
                    0
                } else if col == colors::SKETCH_CENTERLINE {
                    2
                } else {
                    1
                },
                unit,
            );
            for (a, b) in segs {
                sc.line(a.to_f32(), b.to_f32(), sketch_color(&tk, col), if cons { 1.2 } else { 2.0 }, active);
            }
        }
    }
    let guide_unit = grid_step(app.cam.half_height()).0;
    for points in crate::sketch_attachment_ui::display_guide_lines(app, proj) {
        for (a, b) in dashes(&points, &[guide_unit * 0.25, guide_unit * 0.15]) {
            sc.line(a.to_f32(), b.to_f32(), c4(tk.warning), 1.5, true);
        }
    }
    sc
}

fn ground_shadow(sc: &mut GpuScene, app: &SolveApp, st: &std::sync::Arc<solidcraft_engine::doc::ModelState>) {
    // The bodies' box in the camera's Z-up frame (the ground is its XY plane).
    let mut b = solidcraft_engine::geom::Aabb3::EMPTY;
    let boxes = body_bounds(app, st);
    for (x, (_, bb)) in st.bodies.iter().zip(boxes.iter()) {
        if bb.is_empty() || !app.session.body_visible(&x.name) {
            continue;
        }
        for i in 0..8 {
            let p = Vec3::new(
                if i & 1 == 0 { bb.min.x } else { bb.max.x },
                if i & 2 == 0 { bb.min.y } else { bb.max.y },
                if i & 4 == 0 { bb.min.z } else { bb.max.z },
            );
            b.add(app.cam.to_frame(p));
        }
    }
    if b.is_empty() {
        return;
    }
    let size = b.size();
    let c = b.center();
    let z = b.min.z - 1e-3 * (1.0 + b.diagonal());
    let (rx, ry) = (size.x * 0.62 + size.z * 0.15, size.y * 0.62 + size.z * 0.15);
    if !(rx > 0.0 && ry > 0.0) {
        return;
    }
    let dark = if crate::theme::is_dark() { 150 } else { 80 };
    // Rings from the middle out, fading to nothing.
    let rings = [(0.0, dark), (0.55, dark * 3 / 4), (0.85, dark / 3), (1.0, 0)];
    let n = 48;
    let at = |r: f64, k: usize| {
        let a = k as f64 / n as f64 * std::f64::consts::TAU;
        app.cam.from_frame(Vec3::new(c.x + rx * r * a.cos(), c.y + ry * r * a.sin(), z)).to_f32()
    };
    for w in rings.windows(2) {
        let ((r0, a0), (r1, a1)) = (w[0], w[1]);
        let (c0, c1) = ([0, 0, 0, a0 as u8], [0, 0, 0, a1 as u8]);
        for k in 0..n {
            let (p00, p01, p10, p11) = (at(r0, k), at(r0, k + 1), at(r1, k), at(r1, k + 1));
            sc.trans_tri(p00, [0.0; 3], c0);
            sc.trans_tri(p10, [0.0; 3], c1);
            sc.trans_tri(p11, [0.0; 3], c1);
            if r0 > 0.0 {
                sc.trans_tri(p00, [0.0; 3], c0);
                sc.trans_tri(p11, [0.0; 3], c1);
                sc.trans_tri(p01, [0.0; 3], c0);
            }
        }
    }
}

/// Sketch line types share the same world-space dash phase in the scene and overlays.
pub(crate) fn sketch_segments(pts: &[Vec3], mode: u8, unit: f64) -> Vec<(Vec3, Vec3)> {
    match mode {
        // Construction, and infinite lines (#1359), which are construction too.
        1 | 3 => dashes(pts, &[0.3 * unit, 0.2 * unit]),
        2 => dashes(pts, &[0.6 * unit, 0.15 * unit, 0.05 * unit, 0.15 * unit]),
        _ => pts.windows(2).map(|w| (w[0], w[1])).collect(),
    }
}

/// The drawn pieces of a polyline in a dash pattern (on, off, on, off… lengths).
pub(crate) fn dashes(pts: &[Vec3], pattern: &[f64]) -> Vec<(Vec3, Vec3)> {
    let mut out = Vec::new();
    if pattern.is_empty() || pattern.iter().any(|l| !(l.is_finite() && *l > 0.0)) {
        return pts.windows(2).map(|w| (w[0], w[1])).collect();
    }
    let (mut k, mut left) = (0usize, pattern[0]);
    for w in pts.windows(2) {
        let (mut a, b) = (w[0], w[1]);
        let mut len = (b - a).len();
        // Cap the pieces per segment so a huge zoom-out can't explode the scene.
        let mut guard = 0;
        while len > 1e-12 && guard < 10_000 {
            guard += 1;
            let step = left.min(len);
            let q = a + (b - a) * (step / len);
            if k % 2 == 0 {
                out.push((a, q));
            }
            left -= step;
            len -= step;
            a = q;
            if left <= 1e-12 {
                k = (k + 1) % pattern.len();
                left = pattern.get(k).copied().unwrap_or(1.0);
            }
        }
    }
    out
}

/// Colour as straight (unpremultiplied) sRGBA bytes for the GPU.
fn c4(c: Color32) -> [u8; 4] {
    c.to_srgba_unmultiplied()
}

/// Origin planes and axes currently shown, sized for the view: (name, normal, quad).
pub fn origin_planes(app: &SolveApp) -> Vec<(&'static str, Vec3, [Vec3; 4])> {
    if !app.origin_visible() {
        return Vec::new();
    }
    // While the Origin is hidden, Create Sketch brings the planes back: then every plane;
    // otherwise only those not hidden one by one.
    let creating = !app.ui.show_origin && app.picking_sketch_plane();
    origin_quads(origin_size(app.cam.half_height()))
        .into_iter()
        .filter(|(n, _, _)| creating || !app.session.visibility.hidden_origin.iter().any(|h| h == n))
        .collect()
}

/// A point on origin plane `name`'s square that a click at its screen point picks: the middle,
/// or else the middle of a quarter the other planes don't cover from this view (in an iso view
/// the XZ square hides the middle of the XY one). The middle when no candidate is picked.
pub fn origin_plane_target(app: &SolveApp, proj: &Proj, name: &str) -> Option<Vec3> {
    let (_, _, q) = origin_planes(app).into_iter().find(|(n, _, _)| *n == name)?;
    let mid = (q[0] + q[1] + q[2] + q[3]) / 4.0;
    let picked = |w: &Vec3| proj.to_screen(*w).is_some_and(|p| matches!(pick(app, proj, p).first(), Some(Hit::Plane { name: n, .. }) if n == name));
    Some(std::iter::once(mid).chain(q.iter().map(|c| (mid + *c) / 2.0)).find(picked).unwrap_or(mid))
}

pub fn origin_axes(app: &SolveApp) -> Vec<(&'static str, Vec3)> {
    if !app.origin_visible() {
        return Vec::new();
    }
    [("X", Vec3::X), ("Y", Vec3::Y), ("Z", Vec3::Z)]
        .into_iter()
        .filter(|(n, _)| !app.session.visibility.hidden_origin.iter().any(|h| h == n))
        .collect()
}

/// Construction axes shown, as segments: across the model where the axis passes it (a little
/// past it either side), at least the origin axes' length around the axis's origin: (name,
/// from, to).
pub fn construction_axis_segments(app: &SolveApp) -> Vec<(String, Vec3, Vec3)> {
    let axes: Vec<_> = solidcraft_engine::view::construction_axes(&app.session)
        .into_iter()
        .filter(|(_, n, _, _)| !app.session.visibility.hidden_origin.contains(n))
        .collect();
    if axes.is_empty() {
        return Vec::new();
    }
    let h = origin_size(app.cam.half_height()) * AXIS_LEN;
    let bb = view_bounds(app);
    axes.into_iter()
        .map(|(_, name, o, d)| {
            let (mut lo, mut hi) = (-h, h);
            if !bb.is_empty() {
                for i in 0..8 {
                    let c = Vec3::new(
                        if i & 1 == 0 { bb.min.x } else { bb.max.x },
                        if i & 2 == 0 { bb.min.y } else { bb.max.y },
                        if i & 4 == 0 { bb.min.z } else { bb.max.z },
                    );
                    let t = (c - o).dot(d);
                    lo = lo.min(t - h * 0.3);
                    hi = hi.max(t + h * 0.3);
                }
            }
            (name, o + d * lo, o + d * hi)
        })
        .collect()
}

/// Construction points shown: (name, position).
pub fn construction_points(app: &SolveApp) -> Vec<(String, Vec3)> {
    solidcraft_engine::view::construction_points(&app.session)
        .into_iter()
        .filter(|(_, n, _)| !app.session.visibility.hidden_origin.contains(n))
        .map(|(_, n, p)| (n, p))
        .collect()
}

/// Construction planes drawn as squares around their origin: (name, normal, quad).
pub fn construction_quads(app: &SolveApp) -> Vec<(String, Vec3, [Vec3; 4])> {
    let h = origin_size(app.cam.half_height()) * 1.2;
    solidcraft_engine::view::construction_planes(&app.session)
        .into_iter()
        .filter(|(_, n, _)| !app.session.visibility.hidden_origin.contains(n))
        .map(|(_, name, pl)| {
            let (o, x, y) = (pl.origin, pl.x * h, pl.y * h);
            (name, pl.normal(), [o - x - y, o + x - y, o + x + y, o - x + y])
        })
        .collect()
}

/// Pick what is under `pos`: sketch points, sketch curves, vertices, edges and axes first, then
/// the surfaces along the ray (profiles, faces, planes) nearest first.
/// Bounding boxes of the shown model's bodies (cached until the model changes).
fn body_bounds(
    app: &SolveApp,
    st: &std::sync::Arc<solidcraft_engine::doc::ModelState>,
) -> std::sync::Arc<Vec<(String, solidcraft_engine::geom::Aabb3)>> {
    if let Ok(mut g) = app.viewport.pick_bounds.lock() {
        if let Some((k, b)) = g.as_ref()
            && std::sync::Arc::ptr_eq(k, st)
        {
            return b.clone();
        }
        // Boxes of meshes the last state had too are kept (each body's box walks its mesh).
        let mut known = app.viewport.mesh_bounds.lock().map(|mut k| std::mem::take(&mut *k)).unwrap_or_default();
        let mut now = std::collections::HashMap::with_capacity(st.bodies.len());
        let b = std::sync::Arc::new(
            st.bodies
                .iter()
                .map(|b| {
                    let mesh = b.mesh();
                    let ptr = std::sync::Arc::as_ptr(&mesh) as usize;
                    let bounds = match known.remove(&ptr).or_else(|| now.remove(&ptr)) {
                        Some((_, bounds)) => bounds,
                        None => mesh.bounds(),
                    };
                    now.insert(ptr, (mesh, bounds));
                    (b.name.clone(), bounds)
                })
                .collect::<Vec<_>>(),
        );
        app.viewport.pick_index.retain(|k| now.contains_key(&k));
        if let Ok(mut k) = app.viewport.mesh_bounds.lock() {
            *k = now;
        }
        *g = Some((st.clone(), b.clone()));
        return b;
    }
    std::sync::Arc::new(Vec::new())
}

type BodyBounds = (std::sync::Arc<solidcraft_engine::doc::ModelState>, std::sync::Arc<Vec<(String, solidcraft_engine::geom::Aabb3)>>);

/// The model's extent: bodies (from the cached boxes) and sketches.
pub fn model_bounds(app: &SolveApp) -> solidcraft_engine::geom::Aabb3 {
    let st = app.session.world_state();
    let mut b = body_bounds(app, &st).iter().fold(solidcraft_engine::geom::Aabb3::EMPTY, |a, (_, x)| a.union(x));
    for ss in &st.sketches {
        if let Some((lo, hi)) = ss.sketch.bounds() {
            for p in [lo, hi] {
                b.add(ss.plane.to_world(p));
            }
        }
    }
    for point in crate::sketch_attachment_ui::guide_lines(app).into_iter().flatten() {
        b.add(point);
    }
    b
}

/// Where a ray enters a box (None: it misses).
fn ray_box(o: Vec3, d: Vec3, b: &solidcraft_engine::geom::Aabb3, pad: f64) -> Option<f64> {
    let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
    for (oo, dd, lo, hi) in [(o.x, d.x, b.min.x, b.max.x), (o.y, d.y, b.min.y, b.max.y), (o.z, d.z, b.min.z, b.max.z)] {
        let (lo, hi) = (lo - pad, hi + pad);
        if dd.abs() < 1e-15 {
            if oo < lo || oo > hi {
                return None;
            }
        } else {
            let (a, c) = ((lo - oo) / dd, (hi - oo) / dd);
            t0 = t0.max(a.min(c));
            t1 = t1.min(a.max(c));
        }
    }
    (t0 <= t1).then_some(t0)
}

/// The point of segment `a`–`b` nearest the ray from `o` along `d`.
fn ray_segment_closest(o: Vec3, d: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let u = b - a;
    let w = a - o;
    let (dd, du, uu) = (d.dot(d), d.dot(u), u.dot(u));
    let den = dd * uu - du * du;
    // Parallel or degenerate: every point is as near, so the start stands for the segment.
    if !(den.is_finite() && den > 1e-12 * dd * uu) {
        return a;
    }
    let s = (du * d.dot(w) - dd * u.dot(w)) / den;
    if !s.is_finite() {
        return a;
    }
    a + u * s.clamp(0.0, 1.0)
}

/// A box's outline on screen, grown by `px` (None: behind the eye).
fn screen_box(proj: &Proj, b: &solidcraft_engine::geom::Aabb3, px: f32) -> Option<Rect> {
    let mut r = Rect::NOTHING;
    for i in 0..8 {
        let p = Vec3::new(
            if i & 1 == 0 { b.min.x } else { b.max.x },
            if i & 2 == 0 { b.min.y } else { b.max.y },
            if i & 4 == 0 { b.min.z } else { b.max.z },
        );
        r.extend_with(proj.to_screen(p)?);
    }
    Some(r.expand(px))
}

/// What is under `pos`, reusing the last answer while the cursor, the camera and the model are
/// unchanged.
pub(crate) fn pick_cached(app: &mut SolveApp, proj: &Proj, pos: Pos2) -> Vec<Hit> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    pos.x.to_bits().hash(&mut h);
    pos.y.to_bits().hash(&mut h);
    for v in [app.cam.yaw, app.cam.pitch, app.cam.roll, app.cam.distance, app.cam.target.x, app.cam.target.y, app.cam.target.z] {
        v.to_bits().hash(&mut h);
    }
    app.session.revision.hash(&mut h);
    (std::sync::Arc::as_ptr(&app.session.world_state()) as usize).hash(&mut h);
    app.session.active_sketch.hash(&mut h);
    crate::sketch3d::drawing(app).hash(&mut h);
    app.dialog.as_ref().and_then(|d| d.active_input()).map(|i| i.accept).hash(&mut h);
    app.session.visibility.hidden_bodies.hash(&mut h);
    app.session.visibility.hidden_occurrences.hash(&mut h);
    app.perspective_view().hash(&mut h);
    (proj.rect.width().to_bits(), proj.rect.height().to_bits()).hash(&mut h);
    crate::sketch_display::projection_key(proj, app.viewport.pixels_per_point()).hash(&mut h);
    let key = h.finish();
    if let Some((k, hits)) = &app.viewport.last_pick
        && *k == key
    {
        return hits.clone();
    }
    // Hover feedback mirrors what a 3D bridge click will snap to; clicks still go through `pick`.
    let hits = pick_with_sketch_points(app, proj, pos, crate::sketch3d::drawing(app));
    app.viewport.last_pick = Some((key, hits.clone()));
    hits
}

/// What is under `pos`. Sketch points come from the active sketch only (from every shown sketch
/// while a dialog input takes points, while idle, or while projecting), so sketch tools never get
/// another sketch's local point and id. The 3D bridge snap asks for cross-sketch points explicitly with [`pick_3d`].
pub fn pick(app: &SolveApp, proj: &Proj, pos: Pos2) -> Vec<Hit> {
    pick_with_sketch_points(app, proj, pos, false)
}

/// Like [`pick`], but also returns points of every visible sketch: only for the 3D drawing snap,
/// which converts them to world space through their own sketch's plane.
pub fn pick_3d(app: &SolveApp, proj: &Proj, pos: Pos2) -> Vec<Hit> {
    pick_with_sketch_points(app, proj, pos, true)
}

/// Everything under `pos` (Select Other): like [`pick`], but every sketch point, sketch curve,
/// vertex, edge, axis and profile near the cursor, and the nearest face of every body along the
/// ray, instead of the nearest of each kind. Each kind is nearest first.
pub fn pick_all(app: &SolveApp, proj: &Proj, pos: Pos2) -> Vec<Hit> {
    pick_hits(app, proj, pos, false, true)
}

fn pick_with_sketch_points(app: &SolveApp, proj: &Proj, pos: Pos2, other_sketch_points: bool) -> Vec<Hit> {
    pick_hits(app, proj, pos, other_sketch_points, false)
}

/// The same thing picked twice (one row per target in [`pick_all`]).
pub fn same_target(a: &Hit, b: &Hit) -> bool {
    match (a, b) {
        (Hit::SketchPoint { sketch: s, id: i, .. }, Hit::SketchPoint { sketch: t, id: j, .. })
        | (Hit::SketchCurve { sketch: s, id: i, .. }, Hit::SketchCurve { sketch: t, id: j, .. }) => s == t && i == j,
        (Hit::Vertex { body: a, point: p }, Hit::Vertex { body: b, point: q }) => a == b && p.dist(*q) < 1e-9,
        (Hit::Edge { body: a, index: i, .. }, Hit::Edge { body: b, index: j, .. })
        | (Hit::Face { body: a, index: i, .. }, Hit::Face { body: b, index: j, .. }) => a == b && i == j,
        (Hit::Profile { sketch: s, index: i }, Hit::Profile { sketch: t, index: j }) => s == t && i == j,
        (Hit::Plane { name: a, .. }, Hit::Plane { name: b, .. }) | (Hit::Axis { name: a }, Hit::Axis { name: b }) => a == b,
        _ => false,
    }
}

/// The candidates of one kind: the nearest only, or with `all` every target (nearest first).
struct Near<T> {
    all: bool,
    v: Vec<(T, Hit)>,
}

impl<T: PartialOrd + Copy> Near<T> {
    fn new(all: bool) -> Self {
        Near { all, v: Vec::new() }
    }
    /// Whether a candidate at `d` would be kept.
    fn wants(&self, d: T) -> bool {
        self.all || self.v.first().is_none_or(|(b, _)| d < *b)
    }
    fn offer(&mut self, d: T, h: Hit) {
        if !self.all {
            if self.wants(d) {
                self.v = vec![(d, h)];
            }
            return;
        }
        match self.v.iter_mut().find(|(_, x)| same_target(x, &h)) {
            Some(e) if d < e.0 => *e = (d, h),
            Some(_) => {}
            None => self.v.push((d, h)),
        }
    }
    fn sorted(mut self) -> Vec<(T, Hit)> {
        self.v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        self.v
    }
    fn hits(self) -> impl Iterator<Item = Hit> {
        self.sorted().into_iter().map(|x| x.1)
    }
}

fn pick_hits(app: &SolveApp, proj: &Proj, pos: Pos2, other_sketch_points: bool, all: bool) -> Vec<Hit> {
    let s = &app.session;
    let st = s.world_state();
    let bounds = body_bounds(app, &st);
    let mut hits = Vec::new();
    let (o, d) = proj.ray(pos);
    // Sketch points of the active sketch (of every shown sketch while a command takes points,
    // while idle or projecting, and while 3D drawing snaps to points on other visible sketches).
    let wants_points = app.dialog.as_ref().and_then(|d| d.active_input()).is_some_and(|i| i.accept & crate::selection::POINTS != 0);
    let external_points =
        wants_points || other_sketch_points || (app.dialog.is_none() && app.tool.as_ref().is_none_or(|tool| tool.cmd == "sketch.project"));
    let point_sketches: Vec<u64> = if external_points { visible_sketches(app) } else { s.active_sketch.into_iter().collect() };
    // A face place (the Single hole's) snaps only to points their sketches show, so a nearer
    // point that isn't shown (a letter's outline) neither takes the pick nor hides a shown one.
    let snapping = app.dialog.as_ref().and_then(|d| d.active_input()).is_some_and(|i| crate::dialogs::snaps_to_points(i.accept));
    let mut best = Near::new(all);
    for ss in point_sketches.iter().filter_map(|sid| st.sketch(*sid)) {
        let sid = ss.feature;
        let shown = snapping.then(|| crate::dialogs::SnapPoints::of(&ss.sketch));
        let visible = solidcraft_engine::view::SketchPointVisibility::new(&ss.sketch);
        for p in &ss.sketch.points {
            if !visible.takes(p) || shown.as_ref().is_some_and(|f| !f.takes(p)) {
                continue;
            }
            if let Some(sp) = proj.to_screen(ss.plane.to_world(p.pos)) {
                let dd = sp.distance(pos) - if s.active_sketch == Some(sid) { 2.0 } else { 0.0 };
                if dd < 8.0 && best.wants(dd) {
                    best.offer(dd, Hit::SketchPoint { sketch: sid, id: p.id.clone(), at: p.pos });
                }
            }
        }
    }
    hits.extend(best.hits());
    // Sketch curves (the active sketch wins ties).
    let mut bestc = Near::new(all);
    for sid in visible_sketches(app) {
        let Some(ss) = st.sketch(sid) else { continue };
        let bonus = if s.active_sketch == Some(sid) { 2.0 } else { 0.0 };
        for (i, c) in ss.sketch.curves.iter().enumerate() {
            if !solidcraft_engine::view::sketch_curve_visible(&ss.sketch, i) {
                continue;
            }
            let straight = matches!(c.kind, CurveKind::Line { .. });
            for part in &crate::sketch_display::curve_sample(app, sid, i, &ss.plane, proj).parts {
                let pts: Vec<Pos2> = part.iter().filter_map(|q| proj.to_screen(*q)).collect();
                for w in pts.windows(2) {
                    let dd = seg_dist(pos, w[0], w[1]) - bonus;
                    if dd < 6.0 && bestc.wants(dd) {
                        bestc.offer(dd, Hit::SketchCurve { sketch: sid, id: c.id.clone(), straight });
                    }
                }
            }
        }
        // 3D sketch curves (paths for sweeps and pipes).
        for w in &ss.sketch.wires {
            let pts: Vec<Pos2> = w.pts.iter().filter_map(|q| proj.to_screen(*q)).collect();
            for seg in pts.windows(2) {
                let dd = seg_dist(pos, seg[0], seg[1]) - bonus;
                if dd < 6.0 && bestc.wants(dd) {
                    bestc.offer(dd, Hit::SketchCurve { sketch: sid, id: w.id.clone(), straight: false });
                }
            }
        }
    }
    hits.extend(bestc.hits());
    // Nearest face along the ray (hides what is behind it); with `all`, every body's nearest.
    let mut bestf: Option<(f64, Hit)> = None;
    let mut faces = Near::new(all);
    // Bounds retain the state's body order. Looking each one up by name made both passes
    // quadratic for assemblies with many small parts.
    for (i, b) in st.bodies.iter().enumerate() {
        if !app.session.body_visible(&b.name) {
            continue;
        }
        // Bodies whose box the ray misses (or enters behind the best hit so far) are skipped.
        if let Some((_, bb)) = bounds.get(i) {
            let pad = bb.diagonal() * 1e-6 + 1e-9;
            match ray_box(o, d, bb, pad) {
                None => continue,
                Some(t0) if !all && bestf.as_ref().is_some_and(|(bt, _)| t0 > *bt) => continue,
                _ => {}
            }
        }
        let m = b.mesh();
        if let Some((t, ti)) = crate::pick_index::raycast(&app.viewport.pick_index.of(&m), &m, o, d) {
            let fi = m.tri_face.get(ti).copied().unwrap_or(0) as usize;
            let hit = Hit::Face { body: b.name.clone(), index: fi, point: o + d * t };
            if bestf.as_ref().is_none_or(|(bt, _)| t < *bt) {
                bestf = Some((t, hit.clone()));
            }
            if all {
                faces.offer(t, hit);
            }
        }
    }
    let slack = scene_radius(app) * 0.02;
    // Wireframe draws no faces, so nothing hides an edge or a vertex.
    let wireframe = app.ui.visual_style == 2;
    let visible = |p: Vec3| wireframe || bestf.as_ref().is_none_or(|(t, _)| (p - o).dot(d) <= *t + slack);
    // Vertices (edge end points) and edges, within 7 and 6 points of the cursor.
    let reach = crate::pick_index::reach(proj, pos, 8.0);
    // The edges seen near the cursor, when the GPU pick buffer has them for this view (Select
    // Other wants hidden ones too).
    let seen = if all { None } else { crate::pick_buffer::edges_near(app, proj, pos, &st) };
    let mut bestv = Near::new(all);
    let mut beste = Near::new(all);
    for (i, b) in st.bodies.iter().enumerate() {
        if !app.session.body_visible(&b.name) {
            continue;
        }
        // Only bodies whose outline on screen is near the cursor can have an edge under it.
        if let Some((_, bb)) = bounds.get(i)
            && screen_box(proj, bb, 8.0).is_some_and(|r| !r.contains(pos))
        {
            continue;
        }
        let m = b.mesh();
        // Only the segments that can come near the cursor: of the edges the pick buffer sees
        // there, else from the index. Either way in the order a scan of every edge would meet
        // them, so ties go the same way.
        let near = match &seen {
            Some(seen) => seen.segments(i, &m),
            None => crate::pick_index::segments_near(&app.viewport.pick_index.of(&m), &m, o, d, reach),
        };
        let mut last_edge = None;
        for (ei, si) in near {
            let Some(e) = m.edges.get(ei) else { continue };
            if !m.edge_pickable(ei) {
                continue;
            }
            if last_edge != Some(ei) {
                last_edge = Some(ei);
                for v in [e.first(), e.last()].into_iter().flatten() {
                    if let Some(sp) = proj.to_screen(*v) {
                        let dd = sp.distance(pos);
                        if dd < 7.0 && visible(*v) && bestv.wants(dd) {
                            bestv.offer(dd, Hit::Vertex { body: b.name.clone(), point: *v });
                        }
                    }
                }
            }
            let (Some(p0), Some(p1)) = (e.get(si), e.get(si + 1)) else { continue };
            let (Some(a), Some(c)) = (proj.to_screen(*p0), proj.to_screen(*p1)) else { continue };
            let dd = seg_dist(pos, a, c);
            // Visible where the cursor is: the edge's point under the cursor, not an end or
            // the middle, which can lie deeper than the face the cursor is on.
            if dd < 6.0 && beste.wants(dd) && visible(ray_segment_closest(o, d, *p0, *p1)) {
                beste.offer(dd, Hit::Edge { body: b.name.clone(), index: ei, mid: polyline_mid(e) });
            }
        }
    }
    hits.extend(bestv.hits());
    hits.extend(beste.hits());
    // Origin axes.
    let size = origin_size(app.cam.half_height());
    let mut besta = Near::new(all);
    for (name, dir) in origin_axes(app) {
        if let (Some(a), Some(b)) = (proj.to_screen(Vec3::ZERO), proj.to_screen(dir * (size * AXIS_LEN))) {
            let dd = seg_dist(pos, a, b);
            if dd < 5.0 && besta.wants(dd) {
                besta.offer(dd, Hit::Axis { name: name.into() });
            }
        }
    }
    // Construction axes, picked like the origin axes.
    for (name, a, b) in construction_axis_segments(app) {
        if let (Some(a), Some(b)) = (proj.to_screen(a), proj.to_screen(b)) {
            let dd = seg_dist(pos, a, b);
            if dd < 5.0 && besta.wants(dd) {
                besta.offer(dd, Hit::Axis { name });
            }
        }
    }
    hits.extend(besta.hits());
    // Surfaces by depth: profiles (slightly preferred over a face they lie on), faces, planes.
    let mut surf: Vec<(f64, Hit)> = Vec::new();
    let mut bestp: Option<(f64, f64, Hit)> = None;
    let mut profiles_all: Vec<(f64, f64, Hit)> = Vec::new();
    for sid in visible_sketches(app) {
        let Some(ss) = st.sketch(sid) else { continue };
        let n = ss.plane.normal();
        let den = n.dot(d);
        if den.abs() < 1e-12 {
            continue;
        }
        let t = n.dot(ss.plane.origin - o) / den;
        if t < 0.0 {
            continue;
        }
        let lp = ss.plane.to_local(o + d * t);
        // Show Profile off: the sketch being edited offers no profiles.
        let profiles = if ss.sketch.view.hide_profile && s.active_sketch == Some(sid) { &[][..] } else { &ss.profiles[..] };
        for (pi, p) in profiles.iter().enumerate() {
            if !p.region.contains(lp) {
                continue;
            }
            if all {
                profiles_all.push((t, p.area, Hit::Profile { sketch: sid, index: pi }));
            } else if bestp.as_ref().is_none_or(|(_, a, _)| p.area < *a) {
                bestp = Some((t, p.area, Hit::Profile { sketch: sid, index: pi }));
            }
        }
    }
    // Smallest profile first.
    profiles_all.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (t, _, h) in bestp.into_iter().chain(profiles_all) {
        surf.push((t - slack, h));
    }
    if all {
        surf.extend(faces.sorted());
    } else if let Some(f) = bestf {
        surf.push(f);
    }
    // Planes give way to a face or profile at the same depth (a floor on the XZ plane).
    for (name, _, q) in origin_planes(app) {
        if let Some(t) = ray_quad(o, d, &q) {
            surf.push((t + slack, Hit::Plane { name: name.into(), point: o + d * t }));
        }
    }
    for (name, _, q) in construction_quads(app) {
        if let Some(t) = ray_quad(o, d, &q) {
            surf.push((t + slack, Hit::Plane { name, point: o + d * t }));
        }
    }
    surf.sort_by(|a, b| a.0.total_cmp(&b.0));
    hits.extend(surf.into_iter().map(|x| x.1));
    hits
}

/// The section cut shown now: the open Section Analysis dialog's plane, else the session's.
/// The active sketch's display options (defaults when not sketching).
pub fn active_view(app: &SolveApp) -> solidcraft_engine::sketch::SketchView {
    let st = app.session.world_state();
    app.session.active_sketch.and_then(|s| st.sketch(s)).map(|ss| ss.sketch.view.clone()).unwrap_or_default()
}

pub fn section_plane(app: &SolveApp) -> Option<(Vec3, Vec3)> {
    if let Some(d) = app.dialog.as_ref().filter(|d| matches!(d.kind, crate::dialogs::Kind::Section { .. })) {
        let cmds = crate::dialogs::apply_commands(app, d).ok()?;
        let (_, p) = cmds.into_iter().next()?;
        let mut scratch = app.session.scratch();
        scratch.execute("inspect.section", &p).ok()?;
        return scratch.section;
    }
    // Slice (Sketch Palette): cut the model at the plane of the sketch being edited.
    if app.session.section.is_none() {
        let st = app.session.world_state();
        if let Some(ss) = app.session.active_sketch.and_then(|s| st.sketch(s)).filter(|ss| ss.sketch.view.slice) {
            return Some((ss.plane.origin, ss.plane.normal()));
        }
    }
    app.session.section
}

/// Visible geometry belonging to a timeline feature. Body creators retain their whole-body
/// feedback; modifiers use persistent face names, which survive later edits and placement.
/// Do not expand these faces into surface patches: neighbouring faces can belong to another
/// feature even when they share a surface.
fn feature_geometry(app: &SolveApp, id: u64) -> Vec<Sel> {
    if !app.session.model.result(id).is_some_and(|r| !r.skipped && r.error.is_none()) {
        return Vec::new();
    }
    let st = app.session.world_state();
    let prefix = format!("F{id}:");
    let mut out = Vec::new();
    for b in &st.bodies {
        if !app.session.body_visible(&b.name) || app.preview.replaced.contains(&b.name) {
            continue;
        }
        if b.feature == id {
            out.push(Sel::Body { name: b.name.clone() });
            continue;
        }
        let names = solidcraft_engine::doc::naming::face_names(b);
        let m = b.mesh();
        for (index, _) in names.iter().enumerate().filter(|(_, n)| n.starts_with(&prefix)) {
            if let Some(point) = m.triangles.iter().zip(&m.tri_face).find_map(|(t, f)| {
                if *f as usize != index {
                    return None;
                }
                m.tri(t).map(|[a, b, c]| (a + b + c) / 3.0)
            }) {
                out.push(Sel::Face { body: b.name.clone(), index, point });
            }
        }
    }
    out
}

fn displayed_selection(app: &SolveApp, item: &Sel) -> Vec<Sel> {
    match item {
        Sel::Feature { id } => feature_geometry(app, *id),
        Sel::Occurrence { path } => app
            .session
            .occurrence_bodies(path, true)
            .unwrap_or_default()
            .into_iter()
            .filter(|name| app.session.body_visible(name))
            .map(|name| Sel::Body { name })
            .collect(),
        _ => grouped_selection(&app.session, item),
    }
}

/// The box around what is selected (bodies, faces, edges, vertices, sketch curves and the
/// bodies of selected features), where it is shown.
pub fn selection_bounds(app: &SolveApp) -> Option<solidcraft_engine::geom::Aabb3> {
    let st = app.session.world_state();
    let mut b = solidcraft_engine::geom::Aabb3::EMPTY;
    for s in app.session.selection.iter().flat_map(|item| displayed_selection(app, item)) {
        match &s {
            Sel::Body { name } => {
                if let Some(x) = st.body(name) {
                    b = b.union(&x.mesh().bounds());
                }
            }
            Sel::Face { body, index, .. } => {
                if let Some(x) = st.body(body) {
                    let m = x.mesh();
                    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
                        if *f as usize == *index {
                            for p in m.tri(t).into_iter().flatten() {
                                b.add(p);
                            }
                        }
                    }
                }
            }
            Sel::Edge { body, index, .. } => {
                if let Some(e) = st.body(body).and_then(|x| x.mesh().edges.get(*index).cloned()) {
                    for p in e {
                        b.add(p);
                    }
                }
            }
            Sel::Vertex { point, .. } => b.add(*point),
            Sel::ExternalSketchPoint { sketch, id } => {
                if let Some(ss) = st.sketch(*sketch)
                    && let Some(p) = ss.sketch.resolve_point(id).and_then(|i| ss.sketch.point(i))
                {
                    b.add(ss.plane.to_world(p));
                }
            }
            Sel::ExternalSketchCurve { sketch, id } => {
                if let Some(ss) = st.sketch(*sketch)
                    && let Some(i) = ss.sketch.curve_index(id)
                {
                    for p in ss.sketch.polyline(i) {
                        b.add(ss.plane.to_world(p));
                    }
                }
            }
            Sel::SketchCurve { id } => {
                for ss in &st.sketches {
                    if let Some(i) = ss.sketch.curve_index(id) {
                        for q in ss.sketch.polyline(i) {
                            b.add(ss.plane.to_world(q));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (!b.is_empty()).then_some(b)
}

/// The section as the shader's clip plane (normal, d), zero when there is none.
/// The surface analysis for the shader (see `gpu::Frame::analysis`).
fn analysis_uniform(app: &SolveApp) -> [f32; 8] {
    solidcraft_gpu::view::analysis_uniform(app.session.analysis.map(|a| a.mode()), crate::access::map(app).as_ref())
}

/// While Draft, Accessibility or Curvature Map analysis is on, what each of its colours means and
/// the values it stands for, at the bottom left of the viewport (#973). The colours alone don't
/// say, and with colour vision loss they can be hard to tell apart.
fn analysis_legend(app: &SolveApp, painter: &egui::Painter, rect: Rect) {
    use solidcraft_engine::SurfaceAnalysis as A;
    let t = Tokens::get();
    let tr = crate::i18n::tr_label;
    let col = |c: [f32; 3]| {
        let u = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        Color32::from_rgb(u(c[0]), u(c[1]), u(c[2]))
    };
    let dir = |v: Vec3| format!("({:.2}, {:.2}, {:.2})", v.x, v.y, v.z);
    let a = &t.analysis;
    // (title, the direction it measures along, rows of colour, text, hatched).
    let (title, along, rows): (&str, Option<String>, Vec<(Color32, String, bool)>) = match app.session.analysis {
        Some(A::Draft { pull, angle }) => (
            tr("Draft Analysis"),
            Some(format!("{} {}", tr("Pull direction"), dir(pull))),
            vec![
                (col(a.draft[0]), format!("≥ {angle:.1}°"), false),
                (col(a.draft[1]), format!("−{angle:.1}° … {angle:.1}°"), false),
                (col(a.draft[2]), format!("≤ −{angle:.1}°"), a.hatch),
            ],
        ),
        Some(A::Access { dir: d }) => (
            tr("Accessibility Analysis"),
            Some(format!("{} {}", tr("Tool direction"), dir(d))),
            vec![(col(a.access[0]), tr("Reachable").to_string(), false), (col(a.access[1]), tr("Blocked").to_string(), a.hatch)],
        ),
        // The shader's scale reaches its middle at twice the analysis radius.
        Some(A::Curvature { radius }) => (
            tr("Curvature Map Analysis"),
            None,
            vec![
                (col(a.curvature[0]), tr("Flat").to_string(), false),
                (col(a.curvature[1]), format!("{} {}", tr("Radius"), crate::prefs::show_mm(app, 2.0 * radius, 1)), false),
                (col(a.curvature[2]), format!("{} ≤ {}", tr("Radius"), crate::prefs::show_mm(app, radius, 1)), false),
            ],
        ),
        _ => return,
    };
    let (pad, row) = (8.0, 18.0);
    let h = pad * 2.0 + 18.0 + along.as_ref().map_or(0.0, |_| row) + row * rows.len() as f32;
    let r = Rect::from_min_size(pos2(rect.left() + 12.0, rect.bottom() - 12.0 - h), vec2(230.0, h));
    painter.rect(r, 6.0, t.overlay, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    crate::scenario::publish_handle("analysis:legend", r.center());
    let mut y = r.top() + pad;
    painter.text(pos2(r.left() + pad, y), Align2::LEFT_TOP, title, FontId::proportional(13.0), t.text);
    y += 18.0;
    if let Some(along) = along {
        painter.text(pos2(r.left() + pad, y), Align2::LEFT_TOP, along, FontId::proportional(11.5), t.text_dim);
        y += row;
    }
    for (c, text, hatched) in rows {
        let sw = Rect::from_min_size(pos2(r.left() + pad, y + 2.0), vec2(12.0, 12.0));
        painter.rect_filled(sw, 2.0, c);
        if hatched {
            // The stripes the viewport adds to these faces (colour vision modes).
            for k in [3.0, 7.0, 11.0] {
                painter.line_segment(
                    [pos2(sw.left() + k - 3.0, sw.bottom()), pos2(sw.left() + k + 1.0, sw.top())],
                    Stroke::new(1.2, Color32::from_black_alpha(110)),
                );
            }
        }
        painter.text(pos2(sw.right() + 8.0, y), Align2::LEFT_TOP, text, FontId::proportional(12.0), t.text);
        y += row;
    }
}

fn section_clip(app: &SolveApp) -> [f32; 4] {
    match section_plane(app) {
        Some((o, n)) => [n.x as f32, n.y as f32, n.z as f32, n.dot(o) as f32],
        None => [0.0; 4],
    }
}

/// Space kept free at the top right of the viewport for the axis view (docked dialogs start below).
pub(crate) fn axis_view_size(style: crate::prefs::NavigatorStyle) -> f32 {
    match style {
        crate::prefs::NavigatorStyle::Axes => 116.0,
        crate::prefs::NavigatorStyle::Cube => navigation_cube::SIZE,
    }
}
// Keep command panels anchored independently of the compact cube's display size.
pub const AXIS_VIEW_CLEARANCE: f32 = 168.0;

/// Axis length as a multiple of the origin plane size (solid part 1, dashed beyond).
const AXIS_LEN: f64 = 1.8;

/// The hit as a plain selection (no command filter).
pub fn hit_sel(h: &Hit) -> Option<Sel> {
    Some(match h {
        Hit::SketchPoint { id, .. } => Sel::SketchPoint { id: id.clone() },
        Hit::SketchCurve { id, .. } => Sel::SketchCurve { id: id.clone() },
        Hit::Vertex { body, point } => Sel::Vertex { body: body.clone(), point: *point },
        Hit::Edge { body, index, mid } => Sel::Edge { body: body.clone(), index: *index, point: *mid },
        Hit::Profile { sketch, index } => Sel::Profile { sketch: *sketch, index: *index },
        Hit::Face { body, index, point } => Sel::Face { body: body.clone(), index: *index, point: *point },
        Hit::Plane { name, .. } => Sel::Plane { name: name.clone() },
        Hit::Axis { name } => Sel::Axis { name: name.clone() },
    })
}

/// What lights up under the pointer while a tool runs: the nearest hit its click takes.
fn tool_hover(app: &SolveApp, hits: &[Hit]) -> Option<Hit> {
    let cmd = app.tool.as_ref()?.cmd;
    hits.iter().find(|h| crate::sketch_tools::takes(cmd, h)).cloned()
}

/// What a click would pick: the first hit the open dialog's active input accepts, or with no
/// dialog the first hit (sketch entities only belong to the active sketch).
pub fn candidate(app: &SolveApp, hits: &[Hit]) -> Option<(Hit, Sel)> {
    if let Some(d) = app.dialog.as_ref().filter(|d| d.wants_picks()) {
        let snap = d.face_snap(&app.session, hits);
        return hits.iter().find_map(|h| d.candidate_on(&app.session, h, snap.as_ref()).map(|s| (h.clone(), s)));
    }
    hits.iter().filter(|h| pick_allowed(app, h)).find_map(|h| hit_candidate(app, h))
}

/// Every hit a click could pick, in order, each with its selection (Select Other).
pub fn candidates(app: &SolveApp, hits: &[Hit]) -> Vec<(Hit, Sel)> {
    if let Some(d) = app.dialog.as_ref().filter(|d| d.wants_picks()) {
        // The face under the cursor is looked up once for every sketch point a face place may
        // snap to, before Select Other caps its rows.
        let snap = d.face_snap(&app.session, hits);
        return hits.iter().filter_map(|h| d.candidate_on(&app.session, h, snap.as_ref()).map(|s| (h.clone(), s))).collect();
    }
    hits.iter().filter(|h| pick_allowed(app, h)).filter_map(|h| hit_candidate(app, h)).collect()
}

/// What the selection filter lets a click pick with no dialog.
fn pick_allowed(app: &SolveApp, h: &Hit) -> bool {
    let off = |k: &str| app.ui.pick_off.iter().any(|x| x == k);
    match h {
        Hit::SketchCurve { .. } => !off("sketch") && !off("sketch_curves"),
        Hit::SketchPoint { .. } => !off("sketch") && !off("sketch_points"),
        Hit::Profile { .. } => !off("sketch"),
        Hit::Vertex { .. } => !off("vertices") && !app.ui.pick_bodies,
        Hit::Edge { .. } => !off("edges") && !app.ui.pick_bodies,
        Hit::Face { .. } => !off("faces") || app.ui.pick_bodies,
        Hit::Plane { .. } | Hit::Axis { .. } => true,
    }
}

/// The selection a click on `h` makes with no dialog (sketch entities only belong to the active sketch).
fn hit_candidate(app: &SolveApp, h: &Hit) -> Option<(Hit, Sel)> {
    match h {
        Hit::SketchCurve { sketch, id, .. } if app.session.active_sketch != Some(*sketch) => {
            Some((h.clone(), Sel::ExternalSketchCurve { sketch: *sketch, id: id.clone() }))
        }
        Hit::SketchPoint { sketch, id, .. } if app.session.active_sketch != Some(*sketch) => {
            Some((h.clone(), Sel::ExternalSketchPoint { sketch: *sketch, id: id.clone() }))
        }
        Hit::Face { body, .. } if app.ui.pick_bodies => Some((h.clone(), Sel::Body { name: body.clone() })),
        _ => hit_sel(h).map(|s| (h.clone(), s)),
    }
}

/// A pick for the open dialog's active input (a click, or a Select Other row).
pub(crate) fn dialog_pick(app: &mut SolveApp, hit: &Hit, sel: Sel) {
    // While OK computes, the dialog's inputs hold.
    if app.applying.is_some() {
        return;
    }
    let Some(mut d) = app.dialog.take() else { return };
    d.pick(&app.session, sel);
    if let crate::dialogs::Kind::PathPlane { sketch, .. } = &mut d.kind {
        *sketch = match hit {
            Hit::SketchCurve { sketch, .. } => Some(*sketch),
            _ => None,
        };
        d.extra.remove("path");
    }
    // Split Body's sketch line keeps the sketch it was picked in (curve ids repeat across sketches).
    if matches!(d.kind, crate::dialogs::Kind::Split) {
        if let Hit::SketchCurve { sketch, .. } = hit {
            d.extra.insert("tool_sketch".into(), serde_json::json!(sketch));
        } else {
            d.extra.remove("tool_sketch");
        }
    }
    app.dialog = Some(d);
}

/// The edge a selection refers to now: by index if it still passes through the stored point,
/// otherwise the edge nearest the point.
fn edge_of(m: &solidcraft_engine::geom::Mesh, index: usize, point: Vec3) -> Option<&Vec<Vec3>> {
    let tol = m.bounds().diagonal().max(1.0) * 1e-6;
    let near = |e: &Vec<Vec3>| e.windows(2).map(|w| point.dist_to_segment(w[0], w[1])).fold(f64::INFINITY, f64::min);
    if let Some(e) = m.edges.get(index)
        && near(e) < tol
    {
        return Some(e);
    }
    m.edges.iter().min_by(|a, b| near(a).total_cmp(&near(b))).filter(|e| near(e) < tol * 1e3)
}

fn face_tris(sc: &mut GpuScene, m: &solidcraft_engine::geom::Mesh, face: usize, col: [u8; 4], lit: bool) {
    mesh_tris(sc, m, col, lit, |f| f as usize == face);
}

/// Emit a set of faces in one mesh pass. Whole-body feedback must not scan every triangle
/// again for each face of a detailed imported part.
fn mesh_tris(sc: &mut GpuScene, m: &solidcraft_engine::geom::Mesh, col: [u8; 4], lit: bool, takes: impl Fn(u32) -> bool) {
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        if !takes(*f) {
            continue;
        }
        for k in t {
            let i = *k as usize;
            if let (Some(p), Some(n)) = (m.positions.get(i), m.normals.get(i)) {
                sc.tri(p.to_f32(), if lit { n.to_f32() } else { [0.0; 3] }, col);
            }
        }
    }
}

/// A highlighted edge: a halo and a core, and where the model hides it, the core dimmed (so a
/// pick on the far side still shows).
fn edge_lines(sc: &mut GpuScene, e: &[Vec3], core: Color32, halo: Color32, w: f32) {
    for (col, width) in [(halo, w + 3.0), (core, w)] {
        for p in e.windows(2) {
            sc.line(p[0].to_f32(), p[1].to_f32(), c4(col), width, false);
        }
    }
    for p in e.windows(2) {
        sc.hidden_line(p[0].to_f32(), p[1].to_f32(), c4(core.gamma_multiply(0.45)), (w * 0.7).max(1.0));
    }
}

/// The outline of a highlighted face where the model hides it (dimmed).
fn hidden_face_outline(sc: &mut GpuScene, m: &solidcraft_engine::geom::Mesh, face: usize, col: Color32) {
    for (ei, e) in m.edges.iter().enumerate() {
        let on = m.edge_faces.get(ei).is_some_and(|f| f.iter().any(|x| *x as usize == face));
        if on && m.edge_pickable(ei) {
            for p in e.windows(2) {
                sc.hidden_line(p[0].to_f32(), p[1].to_f32(), c4(col.gamma_multiply(0.45)), 1.4);
            }
        }
    }
}

pub(crate) fn profile_fill(sc: &mut GpuScene, app: &SolveApp, sketch: u64, index: usize, fill: Color32, edge: Color32) {
    let st = app.session.world_state();
    let Some(ss) = st.sketch(sketch) else { return };
    let Some(p) = ss.profiles.get(index) else { return };
    let plane = match crate::dialogs::emboss_profile_plane(app, sketch) {
        Ok(Some(plane)) => plane,
        Ok(None) => ss.plane,
        Err(_) => return,
    };
    // Lift toward the eye so a profile on a face shows over it.
    let lift = app.cam.back() * (app.cam.half_height() * 0.002);
    let w = |q: solidcraft_engine::geom::Vec2| (plane.to_world(q) + lift).to_f32();
    let col = c4(fill);
    // In the translucent pass (after the grid and edges) so it covers them.
    let proj = display_proj(app);
    let display = crate::sketch_display::profile_region(app, sketch, index, &plane, &proj);
    for [a, b, c] in display.as_ref().as_ref().map_or_else(Vec::new, |region| crate::sketch_display::fill_triangles(region, &mut 65_536)) {
        sc.trans_tri(w(a), [0.0; 3], col);
        sc.trans_tri(w(b), [0.0; 3], col);
        sc.trans_tri(w(c), [0.0; 3], col);
    }
    for lp in std::iter::once(&p.region.outer).chain(&p.region.holes) {
        for part in crate::sketch_display::boundary_parts(lp, &plane, &proj, app.viewport.pixels_per_point()) {
            for pair in part.windows(2) {
                sc.line((pair[0] + lift).to_f32(), (pair[1] + lift).to_f32(), c4(edge), 2.0, false);
            }
        }
    }
}

/// An origin or construction plane: reference geometry, which a section does not cut (#1229).
/// A reference plane's square. `through` draws it over the model (Create Sketch: a plane inside
/// a body stays visible and pickable), else it is depth-tested like the rest of the scene.
fn quad(sc: &mut GpuScene, q: &[Vec3; 4], fill: Color32, edge: Color32, through: bool) {
    let col = c4(fill);
    for i in [0, 1, 2, 0, 2, 3] {
        if let Some(p) = q.get(i) {
            if through {
                sc.xray_tri(p.to_f32(), [0.0; 3], col);
            } else {
                sc.guide_tri(p.to_f32(), col);
            }
        }
    }
    for i in 0..4 {
        let (a, b) = (q[i].to_f32(), q[(i + 1) % 4].to_f32());
        if through {
            sc.line(a, b, c4(edge), 1.2, true);
        } else {
            sc.guide_line(a, b, c4(edge), 1.2);
        }
    }
}

fn highlight_geometry_key(app: &SolveApp) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let selected = app.highlighted();
    app.session.revision.hash(&mut h);
    (std::sync::Arc::as_ptr(&app.session.world_state()) as usize).hash(&mut h);
    serde_json::to_string(&selected).unwrap_or_default().hash(&mut h);
    format!("{:?}", app.viewport.hover).hash(&mut h);
    app.viewport.hover_feature.hash(&mut h);
    app.viewport.hover_bodies.hash(&mut h);
    // Which mesh lights a face follows how the model draws its body (see `shown_mesh`).
    lod_generation(app).hash(&mut h);
    app.session.active_component.hash(&mut h);
    app.session.active_occurrence_path.hash(&mut h);
    app.ui.visual_style.hash(&mut h);
    app.ui.pick_bodies.hash(&mut h);
    app.session.active_sketch.hash(&mut h);
    (app.dialog.is_some(), app.tool.is_some()).hash(&mut h);
    crate::theme::is_dark().hash(&mut h);
    crate::theme::vision().hash(&mut h);
    app.preview.replaced.hash(&mut h);
    app.origin_visible().hash(&mut h);
    app.picking_sketch_plane().hash(&mut h);
    app.session.visibility.hidden_origin.hash(&mut h);
    app.session.visibility.hidden_bodies.hash(&mut h);
    app.session.visibility.hidden_occurrences.hash(&mut h);
    h.finish()
}

fn highlight_key(app: &SolveApp) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    highlight_geometry_key(app).hash(&mut h);
    let selected = app.highlighted();
    app.cam.half_height().to_bits().hash(&mut h);
    // Body triangles and outlines live in world space; the GPU applies the new camera.
    // Only sketch feedback needs projected tessellation and the profile's lift toward the eye.
    let projected = selected.iter().any(|s| matches!(s, Sel::Profile { .. } | Sel::SketchCurve { .. } | Sel::ExternalSketchCurve { .. }))
        || matches!(app.viewport.hover, Some(Hit::Profile { .. }));
    if projected {
        crate::sketch_display::projection_key(&display_proj(app), app.viewport.pixels_per_point()).hash(&mut h);
    }
    h.finish()
}

struct HoverBlock {
    key: u64,
    state: std::sync::Arc<solidcraft_engine::doc::ModelState>,
    doc: std::sync::Arc<solidcraft_engine::doc::Document>,
    block: crate::gpu::SceneBlock,
}

/// The highlight scene: origin widget, construction planes, selection and hover.
fn build_highlight(app: &SolveApp) -> GpuScene {
    let t = Tokens::get();
    let mut sc = GpuScene::default();
    let st = app.session.world_state();
    let s = &app.session;
    let active_bodies = s.active_occurrence_path.as_ref().and_then(|p| s.occurrence_bodies(p, true).ok());
    // Faces lit over the model use the triangles the model shows them with.
    let shown = |b: &solidcraft_engine::doc::ModelBody| shown_mesh(app, b, &active_bodies);
    let mut sel = Vec::new();
    for item in app.highlighted() {
        for piece in displayed_selection(app, &item) {
            if !sel.iter().any(|x| crate::selection::same_item(x, &piece)) {
                sel.push(piece);
            }
        }
    }
    let hover = app.viewport.hover.as_ref();
    let size = origin_size(app.cam.half_height());
    // Origin planes and construction planes.
    let plane_state = |name: &str| {
        let selected = sel.iter().any(|x| matches!(x, Sel::Plane { name: n } if n == name));
        let hovered = matches!(hover, Some(Hit::Plane { name: n, .. }) if n == name);
        (selected, hovered)
    };
    let mut planes: Vec<(String, [Vec3; 4], Color32)> = origin_planes(app).into_iter().map(|(n, _, q)| (n.to_string(), q, t.origin_plane)).collect();
    planes.extend(construction_quads(app).into_iter().map(|(n, _, q)| (n, q, t.construction_plane)));
    let through = app.picking_sketch_plane();
    for (name, q, base) in &planes {
        let (selected, hovered) = plane_state(name);
        let fill = if hovered {
            t.origin_plane_hover
        } else if selected {
            Color32::from_rgba_unmultiplied(t.sel_face.r(), t.sel_face.g(), t.sel_face.b(), 150)
        } else {
            *base
        };
        quad(&mut sc, q, fill, if selected { t.sel_edge } else { t.origin_plane_edge }, through);
    }
    // Origin axes: solid near the origin, dashed beyond.
    for (name, dir) in origin_axes(app) {
        let col = match name {
            "X" => t.axis_x,
            "Y" => t.axis_y,
            _ => t.axis_z,
        };
        let on = sel.iter().any(|x| matches!(x, Sel::Axis { name: n } if n == name)) || matches!(hover, Some(Hit::Axis { name: n }) if n == name);
        let w = if on { 4.0 } else { 2.0 };
        sc.guide_line([0.0; 3], (dir * size).to_f32(), c4(col), w);
        let dash = size * 0.08;
        let mut a = size + dash;
        while a < size * AXIS_LEN {
            sc.guide_line((dir * a).to_f32(), (dir * (a + dash)).to_f32(), c4(col), w * 0.75);
            a += dash * 2.0;
        }
    }
    // Construction axes (dashed, the length of the origin axes either side) and points (a
    // small star), in the construction colour.
    for (name, a, b) in construction_axis_segments(app) {
        let on = sel.iter().any(|x| matches!(x, Sel::Axis { name: n } if *n == name)) || matches!(hover, Some(Hit::Axis { name: n }) if *n == name);
        let col = c4(if on { t.sel_edge } else { t.construction_geom });
        let w = if on { 3.0 } else { 1.6 };
        let len = a.dist(b);
        let dash = size * 0.08;
        let mut s = 0.0;
        while s < len {
            let e = (s + dash).min(len);
            sc.guide_line(a.lerp(b, s / len).to_f32(), a.lerp(b, e / len).to_f32(), col, w);
            s += dash * 1.6;
        }
    }
    for (_, p) in construction_points(app) {
        let r = size * 0.04;
        for d in [Vec3::X, Vec3::Y, Vec3::Z] {
            sc.line((p - d * r).to_f32(), (p + d * r).to_f32(), c4(t.construction_geom), 2.0, true);
        }
    }
    // Reference widgets follow zoom; the often much larger world-space feedback does not.
    let projected = sel.iter().any(|s| matches!(s, Sel::Profile { .. } | Sel::SketchCurve { .. } | Sel::ExternalSketchCurve { .. }))
        || matches!(hover, Some(Hit::Profile { .. }));
    let key = highlight_geometry_key(app);
    if !projected
        && let Some(block) = app.viewport.hover_block.lock().ok().and_then(|cache| {
            cache
                .as_ref()
                .filter(|c| c.key == key && std::sync::Arc::ptr_eq(&c.state, &st) && std::sync::Arc::ptr_eq(&c.doc, &s.doc))
                .map(|c| c.block.clone())
        })
    {
        sc.blocks.push(block);
        return sc;
    }
    let mut feedback = GpuScene::default();
    // Selected things.
    for x in &sel {
        match x {
            Sel::Face { body, index, .. } => {
                // A live preview stands in for the body: its old faces would fight with it.
                if let Some(b) = st.body(body).filter(|_| app.session.body_visible(body) && !app.preview.replaced.contains(body)) {
                    face_tris(&mut feedback, &shown(b), *index, c4(t.sel_face), false);
                    hidden_face_outline(&mut feedback, &drawn_mesh(app, b), *index, t.sel_edge);
                }
            }
            Sel::Body { name } => {
                if let Some(b) = st.body(name)
                    && app.session.body_visible(name)
                {
                    let m = shown(b);
                    let count = b.body.face_count();
                    mesh_tris(&mut feedback, &m, c4(t.body_selected), true, |f| (f as usize) < count);
                }
            }
            Sel::Edge { body, index, point } => {
                if let Some(b) = st.body(body)
                    && app.session.body_visible(body)
                    && let Some(e) = edge_of(&drawn_mesh(app, b), *index, *point)
                {
                    edge_lines(&mut feedback, e, t.sel_edge, t.sel_edge_rim, 3.0);
                }
            }
            Sel::Profile { sketch, index } => profile_fill(&mut feedback, app, *sketch, *index, t.sel_profile, t.sel_profile_edge),
            Sel::ExternalSketchCurve { sketch, id } => {
                if let Some(ss) = st.sketch(*sketch)
                    && let Some(ci) = ss.sketch.curve_index(id)
                {
                    for pts in &crate::sketch_display::curve_sample(app, *sketch, ci, &ss.plane, &display_proj(app)).parts {
                        let mode = ss.sketch.curves.get(ci).map_or(0, |c| if c.centerline { 2 } else { u8::from(c.construction) });
                        for (a, b) in sketch_segments(pts, mode, grid_step(app.cam.half_height()).0) {
                            feedback.line(a.to_f32(), b.to_f32(), c4(t.sel_edge), 3.5, true);
                        }
                    }
                }
            }
            // A 3D sketch curve, in whichever sketch has it.
            Sel::SketchCurve { id } if st.sketches.iter().any(|ss| ss.sketch.wire_index(id).is_some()) => {
                for ss in st.sketches.iter() {
                    if let Some(w) = ss.sketch.wire_index(id).and_then(|i| ss.sketch.wires.get(i)) {
                        for q in w.pts.windows(2) {
                            feedback.line(q[0].to_f32(), q[1].to_f32(), c4(t.sel_edge), 3.5, true);
                        }
                    }
                }
            }
            Sel::SketchCurve { id } => {
                if let Some(ss) = app.session.active_sketch.and_then(|sid| st.sketch(sid))
                    && let Some(ci) = ss.sketch.curve_index(id)
                {
                    for pts in &crate::sketch_display::curve_sample(app, ss.feature, ci, &ss.plane, &display_proj(app)).parts {
                        let mode = ss.sketch.curves.get(ci).map_or(0, |c| if c.centerline { 2 } else { u8::from(c.construction) });
                        for (a, b) in sketch_segments(pts, mode, grid_step(app.cam.half_height()).0) {
                            feedback.line(a.to_f32(), b.to_f32(), c4(t.sel_edge), 3.5, true);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // Timeline modifiers light only their resulting faces; browser rows light whole bodies.
    let mut hovered = app.viewport.hover_feature.map(|id| feature_geometry(app, id)).unwrap_or_default();
    hovered.extend(app.viewport.hover_bodies.iter().map(|name| Sel::Body { name: name.clone() }));
    for b in st.bodies.iter().filter(|b| app.session.body_visible(&b.name) && !app.preview.replaced.contains(&b.name)) {
        let whole = hovered.iter().any(|x| matches!(x, Sel::Body { name } if name == &b.name));
        let faces: Vec<_> = if whole {
            (0..b.body.face_count()).collect()
        } else {
            hovered
                .iter()
                .filter_map(|x| match x {
                    Sel::Face { body, index, .. } if body == &b.name => Some(*index),
                    _ => None,
                })
                .collect()
        };
        if faces.is_empty() {
            continue;
        }
        let k = t.hover_face_lift;
        let c = body_rgb(app, &b.name);
        let m = drawn_mesh(app, b);
        let lit = shown(b);
        let selected_body = sel.iter().any(|x| matches!(x, Sel::Body { name } if name == &b.name));
        if !selected_body {
            let selected_faces: std::collections::HashSet<usize> = sel
                .iter()
                .filter_map(|x| {
                    if let Sel::Face { body, index, .. } = x
                        && body == &b.name
                    {
                        Some(*index)
                    } else {
                        None
                    }
                })
                .collect();
            let hovered_faces: std::collections::HashSet<usize> = faces.iter().copied().collect();
            mesh_tris(&mut feedback, &lit, [c.0.saturating_add(k), c.1.saturating_add(k), c.2.saturating_add(k), 255], true, |f| {
                hovered_faces.contains(&(f as usize)) && !selected_faces.contains(&(f as usize))
            });
        }
        for (ei, e) in m.edges.iter().enumerate() {
            if m.edge_pickable(ei) && (whole || m.edge_faces.get(ei).is_some_and(|fs| fs.iter().any(|f| faces.contains(&(*f as usize))))) {
                edge_lines(&mut feedback, e, t.hover_edge, t.hover_edge_halo, 1.5);
            }
        }
    }
    // Hover (pre-highlight).
    match hover {
        // Body selection: the whole body lights up.
        Some(Hit::Face { body, .. }) if app.ui.pick_bodies && app.dialog.is_none() && app.tool.is_none() => {
            if let Some(b) = st.body(body) {
                let k = t.hover_face_lift;
                let c = body_rgb(app, body);
                let m = shown(b);
                let count = b.body.face_count();
                mesh_tris(&mut feedback, &m, [c.0.saturating_add(k), c.1.saturating_add(k), c.2.saturating_add(k), 255], true, |f| {
                    (f as usize) < count
                });
            }
        }
        Some(Hit::Face { body, index, .. })
            if !app.preview.replaced.contains(body)
                && !sel.iter().any(|x| matches!(x, Sel::Face { body: b, index: i, .. } if b == body && i == index)) =>
        {
            if let Some(b) = st.body(body) {
                let k = t.hover_face_lift;
                let c = body_rgb(app, body);
                let m = drawn_mesh(app, b);
                let patch = m.surface_patch(*index);
                let lit = shown(b);
                for &f in &patch {
                    face_tris(&mut feedback, &lit, f, [c.0.saturating_add(k), c.1.saturating_add(k), c.2.saturating_add(k), 255], true);
                }
                for (e, _) in m.edge_faces.iter().enumerate().filter(|(_, fs)| fs.iter().any(|f| patch.contains(&(*f as usize)))) {
                    if let Some(p) = m.edges.get(e)
                        && m.edge_pickable(e)
                    {
                        edge_lines(&mut feedback, p, t.hover_edge, t.hover_edge_halo, 1.5);
                    }
                }
            }
        }
        Some(Hit::Edge { body, index, mid }) => {
            if let Some(b) = st.body(body) {
                // Lit over the drawn edges (finer away from Normal display quality).
                let m = drawn_mesh(app, b);
                for i in m.circular_rim(*index) {
                    let selected = sel.iter().any(|x| matches!(x, Sel::Edge { body: b, index: j, .. } if b == body && *j == i));
                    if !selected && let Some(e) = if i == *index { edge_of(&m, i, *mid) } else { m.edges.get(i) } {
                        edge_lines(&mut feedback, e, t.hover_edge, t.hover_edge_halo, 2.0);
                    }
                }
            }
        }
        Some(Hit::Profile { sketch, index })
            if !sel.iter().any(|x| matches!(x, Sel::Profile { sketch: s2, index: i2 } if s2 == sketch && i2 == index)) =>
        {
            profile_fill(&mut feedback, app, *sketch, *index, t.hover_profile, t.hover_profile_edge);
        }
        _ => {}
    }
    if projected {
        sc.tris.append(&mut feedback.tris);
        sc.trans.append(&mut feedback.trans);
        sc.lines.append(&mut feedback.lines);
        sc.hidden.append(&mut feedback.hidden);
        sc.overlay.append(&mut feedback.overlay);
    } else {
        let block = crate::gpu::SceneBlock {
            key,
            bytes: std::sync::Arc::new(crate::gpu::BlockBytes {
                tris: feedback.tris,
                lines: feedback.lines,
                hidden: feedback.hidden,
                ..Default::default()
            }),
        };
        if let Ok(mut cache) = app.viewport.hover_block.lock() {
            *cache = Some(HoverBlock { key, state: st.clone(), doc: s.doc.clone(), block: block.clone() });
        }
        sc.blocks.push(block);
    }

    sc
}

/// A body's display colour (appearance, material or imported colour), if it has one.
fn body_color(doc: &solidcraft_engine::doc::Document, b: &solidcraft_engine::doc::ModelBody) -> Option<Color32> {
    let to8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    doc.body_color(b).map(|[r, g, b]| Color32::from_rgb(to8(r), to8(g), to8(b)))
}

/// A body's display colour (else the theme's) as RGB.
fn body_rgb(app: &SolveApp, name: &str) -> (u8, u8, u8) {
    let st = app.session.world_state();
    let c = st.body(name).and_then(|b| body_color(&app.session.doc, b)).unwrap_or(Tokens::get().body);
    (c.r(), c.g(), c.b())
}

pub(crate) fn polyline_mid(pts: &[Vec3]) -> Vec3 {
    let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    let mut acc = 0.0;
    for w in pts.windows(2) {
        let l = w[0].dist(w[1]);
        if acc + l >= len * 0.5 && l > 0.0 {
            return w[0].lerp(w[1], (len * 0.5 - acc) / l);
        }
        acc += l;
    }
    pts.first().copied().unwrap_or_default()
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_sq();
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// Point on the active sketch plane under the cursor (sketch coordinates), snapped to sketch
/// points within a few pixels and otherwise to a fine grid. Returns the snapped point and the
/// id of the sketch point it snapped to.
pub fn sketch_point_at(app: &SolveApp, proj: &Proj, pos: Pos2) -> Option<(Vec2, Option<String>)> {
    let sid = app.session.active_sketch?;
    let st = app.session.world_state();
    let ss = st.sketch(sid)?;
    for h in pick(app, proj, pos) {
        if let Hit::SketchPoint { sketch, id, at } = h
            && sketch == sid
        {
            return Some((at, Some(id)));
        }
    }
    let (o, d) = proj.ray(pos);
    let w = ss.plane.intersect_ray(o, d)?;
    let lp = ss.plane.to_local(w);
    if crate::drag_snap::suppressed() {
        return Some((lp, None));
    }
    let (minor, _) = grid_step(app.cam.half_height());
    let step = minor / 10.0;
    if let Some(p) = crate::sketch_tools::refine_snap(app, proj, &ss.sketch, &ss.plane, lp) {
        return Some(p);
    }
    // With Snap off (Sketch Palette) the point is where the cursor is.
    if ss.sketch.view.no_snap {
        return Some((lp, None));
    }
    Some((Vec2::new((lp.x / step).round() * step, (lp.y / step).round() * step), None))
}

pub fn delete_selection(app: &mut SolveApp) {
    crate::delete::delete_selection(app);
}

pub fn show(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let rect = ui.max_rect();
    app.viewport.rect = Some(rect);
    refresh_layout(app, ui.ctx().pixels_per_point());
    let resp = ui.interact(rect, ui.id().with("viewport"), Sense::click_and_drag());
    let flat = crate::dialogs_sheet::flat_active(app);
    if !flat {
        app.flat_view.fitted = false;
    }
    let input =
        if flat { crate::dialogs_sheet::navigate(app, ui, rect, &resp) } else { crate::navigation::handle(app, ui, rect, &resp, NavCaps::MODEL) };
    let CameraInput {
        hover,
        scroll,
        mods,
        middle,
        secondary_down,
        primary_down,
        primary_pressed,
        delta,
        inside,
        axes,
        pinch,
        pan,
        primary_nav,
        mode,
    } = input;
    app.viewport.physical_scale = ui.ctx().pixels_per_point();
    let proj = projection(app, rect);
    if let Ok(mut cache) = app.viewport.display.lock() {
        cache.prepare(app.session.world_state(), &proj, app.viewport.pixels_per_point());
    }
    // Suppress hover for the whole navigation gesture, including frames between pointer
    // events. Testing delta made the highlight reappear whenever a held drag paused.
    // Resolve it before rendering so the GPU scene and screen overlays use the same hit.
    // A 3D mouse moving the view is not such a gesture: the pointer hand is free, and hover
    // and clicks keep picking while the other hand turns the view.
    let navigating = scroll != 0.0
        || pinch != 1.0
        || pan != egui::Vec2::ZERO
        || middle
        || secondary_down
        || (primary_nav.is_some() && primary_down)
        || mode.is_some()
        || app.viewport.axis_drag.is_some()
        || app.viewport.spin
        || app.viewport.orbit_snap.is_some()
        || app.cam_anim.is_some();
    let hits = hover.filter(|_| inside && app.viewport.boxsel.is_none() && !navigating).map(|p| pick_cached(app, &proj, p)).unwrap_or_default();
    // The Fillet and Chamfer window resolves its own corners: the nearest sketch point to a
    // corner drawn from the origin is the fixed origin, which ends no two curves.
    let corner = hover.filter(|_| inside && !navigating).and_then(|p| crate::sketch_modify_ui::corner_hit(app, &proj, p));
    let cand = if app.tool.is_some() {
        tool_hover(app, &hits).map(|h| (h, None))
    } else if let Some((hit, sel)) = corner {
        Some((hit, Some(sel)))
    } else {
        candidate(app, &hits).map(|(h, s)| (h, Some(s)))
    };
    app.viewport.hover = cand.as_ref().map(|c| c.0.clone());
    // Select Other: a press held still opens the list of everything under it; while the list is
    // open its row under the pointer is what lights up.
    let held =
        crate::select_other::track_hold(app, ui.ctx(), &proj, inside, primary_pressed, primary_down, hover, primary_nav.is_some() || mode.is_some());
    crate::select_other::hover_override(app);
    let painter = ui.painter_at(rect);
    // Background gradient.
    let (top, bot) = (t.viewport_top, t.viewport_bottom);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bot);
    mesh.colored_vertex(rect.left_bottom(), bot);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));

    // ---- model ----
    update_refinement(app, ui.ctx(), &proj);
    let key = scene_key(app);
    let t0 = crate::now_ms();
    // Drawn on the GPU; without one (the headless harness, tests) the model is not drawn.
    if app.viewport.gpu.is_some() {
        if key != app.viewport.key {
            let sc = build_scene(app);
            if let Ok(mut slot) = app.viewport.slot.lock() {
                *slot = Some(sc);
            }
            app.viewport.key = key;
            app.viewport.build_ms = crate::now_ms() - t0;
        }
        let sk = sketch_scene_key(app, &proj);
        if sk != app.viewport.sketch_key {
            let scene = build_sketch_scene(app, &proj);
            if let Ok(mut slot) = app.viewport.sketch_slot.lock() {
                *slot = Some(scene);
            }
            app.viewport.sketch_key = sk;
        }
        let hk = highlight_key(app);
        if hk != app.viewport.hl_key {
            let hl = build_highlight(app);
            if let Ok(mut slot) = app.viewport.hl_slot.lock() {
                *slot = Some(hl);
            }
            app.viewport.hl_key = hk;
        }
        let ppp = ui.ctx().pixels_per_point();
        let size_px = [rect.width() * ppp, rect.height() * ppp];
        let ssaa = supersampling(app, ui.ctx(), proj.vp.to_f32(), size_px);
        let cb = ViewportCallback(Frame {
            ssaa,
            product: false,
            key: app.viewport.key,
            slot: app.viewport.slot.clone(),
            sketch_key: app.viewport.sketch_key,
            sketch_slot: app.viewport.sketch_slot.clone(),
            hl_key: app.viewport.hl_key,
            hl_slot: app.viewport.hl_slot.clone(),
            pv_key: app.preview.key,
            pv_slot: app.preview.slot.clone(),
            clip: section_clip(app),
            cap: t.section_cap.to_normalized_gamma_f32(),
            view_proj: proj.vp.to_f32(),
            back: proj.cam.back().to_f32(),
            size_px,
            images: crate::ref_images::gpu_images(app),
            analysis: analysis_uniform(app),
            analysis_colors: crate::gpu::analysis_colors(&t.analysis),
            access: crate::access::map(app),
            outline: matches!(app.ui.visual_style, 0 | 3),
            eye: {
                let e = proj.cam.eye().to_f32();
                [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }]
            },
        });
        painter.add(egui_wgpu::Callback::new_paint_callback(rect, cb));
        crate::pick_buffer::paint(app, ui.ctx(), &painter, &proj);
    }
    analysis_legend(app, &painter, rect);
    crate::frame_rate::paint(app, ui.ctx(), &painter, rect);

    // ---- interaction ----
    // A two-point shape being dragged out (#1630) follows the pointer and ends where it is let go,
    // even over the shape's own dimension box, which rides along beside the pointer.
    if app.viewport.draw_drag {
        let at = ui.input(|i| i.pointer.latest_pos()).filter(|p| rect.contains(*p));
        if resp.drag_stopped() {
            app.viewport.draw_drag = false;
            if let Some(p) = at
                && app.tool.as_ref().is_some_and(|t| t.pts.len() == 1)
            {
                crate::tools::on_hover(app, &proj, p);
                crate::tools::on_click(app, &proj, p);
            }
        } else if let Some(p) = at {
            crate::tools::on_hover(app, &proj, p);
        }
    }
    let add = mods.shift || mods.command || mods.ctrl;
    crate::sketch_text::set_one_letter(mods.shift);
    if let Some(p) = hover.filter(|_| inside) {
        if app.tool.is_some() {
            crate::tools::on_hover(app, &proj, p);
        }
        // Rectangles, circles and polygons also draw by press, drag and release (#1630): the press
        // places the first point as a click there would, the release the second.
        let two_point_start = app.tool.as_ref().is_some_and(|t| matches!(t.kind, crate::tools::Kind::Draw(2)) && t.pts.is_empty());
        if two_point_start
            && !held
            && primary_nav.is_none()
            && mode.is_none()
            && resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(a) = ui.input(|i| i.pointer.press_origin())
        {
            crate::tools::on_click(app, &proj, a);
            app.viewport.draw_drag = app.tool.as_ref().is_some_and(|t| t.pts.len() == 1);
        }
        let reference_pick = (resp.clicked() || resp.double_clicked())
            && !held
            && app.dialog.is_none()
            && crate::dim_view::pick_reference(ui.ctx(), p, resp.double_clicked());
        if resp.clicked() && !held && !reference_pick {
            if app.tool.is_some() {
                crate::tools::on_click(app, &proj, p);
            } else if app.dialog.as_ref().is_some_and(|d| d.wants_picks()) {
                if let Some((hit, Some(sel))) = cand {
                    dialog_pick(app, &hit, sel);
                } else if let Some(mut dialog) = app.dialog.take() {
                    let state = app.session.world_state();
                    if let Some(sketch) = app.session.active_sketch.and_then(|id| state.sketch(id)) {
                        let (origin, direction) = proj.ray(p);
                        if let Some(point) = sketch.plane.intersect_ray(origin, direction) {
                            crate::sketch_modify_ui::pick_position(&mut dialog, &app.session, point);
                        }
                    }
                    app.dialog = Some(dialog);
                }
            } else {
                // Dimensions and constraint glyphs sit on top of the geometry.
                let off = |k: &str| app.ui.pick_off.iter().any(|x| x == k);
                let sketching = app.session.active_sketch.is_some();
                // A dimension's text wins; its lines only where no sketch point or curve is (a
                // diameter's line runs through the centre point).
                let on_geometry = matches!(cand.as_ref().map(|c| &c.0), Some(Hit::SketchPoint { .. } | Hit::SketchCurve { .. }));
                let on_dim = crate::dim_view::hit_text(p)
                    .or_else(|| if on_geometry { None } else { crate::dim_view::hit(p) })
                    .filter(|_| sketching && !off("dimensions"));
                let on_glyph = crate::sketch_tools::glyph_at(p).filter(|_| sketching && !off("constraints"));
                if crate::sketch_text_handles::over(app, &proj, p) {
                    // A text's resize or rotation handle: the text stays selected.
                } else if !crate::sketch3d::click(app, &proj, p) {
                    match on_dim.or(on_glyph) {
                        Some(id) => select(app, Some(Sel::SketchConstraint { id }), add),
                        // Clicking again on the same spot selects the next thing under it (not
                        // the second click of a double-click, which has its own actions).
                        None if !add && !resp.double_clicked() && crate::select_other::cycle_click(app, &proj, p) => {}
                        None => {
                            let sel = cand.and_then(|c| c.1);
                            select(app, sel.clone(), add);
                            crate::select_other::note_click(app, p, if add { None } else { sel });
                        }
                    }
                }
            }
        }
        // Double-click a dimension: edit its value in place.
        let dim_edit = resp.double_clicked()
            && !held
            && !crate::dim_view::editing()
            && app.tool.is_none()
            && app.dialog.is_none()
            && (crate::sketch_modify_ui::double_click_pattern(app, p) || crate::dim_view::double_click(app, p));
        // Double-click a text (an outline or inside a letter; the first click selected it): edit it.
        if resp.double_clicked()
            && !held
            && !dim_edit
            && app.tool.is_none()
            && app.dialog.is_none()
            && let Some(entity) = crate::sketch_text::selected_entity(app)
        {
            crate::sketch_text::open(app, &entity);
        }
        // Double-click a sketch curve: select the chain joined to it end to end.
        if resp.double_clicked()
            && !held
            && !dim_edit
            && app.tool.is_none()
            && app.dialog.is_none()
            && let Some(Hit::SketchCurve { sketch, id, .. }) = app.viewport.hover.clone()
            && app.session.active_sketch == Some(sketch)
            && let Ok(v) = app.session.execute("sketch.chain", &json!({ "curve": id }))
        {
            let items: Vec<Sel> =
                v["curves"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).map(|id| Sel::SketchCurve { id: id.into() }).collect();
            let _ = app.run("select.set", json!({ "items": items, "add": add }));
        }
        // Double-click an edge: select the edges that continue it smoothly (the loop).
        if resp.double_clicked()
            && !held
            && !dim_edit
            && app.tool.is_none()
            && app.dialog.is_none()
            && let Some(Hit::Edge { body, index, .. }) = app.viewport.hover.clone()
            && let Some(b) = app.session.world_state().body(&body).cloned()
        {
            let m = b.mesh();
            let items: Vec<Sel> = m
                .tangent_chain(index, 2f64.to_radians())
                .into_iter()
                .filter_map(|i| m.edges.get(i).map(|e| Sel::Edge { body: body.clone(), index: i, point: polyline_mid(e) }))
                .collect();
            let _ = app.run("select.set", json!({ "items": items, "add": add }));
        }
        if resp.secondary_clicked() && delta == egui::Vec2::ZERO {
            if app.tool.is_some() {
                crate::tools::finish(app);
            } else if app.dialog.is_none() {
                crate::context_menu::open(app, p);
            }
        }
    }
    // Dragging a sketch point of the active sketch moves it; the solver keeps the constraints.
    if app.tool.is_none() && app.dialog.is_none() && primary_nav.is_none() && mode.is_none() && app.session.active_sketch.is_some() && !held {
        let on_plane = |app: &SolveApp, p: Pos2| {
            let ss = app.session.active_sketch.and_then(|sid| app.session.world_state().sketch(sid).cloned())?;
            let (o, d) = proj.ray(p);
            ss.plane.intersect_ray(o, d).map(|w| ss.plane.to_local(w))
        };
        // What a drag starts on: the 3D triad, then a dimension's text, then sketch geometry
        // (before the dimension's lines, which end on the geometry they measure), as for clicks.
        let grabbed = resp
            .drag_started_by(egui::PointerButton::Primary)
            .then(|| ui.input(|i| i.pointer.press_origin()))
            .flatten()
            .filter(|a| !crate::sketch_text_handles::drag_start(app, &proj, *a) && !crate::sketch3d::drag_start(app, &proj, *a))
            .map(|a| (a, pick(app, &proj, a).into_iter().next()));
        if let Some((a, hit)) = &grabbed
            && !crate::sketch_text_handles::dragging(app)
            && let Some(q) = on_plane(app, *a)
        {
            let on_geometry = matches!(hit, Some(Hit::SketchPoint { .. } | Hit::SketchCurve { .. }));
            crate::dim_view::drag_start(app, *a, q, on_geometry);
        }
        if crate::sketch3d::dragging()
            && (resp.dragged_by(egui::PointerButton::Primary) || resp.drag_stopped())
            && let Some(p) = hover
        {
            crate::sketch3d::drag_to(app, &proj, p);
        }
        if crate::sketch_text_handles::dragging(app)
            && (resp.dragged_by(egui::PointerButton::Primary) || resp.drag_stopped())
            && let Some(p) = hover
        {
            crate::sketch_text_handles::drag_to(app, &proj, p, mods.shift);
        }
        if crate::dim_view::dragging()
            && (resp.dragged_by(egui::PointerButton::Primary) || resp.drag_stopped())
            && let Some(q) = hover.and_then(|p| on_plane(app, p))
        {
            crate::dim_view::drag_to(app, q);
        }
        // A point or a curve of the sketch being edited follows a drag that starts on it.
        if let Some((a, hit)) = grabbed
            && !crate::dim_view::dragging()
            && !crate::sketch3d::dragging()
            && !crate::sketch_text_handles::dragging(app)
        {
            let active = app.session.active_sketch;
            match hit {
                Some(Hit::SketchPoint { sketch, id, .. }) if Some(sketch) == active => {
                    app.viewport.point_drag = Some((id, app.session.undo.len(), None))
                }
                Some(Hit::SketchCurve { sketch, id, .. }) if Some(sketch) == active => {
                    app.viewport.point_drag = on_plane(app, a).map(|q| (id, app.session.undo.len(), Some(q)));
                }
                _ => {}
            }
            if let Some((id, _, _)) = &app.viewport.point_drag {
                let link = app.session.active_sketch.and_then(|sid| app.session.doc.sketch(sid).ok()).and_then(|sk| {
                    let link = sk
                        .curve_index(id)
                        .and_then(|i| sk.curves.get(i))
                        .and_then(|c| c.link.as_deref())
                        .or_else(|| sk.resolve_point(id).and_then(|i| sk.points.get(i)).and_then(|p| p.link.as_deref()))?;
                    sk.link(link).filter(|l| l.kind == solidcraft_engine::sketch::LinkKind::Text).map(|l| l.id.clone())
                });
                if let Some(link) = link
                    && let Some(q) = on_plane(app, a)
                {
                    if let Some((_, _, grab)) = &mut app.viewport.point_drag {
                        *grab = Some(q);
                    }
                    if let Ok(result) = app.session.execute("edit.checkpoint", &json!({}))
                        && let Some(checkpoint) = result["checkpoint"].as_u64()
                    {
                        app.viewport.text_drag = Some((checkpoint, link));
                    }
                }
            }
        }
        if let Some((id, depth, grab)) = app.viewport.point_drag.clone()
            && resp.dragged_by(egui::PointerButton::Primary)
            && let Some(mut q) = hover.and_then(|p| on_plane(app, p))
        {
            let from = grab.unwrap_or(q);
            // Snap the dragged point (or the line's ends) to other geometry, like drawing.
            let snap = {
                let st = app.session.world_state();
                app.session.active_sketch.and_then(|s| st.sketch(s)).and_then(|ss| {
                    let sk = &ss.sketch;
                    let (own, exclude) = crate::drag_snap::moving_points(sk, &id);
                    if own.iter().any(|&p| sk.point_locked(p)) {
                        return crate::drag_snap::find(sk, &[], &[], 0.0);
                    }
                    let a = proj.to_screen(ss.plane.to_world(q))?;
                    let b = proj.to_screen(ss.plane.to_world(q + Vec2::X))?;
                    let mm_px = 1.0 / (a.distance(b) as f64).max(1e-9);
                    let d = q - from;
                    let moving: Vec<(usize, Vec2)> =
                        own.iter().filter_map(|&p| if grab.is_some() { sk.point(p).map(|x| (p, x + d)) } else { Some((p, q)) }).collect();
                    crate::drag_snap::find(sk, &moving, &exclude, mm_px)
                })
            };
            if let Some(s) = snap {
                q += s.offset;
            }
            // Each frame re-solves the sketch only (`live`); the model follows when the drag ends.
            let at = match app.session.execute("sketch.drag", &json!({"entity": id, "from": [from.x, from.y], "to": [q.x, q.y], "live": true})) {
                Ok(r) => {
                    app.session.undo.truncate(depth + 1);
                    r.get("at").and_then(solidcraft_engine::params::vec2).unwrap_or(q)
                }
                Err(_) => from,
            };
            // The grab stays on the geometry: what it could not follow this frame it catches up
            // with on the next one instead of lagging the cursor for the rest of the drag.
            if grab.is_some() {
                app.viewport.point_drag = Some((id, depth, Some(at)));
            }
        }
        if resp.drag_stopped() {
            crate::sketch_text_handles::drag_end(app);
            if let Some((_, depth, _)) = app.viewport.point_drag.clone() {
                // Letting go on a point, midpoint or curve adds the matching constraint, then the
                // whole model follows the drag, all in the drag's undo step.
                let _ = app.session.execute("sketch.drag_end", &json!({ "constraint": crate::drag_snap::release() }));
                app.session.undo.truncate(depth + 1);
            }
            crate::drag_snap::clear();
            app.viewport.point_drag = None;
            crate::sketch_text_move::end_drag(app);
            crate::dim_view::drag_end();
            crate::sketch3d::drag_end();
        }
    }
    // A sketch drag whose release never came (focus lost, a tool or dialog took over) ends here,
    // so the model does not stay behind its sketch until the next command. Not while CAD input
    // is held back (an issue report's capture): a release then belongs to the report, and the
    // drag continues afterwards.
    if app.viewport.point_drag.is_some() && !crate::issue_report::cad_input_blocked() && !ui.input(|i| i.pointer.primary_down()) {
        app.viewport.point_drag = None;
        crate::drag_snap::clear();
        let _ = app.session.execute("sketch.drag_end", &json!({}));
        crate::sketch_text_move::end_drag(app);
    }
    if crate::sketch_text_handles::dragging(app) && !crate::issue_report::cad_input_blocked() && !ui.input(|i| i.pointer.primary_down()) {
        crate::sketch_text_handles::drag_end(app);
    }
    // Dragging a part that joints place moves it as far as they allow (outside sketches).
    if app.tool.is_none() && app.dialog.is_none() && primary_nav.is_none() && mode.is_none() && app.session.active_sketch.is_none() && !held {
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(a) = ui.input(|i| i.pointer.press_origin())
            && rect.contains(a)
            && !axes.contains(a)
        {
            crate::joint_drag::start(app, &proj, a);
        }
        if crate::joint_drag::active(app)
            && resp.dragged_by(egui::PointerButton::Primary)
            && let Some(p) = hover
        {
            crate::joint_drag::to(app, &proj, p);
        }
    }
    if crate::joint_drag::active(app) && (resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down())) {
        crate::joint_drag::end(app);
    }
    // Box selection: a primary drag on the model when no navigation mode is on.
    if app.tool.is_none()
        && !held
        && primary_nav.is_none()
        && mode.is_none()
        && app.viewport.point_drag.is_none()
        && !crate::joint_drag::active(app)
        && !crate::dim_view::dragging()
        && !crate::sketch3d::dragging()
        && !crate::sketch_text_handles::dragging(app)
    {
        let origin = ui.input(|i| i.pointer.press_origin());
        if resp.dragged_by(egui::PointerButton::Primary)
            && let (Some(a), Some(b)) = (origin, hover)
            && rect.contains(a)
            && !axes.contains(a)
        {
            app.viewport.boxsel = Some(BoxSel::from_drag(a, b));
        }
        if resp.drag_stopped()
            && let Some(bx) = app.viewport.boxsel.take()
        {
            box_select(app, &proj, bx, add);
        }
    } else {
        app.viewport.boxsel = None;
    }
    crate::preview::paint_sketches(app, &painter, &proj);
    crate::ref_images::show(app, ui, &painter, &proj);
    crate::dialogs_assembly::origins(app, &painter, &proj);
    crate::dialogs_assembly::joint_glyphs(app, &painter, &proj);
    overlays(app, &painter, &proj);
    crate::dim_view::show(app, ui, &painter, &proj);
    crate::sketch_text_handles::show(app, ui, &painter, &proj, hover);
    crate::sketch3d::show(app, &painter, &proj);
    crate::inference::show(app, &painter, &proj);
    crate::drag_snap::show(app, &painter, &proj);
    hover_highlight(app, &painter, &proj);
    points_2d(app, &painter, &proj);
    if let Some((plane, sketch)) = crate::sketch_attachment_ui::lost_guide(app)
        && let Some((low, high)) = sketch.bounds()
        && let Some(at) = proj.to_screen(plane.to_world(solidcraft_engine::geom::Vec2::new((low.x + high.x) * 0.5, high.y)))
    {
        painter.text(at - vec2(0.0, 12.0), Align2::CENTER_BOTTOM, "Saved sketch plane — attachment lost", FontId::proportional(12.0), t.warning);
    }
    if let Some(bx) = app.viewport.boxsel {
        let col = if bx.crossing { t.box_crossing } else { t.box_window };
        painter.rect_filled(bx.rect, 0.0, col.gamma_multiply(0.12));
        if bx.crossing {
            let c = [bx.rect.left_top(), bx.rect.right_top(), bx.rect.right_bottom(), bx.rect.left_bottom()];
            for i in 0..4 {
                painter.add(Shape::dashed_line(&[c[i], c[(i + 1) % 4]], Stroke::new(1.2, col), 6.0, 4.0));
            }
        } else {
            painter.rect_stroke(bx.rect, 0.0, Stroke::new(1.2, col), egui::StrokeKind::Inside);
        }
    }
    if let Some(tl) = app.tool.as_ref() {
        crate::tools::preview(app, tl, &painter, &proj);
    }
    crate::canvas::show(app, ui, &painter, &proj);
    crate::sketch_tools::show(app, ui, &painter, &proj);
    crate::sketch_dims::show(app, ui, &proj);
    crate::select_other::show(app, ui.ctx());
    axis_view(app, ui, rect);
    nav_bar(app, ui, rect);
    // A command recomputing the history off the window thread (the marker moved, undo): the
    // window goes on, and says so above the status chip.
    if let Some(since) = app.computing_since() {
        let seconds = ((crate::now_ms() - since) / 1000.0).floor().max(0.0);
        let text = crate::i18n::format("dialog.computing", &[("seconds", &format!("{seconds:.0}"))]);
        let pos = pos2(rect.left() + 12.0, rect.bottom() - 76.0);
        let galley = painter.layout_no_wrap(text, FontId::proportional(12.5), t.text);
        let r = Rect::from_min_size(pos, galley.size() + vec2(34.0, 8.0));
        painter.rect_filled(r, 4.0, t.overlay);
        egui::Spinner::new().size(12.0).paint_at(ui, Rect::from_center_size(pos2(r.left() + 13.0, r.center().y), vec2(12.0, 12.0)));
        painter.galley(pos + vec2(26.0, 4.0), galley, t.text);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    // Status chip.
    if let Some((msg, at, err)) = app.status.clone() {
        if crate::now_ms() - at < 6000.0 {
            let pos = pos2(rect.left() + 12.0, rect.bottom() - 46.0);
            let galley = painter.layout_no_wrap(msg, FontId::proportional(12.5), if err { t.error } else { t.text });
            let r = Rect::from_min_size(pos, galley.size() + vec2(16.0, 8.0));
            painter.rect_filled(r, 4.0, t.overlay);
            painter.galley(pos + vec2(8.0, 4.0), galley, t.text);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        } else {
            app.status = None;
        }
    }
}

/// A click without a command: select the candidate (replacing the selection, or toggling it
/// in with Shift/Ctrl); a click on nothing clears.
pub(crate) fn select(app: &mut SolveApp, sel: Option<Sel>, add: bool) {
    if !add {
        // A joint picked in the browser (highlighted even when hidden) lets go.
        app.tree.picked_joints.clear();
    }
    match sel {
        Some(x) => {
            let items = picked_selection(&app.session, &x);
            if add && items.iter().all(|x| app.session.selection.iter().any(|y| crate::selection::same_item(x, y))) {
                let rest: Vec<Sel> =
                    app.session.selection.iter().filter(|y| !items.iter().any(|x| crate::selection::same_item(x, y))).cloned().collect();
                let _ = app.run("select.set", json!({ "items": rest }));
            } else {
                let _ = app.run("select.set", json!({"items": items, "add": add}));
            }
        }
        None if !add => {
            let _ = app.run("select.clear", json!({}));
        }
        None => {}
    }
}

/// What a click on `sel` picks: its pieces ([`grouped_selection`]), and for a letter of a sketch
/// text the whole text, unless Shift is held (Shift picks the one letter).
pub(crate) fn picked_selection(s: &solidcraft_engine::Session, sel: &Sel) -> Vec<Sel> {
    if let Sel::Profile { sketch, index } = sel
        && !crate::sketch_text::one_letter()
        && let Some(letters) = crate::sketch_text::text_profiles(s, *sketch, *index)
    {
        return letters.into_iter().map(|index| Sel::Profile { sketch: *sketch, index }).collect();
    }
    grouped_selection(s, sel)
}

/// Expand a visible rim or surface into the kernel pieces a command needs. The original
/// face pick keeps its point (used by tools such as Hole and construction geometry).
pub(crate) fn grouped_selection(s: &solidcraft_engine::Session, sel: &Sel) -> Vec<Sel> {
    let st = s.world_state();
    match sel {
        Sel::Edge { body, index, .. } => {
            if let Some(b) = st.body(body) {
                let m = b.mesh();
                return m
                    .circular_rim(*index)
                    .into_iter()
                    .filter_map(|i| m.edges.get(i).map(|e| Sel::Edge { body: body.clone(), index: i, point: polyline_mid(e) }))
                    .collect();
            }
        }
        Sel::Face { body, index, .. } => {
            if let Some(b) = st.body(body) {
                let m = b.mesh();
                let mut patch = m.surface_patch(*index);
                patch.sort_by_key(|i| usize::from(i != index));
                return patch
                    .into_iter()
                    .filter_map(|i| {
                        if i == *index {
                            return Some(sel.clone());
                        }
                        m.triangles.iter().zip(&m.tri_face).find_map(|(t, f)| {
                            if *f as usize != i {
                                return None;
                            }
                            let [a, b, c] = m.tri(t)?;
                            Some(Sel::Face { body: body.clone(), index: i, point: (a + b + c) / 3.0 })
                        })
                    })
                    .collect();
            }
        }
        _ => {}
    }
    vec![sel.clone()]
}

/// Measure keeps one entry per visible item, rather than using its two slots for kernel halves.
pub(crate) fn measurement_selection(s: &solidcraft_engine::Session, sel: &Sel) -> Sel {
    let pieces = grouped_selection(s, sel);
    let mut representative = sel.clone();
    match &mut representative {
        Sel::Edge { index, .. } => {
            if let Some(Sel::Edge { index: i, .. }) = pieces.first() {
                *index = *i;
            }
        }
        Sel::Face { index, .. } => {
            if let Some(i) = pieces
                .iter()
                .filter_map(|p| match p {
                    Sel::Face { index, .. } => Some(*index),
                    _ => None,
                })
                .min()
            {
                *index = i;
            }
        }
        _ => {}
    }
    representative
}

/// What a box selects: sketch entities while sketching, otherwise what the open dialog's
/// active input takes (edges, faces or bodies), otherwise bodies.
fn box_select(app: &mut SolveApp, proj: &Proj, bx: BoxSel, add: bool) {
    use crate::selection::{BODIES, EDGES, FACES, PROFILES};
    // While OK computes, the dialog's inputs hold.
    if app.applying.is_some() {
        return;
    }
    let st = app.session.world_state();
    let mut out: Vec<Sel> = Vec::new();
    let to2 = |pts: &[Vec3]| -> Vec<Pos2> { pts.iter().filter_map(|p| proj.to_screen(*p)).collect() };
    let sketch_dialog = app.dialog.as_ref().is_some_and(|d| matches!(d.kind, crate::dialogs::Kind::SketchModify(_)));
    let accepts = app.dialog.as_ref().and_then(|d| d.active_input()).map(|i| i.accept);
    if (app.dialog.is_none() || sketch_dialog)
        && let Some(ss) = app.session.active_sketch.and_then(|sid| st.sketch(sid))
    {
        // What the selection filter lets through (window: wholly inside; crossing: touching).
        let off = |k: &str| app.ui.pick_off.iter().any(|x| x == k);
        for (i, c) in ss.sketch.curves.iter().enumerate().filter(|_| !off("sketch") && !off("sketch_curves")) {
            if accepts
                .is_some_and(|a| a & crate::selection::CURVES == 0 && (a & crate::selection::AXES == 0 || !matches!(c.kind, CurveKind::Line { .. })))
            {
                continue;
            }
            if !solidcraft_engine::view::sketch_curve_visible(&ss.sketch, i) {
                continue;
            }
            let sampled = if bx.crossing {
                crate::sketch_display::curve_sample(app, ss.feature, i, &ss.plane, proj)
            } else {
                std::sync::Arc::new(crate::sketch_display::curve_unclipped(&ss.sketch, i, &ss.plane, proj, app.viewport.pixels_per_point()))
            };
            let parts: Vec<Vec<Pos2>> = sampled.parts.iter().map(|part| to2(part)).collect();
            let takes = if bx.crossing {
                parts.iter().any(|part| bx.takes(part))
            } else {
                sampled.quality == solidcraft_engine::render::SamplingQuality::Complete
                    && !parts.is_empty()
                    && parts.iter().all(|part| bx.takes(part))
            };
            if takes {
                out.push(Sel::SketchCurve { id: c.id.clone() });
            }
        }
        let visible = solidcraft_engine::view::SketchPointVisibility::new(&ss.sketch);
        for (i, p) in ss.sketch.points.iter().enumerate().filter(|_| !off("sketch") && !off("sketch_points")) {
            if accepts.is_none_or(|a| a & crate::selection::SKETCH_POINTS != 0)
                && i != 0
                && visible.takes(p)
                && bx.takes(&to2(&[ss.plane.to_world(p.pos)]))
            {
                out.push(Sel::SketchPoint { id: p.id.clone() });
            }
        }
        if !sketch_dialog && !off("dimensions") {
            for (id, r) in crate::dim_view::text_rects() {
                if bx.takes(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]) {
                    out.push(Sel::SketchConstraint { id });
                }
            }
        }
        if !sketch_dialog && !off("constraints") {
            for (id, c) in crate::sketch_tools::glyph_positions() {
                if bx.takes(&[c]) {
                    out.push(Sel::SketchConstraint { id });
                }
            }
        }
    } else {
        let accept = app.dialog.as_ref().and_then(|d| d.active_input()).map(|i| i.accept).unwrap_or(BODIES);
        for b in &st.bodies {
            if !app.session.body_visible(&b.name) {
                continue;
            }
            let m = b.mesh();
            if accept & EDGES != 0 {
                for (ei, e) in m.edges.iter().enumerate() {
                    let rim = m.circular_rim(ei);
                    let takes = if bx.crossing { bx.takes(&to2(e)) } else { rim.iter().all(|&i| m.edges.get(i).is_some_and(|p| bx.takes(&to2(p)))) };
                    if m.edge_pickable(ei) && takes {
                        out.push(Sel::Edge { body: b.name.clone(), index: ei, point: polyline_mid(e) });
                    }
                }
            } else if accept & FACES != 0 {
                for f in 0..b.body.face_count() {
                    let mut pts: Vec<Vec3> = Vec::new();
                    for e in m.face_edges(u32::try_from(f).unwrap_or(u32::MAX)) {
                        pts.extend(m.edges.get(e).into_iter().flatten().copied());
                    }
                    let point = m
                        .triangles
                        .iter()
                        .zip(&m.tri_face)
                        .find(|(_, tf)| **tf as usize == f)
                        .and_then(|(t, _)| m.tri(t))
                        .map(|[a, bb, c]| (a + bb + c) / 3.0);
                    let takes = bx.takes(&to2(&pts))
                        && (bx.crossing
                            || m.surface_patch(f).iter().all(|&face| {
                                m.face_edges(u32::try_from(face).unwrap_or(u32::MAX))
                                    .iter()
                                    .all(|&e| m.edges.get(e).is_some_and(|p| bx.takes(&to2(p))))
                            }));
                    if let Some(point) = point
                        && takes
                    {
                        out.push(Sel::Face { body: b.name.clone(), index: f, point });
                    }
                }
            } else if accept & BODIES != 0 {
                let crossing_hit = m.edges.iter().any(|e| bx.takes(&to2(e)));
                let all_in = m.edges.iter().all(|e| bx.takes(&to2(e)));
                if (bx.crossing && crossing_hit) || (!bx.crossing && all_in && !m.edges.is_empty()) {
                    out.push(Sel::Body { name: b.name.clone() });
                }
            } else if accept & PROFILES != 0 {
                for sid in visible_sketches(app) {
                    let Some(ss) = st.sketch(sid) else { continue };
                    for (pi, p) in ss.profiles.iter().enumerate() {
                        let parts = crate::sketch_display::boundary_parts(&p.region.outer, &ss.plane, proj, app.viewport.pixels_per_point());
                        let takes = if bx.crossing {
                            parts.iter().any(|part| bx.takes(&to2(part)))
                        } else {
                            crate::sketch_display::profile_region(app, sid, pi, &ss.plane, proj).as_ref().as_ref().is_some_and(|display| {
                                let points: Vec<_> = display.outer.segs.iter().map(|segment| ss.plane.to_world(segment.start())).collect();
                                bx.takes(&to2(&points))
                            })
                        };
                        if takes {
                            out.push(Sel::Profile { sketch: sid, index: pi });
                        }
                    }
                }
                break;
            }
        }
    }
    let mut grouped = Vec::new();
    for item in out {
        for piece in grouped_selection(&app.session, &item) {
            if !grouped.iter().any(|x| crate::selection::same_item(x, &piece)) {
                grouped.push(piece);
            }
        }
    }
    let out = grouped;
    if let Some(mut d) = app.dialog.take() {
        d.take_box(&app.session, out, add);
        app.dialog = Some(d);
        return;
    }
    let items = serde_json::to_value(&out).unwrap_or_default();
    let _ = app.run("select.set", json!({"items": items, "add": add}));
}

/// Screen-space dots: the origin point, and hovered or selected body vertices.
fn points_2d(app: &SolveApp, painter: &egui::Painter, proj: &Proj) {
    let t = Tokens::get();
    if app.origin_visible()
        && !app.session.visibility.hidden_origin.iter().any(|h| h == "O")
        && let Some(o) = proj.to_screen(Vec3::ZERO)
    {
        painter.circle(o, 4.5, t.origin_point, Stroke::new(1.0, Color32::from_gray(110)));
    }
    for x in app.highlighted() {
        if let Sel::Vertex { point, .. } = x
            && let Some(p) = proj.to_screen(point)
        {
            painter.circle(p, 4.0, t.sel_vertex, Stroke::new(1.5, t.sel_edge));
        }
    }
    if let Some(Hit::Vertex { point, .. }) = &app.viewport.hover
        && let Some(p) = proj.to_screen(*point)
    {
        painter.circle(p, 4.5, t.hover_edge_halo, Stroke::new(1.5, t.hover_edge));
    }
}

/// Hover feedback for the active sketch's points and curves (drawn on top of everything).
fn hover_highlight(app: &SolveApp, painter: &egui::Painter, proj: &Proj) {
    let col = Tokens::get().hover_profile_edge;
    let st = app.session.world_state();
    match &app.viewport.hover {
        Some(Hit::SketchCurve { sketch, id, .. }) => {
            if let Some(ss) = st.sketch(*sketch)
                && let Some(ci) = ss.sketch.curve_index(id)
            {
                for part in &crate::sketch_display::curve_sample(app, *sketch, ci, &ss.plane, proj).parts {
                    let mode = ss.sketch.curves.get(ci).map_or(0, |c| if c.centerline { 2 } else { u8::from(c.construction) });
                    crate::tools::paint_sketch_path(part, mode, painter, proj, Stroke::new(3.0, col));
                }
            }
        }
        Some(Hit::SketchPoint { sketch, at, .. }) => {
            if let Some(ss) = st.sketch(*sketch)
                && let Some(p) = proj.to_screen(ss.plane.to_world(*at))
            {
                painter.circle_stroke(p, 6.0, Stroke::new(2.0, col));
            }
        }
        _ => {}
    }
}

/// Sketch points and the status of the active sketch (constraint glyphs: `sketch_tools`).
fn overlays(app: &SolveApp, painter: &egui::Painter, proj: &Proj) {
    let t = Tokens::get();
    let st = app.session.world_state();
    for item in &app.session.selection {
        if let Sel::ExternalSketchPoint { sketch, id } = item
            && let Some(ss) = st.sketch(*sketch)
            && let Some(point) = ss.sketch.resolve_point(id).and_then(|i| ss.sketch.point(i))
            && let Some(pos) = proj.to_screen(ss.plane.to_world(point))
        {
            painter.circle_stroke(pos, 6.0, Stroke::new(2.0, t.sel_edge));
        }
    }
    let Some(sid) = app.session.active_sketch else { return };
    let Some(ss) = st.sketch(sid) else { return };
    let sk = &ss.sketch;
    let locked = sk.locked_points();
    let virtual_corners: Vec<usize> = sk.virtual_corners().into_iter().map(|(i, _)| i).collect();
    let visible = solidcraft_engine::view::SketchPointVisibility::new(sk);
    for (i, p) in sk.points.iter().enumerate() {
        if !visible.takes(p) {
            continue;
        }
        let hidden = (sk.view.hide_points && i != 0) || (sk.view.hide_projected && p.link.is_some());
        let Some(sp) = proj.to_screen(ss.plane.to_world(p.pos)).filter(|_| !hidden) else { continue };
        let det = ss.report.point_determined.get(i).copied().unwrap_or(false);
        let c = if p.link.is_none() && locked.get(i).copied().unwrap_or(false) {
            t.sketch_locked
        } else if det {
            t.sketch_point
        } else {
            t.sketch_point_free
        };
        if i == 0 {
            painter.circle(sp, 3.5, Color32::from_rgb(240, 200, 60), Stroke::new(1.0, t.body_edge));
        } else if virtual_corners.binary_search(&i).is_ok() {
            // A virtual sharp corner: an open square, it is not on any drawn curve.
            painter.rect_stroke(Rect::from_center_size(sp, vec2(6.0, 6.0)), 0.0, Stroke::new(1.2, c), egui::StrokeKind::Middle);
        } else {
            painter.rect_filled(Rect::from_center_size(sp, vec2(5.0, 5.0)), 0.0, c);
        }
    }
    loose_end_markers(sk, &ss.ends, &ss.plane, painter, proj, &t);
    // Sketch status.
    if let Some(r) = app.viewport.rect {
        let msg = if !ss.report.ok() {
            format!("{} — constraints conflict", ss.name)
        } else if ss.report.dof == 0 {
            format!("{} — fully constrained", ss.name)
        } else {
            format!("{} — {} degrees of freedom", ss.name, ss.report.dof)
        };
        let c = if ss.report.ok() { t.sketch_accent } else { t.error };
        painter.text(pos2(r.left() + 12.0, r.top() + 12.0), Align2::LEFT_TOP, msg, FontId::proportional(13.0), c);
    }
}

/// Where a profile can fail: a ring round each open end (it touches no other curve) and a
/// diamond round each untied one (it touches another curve only by position), told apart by
/// shape. Both are warnings, not errors: an open sketch (a sweep path, an axis) is valid.
fn loose_end_markers(
    sk: &solidcraft_engine::sketch::Sketch,
    ends: &solidcraft_engine::sketch::EndReport,
    plane: &solidcraft_engine::geom::Plane,
    painter: &egui::Painter,
    proj: &Proj,
    t: &Tokens,
) {
    let screen = |i: usize| sk.point(i).and_then(|p| proj.to_screen(plane.to_world(p)));
    for sp in ends.open.iter().filter_map(|&i| screen(i)) {
        painter.circle_stroke(sp, 7.0, Stroke::new(1.8, t.warning));
    }
    for sp in ends.untied.iter().filter_map(|u| screen(u.point)) {
        let r = 7.5;
        let diamond = vec![pos2(sp.x, sp.y - r), pos2(sp.x + r, sp.y), pos2(sp.x, sp.y + r), pos2(sp.x - r, sp.y)];
        painter.add(egui::Shape::closed_line(diamond, Stroke::new(1.8, t.warning)));
    }
}

/// The colours of a headless picture of the model (`ui.render`, the agent's view image) in the
/// dark or the light theme and a colour vision mode, from the tokens the window draws with, so the
/// picture matches the window. The light theme's tokens (Off) are the engine's own colours, so a light
/// render is unchanged.
pub(crate) fn scene_palette(dark: bool, mode: crate::theme::ColorVision) -> solidcraft_engine::view::Palette {
    let t = Tokens::themed(dark, mode);
    let rgb = |c: Color32| Rgb(c.r(), c.g(), c.b());
    solidcraft_engine::view::Palette {
        background: (rgb(t.viewport_top), rgb(t.viewport_bottom)),
        grid: rgb(t.grid),
        grid_major: rgb(t.grid_major),
        edge: rgb(t.body_edge),
        body: rgb(t.body),
        body_selected: rgb(t.body_selected),
        sketch: rgb(t.sketch),
        sketch_fixed: rgb(t.sketch_fixed),
        sketch_done: rgb(t.sketch_fixed),
        sketch_locked: rgb(t.sketch_locked),
        sketch_projected: rgb(t.sketch_projected),
        sketch_lost: rgb(t.sketch_lost),
        sketch_construction: rgb(t.sketch_construction),
        sketch_centerline: rgb(t.sketch_centerline),
        axes: t.triad.map(rgb),
        cap: rgb(t.section_cap),
        analysis: t.analysis,
    }
}

pub(crate) fn refresh_layout(app: &mut SolveApp, scale: f32) {
    if let Some(rect) = app.viewport.rect {
        let docks: Vec<_> =
            [app.viewport.palette_rect.filter(|_| app.session.active_sketch.is_some()), app.viewport.dialog_rect.filter(|_| app.dialog.is_some())]
                .into_iter()
                .flatten()
                .collect();
        app.viewport.layout = Some(crate::viewport_layout::Layout::new(rect, &docks, scale, app.preferences.navigator_style));
    }
}

/// Orientation controls share the layout reservation used by model fitting.
fn axis_center(app: &SolveApp, rect: Rect) -> Pos2 {
    axis_rect(app, rect).center()
}

pub(crate) fn axis_rect(app: &SolveApp, rect: Rect) -> Rect {
    app.viewport.layout.as_ref().filter(|l| l.viewport == rect).map(|l| l.axis_view).unwrap_or_else(|| {
        let size = axis_view_size(app.preferences.navigator_style);
        Rect::from_min_size(pos2(rect.right() - size, rect.top()), vec2(size, size))
    })
}

const AXIS_LENGTH: f64 = 38.0;
use crate::icons::navigation::AXIS_RADIUS;

/// Six signed world axes, sorted back to front so overlapping tips have one owner.
fn axis_tips(cam: &Camera, c: Pos2) -> Vec<(&'static str, Vec3, Pos2, f64)> {
    let (r, u, b) = cam.basis();
    let mut tips: Vec<_> = [("x", Vec3::X), ("-x", -Vec3::X), ("y", Vec3::Y), ("-y", -Vec3::Y), ("z", Vec3::Z), ("-z", -Vec3::Z)]
        .into_iter()
        .map(|(name, d)| (name, d, c + vec2((d.dot(r) * AXIS_LENGTH) as f32, -(d.dot(u) * AXIS_LENGTH) as f32), d.dot(b)))
        .collect();
    tips.sort_by(|a, b| a.3.total_cmp(&b.3));
    tips
}

fn axis_hit(tips: &[(&str, Vec3, Pos2, f64)], p: Pos2) -> Option<usize> {
    tips.iter().enumerate().rev().find(|(_, (_, _, at, _))| at.distance(p) <= AXIS_RADIUS).map(|(i, _)| i)
}

/// Dragging anywhere on the axis gizmo orbits about the view centre. A stationary press
/// on a tip chooses its view. Floating windows own their input over the gizmo.
fn axis_view_orbit(app: &mut SolveApp, ui: &mut egui::Ui, rect: Rect) -> bool {
    let (pressed, down, decided, origin, pos, shift) = ui.input(|i| {
        let p = &i.pointer;
        (p.primary_pressed(), p.primary_down(), p.is_decidedly_dragging(), p.press_origin(), p.interact_pos(), i.modifiers.shift)
    });
    if pressed && ui.rect_contains_pointer(rect) {
        app.viewport.axis_drag = origin.filter(|p| match app.preferences.navigator_style {
            crate::prefs::NavigatorStyle::Axes => axis_rect(app, rect).contains(*p),
            crate::prefs::NavigatorStyle::Cube => {
                navigation_cube::drag_origin(&app.cam, axis_center(app, rect), *p, axis_rect(app, rect).width() / navigation_cube::DESIGN_SIZE)
            }
        });
        if app.viewport.axis_drag.is_some() {
            app.viewport.orbit_free = None;
        }
    }
    if !down && app.viewport.axis_drag.take().is_some() {
        app.viewport.orbit_free = None;
    }
    let Some(last) = app.viewport.axis_drag.filter(|_| decided) else { return false };
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    if let Some(p) = pos {
        app.cancel_view_animation();
        orbit_drag(app, rect, f64::from(p.x - last.x), f64::from(p.y - last.y), None, shift, false);
        app.viewport.axis_drag = Some(p);
        ui.ctx().request_repaint();
    }
    true
}

pub(crate) fn axis_view(app: &mut SolveApp, ui: &mut egui::Ui, rect: Rect) {
    match app.preferences.navigator_style {
        crate::prefs::NavigatorStyle::Axes => signed_axis_view(app, ui, rect),
        crate::prefs::NavigatorStyle::Cube => navigation_cube::show(app, ui, rect),
    }
}

fn signed_axis_view(app: &mut SolveApp, ui: &mut egui::Ui, rect: Rect) {
    if !axis_rect(app, rect).is_positive() {
        app.viewport.axis_drag = None;
        crate::scenario::publish_count("axis_view_highlights", 0.0);
        return;
    }
    crate::scenario::publish_rect("axis_view:outer", axis_rect(app, rect));
    let t = Tokens::get();
    let c = axis_center(app, rect);
    let orbiting = axis_view_orbit(app, ui, rect);
    let tips = axis_tips(&app.cam, c);
    let hover = ui.input(|i| i.pointer.hover_pos()).filter(|_| ui.rect_contains_pointer(rect));
    let hovered = if orbiting { None } else { hover.and_then(|p| axis_hit(&tips, p)) };
    let painter = ui.painter_at(rect);
    if orbiting || hover.is_some_and(|p| axis_rect(app, rect).contains(p)) {
        painter.circle_filled(c, 55.0, t.overlay);
    }
    for (i, (name, d, at, depth)) in tips.iter().enumerate() {
        crate::icons::navigation::axis_tip(&painter, c, name, d, *at, *depth, hovered == Some(i), t);
        crate::scenario::publish_handle(&format!("axis_view:{name}"), *at);
    }
    crate::scenario::publish_count("axis_view_highlights", f64::from(u8::from(hovered.is_some())));
    if let Some(i) = hovered
        && let Some((name, d, _, _)) = tips.get(i)
    {
        crate::scenario::publish_handle(&format!("axis_view_hover:{name}"), hover.unwrap_or(c));
        ui.interact(Rect::from_center_size(hover.unwrap_or(c), vec2(2.0, 2.0)), ui.id().with("axis_view_tip"), Sense::hover())
            .on_hover_text(format!("View from {}. Click again to reverse. Drag to orbit; hold Shift to snap.", name.to_uppercase()));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if ui.input(|i| i.pointer.primary_clicked()) {
            let frame = app.cam.to_frame(*d);
            let dir = if app.cam.back().dot(*d) > 0.9999 { -*d } else { *d };
            let mut to = app.cam.looking_from(dir);
            if frame.z.abs() > 0.5 {
                to.yaw = nearest_quarter_turn(app.cam.yaw);
            }
            if app.preferences.look_at_anim {
                app.cancel_view_animation();
                app.viewport.orbit_free = None;
                app.viewport.orbit_snap = Some(OrbitSnap { target: to, yaw_velocity: 0.0, pitch_velocity: 0.0, pivot: None });
                ui.ctx().request_repaint();
            } else {
                app.animate_to(to);
            }
        }
    }
    let home = Rect::from_center_size(pos2(c.x - 44.0, c.y - 44.0), vec2(18.0, 18.0));
    crate::scenario::publish_rect("axis_view:home", home);
    crate::scenario::publish_handle("axis_view:home", home.center());
    let hr = ui.interact(home, ui.id().with("axis_view_home"), Sense::click());
    icons::paint(&painter, home, "home", t.icon, if hr.hovered() { t.accent_soft } else { t.overlay }, t.accent);
    if hr.on_hover_text(crate::i18n::tr_label("Home view")).clicked() {
        app.animate_view("home");
    }
}

/// At a halfway tie, prefer the square heading nearer the standard plan view.
pub(crate) fn nearest_quarter_turn(yaw: f64) -> f64 {
    if !yaw.is_finite() {
        return 0.0;
    }
    let q = std::f64::consts::FRAC_PI_2;
    let lo = (yaw / q).floor() * q;
    let hi = lo + q;
    let (dlo, dhi) = (yaw - lo, hi - yaw);
    if (dlo - dhi).abs() < 1e-6 {
        let off_front = |a: f64| ((a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI).abs();
        if off_front(lo) <= off_front(hi) { lo } else { hi }
    } else if dlo < dhi {
        lo
    } else {
        hi
    }
}

/// Navigation bar (bottom centre).
/// Turn the view to face the selected planar face or plane, else the active sketch; asked again
/// while it already faces it, from the other side (#1886).
pub fn look_at_selection(app: &mut SolveApp) {
    let target = app.session.selection.iter().find_map(|s| match s {
        Sel::Face { body, index, point } => crate::dialogs::planar_face(&app.session, body, *index).map(|(_, n)| (*point, n)),
        Sel::Plane { name } => solidcraft_engine::geom::Plane::named(name).map(|p| (p.origin, p.normal())),
        _ => None,
    });
    match target {
        Some((p, n)) => {
            let mut to = app.cam.looking_from(crate::dialogs::facing(app, n, true));
            to.target = p;
            app.animate_to(to);
        }
        None => crate::dialogs::look_at_sketch_again(app),
    }
}

fn nav_bar(app: &mut SolveApp, ui: &mut egui::Ui, rect: Rect) {
    if app.viewport.layout.as_ref().is_some_and(|l| !l.navigation.is_positive()) {
        return;
    }
    let t = Tokens::get();
    let items: [(&str, &str); 10] = [
        (
            "orbit",
            if app.preferences.nav == crate::prefs::TOUCHPAD_NAV {
                crate::i18n::tr("nav.touchpad.orbit")
            } else if app.preferences.nav == crate::prefs::OPENSCAD_NAV {
                "Orbit (left drag on empty space; Shift+left drag box-selects)"
            } else {
                "Orbit (right drag; hold Shift to snap)"
            },
        ),
        ("spin", "Continuous orbit (stops at the next click or key)"),
        ("pan", if app.preferences.nav == crate::prefs::TOUCHPAD_NAV { crate::i18n::tr("nav.touchpad.pan") } else { "Pan (drag; or middle drag)" }),
        ("zoom", if app.preferences.nav == crate::prefs::TOUCHPAD_NAV { crate::i18n::tr("nav.touchpad.zoom") } else { "Zoom (drag; or wheel)" }),
        ("fit", "Fit (F6)"),
        ("lookat", "Look At the selected face or plane (or the active sketch)"),
        ("home", "Home view"),
        ("perspective", "Perspective / orthographic"),
        ("style", "Visual style: shaded with edges, shaded, wireframe, shaded with hidden edges"),
        ("settings", "View settings: grid, ground shadow, agent cursor"),
    ];
    let menu_id = ui.id().with("nav_view_menu");
    let mut menu_open = ui.data(|d| d.get_temp::<bool>(menu_id)).unwrap_or(false);
    let w = items.len() as f32 * 30.0 + 10.0;
    let bar = app
        .viewport
        .layout
        .as_ref()
        .map(|l| l.navigation)
        .unwrap_or_else(|| Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 22.0), vec2(w, 30.0)));
    crate::scenario::publish_rect("nav:outer", bar);
    let active = ui.rect_contains_pointer(bar)
        || ui.rect_contains_pointer(axis_rect(app, rect))
        || app.viewport.axis_drag.is_some()
        || app.viewport.nav.is_some()
        || app.viewport.modifier_nav.is_some()
        || app.viewport.spin
        || menu_open;
    let mut painter = ui.painter_at(rect);
    painter.set_opacity(if active || app.preferences.navigator_style == crate::prefs::NavigatorStyle::Axes { 1.0 } else { 0.45 });
    painter.rect(bar, 6.0, t.overlay, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let columns = ((bar.width() - 10.0) / 30.0).round().clamp(1.0, 10.0) as usize;
    for (i, (icon, tip)) in items.iter().enumerate() {
        let br = Rect::from_min_size(
            pos2(bar.left() + 5.0 + (i % columns) as f32 * 30.0, bar.top() + 2.0 + (i / columns) as f32 * 30.0),
            vec2(26.0, 26.0),
        );
        let resp = ui.interact(br, ui.id().with(("nav", *icon)), Sense::click());
        let active = match *icon {
            "orbit" => app.viewport.nav == Some(NavMode::Orbit),
            "pan" => app.viewport.nav == Some(NavMode::Pan),
            "zoom" => app.viewport.nav == Some(NavMode::Zoom),
            "perspective" => app.ui.perspective,
            "settings" => menu_open,
            "spin" => app.viewport.spin,
            _ => false,
        };
        if active || resp.hovered() {
            painter.rect_filled(br, 4.0, if active { t.accent_soft } else { t.hover });
        }
        icons::paint(&painter, br.shrink(4.0), icon, t.icon, t.icon_fill, t.accent);
        if resp.on_hover_text(*tip).clicked() {
            let toggle = |m: NavMode, cur: Option<NavMode>| if cur == Some(m) { None } else { Some(m) };
            match *icon {
                "orbit" => app.viewport.nav = toggle(NavMode::Orbit, app.viewport.nav),
                "pan" => app.viewport.nav = toggle(NavMode::Pan, app.viewport.nav),
                "zoom" => app.viewport.nav = toggle(NavMode::Zoom, app.viewport.nav),
                "fit" => app.animate_view("fit"),
                "spin" => {
                    app.cancel_view_animation();
                    app.viewport.spin = !app.viewport.spin;
                }
                "home" => app.animate_view("home"),
                "lookat" => look_at_selection(app),
                "style" => app.ui.visual_style = (app.ui.visual_style + 1) % 4,
                "perspective" => app.ui.perspective = !app.ui.perspective,
                _ => menu_open = !menu_open,
            }
        }
        if *icon == "settings" {
            crate::scenario::publish_handle("nav:view", br.center());
            if menu_open {
                menu_open = view_menu(app, ui, br);
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(menu_id, menu_open));
}

/// The View settings above the navigation bar's last button: grid, ground shadow and the agent
/// cursor (#35). False once it should close (a click outside it).
fn view_menu(app: &mut SolveApp, ui: &mut egui::Ui, button: Rect) -> bool {
    let area = crate::frame::area(egui::Id::new("sc_view_menu"))
        .order(egui::Order::Foreground)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(button.right_top() - vec2(0.0, 6.0));
    let r = area.show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(220.0);
            ui.label(egui::RichText::new(crate::i18n::tr_label("VIEW")).strong().size(11.5));
            ui.checkbox(&mut app.ui.show_grid, crate::i18n::tr_label("Grid"));
            ui.checkbox(&mut app.ui.ground_shadow, crate::i18n::tr_label("Ground shadow"));
            ui.separator();
            let s = &mut app.ui.agent_cursor;
            let show = ui.checkbox(&mut s.show, crate::i18n::tr_label("Show Agent Cursor"));
            crate::scenario::publish_handle("view:agent_cursor", show.rect.center());
            ui.add_enabled_ui(s.show, |ui| {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::tr_label("Speed"));
                    crate::agent_cursor::speed_slider(ui, &mut s.speed);
                });
                ui.checkbox(&mut s.follow_camera, crate::i18n::tr_label("Follow camera (Look At sketches)"));
            });
            // The agent chat panel, once the chat is turned on in Preferences.
            if crate::agent_chat::shown(app) {
                ui.separator();
                let mut open = app.agent_chat.open;
                let r = ui.checkbox(&mut open, crate::agent_chat::panel_tip());
                crate::scenario::publish_handle("view:agent_chat", r.rect.center());
                if r.changed() {
                    app.agent_chat.open = open;
                    app.preferences.agent_chat_minimized &= !open;
                }
            }
        });
    });
    let clicked_out = ui.input(|i| i.pointer.any_pressed())
        && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| !r.response.rect.contains(p) && !button.contains(p));
    !clicked_out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navigation::*;

    /// Every tangent seam Continuity Analysis reports keeps its colour while the pointer rests on
    /// a face (#1247): a 60 x 40 x 20 box with its four top edges rounded (6 mm) has eight, four
    /// against the sides and four against the top face. The tool lit the face under the pointer,
    /// though its click takes only edges, and the face's outline covered the colours of its
    /// edges. Rendered offscreen with bodies built on the CPU and evaluated on the GPU (skipped
    /// without a GPU adapter); with `SOLIDCRAFT_CONTINUITY_TOP_SHOT` set, the pictures are
    /// written there (`-cpu.png`, `-gpu.png`).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn continuity_colours_stay_on_the_edges_of_the_face_under_the_pointer() {
        use solidcraft_engine::kernel::SeamVerdict;
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 60, "width": 40, "height": 20, "body_name": "B"})).unwrap();
        s.execute("solid.fillet", &json!({"edges": [[30, 0, 20], [30, 40, 20], [0, 20, 20], [60, 20, 20]], "radius": 6})).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        app.fit_view();
        // The toolbar button: every seam shaded, the tool left picking edges.
        app.start("inspect.continuity");
        let tangent = |app: &SolveApp| app.session.continuity.iter().filter(|seam| seam.verdict == SeamVerdict::CurvatureJump).count();
        assert_eq!(tangent(&app), 8);
        const SIZE: u32 = 512; // rows of 256-byte multiples (the copy alignment)
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(SIZE as f32, SIZE as f32));
        // The pointer rests in the middle of the top face, away from its edges: the tool takes
        // only edges, so nothing there lights up over the analysis.
        let proj = projection(&app, rect);
        let middle = proj.to_screen(Vec3::new(30.0, 20.0, 20.0)).unwrap();
        let hits = pick_cached(&mut app, &proj, middle);
        assert!(matches!(hits.first(), Some(Hit::Face { .. })), "{hits:?}");
        app.viewport.hover = tool_hover(&app, &hits);
        assert_eq!(app.viewport.hover, None);
        let t = Tokens::get();
        let g1 = t.continuity[1];
        // Each tangent seam lies in one flat face (the top, or a side under its round): that
        // face's outward normal, and the seam shows where it faces the eye (the body is convex).
        let flat_normal = |e: &[Vec3]| {
            let on = |f: fn(&Vec3) -> f64, at: f64| e.iter().all(|p| (f(p) - at).abs() < 1e-6);
            [
                (on(|p| p.z, 20.0), Vec3::Z),
                (on(|p| p.y, 0.0), -Vec3::Y),
                (on(|p| p.y, 40.0), Vec3::Y),
                (on(|p| p.x, 0.0), -Vec3::X),
                (on(|p| p.x, 60.0), Vec3::X),
            ]
            .into_iter()
            .find_map(|(lies, n)| lies.then_some(n))
        };
        for tess in [false, true] {
            app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess });
            let proj = projection(&app, rect);
            let e = proj.cam.eye().to_f32();
            let eye = [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }];
            let Some(img) = crate::gpu::render_offscreen(
                build_scene(&app).merged(),
                Some(build_highlight(&app)),
                proj.vp.to_f32(),
                proj.cam.back().to_f32(),
                eye,
                SIZE,
                tess.then_some(solidcraft_gpu::SHADER),
                [0.0; 8],
                crate::gpu::analysis_colors(&t.analysis),
            ) else {
                eprintln!("skipping: no GPU adapter");
                return;
            };
            if let Ok(path) = std::env::var("SOLIDCRAFT_CONTINUITY_TOP_SHOT")
                && let Some(png) = image::RgbaImage::from_raw(SIZE, SIZE, img.clone())
            {
                png.save(path.replace(".png", if tess { "-gpu.png" } else { "-cpu.png" })).unwrap();
            }
            let lit = |x: i64, y: i64| {
                (0..SIZE as i64).contains(&x)
                    && (0..SIZE as i64).contains(&y)
                    && img[((y * SIZE as i64 + x) * 4) as usize..][..3].iter().zip([g1.r(), g1.g(), g1.b()]).all(|(a, b)| a.abs_diff(b) <= 12)
            };
            let mut seen = 0;
            for seam in app.session.continuity.iter().filter(|seam| seam.verdict == SeamVerdict::CurvatureJump) {
                let (a, b) = (seam.points[0], seam.points[seam.points.len() - 1]);
                let normal = flat_normal(&seam.points).unwrap_or_else(|| panic!("a tangent seam off the flat faces: {a:?}..{b:?}"));
                if normal.dot(proj.cam.eye() - a.lerp(b, 0.5)) <= 0.0 {
                    continue;
                }
                seen += 1;
                // In its colour a quarter, half and three quarters along.
                for f in [0.25, 0.5, 0.75] {
                    let p = a.lerp(b, f);
                    let q = proj.to_screen(p).unwrap();
                    let (x, y) = (q.x.round() as i64, q.y.round() as i64);
                    let drawn = (-2..=2).any(|dy| (-2..=2).any(|dx| lit(x + dx, y + dy)));
                    assert!(drawn, "tess {tess}: tangent seam {a:?}..{b:?} is not drawn in its colour at {p:?} ({x}, {y})");
                }
            }
            assert_eq!(seen, 6, "the four top seams and the two sides facing the eye");
        }
        // On an edge, the edge lights up: what a click measures.
        let proj = projection(&app, rect);
        let on_edge = proj.to_screen(Vec3::new(30.0, 6.0, 20.0)).unwrap();
        let hits = pick_cached(&mut app, &proj, on_edge);
        assert!(matches!(tool_hover(&app, &hits), Some(Hit::Edge { .. })), "{hits:?}");
    }

    /// Continuity Analysis draws each seam in its verdict's `continuity` token (#770): a box
    /// with one edge rounded shows the round's two tangent seams (G1) and the creases (G0), and
    /// nothing in the smooth or gap colours.
    #[test]
    fn continuity_seams_take_their_colours_from_the_theme() {
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 20, "width": 20, "height": 20})).unwrap();
        s.execute("solid.fillet", &json!({"edges": [[20, 20, 10]], "radius": 4})).unwrap();
        s.execute("inspect.continuity", &json!({})).unwrap();
        let app = SolveApp::new(s, crate::Services::default());
        let sc = build_scene(&app);
        // An overlay line is two points, a colour and a width: 32 bytes.
        let colours: Vec<[u8; 4]> = sc.overlay.as_chunks::<32>().0.iter().map(|r| [r[24], r[25], r[26], r[27]]).collect();
        let t = Tokens::get();
        let count = |c: Color32| colours.iter().filter(|x| **x == c4(c)).count();
        let [smooth, jump, sharp, gap] = t.continuity.map(count);
        assert!(jump > 0 && sharp > 0, "tangent seams {jump}, creases {sharp} segments");
        assert_eq!((smooth, gap), (0, 0));
    }

    /// Bodies drawn from their display plans, evaluated on the GPU, look like the same bodies
    /// built into triangles on the CPU: the sample design rendered both ways through the
    /// viewport's real pipelines, offscreen, on this machine's GPU (skipped without one).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bodies_evaluated_on_the_gpu_look_like_bodies_built_on_the_cpu() {
        let mut s = solidcraft_engine::Session::default();
        s.run_script(&solidcraft_engine::sample::script()).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        app.fit_view();
        const SIZE: u32 = 256;
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(SIZE as f32, SIZE as f32));
        let mut render = |tess: bool| {
            app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess });
            let scene = build_scene(&app).merged();
            let proj = projection(&app, rect);
            let e = proj.cam.eye().to_f32();
            let eye = [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }];
            let (planned, shader) = (scene.planned.len(), tess.then_some(solidcraft_gpu::SHADER));
            (
                planned,
                scene.tris.len(),
                crate::gpu::render_offscreen(
                    scene,
                    None,
                    proj.vp.to_f32(),
                    proj.cam.back().to_f32(),
                    eye,
                    SIZE,
                    shader,
                    [0.0; 8],
                    crate::gpu::analysis_colors(&Tokens::get().analysis),
                ),
            )
        };
        let (planned, gpu_tris, gpu) = render(true);
        let (none, cpu_tris, cpu) = render(false);
        let (Some(gpu), Some(cpu)) = (gpu, cpu) else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        assert!(planned > 0 && none == 0, "{planned} bodies planned");
        assert!(gpu_tris < cpu_tris, "the planned bodies' triangles are not built on the CPU ({gpu_tris} vs {cpu_tris} bytes)");
        let px = |img: &[u8], i: usize| [img[4 * i], img[4 * i + 1], img[4 * i + 2]];
        let n = (SIZE * SIZE) as usize;
        let drawn = (0..n).filter(|&i| px(&cpu, i) != [255, 255, 255]).count();
        assert!(drawn > n / 20, "the model covers the view: {drawn} pixels");
        let differ = (0..n).filter(|&i| px(&gpu, i).iter().zip(px(&cpu, i)).any(|(a, b)| a.abs_diff(b) > 24)).count();
        assert!(differ * 100 < drawn, "{differ} of {drawn} model pixels differ");
    }

    /// A device that cannot build the tessellation pipeline (D3D12's FXC rejected the shader
    /// at startup and the app panicked) still shows the model: the failure is caught, logged,
    /// and bodies are built on the CPU and drawn. A shader the device rejects stands in for FXC.
    /// Skipped without a GPU adapter.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bodies_are_built_on_the_cpu_when_the_tessellation_pipeline_fails() {
        const REJECTED: &str = "
            @group(0) @binding(1) var<storage, read_write> surfaces: array<u32>;
            @compute @workgroup_size(64)
            fn main(@builtin(global_invocation_id) gid: vec3<u32>) { surfaces[gid.x] = 1u; }
        ";
        let Some(tess) = crate::gpu::offscreen_tessellates(REJECTED) else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        assert!(!tess, "the rejected shader built a pipeline");
        let mut s = solidcraft_engine::Session::default();
        s.run_script(&solidcraft_engine::sample::script()).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        app.fit_view();
        app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess });
        const SIZE: u32 = 256;
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(SIZE as f32, SIZE as f32));
        let scene = build_scene(&app).merged();
        assert!(scene.planned.is_empty(), "no body waits for the GPU evaluation");
        assert!(!scene.tris.is_empty(), "the bodies are meshed on the CPU");
        let proj = projection(&app, rect);
        let e = proj.cam.eye().to_f32();
        let eye = [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }];
        let Some(img) = crate::gpu::render_offscreen(
            scene,
            None,
            proj.vp.to_f32(),
            proj.cam.back().to_f32(),
            eye,
            SIZE,
            Some(REJECTED),
            [0.0; 8],
            crate::gpu::analysis_colors(&Tokens::get().analysis),
        ) else {
            return;
        };
        let n = (SIZE * SIZE) as usize;
        let drawn = img.chunks(4).filter(|p| p[..3] != [255, 255, 255]).count();
        assert!(drawn > n / 20, "the model is drawn: {drawn} pixels");
    }

    /// Supersampling (#1288): the sample design drawn at 2× and filtered down shows the same
    /// model (the same pixels covered, give or take its outline) with smoothed edges: pixels part
    /// model, part background, which a single-sampled frame does not have. Skipped without a GPU
    /// adapter.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn supersampling_draws_the_same_model_with_smooth_edges() {
        let mut s = solidcraft_engine::Session::default();
        s.run_script(&solidcraft_engine::sample::script()).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        app.fit_view();
        app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess: false });
        const SIZE: u32 = 256;
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(SIZE as f32, SIZE as f32));
        let proj = projection(&app, rect);
        let e = proj.cam.eye().to_f32();
        let eye = [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }];
        let render =
            |ssaa: f32| crate::gpu::render_offscreen_ssaa(build_scene(&app).merged(), proj.vp.to_f32(), proj.cam.back().to_f32(), eye, SIZE, ssaa);
        let (Some(plain), Some(super2)) = (render(1.0), render(2.0)) else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let covered = |img: &[u8]| img.chunks(4).filter(|p| p[..3] != [255, 255, 255]).count();
        let (a, b) = (covered(&plain), covered(&super2));
        assert!(a > 2000, "the model is drawn: {a} pixels");
        assert!((a as f64 - b as f64).abs() < a as f64 * 0.1, "same model: {a} vs {b} pixels covered");
        // Edge pixels against the white background: neither white nor any colour the plain frame
        // has, so blended. A count well above the plain frame's own.
        let palette: std::collections::HashSet<[u8; 3]> = plain.chunks(4).map(|p| [p[0], p[1], p[2]]).collect();
        let new_colours = super2.chunks(4).filter(|p| !palette.contains(&[p[0], p[1], p[2]])).count();
        assert!(new_colours > 100, "the supersampled frame blends its edges: {new_colours} new colours");
    }

    /// The supersampled frame stays within the device's largest texture (#869) and a memory
    /// budget, and is not drawn at all when that leaves too little to gain.
    #[test]
    fn supersampling_keeps_within_the_device_texture_limit() {
        use crate::gpu::supersampled_size;
        assert_eq!(supersampled_size([1920.0, 1080.0], 2.0, 16384, 1), Some([3840, 2160]));
        assert_eq!(supersampled_size([1920.0, 1080.0], 1.5, 16384, 1), Some([2880, 1620]));
        // A 4096 texture limit: a 2560 px wide view gets 1.6×.
        let [w, h] = supersampled_size([2560.0, 1440.0], 2.0, 4096, 1).unwrap();
        assert!((4095..=4096).contains(&w) && (2303..=2304).contains(&h), "{w} x {h}");
        // Under 1.25× is not worth it: drawn as usual.
        assert_eq!(supersampled_size([3840.0, 2160.0], 2.0, 4096, 1), None);
        assert_eq!(supersampled_size([1920.0, 1080.0], 1.0, 16384, 1), None);
        assert_eq!(supersampled_size([0.0, 1080.0], 2.0, 16384, 1), None);
        // Under 1: drawn smaller and filtered up (the product preview past its budget, #1464).
        assert_eq!(supersampled_size([3840.0, 2160.0], 0.5, 4096, 4), Some([1920, 1080]));
        // 4× MSAA at 1440p: the memory budget caps it below 2×.
        let [w, h] = supersampled_size([2560.0, 1440.0], 2.0, 16384, 4).unwrap();
        assert!(w < 5120 && w as f32 >= 2560.0 * 1.25 && u64::from(w) * u64::from(h) * 32 <= 384 << 20, "{w} x {h}");
    }

    /// A hovered face lights solidly over a body drawn from its display plan on the GPU: the
    /// highlight uses the plan's triangles, not the reference mesh's other triangulation of the
    /// same face, which fought the model at equal depth and shimmered in wedges. Every face of a
    /// box with a pocket and a round boss, hovered, changes the same pixels as on the CPU path (where model and
    /// highlight share the reference mesh). Skipped without a GPU adapter.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_hovered_face_lights_whole_over_a_body_drawn_from_its_plan() {
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 40, "height": 40, "corner": [0, 0, 0], "operation": "new"})).unwrap();
        s.execute("solid.box", &json!({"length": 16, "width": 16, "height": 20, "corner": [12, 12, 20], "operation": "cut"})).unwrap();
        s.execute("solid.cylinder", &json!({"base": [33, 33, 40], "diameter": 10, "height": 10, "operation": "join"})).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        app.fit_view();
        const SIZE: u32 = 256;
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(SIZE as f32, SIZE as f32));
        let body = app.session.world_state().bodies[0].clone();
        let faces = body.body.face_count();
        let mut render = |tess: bool, hover: Option<usize>| {
            app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess });
            app.viewport.hover = hover.map(|index| Hit::Face { body: body.name.clone(), index, point: Vec3::ZERO });
            let (scene, highlight) = (build_scene(&app), build_highlight(&app).merged());
            assert_eq!(!scene.planned.is_empty(), tess, "the box is drawn from its plan exactly on the GPU path");
            let proj = projection(&app, rect);
            let e = proj.cam.eye().to_f32();
            let eye = [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }];
            let shader = tess.then_some(solidcraft_gpu::SHADER);
            crate::gpu::render_offscreen(
                scene,
                Some(highlight),
                proj.vp.to_f32(),
                proj.cam.back().to_f32(),
                eye,
                SIZE,
                shader,
                [0.0; 8],
                crate::gpu::analysis_colors(&Tokens::get().analysis),
            )
        };
        let lit = |plain: &[u8], hovered: &[u8]| -> Vec<bool> { plain.chunks(4).zip(hovered.chunks(4)).map(|(a, b)| a != b).collect() };
        let (Some(gpu_plain), Some(cpu_plain)) = (render(true, None), render(false, None)) else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut checked = 0;
        for f in 0..faces {
            let (Some(gpu), Some(cpu)) = (render(true, Some(f)), render(false, Some(f))) else { return };
            let (on_gpu, on_cpu) = (lit(&gpu_plain, &gpu), lit(&cpu_plain, &cpu));
            let area = on_cpu.iter().filter(|x| **x).count();
            if area < 400 {
                continue; // facing away or hidden
            }
            // Inside the face, two pixels in from its outline: a curved face's outline follows
            // its own facets, which the plan and the reference mesh cut differently.
            let n = SIZE as i32;
            let inside = |i: usize| {
                let (x, y) = (i as i32 % n, i as i32 / n);
                (-2..=2).all(|dy| (-2..=2).all(|dx| on_cpu.get(((y + dy).clamp(0, n - 1) * n + (x + dx).clamp(0, n - 1)) as usize) == Some(&true)))
            };
            let missed = (0..on_cpu.len()).filter(|&i| inside(i) && !on_gpu[i]).count();
            assert!(missed * 200 < area, "face {f}: {missed} of its {area} hovered pixels do not light (the highlight fights the model)");
            checked += 1;
        }
        assert!(checked >= 3, "{checked} faces seen lit");
    }

    /// An origin plane lying on a body's face is tinted evenly over it (#1739: a box built at the
    /// origin, seen from below in orthographic; the XY plane's square z-fought with the bottom
    /// face in stripes). Faces are pushed back by their slope (`FACE_BIAS`), the planes are not.
    /// Skipped without a GPU adapter.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn an_origin_plane_on_a_face_is_tinted_evenly() {
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 10, "corner": [0, 0, 0]})).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        app.cam = Camera {
            target: Vec3::new(16.904, 10.024, 3.661),
            yaw: -0.32,
            pitch: -0.9652,
            roll: 0.0,
            distance: 98.557,
            fov: 0.0,
            ..Default::default()
        };
        app.ui.perspective = false;
        const SIZE: u32 = 256;
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(SIZE as f32, SIZE as f32));
        let mut render = |origin: bool| {
            app.ui.show_origin = origin;
            app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess: false });
            let (scene, highlight) = (build_scene(&app), build_highlight(&app).merged());
            let proj = projection(&app, rect);
            let e = proj.cam.eye().to_f32();
            let img = crate::gpu::render_offscreen(
                scene,
                Some(highlight),
                proj.vp.to_f32(),
                proj.cam.back().to_f32(),
                [e[0], e[1], e[2], 0.0],
                SIZE,
                None,
                [0.0; 8],
                crate::gpu::analysis_colors(&Tokens::get().analysis),
            );
            (img, proj)
        };
        let (Some(with), proj) = render(true) else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let (Some(without), _) = render(false) else { return };
        app.ui.show_origin = true;
        let xy = origin_planes(&app).into_iter().find(|(n, _, _)| *n == "XY").map(|(_, _, q)| q).unwrap();
        let (w, h, r) = (f64::from(SIZE), f64::from(SIZE), scene_radius(&app));
        let (mut samples, mut plain) = (0, Vec::new());
        // Points inside the square (away from its outline), where it lies on the bottom face.
        for i in 1..12 {
            for j in 1..12 {
                let (u, v) = (f64::from(i) / 12.0, f64::from(j) / 12.0);
                let p = xy[0] + (xy[1] - xy[0]) * u + (xy[3] - xy[0]) * v;
                if !(p.x > 0.5 && p.x < 39.5 && p.y > 0.5 && p.y < 29.5) {
                    continue;
                }
                let Some((x, y, _)) = proj.cam.to_screen(p, w, h, r) else { continue };
                let at = (y as usize * SIZE as usize + x as usize) * 4;
                samples += 1;
                if with.get(at..at + 4) == without.get(at..at + 4) {
                    plain.push((x as u32, y as u32));
                }
            }
        }
        assert!(samples > 40, "{samples} points of the square on the face");
        assert!(
            plain.is_empty(),
            "{} of {samples} points of the XY plane's square do not show over the face: {:?}",
            plain.len(),
            &plain[..plain.len().min(6)]
        );
    }

    #[test]
    fn construction_drawing_and_hover_overlays_preserve_visible_gaps() {
        fn length(shape: &Shape) -> f32 {
            match shape {
                Shape::LineSegment { points, .. } => points[0].distance(points[1]),
                Shape::Path(path) => path.points.windows(2).map(|w| w[0].distance(w[1])).sum(),
                Shape::Vec(shapes) => shapes.iter().map(length).sum(),
                _ => 0.0,
            }
        }
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        h.settle();
        let proj = display_proj(&h.app);
        let a = Vec2::ZERO;
        let b = Vec2::new(20.0, 8.0);
        let whole = proj.to_screen(Vec3::new(b.x, b.y, 0.0)).unwrap().distance(proj.to_screen(Vec3::ZERO).unwrap());
        let mut draw = crate::tools::Tool::for_command("sketch.line").unwrap();
        draw.pts.push((a, None));
        draw.hover = Some((b, None));
        for mode in [0, 1, 2] {
            crate::sketch_palette::set_mode(&mut h.app, mode);
            let ctx = egui::Context::default();
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let painter = ui.ctx().layer_painter(egui::LayerId::background());
                crate::tools::preview(&h.app, &draw, &painter, &proj);
            });
            let preview: f32 = output.shapes.iter().map(|s| length(&s.shape)).sum();
            let r = h.app.run("sketch.line", json!({"points":[[0,0],[20,8]]})).unwrap();
            let id = r["curves"][0].as_str().unwrap().to_string();
            h.app.viewport.hover = Some(Hit::SketchCurve { sketch: h.app.session.active_sketch.unwrap(), id, straight: true });
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                hover_highlight(&h.app, &ui.ctx().layer_painter(egui::LayerId::background()), &proj);
            });
            let hover: f32 = output.shapes.iter().map(|s| length(&s.shape)).sum();
            assert!((preview - hover).abs() < 0.01, "mode {mode}: preview {preview}, hover {hover}");
            if mode == 0 {
                assert!((preview - whole).abs() < 0.01);
            } else {
                assert!(preview > whole * 0.25 && preview < whole * 0.85, "mode {mode} covers {preview} of {whole}");
            }
            h.app.run("edit.undo", json!({})).unwrap();
        }
        crate::sketch_palette::set_mode(&mut h.app, 0);
    }

    /// Headless pictures of the view (`ui.render`, the agent's view image) are drawn in the
    /// window's theme (#974) and colour vision mode (#778). The light theme's tokens are the engine's own colours, so a
    /// light picture in Off is unchanged; otherwise each part takes the token the window draws it
    /// with. Sketch states follow what the window does with them (`sketch_color`) and the axes the
    /// mode's triad, so the two can't drift apart.
    #[test]
    fn pictures_are_drawn_in_the_windows_theme() {
        use crate::theme::ColorVision;
        use solidcraft_engine::view::Palette;
        assert_eq!(scene_palette(false, ColorVision::Off), Palette::DEFAULT, "the light theme is the engine's colours");
        let rgb = |c: Color32| Rgb(c.r(), c.g(), c.b());
        let (p, t) = (scene_palette(true, ColorVision::Off), Tokens::dark());
        assert_eq!(p.background, (rgb(t.viewport_top), rgb(t.viewport_bottom)));
        assert_eq!(
            [p.grid, p.grid_major, p.edge, p.body, p.body_selected, p.cap],
            [t.grid, t.grid_major, t.body_edge, t.body, t.body_selected, t.section_cap].map(rgb)
        );
        assert_ne!(p.background, Palette::DEFAULT.background);
        for (dark, mode) in [false, true].into_iter().flat_map(|d| ColorVision::ALL.map(|m| (d, m))) {
            let (p, t) = (scene_palette(dark, mode), Tokens::themed(dark, mode));
            assert_eq!(p.axes, t.triad.map(rgb), "dark {dark}, {mode:?}: the axes");
            assert_eq!(p.analysis, t.analysis, "dark {dark}, {mode:?}: the surface analysis");
            for (tag, cpu) in [
                (colors::SKETCH, p.sketch),
                (colors::SKETCH_FIXED, p.sketch_fixed),
                (colors::SKETCH_DONE, p.sketch_done),
                (colors::SKETCH_LOCKED, p.sketch_locked),
                (colors::SKETCH_PROJECTED, p.sketch_projected),
                (colors::SKETCH_LOST, p.sketch_lost),
                (colors::SKETCH_CONSTRUCTION, p.sketch_construction),
                (colors::SKETCH_CENTERLINE, p.sketch_centerline),
            ] {
                let [r, g, b, _] = sketch_color(&t, tag);
                assert_eq!(cpu, Rgb(r, g, b), "dark {dark}, {mode:?}: the window draws {tag:?} in another colour");
            }
        }
    }

    /// The view's picture follows a theme change: its background is the window's.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_view_picture_follows_the_theme() {
        if !crate::gpu::test_picture_renderer() {
            return;
        }
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length": 40, "width": 30, "height": 20})).unwrap();
        h.app.cam = solidcraft_engine::view::home_camera(&h.app.session);
        for dark in [false, true, false] {
            h.app.ui.dark = dark;
            let img = crate::control::view_rgba(&h.app, 160, 120).unwrap();
            // The top row's most common colour: the background's top.
            let mut counts = std::collections::HashMap::new();
            for px in img.as_chunks::<4>().0.iter().take(160) {
                *counts.entry([px[0], px[1], px[2]]).or_insert(0) += 1;
            }
            let px = counts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c).unwrap_or_default();
            let want = if dark { Tokens::dark() } else { Tokens::light() }.viewport_top;
            let near = px.iter().zip([want.r(), want.g(), want.b()]).all(|(a, b)| a.abs_diff(b) <= 2);
            assert!(near, "dark {dark}: top row {px:?}, the window's background {want:?}");
        }
    }

    /// The view's picture is cut by the section the viewport shows: an open Section Analysis
    /// dialog's preview, a sketch's Slice.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_view_picture_uses_section_preview_and_sketch_slice() {
        fn pixels(h: &mut crate::scenario::Harness, cam: Camera) -> Vec<[u8; 4]> {
            h.app.cam = cam;
            let img = crate::control::view_rgba(&h.app, 320, 240).unwrap();
            img.as_chunks::<4>().0.to_vec()
        }
        if !crate::gpu::test_picture_renderer() {
            return;
        }
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":40,"width":30,"height":20})).unwrap();
        let cam = solidcraft_engine::view::home_camera(&h.app.session);
        let whole = pixels(&mut h, cam);
        let before = h.app.session.doc.clone();
        let undo = h.app.session.undo.len();
        h.app.session.selection = vec![Sel::Plane { name: "XZ".into() }];
        h.app.start("inspect.section");
        let Some(crate::dialogs::Kind::Section { offset, .. }) = h.app.dialog.as_mut().map(|d| &mut d.kind) else { panic!("section dialog") };
        *offset = "15 mm".into();
        assert!(h.app.session.section.is_none());
        let preview = pixels(&mut h, cam);
        assert!(whole.iter().zip(&preview).filter(|(a, b)| a != b).count() > 100, "the picture ignored the uncommitted section preview");
        assert!(std::sync::Arc::ptr_eq(&before, &h.app.session.doc));
        assert_eq!(h.app.session.undo.len(), undo);
        h.app.run("inspect.section", json!({"plane":"XZ","offset":15})).unwrap();
        h.app.dialog = None;
        assert_eq!(preview, pixels(&mut h, cam), "preview and committed section must draw identically");

        h.app.session.section = None;
        h.app.run("sketch.create", json!({"plane":{"origin":[0,0,10],"x_dir":[1,0,0],"y_dir":[0,1,0]}})).unwrap();
        let whole = pixels(&mut h, cam);
        h.app.run("sketch.options", json!({"slice":true})).unwrap();
        assert!(h.app.session.section.is_none());
        let sliced = pixels(&mut h, cam);
        assert!(whole.iter().zip(&sliced).filter(|(a, b)| a != b).count() > 100, "the picture ignored the active Sketch Slice");
        h.app.run("sketch.options", json!({"slice":false})).unwrap();
        assert_eq!(whole, pixels(&mut h, cam), "disabling Slice restores the whole-body pixels");
    }

    #[test]
    fn timeline_modifiers_highlight_on_hover_and_click() {
        for command in ["solid.fillet", "solid.chamfer"] {
            let mut h = crate::scenario::Harness::new();
            h.app.ui.show_origin = false;
            let base = h.app.run("solid.box", json!({"length":20,"width":20,"height":20})).unwrap()["feature"].as_u64().unwrap();
            let id = h.app.run(command, json!({"edges":[[0,0,10]],"radius":2,"distance":2,"name":"Blend"})).unwrap()["feature"].as_u64().unwrap();
            h.app.run("solid.box", json!({"length":5,"width":5,"height":5,"corner":[40,0,0]})).unwrap();
            h.frames(2);
            let geometry = feature_geometry(&h.app, id);
            assert_eq!(geometry.len(), 1, "{command}: {geometry:?}");
            let Sel::Face { body, index, .. } = &geometry[0] else { panic!("a modifier must highlight its face") };
            let st = h.app.session.world_state();
            let m = st.body(body).unwrap().mesh();
            let triangles = m.tri_face.iter().filter(|f| **f as usize == *index).count();
            assert!(triangles > 0 && triangles < m.triangles.len());
            let p = crate::scenario::handle_at("timeline:Blend").unwrap();
            h.step(&json!({"move":[p.x,p.y]})).unwrap();
            assert_eq!(h.app.viewport.hover_feature, Some(id));
            assert_eq!(build_highlight(&h.app).merged().tris.len(), triangles * 3 * crate::gpu::TRI_SIZE);
            assert!(!build_highlight(&h.app).merged().lines.is_empty());
            h.step(&json!({"click":[p.x,p.y]})).unwrap();
            assert_eq!(h.app.session.selection, vec![Sel::Feature { id }]);
            // Moving away removes hover but leaves the selected blend highlighted.
            h.step(&json!({"move":[5,5]})).unwrap();
            assert_eq!(h.app.viewport.hover_feature, None);
            assert_eq!(build_highlight(&h.app).merged().tris.len(), triangles * 3 * crate::gpu::TRI_SIZE);
            let bounds = selection_bounds(&h.app).unwrap();
            assert!(bounds.max.x <= 2.01 && bounds.max.y <= 2.01 && bounds.max.z > 19.99, "{bounds:?}");
            // Creator features retain whole-body feedback, while hidden bodies stay hidden.
            assert_eq!(feature_geometry(&h.app, base), vec![Sel::Body { name: body.clone() }]);
            h.app.session.visibility.hidden_bodies.push(body.clone());
            assert!(feature_geometry(&h.app, id).is_empty());
            assert!(selection_bounds(&h.app).is_none());
            assert!(build_highlight(&h.app).merged().tris.is_empty());
        }
    }

    #[test]
    fn timeline_modifier_highlight_follows_edits_and_rollback() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        app.run("solid.box", json!({"length":20,"width":20,"height":20})).unwrap();
        let id = app.run("solid.fillet", json!({"edges":[[0,0,10]],"radius":2})).unwrap()["feature"].as_u64().unwrap();
        // A later chamfer changes the same body without consuming the fillet.
        app.run("solid.chamfer", json!({"edges":[[20,20,10]],"distance":1})).unwrap();
        app.run("solid.move", json!({"bodies":["Body1"],"translate":[30,0,0]})).unwrap();
        app.run("body.rename", json!({"body":"Body1","name":"Moved"})).unwrap();
        app.session.selection = vec![Sel::Feature { id }];
        let bounds = selection_bounds(&app).unwrap();
        assert!(bounds.min.x >= 29.99 && bounds.max.x <= 32.01, "{bounds:?}");
        assert!(feature_geometry(&app, id).iter().all(|x| matches!(x, Sel::Face { body, .. } if body == "Moved")));
        app.run("timeline.roll_to", json!({"position":1})).unwrap();
        assert!(feature_geometry(&app, id).is_empty());
        app.run("timeline.roll_to", json!({})).unwrap();
        assert!(!feature_geometry(&app, id).is_empty());
        app.run("timeline.suppress", json!({"feature":id})).unwrap();
        assert!(feature_geometry(&app, id).is_empty());
    }

    #[test]
    fn timeline_modifier_highlight_follows_component_instances() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        app.run("component.create", json!({"name":"Part"})).unwrap();
        app.run("solid.box", json!({"length":20,"width":20,"height":20})).unwrap();
        let id = app.run("solid.fillet", json!({"edges":[[0,0,10]],"radius":2})).unwrap()["feature"].as_u64().unwrap();
        app.run("occurrence.move", json!({"occurrence":"Part:1","translate":[30,0,0]})).unwrap();
        app.run("occurrence.copy", json!({"component":"Part","translate":[0,40,0]})).unwrap();
        let geometry = feature_geometry(&app, id);
        assert_eq!(geometry.len(), 2, "both occurrences retain the fillet's face: {geometry:?}");
        app.session.selection = vec![Sel::Feature { id }];
        let bounds = selection_bounds(&app).unwrap();
        assert!(bounds.min.x >= 29.99 && bounds.max.x <= 32.01 && bounds.max.y >= 41.99, "{bounds:?}");
    }

    #[test]
    fn ray_segment_closest_takes_the_point_under_the_ray() {
        let (o, d) = (Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, -1.0));
        let x = |a: Vec3, b: Vec3| ray_segment_closest(o, d, a, b);
        assert!(x(Vec3::new(-5.0, 0.0, 0.0), Vec3::new(5.0, 0.0, -4.0)).dist(Vec3::new(0.0, 0.0, -2.0)) < 1e-9);
        // Beyond an end: the end.
        assert!(x(Vec3::new(1.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.0)).dist(Vec3::new(1.0, 0.0, 0.0)) < 1e-9);
        // Parallel, degenerate and non-finite input: the start, no panic.
        assert_eq!(x(Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 5.0)), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(x(Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)), Vec3::new(2.0, 0.0, 0.0));
        let nan = Vec3::new(f64::NAN, 0.0, 0.0);
        let _ = x(nan, Vec3::ZERO);
    }

    /// Hovering anywhere along a visible edge between two faces picks that edge, not a face
    /// (the edge used to pick only where its start or middle was as near as the face).
    #[test]
    fn edge_between_two_faces_picks_along_its_whole_length() {
        for style in [0u8, 2] {
            let ctx = egui::Context::default();
            let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
            app.run("solid.box", json!({"length":200,"width":120,"height":160})).unwrap();
            app.ui.show_origin = false;
            app.ui.visual_style = style;
            app.cam.yaw = 0.7;
            app.cam.pitch = 0.5;
            app.cam.target = Vec3::new(100.0, 60.0, 80.0);
            app.cam.distance = 500.0;
            hover_frame(&mut app, &ctx, vec![]);
            let proj = projection(&app, app.viewport.rect.unwrap());
            let st = app.session.world_state();
            let m = st.body("Body1").unwrap().mesh();
            let mut checked = 0;
            for (ei, e) in m.edges.iter().enumerate() {
                let (Some(a), Some(b)) = (e.first().copied(), e.last().copied()) else { continue };
                for k in 1..10 {
                    let p = a.lerp(b, k as f64 / 10.0);
                    let Some(sp) = proj.to_screen(p) else { continue };
                    let (o, d) = proj.ray(sp);
                    // Only edges in front at that point (shaded); every edge in wireframe.
                    let depth = (p - o).dot(d);
                    if style != 2 && m.raycast(o, d).is_some_and(|(t, _)| t < depth - 1e-3) {
                        continue;
                    }
                    let hits = pick(&app, &proj, sp);
                    let first = candidate(&app, &hits).map(|x| x.1);
                    assert!(matches!(first, Some(Sel::Edge { index, .. }) if index == ei), "style {style}: edge {ei} at {k}/10 picked {first:?}");
                    checked += 1;
                }
            }
            assert!(checked > 50, "style {style}: only {checked} samples");
        }
    }

    fn hover_frame(app: &mut SolveApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let selected = app.highlighted();
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() },
            |ui| {
                show(app, ui);
            },
        );
        // This headless frame checks interactions without applying textures to a renderer.
        output.textures_delta.clear();
        // Click selection is applied after rendering; hover itself must already be current.
        if app.viewport.gpu.is_some() && selected == app.highlighted() {
            assert_eq!(app.viewport.hl_key, highlight_key(app), "GPU highlight must use this frame's hover");
        }
    }

    fn hover_app(gpu: bool) -> SolveApp {
        let mut s = solidcraft_engine::Session::default();
        s.execute("sketch.create", &json!({"plane": "XY"})).unwrap();
        s.execute("sketch.point", &json!({"point": [0, 0]})).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        if gpu {
            // The callback is queued by egui; no device is needed to inspect scene updates.
            app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess: false });
        }
        app
    }

    fn space(pressed: bool) -> egui::Event {
        egui::Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::default() }
    }

    /// Holding Space is a momentary pan: the drag moves the view rather than the geometry under the
    /// pointer, in every preset. The point is over the sketch's own point, so without Space this
    /// drag would drag it (and the undo stack would grow).
    #[test]
    fn holding_space_pans_instead_of_dragging_geometry() {
        for preset in crate::prefs::NAV_PRESETS.iter().map(|p| p.id) {
            let ctx = egui::Context::default();
            let mut app = hover_app(false);
            app.preferences.nav = preset.into();
            app.ui.show_origin = false;
            hover_frame(&mut app, &ctx, vec![]);
            let point = projection(&app, app.viewport.rect.unwrap()).to_screen(Vec3::ZERO).unwrap();
            let before = app.cam;
            let undo = app.session.undo.len();
            let press = |pressed: bool, at: Pos2| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            };
            hover_frame(&mut app, &ctx, vec![space(true)]);
            hover_frame(&mut app, &ctx, vec![space(true), egui::Event::PointerMoved(point), press(true, point)]);
            hover_frame(&mut app, &ctx, vec![space(true), egui::Event::PointerMoved(point + vec2(40.0, 25.0))]);
            assert_ne!(app.cam, before, "{preset}: holding Space must pan the view");
            assert_eq!(app.session.undo.len(), undo, "{preset}: holding Space must not edit the design");
            assert!(app.viewport.point_drag.is_none(), "{preset}: holding Space must not drag geometry");
            hover_frame(&mut app, &ctx, vec![press(false, point + vec2(40.0, 25.0)), space(false)]);
        }
    }

    #[test]
    fn ctrl_left_drag_from_empty_space_orbits_in_every_preset() {
        for preset in crate::prefs::NAV_PRESETS.iter().map(|p| p.id) {
            for action in ["orbit", "tool", "point", "click"] {
                let ctx = egui::Context::default();
                let mut app = hover_app(false);
                app.preferences.nav = preset.into();
                app.ui.show_origin = false;
                hover_frame(&mut app, &ctx, vec![]);
                let point = projection(&app, app.viewport.rect.unwrap()).to_screen(Vec3::ZERO).unwrap();
                let (start, end) = match action {
                    "point" => (point, point + vec2(30.0, 20.0)),
                    "click" => (point, point),
                    _ => (point - vec2(60.0, 60.0), point - vec2(20.0, 30.0)),
                };
                if action == "tool" {
                    app.start("sketch.line");
                }
                let before = app.cam;
                let undo = app.session.undo.len();
                let modifiers = egui::Modifiers { ctrl: true, command: !cfg!(target_os = "macos"), ..Default::default() };
                let frame = |app: &mut SolveApp, mut events: Vec<egui::Event>| {
                    events.insert(0, egui::Event::ModifiersChanged(modifiers));
                    hover_frame(app, &ctx, events);
                };
                frame(
                    &mut app,
                    vec![
                        egui::Event::PointerMoved(start),
                        egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers },
                    ],
                );
                frame(&mut app, vec![egui::Event::PointerMoved(end)]);
                frame(&mut app, vec![]);
                match action {
                    "orbit" | "tool" => {
                        assert_ne!(app.cam, before, "{preset}: Ctrl+left drag on empty space orbits ({action})");
                        assert!(app.viewport.boxsel.is_none(), "{preset}: no box selection while orbiting");
                    }
                    "point" => {
                        assert_eq!(app.cam, before, "{preset}: Ctrl+drag on sketch geometry is not an orbit");
                        assert!(app.viewport.point_drag.is_some(), "{preset}: Ctrl+drag still drags the point");
                    }
                    _ => assert_eq!(app.cam, before, "{preset}: a Ctrl+click does not orbit"),
                }
                frame(&mut app, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers }]);
                frame(&mut app, vec![]);
                match action {
                    "click" => assert!(!app.session.selection.is_empty(), "{preset}: Ctrl+click still selects"),
                    "tool" => assert_eq!(app.session.undo.len(), undo, "{preset}: the orbit drag adds no geometry"),
                    "orbit" => assert!(app.session.selection.is_empty(), "{preset}: the end of the orbit selects nothing"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn openscad_left_drag_preserves_editing_and_selection() {
        for action in ["point", "box", "tool"] {
            let ctx = egui::Context::default();
            let mut app = hover_app(false);
            app.preferences.nav = crate::prefs::OPENSCAD_NAV.into();
            app.ui.show_origin = false;
            hover_frame(&mut app, &ctx, vec![]);
            let point = projection(&app, app.viewport.rect.unwrap()).to_screen(Vec3::ZERO).unwrap();
            let (start, end) =
                if action == "point" { (point, point + vec2(30.0, 20.0)) } else { (point - vec2(40.0, 40.0), point + vec2(40.0, 40.0)) };
            if action == "tool" {
                app.start("sketch.line");
                assert!(app.tool.is_some());
            }
            let before = app.cam;
            let undo = app.session.undo.len();
            let modifiers = egui::Modifiers { shift: action == "box", ..Default::default() };
            let frame = |app: &mut SolveApp, mut events: Vec<egui::Event>| {
                events.insert(0, egui::Event::ModifiersChanged(modifiers));
                let mut out = ctx.run_ui(
                    egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() },
                    |ui| show(app, ui),
                );
                out.textures_delta.clear();
            };
            frame(
                &mut app,
                vec![
                    egui::Event::PointerMoved(start),
                    egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers },
                ],
            );
            frame(&mut app, vec![egui::Event::PointerMoved(end)]);
            frame(&mut app, vec![]);
            assert_eq!(app.cam, before, "{action} drag must not orbit");
            if action == "point" {
                assert!(app.viewport.point_drag.is_some());
                assert_eq!(app.session.undo.len(), undo + 1);
            } else if action == "box" {
                assert!(app.viewport.boxsel.is_some());
            } else {
                assert!(app.tool.is_some());
                assert!(app.viewport.primary_nav.is_none());
            }
            frame(&mut app, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers }]);
            if action == "box" {
                assert!(!app.session.selection.is_empty());
            }
        }
    }

    #[test]
    fn openscad_empty_space_drags_navigate_without_editing_or_box_selecting() {
        for (button, shift) in [
            (egui::PointerButton::Primary, false),
            (egui::PointerButton::Secondary, false),
            (egui::PointerButton::Middle, false),
            (egui::PointerButton::Secondary, true),
            (egui::PointerButton::Middle, true),
        ] {
            let ctx = egui::Context::default();
            let mut app = hover_app(false);
            app.preferences.nav = crate::prefs::OPENSCAD_NAV.into();
            hover_frame(&mut app, &ctx, vec![]);
            let point = projection(&app, app.viewport.rect.unwrap()).to_screen(Vec3::ZERO).unwrap();
            let p = if button == egui::PointerButton::Primary { point + vec2(80.0, 80.0) } else { point };
            let before = app.cam;
            let undo = app.session.undo.len();
            let modifiers = egui::Modifiers { shift, ..Default::default() };
            let frame = |app: &mut SolveApp, mut events: Vec<egui::Event>| {
                events.insert(0, egui::Event::ModifiersChanged(modifiers));
                let _ = ctx.run_ui(
                    egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() },
                    |ui| show(app, ui),
                );
            };
            frame(&mut app, vec![egui::Event::PointerMoved(p), egui::Event::PointerButton { pos: p, button, pressed: true, modifiers }]);
            let end = p + vec2(30.0, 20.0);
            frame(&mut app, vec![egui::Event::PointerMoved(end)]);
            frame(&mut app, vec![]);
            assert!(app.viewport.hover.is_none());
            assert!(app.viewport.boxsel.is_none() && app.viewport.point_drag.is_none());
            frame(&mut app, vec![egui::Event::PointerButton { pos: end, button, pressed: false, modifiers }]);
            assert_eq!(app.session.undo.len(), undo);
            assert!(app.session.selection.is_empty());
            assert!(app.viewport.context_menu.is_none());
            if button == egui::PointerButton::Primary {
                assert_ne!(app.cam.yaw, before.yaw);
                assert_eq!(app.cam.target, before.target);
                assert_eq!(app.cam.distance, before.distance);
            } else if shift {
                assert_ne!(app.cam.distance, before.distance);
                assert_eq!(app.cam.target, before.target);
                assert_eq!(app.cam.yaw, before.yaw);
            } else {
                assert_ne!(app.cam.target, before.target);
                assert_eq!(app.cam.distance, before.distance);
                assert_eq!(app.cam.yaw, before.yaw);
            }
        }
    }

    #[test]
    fn touchpad_modifier_motion_navigates_without_buttons_or_document_changes() {
        for alt in [false, true] {
            let ctx = egui::Context::default();
            let mut app = hover_app(false);
            app.preferences.nav = "touchpad".into();
            let p = pos2(350.0, 300.0);
            hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
            let before = app.cam;
            let doc = app.session.doc.to_json();
            let undo = app.session.undo.len();
            let log = app.session.log.len();
            let mods = egui::Modifiers { shift: !alt, alt, ..Default::default() };
            hover_frame(&mut app, &ctx, vec![egui::Event::ModifiersChanged(mods)]);
            hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p + vec2(30.0, 20.0))]);
            assert!(!ctx.input(|i| i.pointer.any_down()));
            if alt {
                assert!((app.cam.yaw - before.yaw - 0.24).abs() < 1e-6);
                assert_eq!(app.cam.target, before.target);
            } else {
                assert_ne!(app.cam.target, before.target);
                assert_eq!(app.cam.yaw, before.yaw);
            }
            assert_eq!(app.cam.distance, before.distance);
            let after = app.cam;
            for _ in 0..3 {
                hover_frame(&mut app, &ctx, vec![]);
                assert_eq!(app.cam, after, "no drift between pointer events");
                assert!(app.viewport.hover.is_none());
            }
            hover_frame(&mut app, &ctx, vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE), egui::Event::PointerMoved(p)]);
            assert_eq!(app.cam, after, "release ends modifier-only navigation");
            assert_eq!(app.session.doc.to_json(), doc);
            assert_eq!(app.session.undo.len(), undo);
            assert_eq!(app.session.log.len(), log, "continuous motion must not flood the command log");
            assert!(app.session.selection.is_empty());
            assert!(app.viewport.boxsel.is_none() && app.viewport.point_drag.is_none());
        }
    }

    #[test]
    fn touchpad_motion_uses_the_effective_perspective_projection() {
        let mut app = hover_app(false);
        app.ui.perspective = true;
        app.cam = Camera { yaw: 0.0, pitch: 0.0, distance: 100.0, ..Default::default() };
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let pivot = Vec3::new(15.0, -50.0, 0.0);
        let before = projection(&app, rect).to_screen(pivot).unwrap();
        camera_motion(&mut app, rect, json!({"motion":"orbit","delta":[20,10],"pivot":pivot}));
        let after = projection(&app, rect).to_screen(pivot).unwrap();
        assert!(before.distance(after) < 1e-3, "rendered pivot drifted from {before:?} to {after:?}");
        assert_eq!(app.cam.fov, 0.0, "the projection toggle remains UI state, not a stored-camera mutation");
    }

    #[test]
    fn touchpad_motion_does_not_move_the_camera_under_a_canvas_triad() {
        let ctx = egui::Context::default();
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length":20,"width":20,"height":20})).unwrap();
        s.selection = vec![Sel::Body { name: "Body1".into() }];
        let mut app = SolveApp::new(s, crate::Services::default());
        app.preferences.nav = "touchpad".into();
        app.start("solid.move");
        app.dialog.as_mut().unwrap().focus = false;
        let p = pos2(100.0, 100.0);
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        let triad = crate::dialogs_move::triad(&app, app.dialog.as_ref().unwrap()).unwrap();
        let handle = projection(&app, app.viewport.rect.unwrap()).to_screen(triad.center).unwrap();
        let mods = egui::Modifiers { alt: true, ..Default::default() };
        hover_frame(&mut app, &ctx, vec![egui::Event::ModifiersChanged(mods)]);
        assert!(!ctx.egui_wants_keyboard_input());
        let before = app.cam;
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(handle)]);
        assert_eq!(app.cam, before, "a triad handle owns its input before the camera changes");
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
        assert_eq!(app.cam, before, "leaving the handle rearms without a jump");
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p + vec2(5.0, 0.0))]);
        assert_ne!(app.cam.yaw, before.yaw, "the exposed viewport remains navigable with Move open");
    }

    #[test]
    fn touchpad_motion_preserves_input_ownership_and_rearms_without_jumping() {
        let ctx = egui::Context::default();
        let mut app = hover_app(false);
        app.preferences.nav = "touchpad".into();
        app.preferences.orbit_around = crate::prefs::OrbitAround::Cursor;
        let p = pos2(350.0, 300.0);
        let alt = egui::Modifiers { alt: true, ..Default::default() };
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
        hover_frame(&mut app, &ctx, vec![egui::Event::ModifiersChanged(alt)]);
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p + vec2(10.0, 0.0))]);
        let pivot = app.viewport.orbit_pivot;
        assert!(pivot.is_some());
        hover_frame(&mut app, &ctx, vec![]);
        assert_eq!(app.viewport.orbit_pivot, pivot, "pauses keep the same orbit pivot");
        let before = app.cam;
        let bar = app.viewport.layout.as_ref().unwrap().navigation.center();
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(bar)]);
        assert_eq!(app.cam, before, "the navigation bar owns its input");
        assert!(app.viewport.orbit_pivot.is_none());
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
        assert_eq!(app.cam, before, "reentry must not apply the distance from an overlay");
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p + vec2(5.0, 0.0))]);
        assert_ne!(app.cam.yaw, before.yaw);
        let before = app.cam;
        for mods in [
            egui::Modifiers::NONE,
            egui::Modifiers { shift: true, alt: true, ..Default::default() },
            egui::Modifiers { shift: true, ctrl: true, ..Default::default() },
            egui::Modifiers { alt: true, command: true, mac_cmd: true, ..Default::default() },
        ] {
            hover_frame(&mut app, &ctx, vec![egui::Event::ModifiersChanged(mods), egui::Event::PointerMoved(p)]);
            assert_eq!(app.cam, before, "unbound combinations must not navigate");
        }
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        hover_frame(&mut app, &ctx, vec![egui::Event::ModifiersChanged(shift), egui::Event::PointerMoved(p)]);
        hover_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: shift }],
        );
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p + vec2(30.0, 20.0))]);
        assert_eq!(app.cam, before, "Shift+left drag must still box-select, not pan");
        assert!(app.viewport.boxsel.is_some());
        hover_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerButton { pos: p + vec2(30.0, 20.0), button: egui::PointerButton::Primary, pressed: false, modifiers: shift }],
        );
        assert_eq!(app.cam, before, "the release frame still belongs to selection");
    }

    #[test]
    fn touchpad_focus_and_preset_changes_discard_old_motion() {
        let ctx = egui::Context::default();
        let mut app = hover_app(false);
        app.preferences.nav = "touchpad".into();
        let alt = egui::Modifiers { alt: true, ..Default::default() };
        let frame = |app: &mut SolveApp, focused: bool, p: Pos2| {
            let mut out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                    focused,
                    events: vec![egui::Event::ModifiersChanged(alt), egui::Event::PointerMoved(p)],
                    ..Default::default()
                },
                |ui| show(app, ui),
            );
            out.textures_delta.clear();
        };
        frame(&mut app, true, pos2(300.0, 300.0));
        frame(&mut app, true, pos2(310.0, 300.0));
        let before = app.cam;
        frame(&mut app, false, pos2(500.0, 400.0));
        frame(&mut app, true, pos2(350.0, 300.0));
        assert_eq!(app.cam, before, "focus loss and regain must not jump");
        frame(&mut app, true, pos2(360.0, 300.0));
        assert!((app.cam.yaw - before.yaw - 0.08).abs() < 1e-6);
        app.preferences.nav = "fusion".into();
        let before = app.cam;
        frame(&mut app, true, pos2(450.0, 300.0));
        assert_eq!(app.cam, before);
        app.preferences.nav = "touchpad".into();
        frame(&mut app, true, pos2(500.0, 300.0));
        assert_eq!(app.cam, before, "changing presets must rearm before moving");
        app.viewport.spin = true;
        frame(&mut app, true, pos2(510.0, 300.0));
        assert!(!app.viewport.spin, "pointer-only navigation stops continuous orbit");
        assert!((app.cam.yaw - before.yaw - 0.08).abs() < 1e-6);
    }

    #[test]
    fn touchpad_vertical_trackpad_scroll_zooms_without_overwriting_saved_pan() {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.nav = "touchpad".into();
        h.app.preferences.smooth_wheel_zoom = false;
        h.app.preferences.zoom_to_cursor = false;
        h.call("ui.move", json!({"x":800,"y":500}));
        let before = h.app.cam;
        for phase in [egui::TouchPhase::Start, egui::TouchPhase::Move, egui::TouchPhase::End] {
            h.app.synthetic.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 120.0),
                modifiers: egui::Modifiers::NONE,
                phase,
            });
            h.frame();
        }
        assert!((h.app.cam.distance - before.distance * (-0.18_f64).exp()).abs() < 1e-6);
        assert_eq!(h.app.cam.target, before.target);
        assert!(h.app.preferences.trackpad_pan);
        let after = h.app.cam;
        h.frames(60);
        assert_eq!(h.app.cam, after);
        h.app.preferences.nav = "fusion".into();
        for phase in [egui::TouchPhase::Start, egui::TouchPhase::Move, egui::TouchPhase::End] {
            h.app.synthetic.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 120.0),
                modifiers: egui::Modifiers::NONE,
                phase,
            });
            h.frame();
        }
        assert_eq!(h.app.cam.distance, after.distance);
        assert_ne!(h.app.cam.target, after.target);
    }

    #[test]
    fn hover_stays_off_between_navigation_events_and_returns_on_release() {
        for gpu in [false, true] {
            for button in [egui::PointerButton::Middle, egui::PointerButton::Secondary, egui::PointerButton::Primary] {
                let ctx = egui::Context::default();
                let mut app = hover_app(gpu);
                if button == egui::PointerButton::Primary {
                    app.viewport.nav = Some(NavMode::Orbit);
                }
                hover_frame(&mut app, &ctx, vec![]);
                let p = projection(&app, app.viewport.rect.unwrap()).to_screen(Vec3::ZERO).unwrap();
                hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
                assert!(app.viewport.hover.is_some());
                hover_frame(&mut app, &ctx, vec![egui::Event::PointerButton { pos: p, button, pressed: true, modifiers: Default::default() }]);
                assert!(app.viewport.hover.is_none(), "hover on navigation press");
                for offset in [1.0, 2.0, 3.0] {
                    hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p + vec2(offset, 0.0))]);
                    assert!(app.viewport.hover.is_none(), "hover while moving");
                    for _ in 0..3 {
                        hover_frame(&mut app, &ctx, vec![]);
                        assert!(app.viewport.hover.is_none(), "hover between movement events");
                    }
                }
                hover_frame(&mut app, &ctx, vec![egui::Event::PointerButton { pos: p, button, pressed: false, modifiers: Default::default() }]);
                assert!(app.viewport.hover.is_some(), "hover must return on release");
            }
        }
    }

    /// Two-handed use: while a 3D mouse turns the view, the pointer still preselects what is
    /// under it in the view as it is now, and a click picks it (it neither does nothing nor
    /// clears the selection).
    #[test]
    fn device_navigation_keeps_hover_and_click_picking() {
        use crate::space_mouse::{DeviceMotion, Motion};
        for gpu in [false, true] {
            let ctx = egui::Context::default();
            let mut s = solidcraft_engine::Session::default();
            s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
            let mut app = SolveApp::new(s, crate::Services::default());
            app.ui.show_origin = false;
            if gpu {
                app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess: false });
            }
            app.cam.target = Vec3::new(20.0, 15.0, 10.0);
            hover_frame(&mut app, &ctx, vec![]);
            // The middle of the top face.
            let p = projection(&app, app.viewport.rect.unwrap()).to_screen(Vec3::new(20.0, 15.0, 20.0)).unwrap();
            hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
            assert!(matches!(app.viewport.hover, Some(Hit::Face { .. })), "{:?}", app.viewport.hover);
            let turn = || Some(DeviceMotion { motion: Motion { translation: [0.0; 3], rotation: [0.0, 0.0, 0.1] }, seconds: 0.016 });
            let mut pick = app.viewport.last_pick.as_ref().unwrap().0;
            for _ in 0..3 {
                let before = app.cam;
                app.viewport.device_motion = turn();
                hover_frame(&mut app, &ctx, vec![]);
                assert_ne!(app.cam, before, "the device turns the view");
                assert!(matches!(app.viewport.hover, Some(Hit::Face { .. })), "hover stays on while the device moves the view");
                let now = app.viewport.last_pick.as_ref().unwrap().0;
                assert_ne!(now, pick, "the pick follows the turned view");
                pick = now;
            }
            // A click while the device keeps turning the view selects the face under the pointer.
            let button =
                |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            app.viewport.device_motion = turn();
            hover_frame(&mut app, &ctx, vec![button(true)]);
            app.viewport.device_motion = turn();
            hover_frame(&mut app, &ctx, vec![button(false)]);
            assert!(matches!(app.session.selection.as_slice(), [Sel::Face { .. }]), "gpu {gpu}: {:?}", app.session.selection);
        }
    }

    #[test]
    fn hover_stays_off_during_spin_and_animated_views() {
        let ctx = egui::Context::default();
        let mut app = hover_app(true);
        hover_frame(&mut app, &ctx, vec![]);
        let p = projection(&app, app.viewport.rect.unwrap()).to_screen(Vec3::ZERO).unwrap();
        hover_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
        assert!(app.viewport.hover.is_some());
        app.viewport.spin = true;
        for _ in 0..3 {
            hover_frame(&mut app, &ctx, vec![]);
            assert!(app.viewport.hover.is_none());
        }
        app.viewport.spin = false;
        hover_frame(&mut app, &ctx, vec![]);
        assert!(app.viewport.hover.is_some());
        app.cam_anim = Some(solidcraft_engine::render::CameraAnim::new(app.cam, app.cam, 0.0));
        for _ in 0..3 {
            hover_frame(&mut app, &ctx, vec![]);
            assert!(app.viewport.hover.is_none());
        }
        app.cam_anim = None;
        hover_frame(&mut app, &ctx, vec![]);
        assert!(app.viewport.hover.is_some());
    }

    /// After a close-up refines a body (#1320), the model draws it from the refined plan (on the
    /// GPU) or its mesh (on the CPU), its edges come from that mesh too, and a hovered face lights
    /// exactly triangles the model draws, never another triangulation (#1064).
    #[test]
    fn a_refined_body_highlights_on_its_drawn_triangles() {
        use solidcraft_engine::doc::display_quality::current;
        let tris_of = |bytes: &[u8]| -> Vec<[u32; 9]> {
            bytes
                .chunks(3 * crate::gpu::TRI_SIZE)
                .map(|t| {
                    let mut v: Vec<[u32; 3]> = t
                        .chunks(crate::gpu::TRI_SIZE)
                        .map(|c| std::array::from_fn(|i| u32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().unwrap())))
                        .collect();
                    v.sort_unstable();
                    std::array::from_fn(|i| v[i / 3][i % 3])
                })
                .collect()
        };
        let mesh_tris = |m: &solidcraft_engine::geom::Mesh| -> std::collections::HashSet<[u32; 9]> {
            let mut sc = GpuScene::default();
            for f in 0..m.tri_face.iter().map(|f| *f as usize + 1).max().unwrap_or(0) {
                face_tris(&mut sc, m, f, [0; 4], true);
            }
            tris_of(&sc.tris).into_iter().collect()
        };
        for tess in [true, false] {
            let mut app = cylinder_app();
            app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess });
            let body = app.session.world_state().bodies[0].clone();
            let before = build_scene(&app).merged();
            let key = scene_key(&app);
            assert!(app.viewport.lod.lock().unwrap().force(&body, current(), 2));
            assert_ne!(scene_key(&app), key, "a refinement rebuilds the scene");
            let (plan, mesh) = refined(&app, &body).unwrap();
            assert!(mesh.triangles.len() > body.plan_mesh().unwrap().triangles.len() * 2);
            let scene = build_scene(&app).merged();
            if tess {
                assert!(std::sync::Arc::ptr_eq(&scene.planned[0].plan, &plan), "faces drawn from the refined plan");
            } else {
                assert_eq!(scene.tris.len(), mesh.triangles.len() * 3 * crate::gpu::TRI_SIZE, "faces built from the refined mesh");
                assert!(scene.tris.len() > before.tris.len());
            }
            // Edges (lines) from the refined mesh: finer polylines.
            assert!(scene.lines.len() > before.lines.len(), "{} vs {}", scene.lines.len(), before.lines.len());
            let drawn = mesh_tris(&mesh);
            let mut lit_faces = 0;
            for index in 0..body.body.face_count() {
                app.viewport.hover = Some(Hit::Face { body: body.name.clone(), index, point: Vec3::ZERO });
                let lit = tris_of(&build_highlight(&app).merged().tris);
                assert!(!lit.is_empty(), "face {index} lights");
                assert!(lit.iter().all(|t| drawn.contains(t)), "face {index}: every lit triangle is a drawn one");
                lit_faces += 1;
            }
            assert!(lit_faces >= 3, "{lit_faces} faces");
        }
    }

    fn cylinder_app() -> SolveApp {
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.cylinder", &json!({"radius": 10, "height": 20})).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        app.ui.show_origin = false;
        app
    }

    #[test]
    fn body_highlight_buffers_are_reused_when_the_camera_orbits_or_pans() {
        let mut app = cylinder_app();
        app.ui.show_origin = true;
        app.viewport.hover_bodies = vec!["Body1".into()];
        let key = highlight_key(&app);
        let before = build_highlight(&app).merged();
        assert!(!before.tris.is_empty());
        app.cam.orbit(25.0, 10.0);
        app.cam.roll_by(0.3);
        app.cam.pan(20.0, -15.0, 600.0);
        assert_eq!(highlight_key(&app), key, "world-space feedback must stay resident during navigation");
        let after = build_highlight(&app).merged();
        assert_eq!(before.tris, after.tris);
        assert_eq!(before.lines, after.lines);
        app.cam.zoom_at(1.2, None);
        assert_ne!(highlight_key(&app), key, "reference geometry follows the view's scale");

        let mut app = hover_app(true);
        let sketch = app.session.active_sketch.unwrap();
        app.run("sketch.circle.center", json!({"center":[0,0],"radius":5})).unwrap();
        app.session.selection = vec![Sel::Profile { sketch, index: 0 }];
        let key = highlight_key(&app);
        let before = build_highlight(&app).merged();
        assert!(!before.trans.is_empty());
        app.cam.orbit(25.0, 10.0);
        assert_ne!(highlight_key(&app), key, "profile feedback still follows the projected curves and lift toward the eye");
        assert_ne!(before.trans, build_highlight(&app).merged().trans);
    }

    #[test]
    fn world_highlight_block_survives_zoom_and_refreshes_when_bodies_are_hidden() {
        let mut app = cylinder_app();
        app.ui.show_origin = true;
        app.viewport.hover_bodies = vec!["Body1".into()];
        let before = build_highlight(&app);
        let bytes = before.blocks[0].bytes.clone();
        assert!(!bytes.tris.is_empty());
        app.cam.zoom_at(4.0, None);
        let after = build_highlight(&app);
        assert!(std::sync::Arc::ptr_eq(&bytes, &after.blocks[0].bytes), "zoom must not regenerate or upload body feedback");
        assert_ne!(before.guide_trans, after.guide_trans, "reference planes still resize with the view");
        app.session.visibility.hidden_bodies.push("Body1".into());
        let hidden = build_highlight(&app);
        assert!(!std::sync::Arc::ptr_eq(&bytes, &hidden.blocks[0].bytes));
        assert!(hidden.blocks[0].bytes.tris.is_empty(), "hidden bodies must lose their feedback");
        app.session.visibility.hidden_bodies.clear();
        let shown = build_highlight(&app);
        assert_eq!(shown.blocks[0].bytes.tris, bytes.tris);
    }

    #[test]
    fn whole_body_highlight_preserves_each_triangle_and_selected_face_once() {
        let mut app = cylinder_app();
        let body = app.session.world_state().bodies[0].clone();
        let expected = shown_mesh(&app, &body, &None).triangles.len() * 3 * crate::gpu::TRI_SIZE;
        app.viewport.hover_bodies = vec![body.name.clone()];
        for selected in
            [vec![], vec![Sel::Face { body: body.name.clone(), index: 0, point: Vec3::ZERO }], vec![Sel::Body { name: body.name.clone() }]]
        {
            app.session.selection = selected;
            assert_eq!(build_highlight(&app).merged().tris.len(), expected, "whole-body feedback must neither omit nor duplicate selected faces");
        }
    }

    #[test]
    fn smooth_wall_display_preserves_wireframe_and_edge_selection() {
        let mut s = solidcraft_engine::Session::default();
        let sketch = s.execute("sketch.create", &json!({"plane":"XY"})).unwrap()["sketch"].as_u64().unwrap();
        s.execute("sketch.ellipse", &json!({"center":[0,0],"major":[25,0],"minor_radius":5})).unwrap();
        let inner =
            s.execute("sketch.offset", &json!({"curves":["e1"],"distance":-1,"chain":false})).unwrap()["curves"][0].as_str().unwrap().to_string();
        let outer =
            s.execute("sketch.offset", &json!({"curves":[inner],"distance":-0.5,"chain":false})).unwrap()["curves"][0].as_str().unwrap().to_string();
        s.execute(
            "solid.extrude",
            &json!({"sketch":sketch,"distance":3,"profiles":{"type":"whole_boundaries","profiles":[{"outer":[outer],"holes":[[inner]]}]}}),
        )
        .unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        app.ui.ground_shadow = false;
        let st = app.session.world_state();
        let b = st.bodies.first().unwrap();
        let mesh = b.mesh();
        assert_eq!(mesh.edges.len(), 1536);
        assert_eq!(mesh.smooth_edges.iter().filter(|f| **f).count(), 512);
        let smooth = mesh.smooth_edges.iter().position(|f| *f).unwrap();
        let expected = mesh.edges.iter().enumerate().filter(|(i, _)| mesh.shaded_edge_visible(*i)).map(|(_, e)| e.len() - 1).sum::<usize>();
        let shaded = build_scene(&app).merged();
        assert_eq!(shaded.lines.len() / crate::gpu::LINE_SIZE, expected);
        assert!(!shaded.tris.is_empty());
        app.ui.visual_style = 2;
        let wire = build_scene(&app).merged();
        let expected_wire =
            mesh.edges.iter().enumerate().filter(|(i, _)| !mesh.seams.get(*i).copied().unwrap_or(false)).map(|(_, e)| e.len() - 1).sum::<usize>();
        assert_eq!(wire.lines.len() / crate::gpu::LINE_SIZE, expected_wire);
        assert!(expected_wire > expected);
        assert!(wire.tris.is_empty());
        app.ui.visual_style = 0;
        app.session.selection = vec![Sel::Edge { body: b.name.clone(), index: smooth, point: polyline_mid(&mesh.edges[smooth]) }];
        assert!(!build_highlight(&app).merged().lines.is_empty());
        let mut without_display_mask = (*mesh).clone();
        without_display_mask.smooth_edges.clear();
        assert_eq!(mesh.surface_patch(mesh.edge_faces[smooth][0] as usize), without_display_mask.surface_patch(mesh.edge_faces[smooth][0] as usize));
        let cam = solidcraft_engine::view::home_camera(&app.session);
        let cpu = solidcraft_engine::view::scene(&app.session, &cam);
        let actual_edges = cpu.lines.iter().filter(|line| line.color == solidcraft_engine::view::colors::EDGE).count();
        assert_eq!(actual_edges, mesh.edges.iter().enumerate().filter(|(i, _)| mesh.shaded_edge_visible(*i)).count());
        if let Ok(directory) = solidcraft_engine::geom::env::var("SMOOTH_SCREENSHOTS").ok_or(()) {
            std::fs::create_dir_all(&directory).unwrap();
            for (name, old_edges, wireframe) in [("after", false, false), ("before", true, false), ("wireframe", true, true)] {
                let mut scene = solidcraft_engine::view::scene(&app.session, &cam);
                scene.lines.retain(|line| line.color == solidcraft_engine::view::colors::EDGE);
                if old_edges {
                    scene.lines.clear();
                    for (i, edge) in mesh.edges.iter().enumerate() {
                        if mesh.edge_pickable(i) {
                            scene.lines.push(solidcraft_engine::render::SceneLine {
                                points: edge.clone(),
                                color: solidcraft_engine::view::colors::EDGE,
                                width: 1.2,
                                on_top: false,
                            });
                        }
                    }
                }
                if wireframe {
                    scene.meshes.clear();
                }
                assert!(crate::gpu::test_picture_renderer(), "SMOOTH_SCREENSHOTS needs a GPU");
                let png = solidcraft_engine::capture::png(&scene, &cam, 1200, 800).unwrap();
                std::fs::write(std::path::Path::new(&directory).join(format!("smooth-ring-{name}.png")), png.bytes).unwrap();
            }
        }
    }

    #[test]
    fn certified_small_crease_overrides_old_seam_fit_for_picking_and_patches() {
        let mut s = solidcraft_engine::Session::default();
        let sketch = s.execute("sketch.create", &json!({"plane":"XY"})).unwrap()["sketch"].as_u64().unwrap();
        let step = std::f64::consts::TAU / 8.;
        let k = 4. / 3. * (step / 4.).tan() * 10.;
        for i in 0..8 {
            let a = i as f64 * step;
            let b = (i + 1) as f64 * step;
            let p0 = [10. * a.cos(), 10. * a.sin()];
            let p3 = if i == 7 { [10., 0.] } else { [10. * b.cos(), 10. * b.sin()] };
            let mut p1 = [p0[0] - k * a.sin(), p0[1] + k * a.cos()];
            let mut p2 = [p3[0] + k * b.sin(), p3[1] - k * b.cos()];
            if i == 0 {
                p1[0] += k * 1e-6;
                p2[0] += k * 1e-6 * b.cos();
                p2[1] += k * 1e-6 * b.sin();
            }
            s.execute("sketch.spline.control_point", &json!({"points":[p0,p1,p2,p3]})).unwrap();
        }
        s.execute("solid.extrude", &json!({"sketch":sketch,"distance":3})).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        let st = app.session.world_state();
        let body = st.bodies.first().unwrap();
        let mesh = body.mesh();
        let crease = mesh
            .edges
            .iter()
            .position(|line| {
                line.first().zip(line.last()).is_some_and(|(a, b)| {
                    (a.x - 10.).abs() < 1e-8 && a.y.abs() < 1e-8 && (b.x - 10.).abs() < 1e-8 && b.y.abs() < 1e-8 && (a.z - b.z).abs() > 2.9
                })
            })
            .unwrap();
        assert!(mesh.seams[crease], "the fixture must exercise the coarse seam false positive");
        assert!(mesh.sharp_edges[crease]);
        assert!(mesh.shaded_edge_visible(crease));
        assert!(mesh.edge_pickable(crease));
        assert!(mesh.edge_faces[crease].iter().any(|f| mesh.surface_patch(*f as usize).len() == 1));
        let mid = polyline_mid(&mesh.edges[crease]);
        app.cam = app.cam.looking_from(Vec3::new(30., 0., 1.5));
        app.cam.target = Vec3::new(0., 0., 1.5);
        for perspective in [false, true] {
            app.ui.perspective = perspective;
            let proj = projection(&app, Rect::from_min_size(pos2(0., 0.), vec2(800., 600.)));
            let hits = pick(&app, &proj, proj.to_screen(mid).unwrap());
            assert!(hits.iter().any(|hit| matches!(hit,Hit::Edge{index,..} if *index==crease)), "{hits:?}");
        }
    }

    #[test]
    fn circular_items_measure_as_whole_rims_and_surfaces() {
        let mut app = cylinder_app();
        let st = app.session.world_state();
        let b = &st.bodies[0];
        let m = b.mesh();
        let edges: Vec<Sel> = m
            .edges
            .iter()
            .enumerate()
            .filter(|(_, p)| p.len() > 3 && p.iter().all(|q| (q.z - 20.0).abs() < 1e-6))
            .map(|(index, p)| Sel::Edge { body: b.name.clone(), index, point: polyline_mid(p) })
            .collect();
        let result = app.session.measure_items(&edges);
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        assert!((result["items"][0]["length_mm"].as_f64().unwrap() - 20.0 * std::f64::consts::PI).abs() < 0.2, "{result}");
        app.session.selection = edges;
        let d = crate::dialogs::Dialog::for_command(&app, "inspect.measure").unwrap();
        assert_eq!(d.items().len(), 1);

        let faces = m.edge_faces.iter().zip(&m.seams).find(|(_, seam)| **seam).unwrap().0;
        let wall = Sel::Face { body: b.name.clone(), index: faces[0] as usize, point: Vec3::new(10.0, 0.0, 10.0) };
        let wall_pieces = grouped_selection(&app.session, &wall);
        let result = app.session.measure_items(&wall_pieces);
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        assert!((result["items"][0]["area_mm2"].as_f64().unwrap() - 400.0 * std::f64::consts::PI).abs() < 10.0, "{result}");
        let mut d = crate::dialogs::Dialog::for_command(&app, "inspect.measure").unwrap();
        d.pick(&app.session, wall.clone());
        assert_eq!(d.items().len(), 2, "a rim and a wall are two measurement targets");
        d.pick(&app.session, Sel::Face { body: b.name.clone(), index: faces[1] as usize, point: Vec3::new(-10.0, 0.0, 10.0) });
        assert_eq!(d.items().len(), 1, "the other half toggles the same wall");
    }

    #[test]
    fn colour_analyses_show_a_legend() {
        // #973: Draft, Accessibility and Curvature Map say what their colours mean; Zebra and no
        // analysis draw no legend.
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("solid.box", json!({"length":20,"width":20,"height":20})).unwrap();
        let legend = |h: &mut crate::scenario::Harness| {
            h.frames(2);
            crate::scenario::handle_at("analysis:legend").is_some()
        };
        assert!(!legend(&mut h));
        for (cmd, shown) in [("inspect.draft", true), ("inspect.accessibility", true), ("inspect.curvature_map", true), ("inspect.zebra", false)] {
            h.app.session.analysis = None;
            h.app.run(cmd, json!({})).unwrap();
            assert!(h.app.session.analysis.is_some(), "{cmd}");
            assert_eq!(legend(&mut h), shown, "{cmd}");
        }
        h.app.session.analysis = None;
        assert!(!legend(&mut h));
    }

    #[test]
    fn a_section_leaves_the_origin_planes_and_axes_whole() {
        // #1229: the origin planes and axes are reference geometry; the section plane cuts the
        // model, not them, so they go to the lists the GPU draws without the section clip.
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":20,"width":20,"height":20,"corner":[-10,-10,-10]})).unwrap();
        h.app.run("inspect.section", json!({"plane": "XZ", "offset": 0})).unwrap();
        h.app.ui.show_origin = true;
        h.frames(2);
        assert!(section_plane(&h.app).is_some());
        let sc = build_highlight(&h.app).merged();
        let planes = origin_planes(&h.app).len();
        assert!(planes > 0);
        assert_eq!(sc.guide_trans.len(), planes * 6 * crate::gpu::TRI_SIZE);
        assert!(sc.trans.is_empty(), "no origin plane is drawn where the section clips");
        // Each origin axis: a solid part and dashes beyond, plus each plane's four edges.
        assert!(sc.guide_lines.len() >= (planes * 4 + 3) * crate::gpu::LINE_SIZE);
    }

    #[test]
    fn circular_rim_hover_click_and_dialog_pick_include_both_halves() {
        let mut app = cylinder_app();
        let st = app.session.world_state();
        let b = &st.bodies[0];
        let m = b.mesh();
        let top = m.edges.iter().position(|e| e.len() > 3 && e.iter().all(|p| (p.z - 20.0).abs() < 1e-6)).unwrap();
        let rim = m.circular_rim(top);
        assert_eq!(rim.len(), 2);
        let line_count: usize = rim.iter().map(|&i| m.edges[i].len() - 1).sum();
        for &i in &rim {
            let mid = polyline_mid(&m.edges[i]);
            app.viewport.hover = Some(Hit::Edge { body: b.name.clone(), index: i, mid });
            assert_eq!(build_highlight(&app).merged().lines.len(), 2 * line_count * crate::gpu::LINE_SIZE);
            let item = hit_sel(app.viewport.hover.as_ref().unwrap()).unwrap();
            select(&mut app, Some(item.clone()), false);
            assert_eq!(app.session.selection.len(), 2);
            // Selecting both pieces must not draw each highlight twice.
            assert_eq!(build_highlight(&app).merged().lines.len(), 2 * line_count * crate::gpu::LINE_SIZE);
            // Ctrl/Shift clicking either half deselects the entire rim.
            select(&mut app, Some(item.clone()), true);
            assert!(app.session.selection.is_empty());
            let mut d = crate::dialogs::Dialog::for_command(&app, "solid.chamfer").unwrap();
            if let crate::dialogs::Kind::Fillet { chain, .. } = &mut d.kind {
                *chain = false;
            }
            d.pick(&app.session, item.clone());
            assert_eq!(d.items().len(), 2);
            d.pick(&app.session, item);
            assert!(d.items().is_empty());
        }
        // Screen-space picking works from either side of the rim as the model turns.
        for &i in &rim {
            let mid = polyline_mid(&m.edges[i]);
            app.cam = app.cam.looking_from(Vec3::new(mid.x, mid.y, 20.0));
            app.cam.target = Vec3::new(0.0, 0.0, 10.0);
            for perspective in [false, true] {
                app.ui.perspective = perspective;
                let proj = projection(&app, Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
                let hits = pick(&app, &proj, proj.to_screen(mid).unwrap());
                let (_, sel) = candidate(&app, &hits).unwrap();
                assert!(matches!(sel, Sel::Edge { .. }), "{hits:?}");
                assert_eq!(grouped_selection(&app.session, &sel).len(), 2);
            }
        }
    }

    #[test]
    fn curved_face_hover_selection_and_dialogs_cross_the_seam() {
        let mut app = cylinder_app();
        let st = app.session.world_state();
        let b = &st.bodies[0];
        let m = b.mesh();
        let faces = m.edge_faces.iter().zip(&m.seams).find(|(_, seam)| **seam).unwrap().0;
        assert_eq!(faces.len(), 2);
        let triangle_count = m.tri_face.iter().filter(|f| faces.contains(f)).count();
        for &f in faces {
            let index = f as usize;
            let t = m.triangles.iter().zip(&m.tri_face).find(|(_, tf)| **tf == f).unwrap().0;
            let [a, c, d] = m.tri(t).unwrap();
            let point = (a + c + d) / 3.0;
            app.viewport.hover = Some(Hit::Face { body: b.name.clone(), index, point });
            assert_eq!(build_highlight(&app).merged().tris.len(), triangle_count * 3 * crate::gpu::TRI_SIZE);
            let item = hit_sel(app.viewport.hover.as_ref().unwrap()).unwrap();
            select(&mut app, Some(item.clone()), false);
            assert_eq!(app.session.selection.len(), 2);
            assert_eq!(build_highlight(&app).merged().tris.len(), triangle_count * 3 * crate::gpu::TRI_SIZE);
            assert_eq!(app.session.selection.first(), Some(&item), "keep the original tool placement point first");
            let mut dialog = crate::dialogs::Dialog::for_command(&app, "solid.chamfer").unwrap();
            assert_eq!(dialog.items().len(), 2);
            dialog.pick(&app.session, item.clone());
            assert!(dialog.items().is_empty());
            dialog.pick(&app.session, item);
            assert_eq!(dialog.items().len(), 2);
            // A point elsewhere on the opposite half still toggles the whole surface off.
            let other = faces.iter().find(|&&x| x != f).unwrap();
            select(&mut app, Some(Sel::Face { body: b.name.clone(), index: *other as usize, point: m.positions[0] }), true);
            assert!(app.session.selection.is_empty());
        }
        // Planar caps and box corners stay separate.
        let cap = (0..b.body.face_count()).find(|&i| m.surface_patch(i).len() == 1).unwrap();
        assert_eq!(m.surface_patch(cap), vec![cap]);
        app.session.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
        let st = app.session.world_state();
        let box_body = st.bodies.last().unwrap();
        for i in 0..box_body.body.face_count() {
            assert_eq!(box_body.mesh().surface_patch(i), vec![i]);
        }
    }

    #[test]
    fn immediate_wheel_zoom_applies_normalized_input_once() {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.smooth_wheel_zoom = false;
        h.app.preferences.trackpad_pan = false;
        h.app.preferences.zoom_to_cursor = false;
        h.call("ui.move", json!({"x":800,"y":500}));
        let before = h.app.cam;
        let line = h.ctx.options(|o| o.input_options.line_scroll_speed);
        for (unit, amount, points) in
            [(egui::MouseWheelUnit::Point, 120.0, 120.0), (egui::MouseWheelUnit::Line, 1.0, line), (egui::MouseWheelUnit::Page, 0.1, 100.0)]
        {
            let height = h.app.cam.half_height();
            h.app.synthetic.push(egui::Event::MouseWheel {
                unit,
                delta: vec2(0.0, amount),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            });
            h.frame();
            let expected = height * (-f64::from(points) * 0.0015).exp();
            assert!((h.app.cam.half_height() - expected).abs() < 1e-6);
            let after = h.app.cam;
            h.frames(30);
            assert_eq!(h.app.cam, after, "idle frames must not replay wheel input");
        }
        assert_eq!(h.app.cam.target, before.target);
        h.app.preferences.zoom_reverse = true;
        let height = h.app.cam.half_height();
        h.call("ui.scroll", json!({"x":800,"y":500,"delta":120}));
        assert!((h.app.cam.half_height() - height * 0.18_f64.exp()).abs() < 1e-6);
    }

    #[test]
    fn wheel_zoom_setting_changes_do_not_replay_smoothing() {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.zoom_to_cursor = false;
        h.app.preferences.trackpad_pan = false;
        h.call("ui.move", json!({"x":800,"y":500}));
        h.app.synthetic.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 120.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        h.frame();
        assert!(h.ctx.input(|i| i.is_scrolling()));
        h.app.preferences.smooth_wheel_zoom = false;
        let stopped = h.app.cam;
        h.frame();
        assert_eq!(h.app.cam, stopped);
        h.app.preferences.smooth_wheel_zoom = true;
        h.frames(90);
        assert_eq!(h.app.cam, stopped, "re-enabling must not replay the old smoothing tail");
        assert!(!h.app.viewport.wheel_zoom_raw_until_idle);
        h.app.preferences.smooth_wheel_zoom = false;
        for _ in 0..3 {
            h.app.synthetic.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 120.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            });
            h.frame();
        }
        assert!((h.app.cam.half_height() - stopped.half_height() * (-0.54_f64).exp()).abs() < 1e-6);
        let after = h.app.cam;
        h.frames(90);
        assert_eq!(h.app.cam, after);
    }

    #[test]
    fn trackpad_two_finger_scroll_pans_without_zooming() {
        let mut h = crate::scenario::Harness::new();
        assert!(h.app.preferences.trackpad_pan, "two-finger pan is the default");
        h.app.preferences.zoom_to_cursor = false;
        h.call("ui.move", json!({"x":800,"y":500}));
        let gesture =
            |phase| egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(60.0, -40.0), modifiers: egui::Modifiers::NONE, phase };
        // A bracketed trackpad gesture pans (measured macOS Start…End bracketing).
        let before = h.app.cam;
        h.app.synthetic.push(gesture(egui::TouchPhase::Start));
        h.frame();
        h.app.synthetic.push(gesture(egui::TouchPhase::Move));
        h.frame();
        assert_eq!(h.app.cam.half_height(), before.half_height(), "trackpad scroll must not zoom");
        assert_ne!(h.app.cam.target, before.target, "trackpad scroll must pan");
        h.app.synthetic.push(gesture(egui::TouchPhase::End));
        h.frame();
        let after = h.app.cam;
        h.frames(30);
        assert_eq!(h.app.cam, after, "pan must not drift once input stops");
        // Bare moves are a mouse: vertical zooms…
        h.app.preferences.smooth_wheel_zoom = false;
        h.app.synthetic.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 120.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        h.frame();
        assert!((h.app.cam.half_height() - after.half_height() * (-0.18_f64).exp()).abs() < 1e-6);
        // …and horizontal pans without zooming (thumb wheel).
        let zoomed = h.app.cam;
        h.app.synthetic.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(40.0, 0.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        h.frame();
        assert_eq!(h.app.cam.half_height(), zoomed.half_height());
        assert_ne!(h.app.cam.target, zoomed.target);
        // Opting out restores the legacy wheel behaviour for all point input.
        h.app.preferences.trackpad_pan = false;
        h.app.synthetic.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 120.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        h.frame();
        assert!((h.app.cam.half_height() - zoomed.half_height() * (-0.18_f64).exp()).abs() < 1e-6);
        // Legacy mode also zooms bracketed input instead of panning it.
        let legacy = h.app.cam.half_height();
        let legacy_target = h.app.cam.target;
        for phase in [egui::TouchPhase::Start, egui::TouchPhase::Move, egui::TouchPhase::End] {
            h.app.synthetic.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 120.0),
                modifiers: egui::Modifiers::NONE,
                phase,
            });
            h.frame();
        }
        assert!((h.app.cam.half_height() - legacy * (-0.18_f64).exp()).abs() < 1e-6);
        assert_eq!(h.app.cam.target, legacy_target);
        // The thumb wheel pans sideways even with panning off: it is a mouse.
        let h0 = h.app.cam.half_height();
        h.app.synthetic.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(40.0, 0.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        h.frame();
        assert_eq!(h.app.cam.half_height(), h0);
        assert_ne!(h.app.cam.target, legacy_target);
    }

    #[test]
    fn pinch_gesture_zooms_and_ignores_noise() {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.smooth_wheel_zoom = false;
        h.app.preferences.zoom_to_cursor = false;
        h.call("ui.move", json!({"x":800,"y":500}));
        let height = h.app.cam.half_height();
        let target = h.app.cam.target;
        h.app.synthetic.push(egui::Event::Zoom(1.2));
        h.frame();
        let zoomed = h.app.cam.half_height();
        assert!(((zoomed - height / 1.2) / (height / 1.2)).abs() < 1e-6, "spread must zoom in");
        assert_eq!(h.app.cam.target, target, "centre zoom keeps the target");
        h.app.preferences.zoom_reverse = true;
        let height = h.app.cam.half_height();
        h.app.synthetic.push(egui::Event::Zoom(1.2));
        h.frame();
        let flipped = h.app.cam.half_height();
        assert!(((flipped - height * 1.2) / (height * 1.2)).abs() < 1e-6, "reverse flips the pinch direction");
        h.app.synthetic.push(egui::Event::Zoom(f32::NAN));
        h.app.synthetic.push(egui::Event::Zoom(0.0));
        h.app.synthetic.push(egui::Event::Zoom(1.0));
        let stopped = h.app.cam;
        h.frame();
        assert_eq!(h.app.cam, stopped, "noise must leave the camera still");
    }

    #[test]
    fn trackpad_gestures_over_dialogs_do_not_move_the_view() {
        let ctx = egui::Context::default();
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        assert!(app.preferences.trackpad_pan);
        crate::help::run(&mut app, &ctx, "shortcuts");
        let mut time = 0.0;
        let pan =
            |phase| egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(40.0, -20.0), modifiers: egui::Modifiers::NONE, phase };
        let mut frame = |app: &mut SolveApp, pointer: Option<Pos2>, events: Vec<egui::Event>| {
            time += 1.0 / 60.0;
            let mut input = events;
            if let Some(p) = pointer {
                input.insert(0, egui::Event::PointerMoved(p));
            }
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 800.0))),
                    time: Some(time),
                    events: input,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| show(app, ui));
                    crate::prefs::show(app, ui.ctx());
                },
            );
            output.textures_delta.clear();
        };
        for _ in 0..4 {
            frame(&mut app, None, Vec::new());
        }
        let window = ctx.memory(|m| m.area_rect(egui::Id::new("sc_prefs"))).unwrap();
        let over = window.center();
        let height = app.cam.half_height();
        let target = app.cam.target;
        // A full gesture plus pinch over the dialog stays in the popup.
        for events in
            [vec![pan(egui::TouchPhase::Start)], vec![pan(egui::TouchPhase::Move), egui::Event::Zoom(1.2)], vec![pan(egui::TouchPhase::End)]]
        {
            frame(&mut app, Some(over), events);
            assert_eq!(app.cam.half_height(), height, "trackpad input over a dialog must stay in the popup");
            assert_eq!(app.cam.target, target);
        }
        let exposed = pos2(40.0, 700.0);
        assert!(!window.contains(exposed));
        frame(&mut app, Some(exposed), Vec::new());
        frame(&mut app, Some(exposed), vec![pan(egui::TouchPhase::Start)]);
        frame(&mut app, Some(exposed), vec![pan(egui::TouchPhase::Move)]);
        assert_eq!(app.cam.half_height(), height);
        assert_ne!(app.cam.target, target, "the exposed viewport still pans");
        frame(&mut app, Some(exposed), vec![egui::Event::Zoom(1.2)]);
        assert_ne!(app.cam.half_height(), height, "the exposed viewport still pinch-zooms");
        frame(&mut app, Some(exposed), vec![pan(egui::TouchPhase::End)]);
    }

    #[test]
    fn shortcuts_window_wheel_does_not_zoom_the_viewport() {
        for smooth in [true, false] {
            let ctx = egui::Context::default();
            let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
            app.preferences.smooth_wheel_zoom = smooth;
            app.preferences.trackpad_pan = false;
            crate::help::run(&mut app, &ctx, "shortcuts");
            let mut time = 0.0;
            let mut frame = |app: &mut SolveApp, pointer: Option<Pos2>, wheel: f32| {
                time += 1.0 / 60.0;
                let mut events = Vec::new();
                if let Some(p) = pointer {
                    events.push(egui::Event::PointerMoved(p));
                }
                if wheel != 0.0 {
                    events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: vec2(0.0, wheel),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                    });
                }
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 800.0))),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| show(app, ui));
                        crate::prefs::show(app, ui.ctx());
                    },
                );
                output.textures_delta.clear();
            };
            // Let egui establish the floating window's size and hit-test widgets.
            for _ in 0..4 {
                frame(&mut app, None, 0.0);
            }
            let window = ctx.memory(|m| m.area_rect(egui::Id::new("sc_prefs"))).unwrap();
            let list = window.center() + vec2(140.0, 0.0);
            let height = app.cam.half_height();
            let target = app.cam.target;
            frame(&mut app, Some(list), 0.0);
            for n in 0..12 {
                frame(&mut app, Some(list), if n == 0 { -120.0 } else { 0.0 });
                assert_eq!(app.cam.half_height(), height, "wheel input over shortcuts must stay in the popup");
                assert_eq!(app.cam.target, target);
            }
            // The window's non-scrolling sidebar must also shield the model.
            frame(&mut app, Some(window.left_center() + vec2(30.0, 0.0)), -120.0);
            assert_eq!(app.cam.half_height(), height);
            // An exposed part of the viewport still accepts wheel navigation with the popup open.
            let exposed = pos2(40.0, 700.0);
            assert!(!window.contains(exposed));
            frame(&mut app, Some(exposed), 0.0);
            frame(&mut app, Some(exposed), -120.0);
            assert_ne!(app.cam.half_height(), height);
            // Closing the popup returns its former area to the viewport.
            app.prefs_window.open = false;
            for _ in 0..12 {
                frame(&mut app, Some(list), 0.0);
            }
            let height = app.cam.half_height();
            frame(&mut app, Some(list), -120.0);
            assert_ne!(app.cam.half_height(), height);
        }
    }

    #[test]
    fn wheel_by_a_dialog_list_or_panel_edge_leaves_the_model_alone() {
        // #678: scrolling the Physical Material list, a drop-down that hangs over the model,
        // zoomed the model too. egui's hit test counts the model as under the pointer within a
        // few points of a window's, a list's or the viewport's own edge, so near those edges
        // the same wheel turn reached both.
        use crate::scenario::{Harness, handle_at};
        // One wheel turn at `p`, then frames until egui's smoothing is spent: did the camera move?
        let wheel = |h: &mut Harness, p: Pos2, delta: f32| {
            let before = h.app.cam;
            h.call("ui.scroll", json!({"x": p.x, "y": p.y, "delta": delta}));
            h.frames(20);
            h.app.cam != before
        };
        // A trackpad pinch, then a two-finger scroll (which pans the model): did the camera move?
        let gestures = |h: &mut Harness, p: Pos2| {
            let before = h.app.cam;
            h.call("ui.move", json!({"x": p.x, "y": p.y}));
            h.app.synthetic.push(egui::Event::Zoom(1.2));
            for phase in [egui::TouchPhase::Start, egui::TouchPhase::Move, egui::TouchPhase::End] {
                let unit = egui::MouseWheelUnit::Point;
                h.app.synthetic.push(egui::Event::MouseWheel { unit, delta: vec2(30.0, -20.0), modifiers: egui::Modifiers::NONE, phase });
            }
            h.settle();
            h.frames(20);
            h.app.cam != before
        };
        for smooth in [true, false] {
            let mut h = Harness::new();
            h.app.preferences.smooth_wheel_zoom = smooth;
            h.app.preferences.zoom_to_cursor = false;
            h.call("engine.execute", json!({"command": "solid.box", "params": {"length": 40, "width": 30, "height": 20}}));
            h.call("ui.start", json!({"command": "material.assign"}));
            let field = handle_at("dialog:Material").expect("the dialog's material field");
            let field = json!({"x": field.x, "y": field.y});
            h.call("ui.move", field.clone());
            h.call("ui.click", field.clone());
            let item = "dialog:Material:Steel";
            let first = handle_at(item).expect("the open material list");
            let list = h
                .ctx
                .memory(|m| {
                    let popups = m.areas().visible_layer_ids().into_iter().filter(|l| l.order == egui::Order::Foreground);
                    popups.filter_map(|l| m.area_rect(l.id)).find(|r| r.contains(first))
                })
                .expect("the list's popup");
            let dialog = h.app.viewport.dialog_rect.expect("the dialog");
            let vp = h.app.viewport.rect.expect("the viewport");
            assert!(!dialog.contains(first) && vp.contains(first), "the list must hang over the model: {first:?} {dialog:?}");

            // The wheel scrolls the list and only the list, in its middle and by its edges.
            assert!(!wheel(&mut h, pos2(list.center().x, first.y), -60.0), "smooth {smooth}: the wheel over the list zoomed the model");
            let moved = handle_at(item).expect("the list stays open");
            assert!(moved.y < first.y - 1.0, "the wheel must scroll the list: {first:?} -> {moved:?}");
            for p in [pos2(list.left() + 2.0, list.center().y), pos2(list.right() - 2.0, list.center().y), pos2(list.center().x, list.bottom() - 2.0)]
            {
                assert!(!wheel(&mut h, p, 60.0), "smooth {smooth}: the wheel at {p:?} by the list's edge ({list:?}) zoomed the model");
                assert!(!gestures(&mut h, p), "a pinch or two-finger scroll at {p:?} by the list's edge ({list:?}) moved the model");
                assert!(h.app.viewport.mouse.is_none(), "the model must not see the pointer at {p:?} over the list");
            }

            // With the list closed: by the dialog's edges, and over the browser and the timeline
            // next to the model.
            h.call("ui.move", field.clone());
            h.call("ui.click", field);
            assert!(handle_at(item).is_none(), "the list must close");
            let (mid, x) = (vp.center().y, vp.left() + vp.width() * 0.25);
            for p in [
                pos2(dialog.left() + 2.0, dialog.center().y),
                pos2(dialog.center().x, dialog.bottom() - 2.0),
                pos2(vp.left() - 2.0, mid),
                pos2(x, vp.bottom() + 2.0),
            ] {
                assert!(!wheel(&mut h, p, 60.0), "smooth {smooth}: the wheel at {p:?} off the model ({vp:?}, dialog {dialog:?}) zoomed it");
                assert!(h.app.viewport.mouse.is_none(), "the model must not see the pointer at {p:?}");
            }
            // The model itself still zooms right next to them.
            for p in [pos2(dialog.left() - 2.0, dialog.center().y), pos2(vp.left() + 2.0, mid), pos2(x, vp.bottom() - 2.0)] {
                assert!(wheel(&mut h, p, 60.0), "smooth {smooth}: the wheel at {p:?} on the model ({vp:?}) must zoom it");
                assert_eq!(h.app.viewport.mouse, Some(p));
                assert!(gestures(&mut h, p), "a pinch and a two-finger scroll at {p:?} on the model must move it");
            }
        }
    }

    #[test]
    fn floating_dialog_blocks_viewport_hover_and_clicks() {
        let mut h = crate::scenario::Harness::new();
        h.call("engine.execute", json!({"command": "solid.box", "params": {"length": 40, "width": 30, "height": 20}}));
        h.call("ui.view", json!({"view": "fit"}));
        h.app.ui.pick_bodies = true;
        let p = h.app.viewport.rect.unwrap().center();
        h.call("ui.move", json!({"x": p.x, "y": p.y}));
        assert!(h.app.viewport.hover.is_some(), "the test point must hover over the model");
        h.call("ui.click", json!({"x": p.x, "y": p.y}));
        assert!(!h.app.session.selection.is_empty(), "the uncovered model must be selectable");
        h.app.session.selection.clear();

        h.call("ui.prefs", json!({"open": "Navigation"}));
        let window = h.ctx.memory(|m| m.area_rect(egui::Id::new("sc_prefs"))).unwrap();
        assert!(window.contains(p), "the dialog must cover the model at the test point");
        h.call("ui.move", json!({"x": p.x, "y": p.y}));
        assert!(h.app.viewport.hover.is_none(), "the dialog must block model highlights");
        assert!(h.app.viewport.mouse.is_none());
        h.call("ui.click", json!({"x": p.x, "y": p.y}));
        assert!(h.app.session.selection.is_empty(), "clicks in the dialog must not select the model");

        h.call("ui.prefs", json!({"button": "cancel"}));
        h.call("ui.move", json!({"x": p.x, "y": p.y}));
        assert!(h.app.viewport.hover.is_some(), "closing the dialog must restore model highlights");
        h.call("ui.click", json!({"x": p.x, "y": p.y}));
        assert!(!h.app.session.selection.is_empty());
    }

    #[test]
    fn floating_dialog_blocks_axis_view_highlights_and_clicks() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.resize", json!({"width": 800, "height": 700}));
        let p = crate::scenario::handle_at("axis_view:z").unwrap();
        h.call("ui.move", json!({"x": p.x, "y": p.y}));
        assert_eq!(crate::scenario::count_of("axis_view_highlights"), Some(1.0));
        h.call("ui.prefs", json!({"open": "Navigation"}));
        let window = h.ctx.memory(|m| m.area_rect(egui::Id::new("sc_prefs"))).unwrap();
        assert!(window.contains(p), "the floating dialog must cover the axis target");
        let camera = serde_json::to_value(h.app.cam).unwrap();
        h.call("ui.move", json!({"x": p.x, "y": p.y}));
        assert_eq!(crate::scenario::count_of("axis_view_highlights"), Some(0.0));
        h.call("ui.click", json!({"x": p.x, "y": p.y}));
        assert_eq!(serde_json::to_value(h.app.cam).unwrap(), camera, "dialog clicks must not turn the axis view");
        h.call("ui.drag", json!({"x0": p.x, "y0": p.y, "x1": p.x + 20.0, "y1": p.y, "steps": 8}));
        assert_eq!(serde_json::to_value(h.app.cam).unwrap(), camera, "dialog drags must not orbit the axis gizmo");
        assert!(h.app.viewport.axis_drag.is_none());
        h.call("ui.prefs", json!({"button": "cancel"}));
        h.call("ui.move", json!({"x": p.x, "y": p.y}));
        assert_eq!(crate::scenario::count_of("axis_view_highlights"), Some(1.0));
        h.call("ui.click", json!({"x": p.x, "y": p.y}));
        assert_ne!(serde_json::to_value(h.app.cam).unwrap(), camera);
    }

    #[test]
    fn view_axis_drag_owns_navigation_and_keeps_the_view_centre() {
        for nav in [None, Some(NavMode::Orbit), Some(NavMode::Pan)] {
            let mut h = crate::scenario::Harness::new();
            h.call("ui.view", json!({"view": "front"}));
            h.app.viewport.nav = nav;
            let before = h.app.cam;
            let p = crate::scenario::handle_at("axis_view:-y").unwrap();
            h.call("ui.drag", json!({"x0": p.x, "y0": p.y, "x1": p.x + 48.0, "y1": p.y, "steps": 8}));
            assert!((h.app.cam.yaw - 48.0 * 0.008).abs() < 1e-6, "{nav:?}: axis drag must orbit exactly once");
            assert_eq!(h.app.cam.target, before.target, "{nav:?}: axis drag must not pan");
            assert_eq!(h.app.cam.distance, before.distance);
            assert_eq!(h.app.cam.pitch, before.pitch);
            assert!(h.app.viewport.axis_drag.is_none(), "release clears the axis gesture");
            assert!(h.app.viewport.boxsel.is_none());
        }
    }

    #[test]
    fn section_outlines_follow_the_cut() {
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let st = s.model.state();
        let m = st.bodies[0].mesh();
        let (o, n) = (Vec3::new(0.0, 15.0, 0.0), Vec3::new(0.0, -1.0, 0.0));
        let segs = solidcraft_engine::view::section_segments(&m, o, n);
        assert!(segs.len() >= 4, "{segs:?}");
        // On the plane (a hair to the kept side) and around the 40 x 20 cut.
        let mut len = 0.0;
        for (a, b) in &segs {
            assert!((a.y - 15.0).abs() < 0.01 && (b.y - 15.0).abs() < 0.01);
            len += a.dist(*b);
        }
        assert!((len - 120.0).abs() < 0.5, "outline length {len}");
        // A plane that misses the body cuts nothing.
        assert!(solidcraft_engine::view::section_segments(&m, Vec3::new(0.0, 99.0, 0.0), n).is_empty());
    }

    #[test]
    fn selection_shows_where_the_model_hides_it() {
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        assert!(build_highlight(&app).merged().hidden.is_empty());
        // The back face: its outline is drawn (dimmed) where the box hides it.
        app.session.selection = vec![Sel::Face { body: "Body1".into(), index: 3, point: Vec3::new(20.0, 30.0, 10.0) }];
        assert!(!build_highlight(&app).merged().hidden.is_empty());
    }

    #[test]
    fn axis_view_top_bottom_choose_nearest_square_heading() {
        for (from, to) in [
            (0.0, 0.0),
            (30.0, 0.0),
            (-80.0, -90.0),
            (100.0, 90.0),
            (170.0, 180.0),
            (400.0, 360.0),
            (-400.0, -360.0),
            (-45.0, 0.0),
            (45.0, 0.0),
            (135.0, 90.0),
            (-135.0, -90.0),
            (44.99, 0.0),
            (45.01, 90.0),
            (-44.99, 0.0),
            (-45.01, -90.0),
        ] {
            let got = nearest_quarter_turn(f64::to_radians(from)).to_degrees();
            assert!((got - to).abs() < 1e-9, "{from}° → {got}°, expected {to}°");
        }
        assert_eq!(nearest_quarter_turn(f64::NAN), 0.0);
        assert_eq!(nearest_quarter_turn(f64::INFINITY), 0.0);
    }

    #[test]
    fn shift_orbit_accumulates_and_returns_to_free_rotation() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.view", json!({"view":"front"}));
        let rect = h.app.viewport.rect.unwrap();
        let before = h.app.cam;
        for _ in 0..100 {
            orbit_drag(&mut h.app, rect, 2.0, 0.0, None, true, false);
            step_orbit_snap(&mut h.app, rect, 1.0 / 60.0);
        }
        for _ in 0..120 {
            step_orbit_snap(&mut h.app, rect, 1.0 / 60.0);
        }
        assert!((h.app.cam.yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(h.app.cam.target, before.target);
        assert_eq!(h.app.cam.distance, before.distance);
        orbit_drag(&mut h.app, rect, 0.0, 0.0, None, false, false);
        h.settle();
        assert!((h.app.cam.yaw - 1.6).abs() < 1e-6);
        assert!(h.app.viewport.orbit_free.is_none());
    }

    #[test]
    fn axis_snap_selects_all_directions_in_both_up_frames() {
        assert_eq!(snap_directions().count(), 26);
        for up in [solidcraft_engine::render::UpAxis::Z, solidcraft_engine::render::UpAxis::Y] {
            for d in snap_directions() {
                let mut cam = Camera { up, ..Camera::default() };
                cam = cam.looking_from(cam.from_frame(d + Vec3::new(0.05, 0.03, 0.02)));
                let to = snapped_axis_camera(cam);
                assert!(to.back().dist(cam.from_frame(d)) < 1e-6);
                assert_eq!(to.target, cam.target);
                assert_eq!(to.distance, cam.distance);
                assert_eq!(to.fov, cam.fov);
            }
        }
    }

    #[test]
    fn camera_resets_cancel_pending_axis_motion() {
        for reset in 0..3 {
            let mut h = crate::scenario::Harness::new();
            h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
            let rect = h.app.viewport.rect.unwrap();
            orbit_drag(&mut h.app, rect, 120.0, 40.0, None, true, false);
            assert!(h.app.viewport.orbit_snap.is_some());
            match reset {
                0 => h.app.fit_view(),
                1 => h.app.set_up_axis(solidcraft_engine::render::UpAxis::Y),
                _ => crate::documents::new_design(&mut h.app),
            }
            let before = h.app.cam;
            step_orbit_snap(&mut h.app, rect, 1.0 / 60.0);
            assert_eq!(h.app.cam.yaw, before.yaw);
            assert_eq!(h.app.cam.pitch, before.pitch);
            assert_eq!(h.app.cam.target, before.target);
            assert!(h.app.viewport.orbit_snap.is_none());
            assert!(h.app.viewport.orbit_free.is_none());
        }
    }

    #[test]
    fn axis_snap_spring_eases_in_and_settles_at_each_frame_rate() {
        for fps in [30, 60, 120] {
            let mut h = crate::scenario::Harness::new();
            h.call("ui.view", json!({"view":"front"}));
            let rect = h.app.viewport.rect.unwrap();
            orbit_drag(&mut h.app, rect, 120.0, 0.0, None, true, false);
            assert_eq!(h.app.cam.yaw, 0.0, "choosing a diagonal must not jump the camera");
            step_orbit_snap(&mut h.app, rect, 1.0 / f64::from(fps));
            assert!(h.app.cam.yaw > 0.0 && h.app.cam.yaw < std::f64::consts::FRAC_PI_4 * 0.25);
            for _ in 0..fps * 2 {
                step_orbit_snap(&mut h.app, rect, 1.0 / f64::from(fps));
                assert!(h.app.cam.yaw <= std::f64::consts::FRAC_PI_4 + 0.03, "elastic finish stays small");
            }
            assert!((h.app.cam.yaw - std::f64::consts::FRAC_PI_4).abs() < 1e-6);
            assert!(h.app.viewport.orbit_snap.is_none());
        }
    }

    /// The origin pivot holds through a Shift drag that snaps to an axis: the spring eases the
    /// view round without letting the origin slide on screen.
    #[test]
    fn snapping_keeps_the_origin_still() {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.orbit_around = crate::prefs::OrbitAround::Origin;
        h.app.run("solid.box", json!({"length":40,"width":40,"height":40})).unwrap();
        h.app.fit_view();
        let rect = h.app.viewport.rect.unwrap();
        let at = rect.center() + vec2(50.0, -20.0);
        orbit_drag(&mut h.app, rect, 0.0, 0.0, Some(at), true, true);
        assert_eq!(
            h.app.viewport.orbit_snap.as_ref().and_then(|s| s.pivot).map(|(p, _)| p),
            Some(Vec3::ZERO),
            "the origin is the pivot while snapping"
        );
        let before = projection(&h.app, rect).to_screen(Vec3::ZERO).unwrap();
        for _ in 0..30 {
            orbit_drag(&mut h.app, rect, 4.0, 2.0, Some(at), true, true);
            step_orbit_snap(&mut h.app, rect, 1.0 / 60.0);
            let after = projection(&h.app, rect).to_screen(Vec3::ZERO).unwrap();
            assert!(after.distance(before) < 2.0, "{before:?} -> {after:?}");
        }
    }

    #[test]
    fn shift_orbit_keeps_cursor_pivot_and_default_middle_pan() {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.orbit_around = crate::prefs::OrbitAround::Cursor;
        h.app.run("solid.box", json!({"length":40,"width":40,"height":40})).unwrap();
        h.app.fit_view();
        let rect = h.app.viewport.rect.unwrap();
        let at = rect.center() + vec2(50.0, -20.0);
        orbit_drag(&mut h.app, rect, 0.0, 0.0, Some(at), false, true);
        let pivot = h.app.viewport.orbit_pivot.unwrap();
        let before = projection(&h.app, rect).to_screen(pivot).unwrap();
        for _ in 0..20 {
            orbit_drag(&mut h.app, rect, 4.0, 2.0, Some(at), true, true);
            step_orbit_snap(&mut h.app, rect, 1.0 / 60.0);
            let after = projection(&h.app, rect).to_screen(pivot).unwrap();
            assert!(after.distance(before) < 2.0);
        }
        h.app.viewport.orbit_free = None;
        h.app.viewport.orbit_pivot = None;
        h.call("ui.view", json!({"view":"front"}));
        let before = h.app.cam;
        h.call("ui.drag", json!({"x0":800,"y0":500,"x1":1020,"y1":500,"button":"middle","shift":true,"steps":40}));
        assert!((h.app.cam.yaw - 1.76).abs() < 1e-6, "before={before:?}, after={:?}", h.app.cam);
        assert!(h.app.viewport.orbit_snap.is_none());
        h.call("ui.view", json!({"view":"front"}));
        h.call("ui.drag", json!({"x0":800,"y0":500,"x1":850,"y1":520,"button":"middle"}));
        assert_eq!(h.app.cam.yaw, before.yaw);
        assert_eq!(h.app.cam.pitch, before.pitch);
        assert_ne!(h.app.cam.target, before.target);
    }

    #[test]
    fn preset_shift_middle_orbit_is_free_but_shift_right_orbit_snaps() {
        for preset in ["fusion", "inventor"] {
            let mut h = crate::scenario::Harness::new();
            h.app.preferences.nav = preset.into();
            h.call("ui.view", json!({"view":"front"}));
            let before = h.app.cam;
            h.call("ui.drag", json!({"x0":800,"y0":500,"x1":850,"y1":520,"button":"middle","shift":true,"steps":20}));
            let mut expected = before;
            expected.orbit(50.0, 20.0);
            assert!((h.app.cam.yaw - expected.yaw).abs() < 1e-6, "{preset}: {:?}", h.app.cam);
            assert!((h.app.cam.pitch - expected.pitch).abs() < 1e-6, "{preset}: {:?}", h.app.cam);
            assert_eq!(h.app.cam.target, before.target);
            assert_eq!(h.app.cam.distance, before.distance);
            assert!(h.app.viewport.orbit_snap.is_none());
            h.call("ui.view", json!({"view":"front"}));
            h.call("ui.drag", json!({"x0":800,"y0":500,"x1":1020,"y1":500,"button":"right","shift":true,"steps":40}));
            assert!((h.app.cam.yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-6, "{preset}: {:?}", h.app.cam);
        }
    }

    #[test]
    fn navigation_cube_repeated_face_click_keeps_view_and_drag_snaps() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.prefs", json!({"navigator_style":"cube"}));
        h.run(&json!([
            {"call":"ui.view","params":{"view":"home"}},
            {"click":{"handle":"axis_view:z"}},
            {"expect":{"camera":{"pitch_deg":90,"yaw_deg":0}}},
            {"click":{"handle":"axis_view:z"}},
            {"expect":{"camera":{"pitch_deg":90,"yaw_deg":0}}},
            {"call":"ui.view","params":{"view":"front"}}
        ]))
        .unwrap();
        let c = axis_center(&h.app, h.app.viewport.rect.unwrap());
        h.call("ui.drag", json!({"x0":c.x,"y0":c.y,"x1":c.x+220.0,"y1":c.y,"shift":true,"steps":40}));
        assert!((h.app.cam.yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-6);
        assert!(h.app.viewport.axis_drag.is_none());
        assert!(h.app.viewport.orbit_free.is_none());
        assert!(h.app.viewport.boxsel.is_none());
    }

    #[test]
    fn construction_axes_span_the_model_and_points_show() {
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        s.execute("construct.axis.two_points", &json!({"a": {"vertex": [0, 0, 20]}, "b": {"vertex": [40, 30, 20]}})).unwrap();
        s.execute("construct.point.vertex", &json!({"point": {"vertex": [40, 0, 20]}})).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        let axes = construction_axis_segments(&app);
        assert_eq!(axes.len(), 1);
        let (name, a, b) = axes[0].clone();
        // The segment runs through both points and past them.
        let d = (b - a).normalized().unwrap();
        for p in [Vec3::new(0.0, 0.0, 20.0), Vec3::new(40.0, 30.0, 20.0)] {
            let t = (p - a).dot(d);
            assert!(t > 0.0 && t < a.dist(b) && (a + d * t).dist(p) < 1e-6, "{p:?} not on {a:?}..{b:?}");
        }
        assert_eq!(construction_points(&app).len(), 1);
        // It is an axis for dialogs (pattern directions, revolve axes), and hiding it hides it.
        let (o, dir) = crate::dialogs::axis_of(&app, &Sel::Axis { name: name.clone() }).unwrap();
        assert!(o.dist(Vec3::new(0.0, 0.0, 20.0)) < 1e-6 && dir.dist(d) < 1e-6);
        app.session.visibility.hidden_origin.push(name);
        assert!(construction_axis_segments(&app).is_empty());
    }

    /// Draft Analysis on the GPU takes its colours from the tokens: the sample design through the
    /// viewport's real pipeline, offscreen (skipped without a GPU). In Off the faces that pass are
    /// green; in Protan they are the red-green table's blue, and the negative faces are striped.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn draft_analysis_on_the_gpu_draws_the_modes_colours_and_stripes() {
        use crate::theme::ColorVision;
        let mut s = solidcraft_engine::Session::default();
        s.run_script(&solidcraft_engine::sample::script()).unwrap();
        let mut app = SolveApp::new(s, crate::Services::default());
        app.fit_view();
        app.run("inspect.draft", json!({"pull": [-1, -1, 0]})).unwrap();
        app.viewport.gpu = Some(GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess: false });
        const SIZE: u32 = 256;
        let proj = projection(&app, Rect::from_min_size(Pos2::ZERO, vec2(SIZE as f32, SIZE as f32)));
        let e = proj.cam.eye().to_f32();
        let eye = [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }];
        let render = |c: &crate::theme::AnalysisColors| {
            crate::gpu::render_offscreen(
                build_scene(&app),
                None,
                proj.vp.to_f32(),
                proj.cam.back().to_f32(),
                eye,
                SIZE,
                None,
                analysis_uniform(&app),
                crate::gpu::analysis_colors(c),
            )
        };
        let off = Tokens::themed(false, ColorVision::Off).analysis;
        let protan = Tokens::themed(false, ColorVision::Protan).analysis;
        let flat = crate::theme::AnalysisColors { hatch: false, ..protan };
        let (Some(a), Some(b), Some(c)) = (render(&off), render(&protan), render(&flat)) else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // Pixels of a colour under any light: the same hue, so the same ratios between channels.
        let like = |img: &[u8], c: [f32; 3]| {
            img.as_chunks::<4>()
                .0
                .iter()
                .filter(|p| {
                    let m = c.iter().copied().fold(0.0, f32::max);
                    let k = f32::from(p[0].max(p[1]).max(p[2])) / 255.0 / m;
                    k > 0.3 && (0..3).all(|i| (f32::from(p[i]) / 255.0 - c[i] * k).abs() < 0.04)
                })
                .count()
        };
        let n = (SIZE * SIZE) as usize;
        assert!(like(&a, off.draft[0]) > n / 50 && like(&a, protan.draft[0]) == 0, "Off: green passing faces");
        assert!(like(&b, protan.draft[0]) > n / 50 && like(&b, off.draft[0]) == 0, "Protan: blue passing faces");
        // The stripes: with them on, some negative pixels are darker, and nothing else changes.
        let negative = |p: &[u8]| like(p, protan.draft[2]) == 1;
        let changed: Vec<usize> = (0..n).filter(|&i| b[4 * i..4 * i + 3] != c[4 * i..4 * i + 3]).collect();
        let neg = (0..n).filter(|&i| negative(&c[4 * i..4 * i + 4])).count();
        assert!(!changed.is_empty() && changed.len() * 2 < neg, "{} striped of {neg} negative pixels", changed.len());
        assert!(changed.iter().all(|&i| negative(&c[4 * i..4 * i + 4])), "only negative faces are striped");
        assert!(changed.iter().all(|&i| b[4 * i..4 * i + 3].iter().zip(&c[4 * i..4 * i + 3]).all(|(x, y)| x <= y)), "stripes are darker");
    }

    /// The view's picture draws in the colour vision mode: a fixed sketch line in the light theme
    /// takes Deutan's value, and Off's is gone.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_view_picture_draws_in_the_colour_vision_mode() {
        use crate::theme::ColorVision;
        if !crate::gpu::test_picture_renderer() {
            return;
        }
        let mut h = crate::scenario::Harness::new();
        h.app.ui.dark = false;
        h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
        let made = h.app.run("sketch.line", json!({"points": [[-30, -20], [30, 20]]})).unwrap();
        h.app.run("sketch.constraint.fix", json!({"entity": made["created"]["curves"][0]})).unwrap();
        let mut cam = solidcraft_engine::view::home_camera(&h.app.session);
        cam.set_view(solidcraft_engine::render::StandardView::Top);
        h.app.cam = cam;
        let count = |h: &crate::scenario::Harness, c: Color32| {
            let img = crate::control::view_rgba(&h.app, 320, 240).unwrap();
            img.as_chunks::<4>().0.iter().filter(|p| p[..3].iter().zip([c.r(), c.g(), c.b()]).all(|(a, b)| a.abs_diff(b) <= 8)).count()
        };
        let (off, deutan) = (Tokens::light().sketch_locked, Tokens::themed(false, ColorVision::Deutan).sketch_locked);
        let (a, b) = (count(&h, off), count(&h, deutan));
        assert!(a > 20 && b == 0, "Off: {a} fixed-line pixels, {b} in Deutan's colour");
        h.app.preferences.color_vision = ColorVision::Deutan;
        let (a, b) = (count(&h, off), count(&h, deutan));
        assert!(a == 0 && b > 20, "Deutan: {a} fixed-line pixels in Off's colour, {b} in Deutan's");
    }

    /// A mode's table reaches the headless picture's palette: the sketch states and axes it
    /// changes, and nothing else (the light theme in Off is the engine's own colours).
    #[test]
    fn picture_palette_follows_the_colour_vision_mode() {
        use crate::theme::ColorVision;
        let rgb = |c: Color32| Rgb(c.r(), c.g(), c.b());
        let off = scene_palette(false, ColorVision::Off);
        let rg = scene_palette(false, ColorVision::Protan);
        let t = Tokens::themed(false, ColorVision::Protan);
        assert_eq!((rg.sketch_projected, rg.sketch_locked), (rgb(t.sketch_projected), rgb(t.sketch_locked)));
        assert_ne!(rg.sketch_projected, off.sketch_projected, "the red-green table changes projected lines in light");
        assert_eq!((rg.axes[0], rg.axes[1], rg.axes[2]), (rgb(t.triad[0]), rgb(t.triad[1]), off.axes[2]));
        assert_eq!((rg.sketch, rg.background, rg.grid, rg.body), (off.sketch, off.background, off.grid, off.body));
        let by = scene_palette(false, ColorVision::Tritan);
        assert_eq!(by.sketch_locked, rgb(Tokens::themed(false, ColorVision::Tritan).sketch_locked));
        assert_ne!(by.sketch_locked, off.sketch_locked);
        assert_eq!(by.sketch_projected, off.sketch_projected);
    }

    /// The mode is on from the next frame (theme, widgets, caches, `ui.render`), and Cancel puts
    /// it back.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn colour_vision_mode_reaches_the_frame_and_the_view_caches() {
        use crate::theme::ColorVision;
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        let keys = |app: &SolveApp| (scene_key(app), highlight_key(app));
        h.call("ui.prefs", json!({"open": "Graphics"}));
        assert!(h.widget("Colour vision").is_some() && h.widget("Mode").is_some());
        let before = keys(&h.app);
        h.call("ui.prefs", json!({"color_vision": "deutan"}));
        h.frame();
        assert_eq!(crate::theme::vision(), ColorVision::Deutan);
        assert_eq!(Tokens::get(), Tokens::themed(crate::theme::is_dark(), ColorVision::Deutan));
        let after = keys(&h.app);
        assert!(before.0 != after.0 && before.1 != after.1, "both caches are keyed on the mode");
        h.call("ui.prefs", json!({"button": "cancel"}));
        h.frame();
        assert_eq!((h.app.preferences.color_vision, crate::theme::vision()), (ColorVision::Off, ColorVision::Off));
        assert_eq!(keys(&h.app), before);
        // `ui.render` shows the mode too: a fixed sketch line in Deutan's value (in the light
        // theme, where the red-green table changes it; the render follows the theme).
        if !crate::gpu::test_picture_renderer() {
            return;
        }
        h.call("ui.prefs", json!({"theme": "light"}));
        h.frame();
        h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
        let made = h.app.run("sketch.line", json!({"points": [[-30, -20], [30, 20]]})).unwrap();
        h.app.run("sketch.constraint.fix", json!({"entity": made["created"]["curves"][0]})).unwrap();
        h.call("ui.view", json!({"view": "top"}));
        let path = std::env::temp_dir().join(format!("sc_colour_vision_render_{}.png", std::process::id()));
        let near = |h: &mut crate::scenario::Harness, c: Color32| {
            let r = h.call("ui.render", json!({"path": path.to_string_lossy(), "width": 400, "height": 300}));
            assert_eq!(r["ok"], true, "{r}");
            let img = image::open(&path).unwrap().to_rgb8();
            img.pixels().filter(|p| p.0.iter().zip([c.r(), c.g(), c.b()]).all(|(a, b)| a.abs_diff(b) <= 8)).count()
        };
        let (off, deutan) = (Tokens::light().sketch_locked, Tokens::themed(false, ColorVision::Deutan).sketch_locked);
        let (a, b) = (near(&mut h, off), near(&mut h, deutan));
        assert!(a > 20 && b == 0, "Off: {a} fixed-line pixels, {b} in Deutan's colour");
        h.call("ui.prefs", json!({"color_vision": "deutan"}));
        h.frame();
        let (a, b) = (near(&mut h, off), near(&mut h, deutan));
        h.call("ui.prefs", json!({"color_vision": "off"}));
        h.frame();
        let _ = std::fs::remove_file(&path);
        assert!(a == 0 && b > 20, "Deutan: {a} fixed-line pixels in Off's colour, {b} in Deutan's");
    }
}

#[cfg(test)]
#[path = "view_home_tests.rs"]
mod view_home_tests;

#[cfg(test)]
mod grid_plane_tests {
    use super::*;
    use solidcraft_engine::render::{StandardView, UpAxis};

    fn plane(view: StandardView, up: UpAxis) -> (Vec3, Vec3) {
        let (yaw, pitch) = view.angles();
        let cam = Camera { yaw, pitch, up, ..Camera::default() };
        let (u, v) = grid_plane(cam.to_frame(cam.back()), GridPlane::default()).axes();
        (cam.from_frame(u), cam.from_frame(v))
    }

    /// The camera frame's direction toward the eye for a view at `yaw` and `pitch` degrees.
    fn back_at(yaw_deg: f64, pitch_deg: f64) -> Vec3 {
        let cam = Camera { yaw: yaw_deg.to_radians(), pitch: pitch_deg.to_radians(), ..Camera::default() };
        cam.to_frame(cam.back())
    }

    /// #1470: the grid lies on the ground unless the view sees the ground edge-on; then it lies on
    /// the upright origin plane facing the view, Z up or Y up alike.
    #[test]
    fn the_grid_faces_side_views_and_stays_on_the_ground_otherwise() {
        let normal = |(u, v): (Vec3, Vec3)| u.cross(v);
        for up in [UpAxis::Z, UpAxis::Y] {
            for view in [StandardView::Top, StandardView::Bottom, StandardView::Iso] {
                assert_eq!(normal(plane(view, up)).dot(up.dir()).abs(), 1.0, "{view:?} {up:?}: the ground");
            }
            for view in [StandardView::Front, StandardView::Back, StandardView::Right, StandardView::Left] {
                let (yaw, pitch) = view.angles();
                let back = Camera { yaw, pitch, up, ..Camera::default() }.back();
                let n = normal(plane(view, up));
                assert!(n.dot(up.dir()).abs() < 1e-9, "{view:?} {up:?}: an upright plane");
                assert!((n.dot(back).abs() - 1.0).abs() < 1e-9, "{view:?} {up:?}: facing the view");
            }
        }
    }

    /// #1993: the view has to turn clearly off the ground before the grid leaves it, and clearly
    /// away from a wall before it goes back, so a view wobbling at the threshold does not flicker.
    #[test]
    fn the_grid_plane_thresholds_have_hysteresis() {
        // 9°: edge-on enough to enter the wall.
        assert_eq!(grid_plane(back_at(0.0, 9.0), GridPlane::Ground), GridPlane::Xz);
        // 12°: past the entering limit, but not enough to leave a wall the grid is already on.
        assert_eq!(grid_plane(back_at(0.0, 12.0), GridPlane::Ground), GridPlane::Ground);
        assert_eq!(grid_plane(back_at(0.0, 12.0), GridPlane::Xz), GridPlane::Xz);
        // 16°: clearly off the ground, so even a wall goes back.
        assert_eq!(grid_plane(back_at(0.0, 16.0), GridPlane::Xz), GridPlane::Ground);
        // Below the ground the same, and a view straight down stays on the ground.
        assert_eq!(grid_plane(back_at(30.0, -12.0), GridPlane::Yz), GridPlane::Yz);
        assert_eq!(grid_plane(Vec3::new(0.0, 0.0, 1.0), GridPlane::Xz), GridPlane::Ground);
        assert_eq!(grid_plane(Vec3::new(0.0, 0.0, -1.0), GridPlane::Xz), GridPlane::Ground);
    }

    /// #1993: orbiting at a low elevation keeps the wall until a side view is near; the other
    /// wall takes over only there, never at the 45° crossing.
    #[test]
    fn a_low_orbit_switches_walls_only_near_a_side_view() {
        let low = |yaw_deg: f64| back_at(yaw_deg, 3.0);
        // Entering from the ground still picks the wall facing the view.
        assert_eq!(grid_plane(low(44.0), GridPlane::Ground), GridPlane::Xz);
        assert_eq!(grid_plane(low(50.0), GridPlane::Ground), GridPlane::Yz);
        // A wall is kept across the crossing and only changes near the side view it faces.
        assert_eq!(grid_plane(low(44.0), GridPlane::Xz), GridPlane::Xz);
        assert_eq!(grid_plane(low(50.0), GridPlane::Xz), GridPlane::Xz, "no swap at the 45° crossing");
        assert_eq!(grid_plane(low(70.0), GridPlane::Xz), GridPlane::Xz);
        assert_eq!(grid_plane(low(80.0), GridPlane::Xz), GridPlane::Yz, "the Left view's wall takes over near it");
        assert_eq!(grid_plane(low(10.0), GridPlane::Yz), GridPlane::Xz, "the Front view's wall takes over near it");
    }

    /// #1993: a hostile view direction (a design file, a control request) never panics and never
    /// leaves the grid off the ground's axes.
    #[test]
    fn a_hostile_view_direction_stays_on_the_ground() {
        for back in [Vec3::new(f64::NAN, 0.0, 0.0), Vec3::new(0.0, f64::NEG_INFINITY, 0.0), Vec3::new(0.0, 0.0, f64::INFINITY), Vec3::ZERO] {
            assert_eq!(grid_plane(back, GridPlane::Xz), GridPlane::Ground, "{back:?}");
        }
    }

    /// #1993: the viewport keeps the plane it drew, so the next frame's choice is hysteretic.
    #[test]
    fn the_viewport_keeps_the_grid_plane_between_frames() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        app.preferences.grid_follows_view = true;
        app.cam.yaw = 0.0;
        app.cam.pitch = 0.0;
        let _ = build_sketch_scene(&app, &projection(&app, rect));
        assert_eq!(*app.viewport.grid_plane.lock().unwrap(), GridPlane::Xz, "the Front view takes the wall");
        // Wobble past the entering limit: the wall stays (before #1993 it flipped to the ground).
        app.cam.pitch = 0.23;
        let _ = build_sketch_scene(&app, &projection(&app, rect));
        assert_eq!(*app.viewport.grid_plane.lock().unwrap(), GridPlane::Xz, "a wobble does not flicker");
        app.cam.pitch = 0.3;
        let _ = build_sketch_scene(&app, &projection(&app, rect));
        assert_eq!(*app.viewport.grid_plane.lock().unwrap(), GridPlane::Ground, "a clear turn off the ground returns");
    }

    /// A sketch is drawn in a parallel view even when the preference asks for perspective; the
    /// preference comes back when the sketch is finished.
    #[test]
    fn an_open_sketch_is_drawn_orthographic() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        app.ui.perspective = true;
        assert!(projection(&app, rect).cam.fov > 0.0);
        app.run("sketch.create", json!({"plane": "XZ"})).unwrap();
        assert!(!app.perspective_view());
        assert_eq!(projection(&app, rect).cam.fov, 0.0);
        app.finish_sketch();
        assert!(app.ui.perspective, "the preference is left alone");
        assert!(projection(&app, rect).cam.fov > 0.0);
    }

    /// Create Sketch shows the Origin even when the browser hides it, and draws its planes
    /// through the model so a plane inside a body can be seen and picked; closing it restores
    /// the hidden, depth-tested Origin.
    #[test]
    fn create_sketch_shows_the_origin_planes_through_the_model() {
        let mut app = SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
        app.run("solid.box", json!({"length":40,"width":40,"height":40})).unwrap();
        app.ui.show_origin = false;
        assert!(origin_planes(&app).is_empty());
        assert!(build_highlight(&app).xray.is_empty());
        app.start("sketch.create");
        assert!(app.picking_sketch_plane());
        assert_eq!(origin_planes(&app).len(), 3, "every origin plane is offered");
        let sc = build_highlight(&app);
        assert_eq!(sc.xray.len(), 3 * 6 * (6 * 4 + 4), "each plane's two triangles are drawn through the model");
        assert!(sc.guide_trans.is_empty());
        app.dialog = None;
        assert!(origin_planes(&app).is_empty());
        app.ui.show_origin = true;
        let sc = build_highlight(&app);
        assert!(sc.xray.is_empty() && !sc.guide_trans.is_empty(), "outside Create Sketch the planes are depth-tested");
    }
}
