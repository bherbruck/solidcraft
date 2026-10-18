//! Motion analysis: a study played sample by sample on a copy of the design, the way
//! `motion.play` places it (closed loops following on their branch). `motion.plot` answers the
//! joint values and a point's path with their velocities and accelerations; `motion.interference`
//! answers the pairs of parts that pass through each other along the way.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_doc::joints;
use solidcraft_doc::motion::MotionStudy;
use solidcraft_doc::{Document, Mat, apply_point, mat_mul};
use solidcraft_geom::{Aabb3, Vec3};

use super::contact_sweep::{ContactGeometry, Member};
use super::{drive_with_contact, joint_id, keyed, occurrences, path_world, study_index, traced_point, values_at};
use crate::params::{bad, bool_};
use crate::{EngineError, Result, Session};

pub const PLOT_DOC: &str = "study: id|name; joints?: [id|name] (default: every joint that moves); occurrence? and point? (a point \
     of a part to follow, as motion.trace takes them); samples? (3…1000, default the study's steps + 1); duration?: seconds the \
     whole study takes, 0.001 or more (velocities and accelerations then per second, else per step) → step: [the study step of each sample], \
     time?: [s], joints: [{joint, name, index, dof, unit, value, velocity, acceleration, velocity_unit, acceleration_unit}] \
     (degrees for angles, mm for distances; angles unwrapped), point?: {occurrence, position: [[x,y,z]…], velocity, \
     acceleration (vectors), speed, acceleration_norm, unit, velocity_unit, acceleration_unit}, samples (taken), stopped_at, \
     stopped_by_loop, stopped_by_contact. The study plays as motion.play plays it, each sample driven from the one before, on \
     a copy (the design is not changed); velocities and accelerations are finite differences between samples (central, \
     one-sided at the ends)";

pub const INTERFERENCE_DOC: &str = "study: id|name; occurrences?: [ids or names] (two or more, checked against each other; \
     default every occurrence), with against?: [ids or names] (then each of `occurrences` is checked only against these, such \
     as an arm against the parts it must miss), or contact_sets?: true (the pairs of the design's contact sets); samples? \
     (2…1000, default the study's steps + 1) → collisions: [{a, b, first_step, last_step, steps: [[from, to]…] (the step ranges where they overlap), \
     depth_mm (how far into each other, the most at deepest_step), volume_mm3 (their overlap there; null when it was not \
     cheap to work out)}], clear, \
     pairs (checked), samples (taken), truncated (the work budget ran out; checked_to: the last step checked), stopped_at, \
     stopped_by_loop. The study plays as motion.play plays it but contact sets do not stop it, on a copy (the design is not \
     changed). Parts are tested at the samples only: one that passes wholly through another between two samples is missed";

/// Most samples a plot or an interference check takes: each is a solve.
const MAX_SAMPLES: usize = 1000;
/// Most joint values one plot answers.
const MAX_SERIES: usize = 256;
/// Most collision work over a whole study (see `ContactGeometry::hits`).
const MAX_WORK: usize = 500_000_000;
/// Most colliding pairs whose overlap volume is worked out, and most body booleans that takes.
const MAX_VOLUMES: usize = 16;
const MAX_BOOLEANS: usize = 64;

/// How many samples `p` asks for (`min`…`MAX_SAMPLES`, default the study's steps + 1).
fn samples(p: &Value, m: &MotionStudy, min: usize, cmd: &str) -> Result<usize> {
    if m.steps == 0 {
        return Err(bad(cmd, "the study has no steps"));
    }
    match p.get("samples") {
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite() && *n >= min as f64 && *n <= MAX_SAMPLES as f64)
            .map(|n| n.round() as usize)
            .ok_or_else(|| bad(cmd, format!("`samples` must be {min}…{MAX_SAMPLES}"))),
        None => Ok((m.steps as usize).saturating_add(1).clamp(min, MAX_SAMPLES)),
    }
}

/// The study step sample `k` of `n` is at.
fn step_of(m: &MotionStudy, k: usize, n: usize) -> f64 {
    f64::from(m.steps) * k as f64 / n.saturating_sub(1).max(1) as f64
}

/// Plays study `m` on a copy of the design in `n` samples, each driven from the one before (so
/// closed loops follow on their branch, as `motion.play` places them; contact sets stop it only
/// when `contact`). `each` sees the copy at every sample and answers whether to go on. Returns
/// where the motion stopped short (all null when it did not).
fn play(s: &Session, m: &MotionStudy, n: usize, contact: bool, mut each: impl FnMut(&Session, usize) -> Result<bool>) -> Result<Value> {
    let mut t = s.scratch();
    // Played from where the parts are placed: moves only shown are not where joints are solved.
    t.pending_moves.clear();
    if !contact {
        t.doc_mut().assembly.contact_enabled = false;
    }
    let given = keyed(m);
    for k in 0..n {
        let want = values_at(&t, m, step_of(m, k, n));
        let (r, _) = drive_with_contact(&mut t, &want, &given)?;
        let go = each(&t, k)?;
        if r["stopped_by_loop"].is_object() || r["stopped_by_contact"].is_object() {
            return Ok(json!({"stopped_at": k, "stopped_by_loop": r["stopped_by_loop"], "stopped_by_contact": r["stopped_by_contact"]}));
        }
        if !go {
            break;
        }
    }
    Ok(json!({"stopped_at": null, "stopped_by_loop": null, "stopped_by_contact": null}))
}

/// The derivative of samples `v` spaced `h` apart: central differences inside, second-order
/// one-sided at the ends (a plain difference with two samples, 0 with one).
fn derivative(v: &[f64], h: f64) -> Vec<f64> {
    let n = v.len();
    let at = |i: usize| v.get(i).copied().unwrap_or(0.0);
    (0..n)
        .map(|k| match n {
            0 | 1 => 0.0,
            2 => (at(1) - at(0)) / h,
            _ if k == 0 => (-3.0 * at(0) + 4.0 * at(1) - at(2)) / (2.0 * h),
            _ if k + 1 == n => (3.0 * at(k) - 4.0 * at(k - 1) + at(k - 2)) / (2.0 * h),
            _ => (at(k + 1) - at(k - 1)) / (2.0 * h),
        })
        .collect()
}

/// The second derivative of samples `v` spaced `h` apart: central differences inside,
/// second-order one-sided at the ends (the one difference there is with three samples, 0 with
/// fewer).
fn second_derivative(v: &[f64], h: f64) -> Vec<f64> {
    let n = v.len();
    let at = |i: usize| v.get(i).copied().unwrap_or(0.0);
    let h2 = h * h;
    (0..n)
        .map(|k| match n {
            0..=2 => 0.0,
            3 => (at(0) - 2.0 * at(1) + at(2)) / h2,
            _ if k == 0 => (2.0 * at(0) - 5.0 * at(1) + 4.0 * at(2) - at(3)) / h2,
            _ if k + 1 == n => (2.0 * at(k) - 5.0 * at(k - 1) + 4.0 * at(k - 2) - at(k - 3)) / h2,
            _ => (at(k + 1) - 2.0 * at(k) + at(k - 1)) / h2,
        })
        .collect()
}

/// Angles in degrees made continuous: a jump of more than half a turn between samples is taken
/// as the angle going round (half a turn exactly is a real step).
fn unwrap_degrees(v: &mut [f64]) {
    let (mut turns, mut prev) = (0.0, None::<f64>);
    for x in v.iter_mut() {
        let raw = *x;
        if let Some(p) = prev
            && (raw - p).abs() > 180.0
        {
            turns -= 360.0 * ((raw - p) / 360.0).round();
        }
        prev = Some(raw);
        *x = raw + turns;
    }
}

/// The joint values a plot follows: (joint id, value index).
fn plotted(s: &Session, p: &Value, cmd: &str) -> Result<Vec<(u64, usize)>> {
    let joints = &s.doc.assembly.joints;
    let ids: Vec<u64> = match p.get("joints") {
        None | Some(Value::Null) => joints.iter().filter(|j| !j.suppressed).map(|j| j.id).collect(),
        Some(Value::Array(list)) if list.len() <= MAX_SERIES => list.iter().map(|v| joint_id(s, Some(v), cmd)).collect::<Result<_>>()?,
        Some(_) => return Err(bad(cmd, format!("`joints` must list at most {MAX_SERIES} joints"))),
    };
    let mut out = Vec::new();
    for id in ids {
        let dofs = joints.iter().find(|j| j.id == id).map_or(0, |j| j.kind.dofs().len());
        for i in 0..dofs {
            if !out.contains(&(id, i)) {
                out.push((id, i));
            }
        }
    }
    if out.len() > MAX_SERIES {
        return Err(bad(cmd, format!("more than {MAX_SERIES} joint values: list the `joints` to plot")));
    }
    Ok(out)
}

pub(super) fn plot(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "motion.plot";
    let m = s.doc.assembly.studies.get(study_index(s, p, cmd)?).cloned().ok_or_else(|| bad(cmd, "study"))?;
    let n = samples(p, &m, 3, cmd)?;
    let duration = match p.get("duration") {
        None | Some(Value::Null) => None,
        Some(v) => {
            // At least a millisecond: shorter sample spacings square to nothing.
            Some(v.as_f64().filter(|d| (1e-3..=1e9).contains(d)).ok_or_else(|| bad(cmd, "`duration` must be seconds, 0.001 or more"))?)
        }
    };
    let series = plotted(s, p, cmd)?;
    let given = keyed(&m);
    let point = if p.get("point").is_some() || p.get("occurrence").is_some() { Some(traced_point(s, p, cmd)?) } else { None };
    let mut values: Vec<Vec<f64>> = vec![Vec::new(); series.len()];
    let mut path: Vec<Vec3> = Vec::new();
    let mut taken = 0;
    let stop = play(s, &m, n, true, |t, _| {
        taken += 1;
        let now = joints::joint_values(&t.doc.assembly);
        for ((id, i), out) in series.iter().zip(values.iter_mut()) {
            out.push(now.get(id).and_then(|q| q.get(*i)).copied().unwrap_or(0.0));
        }
        if let Some((occ, local)) = &point {
            path.push(apply_point(&path_world(t, occ, cmd)?, *local));
        }
        Ok(true)
    })?;
    // Sample spacing in steps, or in seconds when the study has a duration.
    let seconds_per_step = duration.map(|d| d / f64::from(m.steps));
    let h = f64::from(m.steps) / (n - 1) as f64 * seconds_per_step.unwrap_or(1.0);
    let per = if duration.is_some() { "s" } else { "step" };
    let steps: Vec<f64> = (0..taken).map(|k| step_of(&m, k, n)).collect();
    let mut out = json!({"study": m.name, "steps": m.steps, "samples": taken, "step": steps});
    if let Some(sps) = seconds_per_step {
        out["time"] = steps.iter().map(|st| st * sps).collect();
    }
    let joints_out: Vec<Value> = series
        .iter()
        .zip(values.iter_mut())
        .map(|((id, i), v)| {
            let j = s.doc.assembly.joints.iter().find(|j| j.id == *id);
            let angle = j.is_some_and(|j| j.kind.is_angle(*i));
            let unit = if angle { "deg" } else { "mm" };
            if angle {
                v.iter_mut().for_each(|x| *x = x.to_degrees());
                // Keyed values are as the study sets them, and values beyond half a turn
                // (through motion links) already run on; a loop can hand back a follower's
                // angle within half a turn, a turn round.
                if !given.contains(&(*id, *i)) && v.iter().all(|x| x.abs() <= 180.0 + 1e-9) {
                    unwrap_degrees(v);
                }
            }
            json!({"joint": id, "name": j.map(|j| j.name.clone()), "index": i, "dof": j.and_then(|j| j.kind.dofs().get(*i).copied()),
                "unit": unit, "velocity_unit": format!("{unit}/{per}"), "acceleration_unit": format!("{unit}/{per}²"),
                "value": v, "velocity": derivative(v, h), "acceleration": second_derivative(v, h)})
        })
        .collect();
    out["joints"] = json!(joints_out);
    if let Some((occ, _)) = &point {
        let axis = |c: usize| -> Vec<f64> { path.iter().map(|q| [q.x, q.y, q.z].get(c).copied().unwrap_or(0.0)).collect() };
        let d: Vec<(Vec<f64>, Vec<f64>)> = (0..3).map(|c| (derivative(&axis(c), h), second_derivative(&axis(c), h))).collect();
        let vec_at = |k: usize, second: bool| -> Vec3 {
            let c = |i: usize| d.get(i).and_then(|(v, a)| if second { a.get(k) } else { v.get(k) }).copied().unwrap_or(0.0);
            Vec3::new(c(0), c(1), c(2))
        };
        let (vel, acc): (Vec<Vec3>, Vec<Vec3>) = (0..path.len()).map(|k| (vec_at(k, false), vec_at(k, true))).unzip();
        let xyz = |list: &[Vec3]| -> Value { list.iter().map(|v| json!([v.x, v.y, v.z])).collect() };
        let name = s.doc.occurrences.iter().find(|o| Some(&o.id) == occ.last()).map_or_else(|| "root".to_string(), |o| o.name.clone());
        out["point"] = json!({"occurrence": name, "unit": "mm", "velocity_unit": format!("mm/{per}"), "acceleration_unit": format!("mm/{per}²"),
            "position": xyz(&path), "velocity": xyz(&vel), "acceleration": xyz(&acc),
            "speed": vel.iter().map(|v| v.len()).collect::<Vec<_>>(), "acceleration_norm": acc.iter().map(|v| v.len()).collect::<Vec<_>>()});
    }
    for k in ["stopped_at", "stopped_by_loop", "stopped_by_contact"] {
        out[k] = stop[k].clone();
    }
    Ok(out)
}

/// One pair of parts that collides along a study.
struct Collision {
    names: (String, String),
    /// The samples where they overlap, in order.
    samples: Vec<usize>,
    depth: f64,
    deepest: usize,
    /// The design at the deepest sample (its placements).
    at_deepest: Arc<Document>,
}

/// The volume the parts under occurrences `a` and `b` share, placed as in `doc` (`None`: not
/// worked out). Only bodies whose boxes overlap are intersected, at most `booleans` of them.
fn shared_volume(s: &Session, doc: &Document, a: u64, b: u64, booleans: &mut usize) -> Option<f64> {
    let st = s.model.state();
    // The bodies under each (and not under the other), with their world frames and boxes.
    let mut sides: [Vec<(Mat, Aabb3, &solidcraft_doc::ModelBody)>; 2] = Default::default();
    for (comp, path, m) in doc.placements() {
        // A body belongs to the innermost of the two on its path, as contact sets pair them.
        let side = match path.iter().rev().find(|o| **o == a || **o == b) {
            Some(o) if *o == a => 0,
            Some(_) => 1,
            None => continue,
        };
        for body in st.bodies.iter().filter(|x| doc.model_body_component(x) == comp && !x.body.is_mesh()) {
            let frame = doc.model_body_offset(body).map(|o| mat_mul(&m, o)).unwrap_or(m);
            let local = body.mesh().bounds();
            let mut world = Aabb3::EMPTY;
            for c in 0..8 {
                let pick = |bit: usize, lo: f64, hi: f64| if c & bit == 0 { lo } else { hi };
                let corner = Vec3::new(pick(1, local.min.x, local.max.x), pick(2, local.min.y, local.max.y), pick(4, local.min.z, local.max.z));
                world.add(apply_point(&frame, corner));
            }
            sides.get_mut(side)?.push((frame, world, body));
        }
    }
    let mut total = 0.0;
    for (fa, xa, ba) in sides.first()? {
        for (fb, xb, bb) in sides.get(1)? {
            if xa.max.x < xb.min.x || xb.max.x < xa.min.x || xa.max.y < xb.min.y || xb.max.y < xa.min.y || xa.max.z < xb.min.z || xb.max.z < xa.min.z
            {
                continue;
            }
            *booleans = booleans.checked_sub(1)?;
            let (pa, pb) = (solidcraft_kernel::transform_matrix(&ba.body, *fa).ok()?, solidcraft_kernel::transform_matrix(&bb.body, *fb).ok()?);
            if let Some(shared) = solidcraft_kernel::boolean(&pa, &pb, solidcraft_kernel::BoolOp::Intersect).ok()? {
                total += solidcraft_kernel::measure(&shared).ok()?.volume;
            }
        }
    }
    Some(total)
}

/// Contiguous runs of samples, as study step ranges.
fn ranges(m: &MotionStudy, samples: &[usize], n: usize) -> Vec<[f64; 2]> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &k in samples {
        match out.last_mut() {
            Some(run) if run.1 + 1 == k => run.1 = k,
            _ => out.push((k, k)),
        }
    }
    out.into_iter().map(|(a, b)| [step_of(m, a, n), step_of(m, b, n)]).collect()
}

/// The pairs of parts a check looks at, as `motion.interference` and `joint.sweep` take them:
/// `occurrences` against each other (or each only `against` others), the design's contact sets
/// with `contact_sets`, else every occurrence.
pub(super) fn checked_pairs(s: &Session, p: &Value, cmd: &str) -> Result<ContactGeometry> {
    let everything = || vec![s.doc.occurrences.iter().map(|o| o.id).collect::<Vec<u64>>()];
    // Each of `occurrences` only against these.
    let against = match p.get("against") {
        None | Some(Value::Null) => None,
        Some(_) if p.get("occurrences").is_none() => return Err(bad(cmd, "`against` checks the `occurrences` against other parts: list them too")),
        Some(v) => Some(occurrences(s, &json!({"occurrences": v}), cmd)?),
    };
    let sets: Vec<Vec<u64>> = match (p.get("occurrences"), bool_(p, "contact_sets").unwrap_or(false)) {
        (Some(_), true) => return Err(bad(cmd, "give `occurrences` or `contact_sets`, not both")),
        (Some(_), false) => {
            let mut list = occurrences(s, p, cmd)?;
            if list.len() + against.as_ref().map_or(0, Vec::len) < 2 {
                return Err(bad(cmd, "list two or more occurrences to check against each other"));
            }
            for o in against.iter().flatten() {
                if !list.contains(o) {
                    list.push(*o);
                }
            }
            vec![list]
        }
        (None, true) => {
            let a = &s.doc.assembly;
            let sets: Vec<Vec<u64>> =
                if a.contact_all { everything() } else { a.contact_sets.iter().filter(|c| !c.suppressed).map(|c| c.occurrences.clone()).collect() };
            if sets.is_empty() {
                return Err(bad(cmd, "the design has no contact sets"));
            }
            sets
        }
        (None, false) => everything(),
    };
    let mut geometry = ContactGeometry::of_sets(s, &sets).map_err(|e| match e {
        EngineError::BadParams { msg, .. } => bad(cmd, format!("{msg} (check fewer parts: list the `occurrences`)")),
        e => e,
    })?;
    if let Some(b) = &against {
        let a = occurrences(s, p, cmd)?;
        geometry.keep_pairs(|x, y| (a.contains(&x) && b.contains(&y)) || (a.contains(&y) && b.contains(&x)));
    }
    Ok(geometry)
}

pub(super) fn interference(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "motion.interference";
    let m = s.doc.assembly.studies.get(study_index(s, p, cmd)?).cloned().ok_or_else(|| bad(cmd, "study"))?;
    let n = samples(p, &m, 2, cmd)?;
    let geometry = checked_pairs(s, p, cmd)?;
    let mut found: BTreeMap<(u64, u64), Collision> = BTreeMap::new();
    let (mut work, mut checked, mut truncated) = (0usize, 0usize, false);
    let stop = play(s, &m, n, false, |t, k| {
        // Placing the meshes is work too, also where nothing comes near.
        work = work.saturating_add(geometry.vertex_count());
        if work > MAX_WORK {
            truncated = true;
            return Ok(false);
        }
        let hits: Vec<(Member, Member, f64)> = match geometry.hits(t, &mut work, MAX_WORK, true) {
            Ok(h) => h,
            Err(_) if work > MAX_WORK => {
                truncated = true;
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        checked = k + 1;
        for ((a, an), (b, bn), depth) in hits {
            // Each pair once, the older occurrence (the lower id) first.
            let ((a, an), (b, bn)) = if a <= b { ((a, an), (b, bn)) } else { ((b, bn), (a, an)) };
            let c = found.entry((a, b)).or_insert_with(|| Collision {
                names: (an, bn),
                samples: Vec::new(),
                depth: 0.0,
                deepest: k,
                at_deepest: t.doc.clone(),
            });
            if c.samples.last() != Some(&k) {
                c.samples.push(k);
            }
            if depth > c.depth {
                (c.depth, c.deepest, c.at_deepest) = (depth, k, t.doc.clone());
            }
        }
        Ok(true)
    })?;
    // Nothing checked is not clear.
    if truncated && checked == 0 {
        return Err(bad(cmd, "the check ran out of its work budget before its first step: check fewer parts (occurrences, against)"));
    }
    let mut list: Vec<(&(u64, u64), &Collision)> = found.iter().collect();
    list.sort_by_key(|(_, c)| c.samples.first().copied().unwrap_or(usize::MAX));
    let mut booleans = MAX_BOOLEANS;
    let collisions: Vec<Value> = list
        .iter()
        .enumerate()
        .map(|(i, ((a, b), c))| {
            let volume = if i < MAX_VOLUMES { shared_volume(s, &c.at_deepest, *a, *b, &mut booleans) } else { None };
            let first = c.samples.first().map(|k| step_of(&m, *k, n));
            let last = c.samples.last().map(|k| step_of(&m, *k, n));
            json!({"a": c.names.0, "b": c.names.1, "first_step": first, "last_step": last, "steps": ranges(&m, &c.samples, n),
                "depth_mm": c.depth, "deepest_step": step_of(&m, c.deepest, n), "volume_mm3": volume})
        })
        .collect();
    let mut out = json!({"study": m.name, "steps": m.steps, "clear": collisions.is_empty(), "collisions": collisions,
        "pairs": geometry.pair_count(), "samples": checked, "truncated": truncated});
    if truncated {
        out["checked_to"] = json!(checked.checked_sub(1).map(|k| step_of(&m, k, n)));
    }
    out["stopped_at"] = stop["stopped_at"].clone();
    out["stopped_by_loop"] = stop["stopped_by_loop"].clone();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The differences are exact for a quadratic: its velocity and acceleration everywhere,
    /// the ends too.
    #[test]
    fn finite_differences_follow_a_polynomial() {
        let h = 0.5;
        let f = |x: f64| 2.0 * x * x - 3.0 * x + 1.0;
        let v: Vec<f64> = (0..7).map(|k| f(k as f64 * h)).collect();
        let d = derivative(&v, h);
        let a = second_derivative(&v, h);
        for (k, (dk, ak)) in d.iter().zip(&a).enumerate() {
            let x = k as f64 * h;
            assert!((dk - (4.0 * x - 3.0)).abs() < 1e-9, "{k}: {dk}");
            assert!((ak - 4.0).abs() < 1e-9, "{k}: {ak}");
        }
        assert_eq!(derivative(&[1.0, 3.0], 2.0), vec![1.0, 1.0]);
        assert_eq!(second_derivative(&[1.0, 3.0], 2.0), vec![0.0, 0.0]);
        assert_eq!(derivative(&[5.0], 1.0), vec![0.0]);
        assert!(derivative(&[], 1.0).is_empty());
    }

    #[test]
    fn angles_unwrap_across_half_turns() {
        let mut v = vec![170.0, 179.0, -172.0, -160.0, 175.0];
        unwrap_degrees(&mut v);
        assert_eq!(v, vec![170.0, 179.0, 188.0, 200.0, 175.0]);
        // Half a turn a sample is a step, not a wrap.
        let mut v = vec![0.0, 180.0, 360.0];
        unwrap_degrees(&mut v);
        assert_eq!(v, vec![0.0, 180.0, 360.0]);
    }
}
