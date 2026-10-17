//! Sketch dimensions drawn as dimensions: extension lines from the measured geometry, a
//! dimension line (or arc) with arrowheads, and the value centred on it; radius and diameter
//! leaders with R / Ø, angles as an arc between the two lines, arc lengths with the arc mark,
//! driven (reference) dimensions in parentheses in a muted colour. The geometry comes from
//! `solidcraft_sketch::dim_layout`.
//!
//! A click selects a dimension (Delete removes it), a drag moves its text (and so the offset of
//! its dimension line), a double-click edits its value in place with parameter completion.
//! Everything acts through commands: `sketch.dimension_text`, `sketch.delete`,
//! `parameters.change`.

use std::cell::RefCell;
use std::sync::Arc;

use egui::{FontId, Pos2, Rect, Shape, Stroke, vec2};
use serde_json::json;
use solidcraft_engine::Sel;
use solidcraft_engine::geom::Vec2;
use solidcraft_engine::sketch::{ConstraintKind, DimFrame, chain_centres, default_text, dim_frame, dim_layout_reaching, dim_reach};

use crate::SolveApp;
use crate::theme::Tokens;
use crate::viewport::Proj;

/// A dimension as drawn this frame (screen space), for hit testing.
#[derive(Clone, Debug)]
struct Drawn {
    id: String,
    param: Option<String>,
    text: Rect,
    segs: Vec<[Pos2; 2]>,
    /// Text centre in sketch coordinates.
    at: Vec2,
}

/// The inline value editor: parameter, expression, where it sits, first frame.
#[derive(Clone, Debug)]
struct Edit {
    param: String,
    expr: String,
    /// Where it sits (None until the dimension has been drawn).
    at: Option<Pos2>,
    /// Frames left in which the box (re)takes the keyboard: the rest of the double-click
    /// that opened it must not close it.
    fresh: u8,
    /// Frames left to wait for the dimension to be drawn.
    wait: u8,
    draft: Option<DimensionDraft>,
    /// Type-first creation: the value took the keyboard as soon as the picks determined the
    /// dimension, and its text follows the pointer until a click pins it or Enter places it.
    live: Option<Live>,
    selection: Option<egui::text::CCursorRange>,
}

#[derive(Clone, Copy, Debug)]
struct Live {
    /// The pointer (screen) at the pick: until it moves away, the text keeps its default place.
    origin: Option<Pos2>,
    moved: bool,
    /// Something was typed into the value.
    typed: bool,
}

/// Creation stays in a scratch session until the user accepts the value. Retaining the
/// document also makes the editor's identity check safe across document replacements.
#[derive(Clone, Debug)]
struct DimensionDraft {
    baseline: Arc<solidcraft_engine::doc::Document>,
    sketch: u64,
    component: u64,
    path: Option<Vec<u64>>,
    pending: std::collections::BTreeMap<u64, solidcraft_engine::doc::Mat>,
    entities: Vec<String>,
    /// Where the text goes (None: the dimension's default place).
    at: Option<Vec2>,
    checked_expr: String,
    preview_doc: Arc<solidcraft_engine::doc::Document>,
    preview: Arc<solidcraft_engine::doc::ModelState>,
    error: Option<String>,
    /// A dimension that would over-constrain the sketch, placed as a driven (reference) one:
    /// it has no parameter and no value to edit (#320).
    driven: bool,
}

impl DimensionDraft {
    fn valid(&self, app: &SolveApp) -> bool {
        Arc::ptr_eq(&self.baseline, &app.session.doc)
            && app.session.active_sketch == Some(self.sketch)
            && app.session.active_component == self.component
            && app.session.active_occurrence_path == self.path
            && app.session.pending_moves == self.pending
    }
    fn params(&self, expr: &str) -> serde_json::Value {
        // The first dimension of a sketch scales it rather than distorting it (#1365).
        let mut params = json!({"entities": self.entities, "value": expr, "scale_sketch": true});
        if let Some(at) = self.at {
            params["text_at"] = json!([at.x, at.y]);
        }
        params
    }
    fn evaluate(&self, app: &SolveApp, expr: &str) -> Result<solidcraft_engine::Session, String> {
        let mut scratch = app.session.scratch();
        let result = scratch.execute("sketch.dimension", &self.params(expr)).map_err(|e| e.to_string())?;
        let param = result.get("param").and_then(serde_json::Value::as_str).ok_or("The dimension value is unavailable")?;
        let state = scratch.world_state();
        let solved = state.sketch(self.sketch).ok_or("The dimension sketch is unavailable")?;
        if !solved.report.ok() {
            return Err("That value conflicts with the sketch constraints".into());
        }
        if let Some(kind) = solved.sketch.constraints.iter().find(|c| c.param.as_deref() == Some(param)).map(|c| &c.kind)
            && !kind.is_angle()
            && kind.value().is_some_and(|value| !value.is_finite() || value <= 0.0)
        {
            return Err("Enter a dimension greater than zero".into());
        }
        Ok(scratch)
    }
}

pub fn cancel() {
    EDIT.with(|edit| *edit.borrow_mut() = None);
    PLACEMENT.with(|preview| *preview.borrow_mut() = None);
}

/// Infer the measured value through the same command as scripts, without creating a real
/// parameter or an undo entry. Only accepting the editor executes the command on the design.
/// A driven dimension has no value to accept, so it is added at once.
pub fn begin_dimension(app: &mut SolveApp, entities: Vec<String>, at: Vec2) -> Result<(), String> {
    let (param, expr, draft) = dimension_draft(app, entities, Some(at))?;
    if draft.driven {
        PLACEMENT.with(|preview| *preview.borrow_mut() = None);
        app.run("sketch.dimension", json!({"entities": draft.entities, "text_at": [at.x, at.y], "driven": true}))?;
        app.set_status("The sketch already determines that dimension, so it was added as a driven (reference) dimension", false);
        return Ok(());
    }
    EDIT.with(|edit| {
        *edit.borrow_mut() = Some(Edit { param, expr, at: None, fresh: FRESH, wait: 10, draft: Some(draft), live: None, selection: None })
    });
    PLACEMENT.with(|preview| *preview.borrow_mut() = None);
    Ok(())
}

/// Type-first creation: the picks determine a dimension, so its value takes the keyboard at
/// once (D, click a line, type). The text follows the pointer; a click pins it, Enter places
/// it. False when the dimension would only be a driven one: that keeps the place-first flow,
/// where placing it adds it.
pub fn begin_live(app: &mut SolveApp, entities: Vec<String>, pointer: Option<Pos2>) -> Result<bool, String> {
    let (param, expr, draft) = dimension_draft(app, entities, None)?;
    if draft.driven {
        return Ok(false);
    }
    let live = Some(Live { origin: pointer, moved: false, typed: false });
    EDIT.with(|edit| *edit.borrow_mut() = Some(Edit { param, expr, at: None, fresh: FRESH, wait: 10, draft: Some(draft), live, selection: None }));
    PLACEMENT.with(|preview| *preview.borrow_mut() = None);
    Ok(true)
}

/// The type-first value is open: Some(whether anything was typed into it yet).
pub fn live() -> Option<bool> {
    EDIT.with(|edit| edit.borrow().as_ref().and_then(|e| e.live.map(|l| l.typed)))
}

/// A click while the type-first value is open places its text there; the value keeps the
/// keyboard (and what was typed).
pub fn pin(app: &SolveApp, at: Vec2) {
    EDIT.with(|edit| {
        let mut edit = edit.borrow_mut();
        let Some(e) = edit.as_mut() else { return };
        let Some(live) = e.live.take() else { return };
        if let Some(draft) = e.draft.as_mut() {
            draft.at = Some(at);
            reevaluate(app, draft, &e.expr);
        }
        // An untouched value stays selected, so typing still replaces it.
        e.fresh = if live.typed { FRESH - 1 } else { FRESH };
    });
}

fn reevaluate(app: &SolveApp, draft: &mut DimensionDraft, expr: &str) {
    draft.checked_expr = expr.to_string();
    match draft.evaluate(app, expr) {
        Ok(scratch) => {
            draft.preview = scratch.world_state();
            draft.preview_doc = scratch.doc.clone();
            draft.error = None;
        }
        Err(error) => draft.error = Some(error),
    }
}

fn dimension_draft(app: &SolveApp, entities: Vec<String>, at: Option<Vec2>) -> Result<(String, String, DimensionDraft), String> {
    let sketch = app.session.active_sketch.ok_or("Edit a sketch to add a dimension")?;
    if entities.is_empty() || entities.len() > 2 || at.is_some_and(|at| !at.x.is_finite() || !at.y.is_finite()) {
        return Err("Pick one edge or two endpoints, then place the dimension".into());
    }
    // A dimension the sketch can't take (it would over-constrain it) becomes a driven one,
    // which only measures; when that fails too, the first refusal is the one to report.
    draft_of(app, sketch, &entities, at, false).or_else(|error| draft_of(app, sketch, &entities, at, true).map_err(|_| error))
}

/// The draft of one dimension: its parameter (for a driven one, its constraint id, since it
/// has no parameter), the parameter's expression, and the scratch session it was added in.
fn draft_of(app: &SolveApp, sketch: u64, entities: &[String], at: Option<Vec2>, driven: bool) -> Result<(String, String, DimensionDraft), String> {
    let mut scratch = app.session.scratch();
    let mut params = json!({"entities": entities, "driven": driven});
    if let Some(at) = at {
        params["text_at"] = json!([at.x, at.y]);
    }
    let result = scratch.execute("sketch.dimension", &params).map_err(|e| e.to_string())?;
    let (param, expr) = if driven {
        let before =
            app.session.doc.sketch(sketch).ok().map(|sk| sk.constraints.iter().map(|c| c.id.clone()).collect::<Vec<_>>()).unwrap_or_default();
        let added =
            scratch.doc.sketch(sketch).ok().and_then(|sk| sk.constraints.iter().find(|c| c.driven && !before.contains(&c.id)).map(|c| c.id.clone()));
        (added.ok_or("The driven dimension is unavailable")?, String::new())
    } else {
        let param = result.get("param").and_then(serde_json::Value::as_str).ok_or("This dimension has no editable value")?.to_string();
        let expr = scratch.doc.param(&param).ok_or("The dimension value is unavailable")?.expr.clone();
        (param, expr)
    };
    let draft = DimensionDraft {
        baseline: app.session.doc.clone(),
        sketch,
        component: app.session.active_component,
        path: app.session.active_occurrence_path.clone(),
        pending: app.session.pending_moves.clone(),
        entities: entities.to_vec(),
        at,
        checked_expr: expr.clone(),
        preview: scratch.world_state(),
        preview_doc: scratch.doc.clone(),
        error: None,
        driven,
    };
    if !draft.preview.sketch(sketch).is_some_and(|solved| solved.report.ok()) {
        return Err("That dimension conflicts with the sketch constraints".into());
    }
    Ok((param, expr, draft))
}

/// Cache inference at a stationary pointer, including incomplete/unsupported picks.
#[derive(Clone)]
struct Placement {
    baseline: Arc<solidcraft_engine::doc::Document>,
    sketch: Option<u64>,
    component: u64,
    path: Option<Vec<u64>>,
    pending: std::collections::BTreeMap<u64, solidcraft_engine::doc::Mat>,
    entities: Vec<String>,
    at: Vec2,
    draft: Option<(String, DimensionDraft)>,
}

fn placement_preview(app: &SolveApp) -> Option<(String, DimensionDraft)> {
    let tool = app.tool.as_ref().filter(|t| t.kind == crate::tools::Kind::Dimension && !t.picks.is_empty())?;
    let at = tool.hover.as_ref()?.0;
    PLACEMENT.with(|preview| {
        let mut preview = preview.borrow_mut();
        let current = preview.as_ref().is_some_and(|p| {
            Arc::ptr_eq(&p.baseline, &app.session.doc)
                && p.sketch == app.session.active_sketch
                && p.component == app.session.active_component
                && p.path == app.session.active_occurrence_path
                && p.pending == app.session.pending_moves
                && p.entities == tool.picks
                && p.at == at
        });
        if !current {
            let draft = dimension_draft(app, tool.picks.clone(), Some(at)).ok().map(|(param, _, draft)| (param, draft));
            *preview = Some(Placement {
                baseline: app.session.doc.clone(),
                sketch: app.session.active_sketch,
                component: app.session.active_component,
                path: app.session.active_occurrence_path.clone(),
                pending: app.session.pending_moves.clone(),
                entities: tool.picks.clone(),
                at,
                draft,
            });
        }
        preview.as_ref().and_then(|p| p.draft.clone())
    })
}

thread_local! {
    static DRAWN: RefCell<Vec<Drawn>> = const { RefCell::new(Vec::new()) };
    /// A text being dragged: dimension id, undo depth at the start, grab offset (sketch).
    static DRAG: RefCell<Option<(String, usize, Vec2)>> = const { RefCell::new(None) };
    static EDIT: RefCell<Option<Edit>> = const { RefCell::new(None) };
    static PLACEMENT: RefCell<Option<Placement>> = const { RefCell::new(None) };
}

/// Frames the inline editor holds on to the keyboard after opening.
const FRESH: u8 = 3;

/// Pixels from a dimension line or the text box that still count as on it.
const HIT_PX: f32 = 4.0;

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// The dimension whose value text is under a screen position (its id).
pub fn hit_text(pos: Pos2) -> Option<String> {
    DRAWN.with(|d| d.borrow().iter().find(|x| x.text.expand(2.0).contains(pos)).map(|x| x.id.clone()))
}

/// The dimension under a screen position (its id).
pub fn hit(pos: Pos2) -> Option<String> {
    DRAWN.with(|d| {
        let d = d.borrow();
        // Text boxes first (they sit on top of the lines), then the lines.
        d.iter()
            .find(|x| x.text.expand(2.0).contains(pos))
            .or_else(|| d.iter().find(|x| x.segs.iter().any(|s| seg_dist(pos, s[0], s[1]) <= HIT_PX)))
            .map(|x| x.id.clone())
    })
}

/// The selected dimensions (constraint ids; they are in the session's selection).
pub fn selected(app: &SolveApp) -> Vec<String> {
    let dims: Vec<String> = DRAWN.with(|d| d.borrow().iter().map(|x| x.id.clone()).collect());
    app.session
        .selection
        .iter()
        .filter_map(|s| if let Sel::SketchConstraint { id } = s { Some(id.clone()) } else { None })
        .filter(|id| dims.contains(id))
        .collect()
}

/// Where the dimension driven by parameter `param` (or with constraint id `param`) shows its
/// value (screen), as drawn last frame.
pub fn text_at(param: &str) -> Option<Pos2> {
    DRAWN.with(|d| d.borrow().iter().find(|x| x.param.as_deref() == Some(param) || x.id == param).map(|x| x.text.center()))
}

/// Dimension text boxes drawn last frame (for box selection).
pub fn text_rects() -> Vec<(String, Rect)> {
    DRAWN.with(|d| d.borrow().iter().map(|x| (x.id.clone(), x.text)).collect())
}

/// A double-click on a dimension opens its value for editing (true: it was on one).
pub fn double_click(app: &mut SolveApp, pos: Pos2) -> bool {
    let Some(d) = DRAWN.with(|d| {
        let id = hit(pos)?;
        d.borrow().iter().find(|x| x.id == id).cloned()
    }) else {
        return false;
    };
    let Some(param) = d.param else {
        app.set_status("A driven dimension measures the sketch; it has no value to edit", false);
        return true;
    };
    let expr = app.session.doc.param(&param).map(|p| p.expr.clone()).unwrap_or_default();
    EDIT.with(|e| {
        *e.borrow_mut() = Some(Edit { param, expr, at: Some(d.text.center()), fresh: FRESH, wait: 0, draft: None, live: None, selection: None })
    });
    true
}

/// Start dragging the text of the dimension under `press` (true: a dimension took the drag).
/// Its lines take it only off sketch geometry (`on_geometry`: a point or curve is under `press`).
pub fn drag_start(app: &SolveApp, press: Pos2, press_sketch: Vec2, on_geometry: bool) -> bool {
    let Some(d) = DRAWN.with(|d| {
        let id = hit_text(press).or_else(|| if on_geometry { None } else { hit(press) })?;
        d.borrow().iter().find(|x| x.id == id).cloned()
    }) else {
        return false;
    };
    DRAG.with(|g| *g.borrow_mut() = Some((d.id, app.session.undo.len(), d.at - press_sketch)));
    true
}

/// Is a dimension text being dragged?
pub fn dragging() -> bool {
    DRAG.with(|g| g.borrow().is_some())
}

/// Move the dragged text so it follows the pointer (at `to`, sketch coordinates); one undo
/// step for the whole drag.
pub fn drag_to(app: &mut SolveApp, to: Vec2) {
    let Some((id, depth, off)) = DRAG.with(|g| g.borrow().clone()) else { return };
    let at = to + off;
    if app.session.execute("sketch.dimension_text", &json!({"dimension": id, "at": [at.x, at.y]})).is_ok() {
        app.session.undo.truncate(depth + 1);
    }
}

pub fn drag_end() {
    DRAG.with(|g| *g.borrow_mut() = None);
}

/// The text shown for a dimension: the value (R / Ø for radius and diameter, degrees for
/// angles), "fx: value" when an expression drives it, in parentheses when driven.
pub fn label(k: &ConstraintKind, expr: Option<&str>, driven: bool, units: &str, decimals: u8) -> String {
    let v = k.value().unwrap_or(0.0);
    // Retain deliberately authored trailing zeros in a four-place inch literal too.
    let authored_four = units == "in"
        && expr.is_some_and(|e| {
            e.trim().strip_suffix("in").map(str::trim).is_some_and(|number| {
                number.parse::<f64>().is_ok()
                    && number.split_once('.').is_some_and(|(_, fraction)| fraction.len() >= 4 && fraction.bytes().all(|b| b.is_ascii_digit()))
            })
        });
    let decimals = if authored_four { decimals.max(4) } else { decimals };
    let num = if k.is_angle() { format!("{:.1}°", v.to_degrees()) } else { crate::prefs::length_number(v, units, decimals) };
    let num = match k {
        ConstraintKind::Radius { .. } => format!("R{num}"),
        ConstraintKind::Diameter { .. } | ConstraintKind::LinearDiameter { .. } => format!("Ø{num}"),
        _ => num,
    };
    let plain = expr.is_none_or(|e| {
        let e = e.trim();
        let e = ["mm", "cm", "in", "ft", "m", "deg", "rad", "°"].iter().find_map(|unit| e.strip_suffix(unit)).unwrap_or(e);
        crate::prefs::parse_number(e).is_some()
    });
    let text = if plain { num } else { format!("fx: {num}") };
    if driven { format!("({text})") } else { text }
}

/// Draw the active sketch's dimensions and remember where they are for picking.
pub fn show(app: &mut SolveApp, ui: &egui::Ui, painter: &egui::Painter, proj: &Proj) {
    let mut drawn = Vec::new();
    let editor_draft = EDIT.with(|edit| {
        let mut edit = edit.borrow_mut();
        if edit.as_ref().and_then(|e| e.draft.as_ref()).is_some_and(|d| !d.valid(app)) {
            *edit = None;
        }
        if let Some(e) = edit.as_mut()
            && let Some(draft) = e.draft.as_mut()
        {
            let mut moved = false;
            if let Some(live) = e.live.as_mut() {
                // The text follows the pointer once it leaves the pick.
                let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| proj.rect.contains(*p));
                if !live.moved && pointer.is_some_and(|p| live.origin.is_none_or(|o| o.distance(p) > 4.0)) {
                    live.moved = true;
                }
                let hover = app.tool.as_ref().and_then(|t| t.hover.as_ref()).map(|h| h.0);
                if live.moved
                    && let Some(at) = hover
                    && draft.at != Some(at)
                {
                    draft.at = Some(at);
                    moved = true;
                }
            }
            if moved || e.expr != draft.checked_expr {
                let expr = e.expr.clone();
                reevaluate(app, draft, &expr);
            }
            Some((e.param.clone(), draft.clone()))
        } else {
            None
        }
    });
    let placing = !editing();
    let draft = editor_draft.or_else(|| placing.then(|| placement_preview(app)).flatten());
    let st = app.session.world_state();
    let Some(ss) = app.session.active_sketch.and_then(|s| st.sketch(s)) else {
        DRAWN.with(|d| d.borrow_mut().clear());
        EDIT.with(|e| *e.borrow_mut() = None);
        return;
    };
    let sk = &ss.sketch;
    if sk.view.hide_dimensions {
        DRAWN.with(|d| d.borrow_mut().clear());
        if draft.is_none() {
            return;
        }
    }
    let tk = Tokens::get();
    let is_sel = |id: &str| app.session.selection.iter().any(|s| matches!(s, Sel::SketchConstraint { id: x } if x == id));
    let hover = ui.input(|i| i.pointer.hover_pos()).filter(|_| ui.rect_contains_pointer(proj.rect)).and_then(hit);
    let font = FontId::proportional(12.0);
    let centres = chain_centres(sk);
    let draft_sketch = draft.as_ref().and_then(|(_, d)| d.preview.sketch(d.sketch));
    let draft_centres = draft_sketch.map(|solved| chain_centres(&solved.sketch));
    let mut constraints = sk.constraints.iter().map(|c| (ss, c, false)).collect::<Vec<_>>();
    if let (Some(source), Some((param, _))) = (draft_sketch, draft.as_ref()) {
        // A driven draft is keyed by its constraint id (it has no parameter).
        let ours = |c: &&solidcraft_engine::sketch::Constraint| c.param.as_deref() == Some(param.as_str()) || (c.driven && c.id == *param);
        constraints.extend(source.sketch.constraints.iter().filter(ours).map(|c| (source, c, true)));
    }
    for (source, c, is_draft) in constraints {
        if ss.sketch.view.hide_dimensions && !is_draft {
            continue;
        }
        let sk = &source.sketch;
        let centres = if is_draft { draft_centres.as_ref().unwrap_or(&centres) } else { &centres };
        let to = |p: Vec2| proj.to_screen(source.plane.to_world(p));
        let Some(frame) = dim_frame(sk, &c.kind) else { continue };
        let anchor = match frame {
            DimFrame::Linear { p0, p1, .. } => (p0 + p1) * 0.5,
            DimFrame::Radial { center, .. } | DimFrame::ArcLength { center, .. } => center,
            DimFrame::Angular { vertex, .. } => vertex,
        };
        // Millimetres per pixel at the dimension.
        let (Some(a), Some(b)) = (to(anchor), to(anchor + Vec2::X)) else { continue };
        let px = 1.0 / f64::from(a.distance(b)).max(1e-9);
        if !px.is_finite() {
            continue;
        }
        let display_doc =
            if is_draft { draft.as_ref().map(|(_, d)| d.preview_doc.as_ref()).unwrap_or(app.session.doc.as_ref()) } else { app.session.doc.as_ref() };
        let expr = c.param.as_ref().and_then(|p| display_doc.param(p)).map(|p| p.expr.as_str());
        let failing = source.report.failing.contains(&c.id);
        let col = if failing {
            tk.error
        } else if is_sel(&c.id) || hover.as_deref() == Some(c.id.as_str()) {
            tk.accent
        } else if c.driven {
            tk.dimension_driven
        } else {
            tk.dimension
        };
        let galley =
            painter.layout_no_wrap(label(&c.kind, expr, c.driven, &app.session.doc.units, app.preferences.length_decimals), font.clone(), col);
        let size = galley.size() + vec2(6.0, 2.0);
        let half = Vec2::new(f64::from(size.x) * 0.5 * px, f64::from(size.y) * 0.5 * px);
        // Unplaced linear dimensions sit outside the shape they measure.
        let away = first_point(sk, &c.kind).and_then(|p| centres.get(p).copied()).unwrap_or(anchor);
        let lay = dim_layout_reaching(&frame, c.text.or_else(|| default_text(&frame, away, px)), px, half, dim_reach(sk, &c.kind));
        let stroke = Stroke::new(1.0, col);
        let mut segs = Vec::new();
        for line in &lay.lines {
            let pts: Vec<Pos2> = line.iter().filter_map(|q| to(*q)).collect();
            for w in pts.windows(2) {
                segs.push([w[0], w[1]]);
            }
            painter.add(Shape::line(pts, stroke));
        }
        for (tip, dir) in &lay.arrows {
            if let (Some(t), Some(back)) = (to(*tip), to(*tip - *dir * px)) {
                crate::icons::markers::dimension_arrow(painter, t, back, col);
            }
        }
        let Some(tc) = to(lay.text) else { continue };
        let r = Rect::from_center_size(tc, size);
        painter.rect_filled(r, 2.0, tk.viewport_top.gamma_multiply(0.85));
        painter.galley(r.min + vec2(3.0, 1.0), galley, col);
        if matches!(frame, DimFrame::ArcLength { .. }) {
            // The arc-length mark over the value.
            let c0 = pos2_mid_top(r);
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| c0 + vec2(5.0 * (std::f32::consts::PI * i as f32 / 12.0).cos(), -3.0 * (std::f32::consts::PI * i as f32 / 12.0).sin()))
                .collect();
            painter.add(Shape::line(pts, stroke));
        }
        if is_draft && !placing {
            EDIT.with(|edit| {
                if let Some(e) = edit.borrow_mut().as_mut() {
                    e.at = Some(tc);
                }
            });
        } else if !is_draft {
            drawn.push(Drawn { id: c.id.clone(), param: c.param.clone(), text: r, segs, at: lay.text });
        }
    }
    DRAWN.with(|d| *d.borrow_mut() = drawn);
    edit_box(app, ui.ctx());
}

/// A point a dimension measures (for the shape it belongs to).
fn first_point(sk: &solidcraft_engine::sketch::Sketch, k: &ConstraintKind) -> Option<usize> {
    use ConstraintKind::*;
    match *k {
        Length { l, .. } => sk.curves.get(l).and_then(|c| c.kind.point_ids().first().copied()),
        Distance { p, .. } | DistanceX { p, .. } | DistanceY { p, .. } | PointLineDistance { p, .. } | LinearDiameter { p, .. } => Some(p),
        _ => None,
    }
}

fn pos2_mid_top(r: Rect) -> Pos2 {
    Pos2::new(r.center().x, r.top() - 1.0)
}

/// The inline value editor: Enter applies, Esc (or a click elsewhere) cancels.
/// Edit a dimension's value in place by its parameter (a new dimension from the Dimension tool:
/// its value is selected, so typing replaces it).
pub fn edit_param(app: &SolveApp, param: &str) {
    let expr = app.session.doc.param(param).map(|p| p.expr.clone()).unwrap_or_default();
    EDIT.with(|e| {
        *e.borrow_mut() = Some(Edit { param: param.to_string(), expr, at: None, fresh: FRESH, wait: 10, draft: None, live: None, selection: None })
    });
}

/// Insert a reference at the expression's selection, using character offsets (not bytes).
fn insert_reference(ctx: &egui::Context, edit: &mut Edit, name: &str) {
    let id = egui::Id::new("sc_dim_edit");
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let len = edit.expr.chars().count();
    let (start, end) = edit
        .selection
        .or_else(|| state.cursor.char_range())
        .map(|r| {
            let a = r.primary.index.0.min(len);
            let b = r.secondary.index.0.min(len);
            (a.min(b), a.max(b))
        })
        .unwrap_or((len, len));
    let before: String = edit.expr.chars().take(start).collect();
    let after: String = edit.expr.chars().skip(end).collect();
    edit.expr = format!("{before}{name}{after}");
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(start + name.chars().count()))));
    edit.selection = state.cursor.char_range();
    state.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
    edit.fresh = FRESH - 1;
    if let Some(live) = edit.live.as_mut() {
        live.typed = true;
    }
}

/// Consume dimension clicks while editing; only a double-click inserts a reference.
pub fn pick_reference(ctx: &egui::Context, pos: Pos2, double: bool) -> bool {
    EDIT.with(|editor| {
        let mut editor = editor.borrow_mut();
        let Some(edit) = editor.as_mut() else { return false };
        let Some(id) = hit(pos) else { return false };
        let param = DRAWN.with(|d| d.borrow().iter().find(|d| d.id == id).and_then(|d| d.param.clone()));
        if double && let Some(param) = param.filter(|p| p != &edit.param) {
            insert_reference(ctx, edit, &param);
        }
        // Self references and driven dimensions cannot supply a reference.
        edit.fresh = FRESH - 1;
        true
    })
}

fn edit_box(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(mut e) = EDIT.with(|x| x.borrow_mut().take()) else { return };
    if e.at.is_none() {
        e.at = DRAWN.with(|d| d.borrow().iter().find(|x| x.param.as_deref() == Some(e.param.as_str())).map(|x| x.text.center()));
    }
    let Some(at) = e.at else {
        // Not drawn yet (this frame's solve): try again next frame, for a few frames.
        if e.wait > 0 {
            e.wait -= 1;
            EDIT.with(|x| *x.borrow_mut() = Some(e));
        }
        return;
    };
    let id = egui::Id::new("sc_dim_edit");
    let mut done: Option<bool> = None;
    let mut escaped = false;
    // A type-first value sits beside its text, clear of the pointer, so a click still reaches
    // the sketch (to place the text or pick a second entity).
    let mut anchor = if e.live.is_some() { at + vec2(18.0, 10.0) } else { at - vec2(45.0, 11.0) };
    let mut popup_width = 220.0;
    if let Some(layout) = &app.viewport.layout
        && layout.drawable.is_finite()
        && layout.drawable.is_positive()
    {
        let rect = layout.drawable;
        popup_width = (rect.width() - 8.0).clamp(24.0, 220.0);
        anchor.x = anchor.x.clamp(rect.left(), (rect.right() - popup_width - 6.0).max(rect.left()));
        anchor.y = anchor.y.clamp(rect.top(), (rect.bottom() - 80.0).max(rect.top()));
    }
    crate::frame::area(egui::Id::new("sc_dim_edit_area")).fixed_pos(anchor).order(egui::Order::Foreground).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(3, 1)).show(ui, |ui| {
            ui.set_max_width(popup_width);
            ui.label(egui::RichText::new(format!("Dimension: {}", e.param)).strong())
                .on_hover_text("Double-click another driving dimension to insert its name");
            let r = ui.add(egui::TextEdit::singleline(&mut e.expr).id(id).desired_width(popup_width));
            if r.has_focus() {
                e.selection = egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range());
            }
            let completed = crate::params_dialog::complete(ui, &r, &mut e.expr);
            if completed {
                e.selection = egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range());
            }
            if let Some(live) = e.live.as_mut() {
                live.typed |= r.changed() || completed;
            }
            if let Some(error) = e.draft.as_ref().and_then(|d| d.error.as_ref()) {
                ui.add(egui::Label::new(egui::RichText::new(error).color(Tokens::get().error)).wrap());
            }
            if e.fresh == FRESH {
                // The whole value is selected, so typing replaces it.
                let n = e.expr.chars().count();
                let mut s = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
                s.cursor.set_char_range(Some(egui::text_selection::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
                e.selection = s.cursor.char_range();
                s.store(ui.ctx(), id);
            }
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if r.lost_focus() && enter {
                done = Some(true);
            } else if e.fresh > 0 {
                if !r.has_focus() {
                    r.request_focus();
                }
            } else if r.lost_focus() {
                done = Some(false);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
                // Esc cancels the whole dimension, picks included.
                escaped = true;
            }
        });
    });
    e.fresh = e.fresh.saturating_sub(1);
    match done {
        Some(true) => {
            let result = if let Some(draft) = &e.draft {
                if draft.valid(app) {
                    draft.evaluate(app, &e.expr).and_then(|_| app.run("sketch.dimension", draft.params(&e.expr)))
                } else {
                    Err("The sketch changed; place the dimension again".into())
                }
            } else {
                let kind = app
                    .session
                    .doc
                    .param(&e.param)
                    .and_then(|p| solidcraft_engine::doc::expr::unit_info(&p.unit))
                    .map(|(k, _)| k)
                    .unwrap_or(solidcraft_engine::doc::expr::Kind::Unitless);
                let expression = app.session.doc.with_design_unit(&e.expr, kind);
                app.run("parameters.change", json!({"name": e.param, "expression": expression}))
            };
            if let Err(err) = result {
                app.set_status(err, true);
                EDIT.with(|x| *x.borrow_mut() = Some(Edit { fresh: FRESH, ..e }));
            } else {
                clear_picks(app);
                app.set_status("Dimension applied", false);
            }
        }
        Some(false) if escaped => clear_picks(app),
        Some(false) => {}
        None => EDIT.with(|x| *x.borrow_mut() = Some(e)),
    }
}

/// The Dimension tool starts over (its picks became a dimension, or were cancelled).
fn clear_picks(app: &mut SolveApp) {
    if let Some(tool) = app.tool.as_mut().filter(|t| t.kind == crate::tools::Kind::Dimension) {
        tool.picks.clear();
    }
}

/// Is the inline editor open (it has the keyboard)?
pub fn editing() -> bool {
    EDIT.with(|e| e.borrow().is_some())
}

/// Dimensions drawn last frame: (id, text centre on screen), for tests and the control channel.
pub fn drawn() -> Vec<(String, Pos2)> {
    DRAWN.with(|d| d.borrow().iter().map(|x| (x.id.clone(), x.text.center())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_previews_radial_and_angular_dimensions_and_invalidates_on_document_change() {
        cancel();
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.circle.center", json!({"center":[30,30],"radius":5})).unwrap();
        h.app.run("sketch.line", json!({"points":[[10,10],[20,10]],"infer":false,"connect":false})).unwrap();
        h.app.run("sketch.line", json!({"points":[[10,10],[10,20]],"infer":false,"connect":false})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        for (picks, at, angular) in [(vec![[35, 30]], [40, 40], false), (vec![[15, 10], [10, 15]], [18, 18], true)] {
            h.step(&json!({"start":"sketch.dimension"})).unwrap();
            for pick in picks {
                h.step(&json!({"click":{"sketch":pick}})).unwrap();
            }
            // The picks determine the dimension, so its value took the keyboard; Tab hands
            // them back to placing first.
            assert_eq!(live(), Some(false));
            h.frames(4);
            h.step(&json!({"key":"Tab"})).unwrap();
            h.step(&json!({"move":{"sketch":at}})).unwrap();
            let (param, draft) = placement_preview(&h.app).unwrap();
            let constraint =
                draft.preview.sketch(draft.sketch).unwrap().sketch.constraints.iter().find(|c| c.param.as_deref() == Some(&param)).unwrap();
            assert_eq!(constraint.kind.is_angle(), angular);
            assert!(!editing());
            let (_, cached) = placement_preview(&h.app).unwrap();
            assert!(Arc::ptr_eq(&draft.preview_doc, &cached.preview_doc), "stationary pointer reuses inference");
            h.app.run("sketch.point", json!({"point":[50,50]})).unwrap();
            let (_, refreshed) = placement_preview(&h.app).unwrap();
            assert!(!Arc::ptr_eq(&draft.preview_doc, &refreshed.preview_doc));
            h.step(&json!({"key":"Escape"})).unwrap();
        }
        cancel();
    }

    /// Placing a dimension the sketch can't take previews it as driven: in the draft it is a
    /// driven constraint with no parameter, labelled in parentheses (#320).
    #[test]
    fn an_over_constraining_placement_previews_a_driven_dimension() {
        cancel();
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.line", json!({"points":[[10,10],[30,10]],"infer":false})).unwrap();
        h.app.run("sketch.dimension", json!({"entities":["l1"],"value":"20 mm"})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        let baseline = h.app.session.doc.to_json();
        h.step(&json!({"start":"sketch.dimension"})).unwrap();
        h.step(&json!({"click":{"sketch":[20,10]}})).unwrap();
        h.step(&json!({"move":{"sketch":[20,20]}})).unwrap();
        h.settle();
        let (id, draft) = placement_preview(&h.app).expect("an over-constraining dimension still previews");
        assert!(draft.driven);
        let sk = &draft.preview.sketch(draft.sketch).unwrap().sketch;
        let c = sk.constraints.iter().find(|c| c.id == id).unwrap();
        assert!(c.driven && c.param.is_none(), "{c:?}");
        assert!(matches!(c.kind, ConstraintKind::Length {value,..} if (value-20.).abs()<1e-6));
        assert_eq!(label(&c.kind, None, c.driven, "mm", 2), "(20.00)");
        assert!(!editing());
        assert_eq!(h.app.session.doc.to_json(), baseline);
        h.step(&json!({"key":"Escape"})).unwrap();
        h.step(&json!({"key":"Escape"})).unwrap();
        cancel();
    }

    #[test]
    fn placement_follows_real_pointer_without_editing_or_mutating_design() {
        cancel();
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.line", json!({"points":[[10,10],[30,10]],"infer":false})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        let baseline = h.app.session.doc.to_json();
        let undo = h.app.session.undo.len();
        h.step(&json!({"start":"sketch.dimension"})).unwrap();
        h.step(&json!({"click":{"sketch":[20,10]}})).unwrap();
        assert_eq!(live(), Some(false));
        h.frames(4);
        h.step(&json!({"key":"Tab"})).unwrap();
        for at in [[20., 20.], [20., -5.]] {
            h.step(&json!({"move":{"sketch":at}})).unwrap();
            h.settle();
            let (param, draft) = placement_preview(&h.app).expect("a line has a dimension preview");
            assert!((draft.at.unwrap() - Vec2::new(at[0], at[1])).len() < 1e-5);
            let sk = &draft.preview.sketch(draft.sketch).unwrap().sketch;
            let c = sk.constraints.iter().find(|c| c.param.as_deref() == Some(&param)).unwrap();
            assert!(matches!(c.kind, ConstraintKind::Length {value,..} if (value-20.).abs()<1e-6));
            assert!(!editing());
            assert!(drawn().is_empty(), "preview must not become a selectable committed dimension");
            assert_eq!(h.app.session.doc.to_json(), baseline);
            assert_eq!(h.app.session.undo.len(), undo);
        }
        h.step(&json!({"click":{"sketch":[20,-5]}})).unwrap();
        assert!(editing());
        assert_eq!(h.app.session.doc.to_json(), baseline);
        h.step(&json!({"key":"Escape"})).unwrap();
        h.step(&json!({"key":"Escape"})).unwrap();
        assert!(placement_preview(&h.app).is_none());
        assert_eq!(h.app.session.doc.to_json(), baseline);
        assert_eq!(h.app.session.undo.len(), undo);
        cancel();
    }

    #[test]
    fn inch_labels_convert_all_lengths_preserve_four_digits_and_leave_angles_alone() {
        let length = ConstraintKind::Length { l: 0, value: 25.4 };
        assert_eq!(label(&length, Some("25.4 mm"), false, "in", 2), "1.000");
        assert_eq!(label(&length, Some("1 in"), false, "in", 2), "1.000");
        assert_eq!(label(&length, Some("1.0000 in"), false, "in", 2), "1.0000");
        let radius = ConstraintKind::Radius { c: 0, value: 1.2345 * 25.4 };
        assert_eq!(label(&radius, Some("1.2345 in"), false, "in", 2), "R1.2345");
        assert_eq!(label(&radius, Some("width / 2"), true, "in", 2), "(fx: R1.2345)");
        let diameter = ConstraintKind::Diameter { c: 0, value: 0.001 * 25.4 };
        assert_eq!(label(&diameter, None, false, "in", 2), "Ø0.001");
        assert_eq!(crate::prefs::length_number(25.4 * (1.0 + 1e-12), "in", 2), "1.000");
        assert_eq!(crate::prefs::length_number(25.4, "in", 5), "1.00000");
        assert_eq!(crate::prefs::length_number(25.4, "cm", 2), "2.54");
        let angle = ConstraintKind::Angle { a: 0, b: 1, value: std::f64::consts::FRAC_PI_4, flip: false };
        assert_eq!(label(&angle, Some("45 deg"), false, "in", 2), "45.0°");
    }

    #[test]
    fn labels_show_the_value_or_fx() {
        let k = ConstraintKind::Length { l: 0, value: 25.0 };
        assert_eq!(label(&k, Some("25 mm"), false, "mm", 2), "25.00");
        assert_eq!(label(&k, Some("width * 2"), false, "mm", 2), "fx: 25.00");
        // A value typed with a decimal comma is a plain value too (#1347).
        assert_eq!(label(&ConstraintKind::Length { l: 0, value: 25.5 }, Some("25,5 mm"), false, "mm", 2), "25.50");
        assert_eq!(label(&k, None, true, "mm", 2), "(25.00)");
        assert_eq!(label(&ConstraintKind::Radius { c: 0, value: 5.0 }, Some("5 mm"), false, "mm", 2), "R5.00");
        assert_eq!(label(&ConstraintKind::Diameter { c: 0, value: 10.0 }, Some("10"), false, "mm", 2), "Ø10.00");
        let a = ConstraintKind::Angle { a: 0, b: 1, value: std::f64::consts::FRAC_PI_4, flip: false };
        assert_eq!(label(&a, Some("45 deg"), false, "mm", 2), "45.0°");
    }
}
