//! What the viewport shows, independent of the UI toolkit: body meshes, edges, sketches, the
//! grid and the origin. Used by the headless snapshot renderer; the GPU viewport draws the same
//! content.

use std::sync::Arc;

use solidcraft_geom::{Aabb3, Plane, Vec3};
use solidcraft_render::{AnalysisColors, Camera, Rgb, Scene, SceneAnalysis, SceneLine, SceneMesh};
use solidcraft_sketch::Sketch;

use crate::{Sel, Session};

/// Colours (shared with the GPU viewport).
pub mod colors {
    use solidcraft_render::Rgb;
    pub const BODY: Rgb = Rgb(176, 186, 198);
    pub const BODY_SELECTED: Rgb = Rgb(110, 170, 235);
    pub const EDGE: Rgb = Rgb(40, 44, 52);
    pub const SKETCH: Rgb = Rgb(30, 90, 200);
    pub const SKETCH_DONE: Rgb = Rgb(20, 20, 20);
    pub const SKETCH_CONSTRUCTION: Rgb = Rgb(230, 130, 40);
    pub const SKETCH_FIXED: Rgb = Rgb(20, 20, 20);
    /// Fixed (locked) geometry.
    pub const SKETCH_LOCKED: Rgb = Rgb(0, 140, 60);
    pub const SKETCH_CENTERLINE: Rgb = Rgb(200, 110, 60);
    /// Linked (projected) reference geometry.
    pub const SKETCH_PROJECTED: Rgb = Rgb(150, 60, 190);
    /// Linked geometry whose reference was lost.
    pub const SKETCH_LOST: Rgb = Rgb(200, 60, 90);
    pub const PROFILE: Rgb = Rgb(255, 196, 120);
    pub const GRID: Rgb = Rgb(196, 202, 212);
    pub const GRID_MAJOR: Rgb = Rgb(170, 178, 190);
    pub const AXIS_X: Rgb = Rgb(220, 60, 60);
    pub const AXIS_Y: Rgb = Rgb(60, 170, 60);
    pub const AXIS_Z: Rgb = Rgb(60, 100, 230);
    pub const BG_TOP: Rgb = Rgb(240, 243, 247);
    pub const BG_BOTTOM: Rgb = Rgb(203, 210, 221);
    /// What the section cap is tinted toward.
    pub const SECTION_CAP: Rgb = solidcraft_render::SECTION_CAP;
}

/// The colours a headless scene is drawn in. [`Palette::DEFAULT`] is [`colors`], the light
/// theme's look, which `solidcraft-cli snapshot`, MCP screenshots and thumbnails keep; the app
/// passes its theme's for `ui.render` and its CPU viewport, so they match the window. Only these
/// defaults change: a body's appearance and a face's own look are the design's, drawn as they are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The background gradient, top and bottom.
    pub background: (Rgb, Rgb),
    pub grid: Rgb,
    pub grid_major: Rgb,
    /// Body edges, and the outline where the section cuts a body.
    pub edge: Rgb,
    /// A body without an appearance of its own.
    pub body: Rgb,
    pub body_selected: Rgb,
    /// Sketch curves: under-constrained, fully constrained (in the active sketch), in a finished
    /// sketch, fixed, projected, with a lost link, construction, centerline.
    pub sketch: Rgb,
    pub sketch_fixed: Rgb,
    pub sketch_done: Rgb,
    pub sketch_locked: Rgb,
    pub sketch_projected: Rgb,
    pub sketch_lost: Rgb,
    pub sketch_construction: Rgb,
    pub sketch_centerline: Rgb,
    /// The origin axes: X, Y, Z.
    pub axes: [Rgb; 3],
    /// What the section cap is tinted toward.
    pub cap: Rgb,
    /// The surface analysis colours.
    pub analysis: AnalysisColors,
}

impl Palette {
    /// [`colors`].
    pub const DEFAULT: Palette = Palette {
        background: (colors::BG_TOP, colors::BG_BOTTOM),
        grid: colors::GRID,
        grid_major: colors::GRID_MAJOR,
        edge: colors::EDGE,
        body: colors::BODY,
        body_selected: colors::BODY_SELECTED,
        sketch: colors::SKETCH,
        sketch_fixed: colors::SKETCH_FIXED,
        sketch_done: colors::SKETCH_DONE,
        sketch_locked: colors::SKETCH_LOCKED,
        sketch_projected: colors::SKETCH_PROJECTED,
        sketch_lost: colors::SKETCH_LOST,
        sketch_construction: colors::SKETCH_CONSTRUCTION,
        sketch_centerline: colors::SKETCH_CENTERLINE,
        axes: [colors::AXIS_X, colors::AXIS_Y, colors::AXIS_Z],
        cap: colors::SECTION_CAP,
        analysis: AnalysisColors::DEFAULT,
    };
}

impl Default for Palette {
    fn default() -> Self {
        Palette::DEFAULT
    }
}

/// Bounds of everything visible now, in world space: shown bodies where their occurrences,
/// joints and moves place them, and the sketches the view draws (not ones used by a feature,
/// hidden ones, or ones in hidden components). What Home, Fit and the orbit pivot frame.
pub fn bounds(s: &Session) -> Aabb3 {
    visible_bounds(s, true)
}

/// [`bounds`], with the browser's Sketches folder shown (`sketches`) or hidden.
pub fn visible_bounds(s: &Session, sketches: bool) -> Aabb3 {
    let st = s.world_state();
    let mut b = Aabb3::EMPTY;
    for body in st.bodies.iter().filter(|x| s.body_visible(&x.name)) {
        b = b.union(&body.mesh().bounds());
    }
    for ss in st.sketches.iter().filter(|x| sketch_drawn(s, x.feature, sketches)) {
        // All four corners: on a tilted plane two opposite ones don't span the rectangle.
        if let Some((lo, hi)) = ss.sketch.bounds() {
            for p in [lo, hi, solidcraft_geom::Vec2::new(lo.x, hi.y), solidcraft_geom::Vec2::new(hi.x, lo.y)] {
                b.add(ss.plane.to_world(p));
            }
        }
        for w in ss.sketch.wires.iter().filter(|w| !w.fit.is_empty() || !ss.sketch.view.hide_projected) {
            for p in &w.pts {
                b.add(*p);
            }
        }
    }
    b
}

/// The origin plane a grid lies on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GridPlane {
    /// The ground: the camera frame's XY.
    #[default]
    Ground,
    /// Upright, facing a view along ±Y (the Front or Back view): the frame's XZ.
    Xz,
    /// Upright, facing a view along ±X (the Left or Right view): the frame's YZ.
    Yz,
}

impl GridPlane {
    /// The plane's two axes, in the camera's Z-up frame.
    pub fn axes(self) -> (Vec3, Vec3) {
        match self {
            GridPlane::Ground => (Vec3::X, Vec3::Y),
            GridPlane::Xz => (Vec3::X, Vec3::Z),
            GridPlane::Yz => (Vec3::Y, Vec3::Z),
        }
    }
}

/// The plane to draw the grid on for a view whose direction toward the eye is `back` (in the
/// camera's Z-up frame), given the plane the last frame drew (`last`).
///
/// The ground (frame XY) unless it is seen nearly edge-on, as in the Front or Right view; then
/// the upright origin plane facing the view, so the grid stays visible (#1470). Both thresholds
/// are further for leaving than for entering, so a view hovering at one does not flicker the
/// grid, and the upright plane changes to the other only close to a side view, never at the 45°
/// crossing (#1993).
pub fn grid_plane(back: Vec3, last: GridPlane) -> GridPlane {
    // About 11.5° above or below the ground to leave it, and 15° to return.
    const EDGE_ON: f64 = 0.2;
    const EDGE_LEAVE: f64 = 0.26;
    // About 15° from the Front/Back or Left/Right view: only that close does the wall change.
    const SIDE_VIEW: f64 = 0.966;
    let horizontal = (back.x * back.x + back.y * back.y).sqrt();
    if !back.x.is_finite() || !back.y.is_finite() || !back.z.is_finite() || !horizontal.is_finite() {
        return GridPlane::Ground;
    }
    if last == GridPlane::Ground && back.z.abs() <= EDGE_ON && horizontal > 0.0 {
        return if back.y.abs() >= back.x.abs() { GridPlane::Xz } else { GridPlane::Yz };
    }
    if last != GridPlane::Ground && back.z.abs() <= EDGE_LEAVE && horizontal > 0.0 {
        if back.x.abs() / horizontal >= SIDE_VIEW {
            return GridPlane::Yz;
        }
        if back.y.abs() / horizontal >= SIDE_VIEW {
            return GridPlane::Xz;
        }
        return last;
    }
    GridPlane::Ground
}

/// Grid spacing (minor, major) for a view of the given half height.
pub fn grid_step(half_height: f64) -> (f64, f64) {
    let raw = (half_height / 10.0).max(1e-3);
    let p = 10f64.powf(raw.log10().floor());
    let minor = if raw / p < 2.0 {
        p
    } else if raw / p < 5.0 {
        2.0 * p
    } else {
        5.0 * p
    };
    (minor, minor * 5.0)
}

/// Sketch curves as world polylines with their colour.
pub fn sketch_lines(sk: &Sketch, plane: &Plane, active: bool, determined: &[bool]) -> Vec<(Vec<Vec3>, Rgb, bool)> {
    sketch_lines_with_sampler(sk, plane, active, determined, world_curve_parts)
}

/// A sketch curve as world polylines: the sampler of [`scene`] and [`sketch_lines`].
pub fn world_curve_parts(sk: &Sketch, i: usize, plane: &Plane) -> Vec<Vec<Vec3>> {
    sk.segs(i).into_iter().map(|seg| seg.polyline(0.02).iter().map(|p| plane.to_world(*p)).collect()).collect()
}

/// Sketch display paths with the same visibility and styling as [`sketch_lines`].
///
/// The sampler receives each visible 2D curve once and returns world-coordinate
/// parts. Parts remain separate, so a view-dependent sampler can leave clipping
/// gaps without drawing a line across them. Already placed 3D wires retain their
/// existing paths. Sampling changes only display geometry, never the sketch.
pub fn sketch_lines_with_sampler(
    sk: &Sketch,
    plane: &Plane,
    active: bool,
    determined: &[bool],
    sampler: impl FnMut(&Sketch, usize, &Plane) -> Vec<Vec<Vec3>>,
) -> Vec<(Vec<Vec3>, Rgb, bool)> {
    sketch_lines_in(sk, plane, active, determined, sampler, &Palette::DEFAULT)
}

/// [`sketch_lines_with_sampler`] in the colours of `p`.
fn sketch_lines_in(
    sk: &Sketch,
    plane: &Plane,
    active: bool,
    determined: &[bool],
    mut sampler: impl FnMut(&Sketch, usize, &Plane) -> Vec<Vec<Vec3>>,
    p: &Palette,
) -> Vec<(Vec<Vec3>, Rgb, bool)> {
    let mut out = Vec::new();
    for (i, c) in sk.curves.iter().enumerate() {
        if !sketch_curve_visible(sk, i) {
            continue;
        }
        let col = if c.link.is_some() && !sk.is_text_curve(i) {
            if sk.is_lost_curve(i) { p.sketch_lost } else { p.sketch_projected }
        } else if c.fixed {
            p.sketch_locked
        } else if c.centerline {
            p.sketch_centerline
        } else if c.construction {
            p.sketch_construction
        } else if !active {
            p.sketch_done
        } else if determined.get(i).copied().unwrap_or(false) {
            p.sketch_fixed
        } else {
            p.sketch
        };
        for part in sampler(sk, i, plane) {
            out.push((part, col, c.construction || c.centerline));
        }
    }
    // Virtual sharp corners (kept by fillets and chamfers): thin extension lines from the
    // curves to the corner, like construction geometry.
    for (_, ext) in sk.virtual_corners() {
        for (a, b) in ext {
            out.push((vec![plane.to_world(a), plane.to_world(b)], p.sketch_construction, true));
        }
    }
    // 3D curves are in world coordinates already.
    for w in sk.wires.iter().filter(|w| !w.fit.is_empty() || !sk.view.hide_projected) {
        // Curves drawn in a 3D sketch look like the sketch's own; included ones are references.
        let col = if w.fit.is_empty() {
            p.sketch_projected
        } else if active {
            p.sketch
        } else {
            p.sketch_done
        };
        out.push((w.pts.clone(), col, false));
    }
    out
}

/// Should sketch curve `i` be drawn and picked? The Sketch Palette's two geometry toggles each
/// hide one category, and a curve belongs to the category its colour already names in
/// [`sketch_lines_with_sampler`]: a linked curve is projected reference geometry (Projected
/// Geometries), anything else carrying the `construction` line type is construction geometry
/// (Construction Geometries). Text outlines are linked curves that the Text feature draws, so
/// they are never reference geometry and always show. Centerlines are their own line type and
/// no toggle hides them.
///
/// Drawing and picking share this one answer so they cannot drift: geometry that is not on
/// screen is never clickable, and geometry on screen always is.
pub fn sketch_curve_visible(sk: &Sketch, i: usize) -> bool {
    let Some(c) = sk.curves.get(i) else { return false };
    if c.link.is_some() && !sk.is_text_curve(i) {
        return !sk.view.hide_projected;
    }
    !(c.construction && sk.view.hide_construction)
}

/// Point roles shown and picked individually. Associated offsets keep their generated
/// fit nodes private until their association is broken. Build once per sketch/frame.
pub struct SketchPointVisibility<'a> {
    hidden: std::collections::HashSet<&'a str>,
}

impl<'a> SketchPointVisibility<'a> {
    pub fn new(sk: &'a Sketch) -> Self {
        let outputs: std::collections::HashSet<&str> = sk.freeform_offsets.iter().map(|o| o.output.as_str()).collect();
        // A repeated link ID names its first link, as Sketch::link does.
        let mut seen = std::collections::HashSet::new();
        let text: std::collections::HashSet<&str> = sk
            .links
            .iter()
            .filter(|l| seen.insert(l.id.as_str()))
            .filter(|l| l.kind == solidcraft_sketch::LinkKind::Text)
            .map(|l| l.id.as_str())
            .collect();
        let mut hidden: std::collections::HashSet<&str> =
            sk.points.iter().filter(|p| p.link.as_deref().is_some_and(|l| text.contains(l))).map(|p| p.id.as_str()).collect();
        for curve in sk.curves.iter().filter(|c| outputs.contains(c.id.as_str())) {
            hidden.extend(curve.kind.point_ids().into_iter().filter_map(|i| sk.points.get(i).map(|p| p.id.as_str())));
        }
        Self { hidden }
    }

    pub fn takes(&self, point: &solidcraft_sketch::SPoint) -> bool {
        !self.hidden.contains(point.id.as_str())
    }
}

/// Should sketch point `i` be drawn or picked individually? Text outlines and derived
/// offset fit nodes are private. Batch callers use [`SketchPointVisibility`].
pub fn sketch_point_visible(sk: &Sketch, i: usize) -> bool {
    sk.points.get(i).is_some_and(|p| SketchPointVisibility::new(sk).takes(p))
}

/// The scene for a headless render, cut by the session's section.
pub fn scene(s: &Session, cam: &Camera) -> Scene {
    scene_with_section(s, cam, s.section)
}

/// The scene for a headless render, cut by `section` (origin, normal) instead of the session's:
/// the app passes the cut its viewport shows (an open Section Analysis dialog's preview, a
/// sketch's Slice), so model renders match the window.
pub fn scene_with_section(s: &Session, cam: &Camera, section: Option<(Vec3, Vec3)>) -> Scene {
    scene_with_guides(s, cam, section, Guides::default())
}

/// What a headless render draws besides the model: the ground grid and the origin's axes.
/// Both are on by default; the grid lies on Z 0, so it cuts through a model that straddles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guides {
    /// The ground grid.
    pub grid: bool,
    /// The origin's axes. An axis hidden with `browser.visibility` (`X`, `Y`, `Z`, or the whole
    /// `origin` folder) stays hidden whatever this says, as in the viewport.
    pub axes: bool,
}

impl Default for Guides {
    fn default() -> Self {
        Guides { grid: true, axes: true }
    }
}

/// [`scene_with_section`] with the grid and the axes shown or left out.
pub fn scene_with_guides(s: &Session, cam: &Camera, section: Option<(Vec3, Vec3)>, guides: Guides) -> Scene {
    scene_with_palette_and_guides(s, cam, section, world_curve_parts, &Palette::DEFAULT, guides)
}

/// A headless scene using supplied display paths for its visible sketch curves.
/// Callers can use the same pixel sampler as a GPU viewport without changing the
/// body, grid, appearance, section or sketch styling policies of [`scene`].
pub fn scene_with_sketch_sampler(s: &Session, cam: &Camera, sampler: impl FnMut(&Sketch, usize, &Plane) -> Vec<Vec<Vec3>>) -> Scene {
    scene_with_section_and_sketch_sampler(s, cam, s.section, sampler)
}

/// A headless scene with an explicit section and supplied sketch display paths.
pub fn scene_with_section_and_sketch_sampler(
    s: &Session,
    cam: &Camera,
    section: Option<(Vec3, Vec3)>,
    sampler: impl FnMut(&Sketch, usize, &Plane) -> Vec<Vec<Vec3>>,
) -> Scene {
    scene_with_palette(s, cam, section, sampler, &Palette::DEFAULT)
}

/// [`scene_with_section_and_sketch_sampler`] in the colours of `p` (the app's theme).
pub fn scene_with_palette(
    s: &Session,
    cam: &Camera,
    section: Option<(Vec3, Vec3)>,
    sampler: impl FnMut(&Sketch, usize, &Plane) -> Vec<Vec<Vec3>>,
    p: &Palette,
) -> Scene {
    build_scene(s, cam, section, Guides::default(), sampler, p)
}

/// [`scene_with_palette`] with the ground grid and the origin's axes shown or left out.
pub fn scene_with_palette_and_guides(
    s: &Session,
    cam: &Camera,
    section: Option<(Vec3, Vec3)>,
    sampler: impl FnMut(&Sketch, usize, &Plane) -> Vec<Vec<Vec3>>,
    p: &Palette,
    guides: Guides,
) -> Scene {
    build_scene(s, cam, section, guides, sampler, p)
}

fn build_scene(
    s: &Session,
    cam: &Camera,
    section: Option<(Vec3, Vec3)>,
    guides: Guides,
    mut sampler: impl FnMut(&Sketch, usize, &Plane) -> Vec<Vec<Vec3>>,
    p: &Palette,
) -> Scene {
    let st = s.world_state();
    let section = section.filter(|(o, n)| o.is_finite() && n.is_finite() && n.len() > 1e-9);
    let mut sc = Scene {
        background: Some(p.background),
        clip: section.map(|(o, n)| (n, n.dot(o))),
        cap: Some(p.cap),
        analysis: s.analysis.map(|a| SceneAnalysis { mode: a.mode(), colors: p.analysis }),
        ..Default::default()
    };
    let b = bounds(s);
    sc.radius = (b.diagonal() * 0.5).max(cam.half_height()).max(10.0) + b.center().dist(cam.target);
    // Grid on the ground plane (XY with Z up, XZ with Y up), drawn under the model as in the viewport.
    if guides.grid {
        let (minor, major) = grid_step(cam.half_height());
        let ext = (cam.half_height() * 3.0 / major).ceil() * major;
        let tf = cam.to_frame(cam.target);
        // Headless pictures keep the grid on the ground, the viewport's default.
        let (u, v) = (Vec3::X, Vec3::Y);
        let snap = |a: Vec3| a * ((tf.dot(a) / major).round() * major);
        let c = snap(u) + snap(v);
        let w = |p: Vec3| cam.from_frame(p);
        let n = ((ext / minor) as i64).clamp(1, 400);
        for i in -n..=n {
            let t = i as f64 * minor;
            let major_line = (t / major).fract().abs() < 1e-9;
            let col = if major_line { p.grid_major } else { p.grid };
            // The viewport's widths (pixels).
            let width = if major_line { 1.0 } else { 0.7 };
            sc.grid.push(SceneLine { points: vec![w(c + u * t - v * ext), w(c + u * t + v * ext)], color: col, width, on_top: false });
            sc.grid.push(SceneLine { points: vec![w(c - u * ext + v * t), w(c + u * ext + v * t)], color: col, width, on_top: false });
        }
    }
    // The origin's axes, each unless hidden one by one (`browser.visibility`), as in the viewport.
    let axis = cam.half_height() * 0.25;
    for ((name, d), col) in [("X", Vec3::X), ("Y", Vec3::Y), ("Z", Vec3::Z)].into_iter().zip(p.axes) {
        if guides.axes && !s.visibility.hidden_origin.iter().any(|h| h == name) {
            sc.lines.push(SceneLine { points: vec![Vec3::ZERO, d * axis], color: col, width: 1.6, on_top: false });
        }
    }
    let selected_occurrences: Vec<String> = s
        .selection
        .iter()
        .filter_map(|x| match x {
            Sel::Occurrence { path } => s.occurrence_bodies(path, true).ok(),
            _ => None,
        })
        .flatten()
        .collect();
    for body in st.bodies.iter().filter(|b| s.body_visible(&b.name)) {
        let selected = selected_occurrences.contains(&body.name) || s.selection.iter().any(|x| matches!(x, Sel::Body { name } if *name == body.name));
        let mesh = body.mesh();
        // The body's appearance (or imported colour) and faces with looks of their own, as in
        // the viewport.
        let to8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        let color = if selected { p.body_selected } else { s.doc.body_color(body).map_or(p.body, |[r, g, b]| Rgb(to8(r), to8(g), to8(b))) };
        let face_colors = if selected {
            Vec::new()
        } else {
            s.doc.face_colors(body).into_iter().map(|(f, l)| (f as u32, Rgb(l.color[0], l.color[1], l.color[2]))).collect()
        };
        sc.meshes.push(SceneMesh { mesh: Arc::clone(&mesh), color, face_colors });
        for (i, e) in mesh.edges.iter().enumerate() {
            if !mesh.shaded_edge_visible(i) {
                continue;
            }
            sc.lines.push(SceneLine { points: e.clone(), color: p.edge, width: 1.2, on_top: false });
        }
        // The outline of the body where the section cuts it.
        if let Some((o, n)) = section {
            for (a, b) in section_segments(&mesh, o, n) {
                sc.lines.push(SceneLine { points: vec![a, b], color: p.edge, width: 1.6, on_top: false });
            }
        }
    }
    for ss in &st.sketches {
        let active = s.active_sketch == Some(ss.feature);
        if !sketch_shown(s, ss.feature) {
            continue;
        }
        for (pts, col, cons) in sketch_lines_in(&ss.sketch, &ss.plane, active, &ss.report.curve_determined, &mut sampler, p) {
            sc.lines.push(SceneLine { points: pts, color: col, width: if cons { 1.0 } else { 1.6 }, on_top: active });
        }
    }
    sc
}

/// Where the plane through `o` with normal `n` cuts a mesh: one segment per crossed triangle,
/// nudged a hair to the kept side (so the clip leaves them drawn).
pub fn section_segments(m: &solidcraft_geom::Mesh, o: Vec3, n: Vec3) -> Vec<(Vec3, Vec3)> {
    let eps = 1e-4 * (1.0 + m.bounds().diagonal());
    let mut out = Vec::new();
    for t in &m.triangles {
        let Some(p) = m.tri(t) else { continue };
        let d = p.map(|q| n.dot(q - o));
        let mut pts = Vec::with_capacity(2);
        for (i, j) in [(0, 1), (1, 2), (2, 0)] {
            if (d[i] < 0.0) != (d[j] < 0.0) {
                let s = d[i] / (d[i] - d[j]);
                pts.push(p[i] + (p[j] - p[i]) * s - n * eps);
            }
        }
        if let [a, b] = pts[..] {
            out.push((a, b));
        }
    }
    out
}

/// Is a sketch drawn? The active one is; others unless hidden. Finished sketches are hidden
/// once a feature uses them, like Fusion's default, unless shown one by one.
pub fn sketch_shown(s: &Session, id: u64) -> bool {
    sketch_drawn(s, id, true)
}

/// Is a sketch drawn, with the browser's Sketches folder shown (`folder`) or hidden? The sketch
/// being edited always is; otherwise not when hidden, nor in a hidden component, and one used by
/// a feature only when shown on its own.
pub fn sketch_drawn(s: &Session, id: u64, folder: bool) -> bool {
    let v = &s.visibility;
    if s.active_sketch == Some(id) {
        return true;
    }
    if v.hidden_sketches.contains(&id) || !sketch_component_visible(s, id) {
        return false;
    }
    v.shown_sketches.contains(&id) || (folder && !sketch_consumed(s, id))
}

/// Is the placement a sketch is drawn at shown (the active one while its component is being
/// edited, else the component's first)?
fn sketch_component_visible(s: &Session, id: u64) -> bool {
    if s.visibility.hidden_occurrences.is_empty() {
        return true;
    }
    let component = s.doc.feature(id).map_or(0, |f| f.component);
    if component == 0 {
        return true;
    }
    let path = match &s.active_occurrence_path {
        Some(p) if s.active_component == component => p.clone(),
        _ => s.doc.first_path(component),
    };
    s.occurrence_visible(&path)
}

/// Is a sketch used by a later feature?
pub fn sketch_consumed(s: &Session, id: u64) -> bool {
    use solidcraft_doc::FeatureKind;
    s.doc.features.iter().any(|f| matches!(f.kind, FeatureKind::Extrude { sketch, .. } | FeatureKind::Revolve { sketch, .. } if sketch == id))
}

/// Camera framing the model (iso view).
pub fn home_camera(s: &Session) -> Camera {
    let mut c = Camera::default();
    let b = bounds(s);
    if !b.is_empty() {
        c.fit(&b);
    } else {
        c.distance = 150.0;
    }
    c
}

/// Construction planes of the timeline, as evaluated: (feature id, name, plane).
pub fn construction_planes(s: &Session) -> Vec<(u64, String, Plane)> {
    constructs(s)
        .into_iter()
        .filter_map(|(id, name, g)| match g {
            solidcraft_doc::construct::ConstructGeom::Plane(p) => Some((id, name, p)),
            _ => None,
        })
        .collect()
}

/// Construction axes, as evaluated: (feature id, name, origin, unit direction).
pub fn construction_axes(s: &Session) -> Vec<(u64, String, Vec3, Vec3)> {
    constructs(s)
        .into_iter()
        .filter_map(|(id, name, g)| match g {
            solidcraft_doc::construct::ConstructGeom::Axis { origin, dir } => Some((id, name, origin, dir)),
            _ => None,
        })
        .collect()
}

/// Construction points, as evaluated: (feature id, name, position).
pub fn construction_points(s: &Session) -> Vec<(u64, String, Vec3)> {
    constructs(s)
        .into_iter()
        .filter_map(|(id, name, g)| match g {
            solidcraft_doc::construct::ConstructGeom::Point(p) => Some((id, name, p)),
            _ => None,
        })
        .collect()
}

/// Construction geometry of unsuppressed features, shown where its component is placed.
fn constructs(s: &Session) -> Vec<(u64, String, solidcraft_doc::construct::ConstructGeom)> {
    use solidcraft_doc::construct::ConstructGeom;
    let st = s.model.state();
    st.construct
        .iter()
        .filter_map(|c| {
            let f = s.doc.feature(c.feature).filter(|f| !f.suppressed)?;
            let m = s.doc.component_transform(f.component);
            if solidcraft_doc::is_identity(&m) {
                return Some((c.feature, c.name.clone(), c.geom.clone()));
            }
            let (pt, v) = (|p: Vec3| solidcraft_doc::apply_point(&m, p), |d: Vec3| solidcraft_doc::apply_vector(&m, d));
            let g = match &c.geom {
                ConstructGeom::Plane(p) => ConstructGeom::Plane(Plane::new(pt(p.origin), v(p.x), v(p.y)).unwrap_or(*p)),
                ConstructGeom::Axis { origin, dir } => ConstructGeom::Axis { origin: pt(*origin), dir: v(*dir) },
                ConstructGeom::Point(p) => ConstructGeom::Point(pt(*p)),
            };
            Some((c.feature, c.name.clone(), g))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Headless pictures show appearances as the viewport does: a body's colour and faces with
    /// looks of their own (the picture itself: `solidcraft-gpu`'s capture tests).
    #[test]
    fn snapshot_scene_carries_appearances() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        s.execute("solid.cylinder", &json!({"diameter": 14, "height": 30, "base": [50, 15, 0], "operation": "new"})).unwrap();
        s.execute("appearance.assign", &json!({"bodies": ["Body2"], "color": [220, 60, 40]})).unwrap();
        s.execute("appearance.assign", &json!({"faces": [[20, 15, 20]], "body": "Body1", "color": [40, 170, 80]})).unwrap();
        let cam = home_camera(&s);
        let sc = scene(&s, &cam);
        assert!(sc.meshes.iter().any(|m| m.color == Rgb(220, 60, 40)));
        assert!(sc.meshes.iter().any(|m| m.face_colors.iter().any(|(_, c)| *c == Rgb(40, 170, 80))));
    }

    /// A render can leave out the ground grid and the origin's axes (a box straddling Z 0 is cut
    /// through by the grid), each on its own; the body's own lines stay.
    #[test]
    fn guides_leave_the_grid_and_the_axes_out() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20, "corner": [10, -15, -10]})).unwrap();
        let cam = home_camera(&s);
        let grid = [colors::GRID, colors::GRID_MAJOR];
        let axes = [colors::AXIS_X, colors::AXIS_Y, colors::AXIS_Z];
        let count = |sc: &Scene, colours: &[Rgb]| sc.grid.iter().chain(&sc.lines).filter(|l| colours.contains(&l.color)).count();
        let all = scene(&s, &cam);
        let (grid_lines, edges) = (count(&all, &grid), count(&all, &[colors::EDGE]));
        assert!(grid_lines > 20 && edges >= 12, "{grid_lines} grid lines, {edges} edges");
        assert_eq!(count(&all, &axes), 3);
        assert_eq!(all.lines.len(), scene_with_guides(&s, &cam, s.section, Guides::default()).lines.len());
        let render = |g: Guides| scene_with_guides(&s, &cam, s.section, g);
        let no_grid = render(Guides { grid: false, axes: true });
        assert_eq!((count(&no_grid, &grid), count(&no_grid, &axes), count(&no_grid, &[colors::EDGE])), (0, 3, edges));
        let no_axes = render(Guides { grid: true, axes: false });
        assert_eq!((count(&no_axes, &grid), count(&no_axes, &axes), count(&no_axes, &[colors::EDGE])), (grid_lines, 0, edges));
        let bare = render(Guides { grid: false, axes: false });
        assert_eq!((bare.lines.len(), bare.grid.len()), (edges, 0));
    }

    /// An origin axis hidden with `browser.visibility` stays out of headless renders, as it does
    /// out of the viewport, whatever the render's own switch says; the grid is not the origin.
    #[test]
    fn hidden_origin_axes_stay_out_of_renders() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let cam = home_camera(&s);
        let axes = [colors::AXIS_X, colors::AXIS_Y, colors::AXIS_Z];
        let shown = |s: &Session| -> Vec<Rgb> {
            let sc = scene(s, &cam);
            axes.into_iter().filter(|c| sc.lines.iter().any(|l| l.color == *c)).collect()
        };
        assert_eq!(shown(&s), axes);
        s.execute("browser.visibility", &json!({"items": ["Y"], "visible": false})).unwrap();
        assert_eq!(shown(&s), [colors::AXIS_X, colors::AXIS_Z]);
        s.execute("browser.visibility", &json!({"folder": "origin", "visible": false})).unwrap();
        assert!(shown(&s).is_empty());
        let sc = scene_with_guides(&s, &cam, None, Guides { grid: true, axes: true });
        assert!(sc.grid.iter().any(|l| l.color == colors::GRID), "the grid is not part of the origin");
        s.execute("browser.visibility", &json!({"folder": "origin", "visible": true})).unwrap();
        assert_eq!(shown(&s), axes);
    }

    /// A section cuts the headless scene: the clip plane and the cap colour, and the outline
    /// where the plane cuts each body (the picture: `solidcraft-gpu`'s capture tests).
    #[test]
    fn the_section_cuts_the_scene() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let cam = home_camera(&s);
        assert!(scene(&s, &cam).clip.is_none());
        s.execute("inspect.section", &json!({"plane": {"origin": [0, 15, 0], "normal": [0, -1, 0]}})).unwrap();
        let sc = scene(&s, &cam);
        let (n, d) = sc.clip.unwrap();
        assert!((n - Vec3::new(0.0, -1.0, 0.0)).len() < 1e-12 && (d + 15.0).abs() < 1e-12, "{n:?} {d}");
        assert_eq!(sc.cap, Some(colors::SECTION_CAP));
        assert!(!section_segments(&s.world_state().bodies[0].mesh(), Vec3::new(0.0, 15.0, 0.0), -Vec3::Y).is_empty());
    }

    /// The surface analysis shades the headless scene too, in the palette's colours.
    #[test]
    fn the_surface_analysis_shades_the_scene() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let cam = home_camera(&s);
        assert!(scene(&s, &cam).analysis.is_none());
        s.analysis = Some(crate::SurfaceAnalysis::Curvature { radius: 5.0 });
        let a = scene(&s, &cam).analysis.unwrap();
        assert_eq!((a.mode, a.colors), (solidcraft_render::AnalysisMode::Curvature { radius: 5.0 }, Palette::DEFAULT.analysis));
    }

    /// The CPU rasterizer (the last tier of headless pictures, `SOLIDCRAFT_GPU=cpu` or no adapter
    /// under `auto`): curved surfaces get a dark silhouette line where they turn away.
    #[test]
    fn cpu_pictures_draw_silhouettes_on_curved_surfaces() {
        let mut s = Session::default();
        s.execute("solid.sphere", &json!({"diameter": 20})).unwrap();
        let cam = home_camera(&s);
        let img = solidcraft_render::render(&scene(&s, &cam), &cam, 400, 400);
        let dark = img.rgba.chunks(4).filter(|p| p[0] < 70 && p[1] < 70 && p[2] < 75).count();
        assert!(dark > 40, "silhouette pixels: {dark}");
    }

    /// The CPU rasterizer: a section shows; the cut part is gone and the cut face is drawn.
    #[test]
    fn cpu_pictures_show_the_section() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        let cam = home_camera(&s);
        let whole = solidcraft_render::render(&scene(&s, &cam), &cam, 200, 140).rgba;
        s.execute("inspect.section", &json!({"plane": {"origin": [0, 15, 0], "normal": [0, -1, 0]}})).unwrap();
        let sc = scene(&s, &cam);
        assert!(sc.clip.is_some());
        let cut = solidcraft_render::render(&sc, &cam, 200, 140).rgba;
        let changed = whole.chunks(4).zip(cut.chunks(4)).filter(|(a, b)| a != b).count();
        assert!(changed > 500, "{changed} pixels changed");
    }

    /// The section's cut face hides what lies inside the body behind it (it is drawn at the
    /// plane's depth, not on the far wall like a pane of glass), shows in the cap colour, and
    /// the cut's outline is drawn.
    #[test]
    fn cpu_picture_section_caps_hide_the_inside() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
        // Cut away the front half (y < 15), so the iso camera looks into the cut.
        s.execute("inspect.section", &json!({"plane": {"origin": [0, 15, 0], "normal": [0, -1, 0]}})).unwrap();
        for fov in [0.0, 0.6] {
            let mut cam = home_camera(&s);
            cam.fov = fov;
            let mut sc = scene(&s, &cam);
            assert!(!section_segments(&s.world_state().bodies[0].mesh(), Vec3::new(0.0, 15.0, 0.0), -Vec3::Y).is_empty());
            // A marker inside the body, behind the cut face.
            let red = Rgb(255, 0, 0);
            sc.lines.push(SceneLine { points: vec![Vec3::new(5.0, 25.0, 10.0), Vec3::new(35.0, 25.0, 10.0)], color: red, width: 3.0, on_top: false });
            let img = solidcraft_render::render(&sc, &cam, 300, 220);
            let px = || img.rgba.chunks(4);
            let marker = px().filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60).count();
            assert_eq!(marker, 0, "fov {fov}: the inside shows through the cut face");
            // The cap: warm (red over blue), unlike the grey-blue body and background.
            let cap = px().filter(|p| p[0] as i32 - p[2] as i32 > 20).count();
            assert!(cap > 500, "fov {fov}: cap pixels {cap}");
        }
    }
}

#[cfg(test)]
#[path = "view_display_tests.rs"]
mod display_tests;

#[cfg(test)]
#[path = "view_palette_tests.rs"]
mod palette_tests;
