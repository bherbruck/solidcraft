//! Select Other: everything under the cursor, for picking what lies under something else (two
//! lines drawn on top of each other, coincident points, an edge in front of a face, overlapping
//! bodies).
//!
//! - Holding the left button still (or a finger, on touch screens) for [`HOLD_SECS`], or the
//!   viewport menu's "Select Other…", opens a list at the cursor of every pickable thing there,
//!   in the selection filter's and the open dialog's terms: sketch points and curves first, then
//!   vertices, edges, faces, bodies, profiles, planes and axes, each kind nearest first. On a
//!   dense mesh (a modelled thread, an imported mesh) thousands of facets can share one cursor,
//!   so at most [`MAX_PER_KIND`] rows are kept per kind, nearest first, and the list reports how
//!   many things there were in all.
//! - Hovering a row lights its entity up in the viewport; clicking it selects it (Shift/Ctrl add,
//!   as clicks do) or, with a dialog open, gives it to the dialog's active selection input.
//!   Esc or a click elsewhere closes the list.
//! - Clicking again on the same spot with nothing changed selects the next thing in that list
//!   (click cycling). The second click of a double-click keeps its own actions.
//!
//! The control channel's `ui.selectOther` opens, describes and picks from the list.

use egui::{Align2, FontId, Pos2, Rect, Sense, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::sketch::CurveKind;
use std::hash::{Hash, Hasher};

use crate::SolveApp;
use crate::theme::Tokens;
use crate::viewport::{Hit, Proj};

/// How long the left button stays down, without moving, before the list opens.
pub const HOLD_SECS: f64 = 0.5;
/// How far (screen points) a repeat click may be from the last one and still cycle.
const CYCLE_SLOP: f32 = 4.0;
/// The most rows kept per kind of thing (vertices, edges, faces, …). The list exists to tell a
/// handful of overlapping things apart; nearest first, farther ones of the same kind are dropped
/// so it stays readable and bounded.
pub(crate) const MAX_PER_KIND: usize = 20;

/// One row of the list.
#[derive(Clone, Debug)]
pub struct Row {
    pub hit: Hit,
    pub sel: Sel,
    pub label: String,
    pub icon: &'static str,
}

/// The open list.
#[derive(Clone, Debug)]
pub struct Popup {
    pub at: Pos2,
    pub rows: Vec<Row>,
    /// How many things were under the cursor in all (the rows are capped per kind).
    pub total: usize,
    /// The row under the pointer (lit in the viewport).
    pub hovered: Option<usize>,
    /// The model and dialog it was made for: it closes when they change.
    revision: u64,
    dialog: bool,
    /// The browser's body highlight is ours to clear.
    lit_body: bool,
}

/// The last plain click, for cycling.
#[derive(Clone, Debug)]
struct Cycle {
    at: Pos2,
    chosen: Sel,
    selection: Vec<Sel>,
    key: u64,
}

#[derive(Default)]
pub struct State {
    pub popup: Option<Popup>,
    /// A left press that may become a hold: where and when.
    hold: Option<(Pos2, f64)>,
    /// The current press belongs to Select Other (it opened or closed the list): the viewport
    /// doesn't also click, drag or box-select with it.
    held: bool,
    cycle: Option<Cycle>,
}

// ---------------------------------------------------------------------------------------------
// What is under the cursor
// ---------------------------------------------------------------------------------------------

/// Every pickable thing under `pos`, ordered by kind (the sketch being edited first), then depth.
pub fn rows(app: &SolveApp, proj: &Proj, pos: Pos2) -> Vec<Row> {
    rows_capped(app, proj, pos).0
}

/// [`rows`], with how many things were under the cursor in all.
fn rows_capped(app: &SolveApp, proj: &Proj, pos: Pos2) -> (Vec<Row>, usize) {
    let hits = crate::viewport::pick_all(app, proj, pos);
    let mut found = crate::viewport::candidates(app, &hits);
    let dialog = app.dialog.as_ref().is_some_and(|d| d.wants_picks());
    // A body behind each face (body picking already turns faces into bodies).
    if !dialog && !app.ui.pick_bodies {
        for h in &hits {
            if let Hit::Face { body, .. } = h {
                found.push((h.clone(), Sel::Body { name: body.clone() }));
            }
        }
    }
    // A face place a sketch point snapped (a Single hole's) is its own row: beside the face as
    // picked, and beside other points on that face.
    let snapped = |h: &Hit| matches!(h, Hit::SketchPoint { .. });
    let mut out: Vec<(Hit, Sel)> = Vec::new();
    for (h, s) in found {
        if !out.iter().any(|(hx, x)| if snapped(hx) || snapped(&h) { *x == s } else { crate::selection::same_item(x, &s) }) {
            out.push((h, s));
        }
    }
    let sketching = app.session.active_sketch.is_some();
    out.sort_by_key(|(h, s)| rank(&shown_as(h, s), sketching));
    let total = out.len();
    // Nearest first within a kind, so capping drops the farther ones.
    let mut per_kind: std::collections::HashMap<u8, usize> = std::collections::HashMap::new();
    let rows: Vec<Row> = out
        .into_iter()
        .filter(|(h, s)| {
            let n = per_kind.entry(rank(&shown_as(h, s), sketching).1).or_default();
            if *n >= MAX_PER_KIND {
                return false;
            }
            *n += 1;
            true
        })
        .map(|(hit, sel)| {
            let shown = shown_as(&hit, &sel);
            let label = label(app, &shown);
            Row { icon: icon(app, &shown), hit, sel, label }
        })
        .collect();
    (rows, total)
}

/// What a row shows itself as: a face place a sketch point snapped is that point (ranked, capped
/// and named with the points).
fn shown_as(hit: &Hit, sel: &Sel) -> Sel {
    match (hit, sel) {
        (Hit::SketchPoint { sketch, id, .. }, Sel::Face { .. }) => Sel::ExternalSketchPoint { sketch: *sketch, id: id.clone() },
        _ => sel.clone(),
    }
}

/// Sort key: in a sketch its own points and curves come first; then by kind.
fn rank(s: &Sel, sketching: bool) -> (u8, u8) {
    let kind = match s {
        Sel::SketchPoint { .. } | Sel::ExternalSketchPoint { .. } => 0,
        Sel::SketchCurve { .. } | Sel::ExternalSketchCurve { .. } => 1,
        Sel::SketchConstraint { .. } => 2,
        Sel::Vertex { .. } => 3,
        Sel::Edge { .. } => 4,
        Sel::Face { .. } | Sel::Feature { .. } => 5,
        Sel::Body { .. } | Sel::Occurrence { .. } => 6,
        Sel::Profile { .. } => 7,
        Sel::Plane { .. } => 8,
        Sel::Axis { .. } => 9,
    };
    let own = matches!(s, Sel::SketchPoint { .. } | Sel::SketchCurve { .. });
    (if sketching && !own { 1 } else { 0 }, kind)
}

/// The number at the end of an id (`l3` → 3).
fn id_number(id: &str) -> Option<u64> {
    let digits: String = id.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse().ok()
}

fn curve_kind_name(k: &CurveKind) -> &'static str {
    match k {
        CurveKind::Line { .. } => "Line",
        CurveKind::Circle { .. } => "Circle",
        CurveKind::Arc { .. } => "Arc",
        CurveKind::Ellipse { .. } => "Ellipse",
        CurveKind::Spline { .. } => "Spline",
        CurveKind::Conic { .. } => "Conic",
    }
}

fn sketch_name(app: &SolveApp, sketch: u64) -> String {
    app.session.doc.feature(sketch).map(|f| f.name.clone()).unwrap_or_else(|| format!("Sketch {sketch}"))
}

/// "Line 3" (its kind and number), with what kind of geometry it is when that matters.
fn curve_label(sk: &solidcraft_engine::sketch::Sketch, id: &str) -> String {
    if let Some((i, c)) = sk.curves.iter().enumerate().find(|(_, c)| c.id == id) {
        let kind = curve_kind_name(&c.kind);
        let n = id_number(id).unwrap_or_else(|| sk.curves.iter().take(i + 1).filter(|x| curve_kind_name(&x.kind) == kind).count() as u64);
        let mut s = format!("{kind} {n}");
        if c.link.is_some() {
            s.push_str(" (projected)");
        } else if c.construction {
            s.push_str(" (construction)");
        }
        return s;
    }
    if sk.wires.iter().any(|w| w.id == id) {
        return format!("3D Curve {}", id_number(id).unwrap_or(0));
    }
    id.to_string()
}

/// "Point (arc 2 centre)": the role it plays in the first curve that uses it.
fn point_label(sk: &solidcraft_engine::sketch::Sketch, id: &str) -> String {
    let Some(pi) = sk.points.iter().position(|p| p.id == id) else { return id.to_string() };
    let role = sk.curves.iter().find_map(|c| {
        let name = format!("{} {}", curve_kind_name(&c.kind).to_lowercase(), id_number(&c.id).unwrap_or(0));
        let (start, end) = if c.reversed { ("end", "start") } else { ("start", "end") };
        let role = match &c.kind {
            CurveKind::Line { a, b } | CurveKind::Arc { a, b, .. } | CurveKind::Conic { a, b, .. } if *a == pi => Some(start.to_string()),
            CurveKind::Line { b, .. } | CurveKind::Arc { b, .. } | CurveKind::Conic { b, .. } if *b == pi => Some(end.to_string()),
            CurveKind::Circle { c, .. } | CurveKind::Arc { c, .. } | CurveKind::Ellipse { c, .. } if *c == pi => Some("centre".to_string()),
            CurveKind::Ellipse { m, .. } if *m == pi => Some("major axis end".to_string()),
            CurveKind::Conic { apex, .. } if *apex == pi => Some("apex".to_string()),
            CurveKind::Spline { pts, .. } => pts.iter().position(|p| *p == pi).map(|k| format!("point {}", k + 1)),
            _ => None,
        }?;
        Some(format!("{name} {role}"))
    });
    let fixed = if sk.point_locked(pi) { ", fixed" } else { "" };
    match role {
        Some(r) => format!("Point ({r}{fixed})"),
        None if fixed.is_empty() => format!("Point {}", id_number(id).unwrap_or(pi as u64 + 1)),
        None => format!("Point {} (fixed)", id_number(id).unwrap_or(pi as u64 + 1)),
    }
}

/// A row's name: "Line 3", "Point (arc 2 centre)", "Edge 4 · Body1", "XY Plane"…
pub fn label(app: &SolveApp, sel: &Sel) -> String {
    let st = app.session.world_state();
    let in_sketch =
        |sketch: u64, f: &dyn Fn(&solidcraft_engine::sketch::Sketch) -> String| st.sketch(sketch).map(|ss| f(&ss.sketch)).unwrap_or_default();
    match sel {
        Sel::SketchPoint { id } => app.session.active_sketch.map(|s| in_sketch(s, &|sk| point_label(sk, id))).unwrap_or_else(|| id.clone()),
        Sel::SketchCurve { id } => app.session.active_sketch.map(|s| in_sketch(s, &|sk| curve_label(sk, id))).unwrap_or_else(|| id.clone()),
        Sel::ExternalSketchPoint { sketch, id } => format!("{} · {}", in_sketch(*sketch, &|sk| point_label(sk, id)), sketch_name(app, *sketch)),
        Sel::ExternalSketchCurve { sketch, id } => format!("{} · {}", in_sketch(*sketch, &|sk| curve_label(sk, id)), sketch_name(app, *sketch)),
        Sel::SketchConstraint { id } => format!("Constraint {id}"),
        Sel::Vertex { body, .. } => format!("Vertex · {body}"),
        Sel::Edge { body, index, .. } => format!("Edge {} · {body}", index + 1),
        Sel::Face { body, index, .. } => format!("Face {} · {body}", index + 1),
        Sel::Body { name } => format!("Body · {name}"),
        Sel::Occurrence { path } => format!("Occurrence {path:?}"),
        Sel::Feature { id } => app.session.doc.feature(*id).map(|f| f.name.clone()).unwrap_or_else(|| format!("Feature {id}")),
        Sel::Profile { sketch, index } => format!("Profile {} · {}", index + 1, sketch_name(app, *sketch)),
        Sel::Plane { name } if matches!(name.as_str(), "XY" | "XZ" | "YZ") => format!("{name} Plane"),
        Sel::Axis { name } if matches!(name.as_str(), "X" | "Y" | "Z") => format!("{name} Axis"),
        Sel::Plane { name } | Sel::Axis { name } => name.clone(),
    }
}

fn icon(app: &SolveApp, sel: &Sel) -> &'static str {
    let st = app.session.world_state();
    let curve = |sketch: Option<u64>, id: &str| {
        let kind = sketch.and_then(|s| st.sketch(s)).and_then(|ss| ss.sketch.curves.iter().find(|c| c.id == id).map(|c| c.kind.clone()));
        match kind {
            Some(CurveKind::Circle { .. }) => "circle",
            Some(CurveKind::Arc { .. }) => "arc_tangent",
            Some(CurveKind::Ellipse { .. }) => "ellipse",
            Some(CurveKind::Spline { .. }) => "spline",
            Some(CurveKind::Conic { .. }) => "conic",
            _ => "line",
        }
    };
    match sel {
        Sel::SketchPoint { .. } | Sel::ExternalSketchPoint { .. } | Sel::Vertex { .. } => "point",
        Sel::SketchCurve { id } => curve(app.session.active_sketch, id),
        Sel::ExternalSketchCurve { sketch, id } => curve(Some(*sketch), id),
        Sel::SketchConstraint { .. } => "dimension",
        Sel::Edge { .. } => "fillet",
        Sel::Face { .. } | Sel::Feature { .. } => "presspull",
        Sel::Body { .. } => "body",
        Sel::Occurrence { .. } => "component",
        Sel::Profile { .. } => "sketch",
        Sel::Plane { .. } => "plane",
        Sel::Axis { .. } => "axis",
    }
}

// ---------------------------------------------------------------------------------------------
// Opening, picking, closing
// ---------------------------------------------------------------------------------------------

fn projection_now(app: &SolveApp) -> Option<Proj> {
    app.viewport.rect.map(|r| crate::viewport::projection(app, r))
}

/// Open the list at a screen point; false (with a status message) when nothing is there.
pub fn open(app: &mut SolveApp, at: Pos2) -> bool {
    let Some(proj) = projection_now(app) else { return false };
    open_with(app, &proj, at)
}

fn open_with(app: &mut SolveApp, proj: &Proj, at: Pos2) -> bool {
    let (rows, total) = rows_capped(app, proj, at);
    if rows.is_empty() {
        app.status = Some(("Nothing to select here".into(), crate::now_ms(), false));
        return false;
    }
    close(app);
    app.viewport.select_other.popup =
        Some(Popup { at, rows, total, hovered: None, revision: app.session.revision, dialog: app.dialog.is_some(), lit_body: false });
    true
}

pub fn close(app: &mut SolveApp) {
    if let Some(p) = app.viewport.select_other.popup.take()
        && p.lit_body
    {
        app.viewport.hover_bodies.clear();
    }
}

pub fn is_open(app: &SolveApp) -> bool {
    app.viewport.select_other.popup.is_some()
}

/// Pick row `i` of the open list and close it.
///
/// `add` / `toggle` match viewport selection (Shift pure-add, Primary toggle when
/// `selection_shift_add_only` is on).
pub fn choose(app: &mut SolveApp, i: usize, add: bool, toggle: bool) -> Result<Row, String> {
    let Some(p) = app.viewport.select_other.popup.as_ref() else { return Err("Select Other is not open".into()) };
    let at = p.at;
    let row = p.rows.get(i).cloned().ok_or_else(|| format!("no row {i} (the list has {})", p.rows.len()))?;
    close(app);
    apply(app, &row, add, toggle);
    remember(app, at, if add { None } else { Some(row.sel.clone()) });
    Ok(row)
}

/// Select a row's entity: into the dialog's active input, or as a click does.
fn apply(app: &mut SolveApp, row: &Row, add: bool, toggle: bool) {
    if app.dialog.as_ref().is_some_and(|d| d.wants_picks()) {
        crate::viewport::dialog_pick(app, &row.hit, row.sel.clone());
    } else {
        crate::viewport::select(app, Some(row.sel.clone()), add, toggle);
    }
}

/// The open list as JSON (for `ui.selectOther`).
pub fn describe(app: &SolveApp) -> Value {
    match &app.viewport.select_other.popup {
        None => json!({ "open": false }),
        Some(p) => json!({
            "open": true,
            "at": [p.at.x, p.at.y],
            "hovered": p.hovered,
            "total": p.total,
            "rows": p.rows.iter().map(|r| json!({ "label": r.label, "icon": r.icon, "sel": r.sel })).collect::<Vec<_>>(),
        }),
    }
}

/// The control channel's `ui.selectOther`: `{x, y}` opens the list there, `{pick: i, add?}`
/// picks a row, `{hover: i}` lights one up, `{close: true}` closes it; it answers with the list.
pub fn control(app: &mut SolveApp, p: &Value) -> Result<Value, String> {
    if p.get("close").and_then(Value::as_bool) == Some(true) {
        close(app);
        return Ok(describe(app));
    }
    if let (Some(x), Some(y)) = (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64)) {
        if !(x.is_finite() && y.is_finite()) {
            return Err("x and y must be numbers".into());
        }
        if app.viewport.rect.is_none() {
            return Err("no viewport".into());
        }
        open(app, pos2(x as f32, y as f32));
        return Ok(describe(app));
    }
    if let Some(i) = p.get("hover").and_then(Value::as_u64) {
        let Some(pop) = app.viewport.select_other.popup.as_mut() else { return Err("Select Other is not open".into()) };
        pop.hovered = Some(i as usize).filter(|i| *i < pop.rows.len());
        hover_override(app);
        return Ok(describe(app));
    }
    if let Some(i) = p.get("pick").and_then(Value::as_u64) {
        // Control channel: `add` enables both add and toggle (legacy API).
        let add = p.get("add").and_then(Value::as_bool).unwrap_or(false);
        let row = choose(app, i as usize, add, add)?;
        return Ok(json!({ "picked": row.label, "sel": row.sel, "selection": app.session.selection }));
    }
    Ok(describe(app))
}

// ---------------------------------------------------------------------------------------------
// Press and hold, and click cycling (called by the viewport)
// ---------------------------------------------------------------------------------------------

/// Follow a left press held still in the viewport; open the list when it has been held long
/// enough. True while the press belongs to Select Other (the viewport must not also click,
/// drag or box-select with it).
#[allow(clippy::too_many_arguments)]
pub fn track_hold(
    app: &mut SolveApp,
    ctx: &egui::Context,
    proj: &Proj,
    inside: bool,
    pressed: bool,
    down: bool,
    hover: Option<Pos2>,
    navigating: bool,
) -> bool {
    let consumed = app.viewport.select_other.held;
    let now = ctx.input(|i| i.time);
    let free = app.tool.is_none()
        && app.viewport.context_menu.is_none()
        && !is_open(app)
        && app.dialog.as_ref().is_none_or(|d| d.wants_picks())
        && app.viewport.point_drag.is_none();
    if pressed && inside && free && !navigating {
        app.viewport.select_other.hold = hover.map(|p| (p, now));
    }
    let mut opened = false;
    if let Some((p0, t0)) = app.viewport.select_other.hold {
        let slop = ctx.options(|o| o.input_options.max_click_dist);
        if !down || navigating || !free || hover.is_some_and(|p| p.distance(p0) > slop) {
            app.viewport.select_other.hold = None;
        } else if now - t0 >= HOLD_SECS {
            app.viewport.select_other.hold = None;
            opened = open_with(app, proj, p0);
            app.viewport.select_other.held = true;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }
    if !down {
        app.viewport.select_other.held = false;
    }
    consumed || opened
}

/// While the list is open, the hovered row is what the viewport lights up.
pub fn hover_override(app: &mut SolveApp) {
    let Some(p) = app.viewport.select_other.popup.as_mut() else { return };
    let row = p.hovered.and_then(|i| p.rows.get(i));
    let body = match row.map(|r| &r.sel) {
        Some(Sel::Body { name }) => Some(name.clone()),
        _ => None,
    };
    app.viewport.hover = if body.is_some() { None } else { row.map(|r| r.hit.clone()) };
    match body {
        Some(b) => {
            app.viewport.hover_bodies = vec![b];
            p.lit_body = true;
        }
        None if p.lit_body => {
            app.viewport.hover_bodies.clear();
            p.lit_body = false;
        }
        None => {}
    }
}

/// What a repeat click must find unchanged: the model, the view, the sketch and the filter.
fn cycle_key(app: &SolveApp) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    app.session.revision.hash(&mut h);
    for v in [app.cam.yaw, app.cam.pitch, app.cam.roll, app.cam.distance, app.cam.target.x, app.cam.target.y, app.cam.target.z] {
        v.to_bits().hash(&mut h);
    }
    app.session.active_sketch.hash(&mut h);
    app.ui.pick_off.hash(&mut h);
    app.ui.pick_bodies.hash(&mut h);
    app.dialog.is_some().hash(&mut h);
    h.finish()
}

fn remember(app: &mut SolveApp, at: Pos2, chosen: Option<Sel>) {
    app.viewport.select_other.cycle = chosen.map(|chosen| Cycle { at, chosen, selection: app.session.selection.clone(), key: cycle_key(app) });
}

/// A plain click selected `sel` at `at`: a repeat click there may cycle from it.
pub fn note_click(app: &mut SolveApp, at: Pos2, sel: Option<Sel>) {
    remember(app, at, sel);
}

/// A plain click at `at`: when it repeats the last one (same spot, nothing changed since), select
/// the next thing under the cursor and return true.
pub fn cycle_click(app: &mut SolveApp, proj: &Proj, at: Pos2) -> bool {
    let Some(c) = app.viewport.select_other.cycle.clone() else { return false };
    if c.at.distance(at) > CYCLE_SLOP || c.key != cycle_key(app) || c.selection != app.session.selection {
        return false;
    }
    let rows = rows(app, proj, at);
    if rows.len() < 2 {
        return false;
    }
    let next = rows.iter().position(|r| crate::selection::same_item(&r.sel, &c.chosen)).map_or(0, |k| (k + 1) % rows.len());
    let Some(row) = rows.get(next).cloned() else { return false };
    apply(app, &row, false, false);
    app.status = Some((format!("{} ({} of {})", row.label, next + 1, rows.len()), crate::now_ms(), false));
    remember(app, c.at, Some(row.sel));
    true
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

/// Draw the open list; handle its hover, clicks, and a press elsewhere (which closes it).
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(p) = app.viewport.select_other.popup.clone() else { return };
    if p.revision != app.session.revision || p.dialog != app.dialog.is_some() || app.tool.is_some() {
        close(app);
        return;
    }
    let t = Tokens::get();
    let mut hovered = None;
    let mut picked = None;
    let w = crate::context_menu::LIST_W - 10.0;
    // Title, rows up to the scroll height, margins and border.
    let size = vec2(w + 10.0, 30.0 + (p.rows.len() as f32 * 24.0).min(360.0));
    let resp = crate::frame::area(egui::Id::new("sc_select_other"))
        .fixed_pos(crate::context_menu::place(p.at, vec2(12.0, 12.0), size, ctx.content_rect()))
        .constrain(true)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            crate::context_menu::menu_frame(ui, |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
                let title = crate::i18n::tr_label("Select Other").to_uppercase();
                let title = if p.total > p.rows.len() { format!("{title} · {} / {}", p.rows.len(), p.total) } else { title };
                ui.painter().text(pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, title, FontId::proportional(10.5), t.text_dim);
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for (i, row) in p.rows.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
                        crate::scenario::publish_handle(&format!("select_other:{i}"), r.center());
                        if resp.hovered() {
                            hovered = Some(i);
                            ui.painter().rect_filled(r.shrink2(vec2(2.0, 1.0)), 4.0, t.hover);
                        }
                        crate::icons::paint(
                            ui.painter(),
                            Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(16.0, 16.0)),
                            row.icon,
                            t.icon,
                            t.icon_fill,
                            t.accent,
                        );
                        ui.painter().text(pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, &row.label, FontId::proportional(13.0), t.text);
                        if resp.clicked() {
                            picked = Some(i);
                        }
                    }
                });
            });
        });
    if let Some(pop) = app.viewport.select_other.popup.as_mut()
        && pop.hovered != hovered
    {
        pop.hovered = hovered;
        ctx.request_repaint();
    }
    let (pressed, pointer, mods) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos(), i.modifiers));
    if let Some(i) = picked {
        let held = crate::tool_modifiers::Held::from_modifiers(mods);
        let (add, toggle) = crate::tool_modifiers::selection_click(held, app.preferences.selection_shift_add_only);
        let _ = choose(app, i, add, toggle);
    } else if pressed && !pointer.is_some_and(|q| resp.response.rect.contains(q)) {
        // A press elsewhere only closes the list.
        close(app);
        app.viewport.select_other.held = true;
    }
}

#[cfg(test)]
#[path = "select_other_tests.rs"]
mod tests;
