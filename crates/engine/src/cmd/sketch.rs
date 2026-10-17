//! Sketch commands: create/edit/finish sketches, draw curves, constraints and dimensions.

use serde_json::{Value, json};
use solidcraft_doc::{FeatureKind, PlaneRef, expr::Kind};
use solidcraft_geom::{Plane, Vec2, Vec3};
use solidcraft_sketch::{Analysis, Budget, ConstraintKind, CurveKind, Hold, Sketch, SolveReport, solve, solve_with};

use super::{CommandSpec, in_sketch};
use crate::params::{bad, bool_, expr, num, req_vec2, str_, string_list, vec2, vec3};
use crate::{EngineError, Result, Session};

#[path = "sketch_dimensioned.rs"]
mod dimensioned;
#[path = "sketch_drag.rs"]
mod drag;
#[path = "sketch_face.rs"]
pub(super) mod face;
use dimensioned::draw as draw_dimensioned;
use drag::drag_entity;

/// The `connect` parameter every drawing command takes (see `connect_points`).
macro_rules! connect_doc {
    () => {
        "; connect?: bool (default true: a new point landing exactly on an existing point, the origin too, is made coincident with it)"
    };
}

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("sketch.create", "Create Sketch", create_sketch)
        .at("SOLID", "CREATE")
        .icon("sketch")
        .params("plane: XY|XZ|YZ | construction plane name or id | {origin, x_dir, y_dir} | {face: [world x,y,z], body?: exact world body, index?: face index, path?: complete occurrence ids, fixed?: bool}; new face attachments require a unique certified source (a flat face of a sheet metal body attaches to its panel instead, in the flat pattern's coordinates), fixed:true explicitly freezes the selected plane; offset?: expr, flip?: bool | \"x\" | \"y\" (not on a face: sketch the plane from its other side, so the normal and the default extrude direction are reversed; a half turn about the plane's own x or y axis, true picks the one keeping the sketch's y axis upward; offset still moves along the unflipped normal), project_edges?: bool, name?"),
    CommandSpec::new("sketch.edit", "Edit Sketch", edit_sketch).noundo().icon("sketch").params("sketch: id or name"),
    CommandSpec::new("sketch.finish", "Finish Sketch", finish_sketch).at("SKETCH", "FINISH SKETCH").icon("finish").enabled(in_sketch).noundo(),
    CommandSpec::new("sketch.line", "Line", draw_line)
        .at("SKETCH", "CREATE")
        .icon("line")
        .key("L")
        .enabled(in_sketch)
        .params(concat!("points: [[x,y] | \"l1.end\" | {at, ref}…], closed?: bool, construction?: bool, ids?: [..], infer?: bool (add horizontal/vertical)", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.draw.dimensioned", "Draw Dimensioned Sketch Geometry", draw_dimensioned).icon("dimension")
        .enabled(in_sketch)
        .params(concat!("command: sketch creation command, params: object, dimensions: object of role to numeric or expression value; creates geometry and all driving dimensions atomically in one undo step", created_doc!())),
    CommandSpec::new("sketch.rectangle.two_point", "2-Point Rectangle", rect_two_point)
        .at("SKETCH", "CREATE")
        .icon("rect")
        .key("R")
        .enabled(in_sketch)
        .params(concat!("p0, p1: opposite corners [x,y]; construction?. Returns sides {bottom, right, top, left} (line ids) and corners {bottom_left, bottom_right, top_right, top_left} (point ids)", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.rectangle.three_point", "3-Point Rectangle", rect_three_point)
        .at("SKETCH", "CREATE")
        .icon("rect3")
        .enabled(in_sketch)
        .params(concat!("p0, p1: first edge; p2: point on the opposite edge", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.rectangle.rounded", "Rounded Rectangle", rect_rounded)
        .at("SKETCH", "CREATE")
        .icon("rect_rounded")
        .enabled(in_sketch)
        .params(concat!("p0, p1: opposite corners [x,y]; radius: expr (the corner radius, less than half the shorter side); construction?. A 2-point rectangle whose four corners are filleted alike: one radius dimension (`param`), the other arcs equal to it. Returns sides {bottom, right, top, left} (line ids) and arcs (the corner arcs: bottom-left, bottom-right, top-right, top-left)", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.rectangle.center", "Center Rectangle", rect_center)
        .at("SKETCH", "CREATE")
        .icon("rect_center")
        .enabled(in_sketch)
        .params(concat!("center, corner. Returns sides {bottom, right, top, left} (line ids) and corners {bottom_left, bottom_right, top_right, top_left} (point ids)", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.circle.center", "Center Diameter Circle", circle_center)
        .at("SKETCH", "CREATE")
        .icon("circle")
        .key("C")
        .enabled(in_sketch)
        .params(concat!("center: [x,y] or point ref, radius | diameter: number", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.circle.two_point", "2-Point Circle", circle_two)
        .at("SKETCH", "CREATE")
        .icon("circle2")
        .enabled(in_sketch)
        .params(concat!("p0, p1: diameter ends", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.circle.three_point", "3-Point Circle", circle_three)
        .at("SKETCH", "CREATE")
        .icon("circle3")
        .enabled(in_sketch)
        .params(concat!("p0, p1, p2", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.arc.three_point", "3-Point Arc", arc_three).at("SKETCH", "CREATE").icon("arc3").enabled(in_sketch).params(concat!("start, end, through", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.arc.center_point", "Center Point Arc", arc_center)
        .at("SKETCH", "CREATE")
        .icon("arc_center")
        .enabled(in_sketch)
        .params(concat!("center, start, end (counter-clockwise from start) | center, start, sweep: signed degrees", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.polygon.inscribed", "Inscribed Polygon", polygon_inscribed)
        .at("SKETCH", "CREATE")
        .icon("polygon")
        .enabled(in_sketch)
        .params(concat!("center, radius (to vertices), sides, angle?: deg", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.polygon.circumscribed", "Circumscribed Polygon", polygon_circumscribed)
        .at("SKETCH", "CREATE")
        .icon("polygon_circumscribed")
        .enabled(in_sketch)
        .params(concat!("center, radius (to edge midpoints), sides, angle?: deg", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.polygon.edge", "Edge Polygon", polygon_edge)
        .at("SKETCH", "CREATE")
        .icon("polygon_edge")
        .enabled(in_sketch)
        .params(concat!("p0, p1: one edge, sides", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.slot.center_to_center", "Center to Center Slot", slot_c2c)
        .at("SKETCH", "CREATE")
        .icon("slot")
        .enabled(in_sketch)
        .params(concat!("p0, p1: arc centres, width", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.slot.overall", "Overall Slot", slot_overall)
        .at("SKETCH", "CREATE")
        .icon("slot_overall")
        .enabled(in_sketch)
        .params(concat!("p0, p1: overall ends, width", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.point", "Point", draw_point).at("SKETCH", "CREATE").icon("point").enabled(in_sketch).params(concat!("point: [x,y], id?", connect_doc!(), created_doc!())),
    CommandSpec::new("sketch.dimension", "Sketch Dimension", dimension).at("SKETCH", "CREATE").icon("dimension").key("D").enabled(in_sketch).params(
        "entities: [refs], type?: auto|distance|horizontal|vertical|length|radius|diameter|angle, value?: number or expression (default: current; a bare number is in the design's units, e.g. 2 means 2 in in an inch design; give a unit such as \"50 mm\" to override); text_at?: [x, y]. Two points or a line off the axes: with auto, text_at across from them is the distance between them, above or below them horizontal, beside them vertical. Two lines: the angle between them (acute, or 180° − it when the value is nearer that) unless text_at picks a sector. A dimension the sketch already determines is refused (naming what determines it); if_redundant?: \"driven\" places it as a driven (reference) dimension instead. scale_sketch?: bool (the dimension tool sends true): when this is the sketch's first length dimension and nothing in the sketch fixes a size (no other driving length dimension, nothing fixed or projected), the whole sketch scales about its origin to the value instead of distorting. Returns measured in unit (mm or deg), and scaled: the factor when the sketch was scaled",
    ),
    CommandSpec::new("sketch.constraint.horizontal_vertical", "Horizontal/Vertical", c_horizontal_vertical)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_hv")
        .enabled(in_sketch)
        .params("line: id | points: [a, b]; mode?: horizontal|vertical"),
    CommandSpec::new("sketch.constraint.coincident", "Coincident", c_coincident)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_coincident")
        .enabled(in_sketch)
        .params("a, b: points, a point and a curve or sketch axis (x / y: the sketch's own axes, unless a curve has that id; an axis becomes a linked construction line on it, reported in created), or two distinct lines (joins their closest endpoints)"),
    CommandSpec::new("sketch.constraint.tangent", "Tangent", c_tangent)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_tangent")
        .enabled(in_sketch)
        .params("a, b: curves or sketch axes (x / y: the sketch's own axes, unless a curve has that id; an axis becomes a linked construction line on it, reported in created)"),
    CommandSpec::new("sketch.constraint.equal", "Equal", c_equal)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_equal")
        .enabled(in_sketch)
        .params("a, b: lines or circles/arcs"),
    CommandSpec::new("sketch.constraint.parallel", "Parallel", c_parallel)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_parallel")
        .enabled(in_sketch)
        .params("a, b: lines or sketch axes (x / y: the sketch's own axes, unless a curve has that id; an axis becomes a linked construction line on it, reported in created)"),
    CommandSpec::new("sketch.constraint.perpendicular", "Perpendicular", c_perpendicular)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_perpendicular")
        .enabled(in_sketch)
        .params("a, b: lines or sketch axes (x / y: the sketch's own axes, unless a curve has that id; an axis becomes a linked construction line on it, reported in created)"),
    CommandSpec::new("sketch.constraint.fix", "Fix/UnFix", c_fix)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_fix")
        .enabled(in_sketch)
        .params("entity: point or curve | entities: [points and curves]; fixed?: bool (default: fix them all unless all are fixed, then unfix)"),
    CommandSpec::new("sketch.constraint.midpoint", "MidPoint", c_midpoint)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_midpoint")
        .enabled(in_sketch)
        .params("point, line"),
    CommandSpec::new("sketch.constraint.concentric", "Concentric", c_concentric)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_concentric")
        .enabled(in_sketch)
        .params("a, b: circles/arcs"),
    CommandSpec::new("sketch.constraint.collinear", "Collinear", c_collinear)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_collinear")
        .enabled(in_sketch)
        .params("a, b: lines or sketch axes (x / y: the sketch's own axes, unless a curve has that id; an axis becomes a linked construction line on it, reported in created)"),
    CommandSpec::new("sketch.constraint.symmetry", "Symmetry", c_symmetry)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_symmetry")
        .enabled(in_sketch)
        .params("a, b: two points or two curves (lines mirror their ends, circles/arcs their centres); line: symmetry line id, or x / y for the sketch's own axes (a curve with that id wins; an axis becomes a linked construction line on it, reported in created); or point (or a point id as line, \"origin\" too): symmetric about that point, each pair joined by a construction line it is the midpoint of"),
    CommandSpec::new("sketch.construction", "Normal/Construction", construction)
        .at("SKETCH", "CREATE")
        .icon("construction")
        .key("X")
        .enabled(in_sketch)
        .params("curves: [ids], value?: bool (default: construction unless all of them already are)"),
    CommandSpec::new("sketch.delete", "Delete Sketch Entities", sketch_delete).icon("delete")
        .enabled(in_sketch)
        .params("entities: [curve, point or constraint ids]"),
    CommandSpec::new("sketch.move_point", "Drag Sketch Point", move_point).icon("sketch_move").enabled(in_sketch).params("point: ref, to: [x,y]"),
    CommandSpec::new("sketch.drag", "Drag Sketch Geometry", drag).icon("sketch_move").enabled(in_sketch).live(drag_live).params(
        "entity: point or curve, from: [x,y] where it was grabbed, to: [x,y], live?: bool: a point goes to `to`, a line (spline, conic) slides by to - from, a circle or arc grabbed on its edge takes the radius to `to`; the rest moves as little as it can and keeps its solution branch (a long move is followed in short steps, each solved with the entity held; a step that cannot be solved that way is approached as far as it can be and the next ones tried; when none past it solves, the last one is left to the sketch's ordinary solve, which may pull it back or leave the sketch unsolved; a step that would sweep an arc through its start is undone and the drag ends there). Returns `reached`: how much of the way from `from` to `to` the geometry followed (0..1), and `at`: where the grabbed spot went (`from` moved as the dragged points did), the `from` of the next move. `live: true` (each frame of an interactive drag) solves the sketch once and leaves the features after it as they were (stale) until `sketch.drag_end` or any command that reads the model; `dof` is then the one from before the drag",
    ),
    CommandSpec::new("sketch.drag_end", "End Sketch Drag", drag_end).icon("sketch_move").enabled(in_sketch).doc_only().params(
        "constraint?: {command: a sketch.constraint.* id, params}: ends a drag made of `sketch.drag` `live: true` frames: adds the constraint it let go on (not kept if it over-constrains the sketch), then evaluates the whole model once; returns the constraint's result and the sketch's solved state and degrees of freedom",
    ),
    CommandSpec::new("sketch.dimension_text", "Move Dimension Text", dimension_text).icon("edit")
        .enabled(in_sketch)
        .params("dimension: constraint id or parameter name; at: [x,y] text centre (sketch coordinates) | reset: true (default place)"),
    CommandSpec::new("sketch.options", "Sketch Palette Options", sketch_options).icon("sketch").params(
        "sketch?: id|name (default active); any of show_profile, show_points, show_dimensions, show_constraints, show_projected, show_construction, grid, snap, slice, sketch_3d: bool. Returns them all",
    ),
    CommandSpec::new("sketch.chain", "Chain", chain_cmd).icon("chain")
        .enabled(in_sketch)
        .noundo()
        .params("curve: id: the curves joined to it end to end (what a double-click selects)"),
    CommandSpec::new("sketch.solve", "Solve Sketch", solve_cmd).icon("compute")
        .enabled(in_sketch)
        .noundo()
        .params("reports the solver status and degrees of freedom"),
];

// ---------------------------------------------------------------------------------------------
// Helpers

/// Resolve the sketch a command acts on: `sketch` param (id or name) or the active sketch.
pub(super) fn target_sketch(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    if let Some(v) = p.get("sketch") {
        let key = match v {
            Value::Number(n) => n.to_string(),
            Value::String(x) => x.clone(),
            _ => return Err(bad(cmd, "`sketch` must be an id or a name")),
        };
        let f = s.doc.find_feature(&key).ok_or_else(|| bad(cmd, format!("no sketch `{key}`")))?;
        if !matches!(f.kind, FeatureKind::Sketch { .. }) {
            return Err(bad(cmd, format!("`{key}` is not a sketch")));
        }
        return Ok(f.id);
    }
    s.active_sketch.ok_or_else(|| EngineError::Other("no sketch is being edited".into()))
}

/// Edit a sketch: clone it, apply `f`, solve with the document's dimension values and store
/// it. With `strict`, an edit that makes the constraints unsolvable is rejected.
pub(super) fn edit<T>(
    s: &mut Session,
    p: &Value,
    cmd: &str,
    strict: bool,
    f: impl FnOnce(&mut Sketch, &mut solidcraft_doc::Document) -> Result<T>,
) -> Result<(T, Value)> {
    let id = target_sketch(s, p, cmd)?;
    let (out, rep) = edit_solved(s, id, strict, |sk, doc| Ok((f(sk, doc)?, Left::Unsolved)))?;
    Ok((out, rep.map(|rep| report_json(&rep)).unwrap_or(Value::Null)))
}

/// A solve's outcome as commands return it.
pub(super) fn report_json(rep: &SolveReport) -> Value {
    json!({"solved": rep.keeps_orientation(), "dof": rep.dof})
}

/// What an [`edit_solved`] step leaves for the edit's own solve.
pub(super) enum Left {
    /// The sketch, solved from where it is.
    Unsolved,
    /// The sketch, solved keeping the orientation of these positions (the last solved ones,
    /// before a drag left a step placed but unsolved).
    MovedFrom(Vec<Vec2>),
    /// The sketch, solved with the dimensions' values (from the parameters) applied.
    Solved,
}

impl Left {
    /// Solved again by the edit even when already solved (for its report).
    pub(super) fn solve_again(self) -> Left {
        match self {
            Left::Solved => Left::Unsolved,
            left => left,
        }
    }
}

/// `edit` of sketch `id` for a step that may solve the sketch itself: when it returns
/// [`Left::Solved`] (and offsets need not be recomputed) the sketch is not solved again and the
/// report is `None`.
pub(super) fn edit_solved<T>(
    s: &mut Session,
    id: u64,
    strict: bool,
    f: impl FnOnce(&mut Sketch, &mut solidcraft_doc::Document) -> Result<(T, Left)>,
) -> Result<(T, Option<SolveReport>)> {
    let mut doc = (*s.doc).clone();
    let mut sk = doc.sketch(id)?.clone();
    super::sketch_project::refresh(s, &doc, id, &mut sk);
    // The stored sketch is solved for its stored values.
    let stored = sk.driving_values();
    let (out, left) = f(&mut sk, &mut doc)?;
    let offsets = !sk.offsets.is_empty() || !sk.freeform_offsets.is_empty();
    let rep = match left {
        // A drag step solved it (held, with the dimensions' values applied).
        Left::Solved if !offsets => None,
        // A drag left a step placed but unsolved: solve it keeping the orientation of the last
        // solved positions. A drag changes no values, so there is nothing to continue.
        Left::MovedFrom(reference) => {
            doc.apply_dimension_values(&doc.param_values().0, &mut sk)?;
            sk.refresh_offsets()?;
            let unsolved = sk.clone();
            let mut rep = counted_solve(&mut sk, &Hold::default(), Some(&reference), Analysis::Full, None);
            if offsets {
                sk.refresh_offsets()?;
                rep = counted_solve(&mut sk, &Hold::default(), Some(&reference), Analysis::Full, None);
            }
            Some(settle(strict, rep, &mut sk, unsolved)?)
        }
        // Solved for the document's values, continuing from the stored solved state.
        Left::Solved | Left::Unsolved => {
            let (vals, _) = doc.param_values();
            sk.refresh_offsets()?;
            let unsolved = sk.clone();
            #[cfg(test)]
            solve_counts::record(Analysis::Full);
            let rep = doc.solve_sketch(&vals, &mut sk, &stored, None)?;
            Some(settle(strict, rep, &mut sk, unsolved)?)
        }
    };
    sk.validate_periodic_splines()?;
    *doc.sketch_mut(id)? = sk;
    *s.doc_mut() = doc;
    Ok((out, rep))
}

/// An edit's solve outcome: with `strict`, a solve that fails or turns the sketch over is an
/// error; otherwise such a sketch is stored as the edit left it before the solve, its values
/// and geometry together, so the stored sketch stays solved for its stored values.
fn settle(strict: bool, rep: SolveReport, sk: &mut Sketch, unsolved: Sketch) -> Result<SolveReport> {
    if strict && !rep.keeps_orientation() {
        return Err(EngineError::Other(rep.failure(|f| format!("that would over-constrain the sketch (conflicts with {f})"))));
    }
    if !rep.keeps_orientation() {
        *sk = unsolved;
    }
    Ok(rep)
}

/// The sketch solves of `edit` and drags, counted in tests (see [`solve_with`]).
pub(super) fn counted_solve(
    sk: &mut Sketch,
    hold: &Hold,
    reference: Option<&[Vec2]>,
    analysis: Analysis,
    budget: Option<&mut Budget>,
) -> SolveReport {
    #[cfg(test)]
    solve_counts::record(analysis);
    solve_with(sk, hold, reference, analysis, budget)
}

/// Inside an `edit` closure: solve the sketch for the document's parameter values from its
/// stored solved state, as the edit's own solve does, for commands that work on the current
/// geometry.
pub(super) fn solve_current(sk: &mut Sketch, doc: &solidcraft_doc::Document) -> Result<SolveReport> {
    let stored = sk.driving_values();
    let (values, _) = doc.param_values();
    Ok(doc.solve_sketch(&values, sk, &stored, None)?)
}

/// Solves run on this thread by [`counted_solve`] and the edit's own solve: (all of them, those
/// with a rank analysis).
#[cfg(test)]
pub(super) mod solve_counts {
    use std::cell::Cell;

    use solidcraft_sketch::Analysis;

    thread_local! {
        static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
    }

    pub fn record(analysis: Analysis) {
        COUNTS.with(|c| {
            let (n, full) = c.get();
            c.set((n + 1, full + usize::from(analysis == Analysis::Full)));
        });
    }

    /// The counts since the last call.
    pub fn take() -> (usize, usize) {
        COUNTS.with(|c| c.replace((0, 0)))
    }
}

/// How close (mm) a new point must land to an existing one to be connected to it.
const CONNECT_TOL: f64 = 1e-6;

/// New points (index `first` on) that land on an existing point, the origin included, get a
/// coincident constraint to it, as when a click snaps there; `connect: false` leaves them free.
fn connect_points(sk: &mut Sketch, first: usize, p: &Value) -> Result<Vec<String>> {
    if !bool_(p, "connect").unwrap_or(true) {
        return Ok(Vec::new());
    }
    // The existing points by x, for a range lookup per new point.
    let mut old: Vec<(Vec2, usize)> = (0..first.min(sk.points.len())).filter_map(|i| Some((sk.point(i)?, i))).collect();
    old.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    let mut ties = Vec::new();
    for q in first..sk.points.len() {
        let Some(at) = sk.point(q) else { continue };
        let from = old.partition_point(|(o, _)| o.x < at.x - CONNECT_TOL);
        let hit =
            old.iter().skip(from).take_while(|(o, _)| o.x <= at.x + CONNECT_TOL).filter(|(o, _)| o.dist(at) < CONNECT_TOL).map(|(_, i)| *i).min();
        if let Some(o) = hit {
            ties.push((o, q));
        }
    }
    ties.into_iter().map(|(o, q)| add_c(sk, ConstraintKind::Coincident { p: o, q })).collect()
}

/// `edit` for drawing commands whose step returns (curve ids, constraint ids): the points it
/// adds are connected to the existing points they land on (see [`connect_points`]).
pub(super) fn draw(
    s: &mut Session,
    p: &Value,
    cmd: &str,
    f: impl FnOnce(&mut Sketch, &mut solidcraft_doc::Document) -> Result<(Vec<String>, Vec<String>)>,
) -> Result<((Vec<String>, Vec<String>), Value)> {
    edit(s, p, cmd, false, |sk, doc| {
        let first = sk.points.len();
        let (curves, mut cons) = f(sk, doc)?;
        cons.extend(connect_points(sk, first, p)?);
        drop_implied_axes(sk, &mut cons);
        Ok((curves, cons))
    })
}

/// A point argument: `[x, y]` (new point), a point reference string, or `{at, ref}`.
#[derive(Clone)]
pub(super) enum PArg {
    At(Vec2),
    Ref(usize, Vec2),
}

impl PArg {
    pub(super) fn pos(&self) -> Vec2 {
        match self {
            PArg::At(p) | PArg::Ref(_, p) => *p,
        }
    }
    pub(super) fn idx(&self) -> Option<usize> {
        match self {
            PArg::Ref(i, _) => Some(*i),
            PArg::At(_) => None,
        }
    }
}

pub(super) fn parg(sk: &Sketch, v: &Value, cmd: &str) -> Result<PArg> {
    if let Some(p) = vec2(v) {
        return Ok(PArg::At(p));
    }
    let r = match v {
        Value::String(r) => Some(r.as_str()),
        Value::Object(o) => o.get("ref").and_then(Value::as_str),
        _ => None,
    };
    if let Some(r) = r {
        let i = sk.resolve_point(r).ok_or_else(|| bad(cmd, format!("unknown sketch point `{r}`")))?;
        return Ok(PArg::Ref(i, sk.point(i).unwrap_or_default()));
    }
    if let Some(at) = v.get("at").and_then(vec2) {
        return Ok(PArg::At(at));
    }
    Err(bad(cmd, "a point must be [x, y], a point id like \"l1.end\", or {at, ref}"))
}

pub(super) fn req_parg(sk: &Sketch, p: &Value, k: &str, cmd: &str) -> Result<PArg> {
    parg(sk, p.get(k).ok_or_else(|| bad(cmd, format!("missing `{k}`")))?, cmd)
}

pub(super) fn curve_ref(sk: &Sketch, v: Option<&Value>, cmd: &str, what: &str) -> Result<usize> {
    let id = v.and_then(Value::as_str).ok_or_else(|| bad(cmd, format!("`{what}` must be a curve id")))?;
    sk.curve_index(id).ok_or_else(|| bad(cmd, format!("unknown sketch curve `{id}`")))
}

/// A line a constraint refers to: a curve id, or `x` / `y` for the sketch's own axes (a curve
/// with that id wins). An axis becomes the sketch's linked construction line on it
/// ([`Sketch::sketch_axis_line`]), added on first use and reported in `created`.
pub(super) fn line_or_axis_ref(sk: &mut Sketch, v: Option<&Value>, cmd: &str, what: &str) -> Result<usize> {
    let id = v.and_then(Value::as_str).ok_or_else(|| bad(cmd, format!("`{what}` must be a curve id, or x / y for a sketch axis")))?;
    curve_or_axis(sk, id)?.ok_or_else(|| bad(cmd, format!("unknown sketch curve `{id}` (x and y name the sketch's own axes)")))
}

/// Curve `id`, or the line of sketch axis `id` (see [`line_or_axis_ref`]); `None` when neither.
fn curve_or_axis(sk: &mut Sketch, id: &str) -> Result<Option<usize>> {
    match sk.curve_index(id) {
        Some(c) => Ok(Some(c)),
        None => Ok(sk.sketch_axis_line(id)?),
    }
}

pub(super) fn point_ref(sk: &Sketch, v: Option<&Value>, cmd: &str, what: &str) -> Result<usize> {
    let id = v.and_then(Value::as_str).ok_or_else(|| bad(cmd, format!("`{what}` must be a point reference")))?;
    sk.resolve_point(id).ok_or_else(|| bad(cmd, format!("unknown sketch point `{id}`")))
}

pub(super) fn ids_of(sk: &Sketch, curves: &[usize]) -> Vec<String> {
    curves.iter().filter_map(|c| sk.curves.get(*c).map(|c| c.id.clone())).collect()
}

pub(super) fn add_c(sk: &mut Sketch, k: ConstraintKind) -> Result<String> {
    Ok(sk.add_constraint(k, None)?)
}

pub(super) fn mark_construction(sk: &mut Sketch, curves: &[usize], p: &Value) {
    if bool_(p, "construction").unwrap_or(false) {
        for c in curves {
            if let Some(cu) = sk.curves.get_mut(*c) {
                cu.construction = true;
            }
        }
    }
}

/// What a command added to a sketch: the ids of the points, curves, constraints and dimensions
/// in `after` that `before` does not have (`None`: a new sketch), in creation order, plus the
/// 3D curves (`wires`) when there are any. Ids are stable and never reused in a sketch, so they
/// can be used in later commands (`sketch.dimension`, `sketch.constraint.*`, `sketch.delete`…).
pub(crate) fn created(before: Option<&Sketch>, after: &Sketch) -> Value {
    use std::collections::HashSet;
    let empty = Sketch::new();
    let before = before.unwrap_or(&empty);
    let old: HashSet<&str> = before
        .points
        .iter()
        .map(|x| x.id.as_str())
        .chain(before.curves.iter().map(|x| x.id.as_str()))
        .chain(before.constraints.iter().map(|x| x.id.as_str()))
        .chain(before.wires.iter().map(|x| x.id.as_str()))
        .collect();
    let new = |id: &&str| !old.contains(*id);
    let points: Vec<&str> = after.points.iter().map(|x| x.id.as_str()).filter(new).collect();
    let curves: Vec<&str> = after.curves.iter().map(|x| x.id.as_str()).filter(new).collect();
    let (dims, cons): (Vec<_>, Vec<_>) = after.constraints.iter().filter(|c| new(&c.id.as_str())).partition(|c| c.kind.is_dimension());
    let ids = |v: Vec<&solidcraft_sketch::Constraint>| v.into_iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    let mut out = json!({"points": points, "curves": curves, "constraints": ids(cons), "dimensions": ids(dims)});
    let wires: Vec<&str> = after.wires.iter().map(|x| x.id.as_str()).filter(new).collect();
    if !wires.is_empty()
        && let Some(o) = out.as_object_mut()
    {
        o.insert("wires".into(), json!(wires));
    }
    out
}

pub(super) fn result(sk_out: (Vec<String>, Vec<String>), info: Value) -> Value {
    json!({"curves": sk_out.0, "constraints": sk_out.1, "sketch": info})
}

// ---------------------------------------------------------------------------------------------
// Sketch lifecycle

pub(crate) fn author_path_for_rebind(s: &Session, feature: &solidcraft_doc::Feature, p: &Value, cmd: &str) -> Result<Vec<u64>> {
    let mut saved = None;
    if let FeatureKind::Sketch { plane, .. } = &feature.kind {
        let mut plane = plane;
        loop {
            match plane {
                PlaneRef::Offset { base, .. } | PlaneRef::AtAngle { base, .. } | PlaneRef::Flipped { base, .. } => plane = base,
                PlaneRef::Face { anchor: Some(anchor), .. } => {
                    saved = Some(anchor.author_path.clone());
                    break;
                }
                _ => break,
            }
        }
    }
    let path = if let Some(value) = p.get("author_path") {
        value
            .as_array()
            .filter(|path| path.len() <= 64)
            .ok_or_else(|| bad(cmd, "author_path must contain at most64 ids"))?
            .iter()
            .map(|id| id.as_u64().filter(|id| *id != 0).ok_or_else(|| bad(cmd, "invalid author_path id")))
            .collect::<Result<Vec<_>>>()?
    } else if let Some(path) = saved {
        path
    } else if feature.component == s.active_component && s.active_occurrence_path.is_some() {
        s.active_occurrence_path.clone().unwrap_or_default()
    } else {
        let placements = super::component::properties::placements(&s.placed_document(), cmd)?;
        let paths: Vec<_> = placements.into_iter().filter(|(component, _, _)| *component == feature.component).map(|(_, path, _)| path).collect();
        if paths.len() != 1 {
            return Err(bad(cmd, "sketch author placement is ambiguous; provide exact author_path or activate the intended occurrence"));
        }
        paths.into_iter().next().ok_or_else(|| bad(cmd, "sketch author placement is missing"))?
    };
    if feature.component != 0 && path.is_empty() {
        return Err(bad(cmd, "a component sketch needs an explicit complete author_path"));
    }
    solidcraft_doc::EmbossFace::component_frame(&s.placed_document(), feature.component, &path)?;
    Ok(path)
}

pub(super) fn plane_ref(s: &Session, p: &Value, cmd: &str) -> Result<PlaneRef> {
    let base = match p.get("plane") {
        None => PlaneRef::Origin { name: "XY".into() },
        // An origin plane, or a construction plane by name or id.
        Some(v @ (Value::String(_) | Value::Number(_))) => super::features::plane_param(s, Some(v), cmd)?,
        Some(v @ Value::Object(o)) => {
            if o.contains_key("face") {
                face::capture(s, v, cmd)?
            } else {
                let origin = v.get("origin").and_then(vec3).unwrap_or(Vec3::ZERO);
                let x = v.get("x_dir").and_then(vec3).ok_or_else(|| bad(cmd, "plane needs x_dir"))?;
                let y = v.get("y_dir").and_then(vec3).ok_or_else(|| bad(cmd, "plane needs y_dir"))?;
                PlaneRef::Custom { plane: Plane::new(origin, x, y).ok_or_else(|| bad(cmd, "degenerate plane"))? }
            }
        }
        _ => return Err(bad(cmd, "`plane` must be XY, XZ, YZ, a construction plane name or id, or an object")),
    };
    Ok(match expr(p, "offset") {
        Some(d) => PlaneRef::Offset { base: Box::new(base), distance: d },
        None => base,
    })
}

/// Does triangle set of face `fi` contain point `p` (within `tol` of the face plane)?
pub(super) fn face_contains(m: &solidcraft_geom::Mesh, fi: usize, p: Vec3, tol: f64) -> bool {
    m.triangles.iter().zip(&m.tri_face).filter(|(_, f)| **f as usize == fi).any(|(t, _)| {
        let Some([a, b, c]) = m.tri(t) else { return false };
        let n = (b - a).cross(c - a);
        let Some(nn) = n.normalized() else { return false };
        if (p - a).dot(nn).abs() > tol {
            return false;
        }
        let s1 = (b - a).cross(p - a).dot(n);
        let s2 = (c - b).cross(p - b).dot(n);
        let s3 = (a - c).cross(p - c).dot(n);
        (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0)
            || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0)
            // Exact curved boundaries can sit just outside their tessellated chords.
            || p.dist_to_segment(a, b).min(p.dist_to_segment(b, c)).min(p.dist_to_segment(c, a)) <= tol * 0.1
    })
}

fn create_sketch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.create";
    // These name the plane, so they belong inside `plane`. Next to it they were ignored and the
    // sketch went to XY (or the plane given) without a word.
    for (key, example) in [
        ("face", r#"{"plane": {"face": [x, y, z]}}"#),
        ("origin", r#"{"plane": {"origin": [x, y, z], "x_dir": [..], "y_dir": [..]}}"#),
        ("x_dir", r#"{"plane": {"origin": [x, y, z], "x_dir": [..], "y_dir": [..]}}"#),
        ("y_dir", r#"{"plane": {"origin": [x, y, z], "x_dir": [..], "y_dir": [..]}}"#),
    ] {
        if p.get(key).is_some() {
            return Err(bad(cmd, format!("`{key}` belongs inside `plane`: {example}")));
        }
    }
    let mut plane = plane_ref(s, p, cmd)?;
    if let Some(about_y) = flip_param(s, p, &plane, cmd)? {
        plane = PlaneRef::Flipped { base: Box::new(plane), about_y };
    }
    create_sketch_with_plane(s, p, plane, None)
}

/// `flip` of `sketch.create`: `None` keeps the plane's own side, otherwise the axis to turn
/// about (`true`: about y when the plane's y axis points upward, so it stays up, else about x).
fn flip_param(s: &Session, p: &Value, plane: &PlaneRef, cmd: &str) -> Result<Option<bool>> {
    let about_y = match p.get("flip") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => return Ok(None),
        Some(Value::String(a)) if a.eq_ignore_ascii_case("x") => false,
        Some(Value::String(a)) if a.eq_ignore_ascii_case("y") => true,
        Some(Value::Bool(true)) => {
            let (vals, _) = s.doc.param_values();
            let resolved = s.doc.resolve_plane_in(&vals, plane, 0, Some(&s.model.state()))?;
            resolved.y.z > 1e-9
        }
        Some(_) => return Err(bad(cmd, "`flip` must be true, false, \"x\" or \"y\"")),
    };
    if face::is_face(p) {
        return Err(bad(cmd, "`flip` is for origin, construction and explicit planes; a face already faces outward"));
    }
    Ok(Some(about_y))
}

/// Used only by face Extrude/Revolve/PressPull, preserving pre-certificate linked profiles.
pub(super) fn create_face_profile(s: &mut Session, p: &Value, body: &str, index: usize) -> Result<Value> {
    let value = p.get("plane").ok_or_else(|| bad("face profile", "missing picked face plane"))?;
    let plane = face::capture_internal(s, value, "face profile")?;
    create_sketch_with_plane(s, p, plane, Some((body, index)))
}

fn create_sketch_with_plane(s: &mut Session, p: &Value, plane: PlaneRef, internal_body: Option<(&str, usize)>) -> Result<Value> {
    if face::is_face(p) {
        face::capture_positions(s, &plane)?;
    }
    // Validate the plane resolves.
    let (vals, _) = s.doc.param_values();
    s.doc.resolve_plane_in(&vals, &plane, 0, Some(&s.model.state()))?;
    let name = str_(p, "name");
    let mut kind = FeatureKind::Sketch { plane, sketch: Sketch::new() };
    if !face::is_face(p) {
        super::component::to_active_frame(s, &mut kind);
    }
    let id = s.doc_mut().add_feature(kind, name)?;
    let comp = s.active_component;
    if let Some(f) = s.doc_mut().feature_mut(id) {
        f.component = comp;
    }
    s.active_sketch = Some(id);
    let name = s.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default();
    // Sketching on a face projects the face's edges (unless turned off).
    let face = p.get("plane").and_then(|v| v.get("face")).and_then(vec3);
    let mut link = None;
    if let Some(at) = face
        && bool_(p, "project_edges").unwrap_or(s.auto_project)
    {
        link = super::sketch_project::auto_project_face(s, id, at)?;
        if link.is_none()
            && let Some((body, index)) = internal_body
        {
            link = Some(super::sketch_project::project_internal_face(s, id, body, index)?);
        }
    }
    let attachment = s.doc.feature(id).and_then(|feature| match &feature.kind {
        FeatureKind::Sketch { plane, .. } => Some(if face::anchor(plane).is_some() {
            "exact_face"
        } else if solidcraft_doc::sheet::plane_sheet_face(plane).is_some() {
            "sheet_face"
        } else if internal_body.is_some() {
            "internal_legacy_face"
        } else {
            "fixed"
        }),
        _ => None,
    });
    Ok(json!({"sketch": id, "name": name, "projected": link,"attachment":attachment}))
}

fn edit_sketch(s: &mut Session, p: &Value) -> Result<Value> {
    let key = match p.get("sketch") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad("sketch.edit", "`sketch` must be an id or a name")),
    };
    let f = s.doc.find_feature(&key).ok_or_else(|| bad("sketch.edit", format!("no sketch `{key}`")))?;
    if !matches!(f.kind, FeatureKind::Sketch { .. }) {
        return Err(bad("sketch.edit", format!("`{key}` is not a sketch")));
    }
    let (id, name, component) = (f.id, f.name.clone(), f.component);
    let authored_path = match &f.kind {
        FeatureKind::Sketch { plane, .. } => face::anchor(plane).map(|anchor| anchor.author_path.clone()),
        _ => None,
    };
    if let Some(path) = authored_path {
        solidcraft_doc::EmbossFace::component_frame(&s.placed_document(), component, &path)
            .map_err(|error| bad("sketch.edit", format!("{error}; explicitly rebind the author placement before editing")))?;
        s.active_component = component;
        s.active_occurrence_path = (!path.is_empty()).then_some(path);
    }
    s.active_sketch = Some(id);
    s.revision += 1;
    Ok(json!({"sketch": id, "name": name}))
}

fn finish_sketch(s: &mut Session, _p: &Value) -> Result<Value> {
    let id = s.active_sketch.take();
    s.revision += 1;
    let profiles = id.and_then(|i| s.model.state().sketch(i).map(|ss| ss.profiles.len())).unwrap_or(0);
    Ok(json!({"sketch": id, "profiles": profiles}))
}

// ---------------------------------------------------------------------------------------------
// Drawing

fn draw_line(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.line";
    let p = &super::sketch_project::snap_points(s, p, cmd)?;
    let list = p.get("points").and_then(Value::as_array).cloned().ok_or_else(|| bad(cmd, "`points` must be a list"))?;
    if list.len() < 2 || list.len() > 10_000 {
        return Err(bad(cmd, "a line needs 2 or more points"));
    }
    let closed = bool_(p, "closed").unwrap_or(false);
    let infer = bool_(p, "infer").unwrap_or(false);
    let ids = string_list(p, "ids");
    let ((curves, cons), info) = draw(s, p, cmd, |sk, _| {
        let args: Vec<PArg> = list.iter().map(|v| parg(sk, v, cmd)).collect::<Result<_>>()?;
        let n = args.len();
        let segs = if closed { n } else { n - 1 };
        let mut pts: Vec<Option<usize>> = args.iter().map(PArg::idx).collect();
        let mut curves = Vec::new();
        let mut cons = Vec::new();
        for i in 0..segs {
            let j = (i + 1) % n;
            let (Some(a), Some(b)) = (args.get(i), args.get(j)) else { continue };
            if a.pos().dist(b.pos()) < 1e-9 {
                return Err(bad(cmd, "zero-length line"));
            }
            let id = ids.get(i).map(String::as_str);
            let c = sk.add_line(a.pos(), b.pos(), pts.get(i).copied().flatten(), pts.get(j).copied().flatten(), id)?;
            if let Some(CurveKind::Line { a: pa, b: pb }) = sk.curves.get(c).map(|c| c.kind.clone()) {
                if let Some(slot) = pts.get_mut(i) {
                    *slot = Some(pa);
                }
                if let Some(slot) = pts.get_mut(j) {
                    *slot = Some(pb);
                }
            }
            if infer {
                let d = b.pos() - a.pos();
                let axis = if d.y.abs() <= d.len() * 1e-9 {
                    cons.push(add_c(sk, ConstraintKind::Horizontal { l: c })?);
                    true
                } else if d.x.abs() <= d.len() * 1e-9 {
                    cons.push(add_c(sk, ConstraintKind::Vertical { l: c })?);
                    true
                } else {
                    false
                };
                if let Some(k) = infer_at_start(sk, c, axis) {
                    cons.push(add_c(sk, k)?);
                }
            }
            curves.push(c);
        }
        mark_construction(sk, &curves, p);
        Ok((ids_of(sk, &curves), cons))
    })?;
    Ok(result((curves, cons), info))
}

/// A relation the new line `c` has with the curve it starts from (drawn exactly so): tangent
/// to an arc, or perpendicular to a line (parallel continuations are left alone). `axis`: the
/// line already got horizontal/vertical, so a perpendicular to an axis-aligned line would be
/// redundant.
fn infer_at_start(sk: &Sketch, c: usize, axis: bool) -> Option<ConstraintKind> {
    let CurveKind::Line { a, b } = sk.curves.get(c)?.kind else { return None };
    let (pa, pb) = (sk.point(a)?, sk.point(b)?);
    let u = (pb - pa).normalized()?;
    for (i, o) in sk.curves.iter().enumerate() {
        if i == c || !o.kind.uses(a) {
            continue;
        }
        match o.kind {
            CurveKind::Arc { c: cc, .. } => {
                let r = (pa - sk.point(cc)?).normalized()?;
                if r.dot(u).abs() < 1e-9 {
                    return Some(ConstraintKind::Tangent { a: i, b: c });
                }
            }
            CurveKind::Line { a: oa, b: ob } => {
                let v = (sk.point(ob)? - sk.point(oa)?).normalized()?;
                let o_axis = v.x.abs() < 1e-9 || v.y.abs() < 1e-9;
                if u.dot(v).abs() < 1e-9 && !(axis && o_axis) {
                    return Some(ConstraintKind::Perpendicular { a: i, b: c });
                }
            }
            _ => {}
        }
    }
    None
}

/// Closed polygon of lines through new or referenced corner points.
fn polygon_lines(sk: &mut Sketch, corners: &[Vec2]) -> Result<(Vec<usize>, Vec<usize>)> {
    let pts: Vec<usize> = corners.iter().map(|c| sk.add_point(*c, None)).collect::<std::result::Result<_, _>>()?;
    let n = pts.len();
    let mut lines = Vec::new();
    for i in 0..n {
        let (Some(a), Some(b)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
        lines.push(sk.add_line_pts(*a, *b, None)?);
    }
    Ok((lines, pts))
}

fn rect_constraints(sk: &mut Sketch, l: &[usize]) -> Result<Vec<String>> {
    let mut cons = Vec::new();
    if let [a, b, c, d] = l[..] {
        cons.push(add_c(sk, ConstraintKind::Horizontal { l: a })?);
        cons.push(add_c(sk, ConstraintKind::Horizontal { l: c })?);
        cons.push(add_c(sk, ConstraintKind::Vertical { l: b })?);
        cons.push(add_c(sk, ConstraintKind::Vertical { l: d })?);
    }
    Ok(cons)
}

/// Whether line `l` being horizontal (`horizontal`) or vertical already follows from another
/// constraint than `own`: its ends coincide with the ends of a line constrained the same way (a
/// side two rectangles share), or with two points constrained level (or plumb).
fn axis_implied(sk: &Sketch, l: usize, horizontal: bool, own: &str) -> bool {
    let Some(CurveKind::Line { a, b }) = sk.curves.get(l).map(|c| &c.kind) else { return false };
    let (a, b) = (*a, *b);
    // Coincident points as one (union-find over the coincident constraints).
    let mut parent: Vec<usize> = (0..sk.points.len()).collect();
    let root = |parent: &[usize], mut i: usize| {
        for _ in 0..parent.len() {
            match parent.get(i) {
                Some(&up) if up != i => i = up,
                _ => break,
            }
        }
        i
    };
    for c in &sk.constraints {
        if let ConstraintKind::Coincident { p, q } = c.kind {
            let (rp, rq) = (root(&parent, p), root(&parent, q));
            if rp != rq
                && let Some(slot) = parent.get_mut(rp)
            {
                *slot = rq;
            }
        }
    }
    let (ra, rb) = (root(&parent, a), root(&parent, b));
    sk.constraints.iter().filter(|c| c.id != own).any(|c| {
        let ends = match (&c.kind, horizontal) {
            (ConstraintKind::Horizontal { l: m }, true) | (ConstraintKind::Vertical { l: m }, false) if *m != l => {
                match sk.curves.get(*m).map(|c| &c.kind) {
                    Some(CurveKind::Line { a, b }) => Some((*a, *b)),
                    _ => None,
                }
            }
            (ConstraintKind::HorizontalPoints { p, q }, true) | (ConstraintKind::VerticalPoints { p, q }, false) => Some((*p, *q)),
            _ => None,
        };
        ends.is_some_and(|(p, q)| {
            let (rp, rq) = (root(&parent, p), root(&parent, q));
            rp != rq && ((rp, rq) == (ra, rb) || (rp, rq) == (rb, ra))
        })
    })
}

/// Drawing adds horizontal/vertical only where it is not already implied: a rectangle drawn
/// onto another's side shares that side's direction through the two coincident corners, so a
/// second "vertical" there would only be redundant. Drops such constraints among `cons` (the
/// drawing's own) from the sketch and the list.
fn drop_implied_axes(sk: &mut Sketch, cons: &mut Vec<String>) {
    let mut dropped = Vec::new();
    for id in cons.iter() {
        let axis = sk.constraints.iter().find(|c| &c.id == id).and_then(|c| match c.kind {
            ConstraintKind::Horizontal { l } => Some((l, true)),
            ConstraintKind::Vertical { l } => Some((l, false)),
            _ => None,
        });
        if let Some((l, horizontal)) = axis
            && axis_implied(sk, l, horizontal, id)
        {
            sk.constraints.retain(|c| &c.id != id);
            dropped.push(id.clone());
        }
    }
    cons.retain(|id| !dropped.contains(id));
}

/// Which side and corner of an axis-aligned rectangle is which, from its lines and corner
/// points as `polygon_lines` made them (counter-clockwise from the bottom-left corner), so a
/// caller can dimension it without inspecting the sketch first.
fn rect_names(sk: &Sketch, l: &[usize], pts: &[usize]) -> Value {
    let curve = |i: usize| l.get(i).and_then(|c| sk.curves.get(*c)).map(|c| c.id.clone());
    let point = |i: usize| pts.get(i).and_then(|q| sk.points.get(*q)).map(|q| q.id.clone());
    json!({
        "sides": {"bottom": curve(0), "right": curve(1), "top": curve(2), "left": curve(3)},
        "corners": {"bottom_left": point(0), "bottom_right": point(1), "top_right": point(2), "top_left": point(3)},
    })
}

/// `result` plus the rectangle's named sides and corners.
fn rect_result(sk_out: (Vec<String>, Vec<String>), names: Value, info: Value) -> Value {
    let mut out = result(sk_out, info);
    if let (Some(o), Value::Object(n)) = (out.as_object_mut(), names) {
        o.extend(n);
    }
    out
}

fn rect_two_point(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.rectangle.two_point";
    let (a, b) = (req_vec2(cmd, p, "p0")?, req_vec2(cmd, p, "p1")?);
    if (a.x - b.x).abs() < 1e-9 || (a.y - b.y).abs() < 1e-9 {
        return Err(bad(cmd, "the corners must differ in x and y"));
    }
    let (lo, hi) = (Vec2::new(a.x.min(b.x), a.y.min(b.y)), Vec2::new(a.x.max(b.x), a.y.max(b.y)));
    let ((out, names), info) = edit(s, p, cmd, false, |sk, _| {
        let first = sk.points.len();
        let (l, pts) = polygon_lines(sk, &[lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)])?;
        let mut cons = rect_constraints(sk, &l)?;
        mark_construction(sk, &l, p);
        cons.extend(connect_points(sk, first, p)?);
        drop_implied_axes(sk, &mut cons);
        Ok(((ids_of(sk, &l), cons), rect_names(sk, &l, &pts)))
    })?;
    Ok(rect_result(out, names, info))
}

/// A 2-point rectangle with its corners filleted, as one undo step: the rectangle and the
/// fillet run in a scratch session that replaces the design only when both succeed.
fn rect_rounded(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.rectangle.rounded";
    let (a, b) = (req_vec2(cmd, p, "p0")?, req_vec2(cmd, p, "p1")?);
    let r_expr = expr(p, "radius").ok_or_else(|| bad(cmd, "`radius` must be a number or an expression"))?;
    let r = s.doc.eval(&r_expr, Kind::Length).map_err(|e| bad(cmd, format!("radius: {e}")))?;
    if !(r > 1e-9 && r < 1e8) {
        return Err(bad(cmd, "radius must be positive"));
    }
    let half = 0.5 * (a.x - b.x).abs().min((a.y - b.y).abs());
    if half > 1e-9 && r >= half - 1e-9 {
        return Err(bad(cmd, format!("the radius must be less than half the shorter side ({half:.3} mm)")));
    }
    let mut rect = p.clone();
    if let Some(o) = rect.as_object_mut() {
        o.remove("radius");
    }
    let mut scratch = s.scratch();
    let mut out = scratch.execute("sketch.rectangle.two_point", &rect)?;
    let corners: Vec<Value> = ["bottom_left", "bottom_right", "top_right", "top_left"]
        .iter()
        .filter_map(|k| out["corners"].get(*k).filter(|v| v.is_string()).cloned())
        .collect();
    if corners.len() != 4 {
        return Err(bad(cmd, "the rectangle's corners are missing"));
    }
    let mut fillet = json!({"points": corners, "radius": r_expr});
    if let Some(sketch) = p.get("sketch") {
        fillet["sketch"] = sketch.clone();
    }
    let rounded = scratch.execute("sketch.fillet", &fillet)?;
    // Session::execute records the wrapper's before snapshot. Nested history must not leak.
    scratch.undo.clear();
    s.accept_computed(scratch);
    let arcs = rounded.get("curves").cloned().unwrap_or_else(|| json!([]));
    if let Some(o) = out.as_object_mut() {
        // The sharp corners are gone (the lines end at the arcs now).
        o.remove("corners");
        if let (Some(Value::Array(curves)), Value::Array(extra)) = (o.get_mut("curves"), &arcs) {
            curves.extend(extra.iter().cloned());
        }
        if let (Some(Value::Array(cons)), Some(Value::Array(extra))) = (o.get_mut("constraints"), rounded.get("constraints")) {
            cons.extend(extra.iter().cloned());
        }
        o.insert("arcs".into(), arcs);
        o.insert("param".into(), rounded.get("param").cloned().unwrap_or(Value::Null));
        if let Some(info) = rounded.get("sketch") {
            o.insert("sketch".into(), info.clone());
        }
    }
    Ok(out)
}

fn rect_center(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.rectangle.center";
    let (c, k) = (req_vec2(cmd, p, "center")?, req_vec2(cmd, p, "corner")?);
    let h = Vec2::new((k.x - c.x).abs(), (k.y - c.y).abs());
    if h.x < 1e-9 || h.y < 1e-9 {
        return Err(bad(cmd, "the corner must differ from the centre in x and y"));
    }
    let ((out, names), info) = edit(s, p, cmd, false, |sk, _| {
        let first = sk.points.len();
        let (l, pts) = polygon_lines(sk, &[c - h, Vec2::new(c.x + h.x, c.y - h.y), c + h, Vec2::new(c.x - h.x, c.y + h.y)])?;
        let mut cons = rect_constraints(sk, &l)?;
        // Both diagonals (construction), the centre at their crossing. Only the first carries
        // the midpoint constraint: diagonals of a parallelogram bisect each other, so a second
        // one would be redundant (over-constrained).
        if let (Some(p0), Some(p1), Some(p2), Some(p3)) = (pts.first(), pts.get(1), pts.get(2), pts.get(3)) {
            let diag = sk.add_line_pts(*p0, *p2, None)?;
            let diag2 = sk.add_line_pts(*p1, *p3, None)?;
            for d in [diag, diag2] {
                if let Some(cu) = sk.curves.get_mut(d) {
                    cu.construction = true;
                }
            }
            let cp = sk.add_point(c, None)?;
            cons.push(add_c(sk, ConstraintKind::Midpoint { p: cp, l: diag })?);
        }
        mark_construction(sk, &l, p);
        cons.extend(connect_points(sk, first, p)?);
        drop_implied_axes(sk, &mut cons);
        Ok(((ids_of(sk, &l), cons), rect_names(sk, &l, &pts)))
    })?;
    Ok(rect_result(out, names, info))
}

fn rect_three_point(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.rectangle.three_point";
    let (a, b, c) = (req_vec2(cmd, p, "p0")?, req_vec2(cmd, p, "p1")?, req_vec2(cmd, p, "p2")?);
    let d = (b - a).normalized().ok_or_else(|| bad(cmd, "p0 and p1 must differ"))?;
    let n = d.perp();
    let w = (c - a).dot(n);
    if w.abs() < 1e-9 {
        return Err(bad(cmd, "p2 must not be on the first edge"));
    }
    let (out, info) = draw(s, p, cmd, |sk, _| {
        let (l, _) = polygon_lines(sk, &[a, b, b + n * w, a + n * w])?;
        let mut cons = Vec::new();
        if let [l0, l1, l2, l3] = l[..] {
            cons.push(add_c(sk, ConstraintKind::Perpendicular { a: l0, b: l1 })?);
            cons.push(add_c(sk, ConstraintKind::Parallel { a: l0, b: l2 })?);
            cons.push(add_c(sk, ConstraintKind::Parallel { a: l1, b: l3 })?);
        }
        mark_construction(sk, &l, p);
        Ok((ids_of(sk, &l), cons))
    })?;
    Ok(result(out, info))
}

pub(super) fn radius_arg(p: &Value, cmd: &str) -> Result<f64> {
    let r = match (num(p, "radius"), num(p, "diameter")) {
        (Some(r), _) => r,
        (None, Some(d)) => d / 2.0,
        _ => return Err(bad(cmd, "needs `radius` or `diameter`")),
    };
    if r > 1e-9 { Ok(r) } else { Err(bad(cmd, "radius must be positive")) }
}

fn circle_center(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.circle.center";
    let p = &super::sketch_project::snap_points(s, p, cmd)?;
    let r = radius_arg(p, cmd)?;
    let id = str_(p, "id").map(str::to_string);
    let (out, info) = draw(s, p, cmd, |sk, _| {
        let c = req_parg(sk, p, "center", cmd)?;
        let ci = sk.add_circle(c.pos(), r, c.idx(), id.as_deref())?;
        mark_construction(sk, &[ci], p);
        Ok((ids_of(sk, &[ci]), Vec::new()))
    })?;
    Ok(result(out, info))
}

fn circle_two(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.circle.two_point";
    let (a, b) = (req_vec2(cmd, p, "p0")?, req_vec2(cmd, p, "p1")?);
    let r = a.dist(b) / 2.0;
    if r < 1e-9 {
        return Err(bad(cmd, "the points must differ"));
    }
    let (out, info) = draw(s, p, cmd, |sk, _| {
        let ci = sk.add_circle((a + b) * 0.5, r, None, None)?;
        Ok((ids_of(sk, &[ci]), Vec::new()))
    })?;
    Ok(result(out, info))
}

/// Circle through three points.
pub(super) fn circumcircle(a: Vec2, b: Vec2, c: Vec2) -> Option<(Vec2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a2, b2, c2) = (a.len2(), b.len2(), c.len2());
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let o = Vec2::new(ux, uy);
    let r = o.dist(a);
    (o.is_finite() && r.is_finite() && r < 1e8).then_some((o, r))
}

fn circle_three(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.circle.three_point";
    let (a, b, c) = (req_vec2(cmd, p, "p0")?, req_vec2(cmd, p, "p1")?, req_vec2(cmd, p, "p2")?);
    let (o, r) = circumcircle(a, b, c).ok_or_else(|| bad(cmd, "the points are collinear"))?;
    let (out, info) = draw(s, p, cmd, |sk, _| {
        let ci = sk.add_circle(o, r, None, None)?;
        Ok((ids_of(sk, &[ci]), Vec::new()))
    })?;
    Ok(result(out, info))
}

fn arc_three(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.arc.three_point";
    let p = &super::sketch_project::snap_points(s, p, cmd)?;
    let id = str_(p, "id").map(str::to_string);
    let (out, info) = draw(s, p, cmd, |sk, _| {
        let (a, b) = (req_parg(sk, p, "start", cmd)?, req_parg(sk, p, "end", cmd)?);
        let t = req_vec2(cmd, p, "through")?;
        let (o, _) = circumcircle(a.pos(), t, b.pos()).ok_or_else(|| bad(cmd, "the points are collinear"))?;
        // Counter-clockwise from start to end must pass the through point; otherwise swap.
        let ang = |q: Vec2| (q - o).angle();
        let ccw_span = |from: f64, to: f64| (to - from).rem_euclid(std::f64::consts::TAU);
        let ccw = ccw_span(ang(a.pos()), ang(t)) < ccw_span(ang(a.pos()), ang(b.pos()));
        let (s0, e0) = if ccw { (a, b) } else { (b, a) };
        let ci = sk.add_arc(o, s0.pos(), e0.pos(), [None, s0.idx(), e0.idx()], id.as_deref())?;
        if let Some(c) = sk.curves.get_mut(ci) {
            c.reversed = !ccw;
        }
        Ok((ids_of(sk, &[ci]), Vec::new()))
    })?;
    Ok(result(out, info))
}

fn arc_center(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.arc.center_point";
    let p = &super::sketch_project::snap_points(s, p, cmd)?;
    let id = str_(p, "id").map(str::to_string);
    let sweep = num(p, "sweep");
    let (out, info) = draw(s, p, cmd, |sk, _| {
        let c = req_parg(sk, p, "center", cmd)?;
        let a = req_parg(sk, p, "start", cmd)?;
        if let Some(sw) = sweep {
            // Start point and signed sweep (degrees); clockwise arcs keep `<id>.start` at the start.
            if !(sw.abs() > 1e-9 && sw.abs() < 360.0) {
                return Err(bad(cmd, "`sweep` must be between -360 and 360 degrees (not 0)"));
            }
            let end = c.pos() + Vec2::from_angle((a.pos() - c.pos()).angle() + sw.to_radians()) * a.pos().dist(c.pos());
            let cid = match &id {
                Some(i) => i.clone(),
                None => sk.fresh("a")?,
            };
            let ai = match a.idx() {
                Some(i) => i,
                None => sk.add_point(a.pos(), Some(&format!("{cid}.start")))?,
            };
            let bi = sk.add_point(end, Some(&format!("{cid}.end")))?;
            let ci = if sw > 0.0 {
                sk.add_arc(c.pos(), a.pos(), end, [c.idx(), Some(ai), Some(bi)], Some(&cid))?
            } else {
                let ci = sk.add_arc(c.pos(), end, a.pos(), [c.idx(), Some(bi), Some(ai)], Some(&cid))?;
                if let Some(cu) = sk.curves.get_mut(ci) {
                    cu.reversed = true;
                }
                ci
            };
            return Ok((ids_of(sk, &[ci]), Vec::new()));
        }
        let b = req_parg(sk, p, "end", cmd)?;
        let ci = sk.add_arc(c.pos(), a.pos(), b.pos(), [c.idx(), a.idx(), b.idx()], id.as_deref())?;
        Ok((ids_of(sk, &[ci]), Vec::new()))
    })?;
    Ok(result(out, info))
}

fn polygon(s: &mut Session, p: &Value, cmd: &str, center: Vec2, r_vertex: f64, start: f64, n: usize, inscribed: bool) -> Result<Value> {
    let mut support_circle = String::new();
    let (out, info) = draw(s, p, cmd, |sk, _| {
        let corners: Vec<Vec2> = (0..n).map(|i| center + Vec2::from_angle(start + std::f64::consts::TAU * i as f64 / n as f64) * r_vertex).collect();
        let (l, pts) = polygon_lines(sk, &corners)?;
        let mut cons = Vec::new();
        for w in l.windows(2) {
            if let [a, b] = w {
                cons.push(add_c(sk, ConstraintKind::Equal { a: *a, b: *b })?);
            }
        }
        // Construction circle tying the polygon to its centre.
        let rc = if inscribed { r_vertex } else { r_vertex * (std::f64::consts::PI / n as f64).cos() };
        let circ = sk.add_circle(center, rc, None, None)?;
        support_circle = sk.curves.get(circ).map(|c| c.id.clone()).unwrap_or_default();
        if let Some(cu) = sk.curves.get_mut(circ) {
            cu.construction = true;
        }
        if inscribed {
            for pt in &pts {
                cons.push(add_c(sk, ConstraintKind::PointOnCurve { p: *pt, c: circ })?);
            }
        } else {
            for li in &l {
                cons.push(add_c(sk, ConstraintKind::Tangent { a: *li, b: circ })?);
            }
        }
        Ok((ids_of(sk, &l), cons))
    })?;
    let mut value = result(out, info);
    value["support_circle"] = json!(support_circle);
    Ok(value)
}

fn sides(p: &Value, cmd: &str) -> Result<usize> {
    let n = num(p, "sides").unwrap_or(6.0);
    if !(3.0..=200.0).contains(&n) || n.fract() != 0.0 {
        return Err(bad(cmd, "sides must be an integer from 3 to 200"));
    }
    Ok(n as usize)
}

fn polygon_inscribed(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.polygon.inscribed";
    let c = req_vec2(cmd, p, "center")?;
    let r = radius_arg(p, cmd)?;
    let n = sides(p, cmd)?;
    let a = num(p, "angle").unwrap_or(90.0).to_radians();
    polygon(s, p, cmd, c, r, a, n, true)
}

fn polygon_circumscribed(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.polygon.circumscribed";
    let c = req_vec2(cmd, p, "center")?;
    let r = radius_arg(p, cmd)?;
    let n = sides(p, cmd)?;
    let a = num(p, "angle").unwrap_or(90.0).to_radians();
    polygon(s, p, cmd, c, r / (std::f64::consts::PI / n as f64).cos(), a + std::f64::consts::PI / n as f64, n, false)
}

fn polygon_edge(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.polygon.edge";
    let (a, b) = (req_vec2(cmd, p, "p0")?, req_vec2(cmd, p, "p1")?);
    let n = sides(p, cmd)?;
    let e = a.dist(b);
    if e < 1e-9 {
        return Err(bad(cmd, "the points must differ"));
    }
    let rv = e / (2.0 * (std::f64::consts::PI / n as f64).sin());
    let apothem = rv * (std::f64::consts::PI / n as f64).cos();
    let mid = (a + b) * 0.5;
    let dir = (b - a).normalized().unwrap_or(Vec2::X);
    let center = mid + dir.perp() * apothem;
    let start = (a - center).angle();
    polygon(s, p, cmd, center, rv, start, n, true)
}

pub(super) fn slot(s: &mut Session, p: &Value, cmd: &str, c0: Vec2, c1: Vec2, w: f64) -> Result<Value> {
    let h = w / 2.0;
    let d = (c1 - c0).normalized().ok_or_else(|| bad(cmd, "the slot ends must differ"))?;
    let n = d.perp();
    let (out, info) = draw(s, p, cmd, |sk, _| {
        let pts: Vec<usize> =
            [c0 - n * h, c1 - n * h, c1 + n * h, c0 + n * h].iter().map(|q| sk.add_point(*q, None)).collect::<std::result::Result<_, _>>()?;
        let (p0, p1, p2, p3) = (pts[0], pts[1], pts[2], pts[3]);
        let l1 = sk.add_line_pts(p0, p1, None)?;
        let a1 = sk.add_arc(c1, c1 - n * h, c1 + n * h, [None, Some(p1), Some(p2)], None)?;
        let l2 = sk.add_line_pts(p2, p3, None)?;
        let a0 = sk.add_arc(c0, c0 + n * h, c0 - n * h, [None, Some(p3), Some(p0)], None)?;
        let mut cons = Vec::new();
        for (l, a) in [(l1, a1), (l2, a1), (l1, a0), (l2, a0)] {
            cons.push(add_c(sk, ConstraintKind::Tangent { a: l, b: a })?);
        }
        cons.push(add_c(sk, ConstraintKind::Equal { a: a0, b: a1 })?);
        Ok((ids_of(sk, &[l1, a1, l2, a0]), cons))
    })?;
    Ok(result(out, info))
}

fn slot_c2c(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.slot.center_to_center";
    let (a, b) = (req_vec2(cmd, p, "p0")?, req_vec2(cmd, p, "p1")?);
    let w = num(p, "width").filter(|w| *w > 1e-9).ok_or_else(|| bad(cmd, "`width` must be positive"))?;
    slot(s, p, cmd, a, b, w)
}

fn slot_overall(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.slot.overall";
    let (a, b) = (req_vec2(cmd, p, "p0")?, req_vec2(cmd, p, "p1")?);
    let w = num(p, "width").filter(|w| *w > 1e-9).ok_or_else(|| bad(cmd, "`width` must be positive"))?;
    let d = (b - a).normalized().ok_or_else(|| bad(cmd, "the slot ends must differ"))?;
    if a.dist(b) <= w {
        return Err(bad(cmd, "the slot must be longer than it is wide"));
    }
    slot(s, p, cmd, a + d * (w / 2.0), b - d * (w / 2.0), w)
}

fn draw_point(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.point";
    let at = req_vec2(cmd, p, "point")?;
    let id = str_(p, "id").map(str::to_string);
    let ((pid, cons), info) = edit(s, p, cmd, false, |sk, _| {
        let i = sk.add_point(at, id.as_deref())?;
        let cons = connect_points(sk, i, p)?;
        Ok((sk.points.get(i).map(|q| q.id.clone()).unwrap_or_default(), cons))
    })?;
    Ok(json!({"point": pid, "constraints": cons, "sketch": info}))
}

// ---------------------------------------------------------------------------------------------
// Constraints

fn constrain(s: &mut Session, p: &Value, cmd: &str, f: impl FnOnce(&mut Sketch) -> Result<ConstraintKind>) -> Result<Value> {
    let mut before = None;
    let mut residual = None;
    let (id, info) = edit(s, p, cmd, true, |sk, _| {
        let k = f(sk)?;
        let rep = solve(&mut sk.clone());
        before = Some(rep.dof);
        let order = entity_order(sk, p);
        let id = add_c(sk, k)?;
        residual = residual_of(sk, &id);
        // A constraint that already holds needs nothing moved (even when both entities are
        // fully determined: it is then redundant, see `redundancy`).
        let holds = residual.is_some_and(|r| r <= solidcraft_sketch::CONSISTENT);
        if let Some((first, second)) = order.filter(|_| !holds) {
            apply_in_order(sk, &rep, &[first], second, cmd)?;
        }
        Ok(id)
    })?;
    let by = redundancy(s, p, cmd, before, &info, residual, &id)?;
    constraint_result(p, cmd, id, info, by, Value::Null)
}

/// A sketch entity a constraint is applied to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Ent {
    Point(usize),
    Curve(usize),
}

fn ent(sk: &Sketch, r: &str) -> Option<Ent> {
    sk.resolve_point(r).map(Ent::Point).or_else(|| sk.curve_index(r).or_else(|| sk.sketch_axis_curve(r)).map(Ent::Curve))
}

/// The entities of a two-entity constraint in the order they were picked (`a` then `b`, or
/// `point` then `line`).
fn entity_order(sk: &Sketch, p: &Value) -> Option<(Ent, Ent)> {
    let r = |k: &str| p.get(k).and_then(Value::as_str).and_then(|r| ent(sk, r));
    Some((r("a").or_else(|| r("point"))?, r("b").or_else(|| r("line"))?))
}

/// Held in place: fixed or projected, or fully determined by the constraints already there.
fn ent_locked(sk: &Sketch, rep: &SolveReport, e: Ent) -> bool {
    match e {
        Ent::Point(i) => sk.point_locked(i) || rep.point_determined.get(i).copied().unwrap_or(false),
        Ent::Curve(c) => sk.curve_locked(c) || rep.curve_determined.get(c).copied().unwrap_or(false),
    }
}

/// Hold entities: their points, and with `radius` their radii too.
fn hold_of(sk: &Sketch, ents: &[Ent], radius: bool) -> Hold {
    let mut h = Hold::default();
    for e in ents {
        match *e {
            Ent::Point(i) => h.points.push(i),
            Ent::Curve(c) => {
                h.points.extend(sk.curves.get(c).map(|c| c.kind.point_ids()).unwrap_or_default());
                h.radii.push(c);
            }
        }
    }
    if !radius {
        h.radii.clear();
    }
    h
}

/// Did a solve shrink a line or an arc to nothing (a degenerate way to satisfy tangency)?
fn collapsed(before: &Sketch, after: &Sketch) -> bool {
    let size = |sk: &Sketch, i: usize| match sk.curves.get(i).map(|c| &c.kind) {
        Some(CurveKind::Line { .. }) => line_pts(sk, i).map(|(a, b)| a.dist(b)),
        Some(CurveKind::Arc { .. } | CurveKind::Circle { .. }) => sk.radius(i),
        _ => None,
    };
    (0..before.curves.len()).any(|i| match (size(before, i), size(after, i)) {
        (Some(b), Some(a)) => b > 1e-6 && a < b * 1e-3,
        _ => false,
    })
}

/// Keep an entity's size while it moves: a line's length (circles keep their radius by holding
/// it).
fn keep_size(sk: &Sketch, e: Ent) -> Vec<ConstraintKind> {
    match (
        e,
        line_pts(
            sk,
            match e {
                Ent::Curve(c) => c,
                Ent::Point(_) => usize::MAX,
            },
        ),
    ) {
        (Ent::Curve(l), Some((a, b))) => vec![ConstraintKind::Length { l, value: a.dist(b) }],
        _ => Vec::new(),
    }
}

/// Satisfy a just-added constraint the way Fusion applies it: the first-picked entities stay
/// put and only the second moves (first keeping its size, so it turns and slides, then also
/// resizing).
/// If the second can't move (fixed, projected or fully determined), the first moves instead;
/// if neither can, the constraint conflicts and nothing changes. Everything else moves as
/// little as it can. Later solves are ordinary ones (nothing is held).
pub(super) fn apply_in_order(sk: &mut Sketch, rep: &SolveReport, first: &[Ent], second: Ent, cmd: &str) -> Result<()> {
    let first_locked = first.iter().any(|e| ent_locked(sk, rep, *e));
    let second_locked = ent_locked(sk, rep, second);
    // (held geometry, temporary size constraints) to try in turn.
    let mut tries: Vec<(Hold, Vec<ConstraintKind>)> = Vec::new();
    let mut ordered = |stay: &[Ent], mover: Ent| {
        let mut h = hold_of(sk, stay, true);
        h.radii.extend(hold_of(sk, &[mover], true).radii);
        tries.push((h.clone(), keep_size(sk, mover)));
        tries.push((hold_of(sk, stay, true), Vec::new()));
        // An end point the two share may slide (a line tangent to an arc it ends on, whose
        // centre is pinned, has to turn about its other end).
        let shared = hold_of(sk, &[mover], false).points;
        if h.points.iter().any(|p| shared.contains(p)) {
            h.points.retain(|p| !shared.contains(p));
            tries.push((h.clone(), keep_size(sk, mover)));
            tries.push((h, Vec::new()));
        }
    };
    if !second_locked {
        ordered(first, second);
    }
    // Each first entity that can move may (a free symmetry line between two fixed lines, #1796),
    // even when another of them can't.
    for f in first.iter().filter(|f| !ent_locked(sk, rep, **f)) {
        ordered(&[second], *f);
    }
    const TEMP: &str = "__keep_size";
    for (h, extra) in tries {
        let mut trial = sk.clone();
        trial.constraints.extend(extra.into_iter().map(|kind| solidcraft_sketch::Constraint {
            id: TEMP.into(),
            kind,
            param: None,
            driven: false,
            text: None,
        }));
        let ok = solve_with(&mut trial, &h, None, Analysis::Full, None).ok();
        trial.constraints.retain(|c| c.id != TEMP);
        if ok && !collapsed(sk, &trial) {
            *sk = trial;
            return Ok(());
        }
    }
    // Both could move but holding either one leads nowhere: solve as usual.
    if !first_locked && !second_locked {
        let mut trial = sk.clone();
        if solve(&mut trial).ok() {
            *sk = trial;
            return Ok(());
        }
    }
    Err(bad(cmd, "that conflicts with the sketch: neither entity can move to satisfy it (they are fixed or fully constrained)"))
}

/// Degrees of freedom of a sketch as it is.
pub(super) fn dof_now(sk: &Sketch) -> usize {
    solve(&mut sk.clone()).dof
}

/// A constraint just added that removes no degree of freedom (`before` → the edit's `info`) is
/// redundant or a conflict. Redundant when it held where the sketch was before it was added
/// (`residual`, see [`solidcraft_sketch::constraint_residual`]): what it asks for is already
/// implied by other constraints, and the solver satisfies it with them. Returns the
/// constraints that determine it (as "id (Name)"), or `None` when it removes a degree of
/// freedom. A conflict is refused; the caller's edit is rolled back by returning the error.
pub(super) fn redundancy(
    s: &Session,
    p: &Value,
    cmd: &str,
    before: Option<usize>,
    info: &Value,
    residual: Option<f64>,
    cid: &str,
) -> Result<Option<Vec<String>>> {
    let after = info.get("dof").and_then(Value::as_u64).map(|d| d as usize);
    let (Some(b), Some(a)) = (before, after) else { return Ok(None) };
    if a < b {
        return Ok(None);
    }
    if !residual.is_some_and(|r| r <= solidcraft_sketch::CONSISTENT) {
        return Err(bad(cmd, "that would over-constrain the sketch (it conflicts with what the other constraints already determine)"));
    }
    Ok(Some(determined_by(s, p, cmd, cid)))
}

/// The constraints that already determine constraint `cid` of the target sketch, as
/// "id (Name)".
fn determined_by(s: &Session, p: &Value, cmd: &str, cid: &str) -> Vec<String> {
    let Some(sk) = target_sketch(s, p, cmd).ok().and_then(|id| s.doc.sketch(id).ok()) else { return Vec::new() };
    let Some(ci) = sk.constraints.iter().position(|c| c.id == cid) else { return Vec::new() };
    solidcraft_sketch::determined_by(sk, ci)
        .into_iter()
        .filter_map(|k| sk.constraints.get(k))
        .map(|c| format!("{} ({})", c.id, c.kind.name()))
        .collect()
}

/// "k3 (Vertical), k9 (Vertical)", or "other constraints" when they are not known.
pub(super) fn listed(by: &[String]) -> String {
    if by.is_empty() { "other constraints".to_string() } else { by.join(", ") }
}

/// A redundant constraint is kept (and shown as redundant) unless the caller asked for
/// `if_redundant: "refuse"` (drawing-time inferences, which only add what is not already there).
pub(super) fn keep_redundant(p: &Value, cmd: &str, by: &[String]) -> Result<()> {
    if str_(p, "if_redundant").is_some_and(|m| m.eq_ignore_ascii_case("refuse")) {
        return Err(bad(cmd, format!("that would over-constrain the sketch (it is already determined by {})", listed(by))));
    }
    Ok(())
}

/// The result of a constraint command: the constraint, and whether it is redundant (and what
/// determines it).
pub(super) fn constraint_result(p: &Value, cmd: &str, id: String, info: Value, by: Option<Vec<String>>, extra: Value) -> Result<Value> {
    let mut out = json!({"constraint": id, "sketch": info});
    if let Some(by) = by {
        keep_redundant(p, cmd, &by)?;
        out["redundant"] = json!(true);
        out["determined_by"] = json!(by);
    }
    if let (Some(o), Value::Object(e)) = (out.as_object_mut(), extra) {
        o.extend(e);
    }
    Ok(out)
}

/// The residual of the constraint `id` where the sketch is now (see
/// [`solidcraft_sketch::constraint_residual`]).
pub(super) fn residual_of(sk: &Sketch, id: &str) -> Option<f64> {
    sk.constraints.iter().position(|c| c.id == id).and_then(|ci| solidcraft_sketch::constraint_residual(sk, ci))
}

fn c_horizontal_vertical(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constraint.horizontal_vertical";
    let mode = str_(p, "mode").map(str::to_ascii_lowercase);
    constrain(s, p, cmd, |sk| {
        if let Some(pts) = p.get("points").and_then(Value::as_array) {
            let a = point_ref(sk, pts.first(), cmd, "points[0]")?;
            let b = point_ref(sk, pts.get(1), cmd, "points[1]")?;
            let d = sk.point(b).unwrap_or_default() - sk.point(a).unwrap_or_default();
            let horiz = match mode.as_deref() {
                Some("horizontal") => true,
                Some("vertical") => false,
                _ => d.x.abs() >= d.y.abs(),
            };
            return Ok(if horiz { ConstraintKind::HorizontalPoints { p: a, q: b } } else { ConstraintKind::VerticalPoints { p: a, q: b } });
        }
        let l = curve_ref(sk, p.get("line"), cmd, "line")?;
        let (a, b) = match sk.curves.get(l).map(|c| &c.kind) {
            Some(CurveKind::Line { a, b }) => (*a, *b),
            _ => return Err(bad(cmd, "`line` must be a line")),
        };
        let d = sk.point(b).unwrap_or_default() - sk.point(a).unwrap_or_default();
        let horiz = match mode.as_deref() {
            Some("horizontal") => true,
            Some("vertical") => false,
            _ => d.x.abs() >= d.y.abs(),
        };
        Ok(if horiz { ConstraintKind::Horizontal { l } } else { ConstraintKind::Vertical { l } })
    })
}

fn c_coincident(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constraint.coincident";
    // Already tied (e.g. connected when it was drawn there): nothing to add.
    let sk = s.doc.sketch(target_sketch(s, p, cmd)?)?;
    let pt = |k: &str| p.get(k).and_then(Value::as_str).and_then(|r| sk.resolve_point(r));
    let line_points = coincident_line_points(sk, p, cmd)?;
    if let Some((a, b)) = line_points.or_else(|| pt("a").zip(pt("b")))
        && let Some(c) = sk.constraints.iter().find(|c| matches!(c.kind, ConstraintKind::Coincident { p, q } if (p, q) == (a, b) || (p, q) == (b, a)))
    {
        let rep = solve(&mut sk.clone());
        return Ok(json!({"constraint": c.id, "existing": true, "sketch": report_json(&rep)}));
    }
    constrain(s, p, cmd, |sk| {
        if let Some((a, b)) = line_points {
            return Ok(ConstraintKind::Coincident { p: a, q: b });
        }
        let aref = p.get("a").and_then(Value::as_str).unwrap_or_default();
        let bref = p.get("b").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`b` must be a point or curve"))?;
        // A curve (or sketch axis) picked first and a point second: the point goes onto it.
        if sk.resolve_point(aref).is_none()
            && let Some(q) = sk.resolve_point(bref)
            && let Some(c) = curve_or_axis(sk, aref)?
        {
            return Ok(ConstraintKind::PointOnCurve { p: q, c });
        }
        let a = point_ref(sk, p.get("a"), cmd, "a")?;
        if let Some(b) = sk.resolve_point(bref) {
            return Ok(ConstraintKind::Coincident { p: a, q: b });
        }
        let c = curve_or_axis(sk, bref)?.ok_or_else(|| bad(cmd, format!("unknown entity `{bref}`")))?;
        Ok(ConstraintKind::PointOnCurve { p: a, c })
    })
}

/// Four endpoint pairs, ordered independently of pick order when distances tie.
fn coincident_line_points(sk: &Sketch, p: &Value, cmd: &str) -> Result<Option<(usize, usize)>> {
    let curve = |key: &str| p.get(key).and_then(Value::as_str).and_then(|id| sk.curve_index(id));
    let (Some(a), Some(b)) = (curve("a"), curve("b")) else { return Ok(None) };
    if a == b {
        return Err(bad(cmd, "choose two distinct lines"));
    }
    let ends = |i: usize| match sk.curves.get(i).map(|curve| &curve.kind) {
        Some(CurveKind::Line { a, b }) => Some([*a, *b]),
        _ => None,
    };
    let (Some(first), Some(second)) = (ends(a), ends(b)) else {
        return Err(bad(cmd, "two curves must both be straight lines; otherwise choose a point and a curve"));
    };
    let mut best: Option<(f64, usize, usize)> = None;
    for a in first {
        for b in second {
            let (Some(pa), Some(pb)) = (sk.point(a), sk.point(b)) else { return Err(bad(cmd, "line endpoint is missing")) };
            let candidate = (pa.dist(pb), a.min(b), a.max(b));
            if best.is_none_or(|old| candidate < old) {
                best = Some(candidate);
            }
        }
    }
    let (_, a, b) = best.ok_or_else(|| bad(cmd, "line endpoints are missing"))?;
    if a == b {
        return Err(bad(cmd, "the lines already share an endpoint"));
    }
    Ok(Some((a, b)))
}

fn two_curves(sk: &Sketch, p: &Value, cmd: &str) -> Result<(usize, usize)> {
    Ok((curve_ref(sk, p.get("a"), cmd, "a")?, curve_ref(sk, p.get("b"), cmd, "b")?))
}

/// Two curves where either may be a sketch axis (`x` / `y`, see [`line_or_axis_ref`]).
fn two_curves_or_axes(sk: &mut Sketch, p: &Value, cmd: &str) -> Result<(usize, usize)> {
    Ok((line_or_axis_ref(sk, p.get("a"), cmd, "a")?, line_or_axis_ref(sk, p.get("b"), cmd, "b")?))
}

fn c_tangent(s: &mut Session, p: &Value) -> Result<Value> {
    constrain(s, p, "sketch.constraint.tangent", |sk| {
        two_curves_or_axes(sk, p, "sketch.constraint.tangent").map(|(a, b)| ConstraintKind::Tangent { a, b })
    })
}
fn c_equal(s: &mut Session, p: &Value) -> Result<Value> {
    constrain(s, p, "sketch.constraint.equal", |sk| two_curves(sk, p, "sketch.constraint.equal").map(|(a, b)| ConstraintKind::Equal { a, b }))
}
fn c_parallel(s: &mut Session, p: &Value) -> Result<Value> {
    constrain(s, p, "sketch.constraint.parallel", |sk| {
        two_curves_or_axes(sk, p, "sketch.constraint.parallel").map(|(a, b)| ConstraintKind::Parallel { a, b })
    })
}
fn c_perpendicular(s: &mut Session, p: &Value) -> Result<Value> {
    constrain(s, p, "sketch.constraint.perpendicular", |sk| {
        two_curves_or_axes(sk, p, "sketch.constraint.perpendicular").map(|(a, b)| ConstraintKind::Perpendicular { a, b })
    })
}
fn c_concentric(s: &mut Session, p: &Value) -> Result<Value> {
    constrain(s, p, "sketch.constraint.concentric", |sk| {
        two_curves(sk, p, "sketch.constraint.concentric").map(|(a, b)| ConstraintKind::Concentric { a, b })
    })
}
fn c_collinear(s: &mut Session, p: &Value) -> Result<Value> {
    constrain(s, p, "sketch.constraint.collinear", |sk| {
        two_curves_or_axes(sk, p, "sketch.constraint.collinear").map(|(a, b)| ConstraintKind::Collinear { a, b })
    })
}
fn c_midpoint(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constraint.midpoint";
    constrain(s, p, cmd, |sk| {
        Ok(ConstraintKind::Midpoint { p: point_ref(sk, p.get("point"), cmd, "point")?, l: curve_ref(sk, p.get("line"), cmd, "line")? })
    })
}
/// The centre of a symmetry about a point (#1075): `point`, or `line` naming a point rather than
/// a curve or a sketch axis (the origin picked as the third entity).
fn symmetry_center(sk: &Sketch, p: &Value, cmd: &str) -> Result<Option<usize>> {
    if let Some(r) = p.get("point") {
        return point_ref(sk, Some(r), cmd, "point").map(Some);
    }
    let Some(id) = p.get("line").and_then(Value::as_str) else { return Ok(None) };
    if sk.curve_index(id).is_some() || matches!(id, "x" | "y") {
        return Ok(None);
    }
    Ok(sk.resolve_point(id))
}

/// Points `a` and `b` symmetric about point `c`: a construction line from `a` to `b` with `c`
/// at its midpoint.
fn symmetric_about_point(sk: &mut Sketch, a: usize, b: usize, c: usize, cmd: &str) -> Result<String> {
    if a == b || a == c || b == c {
        return Err(bad(cmd, "symmetry about a point needs two other points"));
    }
    let l = sk.add_line_pts(a, b, None)?;
    if let Some(cu) = sk.curves.get_mut(l) {
        cu.construction = true;
    }
    add_c(sk, ConstraintKind::Midpoint { p: c, l })
}

fn c_symmetry(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constraint.symmetry";
    let (ids, info) = edit(s, p, cmd, true, |sk, _| {
        if let Some(c) = symmetry_center(sk, p, cmd)? {
            return symmetry_about_point(sk, p, c, cmd);
        }
        let l = line_or_axis_ref(sk, p.get("line"), cmd, "line")?;
        let ra = p.get("a").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`a` must be a point or curve"))?;
        let rb = p.get("b").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`b` must be a point or curve"))?;
        let rep = solve(&mut sk.clone());
        let order = ent(sk, ra).zip(ent(sk, rb));
        let mut out = Vec::new();
        match (sk.resolve_point(ra), sk.resolve_point(rb)) {
            (Some(a), Some(b)) => out.push(add_c(sk, ConstraintKind::Symmetric { p: a, q: b, l })?),
            _ => {
                // Two curves: circles/arcs mirror their centres and keep equal radii; lines mirror
                // their end points.
                let ca = sk.curve_index(ra).ok_or_else(|| bad(cmd, format!("unknown entity `{ra}`")))?;
                let cb = sk.curve_index(rb).ok_or_else(|| bad(cmd, format!("unknown entity `{rb}`")))?;
                match (sk.curves.get(ca).map(|c| c.kind.clone()), sk.curves.get(cb).map(|c| c.kind.clone())) {
                    (Some(CurveKind::Line { a: a0, b: a1 }), Some(CurveKind::Line { a: b0, b: b1 })) => {
                        out.push(add_c(sk, ConstraintKind::Symmetric { p: a0, q: b0, l })?);
                        out.push(add_c(sk, ConstraintKind::Symmetric { p: a1, q: b1, l })?);
                    }
                    (Some(_), Some(_)) => {
                        let pa = round_center(sk, ca).ok_or_else(|| bad(cmd, "symmetry needs two lines or two circles/arcs"))?;
                        let pb = round_center(sk, cb).ok_or_else(|| bad(cmd, "symmetry needs two lines or two circles/arcs"))?;
                        out.push(add_c(sk, ConstraintKind::Symmetric { p: pa, q: pb, l })?);
                        out.push(add_c(sk, ConstraintKind::Equal { a: ca, b: cb })?);
                    }
                    _ => return Err(bad(cmd, "symmetry needs two points or two curves")),
                }
            }
        }
        // The first entity and the symmetry line stay; the second mirrors onto it.
        if let Some((a, b)) = order {
            apply_in_order(sk, &rep, &[a, Ent::Curve(l)], b, cmd)?;
        }
        Ok(out)
    })?;
    Ok(json!({"constraints": ids, "sketch": info}))
}

/// Symmetry about point `c`: two points, two circles/arcs (their centres, and equal radii) or two
/// lines (each end with the end of the other line nearest its mirror image). The constraints
/// (the construction lines show in the response's `created`).
fn symmetry_about_point(sk: &mut Sketch, p: &Value, c: usize, cmd: &str) -> Result<Vec<String>> {
    let ra = p.get("a").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`a` must be a point or curve"))?;
    let rb = p.get("b").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`b` must be a point or curve"))?;
    let rep = solve(&mut sk.clone());
    let order = ent(sk, ra).zip(ent(sk, rb));
    let pairs: Vec<(usize, usize)> = match (sk.resolve_point(ra), sk.resolve_point(rb)) {
        (Some(a), Some(b)) => vec![(a, b)],
        _ => {
            let ca = sk.curve_index(ra).ok_or_else(|| bad(cmd, format!("unknown entity `{ra}`")))?;
            let cb = sk.curve_index(rb).ok_or_else(|| bad(cmd, format!("unknown entity `{rb}`")))?;
            match (sk.curves.get(ca).map(|x| x.kind.clone()), sk.curves.get(cb).map(|x| x.kind.clone())) {
                (Some(CurveKind::Line { a: a0, b: a1 }), Some(CurveKind::Line { a: b0, b: b1 })) => {
                    let pc = sk.point(c).unwrap_or_default();
                    let mirror = |i: usize| pc * 2.0 - sk.point(i).unwrap_or_default();
                    let near = |i: usize, j: usize| mirror(i).dist(sk.point(j).unwrap_or_default());
                    if near(a0, b0) + near(a1, b1) <= near(a0, b1) + near(a1, b0) { vec![(a0, b0), (a1, b1)] } else { vec![(a0, b1), (a1, b0)] }
                }
                (Some(_), Some(_)) => {
                    let pa = round_center(sk, ca).ok_or_else(|| bad(cmd, "symmetry needs two lines or two circles/arcs"))?;
                    let pb = round_center(sk, cb).ok_or_else(|| bad(cmd, "symmetry needs two lines or two circles/arcs"))?;
                    let mut out = symmetry_pairs(sk, &[(pa, pb)], c, cmd)?;
                    out.push(add_c(sk, ConstraintKind::Equal { a: ca, b: cb })?);
                    if let Some((a, b)) = order {
                        apply_in_order(sk, &rep, &[a, Ent::Point(c)], b, cmd)?;
                    }
                    return Ok(out);
                }
                _ => return Err(bad(cmd, "symmetry needs two points or two curves")),
            }
        }
    };
    let out = symmetry_pairs(sk, &pairs, c, cmd)?;
    // The first entity and the centre stay; the second mirrors onto it.
    if let Some((a, b)) = order {
        apply_in_order(sk, &rep, &[a, Ent::Point(c)], b, cmd)?;
    }
    Ok(out)
}

fn symmetry_pairs(sk: &mut Sketch, pairs: &[(usize, usize)], c: usize, cmd: &str) -> Result<Vec<String>> {
    pairs.iter().map(|(a, b)| symmetric_about_point(sk, *a, *b, c, cmd)).collect()
}

fn round_center(sk: &Sketch, c: usize) -> Option<usize> {
    match sk.curves.get(c)?.kind {
        CurveKind::Circle { c, .. } | CurveKind::Arc { c, .. } => Some(c),
        _ => None,
    }
}

fn c_fix(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constraint.fix";
    let want = bool_(p, "fixed");
    let mut refs = string_list(p, "entities");
    if let Some(e) = str_(p, "entity") {
        refs.insert(0, e.to_string());
    }
    if refs.is_empty() {
        return Err(bad(cmd, "`entity` or `entities` must name sketch points or curves"));
    }
    let (n, info) = edit(s, p, cmd, false, |sk, _| {
        // Points and curves; a point is fixed by a Fix constraint, a curve by its flag (which
        // also holds its radius). Projected geometry is always fixed.
        let mut pts = Vec::new();
        let mut curves = Vec::new();
        for r in &refs {
            if let Some(i) = sk.resolve_point(r) {
                pts.push(i);
            } else {
                curves.push(sk.curve_index(r).ok_or_else(|| bad(cmd, format!("unknown entity `{r}`")))?);
            }
        }
        let all_fixed = pts.iter().all(|q| *q == 0 || sk.point_locked(*q)) && curves.iter().all(|c| sk.curve_locked(*c));
        let fix = want.unwrap_or(!all_fixed);
        let mut n = 0;
        for q in pts {
            let own = sk.points.get(q).is_some_and(|x| x.fixed || x.link.is_some());
            if q == 0 || own {
                continue;
            }
            let has = sk.constraints.iter().any(|c| c.kind == ConstraintKind::Fix { p: q });
            if fix && !has {
                add_c(sk, ConstraintKind::Fix { p: q })?;
                n += 1;
            } else if !fix && has {
                sk.constraints.retain(|c| c.kind != ConstraintKind::Fix { p: q });
                n += 1;
            }
        }
        for c in curves {
            let Some(cu) = sk.curves.get_mut(c) else { continue };
            if cu.link.is_some() {
                continue;
            }
            cu.fixed = fix;
            n += 1;
            if !fix {
                let ids = cu.kind.point_ids();
                sk.constraints.retain(|k| !matches!(k.kind, ConstraintKind::Fix { p } if ids.contains(&p)));
            }
        }
        Ok(json!({"fixed": fix, "changed": n}))
    })?;
    Ok(json!({"result": n, "sketch": info}))
}

// ---------------------------------------------------------------------------------------------
// Dimensions

pub(super) fn line_pts(sk: &Sketch, l: usize) -> Option<(Vec2, Vec2)> {
    match sk.curves.get(l)?.kind {
        CurveKind::Line { a, b } => Some((sk.point(a)?, sk.point(b)?)),
        _ => None,
    }
}

fn dimension(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.dimension";
    let ents = string_list(p, "entities");
    if ents.is_empty() || ents.len() > 2 {
        return Err(bad(cmd, "`entities` must list one or two sketch entities"));
    }
    let ty = str_(p, "type").unwrap_or("auto").to_ascii_lowercase();
    let ty = match (ty.as_str(), str_(p, "orientation").map(str::to_ascii_lowercase).as_deref()) {
        ("distance" | "auto", Some("horizontal")) => "horizontal".to_string(),
        ("distance" | "auto", Some("vertical")) => "vertical".to_string(),
        ("offset", _) => "distance".to_string(),
        _ => ty,
    };
    let value = expr(p, "value");
    let text_at = p.get("text_at").and_then(vec2);
    let driven = bool_(p, "driven").unwrap_or(false);
    let scale_sketch = bool_(p, "scale_sketch").unwrap_or(false);
    let mut scaled = None;
    let mut before = None;
    let (mut residual, mut new_id) = (None, String::new());
    let start = s.doc.clone();
    let ((param, kind_name, current), info) = edit(s, p, cmd, true, |sk, doc| {
        before = Some(dof_now(sk));
        let ent = |r: &str| -> Result<(Option<usize>, Option<usize>)> {
            if let Some(c) = sk.curve_index(r) {
                return Ok((None, Some(c)));
            }
            if let Some(q) = sk.resolve_point(r) {
                return Ok((Some(q), None));
            }
            Err(bad(cmd, format!("unknown sketch entity `{r}`")))
        };
        let e0 = ent(ents.first().map(String::as_str).unwrap_or(""))?;
        let e1 = match ents.get(1) {
            Some(r) => Some(ent(r)?),
            None => None,
        };
        let is_line = |c: usize| matches!(sk.curves.get(c).map(|c| &c.kind), Some(CurveKind::Line { .. }));
        let is_circle = |c: usize| matches!(sk.curves.get(c).map(|c| &c.kind), Some(CurveKind::Circle { .. }));
        // The other reading of an angle between two lines (see below).
        let mut alt: Option<(ConstraintKind, f64)> = None;
        let (mut k, mut cur): (ConstraintKind, f64) = match (e0, e1) {
            ((None, Some(c)), None) if ty == "sweep" => {
                let angle = arc_len(sk, c).zip(sk.radius(c)).map(|(len, r)| len / r).ok_or_else(|| bad(cmd, "sweep needs an arc"))?;
                let angle = if sk.curves.get(c).is_some_and(|c| c.reversed) { -angle } else { angle };
                (ConstraintKind::ArcSweep { c, value: angle }, angle)
            }
            ((None, Some(c)), None) if is_line(c) => {
                let (a, b) = line_pts(sk, c).unwrap_or_default();
                match auto_orientation(&ty, a, b, text_at) {
                    "angle" => {
                        let angle = (b - a).angle();
                        (ConstraintKind::LineAngle { l: c, value: angle }, angle)
                    }
                    "horizontal" => {
                        (ConstraintKind::DistanceX { p: line_ends(sk, c).0, q: line_ends(sk, c).1, value: (b.x - a.x).abs() }, (b.x - a.x).abs())
                    }
                    "vertical" => {
                        (ConstraintKind::DistanceY { p: line_ends(sk, c).0, q: line_ends(sk, c).1, value: (b.y - a.y).abs() }, (b.y - a.y).abs())
                    }
                    _ => (ConstraintKind::Length { l: c, value: a.dist(b) }, a.dist(b)),
                }
            }
            ((None, Some(c)), None) if ty == "arc_length" || ty == "arclength" => {
                let len = arc_len(sk, c).ok_or_else(|| bad(cmd, "arc length needs an arc"))?;
                (ConstraintKind::ArcLength { c, value: len }, len)
            }
            ((None, Some(c)), None) if matches!(sk.curves.get(c).map(|x| &x.kind), Some(CurveKind::Ellipse { .. })) => {
                // Ellipse: the major radius is the centre–axis-end distance, the minor its own.
                let Some(CurveKind::Ellipse { c: cp, m, r }) = sk.curves.get(c).map(|x| x.kind.clone()) else {
                    return Err(bad(cmd, "ellipse"));
                };
                if ty == "major" {
                    let d = sk.point(cp).zip(sk.point(m)).map(|(a, b)| a.dist(b)).unwrap_or(0.0);
                    (ConstraintKind::Distance { p: cp, q: m, value: d }, d)
                } else {
                    (ConstraintKind::Radius { c, value: r }, r)
                }
            }
            ((None, Some(c)), None) => {
                let r = sk.radius(c).ok_or_else(|| bad(cmd, "cannot dimension that curve"))?;
                let diameter = ty == "diameter" || (ty != "radius" && is_circle(c));
                if diameter { (ConstraintKind::Diameter { c, value: 2.0 * r }, 2.0 * r) } else { (ConstraintKind::Radius { c, value: r }, r) }
            }
            ((Some(a), None), Some((Some(b), None))) => {
                let (pa, pb) = (sk.point(a).unwrap_or_default(), sk.point(b).unwrap_or_default());
                match auto_orientation(&ty, pa, pb, text_at) {
                    "horizontal" => (ConstraintKind::DistanceX { p: a, q: b, value: (pb.x - pa.x).abs() }, (pb.x - pa.x).abs()),
                    "vertical" => (ConstraintKind::DistanceY { p: a, q: b, value: (pb.y - pa.y).abs() }, (pb.y - pa.y).abs()),
                    _ => (ConstraintKind::Distance { p: a, q: b, value: pa.dist(pb) }, pa.dist(pb)),
                }
            }
            ((Some(q), None), Some((None, Some(l)))) | ((None, Some(l)), Some((Some(q), None))) if is_line(l) => {
                let (a, b) = line_pts(sk, l).unwrap_or_default();
                let pq = sk.point(q).unwrap_or_default();
                let d = (b - a).normalized().map(|d| d.cross(pq - a).abs()).unwrap_or(0.0);
                if diameter_about(sk, l, &ty) {
                    (ConstraintKind::LinearDiameter { p: q, l, value: 2.0 * d }, 2.0 * d)
                } else {
                    (ConstraintKind::PointLineDistance { p: q, l, value: d }, d)
                }
            }
            ((None, Some(a)), Some((None, Some(b)))) if ty == "overall_length" => {
                let current = sk
                    .center(a)
                    .zip(sk.center(b))
                    .zip(sk.radius(a).zip(sk.radius(b)))
                    .map(|((ca, cb), (ra, rb))| ca.dist(cb) + ra + rb)
                    .ok_or_else(|| bad(cmd, "overall slot length needs two arc caps"))?;
                (ConstraintKind::SlotOverallLength { a, b, value: current }, current)
            }
            ((None, Some(a)), Some((None, Some(b)))) if is_line(a) && is_line(b) => {
                let (a0, a1) = line_pts(sk, a).unwrap_or_default();
                let (b0, b1) = line_pts(sk, b).unwrap_or_default();
                let (da, db) = (a1 - a0, b1 - b0);
                if da.cross(db).abs() < 1e-9 * da.len() * db.len() && ty != "angle" {
                    // Parallel lines: distance between them (a diameter about a centerline).
                    let (a, b, b0) = if sk.curves.get(a).is_some_and(|c| c.centerline) { (a, b, b0) } else { (b, a, a0) };
                    let (bq, _) = line_ends(sk, b);
                    let (a0, a1) = line_pts(sk, a).unwrap_or_default();
                    let d = (a1 - a0).normalized().map(|d| d.cross(b0 - a0).abs()).unwrap_or(0.0);
                    if diameter_about(sk, a, &ty) {
                        (ConstraintKind::LinearDiameter { p: bq, l: a, value: 2.0 * d }, 2.0 * d)
                    } else {
                        (ConstraintKind::PointLineDistance { p: bq, l: a, value: d }, d)
                    }
                } else {
                    match text_at {
                        Some(t) => angle_by_sector(a, b, (a0, a1), (b0, b1), t).ok_or_else(|| bad(cmd, "cannot place that angle"))?,
                        None => {
                            let mut ang = da.cross(db).atan2(da.dot(db));
                            let (mut ka, mut kb) = (a, b);
                            if ang < 0.0 {
                                ang = -ang;
                                std::mem::swap(&mut ka, &mut kb);
                            }
                            // Two lines make two angles, θ and 180° − θ, whichever way they were
                            // drawn: the acute one unless the value asks for the other.
                            let direct = (ConstraintKind::Angle { a: ka, b: kb, value: ang, flip: false }, ang);
                            let supplement =
                                (ConstraintKind::Angle { a: kb, b: ka, value: std::f64::consts::PI - ang, flip: true }, std::f64::consts::PI - ang);
                            let (acute, obtuse) = if ang <= std::f64::consts::FRAC_PI_2 { (direct, supplement) } else { (supplement, direct) };
                            alt = Some(obtuse);
                            acute
                        }
                    }
                }
            }
            _ => return Err(bad(cmd, "cannot dimension that combination of entities")),
        };
        let is_angle = k.is_angle();
        if driven {
            // A reference dimension: measures, no parameter.
            let name = k.name();
            let id = sk.add_constraint(k, None)?;
            place_text(sk, &id, text_at);
            if let Some(c) = sk.constraints.iter_mut().find(|c| c.id == id) {
                c.driven = true;
            }
            before = None;
            return Ok((String::new(), name, measured(is_angle, cur)));
        }
        let (unit, default_expr) =
            if is_angle { ("deg", format!("{} deg", round6(cur.to_degrees()))) } else { ("mm", format!("{} mm", round6(cur))) };
        let e = value.as_ref().map(|e| doc.with_design_unit(e, if is_angle { Kind::Angle } else { Kind::Length })).unwrap_or(default_expr);
        let pname = doc.new_model_param(&e, unit);
        // The value must evaluate.
        let (vals, errs) = doc.param_values();
        if let Some(err) = errs.get(&pname) {
            return Err(bad(cmd, format!("value: {err}")));
        }
        // An angle between two lines dimensions whichever of its two readings is nearer the value.
        if let (Some((ka, ca)), Some(want)) = (alt, vals.get(&pname).and_then(|v| v.to_kind(Kind::Angle).ok()))
            && (want - ca).abs() < (want - cur).abs()
        {
            (k, cur) = (ka, ca);
        }
        // How far the value is from what the geometry measures now: zero for a dimension that
        // only restates what the sketch already has.
        let want = vals.get(&pname).and_then(|v| v.to_kind(if is_angle { Kind::Angle } else { Kind::Length }).ok());
        residual = want.map(|w| (w - cur).abs()).filter(|r| r.is_finite());
        // The first length dimension of a sketch with nothing sized yet scales it (#1365).
        if scale_sketch
            && !is_angle
            && let Some(factor) = want.map(|w| w / cur).filter(|f| f.is_finite() && (f - 1.0).abs() > 1e-9)
            && let Some(big) = sk.scaled_for_first_dimension(factor)
        {
            *sk = big;
            scaled = Some(factor);
            residual = Some(0.0);
        }
        let name = k.name();
        let id = sk.add_constraint(k, Some(pname.clone()))?;
        new_id = id.clone();
        place_text(sk, &id, text_at);
        Ok((pname, name, measured(is_angle, cur)))
    })?;
    // Report what the dimension measures once the sketch is solved, not the geometry before.
    let current = solved_measure(s, p, cmd).unwrap_or(current);
    if driven {
        return Ok(json!({"param": Value::Null, "driven": true, "type": kind_name, "measured": current.0, "unit": current.1, "sketch": info}));
    }
    // A dimension the sketch already determines cannot drive it: it is refused, or with
    // `if_redundant: "driven"` placed as a driven (reference) dimension instead.
    let by = redundancy(s, p, cmd, before, &info, residual, &new_id)
        .map_err(|_| bad(cmd, "that dimension conflicts with the sketch: other constraints already determine it with a different value"))?;
    if let Some(by) = by {
        if str_(p, "if_redundant").is_some_and(|m| m.eq_ignore_ascii_case("driven")) {
            s.doc = start;
            let mut q = p.clone();
            q["driven"] = json!(true);
            if let Some(o) = q.as_object_mut() {
                o.remove("if_redundant");
                o.remove("value");
            }
            let mut out = dimension(s, &q)?;
            out["redundant"] = json!(true);
            out["determined_by"] = json!(by);
            return Ok(out);
        }
        return Err(bad(
            cmd,
            format!("that dimension is already determined by {}; add it as a driven (reference) dimension with driven: true", listed(&by)),
        ));
    }
    let v = s.doc.param(&param).map(|p| p.expr.clone()).unwrap_or_default();
    let mut out = json!({"param": param, "type": kind_name, "expression": v, "measured": current.0, "unit": current.1, "sketch": info});
    if let Some(factor) = scaled {
        out["scaled"] = json!(round6(factor));
    }
    Ok(out)
}

/// A dimension's measured value as reported, in the unit its expression is written in.
fn measured(is_angle: bool, v: f64) -> (f64, &'static str) {
    if is_angle { (round6(v.to_degrees()), "deg") } else { (round6(v), "mm") }
}

/// What the dimension just added (the sketch's last constraint) measures on the solved sketch.
fn solved_measure(s: &Session, p: &Value, cmd: &str) -> Option<(f64, &'static str)> {
    let sk = s.doc.sketch(target_sketch(s, p, cmd).ok()?).ok()?;
    let k = &sk.constraints.last()?.kind;
    Some(measured(k.is_angle(), solidcraft_sketch::measure_dimension(sk, k)?))
}

/// The type of a dimension between `a` and `b` (two points or a line's ends). `auto` takes it
/// from where the text is placed: across from the two (between the perpendiculars through
/// them) the distance between them; past that, above or below them the horizontal distance,
/// beside them the vertical one (#257). With no text, or when the two already line up with
/// an axis, `auto` stays the distance between them; other types are returned unchanged.
fn auto_orientation(ty: &str, a: Vec2, b: Vec2, text_at: Option<Vec2>) -> &str {
    let Some(t) = text_at.filter(|_| ty == "auto") else { return ty };
    let d = b - a;
    let len = d.len();
    let along = (t - a).dot(d) / len;
    let between = |v: f64, p: f64, q: f64| p.min(q) <= v && v <= p.max(q);
    if d.x.abs() <= 1e-9 * len || d.y.abs() <= 1e-9 * len || !along.is_finite() || (0.0..=len).contains(&along) {
        ty
    } else if between(t.x, a.x, b.x) {
        "horizontal"
    } else if between(t.y, a.y, b.y) {
        "vertical"
    } else {
        ty
    }
}

/// Keep a dimension's text where it was placed (`at`, sketch coordinates), relative to the
/// dimension so it follows the geometry.
fn place_text(sk: &mut Sketch, id: &str, at: Option<Vec2>) {
    let Some(i) = sk.constraints.iter().position(|c| c.id == id) else { return };
    let text = match (at, sk.constraints.get(i).and_then(|c| solidcraft_sketch::dim_frame(sk, &c.kind))) {
        (Some(at), Some(f)) => Some(solidcraft_sketch::encode_text(&f, at)),
        _ => None,
    };
    if let Some(c) = sk.constraints.get_mut(i) {
        c.text = text.filter(|t| t.is_finite() && t.x.abs() < 1e9 && t.y.abs() < 1e9);
    }
}

fn dimension_text(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.dimension_text";
    let key = str_(p, "dimension").ok_or_else(|| bad(cmd, "`dimension` must be a dimension id or parameter name"))?.to_string();
    let at = if bool_(p, "reset").unwrap_or(false) { None } else { Some(req_vec2(cmd, p, "at")?) };
    let (id, info) = edit(s, p, cmd, false, |sk, _| {
        let c = sk
            .constraints
            .iter()
            .find(|c| c.kind.is_dimension() && (c.id == key || c.param.as_deref() == Some(key.as_str())))
            .ok_or_else(|| bad(cmd, format!("no dimension `{key}` in this sketch")))?;
        let id = c.id.clone();
        place_text(sk, &id, at);
        Ok(id)
    })?;
    Ok(json!({"dimension": id, "sketch": info}))
}

/// Dimension a point (or a parallel line) to line `l` as a diameter: when `l` is a centerline
/// (unless a plain distance was asked for) or when a diameter was asked for.
fn diameter_about(sk: &Sketch, l: usize, ty: &str) -> bool {
    ty == "diameter" || (ty == "auto" && sk.curves.get(l).is_some_and(|c| c.centerline))
}

fn arc_len(sk: &Sketch, c: usize) -> Option<f64> {
    match sk.segs(c).first()? {
        solidcraft_geom::Seg2::Arc { radius, sweep, .. } if matches!(sk.curves.get(c)?.kind, CurveKind::Arc { .. }) => Some(radius * sweep),
        _ => None,
    }
}

/// The angle dimension between two lines whose sector contains `t` (the text position): returns
/// the constraint and the current value of that sector angle.
fn angle_by_sector(a: usize, b: usize, (a0, a1): (Vec2, Vec2), (b0, b1): (Vec2, Vec2), t: Vec2) -> Option<(ConstraintKind, f64)> {
    let (da, db) = ((a1 - a0).normalized()?, (b1 - b0).normalized()?);
    let den = da.cross(db);
    if den.abs() < 1e-12 {
        return None;
    }
    let s = (b0 - a0).cross(db) / den;
    let x = a0 + da * s;
    let ta = (t - x).angle();
    // The four rays from the intersection, with their line and direction sign.
    let mut rays: Vec<(f64, usize, bool)> = vec![(da.angle(), a, true), ((-da).angle(), a, false), (db.angle(), b, true), ((-db).angle(), b, false)];
    rays.sort_by(|p, q| p.0.total_cmp(&q.0));
    let tau = std::f64::consts::TAU;
    for k in 0..4 {
        let (s0, l0, p0) = rays[k];
        let (s1, l1, p1) = rays[(k + 1) % 4];
        let span = (s1 - s0).rem_euclid(tau);
        let off = (ta - s0).rem_euclid(tau);
        if off <= span && l0 != l1 {
            return Some((ConstraintKind::Angle { a: l0, b: l1, value: span, flip: p0 != p1 }, span));
        }
    }
    None
}

pub(super) fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn line_ends(sk: &Sketch, l: usize) -> (usize, usize) {
    match sk.curves.get(l).map(|c| &c.kind) {
        Some(CurveKind::Line { a, b }) => (*a, *b),
        _ => (0, 0),
    }
}

// ---------------------------------------------------------------------------------------------
// Editing

fn construction(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.construction";
    let ids = string_list(p, "curves");
    if ids.is_empty() {
        return Err(bad(cmd, "`curves` must list curve ids"));
    }
    let want = bool_(p, "value");
    let (n, info) = edit(s, p, cmd, false, |sk, _| {
        let cs: Vec<usize> =
            ids.iter().map(|id| sk.curve_index(id).ok_or_else(|| bad(cmd, format!("unknown curve `{id}`")))).collect::<Result<_>>()?;
        // The selection switches together (a slot or polygon as a whole): to construction
        // unless all of it already is. Projected curves may switch too. A centerline that
        // becomes construction stops being a centerline (one line type at a time).
        let to = want.unwrap_or_else(|| !cs.iter().all(|c| sk.curves.get(*c).is_some_and(|c| c.construction)));
        let mut n = 0;
        for c in cs {
            if let Some(cu) = sk.curves.get_mut(c)
                && (cu.construction != to || to && cu.centerline || !to && cu.infinite)
            {
                cu.construction = to;
                if to {
                    cu.centerline = false;
                } else {
                    // A normal line is a finite one.
                    cu.infinite = false;
                }
                n += 1;
            }
        }
        Ok((n, to))
    })?;
    Ok(json!({"changed": n.0, "construction": n.1, "sketch": info}))
}

fn sketch_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.delete";
    let ids = string_list(p, "entities");
    if ids.is_empty() {
        return Err(bad(cmd, "`entities` must list sketch entity ids"));
    }
    let (n, info) = edit(s, p, cmd, false, |sk, doc| {
        let mut curves = Vec::new();
        let mut points = Vec::new();
        let mut cons = Vec::new();
        for id in &ids {
            if let Some(c) = sk.curve_index(id) {
                curves.push(c);
            } else if let Some(q) = sk.point_index(id) {
                if q == 0 {
                    return Err(bad(cmd, "the sketch origin cannot be deleted"));
                }
                points.push(q);
            } else if sk.constraints.iter().any(|c| &c.id == id) {
                cons.push(id.clone());
            } else if sk.wire_index(id).is_some() {
                sk.remove_wires(std::slice::from_ref(id));
            } else if sk.link(id).is_some() {
                curves.extend(sk.link_curves(id));
                points.extend(sk.link_points(id));
            } else {
                return Err(bad(cmd, format!("unknown sketch entity `{id}`")));
            }
        }
        let selected_curves = curves.iter().copied().collect::<std::collections::BTreeSet<_>>();
        let selected_points = points.iter().copied().collect::<std::collections::BTreeSet<_>>();
        let removed = sk
            .curves
            .iter()
            .enumerate()
            .filter(|(index, curve)| selected_curves.contains(index) || curve.kind.point_ids().iter().any(|point| selected_points.contains(point)))
            .map(|(_, curve)| curve.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for offset in &sk.offsets {
            let Some(topology) = &offset.topology else { continue };
            if offset.outputs.iter().any(|id| removed.contains(id.as_str()))
                && topology
                    .roles
                    .iter()
                    .any(|role| !role.retired && role.kind == solidcraft_sketch::OffsetRoleKind::Arc && !removed.contains(role.output.as_str()))
            {
                return Err(bad(cmd, "break the offset association before deleting only part of its curved boundary"));
            }
        }
        if sk.constraints.iter().any(|c| cons.contains(&c.id) && matches!(c.kind, ConstraintKind::OffsetDistance { .. })) {
            let report = solve_current(sk, doc)?;
            if !report.keeps_orientation() {
                return Err(bad(cmd, "cannot remove an offset dimension from a conflicting sketch"));
            }
            sk.refresh_offsets()?;
        }
        sk.constraints.retain(|c| !cons.contains(&c.id));
        let pids: Vec<String> = points.iter().filter_map(|q| sk.points.get(*q).map(|x| x.id.clone())).collect();
        sk.remove_curves(&curves);
        let pidx: Vec<usize> = pids.iter().filter_map(|id| sk.point_index(id)).collect();
        sk.remove_points(&pidx);
        doc.prune_model_params();
        Ok(ids.len())
    })?;
    // Dimension parameters may have been dropped; prune again with the stored sketch.
    s.doc_mut().prune_model_params();
    Ok(json!({"deleted": n, "sketch": info}))
}

fn move_point(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.move_point";
    let to = req_vec2(cmd, p, "to")?;
    let id = target_sketch(s, p, cmd)?;
    let (_, rep) = edit_solved(s, id, false, |sk, _| {
        let q = point_ref(sk, p.get("point"), cmd, "point")?;
        Ok(((), drag_entity(sk, Ent::Point(q), to, to, false).1.solve_again()))
    })?;
    Ok(json!({"sketch": rep.as_ref().map(report_json)}))
}

fn drag(s: &mut Session, p: &Value) -> Result<Value> {
    drag_frame(s, p, false).map(|(v, _)| v)
}

fn drag_live(s: &mut Session, p: &Value) -> Result<(Value, Option<crate::LiveEdit>)> {
    drag_frame(s, p, true)
}

/// `sketch.drag`; `live`: one frame of an interactive drag, returned with the sketch to show.
fn drag_frame(s: &mut Session, p: &Value, live: bool) -> Result<(Value, Option<crate::LiveEdit>)> {
    let cmd = "sketch.drag";
    let to = req_vec2(cmd, p, "to")?;
    let from = match p.get("from") {
        None | Some(Value::Null) => to,
        Some(v) => vec2(v).ok_or_else(|| bad(cmd, "`from` must be a point [x, y]"))?,
    };
    let id = target_sketch(s, p, cmd)?;
    // The sketch's report as last evaluated: a live frame keeps its degrees of freedom.
    let last = s.model.result(id).and_then(|r| r.state.sketch(id)).map(|ss| ss.report.clone());
    let (d, rep) = edit_solved(s, id, false, |sk, doc| {
        let r = p.get("entity").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`entity` must be a sketch point or curve"))?;
        let e = ent(sk, r).ok_or_else(|| bad(cmd, format!("unknown sketch entity `{r}`")))?;
        if let Some(link) = super::sketch_create::text_link(sk, r).map(str::to_owned) {
            let moving = sk.link_points(&link).into_iter().filter_map(|i| sk.point(i).map(|p| (i, p))).collect();
            super::sketch_create::move_text(sk, &link, to - from, 0.0, Vec2::ZERO, false)?;
            return Ok((drag::Dragged { reached: 1.0, moving }, Left::Unsolved));
        }
        // The held solves work on the dimensions' current values (a solved step applies them).
        doc.apply_dimension_values(&doc.param_values().0, sk)?;
        let (d, left) = drag_entity(sk, e, from, to, live);
        // A live frame's held solve is its only one; otherwise the sketch's own solve reports
        // its degrees of freedom.
        Ok((d, if live && last.is_some() { left } else { left.solve_again() }))
    })?;
    // Where the grabbed spot went: `from` moved as the dragged points did on average (with the
    // whole step when it followed; otherwise as the sketch's own solve left them).
    let sk = s.doc.sketch(id)?;
    let moves: Vec<Vec2> = d.moving.iter().filter_map(|&(q, was)| Some(sk.point(q)? - was)).collect();
    let at = from + moves.iter().fold(Vec2::ZERO, |a, m| a + *m) * (1.0 / moves.len().max(1) as f64);
    let rep = rep.or_else(|| last.map(|last| SolveReport { status: solidcraft_sketch::SolveStatus::Solved, failing: Vec::new(), ..last }));
    let out = json!({"sketch": rep.as_ref().map(report_json), "reached": d.reached, "at": [at.x, at.y]});
    Ok((out, rep.filter(|_| live).map(|report| crate::LiveEdit { sketch: id, report })))
}

/// Ends a live drag: the snap constraint it let go on, if any, then one evaluation of the model.
/// It reads the model only before the sketch it edits (never stale), so `execute` does not
/// evaluate first.
fn drag_end(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.drag_end";
    let id = target_sketch(s, p, cmd)?;
    let start = s.doc.clone();
    let mut constraint = Value::Null;
    if let Some(c) = p.get("constraint").filter(|c| !c.is_null()) {
        let name = c.get("command").and_then(Value::as_str).unwrap_or_default();
        let spec = super::find_command(name)
            .filter(|spec| spec.id.starts_with("sketch.constraint."))
            .ok_or_else(|| bad(cmd, "`constraint.command` must be a sketch.constraint.* command"))?;
        let params = c.get("params").cloned().unwrap_or(Value::Null);
        // A constraint that cannot be added (it would over-constrain the sketch) is not kept.
        constraint = spec.run_nested(s, &params).unwrap_or_else(|e| {
            s.doc = start.clone();
            json!({"error": e.to_string()})
        });
    }
    // A new constraint changed the document: `execute` evaluates the model after this.
    if !std::sync::Arc::ptr_eq(&start, &s.doc) {
        return Ok(json!({"constraint": constraint, "sketch": constraint.get("sketch")}));
    }
    if s.model.is_stale() {
        s.refresh();
    }
    let report = s.model.result(id).and_then(|r| r.state.sketch(id)).map(|ss| report_json(&ss.report));
    Ok(json!({"constraint": constraint, "sketch": report}))
}

/// The Sketch Palette options of a sketch, by their palette names (true = shown / on).
fn options_json(v: &solidcraft_sketch::SketchView) -> Value {
    json!({
        "show_profile": !v.hide_profile,
        "show_points": !v.hide_points,
        "show_dimensions": !v.hide_dimensions,
        "show_constraints": !v.hide_constraints,
        "show_projected": !v.hide_projected,
        "show_construction": !v.hide_construction,
        "grid": !v.hide_grid,
        "snap": !v.no_snap,
        "slice": v.slice,
        "sketch_3d": v.three_d,
    })
}

fn sketch_options(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.options";
    let keys = [
        "show_profile",
        "show_points",
        "show_dimensions",
        "show_constraints",
        "show_projected",
        "show_construction",
        "grid",
        "snap",
        "slice",
        "sketch_3d",
    ];
    if let Some(o) = p.as_object()
        && let Some(k) = o.keys().find(|k| *k != "sketch" && !keys.contains(&k.as_str()))
    {
        return Err(bad(cmd, format!("unknown option `{k}`")));
    }
    let set = |k: &str| bool_(p, k);
    if keys.iter().all(|k| set(k).is_none()) {
        let id = target_sketch(s, p, cmd)?;
        return Ok(json!({"options": options_json(&s.doc.sketch(id)?.view)}));
    }
    let (v, info) = edit(s, p, cmd, false, |sk, _| {
        let v = &mut sk.view;
        let put = |k: &str, flag: &mut bool, shown_means_off: bool| {
            if let Some(b) = set(k) {
                *flag = if shown_means_off { !b } else { b };
            }
        };
        put("show_profile", &mut v.hide_profile, true);
        put("show_points", &mut v.hide_points, true);
        put("show_dimensions", &mut v.hide_dimensions, true);
        put("show_constraints", &mut v.hide_constraints, true);
        put("show_projected", &mut v.hide_projected, true);
        put("show_construction", &mut v.hide_construction, true);
        put("grid", &mut v.hide_grid, true);
        put("snap", &mut v.no_snap, true);
        put("slice", &mut v.slice, false);
        put("sketch_3d", &mut v.three_d, false);
        Ok(options_json(v))
    })?;
    Ok(json!({"options": v, "sketch": info}))
}

fn chain_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.chain";
    let id = target_sketch(s, p, cmd)?;
    let sk = s.doc.sketch(id)?;
    let c = curve_ref(sk, p.get("curve"), cmd, "curve")?;
    Ok(json!({"curves": ids_of(sk, &sk.chain(c))}))
}

fn solve_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_sketch(s, p, "sketch.solve")?;
    let st = s.model.state();
    let ss = st.sketch(id).ok_or_else(|| EngineError::Other("the sketch has not been evaluated".into()))?;
    Ok(json!({"status": ss.report.status, "dof": ss.report.dof, "failing": ss.report.failing, "profiles": ss.profiles.len()}))
}

#[cfg(test)]
#[path = "sketch_flip_tests.rs"]
mod flip_tests;

#[cfg(test)]
#[path = "sketch_rect_tests.rs"]
mod rect_tests;

#[cfg(test)]
#[path = "sketch_rounded_rect_tests.rs"]
mod rounded_rect_tests;

#[cfg(test)]
#[path = "sketch_ux_tests.rs"]
mod ux_tests;

#[cfg(test)]
#[path = "sketch_axis_ref_tests.rs"]
mod axis_ref_tests;
#[cfg(test)]
#[path = "sketch_drag_live_tests.rs"]
mod drag_live_tests;
#[cfg(test)]
#[path = "sketch_ids_tests.rs"]
mod ids_tests;
#[cfg(test)]
#[path = "sketch_infinite_tests.rs"]
mod infinite_tests;
#[cfg(test)]
#[path = "sketch_redundant_tests.rs"]
mod redundant_tests;
#[cfg(test)]
#[path = "sketch_symmetry_point_tests.rs"]
mod symmetry_point_tests;
