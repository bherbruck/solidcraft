//! Copy and paste of sketch geometry (#1480). `sketch.copy` keeps the chosen curves and points
//! with the constraints and dimensions that lie wholly among them (a dimension keeps its
//! expression); `sketch.paste` adds an independent copy to the sketch being edited, at a point or
//! offset from the original, as one undo step, and selects it. Constraints that reach outside
//! the copied geometry (to the origin, to projected edges, to other curves) are left behind, as
//! are fixes, so the pasted geometry can be moved straight away.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use solidcraft_geom::Vec2;
use solidcraft_sketch::{ConstraintKind, CurveKind, LinkKind, LinkSource, Sketch};

use super::sketch::{edit, target_sketch};
use super::{CommandSpec, in_sketch};
use crate::params::{bad, string_list, vec2};
use crate::{Result, Sel, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("sketch.copy", "Copy Sketch Entities", copy).icon("copy")
        .noundo()
        .enabled(in_sketch)
        .params("entities?: [curve or point ids] (default: the selected sketch geometry); the constraints and dimensions among them come along"),
    CommandSpec::new("sketch.cut", "Cut Sketch Entities", cut).icon("cut")
        .enabled(in_sketch)
        .params("entities?: [curve or point ids] (default: the selected sketch geometry): copies them as sketch.copy does, then deletes them (one undo step)"),
    CommandSpec::new("sketch.paste", "Paste Sketch Entities", paste).icon("paste").enabled(in_sketch).params(
        "at?: [x, y] (sketch point the copy's centre goes to) | translate?: [x, y]; without either, offset from the original in its own sketch and at the same coordinates in another; sketch?: target (default: the active sketch)",
    ),
];

/// A curve of the clipboard; its points are indices into [`SketchClip::points`].
#[derive(Clone, Debug)]
pub struct ClipCurve {
    pub kind: CurveKind,
    pub construction: bool,
    pub centerline: bool,
    pub reversed: bool,
    pub infinite: bool,
}

/// A constraint or dimension of the clipboard, over clipboard points and curves. A dimension
/// keeps the expression and unit of its parameter.
#[derive(Clone, Debug)]
pub struct ClipConstraint {
    pub kind: ConstraintKind,
    pub param: Option<(String, String)>,
    pub driven: bool,
    pub text: Option<Vec2>,
}

/// Sketch geometry copied with `sketch.copy`, in the sketch coordinates it was copied from.
#[derive(Clone, Debug, Default)]
pub struct SketchClip {
    /// The sketch it was copied from.
    pub sketch: u64,
    pub points: Vec<Vec2>,
    pub curves: Vec<ClipCurve>,
    pub constraints: Vec<ClipConstraint>,
    /// Text sources (their outlines are made again on paste). A path, when kept, is the index of
    /// a clipboard curve.
    pub texts: Vec<(LinkSource, Option<usize>)>,
    /// Lone points (not used by a copied curve), as indices into `points`.
    pub lone: Vec<usize>,
    /// Centre of the copied geometry's bounding box: what `at` places.
    pub center: Vec2,
    /// Size of the bounding box (the larger side), for the paste offset.
    pub extent: f64,
    /// Pastes made without a position since the copy (each lands one offset further on).
    pub pastes: u32,
}

/// Curve and point ids named by `entities`, or the selected sketch geometry.
fn chosen(s: &Session, p: &Value) -> Vec<String> {
    if p.get("entities").is_some() {
        return string_list(p, "entities");
    }
    s.selection
        .iter()
        .filter_map(|sel| match sel {
            Sel::SketchCurve { id } | Sel::SketchPoint { id } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn text_of(sk: &Sketch, ci: usize) -> Option<&str> {
    let link = sk.curves.get(ci)?.link.as_deref()?;
    sk.link(link).filter(|l| l.kind == LinkKind::Text).map(|l| l.id.as_str())
}

fn copy(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.copy";
    let id = target_sketch(s, p, cmd)?;
    let ids = chosen(s, p);
    if ids.is_empty() {
        return Err(bad(cmd, "select the sketch curves or points to copy"));
    }
    let sk = s.doc.sketch(id)?;
    let (mut cs, mut ps, mut texts): (Vec<usize>, Vec<usize>, Vec<String>) = (Vec::new(), Vec::new(), Vec::new());
    for e in &ids {
        if let Some(c) = sk.curve_index(e) {
            match text_of(sk, c) {
                Some(t) if !texts.iter().any(|x| x == t) => texts.push(t.to_string()),
                Some(_) => {}
                None if !cs.contains(&c) => cs.push(c),
                None => {}
            }
        } else if let Some(q) = sk.resolve_point(e) {
            match sk.points.get(q).and_then(|x| x.link.as_deref()).and_then(|l| sk.link(l)).filter(|l| l.kind == LinkKind::Text) {
                Some(l) if !texts.contains(&l.id) => texts.push(l.id.clone()),
                Some(_) => {}
                None if !ps.contains(&q) => ps.push(q),
                None => {}
            }
        } else {
            return Err(bad(cmd, format!("unknown sketch entity `{e}`")));
        }
    }
    let mut clip = SketchClip { sketch: id, ..Default::default() };
    // Points: those of the copied curves, then the lone ones, each once.
    let mut pmap: BTreeMap<usize, usize> = BTreeMap::new();
    let take = |q: usize, clip: &mut SketchClip, pmap: &mut BTreeMap<usize, usize>| -> usize {
        *pmap.entry(q).or_insert_with(|| {
            clip.points.push(sk.point(q).unwrap_or_default());
            clip.points.len() - 1
        })
    };
    let mut cmap: BTreeMap<usize, usize> = BTreeMap::new();
    for &c in &cs {
        let Some(cu) = sk.curves.get(c) else { continue };
        let mut kind = cu.kind.clone();
        let used: Vec<(usize, usize)> = kind.point_ids().into_iter().map(|q| (q, take(q, &mut clip, &mut pmap))).collect();
        kind.map_points(&|q| used.iter().find(|(o, _)| *o == q).map_or(q, |(_, n)| *n));
        cmap.insert(c, clip.curves.len());
        // Projected geometry comes in as ordinary geometry.
        clip.curves.push(ClipCurve { kind, construction: cu.construction, centerline: cu.centerline, reversed: cu.reversed, infinite: cu.infinite });
    }
    for &q in &ps {
        let known = pmap.contains_key(&q);
        let n = take(q, &mut clip, &mut pmap);
        if !known {
            clip.lone.push(n);
        }
    }
    for c in &sk.constraints {
        // Fixes and pattern or offset relations stay with the original.
        if matches!(
            c.kind,
            ConstraintKind::Fix { .. }
                | ConstraintKind::PatternPoint { .. }
                | ConstraintKind::PatternRadius { .. }
                | ConstraintKind::OffsetDistance { .. }
        ) {
            continue;
        }
        let mut kind = c.kind.clone();
        if !kind.remap(&|q| pmap.get(&q).copied(), &|k| cmap.get(&k).copied()) {
            continue;
        }
        let param = c.param.as_ref().map(|name| match s.doc.param(name) {
            Some(x) => (x.expr.clone(), x.unit.clone()),
            None => (format!("{}", kind.value().unwrap_or_default()), if kind.is_angle() { "rad".into() } else { "mm".into() }),
        });
        clip.constraints.push(ClipConstraint { kind, param, driven: c.driven, text: c.text });
    }
    for t in &texts {
        let Some(mut src) = sk.link(t).map(|l| l.source.clone()) else { continue };
        let LinkSource::Text { style, .. } = &mut src else { continue };
        // A text on a path copied with its path follows the copy; otherwise it is set straight.
        let path = style.path.as_deref().and_then(|pid| sk.curve_index(pid)).and_then(|pc| cmap.get(&pc).copied());
        if path.is_none() {
            style.path = None;
        }
        clip.texts.push((src, path));
    }
    // Bounds of everything copied, as drawn.
    let mut lo = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut add = |q: Vec2| {
        lo = Vec2::new(lo.x.min(q.x), lo.y.min(q.y));
        hi = Vec2::new(hi.x.max(q.x), hi.y.max(q.y));
    };
    let mut drawn: Vec<usize> = cs.clone();
    for t in &texts {
        drawn.extend(sk.link_curves(t));
    }
    for c in drawn {
        sk.polyline(c).into_iter().for_each(&mut add);
    }
    for &n in &clip.lone {
        if let Some(q) = clip.points.get(n) {
            add(*q);
        }
    }
    if !(lo.x.is_finite() && hi.x.is_finite()) {
        return Err(bad(cmd, "nothing to copy"));
    }
    clip.center = (lo + hi) * 0.5;
    clip.extent = (hi.x - lo.x).max(hi.y - lo.y);
    let out = json!({
        "curves": clip.curves.len() + texts.iter().map(|t| sk.link_curves(t).len()).sum::<usize>(),
        "points": clip.lone.len(),
        "texts": texts.len(),
        "constraints": clip.constraints.len(),
        "center": [clip.center.x, clip.center.y],
    });
    s.sketch_clipboard = Some(clip);
    Ok(out)
}

fn cut(s: &mut Session, p: &Value) -> Result<Value> {
    // `q["entities"] = …` below panics on anything but an object.
    if !(p.is_object() || p.is_null()) {
        return Err(bad("sketch.cut", "params must be an object"));
    }
    let entities = chosen(s, p);
    let mut q = if p.is_null() { json!({}) } else { p.clone() };
    q["entities"] = json!(entities);
    let out = copy(s, &q)?;
    let delete = super::find_command("sketch.delete").ok_or_else(|| bad("sketch.cut", "sketch.delete is missing"))?;
    let mut d = json!({ "entities": entities });
    if let Some(sk) = p.get("sketch") {
        d["sketch"] = sk.clone();
    }
    delete.run_nested(s, &d)?;
    s.selection.clear();
    // The cut geometry is gone: a paste back into this sketch goes where it was, not offset.
    if let Some(c) = s.sketch_clipboard.as_mut() {
        c.sketch = u64::MAX;
    }
    Ok(out)
}

/// The step a paste without a position moves by: a tenth of the copy's size, rounded up to 1, 2
/// or 5 times a power of ten (at least 1 mm).
fn paste_step(extent: f64) -> f64 {
    let want = (extent * 0.1).max(1.0);
    let mag = 10f64.powf(want.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|k| k * mag).find(|x| *x >= want - 1e-9).unwrap_or(want)
}

fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.paste";
    // `p["sketch"] = …` below panics on anything but an object.
    if !(p.is_object() || p.is_null()) {
        return Err(bad(cmd, "params must be an object"));
    }
    let Some(clip) = s.sketch_clipboard.clone() else { return Err(bad(cmd, "nothing copied (sketch.copy first)")) };
    let target = target_sketch(s, p, cmd)?;
    let positioned = p.get("at").is_some() || p.get("translate").is_some();
    let t = if let Some(v) = p.get("at") {
        vec2(v).ok_or_else(|| bad(cmd, "`at` must be [x, y]"))? - clip.center
    } else if let Some(v) = p.get("translate") {
        vec2(v).ok_or_else(|| bad(cmd, "`translate` must be [x, y]"))?
    } else if target == clip.sketch {
        let d = paste_step(clip.extent) * f64::from(clip.pastes.saturating_add(1));
        Vec2::new(d, -d)
    } else {
        Vec2::ZERO
    };
    if !(t.x.is_finite() && t.y.is_finite()) {
        return Err(bad(cmd, "the paste position must be finite"));
    }
    let mut p = p.clone();
    p["sketch"] = json!(target);
    let ((curves, lone, constraints), info) = edit(s, &p, cmd, false, |sk, doc| {
        if sk.curves.len().saturating_add(clip.curves.len()) > solidcraft_sketch::MAX_CURVES {
            return Err(solidcraft_sketch::SketchError::TooLarge.into());
        }
        let mut pts = Vec::with_capacity(clip.points.len());
        for q in &clip.points {
            pts.push(sk.add_point(*q + t, None)?);
        }
        let mut made = Vec::with_capacity(clip.curves.len());
        for c in &clip.curves {
            let mut kind = c.kind.clone();
            kind.map_points(&|q| pts.get(q).copied().unwrap_or(usize::MAX));
            let i = sk.add_curve(kind, None)?;
            if let Some(cu) = sk.curves.get_mut(i) {
                cu.construction = c.construction;
                cu.centerline = c.centerline;
                cu.reversed = c.reversed;
                cu.infinite = c.infinite;
            }
            made.push(i);
        }
        let mut cons = Vec::new();
        for c in &clip.constraints {
            let mut kind = c.kind.clone();
            if !kind.remap(&|q| pts.get(q).copied(), &|k| made.get(k).copied()) {
                continue;
            }
            // Each pasted dimension gets its own parameter, with the same expression.
            let param = c.param.as_ref().map(|(e, u)| doc.new_model_param(e, u));
            let id = sk.add_constraint(kind, param)?;
            if let Some(nc) = sk.constraints.last_mut() {
                nc.driven = c.driven;
                nc.text = c.text;
            }
            cons.push(id);
        }
        let mut curve_ids: Vec<String> = made.iter().filter_map(|i| sk.curves.get(*i).map(|c| c.id.clone())).collect();
        for (src, path) in &clip.texts {
            let mut src = src.clone();
            let LinkSource::Text { at, style, exprs, .. } = &mut src else { continue };
            style.path = path.and_then(|k| made.get(k)).and_then(|i| sk.curves.get(*i)).map(|c| c.id.clone());
            *at += t;
            // The copy sits where it was pasted: its place no longer follows the original's expressions.
            exprs.x = None;
            exprs.y = None;
            let geom = sk.text_source_geometry(&src)?;
            let link = sk.add_link(LinkKind::Text, src, &geom)?;
            curve_ids.extend(sk.link_curves(&link).into_iter().filter_map(|i| sk.curves.get(i).map(|c| c.id.clone())));
        }
        let lone: Vec<String> = clip.lone.iter().filter_map(|n| pts.get(*n)).filter_map(|i| sk.points.get(*i).map(|q| q.id.clone())).collect();
        Ok((curve_ids, lone, cons))
    })?;
    if !positioned
        && target == clip.sketch
        && let Some(c) = s.sketch_clipboard.as_mut()
    {
        c.pastes = c.pastes.saturating_add(1);
    }
    // The pasted geometry is selected, ready to move.
    if s.active_sketch == Some(target) {
        s.selection =
            curves.iter().map(|id| Sel::SketchCurve { id: id.clone() }).chain(lone.iter().map(|id| Sel::SketchPoint { id: id.clone() })).collect();
    }
    Ok(json!({"curves": curves, "points": lone, "constraints": constraints, "translate": [t.x, t.y], "sketch": info}))
}

#[cfg(test)]
#[path = "sketch_clipboard_tests.rs"]
mod tests;
