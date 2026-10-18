//! Contact Sets, Motion Study and Exploded views.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use solidcraft_doc::Mat;
use solidcraft_doc::joints::{self, JointKind};
use solidcraft_doc::motion::{ContactSet, ExplodedView, MotionStudy, StudyKey};
use solidcraft_geom::Vec3;
use solidcraft_kernel::Body;

use super::CommandSpec;
use super::joints::{cannot_move, effective_values, joint_id, occurrence_id, resolve_holding, value};
use crate::params::{bad, bool_, str_, vec3};
use crate::{EngineError, Result, Session};

#[path = "motion_analysis.rs"]
mod analysis;
#[path = "motion_animation.rs"]
mod animation;
#[path = "contact_sweep.rs"]
mod contact_sweep;

macro_rules! trace_doc {
    () => {
        "occurrence: id|name, or an occurrence path [ids] (an instance inside sub-assemblies); point: [x,y,z] world point on it, where it is placed now; study: id|name, or joint: id|name with from and \
         to (degrees for angles, mm for distances, or expressions) and index? (which value, default 0); samples? (2…500, default \
         the study's steps + 1, or 100) - the point's path as the study plays or the joint is driven, closed loops following"
    };
}

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("contact.enable_sets", "Enable Contact Sets", enable_sets).at("SOLID", "ASSEMBLE").icon("contact_sets").params(
        "enabled?: bool (default true) — driving joints and motion studies stop where bodies in one contact set would pass through each other",
    ),
    CommandSpec::new("contact.enable_all", "Enable All Contact", enable_all)
        .at("SOLID", "ASSEMBLE")
        .icon("contact_all")
        .params("every occurrence takes part in contact (one set of all)"),
    CommandSpec::new("contact.disable_all", "Disable Contact", disable_all).at("SOLID", "ASSEMBLE").icon("contact_off").params("contact off"),
    CommandSpec::new("contact.create", "New Contact Set", new_set)
        .at("SOLID", "ASSEMBLE")
        .icon("contact_new")
        .params("occurrences: [ids or names] (two or more); name?"),
    CommandSpec::new("contact.edit", "Edit Contact Set", edit_set)
        .icon("edit")
        .params("set: id|name; occurrences?; name?; suppressed?: bool; delete?: bool"),
    CommandSpec::new("contact.list", "List Contact Sets", list_sets).icon("list").noundo().doc_only().params("→ sets, enabled, all"),
    CommandSpec::new("contact.check", "Check Contact", check)
        .icon("interference")
        .noundo()
        .params("→ pairs in contact sets that pass through each other now"),
    CommandSpec::new("motion.study", "Motion Study", study).at("SOLID", "ASSEMBLE").icon("play").params(
        "name?; study? (edit an existing one); steps? (timeline length, default 100); keys: [{joint, index?, points: [[step, value], …]}] \
         (values: degrees for angles, mm for distances, or expressions)",
    ),
    CommandSpec::new("motion.play", "Play Motion Study", play).icon("play").params(
        "study: id|name; step (0…steps) — sets the joints to the study's values there (contact sets stop them); quiet?: bool \
         (positions of only the occurrences that moved) → values: [{joint, values: [{name, value, unit}]}], positions, moved: whether \
         any occurrence moved, unmoved: [{joint, name, reason}] (keyed joints that can move nothing, such as a chain joined to \
         nothing grounded)",
    ),
    CommandSpec::new("motion.export", "Export Motion Study", export)
        .icon("export")
        .noundo()
        .params("study; samples? (default steps + 1); path? (.json or .csv) → occurrence positions per step (the design is not changed)"),
    CommandSpec::new("motion.export_animation", "Export Motion Animation", animation::export).icon("export").noundo().params(
        "study: id|name, or joint: id|name with from and to (degrees for angles, mm for distances, or expressions) and index?;          path: a .gif file, or a folder for PNG frames (frame-0001.png, frame-0002.png, …); samples? (frames, 2…1000; default the          study's steps + 1, or 100); width?, height? (16…4096 pixels, default 800 × 500); camera?: {target, yaw, pitch, roll?,          distance, fov?, up?} (ui.camera's reply; fields left out keep the default view: view.apply's camera, or the home view          framing the model); fps? (1…50, default 20); grid?, axes? (default true: the ground grid and the origin's axes) - draws the study as it plays, or the joint as it is driven, with the headless          renderer → path, format, frames, width, height, fps (GIF), bytes, renderer, stopped_at, stopped_by_loop,          stopped_by_contact (the design is not changed)",
    ),
    CommandSpec::new("motion.trace", "Motion Trace", trace).at("SOLID", "ASSEMBLE").icon("trace").noundo().params(concat!(
        trace_doc!(),
        " → points: [[x,y,z]…] world positions, samples (taken), stopped_at (the sample where the motion stopped short; the trace \
         ends there), stopped_by_loop, stopped_by_contact (the design is not changed)"
    )),
    CommandSpec::new("motion.trace.keep", "Keep Motion Trace", keep_trace).icon("trace").params(concat!(
        trace_doc!(),
        "; name? (default Motion TraceN) - keeps the trace as a 3D sketch spline through its points (a new sketch with 3D Sketch \
         on, in the root component, where no joint moves it; a fixed curve, it does not follow later changes) → sketch, name, curves, points"
    )),
    CommandSpec::new("motion.plot", "Plot Motion Study", analysis::plot).icon("graph").noundo().params(analysis::PLOT_DOC),
    CommandSpec::new("motion.interference", "Motion Interference", analysis::interference).icon("interference").noundo().params(analysis::INTERFERENCE_DOC),
    CommandSpec::new("motion.list", "List Motion Studies", list_studies).icon("list").noundo(),
    CommandSpec::new("motion.delete", "Delete Motion Study", delete_study).icon("delete").params("study"),
    CommandSpec::new("explode.create", "Exploded View", explode_create).at("SOLID", "ASSEMBLE").icon("explode").params(
        "name?; scale? (automatic: each occurrence pushed away from the assembly's centre by scale × its offset, default 1); \
         moves?: [{occurrence, translate: [x,y,z]}] (instead of automatic)",
    ),
    CommandSpec::new("explode.show", "Show Exploded View", explode_show)
        .icon("explode")
        .noundo()
        .params("view: id|name (or none: back to the assembled placement) — shown only; the design keeps its placements"),
    CommandSpec::new("explode.list", "List Exploded Views", explode_list).icon("list").noundo(),
    CommandSpec::new("explode.delete", "Delete Exploded View", explode_delete).icon("delete").params("view"),
];

const MAX_ITEMS: usize = 10_000;

fn next_id(s: &Session, cmd: &str) -> Result<u64> {
    let a = &s.doc.assembly;
    let ids = a.contact_sets.iter().map(|c| c.id).chain(a.studies.iter().map(|m| m.id)).chain(a.exploded.iter().map(|e| e.id));
    super::next_free_id(cmd, ids)
}

fn occurrences(s: &Session, p: &Value, cmd: &str) -> Result<Vec<u64>> {
    let list = p.get("occurrences").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`occurrences` must list occurrences"))?;
    if list.len() > MAX_ITEMS {
        return Err(bad(cmd, "too many occurrences"));
    }
    let mut out = Vec::new();
    for v in list {
        let id = occurrence_id(s, Some(v), cmd)?;
        if id == 0 {
            return Err(bad(cmd, "the root is not an occurrence"));
        }
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

// ---- Contact ----

/// The placed bodies of the design, each with the occurrences it sits in.
fn placed(s: &Session) -> Vec<(Vec<u64>, Body)> {
    let st = s.model.state();
    let mut out = Vec::new();
    for (comp, path, m) in s.doc.placements() {
        if comp == 0 {
            continue;
        }
        for b in st.bodies.iter().filter(|b| s.doc.model_body_component(b) == comp && !b.body.is_mesh()) {
            // Moving a body into another component preserves its world placement using an
            // offset from its authoring frame. Contact and explode centres must use that
            // same offset as display and physical properties.
            let frame = s.doc.model_body_offset(b).map(|offset| solidcraft_doc::mat_mul(&m, offset)).unwrap_or(m);
            if let Ok(t) = solidcraft_kernel::transform_matrix(&b.body, frame) {
                out.push((path.clone(), t));
            }
        }
    }
    out
}

/// Pairs of bodies in one active contact set that pass through each other: (occurrence names).
pub fn collisions(s: &Session) -> Vec<(String, String, f64)> {
    let a = &s.doc.assembly;
    if !a.contact_enabled {
        return Vec::new();
    }
    let sets: Vec<Vec<u64>> = if a.contact_all {
        vec![s.doc.occurrences.iter().map(|o| o.id).collect()]
    } else {
        a.contact_sets.iter().filter(|c| !c.suppressed).map(|c| c.occurrences.clone()).collect()
    };
    let bodies = placed(s);
    let name = |id: u64| s.doc.occurrences.iter().find(|o| o.id == id).map(|o| o.name.clone()).unwrap_or_default();
    let mut out = Vec::new();
    for set in &sets {
        // The member of the set each body belongs to (its innermost occurrence in the set).
        let tagged: Vec<(u64, &Body)> = bodies.iter().filter_map(|(path, b)| path.iter().rev().find(|o| set.contains(o)).map(|o| (*o, b))).collect();
        for (i, (oa, ba)) in tagged.iter().enumerate() {
            for (ob, bb) in tagged.iter().skip(i + 1) {
                if oa == ob {
                    continue;
                }
                if let Some(v) = overlap(ba, bb) {
                    out.push((name(*oa), name(*ob), v));
                }
            }
        }
    }
    out
}

/// How deep one body reaches into the other, when more than a touch (`None`: apart or
/// touching). Points of each tessellation (vertices, edge midpoints, triangle centres) are
/// tested against the other closed mesh: inside by ray parity, and deeper than the tolerance.
fn overlap(a: &Body, b: &Body) -> Option<f64> {
    let (ma, mb) = (a.tessellate((a.size() * 1e-3).max(1e-3)).ok()?, b.tessellate((b.size() * 1e-3).max(1e-3)).ok()?);
    mesh_overlap(&ma, &mb, 1e-4 * a.size().max(b.size()).max(1.0))
}

fn mesh_overlap(ma: &solidcraft_geom::Mesh, mb: &solidcraft_geom::Mesh, tol: f64) -> Option<f64> {
    let (ba, bb) = (ma.bounds(), mb.bounds());
    if ba.max.x < bb.min.x + tol
        || bb.max.x < ba.min.x + tol
        || ba.max.y < bb.min.y + tol
        || bb.max.y < ba.min.y + tol
        || ba.max.z < bb.min.z + tol
        || bb.max.z < ba.min.z + tol
    {
        return None;
    }
    let depth = deepest(ma, mb, tol).max(deepest(mb, ma, tol));
    (depth > tol).then_some(depth)
}

fn tris(m: &solidcraft_geom::Mesh) -> Vec<[Vec3; 3]> {
    m.triangles.iter().filter_map(|t| m.tri(t)).collect()
}

/// The deepest any sample point of `a` lies inside `b` (0 if none).
fn deepest(a: &solidcraft_geom::Mesh, b: &solidcraft_geom::Mesh, tol: f64) -> f64 {
    let tb = tris(b);
    let bx = b.bounds();
    let mut pts: Vec<Vec3> = a.positions.clone();
    for [p, q, r] in tris(a) {
        pts.extend([(p + q) * 0.5, (q + r) * 0.5, (r + p) * 0.5, (p + q + r) * (1.0 / 3.0)]);
    }
    let mut best = 0.0f64;
    for p in pts.into_iter().take(200_000) {
        if p.x < bx.min.x - tol
            || p.x > bx.max.x + tol
            || p.y < bx.min.y - tol
            || p.y > bx.max.y + tol
            || p.z < bx.min.z - tol
            || p.z > bx.max.z + tol
        {
            continue;
        }
        let d = tb.iter().map(|[a, b, c]| closest(p, *a, *b, *c).dist(p)).fold(f64::MAX, f64::min);
        if d > tol.max(best) && inside(p, &tb) {
            best = d;
        }
    }
    best
}

/// Ray parity along a skew direction (no axis-aligned edge hits).
fn inside(p: Vec3, tris: &[[Vec3; 3]]) -> bool {
    // Fixed ray coordinates, not an approximation used to compute Euler's constant.
    #[allow(clippy::approx_constant)]
    let dir = Vec3::new(0.577_215_664_9, 0.327_118_441_9, 0.751_891_438_7);
    let mut n = 0;
    for [a, b, c] in tris {
        let (e1, e2) = (*b - *a, *c - *a);
        let h = dir.cross(e2);
        let det = e1.dot(h);
        if det.abs() < 1e-14 {
            continue;
        }
        let f = 1.0 / det;
        let sv = p - *a;
        let u = f * sv.dot(h);
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = sv.cross(e1);
        let v = f * dir.dot(q);
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        if f * e2.dot(q) > 0.0 {
            n += 1;
        }
    }
    n % 2 == 1
}

/// The point of triangle abc nearest p.
fn closest(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let den = va + vb + vc;
    if den.abs() < 1e-300 {
        return a;
    }
    a + ab * (vb / den) + ac * (vc / den)
}

/// Set joint values, stopping short where contact sets would collide or a closed loop could no
/// longer close: the values are moved from where they are toward `want` as far as they go. The
/// `given` values (joint, index) are driven; in a closed loop the others follow. Returns the
/// answer and the last solve.
fn drive_with_contact(s: &mut Session, want: &[(u64, Vec<f64>)], given: &BTreeSet<(u64, usize)>) -> Result<(Value, joints::Solution)> {
    let baseline = s.doc.clone();
    let start: Vec<(u64, Vec<f64>)> =
        want.iter().map(|(id, _)| (*id, s.doc.assembly.joints.iter().find(|j| j.id == *id).map(|j| j.values.clone()).unwrap_or_default())).collect();
    // Closed loops follow the driven values from where they start.
    let held: joints::Held = given
        .iter()
        .filter(|&&(id, i)| want.iter().any(|(w, q)| *w == id && i < q.len()))
        .map(|&(id, i)| {
            // A value the file leaves out is 0, as the solver reads it.
            let from = start.iter().find(|(s, _)| *s == id).and_then(|(_, q)| q.get(i).copied());
            ((id, i), from.unwrap_or(0.0))
        })
        .collect();
    let set = |s: &mut Session, t: f64| {
        // Trial order must not affect free anchors or linked joint placements.
        s.doc = baseline.clone();
        let doc = s.doc_mut();
        for ((id, a), (_, b)) in start.iter().zip(want) {
            if let Some(j) = doc.assembly.joints.iter_mut().find(|j| j.id == *id) {
                let mut q: Vec<f64> = b.iter().enumerate().map(|(i, bv)| a.get(i).copied().unwrap_or(0.0) * (1.0 - t) + bv * t).collect();
                joints::clamp(j, &mut q);
                j.values = q;
            }
        }
        resolve_holding(s, &held)
    };
    // Loops already open at the start stay as they are; a loop the motion opens stops it.
    let open_at_start: BTreeSet<u64> = set(s, 0.0).conflicts.iter().map(|c| c.0).collect();
    let opened = |sol: &joints::Solution| sol.conflicts.iter().find(|c| !open_at_start.contains(&c.0)).map(|c| c.1.clone());
    let cache = if s.doc.assembly.contact_enabled { Some(contact_sweep::ContactGeometry::new(s)?) } else { None };
    let mut sol;
    let mut stopped = None;
    if let Some(cache) = cache.filter(|cache| cache.has_pairs()) {
        let mut end = s.scratch();
        set(&mut end, 1.0);
        let samples = cache.samples(s, &end)?;
        let mut work = 0;
        set(s, 0.0);
        // Keep the existing recovery behaviour: an initially overlapping assembly may move
        // out of its invalid placement. Sweeps constrain motions that start collision-free.
        if cache.first_hit(s, &mut work)?.is_none() {
            let mut lo = 0.0;
            let mut bracket = None;
            for sample in 1..=samples {
                let hi = sample as f64 / samples as f64;
                set(s, hi);
                if let Some(hit) = cache.first_hit(s, &mut work)? {
                    bracket = Some((lo, hi, hit));
                    break;
                }
                lo = hi;
            }
            if let Some((mut lo, mut hi, hit)) = bracket {
                for _ in 0..14 {
                    let mid = (lo + hi) / 2.0;
                    set(s, mid);
                    if cache.first_hit(s, &mut work)?.is_none() { lo = mid } else { hi = mid }
                }
                sol = set(s, lo);
                stopped = Some(json!({"a": hit.0, "b": hit.1, "fraction": lo}));
            } else {
                sol = set(s, 1.0);
            }
        } else {
            sol = set(s, 1.0);
        }
    } else {
        sol = set(s, 1.0);
    }
    let mut stopped_by_loop = None;
    if let Some(mut name) = opened(&sol) {
        // The furthest the loops still close, short of where contact stopped; the joint named is
        // the one whose loop opens first.
        let (mut lo, mut hi) = (0.0, stopped.as_ref().and_then(|v| v["fraction"].as_f64()).unwrap_or(1.0));
        for _ in 0..14 {
            let mid = (lo + hi) / 2.0;
            match opened(&set(s, mid)) {
                None => lo = mid,
                Some(n) => (hi, name) = (mid, n),
            }
        }
        sol = set(s, lo);
        stopped = None;
        stopped_by_loop = Some(json!({"joint": name, "fraction": lo}));
    }
    let values: Vec<Value> = want.iter().map(|(id, _)| json!({"joint": id, "values": effective_values(s, *id)})).collect();
    let out = json!({"values": values, "stopped_by_contact": stopped, "stopped_by_loop": stopped_by_loop, "conflicts": sol.conflicts.len()});
    Ok((out, sol))
}

/// Drive one joint (joint.drive), honouring contact sets.
pub fn drive_joint(s: &mut Session, id: u64, q: Vec<f64>, given: usize) -> Result<Value> {
    let given = (0..given.min(q.len())).map(|i| (id, i)).collect();
    Ok(drive_with_contact(s, &[(id, q)], &given)?.0)
}

fn enable_sets(s: &mut Session, p: &Value) -> Result<Value> {
    let on = bool_(p, "enabled").unwrap_or(true);
    s.doc_mut().assembly.contact_enabled = on;
    Ok(json!({"enabled": on}))
}

fn enable_all(s: &mut Session, _p: &Value) -> Result<Value> {
    let a = &mut s.doc_mut().assembly;
    a.contact_enabled = true;
    a.contact_all = true;
    Ok(json!({"enabled": true, "all": true}))
}

fn disable_all(s: &mut Session, _p: &Value) -> Result<Value> {
    let a = &mut s.doc_mut().assembly;
    a.contact_enabled = false;
    a.contact_all = false;
    Ok(json!({"enabled": false}))
}

fn new_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "contact.create";
    let occ = occurrences(s, p, cmd)?;
    if occ.len() < 2 {
        return Err(bad(cmd, "a contact set needs two or more occurrences"));
    }
    if s.doc.assembly.contact_sets.len() >= MAX_ITEMS {
        return Err(bad(cmd, "too many contact sets"));
    }
    let id = next_id(s, cmd)?;
    let name = str_(p, "name").map(str::to_string).unwrap_or_else(|| format!("Contact Set{}", s.doc.assembly.contact_sets.len() + 1));
    let a = &mut s.doc_mut().assembly;
    a.contact_sets.push(ContactSet { id, name: name.clone(), occurrences: occ, suppressed: false });
    a.contact_enabled = true;
    Ok(json!({"set": id, "name": name}))
}

fn set_index(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    let key = match p.get("set") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, "`set` must be an id or a name")),
    };
    s.doc
        .assembly
        .contact_sets
        .iter()
        .position(|c| c.id.to_string() == key || c.name == key)
        .ok_or_else(|| bad(cmd, format!("no contact set `{key}`")))
}

fn edit_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "contact.edit";
    let i = set_index(s, p, cmd)?;
    if bool_(p, "delete").unwrap_or(false) {
        s.doc_mut().assembly.contact_sets.remove(i);
        return Ok(json!({"deleted": true}));
    }
    let occ = if p.get("occurrences").is_some() { Some(occurrences(s, p, cmd)?) } else { None };
    if occ.as_ref().is_some_and(|o| o.len() < 2) {
        return Err(bad(cmd, "a contact set needs two or more occurrences"));
    }
    let Some(c) = s.doc_mut().assembly.contact_sets.get_mut(i) else { return Err(bad(cmd, "set")) };
    if let Some(o) = occ {
        c.occurrences = o;
    }
    if let Some(n) = str_(p, "name").filter(|n| !n.trim().is_empty()) {
        c.name = n.to_string();
    }
    if let Some(x) = bool_(p, "suppressed") {
        c.suppressed = x;
    }
    Ok(json!({"set": c.id}))
}

fn list_sets(s: &mut Session, _p: &Value) -> Result<Value> {
    let a = &s.doc.assembly;
    Ok(json!({"enabled": a.contact_enabled, "all": a.contact_all, "sets": a.contact_sets}))
}

fn check(s: &mut Session, _p: &Value) -> Result<Value> {
    let c = collisions(s);
    Ok(json!({"collisions": c.iter().map(|(a, b, d)| json!({"a": a, "b": b, "depth_mm": d})).collect::<Vec<_>>()}))
}

// ---- Motion studies ----

fn study_index(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    // A number is an id first, text a name first: a study may be named like another's id.
    let (key, id_first) = match p.get("study") {
        Some(Value::Number(n)) => (n.to_string(), true),
        Some(Value::String(x)) => (x.clone(), false),
        _ => return Err(bad(cmd, "`study` must be an id or a name")),
    };
    let studies = &s.doc.assembly.studies;
    let (by_id, by_name) = (studies.iter().position(|m| m.id.to_string() == key), studies.iter().position(|m| m.name == key));
    if id_first { by_id.or(by_name) } else { by_name.or(by_id) }.ok_or_else(|| bad(cmd, format!("no motion study `{key}`")))
}

fn study(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "motion.study";
    let existing = if p.get("study").is_some() { Some(study_index(s, p, cmd)?) } else { None };
    let steps = match p.get("steps") {
        Some(v) => {
            let n = v.as_f64().filter(|n| n.is_finite() && *n >= 1.0 && *n <= 100_000.0).ok_or_else(|| bad(cmd, "`steps` must be 1…100000"))?;
            Some(n.round() as u32)
        }
        None => None,
    };
    let mut keys = Vec::new();
    if let Some(list) = p.get("keys") {
        let list = list.as_array().ok_or_else(|| bad(cmd, "`keys` must be a list"))?;
        if list.len() > 1000 {
            return Err(bad(cmd, "too many keys"));
        }
        for k in list {
            let joint = joint_id(s, k.get("joint"), cmd)?;
            let kind = s.doc.assembly.joints.iter().find(|j| j.id == joint).map(|j| j.kind).unwrap_or(JointKind::Rigid);
            let index = k.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if index >= kind.dofs().len() {
                return Err(bad(cmd, format!("joint {joint} has no moving value {index}")));
            }
            let pts = k.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "each key needs `points`: [[step, value], …]"))?;
            if pts.is_empty() || pts.len() > 10_000 {
                return Err(bad(cmd, "a key needs 1…10000 points"));
            }
            let mut points = Vec::new();
            for pt in pts {
                let (Some(st), Some(v)) = (pt.get(0).and_then(Value::as_f64).filter(|x| x.is_finite() && x.abs() < 1e9), pt.get(1)) else {
                    return Err(bad(cmd, "points are [step, value]"));
                };
                points.push((st, value(s, v, kind.is_angle(index), cmd)?));
            }
            points.sort_by(|a, b| a.0.total_cmp(&b.0));
            keys.push(StudyKey { joint, index, points });
        }
    }
    let id = next_id(s, cmd)?;
    let count = s.doc.assembly.studies.len();
    let a = &mut s.doc_mut().assembly;
    let out = match existing.and_then(|i| a.studies.get_mut(i)) {
        Some(m) => {
            if let Some(n) = steps {
                m.steps = n;
            }
            if p.get("keys").is_some() {
                m.keys = keys;
            }
            if let Some(n) = str_(p, "name").filter(|n| !n.trim().is_empty()) {
                m.name = n.to_string();
            }
            m.clone()
        }
        None => {
            if count >= MAX_ITEMS {
                return Err(bad(cmd, "too many motion studies"));
            }
            let m = MotionStudy {
                id,
                name: str_(p, "name").map(str::to_string).unwrap_or_else(|| format!("Motion Study{}", count + 1)),
                steps: steps.unwrap_or(100),
                keys,
            };
            a.studies.push(m.clone());
            m
        }
    };
    Ok(json!({"study": out.id, "name": out.name, "steps": out.steps, "keys": out.keys.len()}))
}

/// Joint values of a study at a step: (joint, full value vector).
fn values_at(s: &Session, m: &MotionStudy, step: f64) -> Vec<(u64, Vec<f64>)> {
    let mut out: Vec<(u64, Vec<f64>)> = Vec::new();
    for k in &m.keys {
        let Some(v) = k.value_at(step) else { continue };
        let pos = match out.iter().position(|(j, _)| *j == k.joint) {
            Some(i) => i,
            None => {
                let cur = s.doc.assembly.joints.iter().find(|j| j.id == k.joint).map(|j| j.values.clone()).unwrap_or_default();
                out.push((k.joint, cur));
                out.len() - 1
            }
        };
        if let Some((_, q)) = out.get_mut(pos) {
            if q.len() <= k.index {
                q.resize(k.index + 1, 0.0);
            }
            if let Some(x) = q.get_mut(k.index) {
                *x = v;
            }
        }
    }
    out
}

/// The values a study keys: (joint, index).
fn keyed(m: &MotionStudy) -> BTreeSet<(u64, usize)> {
    m.keys.iter().map(|k| (k.joint, k.index)).collect()
}

/// Where each occurrence is: (name, world transform).
pub(super) fn positions(s: &Session) -> Vec<(String, Mat)> {
    s.doc.occurrences.iter().map(|o| (o.name.clone(), joints::occurrence_world(&s.doc, o.id))).collect()
}

/// The occurrences placed differently `after` than `before` ([`positions`] of one design, so in
/// the same order even where two share a name).
pub(super) fn moved<'a>(before: &'a [(String, Mat)], after: &'a [(String, Mat)]) -> impl Iterator<Item = &'a (String, Mat)> {
    after.iter().zip(before).filter(|((_, a), (_, b))| a.iter().flatten().zip(b.iter().flatten()).any(|(x, y)| (x - y).abs() > 1e-9)).map(|(a, _)| a)
}

fn pose_json(m: &Mat) -> Value {
    json!({"origin": [m[3][0], m[3][1], m[3][2]], "x": [m[0][0], m[0][1], m[0][2]], "y": [m[1][0], m[1][1], m[1][2]], "z": [m[2][0], m[2][1], m[2][2]]})
}

fn play(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "motion.play";
    let i = study_index(s, p, cmd)?;
    let m = s.doc.assembly.studies.get(i).cloned().ok_or_else(|| bad(cmd, "study"))?;
    let step = p.get("step").and_then(Value::as_f64).filter(|x| x.is_finite()).ok_or_else(|| bad(cmd, "`step` must be a number"))?;
    let want = values_at(s, &m, step.clamp(0.0, f64::from(m.steps)));
    let before = positions(s);
    let (mut out, sol) = drive_with_contact(s, &want, &keyed(&m))?;
    let after = positions(s);
    let moved: Vec<&(String, Mat)> = moved(&before, &after).collect();
    out["moved"] = json!(!moved.is_empty());
    let shown: Vec<&(String, Mat)> = if bool_(p, "quiet").unwrap_or(false) { moved } else { after.iter().collect() };
    out["positions"] = shown.iter().map(|(n, t)| json!({"occurrence": n, "pose": pose_json(t)})).collect();
    let joints: BTreeSet<u64> = m.keys.iter().map(|k| k.joint).collect();
    out["unmoved"] = joints
        .iter()
        .filter_map(|id| {
            let (why, _) = cannot_move(s, &sol, *id)?;
            let name = s.doc.assembly.joints.iter().find(|j| j.id == *id).map(|j| j.name.clone());
            Some(json!({"joint": id, "name": name, "reason": why}))
        })
        .collect();
    Ok(out)
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "motion.export";
    let i = study_index(s, p, cmd)?;
    let m = s.doc.assembly.studies.get(i).cloned().ok_or_else(|| bad(cmd, "study"))?;
    let samples = match p.get("samples") {
        Some(v) => v.as_f64().filter(|n| n.is_finite() && *n >= 2.0 && *n <= 10_000.0).ok_or_else(|| bad(cmd, "`samples` must be 2…10000"))? as usize,
        None => (m.steps as usize + 1).min(10_000),
    };
    // On a copy: the design stays as it is.
    let mut t = s.scratch();
    let mut rows = Vec::new();
    for k in 0..samples {
        let step = f64::from(m.steps) * k as f64 / (samples - 1) as f64;
        let want = values_at(&t, &m, step);
        let (r, _) = drive_with_contact(&mut t, &want, &keyed(&m))?;
        rows.push(json!({
            "step": step,
            "stopped_by_contact": r["stopped_by_contact"],
            "stopped_by_loop": r["stopped_by_loop"],
            "positions": positions(&t).iter().map(|(n, x)| json!({"occurrence": n, "pose": pose_json(x)})).collect::<Vec<_>>(),
        }));
    }
    let out = json!({"study": m.name, "steps": m.steps, "samples": rows});
    if let Some(path) = str_(p, "path").filter(|x| !x.trim().is_empty() && x.len() < 4096) {
        let text = if path.to_ascii_lowercase().ends_with(".csv") {
            let mut c = String::from("step,occurrence,x,y,z,xx,xy,xz,yx,yy,yz,zx,zy,zz\n");
            for r in &rows {
                for p in r["positions"].as_array().into_iter().flatten() {
                    let pose = &p["pose"];
                    let n = |k: &str, i: usize| pose[k][i].as_f64().unwrap_or(0.0);
                    c += &format!(
                        "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                        r["step"],
                        p["occurrence"].as_str().unwrap_or_default().replace(',', " "),
                        n("origin", 0),
                        n("origin", 1),
                        n("origin", 2),
                        n("x", 0),
                        n("x", 1),
                        n("x", 2),
                        n("y", 0),
                        n("y", 1),
                        n("y", 2),
                        n("z", 0),
                        n("z", 1),
                        n("z", 2)
                    );
                }
            }
            c
        } else {
            serde_json::to_string_pretty(&out).unwrap_or_default()
        };
        solidcraft_io::vfs::check(path).map_err(|e| EngineError::Other(e.to_string()))?;
        solidcraft_io::write_atomic(std::path::Path::new(path), text.as_bytes(), false).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        return Ok(json!({"path": path, "samples": rows.len()}));
    }
    Ok(out)
}

/// Most samples of a motion trace: each is a solve, and a kept trace keeps every one as a fit
/// point of its spline.
const MAX_TRACE: usize = solidcraft_sketch::MAX_FIT_POINTS;

/// The joint values a trace drives to at sample `k` of `n`.
type Plan = Box<dyn Fn(&Session, usize, usize) -> Vec<(u64, Vec<f64>)>>;

/// The point `p` names (`occurrence` and `point`): the occurrence path placing it and the point
/// in that placement's frame. The occurrence is an occurrence path through nested sub-assemblies
/// (one instance of each), or an occurrence at its first placement (none: the root). The point
/// is a world point where the parts are placed (moves only shown are left out).
fn traced_point(s: &Session, p: &Value, cmd: &str) -> Result<(Vec<u64>, Vec3)> {
    let path: Vec<u64> = match p.get("occurrence") {
        Some(Value::Array(ids)) if ids.len() <= 64 => {
            ids.iter().map(|v| v.as_u64().ok_or_else(|| bad(cmd, "an occurrence path lists occurrence ids"))).collect::<Result<_>>()?
        }
        Some(Value::Array(_)) => return Err(bad(cmd, "an occurrence path has at most 64 ids")),
        v => {
            let occ = occurrence_id(s, v, cmd)?;
            match s.doc.occurrences.iter().find(|o| o.id == occ) {
                Some(o) => s.doc.first_path(o.parent).into_iter().chain([o.id]).collect(),
                None => Vec::new(),
            }
        }
    };
    let at = p.get("point").and_then(vec3).filter(|v| v.len() < 1e7).ok_or_else(|| bad(cmd, "`point` must be [x, y, z], a world point"))?;
    let mut t = s.scratch();
    t.pending_moves.clear();
    let local = solidcraft_doc::mat_inverse(&path_world(&t, &path, cmd)?)
        .map(|m| solidcraft_doc::apply_point(&m, at))
        .ok_or_else(|| bad(cmd, "the occurrence's placement is not invertible"))?;
    Ok((path, local))
}

/// The world placement of an occurrence path (empty: the root).
fn path_world(t: &Session, path: &[u64], cmd: &str) -> Result<Mat> {
    if path.is_empty() { Ok(solidcraft_doc::IDENTITY) } else { t.occurrence_placement(path).map(|o| o.world).map_err(|e| bad(cmd, e.to_string())) }
}

/// Where a point of an occurrence goes as a study plays or a joint is driven: its world
/// positions, sampled on a copy (the design is not changed). Each sample is driven from the one
/// before, so closed loops follow on their branch and contact sets stop them; the trace ends at
/// the sample where the motion stops short. Returns the points and the answer without them.
fn sample_trace(s: &Session, p: &Value, cmd: &str) -> Result<(Vec<Vec3>, Value)> {
    let (path, local) = traced_point(s, p, cmd)?;
    let mut t = s.scratch();
    // Traced from where the parts are placed: moves only shown (an exploded view, a move not
    // captured yet) are not where the joints are solved.
    t.pending_moves.clear();
    let world = |t: &Session| path_world(t, &path, cmd);
    let (default, given, plan) = motion_plan(s, p, cmd)?;
    let n = sample_count(p, default.min(MAX_TRACE), MAX_TRACE, cmd)?;
    let mut points = Vec::new();
    let mut stop = (Value::Null, Value::Null, Value::Null);
    for k in 0..n {
        let want = plan(&t, k, n);
        let (r, _) = drive_with_contact(&mut t, &want, &given)?;
        points.push(solidcraft_doc::apply_point(&world(&t)?, local));
        if r["stopped_by_loop"].is_object() || r["stopped_by_contact"].is_object() {
            stop = (json!(k), r["stopped_by_loop"].clone(), r["stopped_by_contact"].clone());
            break;
        }
    }
    let name = s.doc.occurrences.iter().find(|o| Some(&o.id) == path.last()).map_or_else(|| "root".to_string(), |o| o.name.clone());
    let out = json!({"occurrence": name, "samples": points.len(), "stopped_at": stop.0, "stopped_by_loop": stop.1, "stopped_by_contact": stop.2});
    Ok((points, out))
}

/// `samples`, 2…`max`, or `default` without it.
fn sample_count(p: &Value, default: usize, max: usize, cmd: &str) -> Result<usize> {
    match p.get("samples") {
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 2.0 && *n <= max as f64)
            .map(|n| n.round() as usize)
            .ok_or_else(|| bad(cmd, format!("`samples` must be 2…{max}"))),
        None => Ok(default.max(2)),
    }
}

/// The motion a trace or an animation samples: a study played from start to end, or one joint
/// value driven from `from` to `to`. Returns the default sample count (the study's steps + 1, or
/// 100), the values driven and what they are at sample `k` of `n`.
fn motion_plan(s: &Session, p: &Value, cmd: &str) -> Result<(usize, BTreeSet<(u64, usize)>, Plan)> {
    Ok(match (p.get("study"), p.get("joint")) {
        (Some(_), Some(_)) => return Err(bad(cmd, "give `study` or `joint`, not both")),
        (Some(_), None) => {
            let m = s.doc.assembly.studies.get(study_index(s, p, cmd)?).cloned().ok_or_else(|| bad(cmd, "study"))?;
            let n = (m.steps as usize).saturating_add(1);
            (n, keyed(&m), Box::new(move |t, k, n| values_at(t, &m, f64::from(m.steps) * k as f64 / (n - 1) as f64)))
        }
        (None, Some(j)) => {
            let id = joint_id(s, Some(j), cmd)?;
            let kind = s.doc.assembly.joints.iter().find(|j| j.id == id).map(|j| j.kind).ok_or_else(|| bad(cmd, "joint"))?;
            let dofs = kind.dofs().len();
            if dofs == 0 {
                return Err(bad(cmd, "a rigid joint does not move"));
            }
            let index = match p.get("index") {
                None => 0,
                Some(v) => v
                    .as_u64()
                    .and_then(|i| usize::try_from(i).ok())
                    .filter(|i| *i < dofs)
                    .ok_or_else(|| bad(cmd, format!("`index` must be below {dofs}, the joint's value count")))?,
            };
            let end = |key: &str| match p.get(key) {
                Some(v) => value(s, v, kind.is_angle(index), cmd),
                None => Err(bad(cmd, format!("give `from` and `to`: the joint's values the motion runs between (no `{key}`)"))),
            };
            let (from, to) = (end("from")?, end("to")?);
            let plan: Plan = Box::new(move |t, k, n| {
                let mut q = t.doc.assembly.joints.iter().find(|j| j.id == id).map(|j| j.values.clone()).unwrap_or_default();
                q.resize(q.len().max(dofs), 0.0);
                if let Some(x) = q.get_mut(index) {
                    *x = from + (to - from) * k as f64 / (n - 1) as f64;
                }
                vec![(id, q)]
            });
            (100, BTreeSet::from([(id, index)]), plan)
        }
        (None, None) => return Err(bad(cmd, "give a `study`, or a `joint` with `from` and `to`")),
    })
}

fn trace(s: &mut Session, p: &Value) -> Result<Value> {
    let (points, mut out) = sample_trace(s, p, "motion.trace")?;
    out["points"] = points.iter().map(|v| json!([v.x, v.y, v.z])).collect();
    Ok(out)
}

/// The trace as a 3D sketch spline, in one undo step: a new sketch of the root component (the
/// world, which no joint moves) with 3D Sketch on and the spline through the trace's points.
fn keep_trace(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "motion.trace.keep";
    let (mut points, mut out) = sample_trace(s, p, cmd)?;
    // Where the motion stood still (at a limit or a stop), one fit point.
    points.dedup_by(|a, b| a.dist(*b) < 1e-6);
    // A full cycle comes back to where it started (to the loop solver's tolerance): the spline
    // closes there, smoothly, through the start itself (and a sample that would double it goes).
    if points.len() > 3
        && let (Some(&first), Some(&last)) = (points.first(), points.last())
        && first.dist(last) < 1e-4
    {
        points.pop();
        if points.last().is_some_and(|p| p.dist(first) < 1e-6) {
            points.pop();
        }
        points.push(first);
    }
    if points.len() < 2 {
        return Err(bad(cmd, "the point does not move: there is no trace to keep"));
    }
    let name = match str_(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => (1..=s.doc.features.len() + 1).map(|k| format!("Motion Trace{k}")).find(|n| s.doc.find_feature(n).is_none()).unwrap_or_default(),
    };
    let pts: Vec<Value> = points.iter().map(|v| json!([v.x, v.y, v.z])).collect();
    // Made in the root, whatever is being edited, which is then as it was (also on an error).
    let before = (s.active_sketch, s.active_component, s.active_occurrence_path.take());
    s.active_component = 0;
    let made = (|| {
        let run = |s: &mut Session, id: &str, q: Value| super::find_command(id).ok_or_else(|| bad(cmd, id.to_string()))?.run_nested(s, &q);
        let made = run(s, "sketch.create", json!({"plane": "XY", "project_edges": false, "name": name}))?;
        let sketch = made.get("sketch").cloned().unwrap_or(Value::Null);
        run(s, "sketch.options", json!({"sketch": sketch, "sketch_3d": true}))?;
        let spline = run(s, "sketch.spline3d", json!({"sketch": sketch, "points": pts}))?;
        Ok::<_, EngineError>((made, spline))
    })();
    (s.active_sketch, s.active_component, s.active_occurrence_path) = before;
    s.revision += 1;
    let (made, spline) = made?;
    let sketch = made.get("sketch").cloned().unwrap_or(Value::Null);
    out["sketch"] = sketch;
    out["name"] = made.get("name").cloned().unwrap_or(Value::Null);
    out["curves"] = spline.get("curves").cloned().unwrap_or(Value::Null);
    out["points"] = json!(points.len());
    Ok(out)
}

fn list_studies(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({"studies": s.doc.assembly.studies}))
}

fn delete_study(s: &mut Session, p: &Value) -> Result<Value> {
    let i = study_index(s, p, "motion.delete")?;
    let m = s.doc_mut().assembly.studies.remove(i);
    Ok(json!({"deleted": m.name}))
}

// ---- Exploded views ----

fn view_index(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    let key = match p.get("view") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, "`view` must be an id or a name")),
    };
    s.doc.assembly.exploded.iter().position(|e| e.id.to_string() == key || e.name == key).ok_or_else(|| bad(cmd, format!("no exploded view `{key}`")))
}

/// World centre of each top-level occurrence's bodies.
fn centres(s: &Session) -> Vec<(u64, Vec3)> {
    let bodies = placed(s);
    let mut out = Vec::new();
    for o in s.doc.occurrences.iter().filter(|o| o.parent == 0) {
        let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
        for (_, b) in bodies.iter().filter(|(path, _)| path.first() == Some(&o.id)) {
            if let Ok(m) = b.tessellate((b.size() * 1e-2).max(1e-2)) {
                let bb = m.bounds();
                lo = Vec3::new(lo.x.min(bb.min.x), lo.y.min(bb.min.y), lo.z.min(bb.min.z));
                hi = Vec3::new(hi.x.max(bb.max.x), hi.y.max(bb.max.y), hi.z.max(bb.max.z));
            }
        }
        if lo.x <= hi.x {
            out.push((o.id, (lo + hi) * 0.5));
        }
    }
    out
}

fn explode_create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "explode.create";
    let moves: Vec<(u64, Vec3)> = match p.get("moves") {
        Some(Value::Array(list)) => {
            if list.len() > MAX_ITEMS {
                return Err(bad(cmd, "too many moves"));
            }
            list.iter()
                .map(|m| {
                    let o = occurrence_id(s, m.get("occurrence"), cmd)?;
                    let t = m
                        .get("translate")
                        .and_then(vec3)
                        .filter(|v| v.len() < 1e7)
                        .ok_or_else(|| bad(cmd, "each move needs `translate`: [x, y, z]"))?;
                    Ok((o, t))
                })
                .collect::<Result<_>>()?
        }
        Some(_) => return Err(bad(cmd, "`moves` must be a list")),
        None => {
            let scale = match p.get("scale") {
                Some(v) => v.as_f64().filter(|x| x.is_finite() && x.abs() <= 100.0).ok_or_else(|| bad(cmd, "`scale` must be a number up to 100"))?,
                None => 1.0,
            };
            let c = centres(s);
            if c.len() < 2 {
                return Err(bad(cmd, "an exploded view needs two or more occurrences"));
            }
            let mid = c.iter().fold(Vec3::ZERO, |a, (_, p)| a + *p) * (1.0 / c.len() as f64);
            c.iter().map(|(o, p)| (*o, (*p - mid) * scale)).collect()
        }
    };
    if s.doc.assembly.exploded.len() >= MAX_ITEMS {
        return Err(bad(cmd, "too many exploded views"));
    }
    let id = next_id(s, cmd)?;
    let name = str_(p, "name").map(str::to_string).unwrap_or_else(|| format!("Exploded View{}", s.doc.assembly.exploded.len() + 1));
    s.doc_mut().assembly.exploded.push(ExplodedView { id, name: name.clone(), moves: moves.clone() });
    Ok(json!({"view": id, "name": name, "moves": moves.iter().map(|(o, t)| json!({"occurrence": o, "translate": t})).collect::<Vec<_>>()}))
}

fn explode_show(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "explode.show";
    if p.get("view").is_none_or(|v| v.is_null() || v.as_str().is_some_and(|x| x.eq_ignore_ascii_case("none"))) {
        s.pending_moves.clear();
        s.revision += 1;
        return Ok(json!({"view": null}));
    }
    let i = view_index(s, p, cmd)?;
    let v = s.doc.assembly.exploded.get(i).cloned().ok_or_else(|| bad(cmd, "view"))?;
    s.pending_moves.clear();
    for (o, t) in &v.moves {
        let Some(occ) = s.doc.occurrences.iter().find(|x| x.id == *o) else { continue };
        let mut m = occ.transform;
        if let Some(r) = m.get_mut(3) {
            r[0] += t.x;
            r[1] += t.y;
            r[2] += t.z;
        }
        s.pending_moves.insert(*o, m);
    }
    s.revision += 1;
    Ok(json!({"view": v.id, "name": v.name}))
}

fn explode_list(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({"views": s.doc.assembly.exploded}))
}

fn explode_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let i = view_index(s, p, "explode.delete")?;
    let v = s.doc_mut().assembly.exploded.remove(i);
    Ok(json!({"deleted": v.name}))
}

/// World position of a point of an occurrence (tests).
#[cfg(test)]
pub fn world_point(s: &Session, occurrence: u64, p: Vec3) -> Vec3 {
    solidcraft_doc::apply_point(&joints::occurrence_world(&s.doc, occurrence), p)
}

#[cfg(test)]
#[path = "motion_tests.rs"]
mod tests;
