//! More interactive sketch tools: projection and other model-pick tools, curve tools (trim,
//! extend, break, offset, fillet, chamfer), multi-pick tools (mirror, patterns, polygon), point
//! tools for free-form curves and slots, text placement, and constraint glyphs. Like `tools`,
//! these are UI state only: every action runs a command.

use crate::icons::markers::glyph_icon;
use crate::icons::markers::glyph_letter;
use std::cell::{Cell, RefCell};

use egui::{Pos2, Stroke};
use serde_json::{Value, json};
use solidcraft_engine::geom::{Vec2, Vec3};

use crate::SolveApp;
use crate::tools::{Kind, Tool};
use crate::viewport::{Hit, Proj, pick, sketch_point_at};

thread_local! {
    /// Shift held this frame.
    static SHIFT: Cell<bool> = const { Cell::new(false) };
    /// Glyphs drawn this frame (constraint id, screen centre) and the selected one.
    static GLYPHS: RefCell<Vec<(String, Pos2)>> = const { RefCell::new(Vec::new()) };
    static PICKED: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The inference the cursor snapped to last (label, sketch point), for the hint.
    static SNAP: RefCell<Option<(&'static str, Vec2)>> = const { RefCell::new(None) };
    static PREV: Cell<Option<Vec2>> = const { Cell::new(None) };
    /// Curvature comb teeth (foot, tip) shown while the comb tool is active.
    static COMBS: RefCell<Vec<Vec<[Vec3; 2]>>> = const { RefCell::new(Vec::new()) };
    /// The image file a Canvas or Decal tool places.
    static IMAGE: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Analysis markers shown while an analysis tool is active: (where, label).
    static MARKS: RefCell<Vec<(Vec3, String)>> = const { RefCell::new(Vec::new()) };
    /// Isocurve analysis polylines (as segments) while the tool is active.
    static ISO: RefCell<Vec<Vec<[Vec3; 2]>>> = const { RefCell::new(Vec::new()) };
    /// Center of Mass has just started: measure everything on the next frame.
    static DEFERRED: Cell<bool> = const { Cell::new(false) };
}

/// Commands registered on one panel that Fusion also lists on another: the analyses (sketch
/// INSPECT) on SOLID INSPECT, and Interference (SOLID INSPECT) on sketch INSPECT.
const ALSO_IN: &[(&str, &str, &str)] = &[
    ("SOLID", "INSPECT", "inspect.zebra"),
    ("SOLID", "INSPECT", "inspect.draft"),
    ("SOLID", "INSPECT", "inspect.curvature_map"),
    ("SOLID", "INSPECT", "inspect.environment_map"),
    ("SOLID", "INSPECT", "inspect.accessibility"),
    ("SOLID", "INSPECT", "inspect.minimum_radius"),
    ("SOLID", "INSPECT", "inspect.isocurve"),
    ("SOLID", "INSPECT", "inspect.curvature_comb"),
    ("SOLID", "INSPECT", "inspect.center_of_mass"),
    ("SKETCH", "INSPECT", "inspect.interference"),
];

/// A panel's commands plus the ones it shares with another panel (see [`ALSO_IN`]).
pub fn also_in<'a>(
    tab: &str,
    panel: &str,
    mut cmds: Vec<&'a solidcraft_engine::CommandSpec>,
    specs: &[&'a solidcraft_engine::CommandSpec],
) -> Vec<&'a solidcraft_engine::CommandSpec> {
    for (t, p, id) in ALSO_IN {
        if *t == tab
            && *p == panel
            && !cmds.iter().any(|c| c.id == *id)
            && let Some(c) = specs.iter().find(|c| c.id == *id)
        {
            cmds.push(c);
        }
    }
    cmds
}

/// Run an analysis and keep its result as a marker.
fn analysis(app: &mut SolveApp, cmd: &str, p: Value) {
    let Ok(v) = app.run(cmd, p) else { return };
    let mark = match cmd {
        "inspect.center_of_mass" => v3(&v["center"]).map(|c| (c, "Center of mass".to_string())),
        _ => v3(&v["at"]).zip(v["min_radius"].as_f64()).map(|(c, r)| (c, format!("R min {}", crate::prefs::show_mm(app, r, 1)))),
    };
    if let Some(m) = mark {
        MARKS.with(|k| k.borrow_mut().push(m));
    } else if cmd == "inspect.minimum_radius" {
        app.set_status("Minimum radius: it is straight (no curvature)", false);
    }
}

/// A seam from an `inspect.continuity` reply as a marker label: its class and the measure that
/// decided it (the gap, the angle between the faces' normals, or the curvature jump).
fn continuity_label(app: &SolveApp, seam: &Value) -> String {
    let deg = |key: &str| seam[key]["value"].as_f64().map(|x| format!("{x:.3}\u{b0}")).unwrap_or_default();
    let jump = seam["curvature_jump_per_mm"]["value"].as_f64().map(|k| format!("\u{394}k {k:.4}/mm")).unwrap_or_default();
    match seam["verdict"].as_str().unwrap_or_default() {
        "gap" => format!("Gap {}", seam["gap_mm"]["value"].as_f64().map(|g| crate::prefs::show_mm(app, g, 1)).unwrap_or_default()),
        "sharp" => format!("G0 {}", deg("normal_angle_deg")),
        "curvature_jump" => format!("G1 {} {jump}", deg("normal_angle_deg")),
        "smooth" => format!("G2 {} {jump}", deg("normal_angle_deg")),
        other => other.to_string(),
    }
}

/// The Continuity Analysis legend in the viewport's lower left corner: each class in its
/// colour, while seams are shaded.
fn continuity_legend(app: &SolveApp, painter: &egui::Painter, proj: &Proj) {
    if app.session.continuity.is_empty() {
        return;
    }
    let tk = crate::theme::Tokens::get();
    let [smooth, jump, sharp, gap] = tk.continuity;
    let tr = crate::i18n::tr_label;
    let rows = [(tr("G2 curvature"), smooth), (tr("G1 tangent"), jump), (tr("G0 crease"), sharp), (tr("Gap"), gap)];
    let font = egui::FontId::proportional(12.0);
    let (row, pad, swatch) = (18.0, 8.0, 22.0);
    let text_width = rows.iter().map(|(label, _)| painter.layout_no_wrap(label.to_string(), font.clone(), tk.text).size().x).fold(0.0, f32::max);
    let size = egui::vec2(pad + swatch + 8.0 + text_width + pad, rows.len() as f32 * row + 2.0 * pad);
    let frame = egui::Rect::from_min_size(proj.rect.left_bottom() + egui::vec2(16.0, -(size.y + 16.0)), size);
    painter.rect_filled(frame, 4.0, tk.panel.gamma_multiply(0.9));
    crate::scenario::publish_rect("continuity_legend", frame);
    for (i, (label, col)) in rows.iter().enumerate() {
        let y = frame.top() + pad + row * (i as f32 + 0.5);
        painter.line_segment([egui::pos2(frame.left() + pad, y), egui::pos2(frame.left() + pad + swatch, y)], Stroke::new(3.0, *col));
        painter.text(egui::pos2(frame.left() + pad + swatch + 8.0, y), egui::Align2::LEFT_CENTER, *label, font.clone(), tk.text);
    }
}

/// Isocurve analysis lines.
fn iso_lines(app: &SolveApp, painter: &egui::Painter, proj: &Proj) {
    if app.tool.as_ref().map(|t| t.cmd) != Some("inspect.isocurve") {
        ISO.with(|i| i.borrow_mut().clear());
        return;
    }
    let tk = crate::theme::Tokens::get();
    ISO.with(|i| {
        for c in i.borrow().iter() {
            for [a, b] in c {
                if let (Some(p), Some(q)) = (proj.to_screen(*a), proj.to_screen(*b)) {
                    painter.line_segment([p, q], Stroke::new(1.2, tk.accent));
                }
            }
        }
    });
}

/// Analysis markers: a target with its label.
fn marks(app: &SolveApp, painter: &egui::Painter, proj: &Proj) {
    if !app.tool.as_ref().is_some_and(|t| matches!(t.cmd, "inspect.minimum_radius" | "inspect.center_of_mass" | "inspect.continuity")) {
        MARKS.with(|m| m.borrow_mut().clear());
        return;
    }
    let tk = crate::theme::Tokens::get();
    MARKS.with(|m| {
        for (at, label) in m.borrow().iter() {
            let Some(q) = proj.to_screen(*at) else { continue };
            painter.circle(q, 6.0, tk.panel.gamma_multiply(0.8), Stroke::new(1.6, tk.accent));
            painter.line_segment([q - egui::vec2(10.0, 0.0), q + egui::vec2(10.0, 0.0)], Stroke::new(1.2, tk.accent));
            painter.line_segment([q - egui::vec2(0.0, 10.0), q + egui::vec2(0.0, 10.0)], Stroke::new(1.2, tk.accent));
            let r = painter.text(q + egui::vec2(12.0, -12.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(12.0), tk.text);
            painter.rect_filled(r.expand(3.0), 3.0, tk.panel.gamma_multiply(0.85));
            painter.text(q + egui::vec2(12.0, -12.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(12.0), tk.text);
        }
    });
}

/// Commands that start with a step of their own: Canvas and Decal ask for the image first,
/// Edit Canvas opens the canvas panel. False when the command should not go on.
pub fn start_hook(app: &mut SolveApp, id: &str) -> bool {
    if id == "sketch.edit_text"
        && let Some(dialog) = crate::dialogs::Dialog::for_command(app, id)
    {
        app.dialog = Some(dialog);
        return false;
    }
    // Surface analyses toggle: a second click turns the same one off.
    use solidcraft_engine::SurfaceAnalysis as A;
    let same = match (id, app.session.analysis) {
        ("inspect.zebra", Some(A::Zebra { .. })) => Some(true),
        ("inspect.draft", Some(A::Draft { .. })) => Some(true),
        ("inspect.curvature_map", Some(A::Curvature { .. })) => Some(true),
        ("inspect.environment_map", Some(A::Environment)) => Some(true),
        ("inspect.accessibility", Some(A::Access { .. })) => Some(true),
        ("inspect.zebra" | "inspect.draft" | "inspect.curvature_map" | "inspect.environment_map" | "inspect.accessibility", _) => Some(false),
        _ => None,
    };
    if let Some(on) = same {
        let p = if on { json!({"clear": true}) } else { json!({}) };
        if app.run(id, p).is_ok() {
            app.set_status(if on { "Analysis off" } else { "Analysis on (click it again to turn it off)" }, false);
        }
        return false;
    }
    if id == "inspect.continuity" {
        // Every edge of the shown bodies at once; the tool stays for clicking single edges. With
        // seams shaded, the button turns the analysis off.
        MARKS.with(|m| m.borrow_mut().clear());
        if !app.session.continuity.is_empty() {
            if app.run(id, json!({"clear": true})).is_ok() {
                app.set_status("Analysis off", false);
            }
            return false;
        }
        let Ok(v) = app.run(id, json!({})) else { return false };
        let summary = &v["summary"];
        let counts: Vec<String> = [("smooth", "G2"), ("curvature_jump", "G1"), ("sharp", "G0"), ("gap", "gap")]
            .iter()
            .filter_map(|(key, class)| summary[*key].as_u64().filter(|n| *n > 0).map(|n| format!("{n} {class}")))
            .collect();
        app.set_status(format!("Continuity: {} (click the button again to turn it off)", counts.join(", ")), false);
        return true;
    }
    if id == "inspect.center_of_mass" {
        // The whole model at once; the tool stays for picking single bodies.
        MARKS.with(|m| m.borrow_mut().clear());
        DEFERRED.with(|d| d.set(true));
        return true;
    }
    if id == "canvas.edit" {
        // The browser's canvas panel (one panel for canvases).
        if let Some(first) = app.session.doc.canvases.first().map(|c| c.id) {
            app.tree.canvas_panel = Some(crate::browser::CanvasPanel::new(app, first, false));
        }
        return false;
    }
    if !matches!(id, "canvas.insert" | "canvas.decal") {
        return true;
    }
    let command = id.to_string();
    app.pick_open(move |app, path| {
        if let Some(path) = path {
            IMAGE.with(|i| *i.borrow_mut() = Some(path));
            app.last_command = solidcraft_engine::find_command(&command).map(|spec| (spec.id.into(), spec.label.into()));
            app.tool = crate::tools::Tool::for_command(&command);
        }
    });
    false
}

fn v3(v: &Value) -> Option<Vec3> {
    Some(Vec3::new(v.get(0)?.as_f64()?, v.get(1)?.as_f64()?, v.get(2)?.as_f64()?))
}

/// Curvature combs: teeth and the envelope through their tips.
fn combs(app: &SolveApp, painter: &egui::Painter, proj: &Proj) {
    if app.tool.as_ref().map(|t| t.cmd) != Some("inspect.curvature_comb") {
        COMBS.with(|c| c.borrow_mut().clear());
        return;
    }
    let tk = crate::theme::Tokens::get();
    COMBS.with(|c| {
        for comb in c.borrow().iter() {
            let mut tips: Vec<Pos2> = Vec::new();
            for [a, b] in comb {
                let (Some(p), Some(q)) = (proj.to_screen(*a), proj.to_screen(*b)) else { continue };
                painter.line_segment([p, q], Stroke::new(1.0, tk.warning.gamma_multiply(0.8)));
                tips.push(q);
            }
            painter.add(egui::Shape::line(tips, Stroke::new(1.4, tk.warning)));
        }
    });
}

/// Snap a cursor point on the sketch plane to what Fusion infers while drawing: a line's
/// midpoint, or alignment (horizontal/vertical) with the previous point of the tool. None:
/// nothing near, use the grid.
pub fn refine_snap(
    app: &SolveApp,
    proj: &Proj,
    sk: &solidcraft_engine::sketch::Sketch,
    plane: &solidcraft_engine::geom::Plane,
    lp: Vec2,
) -> Option<(Vec2, Option<String>)> {
    crate::inference::clear();
    // Millimetres per pixel here.
    let (a, b) = (proj.to_screen(plane.to_world(lp))?, proj.to_screen(plane.to_world(lp + Vec2::X))?);
    let px = 1.0 / (a.distance(b) as f64).max(1e-9);
    let tol = 7.0 * px;
    // A model vertex under the cursor: drawing on it projects it into the sketch.
    for h in pick(app, proj, a) {
        if let Hit::Vertex { point, .. } = h {
            SNAP.with(|s| *s.borrow_mut() = Some(("Pt", plane.to_local(point))));
            return Some((plane.to_local(point), Some(format!("vertex:{},{},{}", point.x, point.y, point.z))));
        }
    }
    let mut best: Option<(f64, &'static str, Vec2)> = None;
    let mut mid_of: Option<String> = None;
    for (i, c) in sk.curves.iter().enumerate() {
        if let solidcraft_engine::sketch::CurveKind::Line { .. } = c.kind
            && let Some(solidcraft_engine::sketch::Shape::Line { a, b }) = sk.shape(i)
        {
            let m = (a + b) * 0.5;
            let d = m.dist(lp);
            if d < tol && best.is_none_or(|x| d < x.0) {
                best = Some((d, "Mid", m));
                mid_of = Some(c.id.clone());
            }
        }
    }
    // The tool's previous point (a click takes the tool out of the app while it runs, so the
    // last hover's is kept).
    let prev = match app.tool.as_ref() {
        Some(t) => {
            let p = t.pts.last().map(|p| p.0);
            PREV.with(|c| c.set(p));
            p
        }
        None => PREV.with(Cell::get),
    };
    // Line tool: the tangent point of a circle or arc (before plain "on the curve").
    let line_tool = app.tool.as_ref().is_none_or(|t| t.cmd == "sketch.line");
    if best.is_none()
        && line_tool
        && let Some(prev) = prev
        && let Some(i) = crate::inference::tangent(sk, prev, lp, tol)
    {
        best = Some((i.dist, i.label(), i.at));
    }
    // On a curve (coincident with it).
    if best.is_none() {
        for (i, c) in sk.curves.iter().enumerate() {
            let Some(sh) = sk.shape(i) else { continue };
            let d = sh.dist(lp);
            if d < tol * 0.8 && best.is_none_or(|x| d < x.0) {
                let q = sh.project(lp);
                best = Some((d, "On", q));
                mid_of = Some(format!("{}:{},{}", c.id, q.x, q.y));
            }
        }
    }
    if best.is_none()
        && let Some(prev) = prev
    {
        let (dx, dy) = ((lp.x - prev.x).abs(), (lp.y - prev.y).abs());
        // The free coordinate still follows the grid.
        let step = solidcraft_engine::view::grid_step(app.cam.half_height()).0 / 10.0;
        let g = |v: f64| (v / step).round() * step;
        if dy < tol && dx > tol {
            best = Some((dy, "H", Vec2::new(g(lp.x), prev.y)));
        } else if dx < tol && dy > tol {
            best = Some((dx, "V", Vec2::new(prev.x, g(lp.y))));
        }
    }
    // Line tool: parallel or perpendicular to a line, or on a line's extension.
    if best.is_none()
        && line_tool
        && let Some(prev) = prev
        && let Some(i) = crate::inference::lines(sk, prev, lp, tol)
    {
        best = Some((i.dist, i.label(), i.at));
    }
    SNAP.with(|s| *s.borrow_mut() = best.map(|(_, l, p)| (l, p)));
    // A midpoint snap is a point argument the commands understand ("mid:<line>").
    best.map(|(_, l, p)| {
        let arg = match l {
            "Mid" => mid_of.map(|m| format!("mid:{m}")),
            "On" => mid_of.map(|m| format!("on:{m}")),
            _ => None,
        };
        (p, arg)
    })
}

/// The constraint glyph under a screen position (its constraint id).
pub fn glyph_at(pos: Pos2) -> Option<String> {
    GLYPHS.with(|g| g.borrow().iter().find(|(_, c)| c.distance(pos) <= GLYPH * 0.6).map(|(id, _)| id.clone()))
}

/// Constraint glyphs drawn last frame: (constraint id, screen centre).
pub fn glyph_positions() -> Vec<(String, Pos2)> {
    GLYPHS.with(|g| g.borrow().clone())
}

/// A click on a constraint glyph selects it (true: the click was used).
pub fn click_glyph(app: &mut SolveApp, _proj: &Proj, pos: Pos2) -> bool {
    if app.session.active_sketch.is_none() {
        return false;
    }
    let hit = GLYPHS.with(|g| g.borrow().iter().find(|(_, c)| c.distance(pos) <= GLYPH * 0.6).map(|(id, _)| id.clone()));
    let used = hit.is_some();
    PICKED.with(|p| *p.borrow_mut() = hit.clone());
    if let Some(id) = hit {
        app.set_status(format!("Constraint {id} selected: Delete removes it"), false);
    }
    used
}

/// Delete the selected constraint glyph (true when one was selected).
pub fn delete_glyph(app: &mut SolveApp) -> bool {
    let Some(id) = PICKED.with(|p| p.borrow_mut().take()) else { return false };
    let _ = app.run("sketch.delete", json!({"entities": [id]}));
    true
}

/// What the extra tools do with clicks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Each click on model geometry runs the command with that reference.
    ModelRef,
    /// Click a curve of the active sketch where it should change.
    CurveAt,
    /// Click points; `n` complete the command (0: until right-click).
    Points(usize),
    /// Pick sketch entities; `n` run the command (0: until right-click).
    Entities(usize),
    /// Model faces / sketch curves in steps (see the command).
    Steps,
}

/// Whether a click with tool `id` acts on `hit`: what is under the pointer lights up only when
/// the tool takes it, so the hover shows what a click does and lights nothing over a view the
/// tool draws (Continuity Analysis's seam colours on a face the tool ignores).
pub fn takes(id: &str, hit: &Hit) -> bool {
    match id {
        "inspect.continuity" => matches!(hit, Hit::Edge { .. }),
        _ => true,
    }
}

fn mode(id: &str) -> Option<Mode> {
    Some(match id {
        "sketch.project" | "sketch.intersect" | "sketch.include_3d" | "sketch.fit_curves_to_section" | "sketch.isoparametric_curve" => Mode::ModelRef,
        "inspect.curvature_comb" | "canvas.insert" | "canvas.decal" => Mode::ModelRef,
        "inspect.minimum_radius" | "inspect.center_of_mass" | "inspect.isocurve" | "inspect.continuity" => Mode::ModelRef,
        "sketch.trim" | "sketch.extend" | "sketch.break" => Mode::CurveAt,
        "sketch.line.midpoint" => Mode::Points(2),
        "sketch.arc.tangent" => Mode::Points(2),
        "sketch.ellipse" | "sketch.conic" | "sketch.slot.center_point" => Mode::Points(3),
        "sketch.slot.arc_three_point" | "sketch.slot.arc_center" => Mode::Points(4),
        "sketch.spline.fit_point" | "sketch.spline.control_point" | "sketch.spline.control_point_5" => Mode::Points(0),
        "sketch.text" => Mode::Points(1),
        "sketch.edit_text" => Mode::Entities(1),
        "sketch.blend_curve" => Mode::Points(2),
        "sketch.constraint.curvature" | "sketch.circle.two_tangent" => Mode::Entities(2),
        "sketch.circle.three_tangent" => Mode::Entities(3),
        "sketch.constrainer" | "sketch.constraint.polygon" => Mode::Entities(0),
        "sketch.centerline" => Mode::Entities(1),
        "sketch.project_to_surface" | "sketch.intersection_curve" | "sketch.spun_profile" => Mode::Steps,
        _ => return None,
    })
}

/// A tool for one of the extra sketch commands.
pub fn tool_for(id: &str) -> Option<Tool> {
    mode(id)?;
    let spec = solidcraft_engine::find_command(id)?;
    Some(Tool {
        cmd: spec.id,
        kind: Kind::Ext,
        pts: Vec::new(),
        hover: None,
        picks: Vec::new(),
        dims: Vec::new(),
        dim_history: Vec::new(),
        dims_stage: usize::MAX,
        dims_focus: false,
    })
}

pub fn hint(id: &str) -> Option<String> {
    Some(
        match id {
            "sketch.project" => "Project: click edges, faces, vertices, other sketches' curves, axes or planes",
            "sketch.intersect" => "Intersect: click faces or edges to cut with the sketch plane",
            "sketch.include_3d" => "Include 3D Geometry: click edges or vertices",
            "sketch.fit_curves_to_section" => "Fit Curves to Mesh Section: click a body",
            "sketch.isoparametric_curve" => "Isoparametric Curve: click a point on a face (Shift: along)",
            "inspect.curvature_comb" => "Curvature comb: click sketch curves or model edges",
            "inspect.minimum_radius" => "Minimum radius: click sketch curves, edges or faces",
            "inspect.center_of_mass" => "Center of mass of all bodies; click a body for its own",
            "inspect.isocurve" => "Isocurves: click faces",
            "inspect.continuity" => "Continuity: click an edge for its numbers (the button again turns the colours off)",
            "canvas.insert" => "Canvas: click a plane or a planar face where the image's centre goes",
            "canvas.decal" => "Decal: click a planar face where the image's centre goes",
            "sketch.trim" => "Trim: click the piece of a curve to remove",
            "sketch.extend" => "Extend: click a curve near the end to extend",
            "sketch.break" => "Break: click a curve where it should split",
            "sketch.line.midpoint" => "Midpoint line: click the middle, then an end",
            "sketch.arc.tangent" => "Tangent arc: click the end of a line or arc, then the arc's end",
            "sketch.ellipse" => "Ellipse: click the centre, the end of the major axis, then a point on it",
            "sketch.conic" => "Conic: click the start, the end, then the apex; type Rho to shape it (0.5 is a parabola)",
            "sketch.slot.center_point" => "Slot: click the centre, an arc centre, then the width",
            "sketch.slot.arc_three_point" => "Arc slot: click start, a point on the arc, the end, then the width",
            "sketch.slot.arc_center" => "Arc slot: click the centre, the start, the end, then the width",
            "sketch.spline.fit_point" => "Spline: click fit points; click the start to close smoothly, or right-click to finish",
            "sketch.spline.control_point" | "sketch.spline.control_point_5" => "Control point spline: click control points; right-click to finish",
            "sketch.text" => "Text: click where the text starts",
            "sketch.edit_text" => "Edit Text: click a letter interior or outline",
            "sketch.blend_curve" => "Blend curve: click the ends of two curves",
            "sketch.constraint.curvature" => "Curvature: click two curves that share an end",
            "sketch.circle.two_tangent" => "2-tangent circle: click two curves (the circle goes near the second click)",
            "sketch.circle.three_tangent" => "3-tangent circle: click three curves",
            "sketch.constrainer" => "Constrain: click one or two entities; right-click to apply",
            "sketch.constraint.polygon" => "Polygon: click the lines of a closed chain; right-click to apply",
            "sketch.centerline" => "Centerline: click lines",
            "sketch.project_to_surface" => "Project to surface: click curves of other sketches, right-click, then click the face",
            "sketch.intersection_curve" => "Intersection curve: click two faces",
            "sketch.spun_profile" => "Spun profile: click the body, then the axis line",
            _ => return None,
        }
        .to_string(),
    )
}

fn active_sketch(app: &SolveApp) -> Option<u64> {
    app.session.active_sketch
}

/// A model reference for a hit (for Project and friends).
fn model_ref(app: &SolveApp, h: &Hit) -> Option<Value> {
    let active = active_sketch(app);
    Some(match h {
        Hit::Vertex { body, point } => json!({"vertex": [point.x, point.y, point.z], "body": body}),
        Hit::Edge { body, mid, .. } => json!({"edge": [mid.x, mid.y, mid.z], "body": body}),
        Hit::Face { body, point, .. } => json!({"face": [point.x, point.y, point.z], "body": body}),
        Hit::SketchCurve { sketch, id, .. } if Some(*sketch) != active => json!({"sketch": sketch, "curve": id}),
        Hit::SketchPoint { sketch, id, .. } if Some(*sketch) != active => json!({"sketch": sketch, "point": id}),
        Hit::Axis { name } => json!({"axis": name}),
        Hit::Plane { name, .. } => json!({"plane": name}),
        _ => return None,
    })
}

/// The active sketch's curve or point under the cursor.
fn sketch_entity(app: &SolveApp, hits: &[Hit]) -> Option<(String, bool)> {
    let active = active_sketch(app)?;
    hits.iter().find_map(|h| match h {
        Hit::SketchPoint { sketch, id, .. } if *sketch == active => Some((id.clone(), true)),
        Hit::SketchCurve { sketch, id, .. } if *sketch == active => Some((id.clone(), false)),
        _ => None,
    })
}

/// The two editable curves that meet at sketch point `pi`, in curve order, which is the order
/// the corner commands resolve them in from a `point` parameter. Construction curves are
/// ignored when more than two meet. Each side must be a line, or an arc for a fillet.
pub(crate) fn corner_curves(sk: &solidcraft_engine::sketch::Sketch, pi: usize, arcs_ok: bool) -> Option<(usize, usize)> {
    use solidcraft_engine::sketch::CurveKind;
    let candidates =
        sk.curves.iter().enumerate().filter(|(ci, c)| !sk.is_linked_curve(*ci) && c.kind.ends().is_some_and(|(a, b)| a == pi || b == pi));
    let mut curves: Vec<_> = candidates.clone().take(3).collect();
    // Center rectangles have construction diagonals at their corners. Match the engine's
    // corner selection: when more than two curves meet, use the profile sides.
    if curves.len() > 2 {
        curves = candidates.filter(|(_, c)| !c.construction).take(3).collect();
    }
    if curves.iter().any(|(_, c)| !matches!(c.kind, CurveKind::Line { .. }) && !(arcs_ok && matches!(c.kind, CurveKind::Arc { .. }))) {
        return None;
    }
    match curves[..] {
        [(a, _), (b, _)] => Some((a, b)),
        _ => None,
    }
}

/// Corner tools accept endpoints shared by two editable curves. Pick among those endpoints
/// directly so the origin, a circle centre or an overlapping loose point cannot hide a corner.
pub(crate) fn corner_point(app: &SolveApp, proj: &Proj, pos: Pos2, arcs_ok: bool) -> Option<(Vec2, String)> {
    let st = app.session.world_state();
    let ss = st.sketch(active_sketch(app)?)?;
    let sk = &ss.sketch;
    sk.points
        .iter()
        .enumerate()
        .filter_map(|(pi, point)| {
            let distance = proj.to_screen(ss.plane.to_world(point.pos))?.distance(pos);
            if distance >= 8.0 {
                return None;
            }
            corner_curves(sk, pi, arcs_ok)?;
            Some((distance, point.pos, point.id.clone()))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, at, id)| (at, id))
}

fn xy(p: Vec2) -> Value {
    json!([p.x, p.y])
}

fn arg(p: &(Vec2, Option<String>)) -> Value {
    match &p.1 {
        Some(id) => json!(id),
        None => xy(p.0),
    }
}

/// Length of a curve (for default sizes).
fn curve_len(app: &SolveApp, id: &str) -> f64 {
    let st = app.session.world_state();
    let Some(ss) = active_sketch(app).and_then(|s| st.sketch(s)) else { return 10.0 };
    ss.sketch.curve_index(id).map(|c| ss.sketch.polyline(c).windows(2).map(|w| w[0].dist(w[1])).sum::<f64>()).unwrap_or(10.0)
}

fn round_nice(x: f64) -> f64 {
    if !(x.is_finite() && x > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(x.log10().floor());
    let m = x / p;
    let n = if m < 1.5 {
        1.0
    } else if m < 3.5 {
        2.0
    } else if m < 7.5 {
        5.0
    } else {
        10.0
    };
    n * p
}

/// A click with an extra tool.
pub fn periodic_start_snap(app: &SolveApp, tool: &Tool, proj: &Proj, pos: Pos2) -> Option<Vec2> {
    if tool.cmd != "sketch.spline.fit_point" || tool.pts.len() < 3 || crate::drag_snap::suppressed() {
        return None;
    }
    let first = tool.pts.first()?.0;
    let mut distinct = Vec::new();
    for p in &tool.pts {
        if !distinct.contains(&p.0) {
            distinct.push(p.0);
        }
        if distinct.len() == 3 {
            break;
        }
    }
    if distinct.len() < 3 {
        return None;
    }
    let state = app.session.world_state();
    let plane = &state.sketch(app.session.active_sketch?)?.plane;
    let screen = proj.to_screen(plane.to_world(first))?;
    (screen.distance(pos) <= 8.0).then_some(first)
}

pub fn on_click(app: &mut SolveApp, tool: &mut Tool, proj: &Proj, pos: Pos2) {
    let Some(m) = mode(tool.cmd) else { return };
    let hits = pick(app, proj, pos);
    let at = sketch_point_at(app, proj, pos);
    let cmd = tool.cmd;
    match m {
        Mode::ModelRef if cmd == "inspect.curvature_comb" => {
            let params = hits.iter().find_map(|h| match h {
                Hit::SketchCurve { sketch, id, .. } => Some(json!({"sketch": sketch, "curves": [id]})),
                Hit::Edge { mid, .. } => Some(json!({"edges": [[mid.x, mid.y, mid.z]]})),
                _ => None,
            });
            if let Some(p) = params
                && let Ok(v) = app.run(cmd, p)
            {
                for comb in v["combs"].as_array().into_iter().flatten() {
                    let teeth: Vec<[Vec3; 2]> =
                        comb["teeth"].as_array().into_iter().flatten().filter_map(|t| Some([v3(t.get(0)?)?, v3(t.get(1)?)?])).collect();
                    COMBS.with(|c| c.borrow_mut().push(teeth));
                }
            }
        }
        Mode::ModelRef if cmd == "inspect.isocurve" => {
            let face = hits.iter().find_map(|h| if let Hit::Face { point, .. } = h { Some(*point) } else { None });
            if let Some(at) = face
                && let Ok(v) = app.run(cmd, json!({"faces": [[at.x, at.y, at.z]]}))
            {
                for c in v["curves"].as_array().into_iter().flatten() {
                    let pts: Vec<Vec3> = c.as_array().into_iter().flatten().filter_map(v3).collect();
                    let teeth: Vec<[Vec3; 2]> = pts.windows(2).map(|w| [w[0], w[1]]).collect();
                    ISO.with(|i| i.borrow_mut().push(teeth));
                }
            }
        }
        Mode::ModelRef if cmd == "inspect.continuity" => {
            // The edge measured again, added to the seams shown, and its numbers pinned there.
            let edge = hits.iter().filter(|h| takes(cmd, h)).find_map(|h| if let Hit::Edge { mid, .. } = h { Some(*mid) } else { None });
            if let Some(mid) = edge
                && let Ok(v) = app.run(cmd, json!({"edges": [[mid.x, mid.y, mid.z]], "add": true}))
                && let Some(seam) = v["bodies"][0]["edges"].get(0)
            {
                let label = continuity_label(app, seam);
                MARKS.with(|m| m.borrow_mut().push((mid, label)));
            }
        }
        Mode::ModelRef if matches!(cmd, "inspect.minimum_radius" | "inspect.center_of_mass") => {
            let params = hits.iter().find_map(|h| match (cmd, h) {
                ("inspect.minimum_radius", Hit::SketchCurve { sketch, id, .. }) => Some(json!({"sketch": sketch, "curves": [id]})),
                ("inspect.minimum_radius", Hit::Edge { mid, .. }) => Some(json!({"edges": [[mid.x, mid.y, mid.z]]})),
                ("inspect.minimum_radius", Hit::Face { point, .. }) => Some(json!({"faces": [[point.x, point.y, point.z]]})),
                ("inspect.center_of_mass", Hit::Face { body, .. }) => Some(json!({"bodies": [body]})),
                _ => None,
            });
            if let Some(p) = params {
                analysis(app, cmd, p);
            }
        }
        Mode::ModelRef if matches!(cmd, "canvas.insert" | "canvas.decal") => {
            let Some(path) = IMAGE.with(|i| i.borrow().clone()) else { return };
            let decal = cmd == "canvas.decal";
            let target = hits.iter().find_map(|h| match h {
                Hit::Face { point, .. } => Some((json!({"face": [point.x, point.y, point.z]}), *point)),
                Hit::Plane { name, point } if !decal => Some((json!(name), *point)),
                _ => None,
            });
            // Empty space while sketching: the sketch plane.
            let target = target.or_else(|| {
                let st = app.session.world_state();
                let ss = st.sketch(app.session.active_sketch?)?;
                let (o, d) = proj.ray(pos);
                let w = ss.plane.intersect_ray(o, d)?;
                let pl = &ss.plane;
                Some((
                    json!({"origin": [pl.origin.x, pl.origin.y, pl.origin.z], "x_dir": [pl.x.x, pl.x.y, pl.x.z], "y_dir": [pl.y.x, pl.y.y, pl.y.z]}),
                    w,
                ))
            });
            let Some((plane, at)) = target else { return };
            let p = if decal {
                json!({"path": path, "face": plane.get("face").cloned().unwrap_or(Value::Null)})
            } else {
                json!({"path": path, "plane": plane, "at": [at.x, at.y, at.z]})
            };
            if let Ok(v) = app.run(cmd, p) {
                IMAGE.with(|i| *i.borrow_mut() = None);
                app.tool = None;
                if let Some(id) = v["canvas"].as_u64() {
                    app.tree.canvas_panel = Some(crate::browser::CanvasPanel::new(app, id, false));
                }
            }
        }
        Mode::ModelRef => {
            let r = hits.iter().find_map(|h| model_ref(app, h));
            let Some(r) = r else { return };
            match cmd {
                "sketch.fit_curves_to_section" => {
                    if let Some(b) = r.get("body").and_then(Value::as_str) {
                        let _ = app.run(cmd, json!({"body": b}));
                    }
                }
                "sketch.isoparametric_curve" => {
                    if r.get("face").is_some() {
                        let along = SHIFT.with(Cell::get);
                        let _ = app.run(cmd, json!({"face": r["face"], "body": r["body"], "direction": if along { "v" } else { "u" }}));
                    }
                }
                _ => {
                    let _ = app.run(cmd, json!({"refs": [r]}));
                }
            }
        }
        Mode::CurveAt => {
            let curve = active_sketch(app).and_then(|active| {
                hits.iter().find_map(|hit| match hit {
                    Hit::SketchCurve { sketch, id, .. } if *sketch == active => Some(id.clone()),
                    _ => None,
                })
            });
            if let (Some(id), Some((p, _))) = (curve, at) {
                let _ = app.run(cmd, json!({"curve": id, "at": xy(p)}));
            }
        }
        Mode::Points(_) => {
            if periodic_start_snap(app, tool, proj, pos).is_some() {
                // Only this deliberate click selects periodic interpolation. A
                // right-click finish and all previously saved splines stay natural.
                let params = json!({"points":tool.pts.iter().map(arg).collect::<Vec<_>>(),"periodic":true});
                if app.run(cmd, params).is_ok() {
                    tool.pts.clear();
                    crate::sketch_dims::reset(tool);
                }
                return;
            }
            let Some(p) = at else { return };
            place_point(app, tool, p);
        }
        Mode::Entities(n) => {
            if cmd == "sketch.edit_text" {
                let selection = hits.first().and_then(|hit| match hit {
                    Hit::SketchCurve { sketch, id, .. } if Some(*sketch) == active_sketch(app) => {
                        Some(solidcraft_engine::Sel::SketchCurve { id: id.clone() })
                    }
                    Hit::Profile { sketch, index } => Some(solidcraft_engine::Sel::Profile { sketch: *sketch, index: *index }),
                    _ => None,
                });
                if let Some(id) = selection.as_ref().and_then(|sel| crate::sketch_text::entity_for_selection(app, sel)) {
                    crate::sketch_text::open(app, &id);
                }
                return;
            }
            let Some((id, _)) = sketch_entity(app, &hits) else { return };
            if cmd == "sketch.centerline" {
                let _ = app.run(cmd, json!({"curves": [id]}));
                return;
            }
            if !tool.picks.contains(&id) {
                tool.picks.push(id);
                if let Some((p, _)) = at {
                    tool.pts.push((p, None));
                }
            }
            if n > 0 && tool.picks.len() >= n {
                run_entities(app, tool);
                tool.picks.clear();
                tool.pts.clear();
            }
        }
        Mode::Steps => match cmd {
            "sketch.intersection_curve" => {
                if let Some(r) = hits.iter().find_map(|h| model_ref(app, h)).filter(|r| r.get("face").is_some()) {
                    tool.picks.push(r.to_string());
                    if tool.picks.len() >= 2 {
                        let a: Value = serde_json::from_str(&tool.picks[0]).unwrap_or(Value::Null);
                        let b: Value = serde_json::from_str(&tool.picks[1]).unwrap_or(Value::Null);
                        let _ = app.run(cmd, json!({"a": a, "b": b}));
                        tool.picks.clear();
                    }
                }
            }
            "sketch.spun_profile" => {
                if tool.picks.is_empty() {
                    if let Some(b) =
                        hits.iter().find_map(|h| model_ref(app, h)).and_then(|r| r.get("body").and_then(Value::as_str).map(str::to_string))
                    {
                        tool.picks.push(b);
                    }
                } else if let Some((line, false)) = sketch_entity(app, &hits) {
                    let _ = app.run(cmd, json!({"body": tool.picks[0], "axis": line}));
                    tool.picks.clear();
                }
            }
            _ => {
                // sketch.project_to_surface: curves of other sketches, then (after a right-click) a face.
                let staged = tool.picks.last().is_some_and(|x| x == "|");
                if staged {
                    if let Some(r) = hits.iter().find_map(|h| model_ref(app, h)).filter(|r| r.get("face").is_some()) {
                        let curves: Vec<Value> = tool.picks.iter().filter(|x| *x != "|").filter_map(|x| serde_json::from_str(x).ok()).collect();
                        let _ = app.run(cmd, json!({"curves": curves, "face": r["face"], "body": r["body"]}));
                        tool.picks.clear();
                    }
                } else if let Some(r) = hits.iter().find_map(|h| model_ref(app, h)).filter(|r| r.get("curve").is_some()) {
                    tool.picks.push(r.to_string());
                }
            }
        },
    }
}

/// Put down the next point of a point tool: a click, or Enter in one of its value boxes. The
/// last point runs the command. Typed values are checked first, so a rejected one stays in its
/// box to be corrected rather than reaching the command.
pub(crate) fn place_point(app: &mut SolveApp, tool: &mut Tool, p: (Vec2, Option<String>)) {
    let Some(Mode::Points(n)) = mode(tool.cmd) else { return };
    if let Err(error) = crate::sketch_dims::validate(app, tool) {
        app.set_status(error, true);
        return;
    }
    tool.pts.push(p);
    if n > 0 && tool.pts.len() >= n {
        run_points(app, tool);
        tool.pts.clear();
    }
}

fn run_points(app: &mut SolveApp, t: &Tool) {
    let p = &t.pts;
    let width = |a: Vec2, b: Vec2, c: Vec2| -> f64 { 2.0 * (b - a).normalized().map(|d| d.cross(c - a).abs()).unwrap_or(1.0) };
    let params = match (t.cmd, p.as_slice()) {
        ("sketch.line.midpoint", [m, e]) => json!({"mid": arg(m), "end": xy(e.0)}),
        ("sketch.arc.tangent", [s, e]) => json!({"start": arg(s), "end": xy(e.0)}),
        ("sketch.ellipse", [c, m, q]) => json!({"center": xy(c.0), "major": xy(m.0), "minor": xy(q.0)}),
        ("sketch.conic", [a, b, x]) => {
            // The typed Rho box, or the parabola the command defaults to.
            let rho = crate::sketch_dims::locked_value(app, t, "Rho").unwrap_or(0.5);
            json!({"start": arg(a), "end": arg(b), "apex": arg(x), "rho": rho})
        }
        ("sketch.slot.center_point", [c, e, w]) => json!({"center": xy(c.0), "end": xy(e.0), "width": width(c.0, e.0, w.0).max(1e-3)}),
        ("sketch.slot.arc_three_point", [a, th, b, w]) => {
            json!({"start": xy(a.0), "through": xy(th.0), "end": xy(b.0), "width": (2.0 * w.0.dist(b.0)).max(1e-3)})
        }
        ("sketch.slot.arc_center", [c, a, b, w]) => {
            let r = c.0.dist(a.0);
            json!({"center": xy(c.0), "start": xy(a.0), "end": xy(b.0), "width": (2.0 * (w.0.dist(c.0) - r).abs()).max(1e-3)})
        }
        ("sketch.spline.fit_point" | "sketch.spline.control_point" | "sketch.spline.control_point_5", pts) if pts.len() >= 2 => {
            json!({"points": pts.iter().map(arg).collect::<Vec<_>>()})
        }
        ("sketch.blend_curve", [a, b]) => match (&a.1, &b.1) {
            (Some(x), Some(y)) => json!({"a": x, "b": y}),
            _ => return,
        },
        ("sketch.text", [at]) => {
            crate::sketch_text::place(app, at.0, round_nice(app.cam.half_height() * 0.1));
            return;
        }
        _ => return,
    };
    let _ = app.run(t.cmd, params);
}

fn run_entities(app: &mut SolveApp, t: &Tool) {
    let k = &t.picks;
    let params = match t.cmd {
        "sketch.constraint.curvature" => json!({"a": k[0], "b": k[1]}),
        "sketch.circle.two_tangent" => {
            let r = round_nice(curve_len(app, &k[0]).min(curve_len(app, &k[1])) * 0.1);
            let near = t.pts.last().map(|p| xy(p.0)).unwrap_or(json!([0, 0]));
            json!({"curves": k, "radius": r, "near": near})
        }
        "sketch.circle.three_tangent" => {
            // Start inside the three picks.
            let c = t.pts.iter().fold(Vec2::ZERO, |a, p| a + p.0) / t.pts.len().max(1) as f64;
            json!({"curves": k, "near": xy(c)})
        }
        "sketch.constrainer" => json!({"entities": k}),
        "sketch.constraint.polygon" => json!({"lines": k}),
        _ => return,
    };
    let _ = app.run(t.cmd, params);
}

/// Right-click with an extra tool: finish a chain or move to the next stage. Returns false
/// when the tool should be dropped.
pub fn finish(app: &mut SolveApp, tool: &mut Tool) -> bool {
    let Some(m) = mode(tool.cmd) else { return false };
    match m {
        Mode::Points(0) if tool.pts.len() >= 2 => {
            run_points(app, tool);
            tool.pts.clear();
            true
        }
        Mode::Entities(0) if !tool.picks.is_empty() => {
            run_entities(app, tool);
            tool.picks.clear();
            tool.pts.clear();
            true
        }
        Mode::Steps if !tool.picks.is_empty() && tool.picks.last().is_some_and(|x| x != "|") => {
            tool.picks.push("|".into());
            true
        }
        _ if !tool.pts.is_empty() || !tool.picks.is_empty() => {
            tool.pts.clear();
            tool.picks.clear();
            true
        }
        _ => false,
    }
}

/// Rubber band for point tools and pick markers.
pub fn preview(app: &SolveApp, t: &Tool, painter: &egui::Painter, proj: &Proj) {
    let Some(sid) = active_sketch(app) else { return };
    let st = app.session.world_state();
    let Some(ss) = st.sketch(sid) else { return };
    let to = |p: Vec2| proj.to_screen(ss.plane.to_world(p));
    let tk = crate::theme::Tokens::get();
    let stroke = Stroke::new(1.5, tk.rubber_band);
    if let Some((hp, snap)) = &t.hover
        && let Some(sp) = to(*hp)
    {
        painter.circle_stroke(sp, if snap.is_some() { 6.0 } else { 3.0 }, stroke);
    }
    let faint = Stroke::new(1.0, tk.rubber_band.gamma_multiply(0.6));
    let mode = crate::sketch_palette::mode(app);
    for w in t.pts.windows(2) {
        crate::tools::paint_sketch_path(&[ss.plane.to_world(w[0].0), ss.plane.to_world(w[1].0)], mode, painter, proj, faint);
    }
    // Placing a conic's apex: the curve is the preview, and the two tangent legs it is built
    // from are a faint guide. A full-strength cursor rubber band here read as a straight line
    // being drawn rather than as a curve being shaped.
    let legs = t.cmd == "sketch.conic" && t.pts.len() == 2;
    if let (Some(last), Some((h, _))) = (t.pts.last(), t.hover.as_ref()) {
        let hw = ss.plane.to_world(*h);
        if legs {
            if let Some(first) = t.pts.first() {
                crate::tools::paint_sketch_path(&[ss.plane.to_world(first.0), hw], mode, painter, proj, faint);
            }
            crate::tools::paint_sketch_path(&[ss.plane.to_world(last.0), hw], mode, painter, proj, faint);
        } else {
            crate::tools::paint_sketch_path(&[ss.plane.to_world(last.0), hw], mode, painter, proj, stroke);
        }
    }
    if let Some((h, _)) = t.hover.as_ref()
        && let Some(curve) = crate::tools::preview_curve(t, *h, None, crate::sketch_dims::locked_value(app, t, "Rho"))
    {
        crate::tools::paint_preview_curve(curve, &ss.plane, painter, proj, stroke, crate::sketch_palette::mode(app));
    }
    // Picked curves highlighted.
    let mut display_budget = solidcraft_engine::render::SamplingBudget::new(32_768, 65_536);
    for id in t.picks.iter().filter(|x| *x != "|") {
        if let Some(ci) = ss.sketch.curve_index(id) {
            crate::tools::paint_curve(&ss.sketch, ci, &ss.plane, painter, proj, Stroke::new(3.0, tk.accent.gamma_multiply(0.7)), &mut display_budget);
        }
    }
}

/// Constraint glyphs of the active sketch: a small badge per constraint at its anchor
/// (conflicting ones red).
/// Glyph size and spacing (screen pixels).
const GLYPH: f32 = 15.0;

fn glyphs(app: &SolveApp, painter: &egui::Painter, proj: &Proj, hover: Option<Pos2>) {
    let Some(sid) = active_sketch(app) else { return };
    let st = app.session.world_state();
    let Some(ss) = st.sketch(sid) else { return };
    if ss.sketch.view.hide_constraints {
        GLYPHS.with(|g| g.borrow_mut().clear());
        return;
    }
    let tk = crate::theme::Tokens::get();
    let picked = PICKED.with(|p| p.borrow().clone());
    // Where each glyph goes: beside its entity (to its left, away from the line), every entity
    // of a two-entity constraint getting one; glyphs that would overlap step along. A glyph
    // that would sit on a dimension's value goes to the other side of its line.
    let dim_texts: Vec<egui::Rect> = crate::dim_view::text_rects().into_iter().map(|(_, r)| r.expand(2.0)).collect();
    let on_dim = |p: Pos2| {
        let r = egui::Rect::from_center_size(p, egui::vec2(GLYPH, GLYPH));
        dim_texts.iter().any(|d| d.intersects(r))
    };
    let mut placed: Vec<(String, Pos2, Pos2)> = Vec::new();
    for c in &ss.sketch.constraints {
        if c.kind.is_dimension() {
            continue;
        }
        if matches!(
            c.kind,
            solidcraft_engine::sketch::ConstraintKind::PatternPoint { .. } | solidcraft_engine::sketch::ConstraintKind::PatternRadius { .. }
        ) && !ss.sketch.patterns.iter().any(|p| p.instances.first().and_then(|i| i.constraints.first()) == Some(&c.id))
        {
            continue;
        }
        for (at, dir) in glyph_anchors(&ss.sketch, &c.kind) {
            let Some(base) = proj.to_screen(ss.plane.to_world(at)) else { continue };
            // The entity's direction on screen, and its side.
            let side = match dir.and_then(|d| proj.to_screen(ss.plane.to_world(at + d))) {
                Some(q) => {
                    let t = (q - base).normalized();
                    if t.length() > 0.5 { egui::vec2(-t.y, t.x) } else { egui::vec2(0.7, -0.7) }
                }
                None => egui::vec2(0.7, -0.7),
            };
            let step = if dir.is_some() { egui::vec2(side.y, -side.x) } else { egui::vec2(1.0, 0.0) };
            let free = |sp: Pos2| !placed.iter().any(|(_, q, _)| q.distance(sp) < GLYPH + 1.0);
            let start = |s: egui::Vec2| base + s * (GLYPH * 0.9);
            let side = if on_dim(start(side)) && !on_dim(start(-side)) { -side } else { side };
            // Step along until clear of other glyphs, preferring a spot clear of dimension
            // values too.
            let settle = |avoid_dims: bool| {
                let mut sp = start(side);
                for _ in 0..12 {
                    if free(sp) && !(avoid_dims && on_dim(sp)) {
                        return Some(sp);
                    }
                    sp += step * (GLYPH + 2.0);
                }
                None
            };
            let sp = settle(true).or_else(|| settle(false)).unwrap_or_else(|| start(side) + step * (12.0 * (GLYPH + 2.0)));
            placed.push((c.id.clone(), sp, base));
        }
    }
    let hovered = hover.and_then(|h| placed.iter().find(|(_, q, _)| q.distance(h) <= GLYPH * 0.6).map(|(id, _, _)| id.clone()));
    // The hovered (or selected) constraint's entities lit up.
    if let Some(id) = hovered.as_ref().or(picked.as_ref())
        && let Some(c) = ss.sketch.constraints.iter().find(|c| &c.id == id)
    {
        let (cs, ps) = constraint_entities(&c.kind);
        let mut display_budget = solidcraft_engine::render::SamplingBudget::new(32_768, 65_536);
        for ci in cs {
            crate::tools::paint_curve(&ss.sketch, ci, &ss.plane, painter, proj, Stroke::new(3.0, tk.accent.gamma_multiply(0.7)), &mut display_budget);
        }
        for pi in ps {
            if let Some(q) = ss.sketch.point(pi).and_then(|q| proj.to_screen(ss.plane.to_world(q))) {
                painter.circle_filled(q, 4.5, tk.accent);
            }
        }
    }
    let mut drawn: Vec<(String, Pos2)> = Vec::new();
    for (id, sp, base) in &placed {
        let Some(c) = ss.sketch.constraints.iter().find(|c| &c.id == id) else { continue };
        let failing = ss.report.failing.contains(id);
        // Redundant (already implied by the others, but holding): dimmed, not an error.
        let redundant = !failing && ss.report.redundant.contains(id);
        let selected = picked.as_deref() == Some(id.as_str())
            || app.session.selection.iter().any(|s| matches!(s, solidcraft_engine::Sel::SketchConstraint { id: x } if x == id));
        let lit = selected || hovered.as_deref() == Some(id.as_str());
        let col = if failing {
            tk.error
        } else if lit {
            tk.accent
        } else if redundant {
            tk.glyph_text.gamma_multiply(0.45)
        } else {
            tk.glyph_text
        };
        // A faint leader back to the entity when the glyph was pushed away from it.
        if sp.distance(*base) > GLYPH * 1.6 {
            painter.line_segment([*base, *sp], Stroke::new(0.8, tk.text_dim.gamma_multiply(0.6)));
        }
        let r = egui::Rect::from_center_size(*sp, egui::vec2(GLYPH, GLYPH));
        painter.rect_filled(r, 3.0, if lit { tk.accent_soft } else { tk.glyph_bg });
        painter.rect_stroke(r, 3.0, Stroke::new(1.0, if failing || lit { col } else { tk.glyph_edge }), egui::StrokeKind::Inside);
        let pattern_icon = ss.sketch.patterns.iter().find(|p| p.instances.first().and_then(|i| i.constraints.first()) == Some(id)).map(|p| {
            if matches!(p.kind, solidcraft_engine::sketch::SketchPatternKind::Circular { .. }) {
                "sketch_circ_pattern"
            } else {
                "sketch_rect_pattern"
            }
        });
        match pattern_icon.or_else(|| glyph_icon(c.kind.name())) {
            Some(icon) => crate::icons::paint(painter, r.shrink(2.0), icon, col, tk.glyph_bg, col),
            None => {
                painter.text(*sp, egui::Align2::CENTER_CENTER, glyph_letter(c.kind.name()), egui::FontId::proportional(10.5), col);
            }
        }
        drawn.push((id.clone(), *sp));
    }
    if let (Some(id), Some(h)) = (hovered, hover)
        && let Some(c) = ss.sketch.constraints.iter().find(|c| c.id == id)
    {
        let label = if ss.report.redundant.contains(&id) {
            format!("{} ({id}), redundant: already implied by other constraints", c.kind.name())
        } else {
            format!("{} ({id})", c.kind.name())
        };
        let at = h + egui::vec2(12.0, 12.0);
        let g = painter.layout_no_wrap(label, egui::FontId::proportional(11.0), tk.text);
        painter.rect_filled(egui::Rect::from_min_size(at, g.size()).expand(3.0), 3.0, tk.panel);
        painter.galley(at, g, tk.text);
    }
    GLYPHS.with(|g| *g.borrow_mut() = drawn);
}

/// The curves and points a constraint ties together.
fn constraint_entities(k: &solidcraft_engine::sketch::ConstraintKind) -> (Vec<usize>, Vec<usize>) {
    use solidcraft_engine::sketch::ConstraintKind::*;
    match *k {
        Coincident { p, q } | HorizontalPoints { p, q } | VerticalPoints { p, q } => (vec![], vec![p, q]),
        PatternPoint { p, q, center, directions, .. } => {
            (directions.into_iter().flatten().map(|(l, _)| l).collect(), vec![p, q].into_iter().chain(center).collect())
        }
        PatternRadius { a, b } => (vec![a, b], vec![]),
        Fix { p } => (vec![], vec![p]),
        Midpoint { p, l } => (vec![l], vec![p]),
        PointOnCurve { p, c } => (vec![c], vec![p]),
        Symmetric { p, q, l } => (vec![l], vec![p, q]),
        Horizontal { l } | Vertical { l } => (vec![l], vec![]),
        Parallel { a, b }
        | Perpendicular { a, b }
        | Collinear { a, b }
        | Equal { a, b }
        | Concentric { a, b }
        | Tangent { a, b }
        | Smooth { a, b } => (vec![a, b], vec![]),
        _ => (vec![], vec![]),
    }
}

/// Where a constraint's glyphs go: one per entity (the middle of a curve, with its direction
/// there; a point, without).
fn glyph_anchors(sk: &solidcraft_engine::sketch::Sketch, k: &solidcraft_engine::sketch::ConstraintKind) -> Vec<(Vec2, Option<Vec2>)> {
    use solidcraft_engine::sketch::ConstraintKind::*;
    // Half way along the curve, and the direction there.
    let mid = |c: usize| -> Option<(Vec2, Option<Vec2>)> {
        let poly = sk.polyline(c);
        let total: f64 = poly.windows(2).map(|w| w[0].dist(w[1])).sum();
        let mut acc = 0.0;
        for w in poly.windows(2) {
            let l = w[0].dist(w[1]);
            if acc + l >= total * 0.5 && l > 0.0 {
                return Some((w[0].lerp(w[1], (total * 0.5 - acc) / l), (w[1] - w[0]).normalized()));
            }
            acc += l;
        }
        poly.first().map(|p| (*p, None))
    };
    let pt = |p: usize| sk.point(p).map(|q| (q, None));
    match *k {
        Coincident { p, .. } | Fix { p } | Midpoint { p, .. } | PointOnCurve { p, .. } => pt(p).into_iter().collect(),
        PatternPoint { p, center, .. } => pt(center.unwrap_or(p)).into_iter().collect(),
        PatternRadius { .. } => Vec::new(),
        HorizontalPoints { p, q } | VerticalPoints { p, q } | Symmetric { p, q, .. } => {
            sk.point(p).zip(sk.point(q)).map(|(a, b)| ((a + b) * 0.5, None)).into_iter().collect()
        }
        Horizontal { l } | Vertical { l } => mid(l).into_iter().collect(),
        Parallel { a, b }
        | Perpendicular { a, b }
        | Collinear { a, b }
        | Equal { a, b }
        | Concentric { a, b }
        | Tangent { a, b }
        | Smooth { a, b } => [mid(a), mid(b)].into_iter().flatten().collect(),
        _ => Vec::new(),
    }
}

/// The inference label next to the snapped cursor (while a tool is active).
fn snap_hint(app: &SolveApp, painter: &egui::Painter, proj: &Proj) {
    if app.tool.is_none() {
        return;
    }
    let Some((label, p)) = SNAP.with(|s| *s.borrow()) else { return };
    let st = app.session.world_state();
    let Some(ss) = active_sketch(app).and_then(|s| st.sketch(s)) else { return };
    let Some(sp) = proj.to_screen(ss.plane.to_world(p)) else { return };
    let tk = crate::theme::Tokens::get();
    let r = egui::Rect::from_min_size(sp + egui::vec2(12.0, -24.0), egui::vec2(24.0, 14.0));
    painter.rect_filled(r, 3.0, tk.accent_soft);
    painter.text(r.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(10.0), tk.text);
}

/// Per frame, after the viewport is drawn: constraint glyphs, snap hints and analysis marks.
pub fn show(app: &mut SolveApp, ui: &egui::Ui, painter: &egui::Painter, proj: &Proj) {
    SHIFT.with(|c| c.set(ui.input(|i| i.modifiers.shift)));
    glyphs(app, painter, proj, ui.input(|i| i.pointer.hover_pos()).filter(|_| ui.rect_contains_pointer(proj.rect)));
    snap_hint(app, painter, proj);
    combs(app, painter, proj);
    if DEFERRED.with(|d| d.replace(false)) {
        analysis(app, "inspect.center_of_mass", json!({}));
    }
    marks(app, painter, proj);
    iso_lines(app, painter, proj);
    continuity_legend(app, painter, proj);
}

#[cfg(test)]
#[path = "sketch_corner_ui_tests.rs"]
mod corner_ui_tests;

#[cfg(test)]
#[path = "sketch_continuity_tests.rs"]
mod continuity_tests;
