//! Hole stacks: holes that line up across touching parts, the places a rivet, bolt or pin can
//! pass through several layers at once.
//!
//! Each body's hole walls come from [`solidcraft_kernel::hole_walls`] (exact or B-spline
//! cylinders, so holes of refolded sheet parts count). A wall that closes is a round hole; two
//! half walls of one radius facing each other across a gap are the ends of an obround slot.
//! Round holes on one axis (within `axis_tolerance`) and the slots that axis passes through
//! are ordered along it; layers whose walls meet (gap at most `touching`) form a stack.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use super::CommandSpec;
use crate::params::{bad, bool_, num, string_list};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("inspect.hole_stacks", "Find Hole Stacks", hole_stacks).icon("measure").noundo().params(
    "bodies?: [names] (default all); diameters?: [[min, max]] (mm; holes and slot widths in any range); slots?: bool (default true: obround slots count); \
     touching?: mm (largest gap between layers, default 0.05); axis_tolerance?: mm (largest offset between hole axes, default 0.1) \
     → stacks: [{axis (unit, from start into the stack: −z for a vertical stack), start, end: [x,y,z] on the axis, grip_mm, \
     layers: [{body, d_mm, t: [t0, t1] (mm from start), slot?: {length_mm, along}}]}] (two or more bodies each); \
     lone: [{body, d_mm, at, axis, depth_mm, slot?}] (holes in no stack). Standard parts' bodies count only when named in bodies",
)];

/// Bodies and hole walls looked at, at most.
const MAX_BODIES: usize = 10_000;
const MAX_HOLES: usize = 100_000;

/// What to look for.
#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub bodies: Vec<String>,
    pub diameters: Vec<(f64, f64)>,
    pub slots: bool,
    pub touching: f64,
    pub axis_tolerance: f64,
}

impl Options {
    pub(crate) fn parse(cmd: &str, p: &Value) -> Result<Self> {
        let mm = |k: &str, default: f64, max: f64| -> Result<f64> {
            match p.get(k) {
                None | Some(Value::Null) => Ok(default),
                Some(_) => num(p, k).filter(|x| x.is_finite() && *x >= 0.0 && *x <= max).ok_or_else(|| bad(cmd, format!("`{k}` must be 0…{max} mm"))),
            }
        };
        let mut diameters = Vec::new();
        match p.get("diameters") {
            None | Some(Value::Null) => {}
            Some(Value::Array(ranges)) if ranges.len() <= 64 => {
                for r in ranges {
                    let pair = r.as_array().map(|a| a.iter().map(Value::as_f64).collect::<Vec<_>>());
                    match pair.as_deref() {
                        Some([Some(lo), Some(hi)]) if lo.is_finite() && hi.is_finite() && *lo >= 0.0 && lo <= hi => diameters.push((*lo, *hi)),
                        _ => return Err(bad(cmd, "`diameters` must be [[min, max], …] in mm, min ≤ max")),
                    }
                }
            }
            Some(_) => return Err(bad(cmd, "`diameters` must be a list of at most 64 [min, max] ranges")),
        }
        let slots = match p.get("slots") {
            None | Some(Value::Null) => true,
            Some(_) => bool_(p, "slots").ok_or_else(|| bad(cmd, "`slots` must be true or false"))?,
        };
        Ok(Self {
            bodies: string_list(p, "bodies"),
            diameters,
            slots,
            touching: mm("touching", 0.05, 10.0)?,
            axis_tolerance: mm("axis_tolerance", 0.1, 10.0)?,
        })
    }
}

/// A slot's shape across the axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SlotShape {
    /// Overall length (end to end).
    pub length: f64,
    /// Unit direction of its length.
    pub along: Vec3,
}

/// One hole of one body: a round hole, or an obround slot (`slot`), with its wall's extent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hole {
    pub body: String,
    /// The axis through the hole's centre (a slot's centre: halfway between its ends).
    pub point: Vec3,
    /// Unit axis, oriented the stack's way (see [`canonical`]).
    pub axis: Vec3,
    pub d: f64,
    /// The wall's extent along `axis` from `point`.
    pub t0: f64,
    pub t1: f64,
    pub slot: Option<SlotShape>,
}

/// One layer of a stack: the hole of one body.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layer {
    pub body: String,
    pub d: f64,
    /// Extent from the stack's start.
    pub t0: f64,
    pub t1: f64,
    pub slot: Option<SlotShape>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stack {
    pub axis: Vec3,
    pub start: Vec3,
    pub grip: f64,
    pub layers: Vec<Layer>,
}

impl Stack {
    pub(crate) fn end(&self) -> Vec3 {
        self.start + self.axis * self.grip
    }
    pub(crate) fn json(&self) -> Value {
        json!({
            "axis": round3(self.axis),
            "start": round3(self.start),
            "end": round3(self.end()),
            "grip_mm": round(self.grip),
            "layers": self.layers.iter().map(|l| {
                let mut v = json!({"body": l.body, "d_mm": round(l.d), "t": [round(l.t0), round(l.t1)]});
                if let Some(s) = l.slot {
                    v["slot"] = slot_json(s);
                }
                v
            }).collect::<Vec<_>>(),
        })
    }
}

/// Millimetres to a nanometre (and no negative zero): readable output, far finer than any fit.
pub(crate) fn round(x: f64) -> f64 {
    (x * 1e6).round() / 1e6 + 0.0
}

pub(crate) fn round3(v: Vec3) -> [f64; 3] {
    [round(v.x), round(v.y), round(v.z)]
}

fn slot_json(s: SlotShape) -> Value {
    json!({"length_mm": round(s.length), "along": round3(s.along)})
}

/// The stack's way along an axis: the largest component negative (−z for a vertical axis), so
/// a stack of plates lying on the XY plane starts at its top.
pub(crate) fn canonical(a: Vec3) -> Vec3 {
    let big = if a.x.abs() >= a.y.abs() && a.x.abs() >= a.z.abs() {
        a.x
    } else if a.y.abs() >= a.z.abs() {
        a.y
    } else {
        a.z
    };
    if big > 0.0 { -a } else { a }
}

/// The round holes and slots of the bodies `o` asks for, in world coordinates.
pub(crate) fn holes(s: &Session, o: &Options, cmd: &str) -> Result<Vec<Hole>> {
    let st = s.world_state();
    for name in &o.bodies {
        if st.body(name).is_none() {
            return Err(bad(cmd, format!("no body `{name}`")));
        }
    }
    // Standard parts already placed (a washer's or nut's hole) are not part of the assembly's
    // stacks unless asked for by name.
    let part = |b: &solidcraft_doc::ModelBody| s.doc.parts.contains_key(&s.doc.model_body_component(b));
    let bodies: Vec<_> = st.bodies.iter().filter(|b| if o.bodies.is_empty() { !part(b) } else { o.bodies.contains(&b.name) }).collect();
    if bodies.len() > MAX_BODIES {
        return Err(bad(cmd, format!("at most {MAX_BODIES} bodies (pass `bodies`)")));
    }
    let wanted = |d: f64| o.diameters.is_empty() || o.diameters.iter().any(|(lo, hi)| d >= lo - 1e-9 && d <= hi + 1e-9);
    let mut out = Vec::new();
    for b in bodies {
        let walls = solidcraft_kernel::hole_walls(&b.body, &b.mesh());
        let tol = (b.body.size() * 1e-4).max(1e-6) * 10.0;
        let full = std::f64::consts::TAU * (1.0 - 3.0 / 72.0);
        let mut halves = Vec::new();
        for w in &walls {
            let axis = canonical(w.axis);
            let (t0, t1) = if axis.dot(w.axis) > 0.0 { (w.start, w.end) } else { (-w.end, -w.start) };
            if w.sweep >= full {
                let d = 2.0 * w.radius;
                if wanted(d) {
                    out.push(Hole { body: b.name.clone(), point: w.axis_point, axis, d, t0, t1, slot: None });
                }
            } else if let Some(open) = w.opening
                && (w.sweep - std::f64::consts::PI).abs() < 0.6
            {
                halves.push((w, axis, t0, t1, open));
            }
        }
        if o.slots {
            // A slot: two half walls of one radius and extent, on parallel axes, open toward each other.
            let mut used = vec![false; halves.len()];
            for i in 0..halves.len() {
                for j in i + 1..halves.len() {
                    if used.get(i).copied().unwrap_or(true) || used.get(j).copied().unwrap_or(true) {
                        continue;
                    }
                    let (Some(&(a, axis, t0, t1, oa)), Some(&(b2, b_axis, u0, u1, ob))) = (halves.get(i), halves.get(j)) else { continue };
                    let (u0, u1) = if b_axis.dot(axis) >= 0.0 { (u0, u1) } else { (-u1, -u0) };
                    let gap = b2.axis_point - a.axis_point;
                    let across = gap - axis * gap.dot(axis);
                    let Some(dir) = across.normalized() else { continue };
                    let fits = a.axis.dot(b2.axis).abs() > 1.0 - 1e-6
                        && (a.radius - b2.radius).abs() < tol
                        && (t0 - (u0 + gap.dot(axis))).abs() < tol
                        && (t1 - (u1 + gap.dot(axis))).abs() < tol
                        && oa.dot(dir) > 0.9
                        && ob.dot(dir) < -0.9;
                    let d = 2.0 * a.radius;
                    if !fits || !wanted(d) {
                        continue;
                    }
                    for k in [i, j] {
                        if let Some(x) = used.get_mut(k) {
                            *x = true;
                        }
                    }
                    let point = a.axis_point + across * 0.5;
                    let slot = SlotShape { length: across.len() + d, along: dir };
                    out.push(Hole { body: b.name.clone(), point, axis, d, t0, t1, slot: Some(slot) });
                }
            }
        }
        if out.len() > MAX_HOLES {
            return Err(bad(cmd, format!("more than {MAX_HOLES} holes (pass `bodies` or `diameters`)")));
        }
    }
    Ok(out)
}

/// The distance, across `axis`, from the axis line through `p` to a hole's centre line (a
/// point for a round hole, a segment for a slot).
fn offset(p: Vec3, axis: Vec3, h: &Hole) -> f64 {
    let flat = |v: Vec3| v - axis * v.dot(axis);
    let w = flat(h.point - p);
    match h.slot {
        None => w.len(),
        Some(s) => {
            let half = ((s.length - h.d) / 2.0).max(0.0);
            let along = w.dot(s.along).clamp(-half, half);
            (w - s.along * along).len()
        }
    }
}

/// A hole's extent along `axis` measured from `p`: its own extent, turned round when its axis
/// points the other way (two near-diagonal axes can come out of [`canonical`] opposed).
fn extent(h: &Hole, p: Vec3, axis: Vec3) -> (f64, f64) {
    let base = (h.point - p).dot(axis);
    if h.axis.dot(axis) >= 0.0 { (base + h.t0, base + h.t1) } else { (base - h.t1, base - h.t0) }
}

/// Two hole axes are parallel, either way round.
fn parallel(a: Vec3, b: Vec3) -> bool {
    a.dot(b).abs() > 1.0 - 1e-5
}

/// Two slots on one axis: parallel, the same way across, one centre line, and overlapping.
fn same_slot_line(a: &Hole, b: &Hole, tol: f64) -> bool {
    let (Some(sa), Some(sb)) = (a.slot, b.slot) else { return false };
    if !parallel(a.axis, b.axis) || sa.along.dot(sb.along).abs() < 1.0 - 1e-4 {
        return false;
    }
    let w = b.point - a.point;
    let flat = w - a.axis * w.dot(a.axis);
    let along = flat.dot(sa.along);
    let across = (flat - sa.along * along).len();
    let (ha, hb) = (((sa.length - a.d) / 2.0).max(0.0), ((sb.length - b.d) / 2.0).max(0.0));
    across <= tol && along.abs() <= ha + hb + tol
}

/// Union-find root.
fn find(p: &mut [usize], mut i: usize) -> usize {
    for _ in 0..p.len() {
        match p.get(i).copied() {
            Some(up) if up != i => i = up,
            _ => break,
        }
    }
    i
}

fn join(p: &mut [usize], i: usize, j: usize) {
    let (ri, rj) = (find(p, i), find(p, j));
    if let Some(x) = p.get_mut(ri.max(rj)) {
        *x = ri.min(rj);
    }
}

/// The groups of a union-find over `ids`, each with its root first.
fn groups(parent: &mut [usize], ids: &[usize]) -> Vec<Vec<usize>> {
    let mut out: Vec<(usize, Vec<usize>)> = Vec::new();
    for &i in ids {
        let r = find(parent, i);
        match out.iter_mut().find(|(g, _)| *g == r) {
            Some((_, m)) => m.push(i),
            None => out.push((r, vec![i])),
        }
    }
    out.into_iter()
        .map(|(r, mut m)| {
            m.sort_by_key(|i| *i != r);
            m
        })
        .collect()
}

/// The stacks among `holes`, and the holes in none (by index).
pub(crate) fn stacks(holes: &[Hole], o: &Options) -> (Vec<Stack>, Vec<usize>) {
    let (rounds, slots): (Vec<usize>, Vec<usize>) = (0..holes.len()).partition(|i| holes.get(*i).is_some_and(|h| h.slot.is_none()));
    let mut parent: Vec<usize> = (0..holes.len()).collect();
    // Round holes on one axis.
    for (k, &i) in rounds.iter().enumerate() {
        for &j in rounds.iter().skip(k + 1) {
            let (Some(a), Some(b)) = (holes.get(i), holes.get(j)) else { continue };
            if parallel(a.axis, b.axis) && offset(a.point, a.axis, b) <= o.axis_tolerance {
                join(&mut parent, i, j);
            }
        }
    }
    // Each group: its axis, and its members.
    let mut lines: Vec<(Vec3, Vec3, Vec<usize>)> = Vec::new();
    let mut threaded = vec![false; holes.len()];
    for members in groups(&mut parent, &rounds) {
        let Some(h) = members.first().and_then(|i| holes.get(*i)) else { continue };
        let (p, axis) = (h.point, h.axis);
        let mut members = members;
        // The slots this axis runs through.
        for &i in &slots {
            if let Some(sl) = holes.get(i)
                && parallel(sl.axis, axis)
                && offset(p, axis, sl) <= o.axis_tolerance
            {
                members.push(i);
                if let Some(x) = threaded.get_mut(i) {
                    *x = true;
                }
            }
        }
        lines.push((p, axis, members));
    }
    // Slots that no round hole runs through, on one centre line: the axis through the middle
    // of the length they share.
    let free: Vec<usize> = slots.iter().copied().filter(|i| !threaded.get(*i).copied().unwrap_or(true)).collect();
    for (k, &i) in free.iter().enumerate() {
        for &j in free.iter().skip(k + 1) {
            let (Some(a), Some(b)) = (holes.get(i), holes.get(j)) else { continue };
            if same_slot_line(a, b, o.axis_tolerance) {
                join(&mut parent, i, j);
            }
        }
    }
    for members in groups(&mut parent, &free) {
        let Some(first) = members.first().and_then(|i| holes.get(*i)) else { continue };
        let Some(sa) = first.slot else { continue };
        let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
        for h in members.iter().filter_map(|i| holes.get(*i)) {
            let Some(s) = h.slot else { continue };
            let c = (h.point - first.point).dot(sa.along);
            let half = ((s.length - h.d) / 2.0).max(0.0);
            lo = lo.max(c - half);
            hi = hi.min(c + half);
        }
        let mid = if lo <= hi { (lo + hi) / 2.0 } else { 0.0 };
        lines.push((first.point + sa.along * mid, first.axis, members));
    }
    let mut in_stack = vec![false; holes.len()];
    let mut out = Vec::new();
    for (p, axis, members) in lines {
        // Along the axis, from the start.
        let mut pieces: Vec<(f64, f64, usize)> = members
            .iter()
            .filter_map(|&i| {
                let (t0, t1) = extent(holes.get(i)?, p, axis);
                Some((t0, t1, i))
            })
            .collect();
        pieces.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        // Runs of pieces whose walls meet.
        let mut runs: Vec<Vec<(f64, f64, usize)>> = Vec::new();
        let mut reach = f64::NEG_INFINITY;
        for piece in pieces {
            match runs.last_mut() {
                Some(run) if piece.0 <= reach + o.touching => {
                    run.push(piece);
                    reach = reach.max(piece.1);
                }
                _ => {
                    runs.push(vec![piece]);
                    reach = piece.1;
                }
            }
        }
        for run in runs {
            // One layer per body: a counterbore or two pieces of one wall are one layer, its
            // narrowest opening.
            let mut layers: Vec<(Layer, Vec<usize>)> = Vec::new();
            for (t0, t1, i) in &run {
                let Some(h) = holes.get(*i) else { continue };
                match layers.last_mut() {
                    Some((l, ids)) if l.body == h.body => {
                        l.t0 = l.t0.min(*t0);
                        l.t1 = l.t1.max(*t1);
                        if h.d < l.d {
                            l.d = h.d;
                            l.slot = h.slot;
                        }
                        ids.push(*i);
                    }
                    _ => layers.push((Layer { body: h.body.clone(), d: h.d, t0: *t0, t1: *t1, slot: h.slot }, vec![*i])),
                }
            }
            let bodies: std::collections::BTreeSet<&str> = layers.iter().map(|(l, _)| l.body.as_str()).collect();
            if bodies.len() < 2 {
                continue;
            }
            let t_start = layers.iter().map(|(l, _)| l.t0).fold(f64::INFINITY, f64::min);
            let t_end = layers.iter().map(|(l, _)| l.t1).fold(f64::NEG_INFINITY, f64::max);
            for (_, ids) in &layers {
                for i in ids {
                    if let Some(x) = in_stack.get_mut(*i) {
                        *x = true;
                    }
                }
            }
            let layers = layers
                .into_iter()
                .map(|(mut l, _)| {
                    l.t0 -= t_start;
                    l.t1 -= t_start;
                    l
                })
                .collect();
            out.push(Stack { axis, start: p + axis * t_start, grip: t_end - t_start, layers });
        }
    }
    let key = |v: Vec3| (round(v.x), round(v.y), round(v.z));
    out.sort_by(|a, b| {
        let (x, y) = (key(a.start), key(b.start));
        x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)).then(x.2.total_cmp(&y.2))
    });
    let lone = (0..holes.len()).filter(|i| !in_stack.get(*i).copied().unwrap_or(true)).collect();
    (out, lone)
}

fn hole_stacks(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.hole_stacks";
    let o = Options::parse(cmd, p)?;
    let holes = holes(s, &o, cmd)?;
    let (stacks, lone) = stacks(&holes, &o);
    let lone: Vec<Value> = lone
        .iter()
        .filter_map(|i| holes.get(*i))
        .map(|h| {
            let mut v = json!({"body": h.body, "d_mm": round(h.d), "at": round3(h.point + h.axis * h.t0), "axis": round3(h.axis), "depth_mm": round(h.t1 - h.t0)});
            if let Some(sl) = h.slot {
                v["slot"] = slot_json(sl);
            }
            v
        })
        .collect();
    Ok(json!({"stacks": stacks.iter().map(Stack::json).collect::<Vec<_>>(), "lone": lone}))
}

#[cfg(test)]
#[path = "hole_stacks_tests.rs"]
mod tests;
