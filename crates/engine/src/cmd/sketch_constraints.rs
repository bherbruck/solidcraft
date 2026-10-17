//! More sketch constraints (curvature, polygon), constraint glyph anchors and snapping
//! inference for interactive tools.

use serde_json::{Value, json};
use solidcraft_geom::Vec2;
use solidcraft_sketch::{ConstraintKind, CurveKind, Sketch};

use super::sketch::{add_c, constraint_result, dof_now, edit, listed, redundancy, residual_of};
use super::{CommandSpec, in_sketch};
use crate::params::{bad, num, req_vec2, str_, string_list, vec2};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("sketch.constraint.curvature", "Curvature", c_smooth)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_smooth")
        .enabled(in_sketch)
        .params("a, b: two curves sharing an end point (curvature continuous, G2)"),
    CommandSpec::new("sketch.constraint.polygon", "Polygon", c_polygon)
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_polygon")
        .enabled(in_sketch)
        .params("lines: [line ids] of a closed chain: equal sides with every corner on one circle (a regular polygon)"),
    CommandSpec::new("sketch.auto_constrain", "AutoConstrain", auto_constrain)
        .at("SKETCH", "CONSTRAINTS")
        .icon("auto_constrain")
        .enabled(in_sketch)
        .params("tolerance?: mm (default 0.01), angle_tolerance?: deg (default 1), dimensions?: bool (default true): infer constraints, then dimension until fully constrained"),
    CommandSpec::new("sketch.auto_constrain_from_datum", "AutoConstrain from datum", auto_constrain_datum)
        .at("SKETCH", "CONSTRAINTS")
        .icon("auto_constrain")
        .enabled(in_sketch)
        .params("datum: point ref (default origin); like AutoConstrain with positions measured from the datum"),
    CommandSpec::new("sketch.finish_auto_constrain", "Finish with AutoConstrain", auto_constrain_finish)
        .at("SKETCH", "FINISH SKETCH")
        .icon("finish")
        .enabled(in_sketch)
        .params("like AutoConstrain, then Finish Sketch"),
    CommandSpec::new("sketch.constrainer", "Sketch Constrainer", smart_constrain)
        .at("SKETCH", "CREATE")
        .icon("constrainer")
        .enabled(in_sketch)
        .params("entities: [one or two sketch entity ids]: applies the constraint the geometry nearly has (horizontal/vertical, parallel, perpendicular, collinear, coincident, concentric, equal, tangent)"),
    CommandSpec::new("sketch.toggle_driven", "Toggle Driven Dimension", toggle_driven).icon("dimension")
        .enabled(in_sketch)
        .params("constraint: dimension id, driven?: bool (default toggles); a driving dimension gets a parameter with its current value"),
    CommandSpec::new("sketch.glyphs", "Constraint Glyphs", glyphs).icon("eye")
        .enabled(in_sketch)
        .noundo()
        .params("sketch?: id|name: every constraint with where to draw its glyph (sketch coordinates)"),
    CommandSpec::new("sketch.snap", "Snap Inference", snap).icon("snap")
        .enabled(in_sketch)
        .noundo()
        .params("at: [x,y], from?: [x,y] (previous click), radius?: mm (snap distance, default 1): what a click there would snap to and infer"),
];

fn c_smooth(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constraint.curvature";
    let (mut before, mut residual) = (None, None);
    let (id, info) = edit(s, p, cmd, true, |sk, _| {
        let a = curve(sk, p.get("a"), cmd)?;
        let b = curve(sk, p.get("b"), cmd)?;
        before = Some(dof_now(sk));
        let id = add_c(sk, ConstraintKind::Smooth { a, b })?;
        residual = residual_of(sk, &id);
        Ok(id)
    })?;
    let by = redundancy(s, p, cmd, before, &info, residual, &id)?;
    constraint_result(p, cmd, id, info, by, Value::Null)
}

fn curve(sk: &Sketch, v: Option<&Value>, cmd: &str) -> Result<usize> {
    let id = v.and_then(Value::as_str).ok_or_else(|| bad(cmd, "expected a curve id"))?;
    sk.curve_index(id).ok_or_else(|| bad(cmd, format!("unknown curve `{id}`")))
}

fn c_polygon(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constraint.polygon";
    let ids = string_list(p, "lines");
    if ids.len() < 3 || ids.len() > 200 {
        return Err(bad(cmd, "`lines` must list 3 to 200 lines of a closed chain"));
    }
    let (out, info) = edit(s, p, cmd, true, |sk, _| {
        let ls: Vec<usize> = ids.iter().map(|i| sk.curve_index(i).ok_or_else(|| bad(cmd, format!("unknown curve `{i}`")))).collect::<Result<_>>()?;
        let mut corners: Vec<usize> = Vec::new();
        for l in &ls {
            match sk.curves.get(*l).map(|c| &c.kind) {
                Some(CurveKind::Line { a, b }) => {
                    for q in [*a, *b] {
                        if !corners.contains(&q) {
                            corners.push(q);
                        }
                    }
                }
                _ => return Err(bad(cmd, "the polygon must be made of lines")),
            }
        }
        if corners.len() != ls.len() {
            return Err(bad(cmd, "the lines must form one closed chain"));
        }
        let pts: Vec<Vec2> = corners.iter().filter_map(|q| sk.point(*q)).collect();
        let c = pts.iter().fold(Vec2::ZERO, |a, q| a + *q) / pts.len() as f64;
        let r = pts.iter().map(|q| q.dist(c)).sum::<f64>() / pts.len() as f64;
        let circ = sk.add_circle(c, r.max(1e-6), None, None)?;
        if let Some(cu) = sk.curves.get_mut(circ) {
            cu.construction = true;
        }
        let mut cons = Vec::new();
        for w in ls.windows(2) {
            cons.push(add_c(sk, ConstraintKind::Equal { a: w[0], b: w[1] })?);
        }
        for q in corners {
            cons.push(add_c(sk, ConstraintKind::PointOnCurve { p: q, c: circ })?);
        }
        Ok(cons)
    })?;
    Ok(json!({"constraints": out, "sketch": info}))
}

/// Most constraints AutoConstrain tries.
const MAX_TRIALS: usize = 2000;

/// Add `k` if it removes a degree of freedom and the sketch still solves; else leave the
/// sketch as it was. Returns the new dof when kept.
fn try_add(sk: &mut Sketch, doc: &mut solidcraft_doc::Document, k: ConstraintKind, dof: usize, dim: bool) -> Option<usize> {
    let saved = sk.clone();
    let param = if dim {
        let v = k.value()?;
        let e = if k.is_angle() { format!("{} deg", (v.to_degrees() * 1e6).round() / 1e6) } else { format!("{} mm", (v * 1e6).round() / 1e6) };
        Some(doc.new_model_param(&e, if k.is_angle() { "deg" } else { "mm" }))
    } else {
        None
    };
    if sk.add_constraint(k, param.clone()).is_err() {
        if let Some(pn) = &param {
            let _ = doc.remove_param(pn);
        }
        return None;
    }
    let mut trial = sk.clone();
    let rep = solidcraft_sketch::solve(&mut trial);
    if rep.ok() && rep.dof < dof {
        *sk = trial;
        return Some(rep.dof);
    }
    *sk = saved;
    if let Some(pn) = &param {
        let _ = doc.remove_param(pn);
    }
    None
}

fn run_auto(sk: &mut Sketch, doc: &mut solidcraft_doc::Document, p: &Value, datum: usize) -> (Vec<String>, usize) {
    use ConstraintKind::*;
    let tol = num(p, "tolerance").filter(|t| *t > 0.0).unwrap_or(0.01);
    let atol = num(p, "angle_tolerance").filter(|t| *t > 0.0).unwrap_or(1.0).to_radians();
    let dims = crate::params::bool_(p, "dimensions").unwrap_or(true);
    let n0 = sk.constraints.len();
    let mut dof = dof_now(sk);
    let mut trials = 0;
    let mut attempt = |sk: &mut Sketch, doc: &mut solidcraft_doc::Document, k: ConstraintKind, dim: bool, dof: &mut usize| {
        if *dof == 0 || trials >= MAX_TRIALS {
            return;
        }
        trials += 1;
        if let Some(d) = try_add(sk, doc, k, *dof, dim) {
            *dof = d;
        }
    };
    // Coincident points.
    let np = sk.points.len();
    for i in 0..np {
        for j in (i + 1)..np {
            let (Some(a), Some(b)) = (sk.point(i), sk.point(j)) else { continue };
            if a.dist(b) <= tol {
                attempt(sk, doc, Coincident { p: i, q: j }, false, &mut dof);
            }
        }
    }
    // Horizontal / vertical lines, tangent line–arc and arc–arc joints.
    let nc = sk.curves.len();
    for i in 0..nc {
        if let Some(CurveKind::Line { a, b }) = sk.curves.get(i).map(|c| c.kind.clone())
            && let (Some(pa), Some(pb)) = (sk.point(a), sk.point(b))
        {
            let ang = (pb - pa).angle();
            let off_h = ang.sin().abs().asin();
            let off_v = ang.cos().abs().asin();
            if off_h <= atol {
                attempt(sk, doc, Horizontal { l: i }, false, &mut dof);
            } else if off_v <= atol {
                attempt(sk, doc, Vertical { l: i }, false, &mut dof);
            }
        }
    }
    for i in 0..nc {
        for j in (i + 1)..nc {
            let (Some(ci), Some(cj)) = (sk.curves.get(i), sk.curves.get(j)) else { continue };
            let round = |k: &CurveKind| matches!(k, CurveKind::Arc { .. } | CurveKind::Circle { .. });
            if !(round(&ci.kind) || round(&cj.kind)) {
                continue;
            }
            let (Some((a0, a1)), Some((b0, b1))) = (ci.kind.ends(), cj.kind.ends()) else { continue };
            let Some(sh) = [a0, a1].into_iter().find(|x| *x == b0 || *x == b1) else { continue };
            let dir = |c: usize| -> Option<Vec2> {
                let k = sk.curves.get(c)?.kind.clone();
                let q = sk.point(sh)?;
                match k {
                    CurveKind::Line { a, b } => (sk.point(b)? - sk.point(a)?).normalized(),
                    CurveKind::Arc { c, .. } => (q - sk.point(c)?).perp().normalized(),
                    _ => None,
                }
            };
            if let (Some(u), Some(v)) = (dir(i), dir(j))
                && u.cross(v).abs() <= atol.sin()
            {
                attempt(sk, doc, Tangent { a: i, b: j }, false, &mut dof);
            }
        }
    }
    if dims {
        for i in 0..nc {
            let k = match sk.curves.get(i).map(|c| (c.kind.clone(), c.construction)) {
                Some((CurveKind::Circle { r, .. }, false)) => Diameter { c: i, value: 2.0 * r },
                Some((CurveKind::Arc { .. }, false)) => Radius { c: i, value: sk.radius(i).unwrap_or(1.0) },
                Some((CurveKind::Line { a, b }, false)) => {
                    Length { l: i, value: sk.point(a).zip(sk.point(b)).map(|(x, y)| x.dist(y)).unwrap_or(1.0) }
                }
                _ => continue,
            };
            attempt(sk, doc, k, true, &mut dof);
        }
        // Positions from the datum.
        let d = sk.point(datum).unwrap_or_default();
        for i in 0..sk.points.len() {
            if i == datum {
                continue;
            }
            let Some(q) = sk.point(i) else { continue };
            attempt(sk, doc, DistanceX { p: datum, q: i, value: (q.x - d.x).abs() }, true, &mut dof);
            attempt(sk, doc, DistanceY { p: datum, q: i, value: (q.y - d.y).abs() }, true, &mut dof);
        }
    }
    let added = sk.constraints.iter().skip(n0).map(|c| c.id.clone()).collect();
    (added, dof)
}

fn auto(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    let ((added, dof), info) = edit(s, p, cmd, false, |sk, doc| {
        let datum = match str_(p, "datum") {
            Some(r) => sk.resolve_point(r).ok_or_else(|| bad(cmd, format!("unknown point `{r}`")))?,
            None => 0,
        };
        // Values of existing dimensions must be applied (and reached from the stored solved
        // state) before judging degrees of freedom; a sketch they would turn over keeps its
        // stored geometry, as the edit's own solve then refuses them.
        let rep = super::sketch::solve_current(sk, doc)?;
        if !rep.keeps_orientation() {
            return Err(bad(cmd, rep.failure(|f| format!("the sketch constraints conflict ({f})"))));
        }
        Ok(run_auto(sk, doc, p, datum))
    })?;
    Ok(json!({"added": added, "dof": dof, "sketch": info}))
}

fn auto_constrain(s: &mut Session, p: &Value) -> Result<Value> {
    auto(s, p, "sketch.auto_constrain")
}
fn auto_constrain_datum(s: &mut Session, p: &Value) -> Result<Value> {
    auto(s, p, "sketch.auto_constrain_from_datum")
}
fn auto_constrain_finish(s: &mut Session, p: &Value) -> Result<Value> {
    let r = auto(s, p, "sketch.finish_auto_constrain")?;
    s.active_sketch = None;
    s.revision += 1;
    Ok(r)
}

/// The constraint two entities (or one line) are closest to satisfying.
fn guess(sk: &Sketch, ents: &[String], cmd: &str) -> Result<ConstraintKind> {
    use ConstraintKind::*;
    let ent = |r: &str| -> Result<(Option<usize>, Option<usize>)> {
        if let Some(c) = sk.curve_index(r) {
            return Ok((None, Some(c)));
        }
        sk.resolve_point(r).map(|p| (Some(p), None)).ok_or_else(|| bad(cmd, format!("unknown sketch entity `{r}`")))
    };
    let line_dir = |c: usize| match sk.shape(c) {
        Some(solidcraft_sketch::Shape::Line { a, b }) => (b - a).normalized(),
        _ => None,
    };
    let round = |c: usize| matches!(sk.curves.get(c).map(|c| &c.kind), Some(CurveKind::Circle { .. } | CurveKind::Arc { .. }));
    match ents {
        [a] => {
            let (_, Some(c)) = ent(a)? else { return Err(bad(cmd, "pick a line, or two entities")) };
            let d = line_dir(c).ok_or_else(|| bad(cmd, "a single entity must be a line"))?;
            Ok(if d.x.abs() >= d.y.abs() { Horizontal { l: c } } else { Vertical { l: c } })
        }
        [a, b] => match (ent(a)?, ent(b)?) {
            ((Some(p), _), (Some(q), _)) => Ok(Coincident { p, q }),
            ((Some(p), _), (_, Some(c))) | ((_, Some(c)), (Some(p), _)) => {
                // A point at a curve's centre is concentric-like: coincident with the centre.
                if let Some(cp) = sk.curves.get(c).and_then(|cu| match cu.kind {
                    CurveKind::Circle { c, .. } | CurveKind::Arc { c, .. } => Some(c),
                    _ => None,
                }) && sk.point(cp).zip(sk.point(p)).is_some_and(|(x, y)| x.dist(y) < sk.radius(c).unwrap_or(1.0) * 0.25)
                {
                    return Ok(Coincident { p, q: cp });
                }
                Ok(PointOnCurve { p, c })
            }
            ((_, Some(x)), (_, Some(y))) => {
                if let (Some(u), Some(v)) = (line_dir(x), line_dir(y)) {
                    let cross = u.cross(v).abs();
                    if cross < 0.2 {
                        // Parallel; on one line → collinear.
                        let off = match (sk.shape(x), sk.shape(y)) {
                            (Some(solidcraft_sketch::Shape::Line { a, .. }), Some(solidcraft_sketch::Shape::Line { a: b0, .. })) => {
                                u.cross(b0 - a).abs()
                            }
                            _ => f64::INFINITY,
                        };
                        let len = sk.polyline(x).windows(2).map(|w| w[0].dist(w[1])).sum::<f64>();
                        return Ok(if off < len * 0.05 { Collinear { a: x, b: y } } else { Parallel { a: x, b: y } });
                    }
                    if u.dot(v).abs() < 0.2 {
                        return Ok(Perpendicular { a: x, b: y });
                    }
                    return Ok(Equal { a: x, b: y });
                }
                if round(x) && round(y) {
                    let (cx, cy) = (sk.center(x).unwrap_or_default(), sk.center(y).unwrap_or_default());
                    let (rx, ry) = (sk.radius(x).unwrap_or(1.0), sk.radius(y).unwrap_or(1.0));
                    if cx.dist(cy) < 0.25 * rx.min(ry) {
                        return Ok(Concentric { a: x, b: y });
                    }
                    let d = cx.dist(cy);
                    if (d - (rx + ry)).abs() < 0.15 * rx.min(ry) || (d - (rx - ry).abs()).abs() < 0.15 * rx.min(ry) {
                        return Ok(Tangent { a: x, b: y });
                    }
                    return Ok(Equal { a: x, b: y });
                }
                Ok(Tangent { a: x, b: y })
            }
            _ => Err(bad(cmd, "cannot constrain that combination")),
        },
        _ => Err(bad(cmd, "`entities` must list one or two entities")),
    }
}

fn smart_constrain(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constrainer";
    let ents = string_list(p, "entities");
    let (mut before, mut residual) = (None, None);
    let mut name = "";
    let (id, info) = edit(s, p, cmd, true, |sk, _| {
        let k = guess(sk, &ents, cmd)?;
        name = k.name();
        before = Some(dof_now(sk));
        let id = add_c(sk, k)?;
        residual = residual_of(sk, &id);
        Ok(id)
    })?;
    let by = redundancy(s, p, cmd, before, &info, residual, &id)?;
    constraint_result(p, cmd, id, info, by, json!({"type": name}))
}

fn toggle_driven(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.toggle_driven";
    let cid = str_(p, "constraint").ok_or_else(|| bad(cmd, "`constraint` must be a dimension id"))?.to_string();
    let want = crate::params::bool_(p, "driven");
    let mut before = None;
    let (driven, info) = edit(s, p, cmd, true, |sk, doc| {
        let c = sk.constraints.iter().find(|c| c.id == cid).cloned().ok_or_else(|| bad(cmd, format!("no constraint `{cid}`")))?;
        let v = c.kind.value().ok_or_else(|| bad(cmd, format!("`{cid}` is not a dimension")))?;
        let driven = want.unwrap_or(!c.driven);
        if driven == c.driven {
            return Ok(driven);
        }
        let slot = sk.constraints.iter_mut().find(|c| c.id == cid).ok_or_else(|| bad(cmd, "constraint"))?;
        if driven {
            slot.driven = true;
            slot.param = None;
        } else {
            let (unit, e) = if c.kind.is_angle() {
                ("deg", format!("{} deg", (v.to_degrees() * 1e6).round() / 1e6))
            } else {
                ("mm", format!("{} mm", (v * 1e6).round() / 1e6))
            };
            slot.driven = false;
            slot.param = Some(doc.new_model_param(&e, unit));
            before = None;
        }
        Ok(driven)
    })?;
    if !driven {
        // Making it driving must not over-constrain: compare with the sketch without it.
        let id = s.active_sketch.ok_or_else(|| bad(cmd, "no sketch"))?;
        let mut sk = s.doc.sketch(id)?.clone();
        if let Some(c) = sk.constraints.iter_mut().find(|c| c.id == cid) {
            c.driven = true;
        }
        before = Some(dof_now(&sk));
        // It holds (its parameter takes the value it measures), so it can only be redundant.
        if let Some(by) = redundancy(s, p, cmd, before, &info, Some(0.0), &cid)? {
            return Err(bad(cmd, format!("that dimension is already determined by {}; it stays driven", listed(&by))));
        }
    }
    s.doc_mut().prune_model_params();
    Ok(json!({"constraint": cid, "driven": driven, "sketch": info}))
}

/// Where a constraint's glyph goes (sketch coordinates): beside the middle of its first curve,
/// or at its point.
fn anchor(sk: &Sketch, k: &ConstraintKind) -> Option<Vec2> {
    use ConstraintKind::*;
    // Half way along the curve.
    let mid = |c: usize| -> Option<Vec2> {
        let poly = sk.polyline(c);
        let total: f64 = poly.windows(2).map(|w| w[0].dist(w[1])).sum();
        let mut acc = 0.0;
        for w in poly.windows(2) {
            let l = w[0].dist(w[1]);
            if acc + l >= total * 0.5 && l > 0.0 {
                return Some(w[0].lerp(w[1], (total * 0.5 - acc) / l));
            }
            acc += l;
        }
        poly.first().copied()
    };
    let pt = |p: usize| sk.point(p);
    match *k {
        OffsetDistance { ref output, .. } => sk.offset_dimension_points(output).map(|(a, b)| (a + b) * 0.5),
        PatternPoint { q, center, .. } => pt(center.unwrap_or(q)),
        PatternRadius { b, .. } => mid(b),
        Coincident { p, .. } | Fix { p } | Midpoint { p, .. } => pt(p),
        PointOnCurve { p, .. } => pt(p),
        HorizontalPoints { p, q } | VerticalPoints { p, q } | Symmetric { p, q, .. } => Some((pt(p)? + pt(q)?) * 0.5),
        Horizontal { l } | Vertical { l } => mid(l),
        Parallel { a, .. } | Perpendicular { a, .. } | Collinear { a, .. } | Equal { a, .. } | Concentric { a, .. } | Angle { a, .. } => mid(a),
        Tangent { a, b } | Smooth { a, b } => {
            // At the shared end when there is one.
            let ends = |c: usize| sk.curves.get(c).and_then(|c| c.kind.ends());
            match (ends(a), ends(b)) {
                (Some((a0, a1)), Some((b0, b1))) => [a0, a1].into_iter().find(|x| *x == b0 || *x == b1).and_then(pt).or_else(|| mid(a)),
                _ => mid(a),
            }
        }
        Distance { p, q, .. } | DistanceX { p, q, .. } | DistanceY { p, q, .. } => Some((pt(p)? + pt(q)?) * 0.5),
        PointLineDistance { p, .. } | LinearDiameter { p, .. } => pt(p),
        Length { l, .. } | LineAngle { l, .. } => mid(l),
        Radius { c, .. } | Diameter { c, .. } | ArcLength { c, .. } | ArcSweep { c, .. } => mid(c),
        SlotOverallLength { a, b, .. } => Some((sk.center(a)? + sk.center(b)?) * 0.5),
    }
}

fn glyphs(s: &mut Session, p: &Value) -> Result<Value> {
    let id = match p.get("sketch") {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(x)) => s.doc.find_feature(x).map(|f| f.id),
        _ => s.active_sketch,
    }
    .ok_or_else(|| bad("sketch.glyphs", "no such sketch"))?;
    let st = s.model.state();
    let ss = st.sketch(id).ok_or_else(|| EngineError::Other("that sketch is not evaluated".into()))?;
    let failing = &ss.report.failing;
    let list: Vec<Value> = ss
        .sketch
        .constraints
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "name": c.kind.name(),
                "dimension": c.kind.is_dimension(),
                "driven": c.driven,
                "value": c.kind.value(),
                "param": c.param,
                "at": anchor(&ss.sketch, &c.kind),
                "failing": failing.contains(&c.id),
                // Implied by the others (it removes no freedom) but holding: shown dimmed.
                "redundant": ss.report.redundant.contains(&c.id),
            })
        })
        .collect();
    Ok(json!({"sketch": id, "glyphs": list, "dof": ss.report.dof, "fully_constrained": ss.report.fully_constrained(), "conflicts": failing}))
}

/// Snap inference for a click at `at`: the nearest sketch point (coincident), the midpoint of a
/// line, a point on a curve, and horizontal/vertical alignment with `from` or with other
/// points. Results are ordered by priority; the first one is what a click would use.
fn snap(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.snap";
    let at = req_vec2(cmd, p, "at")?;
    let from = p.get("from").and_then(vec2);
    let r = num(p, "radius").filter(|r| *r > 0.0).unwrap_or(1.0);
    let id = s.active_sketch.ok_or_else(|| bad(cmd, "no sketch is being edited"))?;
    let st = s.model.state();
    let ss = st.sketch(id).ok_or_else(|| EngineError::Other("the sketch is not evaluated".into()))?;
    let sk = &ss.sketch;
    let mut out: Vec<(f64, Value)> = Vec::new();
    // Points (end points, centres, the origin).
    for q in &sk.points {
        let d = q.pos.dist(at);
        if d <= r {
            out.push((d * 0.5, json!({"type": "coincident", "point": q.id, "at": q.pos})));
        }
    }
    for (i, c) in sk.curves.iter().enumerate() {
        if let CurveKind::Line { a, b } = c.kind
            && let (Some(pa), Some(pb)) = (sk.point(a), sk.point(b))
        {
            let m = (pa + pb) * 0.5;
            let d = m.dist(at);
            if d <= r {
                out.push((d * 0.6, json!({"type": "midpoint", "curve": c.id, "at": m})));
            }
        }
        if let Some(sh) = sk.shape(i) {
            let q = sh.project(at);
            if sh.dist(at) <= r && q.dist(at) <= r {
                out.push((sh.dist(at) + r * 0.5, json!({"type": "on_curve", "curve": c.id, "at": q})));
            }
        }
    }
    // Alignment with the previous click.
    if let Some(f) = from {
        let d = at - f;
        if d.y.abs() <= r && d.x.abs() > r {
            out.push((d.y.abs() + r, json!({"type": "horizontal", "at": [at.x, f.y]})));
        }
        if d.x.abs() <= r && d.y.abs() > r {
            out.push((d.x.abs() + r, json!({"type": "vertical", "at": [f.x, at.y]})));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let list: Vec<Value> = out.into_iter().take(8).map(|x| x.1).collect();
    Ok(json!({"snaps": list}))
}

#[cfg(test)]
#[path = "sketch_constraints_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sketch_fuzz_tests.rs"]
mod fuzz;

#[cfg(test)]
#[path = "sketch_perf_tests.rs"]
mod perf;
