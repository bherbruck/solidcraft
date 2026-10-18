//! Sweeping a joint until something touches: `joint.sweep` drives one joint value from `from`
//! toward `to` on a copy of the design (closed loops following on their branch), measures the
//! gaps between parts at each sample and bisects the first contact down to `refine`, so a hinge,
//! door or linkage answers how far it opens, what stops it and how much room each part has.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_doc::Document;

use super::analysis::checked_pairs;
use super::contact_sweep::{ContactGeometry, Gap};
use super::{drive_with_contact, joint_id, value};
use crate::params::{bad, bool_};
use crate::{Result, Session};

pub const SWEEP_DOC: &str = "joint: id|name; from, to: its value the sweep runs between (degrees for angles, mm for distances, \
     or expressions); index? (which value, default 0); step? (between samples, in the joint's unit; default: fine enough that \
     no part passes through another between two); refine?: mm (the first contact is bisected until no part moves more than \
     this between the last clear value and the touching one; default 0.01); occurrences?: [ids or names] (checked against \
     each other; default every occurrence), with against?: [ids or names] (the parts that stay: each of `occurrences` only \
     against these), or contact_sets?: true (the pairs of the design's contact sets); report_gaps?: bool (the nearest gap at \
     every sample, default false); apply?: bool (leave the design where the sweep stopped; default false: the design is not \
     changed) → stopped_at (the last value clear of contact, or where a loop or limit stopped it; null: it reached `to`), \
     stopped_by: {a, b, at: [x,y,z] (where they touch, world)} | \"loop\" | \"limit\" | null, samples: [{value, min_gap_mm?, \
     pair?}], min_gap? (report_gaps: {part: {mm, at_value, to}}, the nearest each part came to another), unit, gap_method \
     (\"mesh\": distances between the tessellations), gap_tolerance_mm (how far those can be off: 0 when every face is flat), \
     pairs (checked), touching_at_start: [[a, b]] (pairs already touching at `from`, such as parts joined face to face: not \
     checked), applied. The parts are tested at the samples and through the bisection; the motion is on a copy (closed loops \
     follow, contact sets do not stop it)";

/// Most samples a sweep takes before its bisection: each is a solve.
const MAX_SAMPLES: usize = 4096;
/// Most bisection steps: each halves the bracket around the first contact.
const MAX_BISECTIONS: usize = 64;
/// Most gap work over a whole sweep (see `ContactGeometry::gaps`).
const MAX_WORK: usize = 500_000_000;

/// The nearest a part came to another along the sweep.
struct Nearest {
    mm: f64,
    at: f64,
    to: String,
}

pub(super) fn sweep(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.sweep";
    let id = joint_id(s, p.get("joint"), cmd)?;
    let joint = s.doc.assembly.joints.iter().find(|j| j.id == id).ok_or_else(|| bad(cmd, "joint"))?;
    let (kind, name) = (joint.kind, joint.name.clone());
    let dofs = kind.dofs().len();
    if dofs == 0 {
        return Err(bad(cmd, "a rigid joint does not move"));
    }
    let index = match p.get("index") {
        None | Some(Value::Null) => 0,
        Some(v) => v
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| *i < dofs)
            .ok_or_else(|| bad(cmd, format!("`index` must be below {dofs}, the joint's value count")))?,
    };
    let angle = kind.is_angle(index);
    // Shown in degrees for angles, mm for distances.
    let shown = |v: f64| if angle { v.to_degrees() } else { v };
    let end = |key: &str| match p.get(key) {
        Some(v) => value(s, v, angle, cmd),
        None => Err(bad(cmd, format!("give `from` and `to`: the joint's values the sweep runs between (no `{key}`)"))),
    };
    let (from, to) = (end("from")?, end("to")?);
    let positive = |key: &str, max: f64| -> Result<Option<f64>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|x| x.is_finite() && *x > 0.0 && *x <= max)
                .map(Some)
                .ok_or_else(|| bad(cmd, format!("`{key}` must be a number above 0, at most {max}"))),
        }
    };
    let refine = positive("refine", 1e3)?.unwrap_or(0.01);
    let step = positive("step", 1e9)?.map(|x| if angle { x.to_radians() } else { x });
    let report_gaps = bool_(p, "report_gaps").unwrap_or(false);
    let apply = bool_(p, "apply").unwrap_or(false);

    let mut geometry = checked_pairs(s, p, cmd)?;
    if !geometry.has_pairs() {
        return Err(bad(cmd, "no pairs of parts to check: list two or more `occurrences` with bodies, or give `against`"));
    }
    // On a copy, played from where the parts are placed; contact sets do not stop it.
    let mut t = s.scratch();
    t.pending_moves.clear();
    t.doc_mut().assembly.contact_enabled = false;
    let given = BTreeSet::from([(id, index)]);
    let drive_to = |t: &mut Session, v: f64| -> Result<Value> {
        let mut q = t.doc.assembly.joints.iter().find(|j| j.id == id).map(|j| j.values.clone()).unwrap_or_default();
        q.resize(q.len().max(dofs), 0.0);
        if let Some(x) = q.get_mut(index) {
            *x = v;
        }
        Ok(drive_with_contact(t, &[(id, q)], &given)?.0)
    };
    let reached = |t: &Session| t.doc.assembly.joints.iter().find(|j| j.id == id).and_then(|j| j.values.get(index).copied()).unwrap_or(0.0);

    let r = drive_to(&mut t, from)?;
    if r["stopped_by_loop"].is_object() {
        return Err(bad(cmd, "the joint cannot reach `from`: a closed loop it is in does not close there"));
    }
    if (reached(&t) - from).abs() > 1e-9 {
        return Err(bad(cmd, "`from` is outside the joint's limits"));
    }
    let scale = geometry.scale();
    // Nearer than this is touching (well below any `refine`).
    let touch = 1e-6 * scale;
    let mut work = 0usize;
    // Pairs touching where the sweep starts (parts joined face to face, a door on its stop)
    // are not what stops it.
    let start: Vec<Gap> = geometry.gaps(&t, touch, &mut work, MAX_WORK)?;
    let touching: BTreeSet<(u64, u64)> = start.iter().map(|g| (g.a.0.min(g.b.0), g.a.0.max(g.b.0))).collect();
    let touching_names: Vec<Value> = start.iter().map(|g| json!([g.a.1, g.b.1])).collect();
    geometry.keep_pairs(|x, y| !touching.contains(&(x.min(y), x.max(y))));
    if !geometry.has_pairs() {
        return Err(bad(cmd, "every pair of parts checked already touches at `from`: list the parts that must stay clear"));
    }

    let n = match step {
        Some(h) => {
            let n = ((to - from).abs() / h).ceil();
            if !n.is_finite() || n > MAX_SAMPLES as f64 {
                return Err(bad(cmd, format!("`step` makes more than {MAX_SAMPLES} samples: take a larger step")));
            }
            n as usize
        }
        None => {
            let mut end = t.scratch();
            drive_to(&mut end, to)?;
            geometry.samples(&t, &end).map_err(|e| match e {
                crate::EngineError::BadParams { msg, .. } => bad(cmd, format!("{msg} (or give a `step`)")),
                e => e,
            })?
        }
    }
    .max(1);

    let mut samples: Vec<Value> = Vec::new();
    let mut nearest: BTreeMap<String, Nearest> = BTreeMap::new();
    let mut record = |t: &Session, v: f64, work: &mut usize, geometry: &ContactGeometry| -> Result<()> {
        if !report_gaps {
            samples.push(json!({"value": shown(v)}));
            return Ok(());
        }
        let gaps = geometry.gaps(t, f64::INFINITY, work, MAX_WORK)?;
        match gaps.first() {
            Some(g) => samples.push(json!({"value": shown(v), "min_gap_mm": g.mm, "pair": [g.a.1, g.b.1]})),
            None => samples.push(json!({"value": shown(v), "min_gap_mm": null, "pair": null})),
        }
        for g in &gaps {
            for (me, other) in [(&g.a.1, &g.b.1), (&g.b.1, &g.a.1)] {
                if nearest.get(me).is_none_or(|x| g.mm < x.mm) {
                    nearest.insert(me.clone(), Nearest { mm: g.mm, at: shown(v), to: other.clone() });
                }
            }
        }
        Ok(())
    };
    let contact = |t: &Session, work: &mut usize, geometry: &ContactGeometry| -> Result<Option<Gap>> {
        Ok(geometry.gaps(t, touch, work, MAX_WORK)?.into_iter().next())
    };

    record(&t, from, &mut work, &geometry)?;
    let (mut lo_doc, mut lo_v): (Arc<Document>, f64) = (t.doc.clone(), from);
    let mut stop: Option<(f64, Value)> = None;
    for k in 1..=n {
        let target = from + (to - from) * k as f64 / n as f64;
        let r = drive_to(&mut t, target)?;
        let v = reached(&t);
        if let Some(mut hit) = contact(&t, &mut work, &geometry)? {
            // Halve the bracket from the last clear value until no part moves more than
            // `refine` across it; each try is driven from the clear side, so loops keep their branch.
            let (mut hi_doc, mut hi_v) = (t.doc.clone(), v);
            for _ in 0..MAX_BISECTIONS {
                if geometry.travel(&lo_doc, &hi_doc)? <= refine {
                    break;
                }
                let mid = (lo_v + hi_v) / 2.0;
                t.doc = lo_doc.clone();
                drive_to(&mut t, mid)?;
                match contact(&t, &mut work, &geometry)? {
                    Some(h) => (hi_doc, hi_v, hit) = (t.doc.clone(), mid, h),
                    None => (lo_doc, lo_v) = (t.doc.clone(), mid),
                }
            }
            t.doc = lo_doc.clone();
            let at = (hit.at.0 + hit.at.1) * 0.5;
            stop = Some((lo_v, json!({"a": hit.a.1, "b": hit.b.1, "at": [at.x, at.y, at.z]})));
            break;
        }
        record(&t, v, &mut work, &geometry)?;
        if r["stopped_by_loop"].is_object() {
            stop = Some((v, json!("loop")));
            break;
        }
        if (v - target).abs() > 1e-9 {
            stop = Some((v, json!("limit")));
            break;
        }
        (lo_doc, lo_v) = (t.doc.clone(), v);
    }

    let applied = apply && !Arc::ptr_eq(&t.doc, &s.doc);
    if applied {
        let mut doc = (*t.doc).clone();
        doc.assembly.contact_enabled = s.doc.assembly.contact_enabled;
        *s.doc_mut() = doc;
    }
    let mut out = json!({
        "joint": id, "name": name, "index": index, "unit": if angle { "deg" } else { "mm" },
        "from": shown(from), "to": shown(to),
        "stopped_at": stop.as_ref().map(|(v, _)| shown(*v)),
        "stopped_by": stop.map(|(_, by)| by),
        "samples": samples,
        "refine_mm": refine,
        "gap_method": "mesh", "gap_tolerance_mm": geometry.chord(),
        "pairs": geometry.pair_count(),
        "touching_at_start": touching_names,
        "applied": applied,
    });
    if report_gaps {
        out["min_gap"] =
            nearest.into_iter().map(|(k, g)| (k, json!({"mm": g.mm, "at_value": g.at, "to": g.to}))).collect::<serde_json::Map<_, _>>().into();
    }
    Ok(out)
}
