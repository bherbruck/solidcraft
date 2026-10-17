//! Sketch modify tools: trim, extend, break, fillet, chamfer, offset, mirror, circular and
//! rectangular patterns, move/copy and scale.

use std::f64::consts::TAU;

use serde_json::{Value, json};
use solidcraft_doc::ProfileSel;
use solidcraft_geom::Vec2;
use solidcraft_sketch::{ConstraintKind, Curve, CurveKind, Shape, Sketch, intersections};

use super::sketch::{add_c, edit, ids_of, solve_current};
use super::{CommandSpec, in_sketch};
use crate::params::{bad, bool_, expr, num, req_vec2, str_, string_list, vec2, vec2_list};
use crate::{Result, Session};

#[path = "sketch_pattern.rs"]
mod pattern;
#[path = "sketch_support.rs"]
mod support;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("sketch.fillet", "Fillet", fillet)
        .at("SKETCH", "MODIFY")
        .icon("sketch_fillet")
        .enabled(in_sketch)
        .params(concat!("point: corner shared by two curves (\"l1.end\") | points: [corners] (all rounded alike: one radius, the rest equal) | a, b: two lines or arcs meeting at a corner; radius: expr. A corner that dimensions or constraints measure to stays as a virtual sharp corner on both curves' extensions (`virtual_corners`; the moved ones in `moved_to_virtual_corner`)", created_doc!())),
    CommandSpec::new("sketch.chamfer.equal_distance", "Equal Distance Chamfer", chamfer_equal)
        .at("SKETCH", "MODIFY")
        .icon("sketch_chamfer")
        .enabled(in_sketch)
        .params(concat!("point | a, b: a corner of two lines; distance", created_doc!())),
    CommandSpec::new("sketch.chamfer.distance_angle", "Distance and Angle Chamfer", chamfer_dist_angle)
        .at("SKETCH", "MODIFY")
        .icon("sketch_chamfer_angle")
        .enabled(in_sketch)
        .params(concat!("point | a, b: a corner of two lines; distance: along a, angle: deg from a", created_doc!())),
    CommandSpec::new("sketch.chamfer.two_distance", "Two Distance Chamfer", chamfer_two)
        .at("SKETCH", "MODIFY")
        .icon("sketch_chamfer_two_distances")
        .enabled(in_sketch)
        .params(concat!("point | a, b: a corner of two lines; distance: along a, distance2: along b", created_doc!())),
    CommandSpec::new("sketch.offset", "Offset", offset)
        .at("SKETCH", "MODIFY")
        .icon("offset")
        .key("O")
        .enabled(in_sketch)
        .params(concat!(
            "curves: [ids], distance: number in mm or length expression (creates an editable dimension; positive = left of the chain, or toward `side`), side?: [x,y], chain?: bool (default true: the connected chain); ",
            "direction?: one_side|symmetric (default one_side) or symmetric?: bool (the same distance each way; no `side`, whose job it takes over), ",
            "distance2?: positive length expression (Two Sides: an independent distance the other way, as for `solid.extrude`). Each side is its own offset association, listed in `sides`; flip?: bool reverses the primary side; `param`/`param2` name the driving dimensions",
            created_doc!()
        )),
    CommandSpec::new("sketch.offset.break", "Break Offset Association", break_offset).icon("sketch_break")
        .enabled(in_sketch)
        .params("curve: derived offset curve id (retains geometry and makes its points editable)"),
    CommandSpec::new("sketch.trim", "Trim", trim)
        .at("SKETCH", "MODIFY")
        .icon("trim")
        .key("T")
        .enabled(in_sketch)
        .params(concat!("curve: id, at: [x,y] (the piece to remove: between the nearest intersections around it)", created_doc!())),
    CommandSpec::new("sketch.extend", "Extend", extend)
        .at("SKETCH", "MODIFY")
        .icon("extend")
        .enabled(in_sketch)
        .params("curve: id, at: [x,y] near the end to extend (to the nearest curve)"),
    CommandSpec::new("sketch.break", "Break", break_cmd)
        .at("SKETCH", "MODIFY")
        .icon("sketch_break")
        .enabled(in_sketch)
        .params(concat!("curve: id, at: [x,y] (split at intersections) | points: [[x,y]…] (1…1000 positions projected onto the curve; duplicates are merged)", created_doc!())),
    CommandSpec::new("sketch.scale", "Sketch Scale", scale)
        .at("SKETCH", "MODIFY")
        .icon("sketch_scale")
        .enabled(in_sketch)
        .params("entities: [curve/point ids], base: [x,y] or point ref, factor: number"),
    CommandSpec::new("sketch.move", "Move/Copy Sketch Entities", move_copy)
        .at("SKETCH", "MODIFY")
        .icon("sketch_move")
        .enabled(in_sketch)
        .params(concat!("entities: [ids], translate?: [dx,dy], angle?: deg, center?: [x,y], copy?: bool; picking a text outline or linked point transforms its whole text source once, and copies remain editable text", created_doc!())),
    CommandSpec::new("sketch.mirror", "Mirror", mirror)
        .at("SKETCH", "PATTERN")
        .icon("sketch_mirror")
        .enabled(in_sketch)
        .params(concat!("entities: [curve/point ids], line: mirror line id (symmetry constraints are added)", created_doc!())),
    CommandSpec::new("sketch.pattern.circular", "Circular Pattern", pattern_circular)
        .at("SKETCH", "PATTERN")
        .icon("sketch_circ_pattern")
        .enabled(in_sketch)
        .params(concat!("entities: [ids], center: [x,y] or point ref, count: 2…500, angle?: total deg (default 360); alignment?: left|center|right (default right); reversed?: bool (default false). Quantity includes the original. Center balances copies around it; reversed flips the direction and extra side for even totals. Partial arcs span angle with angle/(count-1) spacing; full turns use angle/count. Adds associative pattern constraints; a coordinate center creates a constrainable point. Returns pattern and center ids. pattern?: existing pattern id to edit (omitted values retained; surviving instance ids retained)", created_doc!())),
    CommandSpec::new("sketch.pattern.rectangular", "Rectangular Pattern", pattern_rect)
        .at("SKETCH", "PATTERN")
        .icon("sketch_rect_pattern")
        .enabled(in_sketch)
        .params(concat!("entities: [ids], count, spacing; count2?, spacing2?; direction?, direction2?: sketch line ids (live references); dir?, dir2?: [x,y] vectors (defaults: sketch x, perpendicular to direction 1); alignment?, alignment2?: left|center|right (default right). Signed spacings reverse each direction. reversed?, reversed2?: bool (default false); reverses placement and the extra side of even centered totals. Quantities include the original. Adds associative pattern constraints. Returns pattern id. pattern?: existing pattern id to edit (omitted values retained; surviving instance ids retained)", created_doc!())),
];

const TOL: f64 = 1e-7;
const MAX_COPIES: usize = 500;

fn curve_arg(sk: &Sketch, p: &Value, cmd: &str) -> Result<usize> {
    let id = str_(p, "curve").ok_or_else(|| bad(cmd, "`curve` must be a curve id"))?;
    let c = sk.curve_index(id).ok_or_else(|| bad(cmd, format!("unknown curve `{id}`")))?;
    if sk.is_linked_curve(c) {
        return Err(bad(cmd, format!("`{id}` is projected geometry: break its link first")));
    }
    if sk.freeform_offsets.iter().any(|offset| offset.output == id)
        || sk.offsets.iter().filter_map(|offset| offset.topology.as_ref()).any(|topology| topology.roles.iter().any(|role| role.output == id))
    {
        return Err(bad(cmd, "break the offset association before editing its output"));
    }
    Ok(c)
}

/// Parameter of `p` on the bounded shape, ends included.
fn on_bounded(sh: &Shape, p: Vec2) -> Option<f64> {
    if sh.dist(p) > TOL * 10.0 {
        return None;
    }
    let t = sh.param(p);
    let (t0, t1) = sh.range();
    let slack = match *sh {
        Shape::Line { a, b } => TOL * 10.0 / a.dist(b).max(1e-12),
        Shape::Round { r, .. } => TOL * 10.0 / r.max(1e-12),
    };
    if sh.is_full() || (t >= t0 - slack && t <= t1 + slack) { Some(t) } else { None }
}

/// Points where other curves meet curve `ci` (crossings and end points touching it), with the
/// curve that makes each one and the parameter on `ci`.
fn cut_points(sk: &Sketch, ci: usize) -> Vec<(f64, Vec2, usize)> {
    let Some(sh) = sk.shape(ci) else { return Vec::new() };
    let mut out: Vec<(f64, Vec2, usize)> = Vec::new();
    for (oi, _) in sk.curves.iter().enumerate() {
        if oi == ci {
            continue;
        }
        let Some(other) = sk.shape(oi) else { continue };
        let mut pts = intersections(&sh, &other);
        // Overlapping or touching ends that the analytic intersection misses (parallel lines).
        if let Shape::Line { a, b } = other {
            pts.extend([a, b]);
        }
        if let Shape::Round { sweep, .. } = other
            && sweep < TAU - 1e-12
        {
            pts.extend([other.point(0.0), other.point(sweep)]);
        }
        // An infinite line (#1359) cuts wherever its line crosses, beyond its two points too.
        let other_infinite = sk.curves.get(oi).is_some_and(|c| c.infinite);
        for q in pts {
            if (other_infinite || on_bounded(&other, q).is_some())
                && let Some(t) = on_bounded(&sh, q)
                && !out.iter().any(|(_, x, _)| x.dist(q) < TOL * 10.0)
            {
                out.push((t, q, oi));
            }
        }
    }
    for (q, oi) in super::sketch_freeform::analytic_cut_points(sk, ci) {
        if let Some(t) = on_bounded(&sh, q)
            && !out.iter().any(|(_, x, _)| x.dist(q) < TOL * 10.0)
        {
            out.push((t, q, oi));
        }
    }
    out
}

/// Constrain a new end point onto the curve that cut there.
fn pin(sk: &mut Sketch, p: usize, cutter: usize) -> Result<()> {
    let at = sk.point(p).unwrap_or_default();
    // An end point of the cutter there: coincident with it; otherwise on the curve.
    let ends: Vec<usize> = match sk.curves.get(cutter).map(|c| c.kind.clone()) {
        Some(CurveKind::Line { a, b }) | Some(CurveKind::Arc { a, b, .. }) => vec![a, b],
        _ => Vec::new(),
    };
    if let Some(e) = ends.into_iter().find(|e| sk.point(*e).is_some_and(|q| q.dist(at) < TOL * 10.0)) {
        if e != p {
            add_c(sk, ConstraintKind::Coincident { p, q: e })?;
        }
        return Ok(());
    }
    add_c(sk, ConstraintKind::PointOnCurve { p, c: cutter })?;
    Ok(())
}

/// Split curve `ci` at interior parameters `ts` (sorted). Returns the pieces in parameter
/// order (the first keeps the original index) and the new points.
fn split_at(sk: &mut Sketch, ci: usize, ts: &[f64]) -> Result<(Vec<usize>, Vec<usize>)> {
    let sh = sk.shape(ci).ok_or_else(|| bad("split", "curve"))?;
    let kind = sk.curves.get(ci).map(|c| c.kind.clone()).ok_or_else(|| bad("split", "curve"))?;
    let construction = sk.curves.get(ci).is_some_and(|c| c.construction);
    let mut pts = Vec::new();
    for t in ts {
        pts.push(sk.add_point(sh.point(*t), None)?);
    }
    let mut pieces = vec![ci];
    let mut push = |sk: &mut Sketch, kind: CurveKind, prefix: &str| -> Result<()> {
        let id = sk.fresh(prefix)?;
        sk.curves.push(Curve { id, kind, construction, reversed: false, link: None, centerline: false, fixed: false, infinite: false });
        pieces.push(sk.curves.len() - 1);
        Ok(())
    };
    match kind {
        CurveKind::Line { a, b } => {
            let mut chain = vec![a];
            chain.extend(&pts);
            chain.push(b);
            if let Some(c) = sk.curves.get_mut(ci) {
                c.kind = CurveKind::Line { a, b: chain.get(1).copied().unwrap_or(b) };
            }
            for w in chain.windows(2).skip(1) {
                push(sk, CurveKind::Line { a: w[0], b: w[1] }, "l")?;
            }
        }
        CurveKind::Arc { c, a, b } => {
            let mut chain = vec![a];
            chain.extend(&pts);
            chain.push(b);
            if let Some(cu) = sk.curves.get_mut(ci) {
                cu.kind = CurveKind::Arc { c, a, b: chain.get(1).copied().unwrap_or(b) };
            }
            for w in chain.windows(2).skip(1) {
                push(sk, CurveKind::Arc { c, a: w[0], b: w[1] }, "a")?;
            }
        }
        CurveKind::Circle { c, .. } => {
            if pts.len() < 2 {
                return Err(bad("split", "a circle needs two points to split at"));
            }
            let mut chain = pts.clone();
            if let Some(f) = pts.first() {
                chain.push(*f);
            }
            if let Some(cu) = sk.curves.get_mut(ci) {
                cu.kind = CurveKind::Arc { c, a: chain[0], b: chain[1] };
                cu.reversed = false;
            }
            for w in chain.windows(2).skip(1) {
                push(sk, CurveKind::Arc { c, a: w[0], b: w[1] }, "a")?;
            }
        }
        _ => return Err(bad("split", "not supported yet for ellipses, splines and conics")),
    }
    Ok((pieces, pts))
}

/// Keep the pieces of a split tied together: lines collinear, arcs on one circle.
fn tie(sk: &mut Sketch, pieces: &[usize]) -> Result<()> {
    for w in pieces.windows(2) {
        let (a, b) = (w[0], w[1]);
        let line = matches!(sk.curves.get(a).map(|c| &c.kind), Some(CurveKind::Line { .. }));
        add_c(sk, if line { ConstraintKind::Collinear { a, b } } else { ConstraintKind::Equal { a, b } })?;
    }
    Ok(())
}

/// The cut parameters bracketing `tq` on curve `ci`: (before, after) with their cutter curves.
fn bracket(sk: &Sketch, ci: usize, tq: f64) -> (Vec<(f64, Vec2, usize)>, bool) {
    let Some(sh) = sk.shape(ci) else { return (Vec::new(), false) };
    let (t0, t1) = sh.range();
    let full = sh.is_full();
    let span = t1 - t0;
    let mut cuts: Vec<(f64, Vec2, usize)> =
        cut_points(sk, ci).into_iter().filter(|(t, _, _)| full || (*t > t0 + 1e-9 * span && *t < t1 - 1e-9 * span)).collect();
    cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
    if full {
        if cuts.len() < 2 {
            return (Vec::new(), true);
        }
        // Cuts around tq on the circle (wrapping).
        let after = cuts.iter().position(|c| c.0 > tq).unwrap_or(0);
        let before = (after + cuts.len() - 1) % cuts.len();
        let mut two = vec![cuts[before], cuts[after]];
        two.sort_by(|a, b| a.0.total_cmp(&b.0));
        return (two, true);
    }
    let before = cuts.iter().rev().find(|c| c.0 < tq).copied();
    let after = cuts.iter().find(|c| c.0 > tq).copied();
    (before.into_iter().chain(after).collect(), false)
}

fn trim(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.trim";
    let at = req_vec2(cmd, p, "at")?;
    let (out, info) = edit(s, p, cmd, false, |sk, doc| {
        let ci = curve_arg(sk, p, cmd)?;
        let id = sk.curves.get(ci).map(|c| c.id.clone()).unwrap_or_default();
        if sk.curves.get(ci).is_some_and(|c| c.infinite) {
            return Err(bad(cmd, "an infinite line has no ends to trim back to: make it a normal line first"));
        }
        if sk.curves.get(ci).is_some_and(|c| c.kind.is_freeform()) {
            let r = super::sketch_freeform::trim_or_break(sk, ci, at, true, cmd)?;
            doc.prune_model_params();
            return Ok(json!({"trimmed": id, "pieces": r["curves"]}));
        }
        let sh = sk.shape(ci).ok_or_else(|| bad(cmd, "curve"))?;
        let tq = sh.param(sh.project(at));
        let (cuts, full) = bracket(sk, ci, tq);
        if cuts.is_empty() {
            // Nothing crosses it: the whole curve goes.
            sk.remove_curves(&[ci]);
            doc.prune_model_params();
            return Ok(json!({"deleted": id}));
        }
        let ts: Vec<f64> = cuts.iter().map(|c| c.0).collect();
        let (pieces, pts) = split_at(sk, ci, &ts)?;
        // The piece containing tq.
        let k = if full {
            let (a, b) = (ts[0], ts[1]);
            if tq > a && tq < b { 0 } else { 1 }
        } else {
            ts.iter().filter(|t| **t < tq).count()
        };
        for (pi, c) in pts.iter().zip(&cuts) {
            pin(sk, *pi, c.2)?;
        }
        let gone = pieces.get(k).copied().ok_or_else(|| bad(cmd, "piece"))?;
        let keep: Vec<usize> = pieces.iter().copied().filter(|x| *x != gone).collect();
        // The original curve keeps its id (and constraints): swap geometry into it if needed.
        let gone = match keep.first() {
            Some(&other) if gone == ci => {
                let ka = sk.curves.get(ci).map(|c| c.kind.clone());
                let kb = sk.curves.get(other).map(|c| c.kind.clone());
                if let (Some(ka), Some(kb)) = (ka, kb) {
                    if let Some(c) = sk.curves.get_mut(ci) {
                        c.kind = kb;
                    }
                    if let Some(c) = sk.curves.get_mut(other) {
                        c.kind = ka;
                    }
                }
                other
            }
            _ => gone,
        };
        let keep: Vec<usize> = pieces.iter().copied().filter(|x| *x != gone).collect();
        tie(sk, &keep)?;
        sk.remove_curves(&[gone]);
        doc.prune_model_params();
        Ok(json!({"trimmed": id}))
    })?;
    Ok(json!({"result": out, "sketch": info}))
}

fn break_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.break";
    let points = if p.get("points").is_some() { Some(vec2_list(cmd, p, "points", 1000)?) } else { None };
    let (out, info) = edit(s, p, cmd, false, |sk, doc| {
        let ci = curve_arg(sk, p, cmd)?;
        if sk.curves.get(ci).is_some_and(|c| c.kind.is_freeform()) {
            let r = if let Some(points) = &points {
                super::sketch_freeform::break_at_points(sk, ci, points, cmd)?
            } else {
                let at = req_vec2(cmd, p, "at")?;
                super::sketch_freeform::trim_or_break(sk, ci, at, false, cmd)?
            };
            doc.prune_model_params();
            return Ok(r["curves"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default());
        }
        let sh = sk.shape(ci).ok_or_else(|| bad(cmd, "curve"))?;
        let (ts, cutters): (Vec<f64>, Vec<Option<usize>>) = if let Some(list) = &points {
            let mut v: Vec<f64> = list.iter().map(|q| sh.param(sh.project(*q))).collect();
            v.sort_by(|a, b| a.total_cmp(b));
            v.dedup_by(|a, b| (*a - *b).abs() <= 1e-9);
            let n = v.len();
            (v, vec![None; n])
        } else {
            let at = req_vec2(cmd, p, "at")?;
            let (cuts, _) = bracket(sk, ci, sh.param(sh.project(at)));
            (cuts.iter().map(|c| c.0).collect(), cuts.iter().map(|c| Some(c.2)).collect())
        };
        let (t0, t1) = sh.range();
        if ts.is_empty() || (!sh.is_full() && ts.iter().any(|t| *t <= t0 || *t >= t1)) {
            return Err(bad(cmd, "nothing to break at there"));
        }
        let (pieces, pts) = split_at(sk, ci, &ts)?;
        for (pi, c) in pts.iter().zip(&cutters) {
            if let Some(c) = c {
                pin(sk, *pi, *c)?;
            }
        }
        tie(sk, &pieces)?;
        Ok(ids_of(sk, &pieces))
    })?;
    Ok(json!({"curves": out, "sketch": info}))
}

fn extend(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.extend";
    let at = req_vec2(cmd, p, "at")?;
    let (out, info) = edit(s, p, cmd, true, |sk, doc| {
        let rep = solve_current(sk, doc)?;
        if !rep.keeps_orientation() {
            return Err(bad(cmd, rep.failure(|_| "solve the existing sketch constraints before extending".into())));
        }
        let ci = curve_arg(sk, p, cmd)?;
        let sh = sk.shape(ci).ok_or_else(|| bad(cmd, "curve"))?;
        let (pa, pb) = match sk.curves.get(ci).map(|c| c.kind.clone()) {
            Some(CurveKind::Line { a, b }) | Some(CurveKind::Arc { a, b, .. }) => (a, b),
            _ => return Err(bad(cmd, "only lines and arcs can be extended")),
        };
        let (qa, qb) = (sk.point(pa).unwrap_or_default(), sk.point(pb).unwrap_or_default());
        let at_b = at.dist(qb) <= at.dist(qa);
        let end = if at_b { pb } else { pa };
        let shared = sk.curves.iter().enumerate().filter(|(i, c)| *i != ci && uses(c, end)).count();
        if shared > 0 || sk.points.get(end).is_some_and(|q| q.fixed) {
            return Err(bad(cmd, "that end is connected to other geometry"));
        }
        // Candidate hits on the unbounded curve beyond the chosen end.
        let unbounded = match sh {
            Shape::Line { .. } => sh,
            Shape::Round { c, r, start, .. } => Shape::Round { c, r, start, sweep: TAU },
        };
        let (_, t1) = sh.range();
        let mut best: Option<(f64, Vec2, usize)> = None;
        let mut ambiguous = false;
        let mut budget = 1_000_000usize;
        let mut segment_count = 0usize;
        for (oi, _) in sk.curves.iter().enumerate() {
            if oi == ci {
                continue;
            }
            let mut pieces = sk.exact_segs(oi).unwrap_or_else(|| sk.segs(oi));
            // An infinite line (#1359) is a boundary along its whole line.
            if sk.curves.get(oi).is_some_and(|c| c.infinite) {
                pieces = pieces.iter().map(|s| support::endless(s, sh.point(sh.param(at)))).collect();
            }
            segment_count += pieces.len();
            if segment_count > 16_384 {
                return Err(bad(cmd, "too many exact curve pieces to extend safely"));
            }
            for piece in &pieces {
                for q in support::hits(&unbounded, piece, &mut budget)? {
                    let t = sh.param(q);
                    let gap = match sh {
                        Shape::Line { a, b } => {
                            let l = a.dist(b);
                            if at_b { (t - 1.0) * l } else { -t * l }
                        }
                        Shape::Round { r, .. } => {
                            // A hit already inside the arc cannot lengthen either end. In
                            // particular, extending the start must not turn a quarter into an eighth.
                            if t <= t1 + TOL / r.max(1e-12) || t >= TAU - TOL / r.max(1e-12) {
                                continue;
                            }
                            if at_b { (t - t1) * r } else { (TAU - t) * r }
                        }
                    };
                    if gap > TOL * 10.0 {
                        if best.is_none_or(|b| gap < b.0 - TOL) {
                            best = Some((gap, q, oi));
                            ambiguous = false;
                        } else if best.is_some_and(|b| (gap - b.0).abs() <= TOL && (b.2 != oi || b.1.dist(q) > TOL)) {
                            ambiguous = true;
                        }
                    }
                }
            }
        }
        if ambiguous {
            return Err(bad(cmd, "multiple curves stop the extension at the same distance"));
        }
        let (_, q, oi) = best.ok_or_else(|| bad(cmd, "nothing to extend to"))?;
        if let Some(pt) = sk.points.get_mut(end) {
            pt.pos = q;
        }
        pin(sk, end, oi)?;
        let before = sk.points.iter().map(|p| p.pos).collect::<Vec<_>>();
        let hold = solidcraft_sketch::Hold { points: (0..sk.points.len()).filter(|i| *i != end).collect(), radii: (0..sk.curves.len()).collect() };
        if !solidcraft_sketch::solve_with(sk, &hold, None, solidcraft_sketch::Analysis::Full, None).ok()
            || sk.point(end).is_none_or(|point| point.dist(q) > TOL)
            || sk.points.iter().enumerate().any(|(i, point)| i != end && point.pos.dist(before[i]) > TOL)
        {
            return Err(bad(cmd, "the extension conflicts with constraints or cannot preserve its original support"));
        }
        Ok(sk.points.get(end).map(|x| x.id.clone()).unwrap_or_default())
    })?;
    Ok(json!({"moved": out, "sketch": info}))
}

fn uses(c: &Curve, p: usize) -> bool {
    c.kind.uses(p)
}

// ---------------------------------------------------------------------------------------------
// Corners: fillet and chamfer

/// A corner drawn as two curves whose ends are separate points held together by a Coincident
/// constraint (a line drawn to where an arc starts, #1208) becomes one shared point, so it can be
/// rounded or cut like a corner drawn through one point. Only a partner held by nothing but that
/// constraint is welded; anything else is left as it is.
fn weld_corner(sk: &mut Sketch, p: &Value) {
    let ends = |sk: &Sketch, c: usize| match sk.curves.get(c).map(|c| &c.kind) {
        Some(CurveKind::Line { a, b }) | Some(CurveKind::Arc { a, b, .. }) => Some([*a, *b]),
        _ => None,
    };
    let q = if let Some(r) = str_(p, "point") {
        sk.resolve_point(r)
    } else {
        let curve = |k: &str| str_(p, k).and_then(|i| sk.curve_index(i)).and_then(|c| ends(sk, c));
        let (Some(ea), Some(eb)) = (curve("a"), curve("b")) else { return };
        if ea.iter().any(|x| eb.contains(x)) {
            return;
        }
        // The end of `a` held coincident with an end of `b`.
        ea.into_iter().find(|x| {
            sk.constraints
                .iter()
                .any(|k| matches!(k.kind, ConstraintKind::Coincident { p, q } if (p == *x && eb.contains(&q)) || (q == *x && eb.contains(&p))))
        })
    };
    let Some(mut q) = q else { return };
    // Each pass welds one partner; a pass removes a constraint, so this ends.
    for _ in 0..sk.constraints.len() {
        let partner = sk.constraints.iter().enumerate().find_map(|(i, k)| {
            let o = match k.kind {
                ConstraintKind::Coincident { p, q: r } if p == q && r != q => r,
                ConstraintKind::Coincident { p, q: r } if r == q && p != q => p,
                _ => return None,
            };
            let held_elsewhere = sk.constraints.iter().enumerate().any(|(j, k)| j != i && k.kind.refers_to_point(o));
            let linked = (0..sk.curves.len()).any(|c| sk.is_linked_curve(c) && sk.curves.get(c).is_some_and(|x| x.kind.uses(o)));
            (o != 0 && !held_elsewhere && !linked).then_some((i, o))
        });
        let Some((i, o)) = partner else { return };
        for c in sk.curves.iter_mut() {
            c.kind.map_points(&|x| if x == o { q } else { x });
        }
        sk.constraints.remove(i);
        sk.remove_points(&[o]);
        if o < q {
            q -= 1;
        }
    }
}

/// Fillets and chamfers refuse a corner on an infinite line (#1359): it has no end there.
fn refuse_infinite_corner(sk: &Sketch, p: &Value, cmd: &str) -> Result<()> {
    let infinite = |c: Option<usize>| c.and_then(|c| sk.curves.get(c)).is_some_and(|c| c.infinite);
    let at_corner = match str_(p, "point").and_then(|r| sk.resolve_point(r)) {
        Some(q) => sk.curves.iter().any(|c| c.infinite && c.kind.uses(q)),
        None => ["a", "b"].iter().any(|k| infinite(str_(p, k).and_then(|i| sk.curve_index(i)))),
    };
    if at_corner { Err(bad(cmd, "an infinite line has no corner to round or cut: make it a normal line first")) } else { Ok(()) }
}

/// A corner of two lines: (line a, line b, corner point, far end of a, far end of b).
fn corner(sk: &Sketch, p: &Value, cmd: &str) -> Result<(usize, usize, usize, usize, usize)> {
    let ends = |l: usize| match sk.curves.get(l).map(|c| &c.kind) {
        Some(CurveKind::Line { a, b }) => Some((*a, *b)),
        _ => None,
    };
    let (la, lb) = if let Some(r) = str_(p, "point") {
        let q = sk.resolve_point(r).ok_or_else(|| bad(cmd, format!("unknown point `{r}`")))?;
        let lines: Vec<usize> = (0..sk.curves.len()).filter(|i| ends(*i).is_some_and(|(a, b)| a == q || b == q) && !sk.is_linked_curve(*i)).collect();
        // A construction line ending there too (a diagonal that centres something) is not one of
        // the corner's sides: it keeps its end on the virtual sharp corner (#1026).
        let sides: Vec<usize> =
            if lines.len() == 2 { lines } else { lines.into_iter().filter(|i| sk.curves.get(*i).is_some_and(|c| !c.construction)).collect() };
        match sides[..] {
            [a, b] => (a, b),
            _ => return Err(bad(cmd, "the corner must join exactly two lines (or give the two lines as `a` and `b`)")),
        }
    } else {
        let a = str_(p, "a").and_then(|i| sk.curve_index(i)).ok_or_else(|| bad(cmd, "`a` must be a line"))?;
        let b = str_(p, "b").and_then(|i| sk.curve_index(i)).ok_or_else(|| bad(cmd, "`b` must be a line"))?;
        (a, b)
    };
    let (a0, a1) = ends(la).ok_or_else(|| bad(cmd, "fillets and chamfers work on two lines"))?;
    let (b0, b1) = ends(lb).ok_or_else(|| bad(cmd, "fillets and chamfers work on two lines"))?;
    let shared = [a0, a1].into_iter().find(|x| *x == b0 || *x == b1).ok_or_else(|| bad(cmd, "the lines must share a corner point"))?;
    let fa = if a0 == shared { a1 } else { a0 };
    let fb = if b0 == shared { b1 } else { b0 };
    Ok((la, lb, shared, fa, fb))
}

/// Replace the corner end of line `l` with point `np`.
fn reattach(sk: &mut Sketch, l: usize, corner: usize, np: usize) {
    if let Some(Curve { kind: CurveKind::Line { a, b }, .. }) = sk.curves.get_mut(l) {
        if *a == corner {
            *a = np;
        } else if *b == corner {
            *b = np;
        }
    }
}

/// Equal lengths and midpoints describe the original side, not its shortened tangent
/// segment. Keep that side as construction geometry sharing the original corner points.
/// Later corner cuts retain those points through `keep_corner`, so this also works when
/// the other end is rounded in a separate command.
fn keep_side_relations(sk: &mut Sketch, line: usize) -> Result<Vec<String>> {
    let affected = |k: &ConstraintKind| {
        matches!(*k, ConstraintKind::Equal { a, b } if a == line || b == line) || matches!(*k, ConstraintKind::Midpoint { l, .. } if l == line)
    };
    if !sk.constraints.iter().any(|k| affected(&k.kind)) {
        return Ok(Vec::new());
    }
    let Some(CurveKind::Line { a, b }) = sk.curves.get(line).map(|c| c.kind.clone()) else { return Ok(Vec::new()) };
    let support = sk.add_line_pts(a, b, None)?;
    if let Some(c) = sk.curves.get_mut(support) {
        c.construction = true;
    }
    let mut moved = Vec::new();
    for k in &mut sk.constraints {
        if !affected(&k.kind) {
            continue;
        }
        match &mut k.kind {
            ConstraintKind::Equal { a, b } => {
                if *a == line {
                    *a = support;
                }
                if *b == line {
                    *b = support;
                }
            }
            ConstraintKind::Midpoint { l, .. } => *l = support,
            _ => {}
        }
        moved.push(k.id.clone());
    }
    Ok(moved)
}

/// What became of a rounded or cut corner's point: kept as a virtual sharp corner (its id),
/// and the constraints and dimensions that now measure to it.
#[derive(Default)]
struct CornerKept {
    virtual_corner: Option<String>,
    moved: Vec<String>,
}

/// Keep corner point `c` of curves `ca`, `cb` (whose corner ends were just moved to `na`, `nb`)
/// when anything measures to it: a constraint or dimension on the point, the length of either
/// line, or it is the origin or still ends another curve. It stays as a virtual sharp corner,
/// held on both curves' extensions (their intersection), and the line lengths become distances
/// from the far end to it, so the part keeps its size, as in Fusion. Otherwise None: the
/// caller removes the point.
fn keep_corner(sk: &mut Sketch, c: usize, [(ca, na), (cb, nb)]: [(usize, usize); 2]) -> Result<Option<CornerKept>> {
    let used = sk.curves.iter().any(|x| uses(x, c));
    let mut moved: Vec<String> = sk.constraints.iter().filter(|k| k.kind.refers_to_point(c)).map(|k| k.id.clone()).collect();
    for k in sk.constraints.iter_mut() {
        let ConstraintKind::Length { l, value } = k.kind else { continue };
        let new_end = if l == ca {
            na
        } else if l == cb {
            nb
        } else {
            continue;
        };
        let Some(CurveKind::Line { a, b }) = sk.curves.get(l).map(|x| &x.kind) else { continue };
        // Same direction as the line, so the dimension keeps its side and text place.
        k.kind = if *a == new_end { ConstraintKind::Distance { p: c, q: *b, value } } else { ConstraintKind::Distance { p: *a, q: c, value } };
        moved.push(k.id.clone());
    }
    if moved.is_empty() && !used && c != 0 {
        return Ok(None);
    }
    for cu in [ca, cb] {
        let k = ConstraintKind::PointOnCurve { p: c, c: cu };
        if !sk.constraints.iter().any(|x| x.kind == k) {
            add_c(sk, k)?;
        }
    }
    // A point of its own now: `l1.end` names the line's new end, not the virtual corner.
    let virtual_corner = if used || c == 0 {
        None
    } else {
        let id = sk.fresh("corner")?;
        if let Some(pt) = sk.points.get_mut(c) {
            pt.id = id.clone();
        }
        Some(id)
    };
    Ok(Some(CornerKept { virtual_corner, moved }))
}

/// Points at distances `da` along a and `db` along b from the corner; lines shortened to them.
fn cut_corner(
    sk: &mut Sketch,
    la: usize,
    lb: usize,
    c: usize,
    fa: usize,
    fb: usize,
    da: f64,
    db: f64,
    cmd: &str,
) -> Result<(usize, usize, Vec2, CornerKept)> {
    let (pc, pa, pb) = (sk.point(c).unwrap_or_default(), sk.point(fa).unwrap_or_default(), sk.point(fb).unwrap_or_default());
    if !(da > 1e-9 && db > 1e-9 && da < pc.dist(pa) - 1e-9 && db < pc.dist(pb) - 1e-9) {
        return Err(bad(cmd, "too large for these lines"));
    }
    let u = (pa - pc).normalized().ok_or_else(|| bad(cmd, "degenerate line"))?;
    let v = (pb - pc).normalized().ok_or_else(|| bad(cmd, "degenerate line"))?;
    let ta = sk.add_point(pc + u * da, None)?;
    let tb = sk.add_point(pc + v * db, None)?;
    let mut moved = keep_side_relations(sk, la)?;
    moved.extend(keep_side_relations(sk, lb)?);
    reattach(sk, la, c, ta);
    reattach(sk, lb, c, tb);
    let mut kept = keep_corner(sk, c, [(la, ta), (lb, tb)])?;
    if let Some(k) = &mut kept {
        k.moved.extend(moved);
    }
    let removed = kept.is_none();
    if removed {
        sk.remove_points(&[c]);
    }
    // Indices shift when the corner point was removed.
    let fix = |i: usize| if removed && i > c { i - 1 } else { i };
    Ok((fix(ta), fix(tb), pc, kept.unwrap_or_default()))
}

fn fillet(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.fillet";
    let r_expr = expr(p, "radius").ok_or_else(|| bad(cmd, "`radius` must be a number or an expression"))?;
    // Several corners at once (`points`), each like a single `point`.
    let corners: Vec<Value> = match p.get("points").and_then(Value::as_array) {
        Some(a) if !a.is_empty() && a.len() <= 500 => a.iter().map(|q| json!({ "point": q })).collect(),
        Some(_) => return Err(bad(cmd, "`points` must list 1…500 corners")),
        None => vec![p.clone()],
    };
    let (out, info) = edit(s, p, cmd, false, |sk, doc| {
        let r = doc.eval(&r_expr, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("radius: {e}")))?;
        if !(r > 1e-9 && r < 1e8) {
            return Err(bad(cmd, "radius must be positive"));
        }
        let pname = doc.new_model_param(&r_expr, "mm");
        let (mut arcs, mut cons, mut kept) = (Vec::new(), Vec::new(), Vec::new());
        for q in &corners {
            let (arc, mut c, k) = fillet_corner(sk, q, r, cmd)?;
            kept.push(k);
            // One radius for them all: the first carries the dimension, the rest are equal to it.
            match arcs.first() {
                None => c.push(sk.add_constraint(ConstraintKind::Radius { c: arc, value: r }, Some(pname.clone()))?),
                Some(first) => c.push(add_c(sk, ConstraintKind::Equal { a: *first, b: arc })?),
            }
            cons.extend(c);
            arcs.push(arc);
        }
        Ok((ids_of(sk, &arcs), cons, pname, kept_json(kept)))
    })?;
    let (vc, moved) = out.3;
    Ok(json!({"curves": out.0, "constraints": out.1, "param": out.2, "virtual_corners": vc, "moved_to_virtual_corner": moved, "sketch": info}))
}

/// The virtual corners kept and the constraints moved to them, for a response.
fn kept_json(kept: Vec<CornerKept>) -> (Vec<String>, Vec<String>) {
    let vcs = kept.iter().filter_map(|k| k.virtual_corner.clone()).collect();
    let mut moved: Vec<String> = Vec::new();
    for id in kept.into_iter().flat_map(|k| k.moved) {
        if !moved.contains(&id) {
            moved.push(id);
        }
    }
    (vcs, moved)
}

/// Round one corner (`point` or `a`, `b`) with an arc of radius `r` tangent to both curves:
/// the arc, the tangent constraints and what became of the corner point.
fn fillet_corner(sk: &mut Sketch, p: &Value, r: f64, cmd: &str) -> Result<(usize, Vec<String>, CornerKept)> {
    refuse_infinite_corner(sk, p, cmd)?;
    weld_corner(sk, p);
    let (la, lb, c, fa, fb) = match corner(sk, p, cmd) {
        Ok(x) => x,
        // Not two lines: a corner with an arc.
        Err(e) => return round_corner(sk, p, r, cmd)?.ok_or(e),
    };
    let (pc, pa, pb) = (sk.point(c).unwrap_or_default(), sk.point(fa).unwrap_or_default(), sk.point(fb).unwrap_or_default());
    let u = (pa - pc).normalized().ok_or_else(|| bad(cmd, "degenerate line"))?;
    let v = (pb - pc).normalized().ok_or_else(|| bad(cmd, "degenerate line"))?;
    let half = 0.5 * u.cross(v).atan2(u.dot(v)).abs();
    if !(1e-6..=std::f64::consts::FRAC_PI_2 - 1e-9).contains(&half) {
        return Err(bad(cmd, "the lines are parallel"));
    }
    let d = r / half.tan();
    if d >= pa.dist(pc) - 1e-9 || d >= pb.dist(pc) - 1e-9 {
        return Err(bad(cmd, format!("the radius is too large for that corner (it needs {d:.3} mm of each line)")));
    }
    let bis = (u + v).normalized().ok_or_else(|| bad(cmd, "the lines are parallel"))?;
    let center = pc + bis * (r / half.sin());
    let (ta, tb, _, kept) = cut_corner(sk, la, lb, c, fa, fb, d, d, cmd)?;
    let (qa, qb) = (sk.point(ta).unwrap_or_default(), sk.point(tb).unwrap_or_default());
    let ccw = (qa - center).cross(qb - center) > 0.0;
    let arc = if ccw {
        sk.add_arc(center, qa, qb, [None, Some(ta), Some(tb)], None)?
    } else {
        sk.add_arc(center, qb, qa, [None, Some(tb), Some(ta)], None)?
    };
    let cons = vec![add_c(sk, ConstraintKind::Tangent { a: la, b: arc })?, add_c(sk, ConstraintKind::Tangent { a: lb, b: arc })?];
    Ok((arc, cons, kept))
}

/// Fillet a corner where a line meets an arc, or two arcs meet: the arc of radius `r` touching
/// both, nearest the corner; both curves are cut back to it. None when the corner is not of
/// that kind.
fn round_corner(sk: &mut Sketch, p: &Value, r: f64, cmd: &str) -> Result<Option<(usize, Vec<String>, CornerKept)>> {
    let ends = |c: usize| match sk.curves.get(c).map(|c| &c.kind) {
        Some(CurveKind::Line { a, b }) | Some(CurveKind::Arc { a, b, .. }) => Some((*a, *b)),
        _ => None,
    };
    let (ca, cb, q) = if let Some(rf) = str_(p, "point") {
        let Some(q) = sk.resolve_point(rf) else { return Ok(None) };
        let cs: Vec<usize> = (0..sk.curves.len()).filter(|i| ends(*i).is_some_and(|(a, b)| a == q || b == q) && !sk.is_linked_curve(*i)).collect();
        match cs[..] {
            [a, b] => (a, b, q),
            _ => return Ok(None),
        }
    } else {
        let (Some(a), Some(b)) = (str_(p, "a").and_then(|i| sk.curve_index(i)), str_(p, "b").and_then(|i| sk.curve_index(i))) else {
            return Ok(None);
        };
        let (Some((a0, a1)), Some((b0, b1))) = (ends(a), ends(b)) else { return Ok(None) };
        let Some(q) = [a0, a1].into_iter().find(|x| *x == b0 || *x == b1) else { return Ok(None) };
        (a, b, q)
    };
    let (Some(sa), Some(sb), Some(pc)) = (sk.shape(ca), sk.shape(cb), sk.point(q)) else { return Ok(None) };
    let offsets = |sh: &Shape| -> Vec<Shape> {
        match *sh {
            Shape::Line { a, b } => match (b - a).normalized() {
                Some(d) => [1.0, -1.0].iter().map(|s| Shape::Line { a: a + d.perp() * (r * s), b: b + d.perp() * (r * s) }).collect(),
                None => Vec::new(),
            },
            Shape::Round { c, r: rr, .. } => {
                let mut v = vec![Shape::Round { c, r: rr + r, start: 0.0, sweep: TAU }];
                if rr - r > 1e-9 {
                    v.push(Shape::Round { c, r: rr - r, start: 0.0, sweep: TAU });
                }
                v
            }
        }
    };
    let mut best: Option<(f64, Vec2, Vec2, Vec2)> = None;
    for oa in offsets(&sa) {
        for ob in offsets(&sb) {
            for cen in intersections(&oa, &ob) {
                let (ta, tb) = (sa.project(cen), sb.project(cen));
                if on_bounded(&sa, ta).is_none() || on_bounded(&sb, tb).is_none() || ta.dist(pc) < 1e-9 || tb.dist(pc) < 1e-9 {
                    continue;
                }
                let score = ta.dist(pc) + tb.dist(pc);
                if best.is_none_or(|b| score < b.0) {
                    best = Some((score, cen, ta, tb));
                }
            }
        }
    }
    let (_, cen, ta, tb) = best.ok_or_else(|| bad(cmd, "that radius does not fit the corner"))?;
    let (na, nb) = (sk.add_point(ta, None)?, sk.add_point(tb, None)?);
    for (c, n) in [(ca, na), (cb, nb)] {
        if let Some(cu) = sk.curves.get_mut(c) {
            cu.kind.map_points(&|i| if i == q { n } else { i });
        }
    }
    let ccw = (ta - cen).cross(tb - cen) > 0.0;
    let arc =
        if ccw { sk.add_arc(cen, ta, tb, [None, Some(na), Some(nb)], None)? } else { sk.add_arc(cen, tb, ta, [None, Some(nb), Some(na)], None)? };
    let cons = vec![add_c(sk, ConstraintKind::Tangent { a: ca, b: arc })?, add_c(sk, ConstraintKind::Tangent { a: cb, b: arc })?];
    match keep_corner(sk, q, [(ca, na), (cb, nb)])? {
        Some(kept) => Ok(Some((arc, cons, kept))),
        None => {
            let arc_id = sk.curves.get(arc).map(|c| c.id.clone()).unwrap_or_default();
            sk.remove_points(&[q]);
            let arc = sk.curve_index(&arc_id).unwrap_or(arc);
            Ok(Some((arc, cons, CornerKept::default())))
        }
    }
}

fn chamfer(s: &mut Session, p: &Value, cmd: &str, dists: impl FnOnce(f64) -> Result<(f64, f64)>) -> Result<Value> {
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        refuse_infinite_corner(sk, p, cmd)?;
        weld_corner(sk, p);
        let (la, lb, c, fa, fb) = corner(sk, p, cmd)?;
        let (pc, pa, pb) = (sk.point(c).unwrap_or_default(), sk.point(fa).unwrap_or_default(), sk.point(fb).unwrap_or_default());
        let (u, v) = ((pa - pc).normalized().unwrap_or(Vec2::X), (pb - pc).normalized().unwrap_or(Vec2::Y));
        let corner_angle = u.cross(v).atan2(u.dot(v)).abs();
        let (da, db) = dists(corner_angle)?;
        let (ta, tb, _, kept) = cut_corner(sk, la, lb, c, fa, fb, da, db, cmd)?;
        let l = sk.add_line_pts(ta, tb, None)?;
        Ok((ids_of(sk, &[l]), kept_json(vec![kept])))
    })?;
    let (curves, (vc, moved)) = out;
    Ok(json!({"curves": curves, "virtual_corners": vc, "moved_to_virtual_corner": moved, "sketch": info}))
}

fn chamfer_equal(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.chamfer.equal_distance";
    let d = num(p, "distance").ok_or_else(|| bad(cmd, "`distance` must be a number"))?;
    chamfer(s, p, cmd, |_| Ok((d, d)))
}

fn chamfer_two(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.chamfer.two_distance";
    let d = num(p, "distance").ok_or_else(|| bad(cmd, "`distance` must be a number"))?;
    let d2 = num(p, "distance2").ok_or_else(|| bad(cmd, "`distance2` must be a number"))?;
    chamfer(s, p, cmd, |_| Ok((d, d2)))
}

fn chamfer_dist_angle(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.chamfer.distance_angle";
    let d = num(p, "distance").ok_or_else(|| bad(cmd, "`distance` must be a number"))?;
    let a = num(p, "angle").ok_or_else(|| bad(cmd, "`angle` must be a number (degrees)"))?.to_radians();
    chamfer(s, p, cmd, |phi| {
        // Triangle corner–Ta–Tb: angle phi at the corner, `a` at Ta.
        let opp = std::f64::consts::PI - a - phi;
        if !(a > 1e-6 && opp > 1e-6) {
            return Err(bad(cmd, "that angle does not meet the other line"));
        }
        Ok((d, d * a.sin() / opp.sin()))
    })
}

// ---------------------------------------------------------------------------------------------
// Offset

/// Curves of the connected chain through `start` (joined where exactly two curves meet), in
/// order, with whether each is walked from its stored start (`a`) to its end (`b`).
fn chain(sk: &Sketch, start: usize, pool: &[usize]) -> (Vec<(usize, bool)>, bool) {
    let ends = |c: usize| sk.curves.get(c).and_then(|c| c.kind.ends());
    let Some((s0, s1)) = ends(start) else { return (vec![(start, true)], true) };
    let next = |from: usize, at: usize, used: &[usize]| -> Option<usize> {
        let users: Vec<usize> = pool.iter().copied().filter(|c| ends(*c).is_some_and(|(a, b)| a == at || b == at)).collect();
        if users.len() != 2 {
            return None;
        }
        users.into_iter().find(|c| *c != from && !used.contains(c))
    };
    let mut fwd = vec![(start, true)];
    let mut used = vec![start];
    let (mut cur, mut at) = (start, s1);
    let mut closed = false;
    while let Some(n) = next(cur, at, &used) {
        let Some((a, b)) = ends(n) else { break };
        let forward = a == at;
        fwd.push((n, forward));
        used.push(n);
        cur = n;
        at = if forward { b } else { a };
        if used.len() > 10_000 {
            break;
        }
    }
    if at == s0 && fwd.len() > 1 {
        closed = true;
    } else {
        // Walk backwards from the start.
        let (mut cur, mut at) = (start, s0);
        let mut back = Vec::new();
        while let Some(n) = next(cur, at, &used) {
            let Some((a, b)) = ends(n) else { break };
            let forward = b == at;
            back.push((n, forward));
            used.push(n);
            cur = n;
            at = if forward { a } else { b };
            if used.len() > 10_000 {
                break;
            }
        }
        back.reverse();
        back.extend(fwd);
        fwd = back;
    }
    (fwd, closed)
}

/// An offset piece: shape walked in chain direction, offset by `d` to the left.
#[derive(Clone)]
enum Piece {
    Line(Vec2, Vec2),
    /// Centre point index, centre, radius, start point, end point (walk direction), ccw walk.
    Arc(usize, Vec2, f64, Vec2, Vec2, bool),
    /// A free-form curve's offset, sampled in walk direction (made into a fit spline).
    Free(Vec<Vec2>),
}

fn offset_piece(sk: &Sketch, c: usize, forward: bool, d: f64) -> Option<Piece> {
    match sk.curves.get(c)?.kind {
        CurveKind::Line { a, b } => {
            let (mut p, mut q) = (sk.point(a)?, sk.point(b)?);
            if !forward {
                std::mem::swap(&mut p, &mut q);
            }
            let n = (q - p).normalized()?.perp();
            Some(Piece::Line(p + n * d, q + n * d))
        }
        CurveKind::Arc { c: ci, a, b } => {
            let (cc, pa, pb) = (sk.point(ci)?, sk.point(a)?, sk.point(b)?);
            let r = cc.dist(pa);
            // Walking counter-clockwise, left is toward the centre.
            let nr = if forward { r - d } else { r + d };
            if nr <= 1e-9 {
                return None;
            }
            let at = |q: Vec2| cc + (q - cc).normalized().unwrap_or(Vec2::X) * nr;
            let (s, e) = if forward { (at(pa), at(pb)) } else { (at(pb), at(pa)) };
            Some(Piece::Arc(ci, cc, nr, s, e, forward))
        }
        CurveKind::Circle { .. } => None,
        // Free-form: offset the curve's samples along their normals.
        _ => {
            let mut poly = sk.polyline(c);
            if !forward {
                poly.reverse();
            }
            let n = poly.len();
            let pts: Vec<Vec2> = (0..n)
                .filter_map(|i| {
                    let (a, b) = (poly.get(i.saturating_sub(1))?, poly.get((i + 1).min(n - 1))?);
                    let t = (*b - *a).normalized()?;
                    Some(*poly.get(i)? + t.perp() * d)
                })
                .collect();
            (pts.len() >= 2).then_some(Piece::Free(pts))
        }
    }
}

fn piece_shape(p: &Piece) -> Shape {
    match p {
        Piece::Line(a, b) => Shape::Line { a: *a, b: *b },
        Piece::Arc(_, c, r, ..) => Shape::Round { c: *c, r: *r, start: 0.0, sweep: TAU },
        Piece::Free(v) => Shape::Line { a: v.first().copied().unwrap_or_default(), b: v.last().copied().unwrap_or_default() },
    }
}

/// Distance from `q` to an offset piece.
fn piece_dist(p: &Piece, q: Vec2) -> f64 {
    match p {
        Piece::Free(v) => v
            .windows(2)
            .map(|w| {
                let (a, b) = (w[0], w[1]);
                let t = if (b - a).len2() > 0.0 { ((q - a).dot(b - a) / (b - a).len2()).clamp(0.0, 1.0) } else { 0.0 };
                q.dist(a + (b - a) * t)
            })
            .fold(f64::INFINITY, f64::min),
        _ => piece_shape(p).dist(q),
    }
}

/// One side of an offset. The curves created at the signed distance, the parallel constraints
/// added with them, and the association when the boundary carries an offset topology.
struct Side {
    distance: f64,
    curves: Vec<String>,
    constraints: Vec<String>,
    association: Option<String>,
    param: Option<String>,
}

/// The signed distances an offset runs at. One side is the distance as given. `symmetric` is the
/// same distance each way. `distance2` is an
/// independent distance the other way, as `distance2` means for `solid.extrude`. The primary side
/// comes first, so a one-sided caller reads the same first entry it always did.
fn offset_runs(d: f64, symmetric: bool, distance2: Option<f64>) -> Vec<f64> {
    match (symmetric, distance2) {
        (true, _) => vec![d.abs(), -d.abs()],
        (_, Some(second)) => vec![d, -second * d.signum()],
        _ => vec![d],
    }
}

/// Build one side of a chain offset at the signed distance `d`. `order` and `sources` are the
/// walked chain, computed once before anything is created so a second side walks the same
/// curves rather than a pool the first side has already grown.
fn offset_side(sk: &mut Sketch, cmd: &str, order: &[(usize, bool)], sources: &[(String, bool)], closed: bool, convex: bool, d: f64) -> Result<Side> {
    let mut made = Vec::new();
    let mut cons = Vec::new();
    if convex {
        let created = sk.add_convex_offset(sources.to_vec(), d)?;
        let roles = sk
            .offsets
            .iter()
            .filter_map(|offset| offset.topology.as_ref())
            .find(|topology| topology.id == created.association)
            .map(|topology| topology.roles.clone())
            .ok_or_else(|| bad(cmd, "new offset association is missing"))?;
        for (source, role) in sources.iter().zip(&roles) {
            if role.kind == solidcraft_sketch::OffsetRoleKind::Line {
                let a = sk.curve_index(&source.0).ok_or_else(|| bad(cmd, "offset source line vanished"))?;
                let b = sk.curve_index(&role.output).ok_or_else(|| bad(cmd, "offset output line vanished"))?;
                cons.push(add_c(sk, ConstraintKind::Parallel { a, b })?);
            }
        }
        return Ok(Side { distance: d, curves: created.curves, constraints: cons, association: Some(created.association), param: None });
    }
    let pieces: Vec<Piece> =
        order.iter().map(|(c, f)| offset_piece(sk, *c, *f, d)).collect::<Option<_>>().ok_or_else(|| bad(cmd, "the offset collapses an arc"))?;
    let n = pieces.len();
    let start_of = |p: &Piece| match p {
        Piece::Line(a, _) => *a,
        Piece::Arc(_, _, _, s, _, _) => *s,
        Piece::Free(v) => v.first().copied().unwrap_or_default(),
    };
    let end_of = |p: &Piece| match p {
        Piece::Line(_, b) => *b,
        Piece::Arc(_, _, _, _, e, _) => *e,
        Piece::Free(v) => v.last().copied().unwrap_or_default(),
    };
    // Junction points between consecutive pieces.
    let joint = |x: &Piece, y: &Piece| -> Vec2 {
        let (e, s0) = (end_of(x), start_of(y));
        if e.dist(s0) < TOL * 10.0 {
            return e;
        }
        let guess = (e + s0) * 0.5;
        if matches!(x, Piece::Free(_)) || matches!(y, Piece::Free(_)) {
            return guess;
        }
        intersections(&piece_shape(x), &piece_shape(y)).into_iter().min_by(|a, b| a.dist(guess).total_cmp(&b.dist(guess))).unwrap_or(guess)
    };
    let (Some(head), Some(tail)) = (pieces.first(), pieces.last()) else {
        return Err(bad(cmd, "no curves"));
    };
    let mut joints: Vec<Vec2> = Vec::new();
    joints.push(if closed { joint(tail, head) } else { start_of(head) });
    for k in 0..n.saturating_sub(1) {
        let (Some(x), Some(y)) = (pieces.get(k), pieces.get(k + 1)) else {
            return Err(bad(cmd, "the offset chain changed while it was built"));
        };
        joints.push(joint(x, y));
    }
    if !closed {
        joints.push(end_of(tail));
    }
    // A straight piece whose joints come out reversed (or meet) was overrun by its
    // neighbours: the offset is larger than the shape allows there.
    for (k, pc) in pieces.iter().enumerate() {
        if let Piece::Line(a, b) = pc {
            let (ja, jb) = (joints.get(k).copied(), joints.get(if closed { (k + 1) % n } else { k + 1 }).copied());
            if let (Some(ja), Some(jb)) = (ja, jb)
                && (jb - ja).dot(*b - *a) <= TOL * a.dist(*b)
            {
                return Err(bad(cmd, "the offset is larger than the shape allows (the offset curves would cross)"));
            }
        }
    }
    let pts: Vec<usize> = joints.iter().map(|q| sk.add_point(*q, None)).collect::<std::result::Result<_, _>>()?;
    for (k, pc) in pieces.iter().enumerate() {
        let (Some(pa), Some(pb)) = (pts.get(k).copied(), pts.get(if closed { (k + 1) % n } else { k + 1 }).copied()) else {
            return Err(bad(cmd, "the offset chain changed while it was built"));
        };
        let orig = order.get(k).map(|(c, _)| *c).ok_or_else(|| bad(cmd, "the offset chain changed while it was built"))?;
        match pc.clone() {
            Piece::Free(v) => {
                // A fit spline through a few of the samples, ending at the joints.
                let step = (v.len() / 24).max(1);
                let mut idx = vec![pa];
                for q in v.iter().skip(step).step_by(step) {
                    if q.dist(*v.last().unwrap_or(q)) > 1e-6 {
                        idx.push(sk.add_point(*q, None)?);
                    }
                }
                idx.push(pb);
                idx.dedup();
                if idx.len() >= 2 {
                    made.push(sk.add_curve(CurveKind::Spline { pts: idx, control: false, degree: 3, periodic: false }, None)?);
                }
            }
            Piece::Line(..) => {
                let l = sk.add_line_pts(pa, pb, None)?;
                cons.push(add_c(sk, ConstraintKind::Parallel { a: orig, b: l })?);
                made.push(l);
            }
            Piece::Arc(ci, cc, _, _, _, ccw) => {
                let (qa, qb) = (sk.point(pa).unwrap_or_default(), sk.point(pb).unwrap_or_default());
                let a = if ccw {
                    sk.add_arc(cc, qa, qb, [Some(ci), Some(pa), Some(pb)], None)?
                } else {
                    sk.add_arc(cc, qb, qa, [Some(ci), Some(pb), Some(pa)], None)?
                };
                made.push(a);
            }
        }
    }
    let outputs = ids_of(sk, &made);
    if pieces.iter().all(|p| !matches!(p, Piece::Free(_))) {
        sk.offsets.push(solidcraft_sketch::OffsetChain { sources: sources.to_vec(), outputs: outputs.clone(), distance: d, closed, topology: None });
        sk.refresh_offsets()?;
    }
    Ok(Side { distance: d, curves: outputs, constraints: cons, association: None, param: None })
}

/// Numbers remain millimetres for programmatic callers; expressions use design units.
fn offset_length(s: &Session, p: &Value, key: &str, positive: bool) -> Result<(f64, String)> {
    let cmd = "sketch.offset";
    let expression = match num(p, key) {
        Some(v) => format!("{v} mm"),
        None => expr(p, key).ok_or_else(|| bad(cmd, format!("`{key}` must be a length expression")))?.into_string(),
    };
    let expression = s.doc.with_design_unit(&expression, solidcraft_doc::expr::Kind::Length);
    let value = s.doc.eval(&expression, solidcraft_doc::expr::Kind::Length)?;
    if !value.is_finite() || value.abs() <= 1e-9 || value.abs() > solidcraft_sketch::MAX_COORD || (positive && value < 0.0) {
        return Err(bad(cmd, format!("`{key}` must be a {}length within the sketch bounds", if positive { "positive " } else { "finite nonzero " })));
    }
    Ok((value, expression))
}

#[derive(Clone, Copy)]
struct OffsetOptions {
    side: Option<Vec2>,
    chain: bool,
    distance: f64,
    symmetric: bool,
    distance2: Option<f64>,
}

/// Build every side of one connected source group. The pool is frozen before any output
/// is created, so later groups cannot accidentally walk a newly created offset.
fn offset_group(sk: &mut Sketch, ids: &[String], eligible: &std::collections::HashSet<String>, options: OffsetOptions) -> Result<Vec<Side>> {
    let OffsetOptions { side, chain: use_chain, distance: d0, symmetric, distance2 } = options;
    let cmd = "sketch.offset";
    sk.refresh_offsets()?;
    let sel: Vec<usize> = ids.iter().map(|i| sk.curve_index(i).ok_or_else(|| bad(cmd, format!("unknown curve `{i}`")))).collect::<Result<_>>()?;
    let first = *sel.first().ok_or_else(|| bad(cmd, "no curves"))?;
    if sk.offsets.iter().filter_map(|offset| offset.topology.as_ref()).any(|topology| {
        topology.roles.iter().any(|role| role.retired) && topology.roles.iter().any(|role| !role.retired && ids.contains(&role.output))
    }) {
        return Err(bad(
            cmd,
            "offsets of a boundary with retired corner arcs are not supported yet; break its offset association to use the current fixed topology",
        ));
    }
    let mut sides: Vec<Side> = Vec::new();
    // Circles offset on their own, concentrically. Both sides are then a smaller circle and
    // a larger one; the one toward the centre still has to have a size.
    if let Some(CurveKind::Circle { c, r }) = sk.curves.get(first).map(|c| c.kind.clone()) {
        let cc = sk.point(c).unwrap_or_default();
        let source = sk.curves.get(first).map(|c| c.id.clone()).ok_or_else(|| bad(cmd, "offset source circle vanished"))?;
        let grow = match side {
            Some(q) => q.dist(cc) > r,
            None => d0 < 0.0,
        };
        let radii: Vec<f64> = match (symmetric, distance2) {
            (true, _) => vec![r - d0.abs(), r + d0.abs()],
            (_, Some(second)) if grow => vec![r + d0.abs(), r - second],
            (_, Some(second)) => vec![r - d0.abs(), r + second],
            _ if grow => vec![r + d0.abs()],
            _ => vec![r - d0.abs()],
        };
        for nr in radii {
            if nr <= 1e-9 {
                return Err(bad(cmd, "the offset circle would have no size"));
            }
            let ci = sk.add_circle(cc, nr, Some(c), None)?;
            let outputs = ids_of(sk, &[ci]);
            sk.offsets.push(solidcraft_sketch::OffsetChain {
                sources: vec![(source.clone(), true)],
                outputs: outputs.clone(),
                distance: r - nr,
                closed: true,
                topology: None,
            });
            sides.push(Side { distance: r - nr, curves: outputs, constraints: Vec::new(), association: None, param: None });
        }
        return Ok(sides);
    }
    // The chain is walked once, before anything is created: a second side has to follow the
    // same source curves, not a pool the first side has already added its output to.
    let pool: Vec<usize> = if use_chain {
        (0..sk.curves.len())
            .filter(|i| {
                sk.curves.get(*i).is_some_and(|c| eligible.contains(&c.id) && c.construction == sk.curves.get(first).is_some_and(|f| f.construction))
            })
            .collect()
    } else {
        sel.clone()
    };
    let (order, closed) = chain(sk, first, &pool);
    // Side: toward `side` (compare both offsets' distance to it), else the sign.
    let d = match side {
        Some(q) => {
            let dist = |dd: f64| {
                offset_piece(sk, first, order.iter().find(|x| x.0 == first).is_none_or(|x| x.1), dd).map(|pc| piece_dist(&pc, q)).unwrap_or(f64::MAX)
            };
            if dist(d0.abs()) <= dist(-d0.abs()) { d0.abs() } else { -d0.abs() }
        }
        None => d0,
    };
    let runs = offset_runs(d, symmetric, distance2);
    if order.iter().any(|(i, _)| sk.shape(*i).is_none()) {
        // A smooth free-form entity is offset from exact pieces. Junctions that
        // would require trimming/intersecting independent normal offsets are
        // refused rather than joined by a guessed average point.
        if order.len() != 1 {
            return Err(bad(cmd, "offset smooth free-form curves individually; mixed chain junctions are ambiguous"));
        }
        let (source, forward) = *order.first().ok_or_else(|| bad(cmd, "no curves"))?;
        for dd in runs {
            let output = sk.add_freeform_offset(source, forward, dd)?;
            sides.push(Side { distance: dd, curves: ids_of(sk, &[output]), constraints: Vec::new(), association: None, param: None });
        }
        return Ok(sides);
    }
    let sources = order
        .iter()
        .map(|(index, forward)| {
            sk.curves.get(*index).map(|curve| (curve.id.clone(), *forward)).ok_or_else(|| bad(cmd, "offset chain source vanished"))
        })
        .collect::<Result<Vec<_>>>()?;
    let convex = closed && sk.convex_offset_supported(&sources)?;
    for dd in runs {
        sides.push(offset_side(sk, cmd, &order, &sources, closed, convex, dd)?);
    }
    Ok(sides)
}

fn offset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.offset";
    let ids = string_list(p, "curves");
    if ids.is_empty() || ids.len() > 500 {
        return Err(bad(cmd, "`curves` must list 1…500 curves"));
    }
    let (value, expression) = offset_length(s, p, "distance", false)?;
    let expression = if value > 0.0 { expression } else { format!("-({expression})") };
    let d0 = if bool_(p, "flip").unwrap_or(false) { -value } else { value };
    let side = p.get("side").and_then(vec2);
    let use_chain = bool_(p, "chain").unwrap_or(true);
    // Both sides at once: `symmetric` is
    // one distance applied each way, `distance2` is an independent distance the other way.
    let symmetric = match str_(p, "direction").map(str::to_ascii_lowercase).as_deref() {
        None | Some("positive") | Some("one_side") => bool_(p, "symmetric").unwrap_or(false),
        Some("symmetric") => true,
        Some("negative" | "reverse" | "flip") => {
            return Err(bad(cmd, "an offset has no negative direction: give a negative `distance`, or a `side` to offset toward"));
        }
        Some(other) => return Err(bad(cmd, format!("unknown direction `{other}` (one_side, symmetric)"))),
    };
    let second = if p.get("distance2").is_some() { Some(offset_length(s, p, "distance2", true)?) } else { None };
    let distance2 = second.as_ref().map(|(value, _)| *value);
    let expression2 = second.map(|(_, expression)| expression);
    if symmetric && distance2.is_some() {
        return Err(bad(cmd, "give `distance2` or `symmetric`, not both"));
    }
    if symmetric && side.is_some() {
        return Err(bad(cmd, "`symmetric` offsets both sides, so `side` has no side left to pick: drop one of them"));
    }
    let ((sides, param, param2), info) = edit(s, p, cmd, true, |sk, doc| {
        sk.refresh_offsets()?;
        let rep = solve_current(sk, doc)?;
        if !rep.keeps_orientation() {
            return Err(bad(cmd, rep.failure(|_| "solve existing constraints before offsetting".into())));
        }
        sk.refresh_offsets()?;
        let eligible = sk.curves.iter().map(|c| c.id.clone()).collect::<std::collections::HashSet<_>>();
        let param = doc.new_model_param(&expression, "mm");
        let param2 = expression2.as_ref().map(|e| doc.new_model_param(e, "mm"));
        let mut pending = ids.clone();
        let mut sides = Vec::new();
        while !pending.is_empty() {
            let mut group = offset_group(sk, &pending, &eligible, OffsetOptions { side, chain: use_chain, distance: d0, symmetric, distance2 })?;
            let output = group.first().and_then(|side| side.curves.first()).ok_or_else(|| bad(cmd, "offset produced no curves"))?;
            let sources = sk
                .offsets
                .iter()
                .find(|o| o.owns_output(output))
                .map(|o| o.sources.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>())
                .or_else(|| sk.freeform_offsets.iter().find(|o| o.output == *output).map(|o| vec![o.source.clone()]))
                .ok_or_else(|| bad(cmd, "offset association is missing"))?;
            // An offset of infinite lines is infinite lines too (#1359).
            let infinite = sources.iter().all(|id| sk.curve_index(id).and_then(|c| sk.curves.get(c)).is_some_and(|c| c.infinite));
            if infinite {
                for id in group.iter().flat_map(|side| side.curves.iter()) {
                    if let Some(cu) = sk.curve_index(id).and_then(|c| sk.curves.get_mut(c))
                        && matches!(cu.kind, CurveKind::Line { .. })
                    {
                        cu.infinite = true;
                        cu.construction = true;
                    }
                }
            }
            for (index, side) in group.iter_mut().enumerate() {
                let name = if index == 1 { param2.as_ref().unwrap_or(&param) } else { &param };
                let output = side.curves.first().ok_or_else(|| bad(cmd, "offset produced no curves"))?;
                side.constraints.push(sk.add_constraint(
                    ConstraintKind::OffsetDistance { output: output.clone(), value: side.distance.abs(), reverse: side.distance < 0.0 },
                    Some(name.clone()),
                )?);
                side.param = Some(name.clone());
            }
            let before = pending.len();
            pending.retain(|id| !sources.contains(id));
            if pending.len() == before {
                return Err(bad(cmd, "offset did not consume its source selection"));
            }
            sides.extend(group);
        }
        Ok((sides, param, param2))
    })?;
    let curves: Vec<&String> = sides.iter().flat_map(|side| side.curves.iter()).collect();
    let constraints: Vec<&String> = sides.iter().flat_map(|side| side.constraints.iter()).collect();
    let each: Vec<Value> = sides
        .iter()
        .map(|side| {
            let mut entry = json!({"distance": side.distance, "curves": side.curves, "param": side.param});
            if let Some(association) = &side.association {
                entry["association"] = json!(association);
            }
            entry
        })
        .collect();
    let mut response = json!({"curves":curves,"constraints":constraints,"sketch":info,"sides":each,"param":param});
    if let Some(param2) = param2 {
        response["param2"] = json!(param2);
    }
    // One side keeps the single `association` it always returned. Two sides have one each, in
    // `sides`, so there is no single association left to name.
    if let [only] = sides.as_slice()
        && let Some(association) = &only.association
    {
        response["association"] = json!(association);
    }
    Ok(response)
}

fn break_offset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.offset.break";
    let sketch = super::sketch::target_sketch(s, p, cmd)?;
    let (out, info) = edit(s, p, cmd, true, |sk, doc| {
        let output = str_(p, "curve").ok_or_else(|| bad(cmd, "`curve` must name an offset output"))?;
        let record = sk.freeform_offsets.iter().position(|record| record.output == output);
        let topology = sk
            .offsets
            .iter()
            .filter_map(|record| record.topology.as_ref())
            .any(|record| record.id == output || record.roles.iter().any(|role| role.output == output));
        let analytic = sk.offsets.iter().any(|o| o.outputs.iter().any(|id| id == output));
        if record.is_none() && !topology && !analytic {
            return Err(bad(cmd, "curve is not an offset output"));
        }
        // A parameter edit may have changed the evaluated source without
        // rewriting the stored fit points. Break must freeze the displayed,
        // current dimensioned geometry, rather than its stale stored predecessor.
        let rep = solve_current(sk, doc)?;
        if !rep.keeps_orientation() {
            return Err(bad(cmd, rep.failure(|_| "cannot break an offset with conflicting dimensions".into())));
        }
        sk.validate_periodic_splines()?;
        if let Some(association) = sk
            .offsets
            .iter()
            .filter_map(|record| record.topology.as_ref())
            .find(|record| record.id == output || record.roles.iter().any(|role| role.output == output))
            .map(|record| record.id.clone())
        {
            freeze_offset_profiles(doc, sketch, sk, &association)?;
        }
        if let Some(curves) = sk.break_offset_topology(output)? {
            return Ok((output.to_string(), Some(curves)));
        }
        let ordinary = sk.offsets.iter().position(|o| o.topology.is_none() && o.outputs.iter().any(|id| id == output));
        if let Some(index) = ordinary {
            let record = sk.offsets.remove(index);
            let centres = record
                .outputs
                .iter()
                .filter_map(|id| sk.curve_index(id).and_then(|i| sk.curves.get(i)))
                .filter_map(|curve| match curve.kind {
                    CurveKind::Circle { c, .. } | CurveKind::Arc { c, .. } => Some(c),
                    _ => None,
                })
                .collect::<std::collections::BTreeSet<_>>();
            for centre in centres {
                let position = sk.point(centre).ok_or_else(|| bad(cmd, "missing offset centre"))?;
                let detached = sk.add_point(position, None)?;
                for curve in &mut sk.curves {
                    if record.outputs.contains(&curve.id) {
                        match &mut curve.kind {
                            CurveKind::Circle { c, .. } | CurveKind::Arc { c, .. } if *c == centre => *c = detached,
                            _ => {}
                        }
                    }
                }
            }
            sk.constraints.retain(|c| !matches!(&c.kind, ConstraintKind::OffsetDistance { output, .. } if record.outputs.contains(output)));
            for id in &record.outputs {
                if let Some(curve) = sk.curve_index(id).and_then(|i| sk.curves.get(i)) {
                    let points = curve.kind.point_ids();
                    for i in points {
                        if i != 0
                            && sk.curves.iter().all(|c| !c.kind.uses(i) || record.outputs.contains(&c.id))
                            && let Some(p) = sk.points.get_mut(i)
                        {
                            p.fixed = false;
                        }
                    }
                }
            }
            return Ok((output.to_string(), Some(record.outputs)));
        }
        let record = record.ok_or_else(|| bad(cmd, "offset association changed while materializing its current geometry"))?;
        sk.constraints.retain(|c| !matches!(&c.kind, ConstraintKind::OffsetDistance { output: id, .. } if id == output));
        let ci = sk.curve_index(output).ok_or_else(|| bad(cmd, "missing offset output"))?;
        let indices = sk.curves[ci].kind.point_ids();
        sk.freeform_offsets.remove(record);
        for i in indices {
            if let Some(point) = sk.points.get_mut(i) {
                point.fixed = false;
            }
        }
        Ok((output.to_string(), None))
    })?;
    s.doc_mut().prune_model_params();
    let mut response = json!({"curve":out.0,"sketch":info});
    if let Some(curves) = out.1 {
        response["curves"] = json!(curves);
    }
    Ok(response)
}

/// A broken logical boundary becomes ordinary geometry. Keep downstream features
/// on that exact current boundary while other associations retain their full roles.
fn freeze_offset_profiles(doc: &mut solidcraft_doc::Document, sketch: u64, boundary: &Sketch, association: &str) -> Result<()> {
    let cmd = "sketch.offset.break";
    let mut groups = std::collections::BTreeMap::new();
    let mut work = 0usize;
    for offset in &boundary.offsets {
        let Some(topology) = &offset.topology else { continue };
        work = work.saturating_add(topology.roles.len());
        if groups.len() >= 128 || topology.roles.len() > 512 || work > 1_000_000 {
            return Err(bad(cmd, "too many offset roles to freeze safely"));
        }
        let reserved = topology.roles.iter().map(|role| role.output.clone()).collect::<Vec<_>>();
        if groups.insert(topology.id.clone(), (offset.outputs.clone(), reserved)).is_some() {
            return Err(bad(cmd, "ambiguous offset association identity"));
        }
    }
    let (active, reserved) = groups.get(association).ok_or_else(|| bad(cmd, "offset association disappeared before freezing"))?;
    let mut reserved_sorted = reserved.clone();
    reserved_sorted.sort();
    for feature in &mut doc.features {
        for (source, selection) in feature.kind.profile_sels_mut() {
            if source != sketch {
                continue;
            }
            match selection {
                ProfileSel::OffsetBoundaries { associations } if associations.iter().any(|id| id == association) => {
                    let mut loops = Vec::with_capacity(associations.len());
                    for id in associations.iter() {
                        let (current, all_roles) = groups.get(id).ok_or_else(|| bad(cmd, format!("cannot freeze missing offset boundary `{id}`")))?;
                        let roles = if id == association { current } else { all_roles };
                        work = work.saturating_add(roles.len());
                        if work > 1_000_000 {
                            return Err(bad(cmd, "too many downstream offset references to freeze safely"));
                        }
                        loops.push(roles.clone());
                    }
                    *selection = ProfileSel::Curves { loops };
                }
                ProfileSel::Curves { loops } => {
                    for ids in loops {
                        work = work.saturating_add(ids.len());
                        if work > 1_000_000 {
                            return Err(bad(cmd, "too many downstream curve references to freeze safely"));
                        }
                        if ids.len() == reserved_sorted.len() {
                            let mut sorted = ids.clone();
                            sorted.sort();
                            if sorted == reserved_sorted {
                                *ids = active.clone();
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Copies: mirror, patterns, move, scale

/// Curves and lone points named in `entities`.
fn entities(sk: &Sketch, p: &Value, cmd: &str) -> Result<(Vec<usize>, Vec<usize>)> {
    let ids = string_list(p, "entities");
    if ids.is_empty() {
        return Err(bad(cmd, "`entities` must list sketch curves or points"));
    }
    let (mut cs, mut ps) = (Vec::new(), Vec::new());
    for id in &ids {
        if let Some(c) = sk.curve_index(id) {
            cs.push(c);
        } else if let Some(q) = sk.resolve_point(id) {
            ps.push(q);
        } else {
            return Err(bad(cmd, format!("unknown sketch entity `{id}`")));
        }
    }
    Ok((cs, ps))
}

/// Points used by curves plus lone points, in a stable order.
fn involved_points(sk: &Sketch, cs: &[usize], ps: &[usize]) -> Vec<usize> {
    let mut v: Vec<usize> = Vec::new();
    for c in cs {
        if let Some(k) = sk.curves.get(*c).map(|c| &c.kind) {
            v.extend(k.point_ids());
        }
    }
    v.extend(ps);
    let mut out = Vec::new();
    for q in v {
        if !out.contains(&q) {
            out.push(q);
        }
    }
    out
}

/// Copy curves and points through `f`. `flip` reverses arcs (mirroring). `keep(p)`: reuse the
/// original point instead of copying it (points on a mirror line). Returns the new curves and
/// the point map (original → copy).
fn copy_through(
    sk: &mut Sketch,
    cs: &[usize],
    ps: &[usize],
    f: &dyn Fn(Vec2) -> Vec2,
    flip: bool,
    keep: &dyn Fn(Vec2) -> bool,
    scale_r: f64,
) -> Result<(Vec<usize>, Vec<(usize, usize)>)> {
    if sk.curves.len().saturating_add(cs.len()) > solidcraft_sketch::MAX_CURVES {
        return Err(solidcraft_sketch::SketchError::TooLarge.into());
    }
    let pts = involved_points(sk, cs, ps);
    let mut map: Vec<(usize, usize)> = Vec::new();
    for q in &pts {
        let pos = sk.point(*q).unwrap_or_default();
        let n = if keep(pos) { *q } else { sk.add_point(f(pos), None)? };
        map.push((*q, n));
    }
    let m = |q: usize| map.iter().find(|x| x.0 == q).map(|x| x.1).unwrap_or(q);
    let mut made = Vec::new();
    for c in cs {
        let Some(cu) = sk.curves.get(*c).cloned() else { continue };
        let mut kind = cu.kind.clone();
        kind.map_points(&m);
        let (kind, prefix) = match cu.kind {
            CurveKind::Ellipse { r, .. } => {
                if let CurveKind::Ellipse { r: slot, .. } = &mut kind {
                    *slot = r * scale_r;
                }
                (kind, "e")
            }
            CurveKind::Spline { .. } => (kind, "s"),
            CurveKind::Conic { .. } => (kind, "k"),
            CurveKind::Line { a, b } => (CurveKind::Line { a: m(a), b: m(b) }, "l"),
            CurveKind::Circle { c, r } => (CurveKind::Circle { c: m(c), r: r * scale_r }, "c"),
            CurveKind::Arc { c, a, b } => {
                if flip {
                    (CurveKind::Arc { c: m(c), a: m(b), b: m(a) }, "a")
                } else {
                    (CurveKind::Arc { c: m(c), a: m(a), b: m(b) }, "a")
                }
            }
        };
        let id = sk.fresh(prefix)?;
        // A mirrored or patterned infinite line is an infinite line too (#1359).
        let infinite = cu.infinite && matches!(kind, CurveKind::Line { .. });
        sk.curves.push(Curve {
            id,
            kind,
            construction: cu.construction,
            reversed: if flip { !cu.reversed } else { cu.reversed },
            link: None,
            centerline: false,
            fixed: false,
            infinite,
        });
        made.push(sk.curves.len() - 1);
    }
    Ok((made, map))
}

fn mirror(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.mirror";
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let (cs, ps) = entities(sk, p, cmd)?;
        let lid = str_(p, "line").ok_or_else(|| bad(cmd, "`line` must be the mirror line"))?;
        let l = sk.curve_index(lid).ok_or_else(|| bad(cmd, format!("unknown curve `{lid}`")))?;
        let Some(Shape::Line { a, b }) = sk.shape(l) else { return Err(bad(cmd, "the mirror must be a line")) };
        let u = (b - a).normalized().ok_or_else(|| bad(cmd, "degenerate mirror line"))?;
        let refl = move |q: Vec2| {
            let foot = a + u * (q - a).dot(u);
            foot * 2.0 - q
        };
        let on_line = move |q: Vec2| u.cross(q - a).abs() < TOL * 10.0;
        let cs: Vec<usize> = cs.into_iter().filter(|c| *c != l).collect();
        let (made, map) = copy_through(sk, &cs, &ps, &refl, true, &on_line, 1.0)?;
        let mut cons = Vec::new();
        for (o, n) in &map {
            if o != n {
                cons.push(add_c(sk, ConstraintKind::Symmetric { p: *o, q: *n, l })?);
            } else if !sk.curves.get(l).is_some_and(|curve| curve.kind.point_ids().contains(o)) {
                // A shared endpoint must stay on the mirror line as the source changes.
                cons.push(add_c(sk, ConstraintKind::PointOnCurve { p: *o, c: l })?);
            }
        }
        for (o, n) in cs.iter().zip(&made) {
            if matches!(sk.curves.get(*o).map(|c| &c.kind), Some(CurveKind::Circle { .. })) {
                cons.push(add_c(sk, ConstraintKind::Equal { a: *o, b: *n })?);
            }
        }
        Ok((ids_of(sk, &made), cons))
    })?;
    Ok(json!({"curves": out.0, "constraints": out.1, "sketch": info}))
}

fn count(p: &Value, k: &str, cmd: &str) -> Result<usize> {
    let n = num(p, k).ok_or_else(|| bad(cmd, format!("`{k}` must be a number")))?;
    if !(1.0..=MAX_COPIES as f64).contains(&n) || n.fract() != 0.0 {
        return Err(bad(cmd, format!("`{k}` must be 1…{MAX_COPIES}")));
    }
    Ok(n as usize)
}

fn point_or_ref(sk: &Sketch, p: &Value, k: &str, cmd: &str) -> Result<Vec2> {
    match p.get(k) {
        Some(Value::String(r)) => sk.resolve_point(r).and_then(|i| sk.point(i)).ok_or_else(|| bad(cmd, format!("unknown point `{r}`"))),
        Some(v) => vec2(v).ok_or_else(|| bad(cmd, format!("`{k}` must be [x, y] or a point id"))),
        None => Err(bad(cmd, format!("missing `{k}`"))),
    }
}

fn rotate(c: Vec2, ang: f64) -> impl Fn(Vec2) -> Vec2 {
    move |q: Vec2| {
        let d = q - c;
        let (s, co) = ang.sin_cos();
        c + Vec2::new(d.x * co - d.y * s, d.x * s + d.y * co)
    }
}

fn pattern_circular(s: &mut Session, p: &Value) -> Result<Value> {
    pattern::run(s, p, true)
}

fn pattern_rect(s: &mut Session, p: &Value) -> Result<Value> {
    pattern::run(s, p, false)
}

/// Move the points of the entities (in place) through `f`; circle radii scale by `k`.
fn transform_in_place(sk: &mut Sketch, cs: &[usize], ps: &[usize], f: &dyn Fn(Vec2) -> Vec2, k: f64, cmd: &str) -> Result<()> {
    let pts = involved_points(sk, cs, ps);
    if pts.iter().any(|q| sk.points.get(*q).is_some_and(|x| x.fixed || x.link.is_some())) {
        return Err(bad(cmd, "fixed or projected geometry cannot move"));
    }
    for q in pts {
        if let Some(x) = sk.points.get_mut(q) {
            x.pos = f(x.pos);
        }
    }
    for c in cs {
        if let Some(Curve { kind: CurveKind::Circle { r, .. } | CurveKind::Ellipse { r, .. }, .. }) = sk.curves.get_mut(*c) {
            *r *= k;
        }
    }
    sk.follow_pattern_seeds();
    Ok(())
}

fn move_copy(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.move";
    let t = if p.get("translate").is_some() { req_vec2(cmd, p, "translate")? } else { Vec2::ZERO };
    let ang = if p.get("angle").is_some() { num(p, "angle").ok_or_else(|| bad(cmd, "`angle` must be finite"))? } else { 0.0 }.to_radians();
    let copy = bool_(p, "copy").unwrap_or(false);
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let (mut cs, mut ps) = entities(sk, p, cmd)?;
        let c = if p.get("center").is_some() { point_or_ref(sk, p, "center", cmd)? } else { Vec2::ZERO };
        let rot = rotate(c, ang);
        let f = move |q: Vec2| rot(q) + t;
        let mut links = Vec::new();
        for entity in string_list(p, "entities") {
            if let Some(link) = super::sketch_create::text_link(sk, &entity)
                && !links.iter().any(|old| old == link)
            {
                links.push(link.to_owned());
            }
        }
        cs.retain(|&i| !sk.curves.get(i).and_then(|c| c.link.as_ref()).is_some_and(|link| links.contains(link)));
        ps.retain(|&i| !sk.points.get(i).and_then(|p| p.link.as_ref()).is_some_and(|link| links.contains(link)));
        let mut text_curves = Vec::new();
        for link in links {
            text_curves.extend(super::sketch_create::move_text(sk, &link, t, ang, c, copy)?);
        }
        if copy {
            let (mut m, _) = copy_through(sk, &cs, &ps, &f, false, &|_| false, 1.0)?;
            m.extend(text_curves);
            return Ok(ids_of(sk, &m));
        }
        transform_in_place(sk, &cs, &ps, &f, 1.0, cmd)?;
        cs.extend(text_curves);
        Ok(ids_of(sk, &cs))
    })?;
    Ok(json!({"curves": out, "sketch": info}))
}

fn scale(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.scale";
    let k = num(p, "factor").filter(|k| *k > 1e-9 && *k < 1e6).ok_or_else(|| bad(cmd, "`factor` must be a positive number"))?;
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let (cs, ps) = entities(sk, p, cmd)?;
        let b = if p.get("base").is_some() { point_or_ref(sk, p, "base", cmd)? } else { Vec2::ZERO };
        let f = move |q: Vec2| b + (q - b) * k;
        transform_in_place(sk, &cs, &ps, &f, k, cmd)?;
        Ok(ids_of(sk, &cs))
    })?;
    Ok(json!({"curves": out, "sketch": info}))
}

#[cfg(test)]
#[path = "sketch_modify_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sketch_robust_tests.rs"]
mod robust_tests;

#[cfg(test)]
#[path = "sketch_corner_tests.rs"]
mod corner_tests;
