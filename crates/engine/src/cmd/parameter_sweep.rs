//! Sweep a parameter and check that the design holds together at every value: that each feature
//! still builds, that every joint still finds its origins and none turned over, and that no
//! sketch plane flipped or sketch solve folded. A template driven by its parameters (one
//! design, many sizes) can be checked across its whole range before it is trusted, and the
//! report says what moved where, so a lost face or a flipped joint is found before a part is.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use super::CommandSpec;
use crate::params::{bad, num, str_};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("inspect.parameter_sweep", "Parameter Sweep", sweep).icon("params").noundo().params(
    "parameter: name; values: [number | expression] (at most 200) | from, to: numbers, steps? (default 10, at most 200: steps + 1 values) → \
     for each value, in a copy of the design (the design itself is not changed): {value, ms (the rebuild), ok, failed: [{feature, error}], warned: [{feature, warning}] \
     (features that build at the current value and fail or warn at this one), joints_lost: [{joint, name, why}], joints_flipped: [{joint, name, origin: a|b, degrees}] \
     (an origin's z turned more than 90° on its own part from where it is now), sketches_flipped: [{sketch, name, degrees}] (the sketch plane's normal turned more than 90°), \
     sketches_failed: [{sketch, name, failing, folded}] (a solve that fails, or keeps a triangle's turn or an offset's side only by flipping it)}; \
     summary: {values, ok, slowest_ms, failed, joints_lost, joints_flipped, sketches_flipped, sketches_failed} (how many values show each)",
)];

/// Values swept by one command, at most.
const MAX_VALUES: usize = 200;

/// A frame's origin and z axis, in its occurrence's coordinates.
type Frame = (Vec3, Vec3);

/// What the design looks like at one value.
struct Look {
    errors: BTreeMap<u64, (String, String)>,
    warnings: BTreeMap<u64, (String, String)>,
    /// Per joint: its name, its two origins (none: not found, with why).
    joints: BTreeMap<u64, (String, std::result::Result<Frame, String>, std::result::Result<Frame, String>)>,
    /// Joints the solve could not apply.
    unsolved: BTreeMap<u64, String>,
    /// Per sketch: its name, its plane's normal and whether its solve is sound (failing and
    /// folded constraints).
    sketches: BTreeMap<u64, (String, Vec3, Vec<String>, Vec<String>)>,
}

fn look(s: &Session) -> Look {
    let st = s.model.state();
    let mut errors = BTreeMap::new();
    let mut warnings = BTreeMap::new();
    for r in &s.model.results {
        if let Some(e) = &r.error {
            errors.insert(r.id, (r.name.clone(), e.clone()));
        }
        if let Some(w) = &r.warning {
            warnings.insert(r.id, (r.name.clone(), w.clone()));
        }
    }
    // In its own part's frame: an origin that turns over on its part is a lost reference; a part
    // the joints place anew is only its consequence.
    let frame = |o: &solidcraft_doc::joints::JointOrigin| -> std::result::Result<Frame, String> {
        let f = solidcraft_doc::joints::resolve_origin(&s.doc, &st, o).map_err(|e| e.to_string())?;
        Ok((Vec3::new(f[3][0], f[3][1], f[3][2]), Vec3::new(f[2][0], f[2][1], f[2][2])))
    };
    let joints = s.doc.assembly.joints.iter().filter(|j| !j.suppressed).map(|j| (j.id, (j.name.clone(), frame(&j.a), frame(&j.b)))).collect();
    let unsolved = if s.doc.assembly.joints.is_empty() {
        BTreeMap::new()
    } else {
        solidcraft_doc::joints::solve(&s.doc, &st).errors.into_iter().map(|(id, _, why)| (id, why)).collect()
    };
    let sketches = st
        .sketches
        .iter()
        .map(|k| {
            let failing = if k.report.status == solidcraft_sketch::SolveStatus::Failed { k.report.failing.clone() } else { Vec::new() };
            (k.feature, (k.name.clone(), k.plane.normal(), failing, k.report.folded.clone()))
        })
        .collect();
    Look { errors, warnings, joints, unsolved, sketches }
}

/// The angle between two directions, in degrees (none if either has no length).
fn degrees(a: Vec3, b: Vec3) -> Option<f64> {
    let (a, b) = (a.normalized()?, b.normalized()?);
    Some(a.dot(b).clamp(-1.0, 1.0).acos().to_degrees())
}

/// What changed from `base` (the design as it is) to `at` (at one value).
fn compare(base: &Look, at: &Look) -> Value {
    let failed: Vec<Value> = at
        .errors
        .iter()
        .filter(|(id, _)| !base.errors.contains_key(id))
        .map(|(id, (name, e))| json!({"feature": id, "name": name, "error": e}))
        .collect();
    let warned: Vec<Value> = at
        .warnings
        .iter()
        .filter(|(id, (_, w))| base.warnings.get(id).is_none_or(|(_, b)| b != w) && !at.errors.contains_key(id))
        .map(|(id, (name, w))| json!({"feature": id, "name": name, "warning": w}))
        .collect();
    let mut lost = Vec::new();
    let mut flipped = Vec::new();
    for (id, (name, a, b)) in &at.joints {
        let Some((_, a0, b0)) = base.joints.get(id) else { continue };
        for (side, now, was) in [("a", a, a0), ("b", b, b0)] {
            match (now, was) {
                (Err(why), Ok(_)) => lost.push(json!({"joint": id, "name": name, "origin": side, "why": why})),
                (Ok(f), Ok(f0)) => {
                    if let Some(d) = degrees(f.1, f0.1).filter(|d| *d > 90.0) {
                        flipped.push(json!({"joint": id, "name": name, "origin": side, "degrees": (d * 10.0).round() / 10.0}));
                    }
                }
                _ => {}
            }
        }
        if let (Some(why), None) = (at.unsolved.get(id), base.unsolved.get(id))
            && a.is_ok()
            && b.is_ok()
        {
            lost.push(json!({"joint": id, "name": name, "why": why}));
        }
    }
    let mut sketches_flipped = Vec::new();
    let mut sketches_failed = Vec::new();
    for (id, (name, n, failing, folded)) in &at.sketches {
        let Some((_, n0, failing0, folded0)) = base.sketches.get(id) else { continue };
        if let Some(d) = degrees(*n, *n0).filter(|d| *d > 90.0) {
            sketches_flipped.push(json!({"sketch": id, "name": name, "degrees": (d * 10.0).round() / 10.0}));
        }
        if (!failing.is_empty() && failing0.is_empty()) || (!folded.is_empty() && folded0.is_empty()) {
            sketches_failed.push(json!({"sketch": id, "name": name, "failing": failing, "folded": folded}));
        }
    }
    let ok = failed.is_empty() && lost.is_empty() && flipped.is_empty() && sketches_flipped.is_empty() && sketches_failed.is_empty();
    json!({"ok": ok, "failed": failed, "warned": warned, "joints_lost": lost, "joints_flipped": flipped,
           "sketches_flipped": sketches_flipped, "sketches_failed": sketches_failed})
}

/// The values to sweep.
fn values(p: &Value, cmd: &str) -> Result<Vec<Value>> {
    match (p.get("values"), p.get("from"), p.get("to")) {
        (Some(Value::Array(list)), None, None) => {
            if list.is_empty() || list.len() > MAX_VALUES {
                return Err(bad(cmd, format!("`values` must list 1…{MAX_VALUES} values")));
            }
            list.iter()
                .map(|v| match v {
                    Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => Ok(v.clone()),
                    Value::String(x) if !x.trim().is_empty() && x.len() <= 1000 => Ok(v.clone()),
                    _ => Err(bad(cmd, "each value is a number or an expression")),
                })
                .collect()
        }
        (None, Some(_), Some(_)) => {
            let finite = |k: &str| num(p, k).filter(|x| x.is_finite() && x.abs() <= 1e9).ok_or_else(|| bad(cmd, format!("`{k}` must be a number")));
            let (from, to) = (finite("from")?, finite("to")?);
            let steps = match p.get("steps") {
                None | Some(Value::Null) => 10,
                Some(_) => num(p, "steps")
                    .filter(|k| k.fract() == 0.0 && *k >= 1.0 && *k < MAX_VALUES as f64)
                    .ok_or_else(|| bad(cmd, format!("`steps` must be a whole number 1…{}", MAX_VALUES - 1)))? as usize,
            };
            Ok((0..=steps).map(|i| json!(from + (to - from) * i as f64 / steps as f64)).collect())
        }
        _ => Err(bad(cmd, "give `values`, or `from` and `to`")),
    }
}

fn sweep(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.parameter_sweep";
    let name = str_(p, "parameter").ok_or_else(|| bad(cmd, "`parameter` is required (parameters.list)"))?.to_string();
    if !super::parameters::known(s, &name) {
        return Err(bad(cmd, format!("no parameter `{name}` (parameters.list)")));
    }
    let values = values(p, cmd)?;
    let base = look(s);
    let mut results = Vec::new();
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    let mut slowest: f64 = 0.0;
    for v in values {
        let mut x = s.scratch();
        let t0 = std::time::Instant::now();
        let changed = x.execute("parameters.change", &json!({"name": name, "expression": v}));
        let ms = (t0.elapsed().as_secs_f64() * 1e4).round() / 10.0;
        slowest = slowest.max(ms);
        let mut r = match changed {
            Ok(_) => compare(&base, &look(&x)),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        };
        for key in ["failed", "joints_lost", "joints_flipped", "sketches_flipped", "sketches_failed"] {
            if r[key].as_array().is_some_and(|a| !a.is_empty()) {
                *count.entry(key).or_default() += 1;
            }
        }
        if r["ok"] == json!(true) {
            *count.entry("ok").or_default() += 1;
        }
        r["value"] = v;
        r["ms"] = json!(ms);
        results.push(r);
    }
    let get = |k: &str| count.get(k).copied().unwrap_or(0);
    Ok(json!({
        "parameter": name,
        "results": results,
        "summary": {"values": results.len(), "ok": get("ok"), "slowest_ms": slowest, "failed": get("failed"), "joints_lost": get("joints_lost"),
                    "joints_flipped": get("joints_flipped"), "sketches_flipped": get("sketches_flipped"), "sketches_failed": get("sketches_failed")},
    }))
}

#[cfg(test)]
#[path = "parameter_sweep_tests.rs"]
mod tests;
