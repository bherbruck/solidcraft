//! Joints between occurrences (`solidcraft_doc::joints`): Joint Origin, Joint, As-built Joint,
//! Rigid Group, Drive Joints, Motion Link, limits, and Interference. Every change re-solves the
//! joint graph and moves the occurrences it places; the session re-solves after any edit too,
//! so joints follow the geometry their origins snap to.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use solidcraft_doc::expr::Kind;
use solidcraft_doc::joints::cam::{self, Cam, CamProfile, Follower};
use solidcraft_doc::joints::{self, Coupling, Joint, JointKind, JointOrigin, MotionLink, NamedOrigin, Size, Snap};
use solidcraft_geom::{Aabb3, Vec3};

use super::CommandSpec;
use crate::params::{bad, bool_, num, str_, string_list, vec3};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("joint.origin", "Joint Origin", joint_origin).at("SOLID", "CREATE").icon("joint_origin").params(
        "name?, occurrence?: id|name (default: from the pick), face: [x,y,z] (centre of a planar face) | circle: [x,y,z] (centre of a round end) | point: [x,y,z], z?: [x,y,z]",
    ),
    CommandSpec::new("joint.create", "Joint", joint).at("SOLID", "ASSEMBLE").icon("joint").key("J").params(
        "type: rigid|revolute|slider|cylindrical|pin_slot|planar|ball; a, b: joint origins ({occurrence, face|circle|point: [x,y,z] in world} | origin name); \
         values?: [angles in deg or expressions, distances]; limits?: [[min, max] | null…]; flip?: bool (default false: the origins are mated, \
         b's z against a's as two faces meet; true: b's z along a's, so two origins given at one point with the same z stay as they are); \
         offset?; angle?; name?; quiet?: bool (leave out dof and fixed)",
    ),
    CommandSpec::new("joint.as_built", "As-Built Joint", as_built).at("SOLID", "ASSEMBLE").icon("as_built").params(
        "type, a: occurrence, b: occurrence (stay where they are); at?: joint origin (default: b's origin) — the joint's frame; \
         name?; limits?: [[min, max] | null…]; quiet?: bool",
    ),
    CommandSpec::new("joint.rigid_group", "Rigid Group", rigid_group).at("SOLID", "ASSEMBLE").icon("rigid_group").params("occurrences: [ids or names] (move together, as they are)"),
    CommandSpec::new("joint.drive", "Drive Joints", drive).at("SOLID", "ASSEMBLE").icon("drive").params(
        "joint: id|name; value | values: [angles in deg or expressions, distances] (values not given keep theirs; clamped to the limits; the other joints of a \
         closed loop follow, and the motion stops where the loop can no longer close → stopped_by_loop); quiet?: bool (leave out dof \
         and fixed) → values: [{name, value, unit: deg|mm, limits}] as joint.list gives them; moved: whether any occurrence moved; \
         reason: why the joint can move nothing (suppressed, its origins lost, or its chain joined to nothing grounded → unconnected: \
         the occurrence named)",
    ),
    CommandSpec::new("joint.reach", "Reach Point", reach).icon("reach").params(
        "occurrence: id|name; point: [x,y,z] (where its point goes, world); at?: [x,y,z] (its point, in the world where it is now; default: the centre of          its bodies' box); z?: [x,y,z] (the way its axis then points); axis?: [x,y,z] (that axis, in the world now; default: its own z); joints?: [ids          or names] (only these move toward the point; the other joints of a closed loop they are in follow); quiet?: bool; continuous?: bool (a frame          of a drag: the answer says continuous, and the app's command line doesn't echo it). Moves the values of the joints          on its chain from where they are to the nearest pose, within their limits, through motion links, keeping closed loops closed as joint.drive does          (contact sets are not checked) → reached; error_mm (and error_deg with z): how close it got; point: where its point is; values: [{joint, name,          values}] of the joints that moved, as joint.list gives them; moved; stopped_by_limit: [{joint, value, limit: min|max}]: the limits it rests on          short of the point; stopped_by_loop: {joint}: the closed loop that stopped it (its chain alone gets closer); reason: why nothing can move",
    ),
    CommandSpec::new("joint.motion_link", "Motion Link", motion_link)
        .at("SOLID", "ASSEMBLE")
        .icon("motion_link")
        .params("a, b: joints; ratio (b = ratio · a + offset): b's units per a's (mm per deg for an angle driving a distance); offset?: in b's units (deg for an angle, mm for a distance, or an expression); ia?, ib?: value indices (default 0)"),
    CommandSpec::new("joint.gear", "Gear", gear).at("SOLID", "ASSEMBLE").icon("gear").params(
        "a, b: joints that turn (revolute), b driven by a; ra, rb: the gears' pitch radii (or tooth counts): mm, an expression, or \
         {circle: [x,y,z], occurrence?}: the round face there, its radius (read again whenever the design changes, so the ratio follows \
         edits); internal?: bool (one is a ring gear: both turn the same way; by default b turns the other way); reverse?: bool; \
         link?: index (replace that motion link) → link, ratio (b's degrees per a's); b keeps where it is",
    ),
    CommandSpec::new("joint.rack_pinion", "Rack and Pinion", rack_pinion).at("SOLID", "ASSEMBLE").icon("rack_pinion").params(
        "a, b: a joint that turns (revolute: the pinion) and one that slides (slider: the rack), in either order, b driven by a; \
         radius: the pinion's pitch radius (mm, an expression, or {circle: [x,y,z], occurrence?}): the rack travels radius × the turn in \
         radians; reverse?: bool; link?: index (replace that motion link) → link, ratio (mm per degree, or degrees per mm); b keeps \
         where it is",
    ),
    CommandSpec::new("joint.screw", "Screw", screw).at("SOLID", "ASSEMBLE").icon("screw").params(
        "joint: a cylindrical joint (its turn drives its travel) | a, b: a joint that turns and one that slides, b driven by a; pitch: \
         travel per turn (mm or an expression); reverse?: bool (a left-hand thread); link?: index (replace that motion link) → link, \
         ratio (mm per degree, or degrees per mm); b keeps where it is",
    ),
    CommandSpec::new("joint.cam", "Cam", cam_cmd).at("SOLID", "ASSEMBLE").icon("cam").params(
        "a: the cam's joint (revolute); b: the follower's joint, a slider (a translating follower) or a revolute (an oscillating one),          b driven by a; profile: {sketch: id|name, index?: the profile (default: the largest)} | {edges: [[x,y,z]…] (points on edges of          the cam's bodies forming one closed loop, world), occurrence?}: the cam's outline, in a plane square to its axis, read again          whenever the design changes; follower?: knife|roller|flat (default knife; flat: a face square to a slider's line); radius?: the          roller's radius (mm, an expression or {circle: [x,y,z], occurrence?}: the roller's round face, which is its tip too); tip?: a          joint origin where the follower touches: its knife's point or its roller's centre, such as {circle: [x,y,z]} on the roller          (needed for an oscillating follower; a translating one's line runs through it, by default along its slider's axis); flip?: bool (ride the cam from the other side          than the one the follower is on now); link?: index (replace that motion link) → link, lift: {min, max, rise, unit} (b's travel          over a turn of the cam), reverse (the side it rides from), follower_offset (mm: how far a translating follower's line passes          from the cam's axis); b keeps where it is",
    ),
    CommandSpec::new("joint.limits", "Joint Limits", limits).icon("joint").params("joint; index?: value index (default 0); min?, max? (omit both to clear)"),
    CommandSpec::new("joint.edit", "Edit Joint", edit).icon("edit").params("joint; type?, flip?, offset?, angle?, suppressed?, name?"),
    CommandSpec::new("joint.delete", "Delete Joint", delete).icon("delete").params("joint: id|name (or origin: name, link: index)"),
    CommandSpec::new("joint.list", "List Joints", list).icon("list").noundo().params(
        "→ joints (type, occurrences, values, limits), origins, links, degrees of freedom, conflicts, floating: [{occurrences, joints}] \
         (joint chains joined to nothing grounded: the solver places none of them, so driving them moves nothing)",
    ),
    CommandSpec::new("joint.solve", "Solve Joints", solve_cmd).icon("compute").params("re-place the occurrences from the joints → conflicts, errors, dof"),
    CommandSpec::new("inspect.interference", "Interference", interference).at("SOLID", "INSPECT").icon("interference").noundo().params(
        "bodies?: [names] (default: all, in world placement) → interferences: pairs that overlap with the overlap volume, area, bounding box and centre (a pin a hair off the axis of a hole of its size also has near_coaxial: {offset_mm, length_mm}, its volume the crescent the walls share); unchecked: pairs that could not be checked, with the reason. Bodies are pruned by bounding box first, so any number of bodies is fine; at most 20000 pairs whose boxes overlap are intersected per call (truncated: true and skipped_pairs: n say when more were left out)",
    ),
];

/// Re-solve the joints and move the occurrences they place (after any edit). Gears, racks and
/// screws first take their ratios from the radii and pitches they are defined by.
pub fn resolve(s: &mut Session) -> joints::Solution {
    recouple(s);
    resolve_holding(s, &BTreeMap::new())
}

/// Work out the ratios (and cam tables) of the coupled motion links again (a radius, a parameter
/// or a cam's profile may have changed). A coupling that can't be worked out (its face gone)
/// keeps its last ratio and table; `joint.list` says why.
fn recouple(s: &mut Session) {
    if s.doc.assembly.links.iter().all(|l| l.coupling.is_none()) {
        return;
    }
    let st = s.model.state();
    let changed: Vec<(usize, (f64, Vec<f64>))> = s
        .doc
        .assembly
        .links
        .iter()
        .enumerate()
        .filter_map(|(i, l)| joints::coupling_values(&s.doc, &st, l)?.ok().filter(|(r, t)| *r != l.ratio || *t != l.lift).map(|v| (i, v)))
        .collect();
    if changed.is_empty() {
        return;
    }
    let doc = s.doc_mut();
    for (i, (r, t)) in changed {
        if let Some(l) = doc.assembly.links.get_mut(i) {
            l.ratio = r;
            l.lift = t;
        }
    }
}

/// Re-solve the joints, keeping the `held` values (the ones being driven, mapped to the values
/// they move from) where loops are closed.
pub fn resolve_holding(s: &mut Session, held: &joints::Held) -> joints::Solution {
    let st = s.model.state();
    let sol = joints::solve_holding(&s.doc, &st, held);
    if !sol.world.is_empty() {
        let mut next = (*s.doc).clone();
        if next.apply_joint_solution(&sol) {
            *s.doc_mut() = next;
            s.revision += 1;
        }
    }
    sol
}

/// A solve's answer; `quiet: true` in the command's params leaves out the per-occurrence lists.
fn sol_json(sol: &joints::Solution, p: &Value) -> Value {
    let mut out = json!({
        "conflicts": sol.conflicts.iter().map(|(id, n, w)| json!({"joint": id, "name": n, "why": w})).collect::<Vec<_>>(),
        "errors": sol.errors.iter().map(|(id, n, w)| json!({"joint": id, "name": n, "why": w})).collect::<Vec<_>>(),
    });
    if !bool_(p, "quiet").unwrap_or(false) {
        out["dof"] = sol.dof.iter().map(|(o, d)| json!({"occurrence": o, "dof": d})).collect();
        out["fixed"] = json!(sol.fixed);
    }
    out
}

/// A joint's values as `joint.list` gives them: degrees for angles, mm for distances, each with
/// its name and limits.
pub(crate) fn named_values(j: &Joint, q: &[f64]) -> Vec<Value> {
    j.kind
        .dofs()
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let (v, angle) = (q.get(i).copied().unwrap_or(0.0), j.kind.is_angle(i));
            json!({"name": n, "value": if angle { v.to_degrees() } else { v }, "unit": if angle { "deg" } else { "mm" },
                "limits": j.limits.get(i).copied().flatten().map(|(a, b)| if angle { [a.to_degrees(), b.to_degrees()] } else { [a, b] })})
        })
        .collect()
}

/// A joint's values as the solver places it (after motion links, so a screw's travel follows its
/// turn), named as `joint.list` gives them.
pub(crate) fn effective_values(s: &Session, id: u64) -> Option<Vec<Value>> {
    let j = s.doc.assembly.joints.iter().find(|j| j.id == id)?;
    let vals = joints::joint_values(&s.doc.assembly);
    Some(named_values(j, vals.get(&id).map(Vec::as_slice).unwrap_or(&j.values)))
}

fn occurrence_name(s: &Session, id: u64) -> String {
    s.doc.occurrences.iter().find(|o| o.id == id).map(|o| o.name.clone()).unwrap_or_else(|| id.to_string())
}

/// Why driving joint `id` moves nothing whatever its values (`None`: it places what it joins),
/// with the occurrence that is joined to nothing grounded when that is the reason.
pub(crate) fn cannot_move(s: &Session, sol: &joints::Solution, id: u64) -> Option<(String, Option<String>)> {
    let j = s.doc.assembly.joints.iter().find(|j| j.id == id)?;
    // Its value carries through motion links to joints that may move (a suppressed dial too).
    if s.doc.assembly.links.iter().any(|l| l.a == id || l.b == id) {
        return None;
    }
    if j.suppressed {
        return Some((format!("{} is suppressed", j.name), None));
    }
    if let Some((_, _, why)) = sol.errors.iter().find(|e| e.0 == id) {
        return Some((why.clone(), None));
    }
    // The solver places what it reaches from the root and grounded occurrences.
    let placed = |o: u64| o == 0 || sol.world.contains_key(&o);
    if placed(j.a.occurrence) || placed(j.b.occurrence) {
        return None;
    }
    let name = occurrence_name(s, j.a.occurrence);
    Some((format!("{name} is not connected to anything grounded (ground it, or join it to a grounded part)"), Some(name)))
}

/// Joint chains joined to nothing grounded, which the solver places none of: each chain's
/// occurrences and joints, by name.
fn floating_chains(s: &Session, sol: &joints::Solution) -> Vec<Value> {
    let placed = |o: u64| o == 0 || sol.world.contains_key(&o);
    let errors: BTreeSet<u64> = sol.errors.iter().map(|e| e.0).collect();
    let mut next: BTreeMap<u64, Vec<(u64, &Joint)>> = BTreeMap::new();
    for j in s.doc.assembly.joints.iter().filter(|j| !j.suppressed && !errors.contains(&j.id)) {
        if !placed(j.a.occurrence) && !placed(j.b.occurrence) {
            next.entry(j.a.occurrence).or_default().push((j.b.occurrence, j));
            next.entry(j.b.occurrence).or_default().push((j.a.occurrence, j));
        }
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for &start in next.keys() {
        if !seen.insert(start) {
            continue;
        }
        let (mut occs, mut js, mut todo) = (vec![start], BTreeSet::new(), vec![start]);
        while let Some(o) = todo.pop() {
            for (other, j) in next.get(&o).into_iter().flatten() {
                js.insert((j.id, j.name.as_str()));
                if seen.insert(*other) {
                    occs.push(*other);
                    todo.push(*other);
                }
            }
        }
        out.push(json!({"occurrences": occs.iter().map(|o| occurrence_name(s, *o)).collect::<Vec<_>>(),
            "joints": js.iter().map(|(_, n)| *n).collect::<Vec<_>>()}));
    }
    out
}

pub(crate) fn occurrence_id(s: &Session, v: Option<&Value>, cmd: &str) -> Result<u64> {
    let key = match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, "an occurrence must be an id or a name")),
    };
    if key == "0" || key.eq_ignore_ascii_case("root") {
        return Ok(0);
    }
    if let Some(o) = s.doc.occurrences.iter().find(|o| o.id.to_string() == key || o.name == key) {
        return Ok(o.id);
    }
    // A component name: its first occurrence.
    s.doc.find_component(&key).and_then(|c| s.doc.occurrence_of(c)).map(|o| o.id).ok_or_else(|| bad(cmd, format!("no occurrence `{key}`")))
}

/// The occurrence whose bodies are nearest a world point (0 = root bodies).
pub(crate) fn occurrence_at(s: &Session, p: Vec3) -> Option<u64> {
    let st = s.model.state();
    let mut best: Option<(f64, u64)> = None;
    for b in &st.bodies {
        let comp = s.doc.model_body_component(b);
        let occs: Vec<u64> = if comp == 0 { vec![0] } else { s.doc.occurrences.iter().filter(|o| o.component == comp).map(|o| o.id).collect() };
        for occ in occs {
            let t = if occ == 0 { solidcraft_doc::IDENTITY } else { joints::occurrence_world(&s.doc, occ) };
            let inv = solidcraft_doc::mat_inverse(&t)?;
            let q = solidcraft_doc::apply_point(&inv, p);
            let bb = b.mesh().bounds();
            let c = Vec3::new(q.x.clamp(bb.min.x, bb.max.x), q.y.clamp(bb.min.y, bb.max.y), q.z.clamp(bb.min.z, bb.max.z));
            let d = c.dist(q);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, occ));
            }
        }
    }
    best.map(|(_, o)| o)
}

/// A joint origin from parameters: a named origin, or a snap given in world coordinates.
fn origin_param(s: &Session, v: Option<&Value>, cmd: &str) -> Result<JointOrigin> {
    let v = v.ok_or_else(|| bad(cmd, "joint origins `a` and `b` are required"))?;
    if let Value::String(name) = v {
        return s
            .doc
            .assembly
            .origins
            .iter()
            .find(|o| &o.name == name)
            .map(|o| o.origin.clone())
            .ok_or_else(|| bad(cmd, format!("no joint origin `{name}`")));
    }
    let pick = ["face", "circle", "point"].iter().find_map(|k| v.get(*k).and_then(vec3).map(|p| (*k, p)));
    let (what, world) = match (pick, v.get("frame")) {
        (Some(x), _) => x,
        (None, Some(f)) => ("frame", f.get("origin").and_then(vec3).ok_or_else(|| bad(cmd, "`frame` needs origin and z"))?),
        _ => return Err(bad(cmd, "a joint origin needs face, circle, point or frame [x, y, z]")),
    };
    let occurrence = match v.get("occurrence") {
        Some(o) => occurrence_id(s, Some(o), cmd)?,
        None => occurrence_at(s, world).ok_or_else(|| bad(cmd, "no body there"))?,
    };
    // World → the component's own coordinates.
    let t = if occurrence == 0 { solidcraft_doc::IDENTITY } else { joints::occurrence_world(&s.doc, occurrence) };
    let inv = solidcraft_doc::mat_inverse(&t).ok_or_else(|| bad(cmd, "occurrence transform"))?;
    let local = |p: Vec3| solidcraft_doc::apply_point(&inv, p);
    let local_dir = |d: Vec3| solidcraft_doc::apply_vector(&inv, d);
    let snap = match what {
        "face" | "circle" => {
            let pick = local(world);
            // A circle's name is its cylindrical wall's (a rim pick also touches the end face).
            let named_at = if what == "circle" {
                s.model.state().bodies.iter().filter_map(|b| cylinder_wall_point(&b.body, pick)).next().unwrap_or(pick)
            } else {
                pick
            };
            let face = solidcraft_doc::naming::face_names_at(&s.model.state(), &[named_at]).into_iter().next().filter(|n| !n.is_empty());
            if what == "face" { Snap::FaceCenter { pick, face } } else { Snap::CircleCenter { pick, face } }
        }
        "point" => Snap::Point { point: local(world), z: v.get("z").and_then(vec3).map(local_dir) },
        _ => {
            let f = v.get("frame").cloned().unwrap_or_default();
            let z = f.get("z").and_then(vec3).ok_or_else(|| bad(cmd, "`frame` needs a z axis"))?;
            Snap::Frame { origin: local(world), z: local_dir(z), x: f.get("x").and_then(vec3).map(local_dir) }
        }
    };
    let o = JointOrigin { occurrence, snap };
    match joints::resolve_origin(&s.doc, &s.model.state(), &o) {
        Ok(_) => Ok(o),
        // A circle given by its centre, where no face lies (#393).
        Err(e) => match (what == "circle").then(|| circle_centre(s, occurrence, local(world))).flatten() {
            Some(snap) => Ok(JointOrigin { occurrence, snap }),
            None => Err(bad(cmd, e.to_string())),
        },
    }
}

/// The round end of a cylindrical face of the occurrence's bodies centred at `pick`: a point on
/// its axis there, pointing out of that end (as picking its circular edge gives).
fn circle_centre(s: &Session, occurrence: u64, pick: Vec3) -> Option<Snap> {
    let component = if occurrence == 0 { 0 } else { s.doc.occurrences.iter().find(|o| o.id == occurrence)?.component };
    let st = s.model.state();
    let c = st
        .bodies
        .iter()
        .filter(|b| s.doc.model_body_component(b) == component)
        .find_map(|b| solidcraft_doc::kernel::cylinder_end_at(&b.body, pick))?;
    let along = (pick - c.axis_point).dot(c.axis);
    let (at, dir) = if (along - c.start).abs() <= (c.end - along).abs() { (c.start, -1.0) } else { (c.end, 1.0) };
    Some(Snap::Point { point: c.axis_point + c.axis * at, z: Some(c.axis * dir) })
}

pub(crate) fn joint_id(s: &Session, v: Option<&Value>, cmd: &str) -> Result<u64> {
    let key = match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, "`joint` must be an id or a name")),
    };
    s.doc.assembly.joints.iter().find(|j| j.id.to_string() == key || j.name == key).map(|j| j.id).ok_or_else(|| bad(cmd, format!("no joint `{key}`")))
}

/// A joint value: numbers are degrees for angles and mm for distances; text is an expression.
pub(crate) fn value(s: &Session, v: &Value, angle: bool, cmd: &str) -> Result<f64> {
    let kind = if angle { Kind::Angle } else { Kind::Length };
    let e = match v {
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite() && x.abs() < 1e9).map(|x| x.to_string()),
        Value::String(t) => Some(t.clone()),
        _ => None,
    }
    .ok_or_else(|| bad(cmd, "values must be numbers or expressions"))?;
    s.doc.eval(&e, kind).map_err(|e| bad(cmd, e.to_string()))
}

fn values_param(s: &Session, p: &Value, kind: JointKind, cmd: &str) -> Result<Vec<f64>> {
    let n = kind.dofs().len();
    let list: Vec<Value> = match (p.get("values"), p.get("value")) {
        (Some(Value::Array(a)), _) => a.clone(),
        (_, Some(v)) => vec![v.clone()],
        _ => Vec::new(),
    };
    if list.len() > n {
        return Err(bad(cmd, format!("a {kind:?} joint has {n} values ({})", kind.dofs().join(", "))));
    }
    let mut out = vec![0.0; n];
    for (i, v) in list.iter().enumerate() {
        if let Some(slot) = out.get_mut(i) {
            *slot = value(s, v, kind.is_angle(i), cmd)?;
        }
    }
    Ok(out)
}

fn limits_param(s: &Session, p: &Value, kind: JointKind, cmd: &str) -> Result<Vec<Option<(f64, f64)>>> {
    let n = kind.dofs().len();
    let mut out = vec![None; n];
    if let Some(Value::Array(a)) = p.get("limits") {
        if a.len() > n {
            return Err(bad(cmd, "more limits than values"));
        }
        for (i, l) in a.iter().enumerate() {
            if let (Some(lo), Some(hi)) = (l.get(0), l.get(1))
                && let Some(slot) = out.get_mut(i)
            {
                *slot = Some((value(s, lo, kind.is_angle(i), cmd)?, value(s, hi, kind.is_angle(i), cmd)?));
            }
        }
    }
    Ok(out)
}

fn next_joint_id(s: &Session) -> u64 {
    s.doc.assembly.joints.iter().map(|j| j.id).max().unwrap_or(0) + 1
}

fn add_joint(s: &mut Session, mut j: Joint, p: &Value) -> Result<Value> {
    if s.doc.assembly.joints.len() >= 10_000 {
        return Err(EngineError::Other("too many joints".into()));
    }
    // Several parts as bodies of one component (a multi-body STEP import, #1239): say how to
    // get two components.
    if j.a.occurrence == j.b.occurrence {
        return Err(bad(
            "joint.create",
            "a joint needs two different occurrences: both origins are on one component; give each part a component of its own first \
             (component.from_bodies)",
        ));
    }
    // It starts at its values inside its limits. A value moved to a limit moves the part as it
    // is joined, which the answer says.
    let mut q = j.values.clone();
    q.resize(j.kind.dofs().len(), 0.0);
    joints::clamp(&j, &mut q);
    let round = |v: &Value| v.as_f64().map(|x| (x * 1e6).round() / 1e6).unwrap_or(f64::NAN);
    let warnings: Vec<String> = named_values(&j, &q)
        .iter()
        .zip(named_values(&j, &j.values))
        .filter(|(now, was)| now["value"] != was["value"])
        .map(|(now, was)| {
            let unit = now["unit"].as_str().unwrap_or("");
            format!(
                "{} {} starts at {} {unit}, inside its limits [{}, {}], not at {} {unit}: the part moved there as it was joined",
                j.name,
                now["name"].as_str().unwrap_or(""),
                round(&now["value"]),
                round(&now["limits"][0]),
                round(&now["limits"][1]),
                round(&was["value"])
            )
        })
        .collect();
    j.values = q;
    let id = j.id;
    let name = j.name.clone();
    // A new joint can take a deleted one's id; it starts shown.
    s.visibility.hidden_joints.retain(|h| *h != id);
    s.doc_mut().assembly.joints.push(j);
    let sol = resolve(s);
    let mut out = sol_json(&sol, p);
    out["joint"] = json!(id);
    out["name"] = json!(name);
    if !warnings.is_empty() {
        out["warnings"] = json!(warnings);
    }
    Ok(out)
}

fn joint_origin(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.origin";
    let origin = origin_param(s, Some(p), cmd)?;
    let name = match str_(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => (1..).map(|k| format!("Joint Origin{k}")).find(|n| !s.doc.assembly.origins.iter().any(|o| &o.name == n)).unwrap_or_default(),
    };
    if s.doc.assembly.origins.iter().any(|o| o.name == name) {
        return Err(bad(cmd, format!("there is already a joint origin `{name}`")));
    }
    s.doc_mut().assembly.origins.push(NamedOrigin { name: name.clone(), origin });
    Ok(json!({"origin": name}))
}

fn kind_param(p: &Value, cmd: &str) -> Result<JointKind> {
    str_(p, "type")
        .and_then(JointKind::parse)
        .ok_or_else(|| bad(cmd, "`type` must be rigid, revolute, slider, cylindrical, pin_slot, planar or ball"))
}

fn joint(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.create";
    let kind = kind_param(p, cmd)?;
    let a = origin_param(s, p.get("a"), cmd)?;
    let b = origin_param(s, p.get("b"), cmd)?;
    let id = next_joint_id(s);
    let name = str_(p, "name").map(str::to_string).unwrap_or_else(|| format!("Joint{id}"));
    let offset = match p.get("offset") {
        Some(v) => value(s, v, false, cmd)?,
        None => 0.0,
    };
    let angle = match p.get("angle") {
        Some(v) => value(s, v, true, cmd)?,
        None => 0.0,
    };
    let j = Joint {
        id,
        name,
        kind,
        a,
        b,
        values: values_param(s, p, kind, cmd)?,
        limits: limits_param(s, p, kind, cmd)?,
        flip: bool_(p, "flip").unwrap_or(false),
        offset,
        angle,
        suppressed: false,
    };
    add_joint(s, j, p)
}

fn as_built(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.as_built";
    let kind = kind_param(p, cmd)?;
    let (oa, ob) = (occurrence_id(s, p.get("a"), cmd)?, occurrence_id(s, p.get("b"), cmd)?);
    // The joint's frame in the world: the given origin, or B's component origin.
    let w = match p.get("at") {
        Some(v) => {
            let o = origin_param(s, Some(v), cmd)?;
            s.doc.origin_world(&s.model.state(), &o).map_err(|e| bad(cmd, e.to_string()))?
        }
        None => joints::occurrence_world(&s.doc, ob),
    };
    let fa = s.doc.frame_in_occurrence(oa, &w).ok_or_else(|| bad(cmd, "occurrence transform"))?;
    let fb = s.doc.frame_in_occurrence(ob, &w).ok_or_else(|| bad(cmd, "occurrence transform"))?;
    let id = next_joint_id(s);
    let name = str_(p, "name").map(str::to_string).unwrap_or_else(|| format!("Joint{id}"));
    let n = kind.dofs().len();
    let j = Joint {
        id,
        name,
        kind,
        a: JointOrigin { occurrence: oa, snap: fa },
        b: JointOrigin { occurrence: ob, snap: fb },
        values: vec![0.0; n],
        limits: limits_param(s, p, kind, cmd)?,
        flip: true, // as built: the origins coincide as they are, not mated
        offset: 0.0,
        angle: 0.0,
        suppressed: false,
    };
    add_joint(s, j, p)
}

fn rigid_group(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.rigid_group";
    let list = p.get("occurrences").and_then(Value::as_array).cloned().unwrap_or_default();
    if list.len() < 2 || list.len() > 1000 {
        return Err(bad(cmd, "`occurrences` must list 2…1000 occurrences"));
    }
    let ids: Vec<u64> = list.iter().map(|v| occurrence_id(s, Some(v), cmd)).collect::<Result<_>>()?;
    let first = ids.first().copied().unwrap_or(0);
    let mut made = Vec::new();
    for other in ids.iter().skip(1) {
        let r = as_built(s, &json!({"type": "rigid", "a": first, "b": other, "name": format!("Rigid Group {first}:{other}")}))?;
        made.push(r["joint"].clone());
    }
    Ok(json!({"joints": made}))
}

fn drive(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.drive";
    let id = joint_id(s, p.get("joint"), cmd)?;
    let kind = s.doc.assembly.joints.iter().find(|j| j.id == id).map(|j| j.kind).ok_or_else(|| bad(cmd, "joint"))?;
    if kind == JointKind::Rigid {
        return Err(bad(cmd, "a rigid joint does not move"));
    }
    let mut q = values_param(s, p, kind, cmd)?;
    if p.get("values").is_none() && p.get("value").is_none() {
        return Err(bad(cmd, "give `value` or `values`"));
    }
    let doc = s.doc_mut();
    let Some(j) = doc.assembly.joints.iter_mut().find(|j| j.id == id) else { return Err(bad(cmd, "joint")) };
    // Values not given (after `value`'s first, or past the end of `values`) keep theirs.
    let given = match p.get("values") {
        Some(Value::Array(a)) => a.len(),
        _ => 1,
    };
    for (i, v) in q.iter_mut().enumerate().skip(given) {
        *v = j.values.get(i).copied().unwrap_or(*v);
    }
    joints::clamp(j, &mut q);
    // Moved as far toward `q` as contact sets and closed loops allow. Values left out of
    // `values` are not driven: in a closed loop they follow.
    let before = super::motion::positions(s);
    let r = super::motion::drive_joint(s, id, q, given)?;
    let sol = joints::solve(&s.doc, &s.model.state());
    let mut out = sol_json(&sol, p);
    out["joint"] = json!(id);
    out["values"] = json!(effective_values(s, id));
    out["stopped_by_contact"] = r["stopped_by_contact"].clone();
    out["stopped_by_loop"] = r["stopped_by_loop"].clone();
    out["moved"] = json!(super::motion::moved(&before, &super::motion::positions(s)).next().is_some());
    if let Some((why, unconnected)) = cannot_move(s, &sol, id) {
        out["reason"] = json!(why);
        if let Some(o) = unconnected {
            out["unconnected"] = json!(o);
        }
    }
    Ok(out)
}

/// Joints `joint.reach` takes at most in `joints`.
const MAX_REACH_JOINTS: usize = 1000;

fn reach(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.reach";
    let occ = occurrence_id(s, p.get("occurrence"), cmd)?;
    let given = |k: &str| p.get(k).and_then(vec3).filter(|v| v.is_finite() && v.len() < 1e9);
    let to = given("point").ok_or_else(|| bad(cmd, "`point` must be [x, y, z]"))?;
    let world = if occ == 0 { solidcraft_doc::IDENTITY } else { joints::occurrence_world(&s.doc, occ) };
    let inv = solidcraft_doc::mat_inverse(&world).ok_or_else(|| bad(cmd, "occurrence transform"))?;
    let at = match p.get("at") {
        Some(_) => solidcraft_doc::apply_point(&inv, given("at").ok_or_else(|| bad(cmd, "`at` must be [x, y, z]"))?),
        None => body_centre(s, occ),
    };
    let unit = |k: &str| -> Result<Option<Vec3>> {
        match p.get(k) {
            None => Ok(None),
            Some(_) => given(k).and_then(Vec3::normalized).map(Some).ok_or_else(|| bad(cmd, format!("`{k}` must be a direction [x, y, z]"))),
        }
    };
    let (z, axis) = (unit("z")?, unit("axis")?);
    if axis.is_some() && z.is_none() {
        return Err(bad(cmd, "`axis` goes with `z`, the way it points"));
    }
    let axis = z.map(|z| (axis.map(|a| solidcraft_doc::apply_vector(&inv, a)).and_then(Vec3::normalized).unwrap_or(Vec3::Z), z));
    let only = match p.get("joints") {
        None => None,
        Some(Value::Array(a)) if a.len() <= MAX_REACH_JOINTS => Some(a.iter().map(|v| joint_id(s, Some(v), cmd)).collect::<Result<BTreeSet<u64>>>()?),
        Some(_) => return Err(bad(cmd, format!("`joints` must be a list of joints (ids or names), at most {MAX_REACH_JOINTS}"))),
    };
    let before = super::motion::positions(s);
    let st = s.model.state();
    let r = joints::reach(&s.doc, &st, &joints::Reach { occurrence: occ, point: at, to, axis, joints: only });
    let reached = match &r {
        Ok(x) => {
            // Only a reach that moves something touches the design (and so adds an undo step).
            if !x.values.is_empty() {
                let doc = s.doc_mut();
                for (id, q) in &x.values {
                    if let Some(j) = doc.assembly.joints.iter_mut().find(|j| j.id == *id) {
                        j.values.clone_from(q);
                    }
                }
            }
            x.clone()
        }
        Err(_) => joints::Reached::default(),
    };
    // Loops closed again from there, and where it ended up.
    let sol = resolve(s);
    let t = joints::occurrence_world(&s.doc, occ);
    let got = solidcraft_doc::apply_point(&t, at);
    let mut out = sol_json(&sol, p);
    out["occurrence"] = json!(occurrence_name(s, occ));
    out["point"] = json!([got.x, got.y, got.z]);
    out["error_mm"] = json!(got.dist(to));
    let mut there = got.dist(to) <= joints::REACH_TOL;
    if let Some((a, z)) = axis {
        // From the sine too: the cosine alone loses small angles to rounding.
        let v = solidcraft_doc::apply_vector(&t, a);
        let deg = v.cross(z).len().atan2(v.dot(z)).to_degrees();
        out["error_deg"] = json!(deg);
        there &= deg <= joints::REACH_TOL;
    }
    out["reached"] = json!(there);
    let name = |id: u64| s.doc.assembly.joints.iter().find(|j| j.id == id);
    // As joint.list gives them: with what motion links set.
    let effective = joints::joint_values(&s.doc.assembly);
    out["values"] = reached
        .values
        .keys()
        .filter_map(|id| name(*id))
        .map(|j| json!({"joint": j.id, "name": j.name, "values": named_values(j, effective.get(&j.id).unwrap_or(&j.values))}))
        .collect();
    out["moved"] = json!(super::motion::moved(&before, &super::motion::positions(s)).next().is_some());
    out["stopped_by_limit"] = if reached.limits.is_empty() {
        Value::Null
    } else {
        reached
            .limits
            .iter()
            .filter_map(|(id, i, upper)| {
                let j = name(*id)?;
                Some(json!({"joint": j.name, "value": j.kind.dofs().get(*i), "limit": if *upper { "max" } else { "min" }}))
            })
            .collect()
    };
    out["stopped_by_loop"] = json!(reached.stopped_by_loop.and_then(name).map(|j| json!({"joint": j.name})));
    if let Err(why) = r {
        out["reason"] = json!(why);
    }
    if bool_(p, "continuous") == Some(true) {
        out["continuous"] = json!(true);
    }
    Ok(out)
}

/// The centre of the box around an occurrence's bodies, in its own coordinates (its origin when
/// it has none).
fn body_centre(s: &Session, occ: u64) -> Vec3 {
    let comp = s.doc.occurrences.iter().find(|o| o.id == occ).map(|o| o.component).unwrap_or(0);
    let st = s.model.state();
    st.bodies.iter().filter(|b| s.doc.model_body_component(b) == comp).fold(Aabb3::EMPTY, |bb, b| bb.union(&b.mesh().bounds())).center()
}

fn motion_link(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.motion_link";
    let (a, b) = (joint_id(s, p.get("a"), cmd)?, joint_id(s, p.get("b"), cmd)?);
    if a == b {
        return Err(bad(cmd, "link two different joints"));
    }
    let ratio = num(p, "ratio").ok_or_else(|| bad(cmd, "`ratio` must be a number"))?;
    let (ia, ib) = (num(p, "ia").unwrap_or(0.0).max(0.0) as usize, num(p, "ib").unwrap_or(0.0).max(0.0) as usize);
    let dofs = |id: u64| s.doc.assembly.joints.iter().find(|j| j.id == id).map(|j| j.kind.dofs().len()).unwrap_or(0);
    if ia >= dofs(a) || ib >= dofs(b) {
        return Err(bad(cmd, "the linked joints need moving values at those indices"));
    }
    // In b's units, like its values and limits (stored in radians for an angle).
    let offset = match p.get("offset") {
        Some(v) => value(s, v, is_angle(s, b, ib), cmd)?,
        None => 0.0,
    };
    let ratio = ratio * ratio_scale(s, a, ia, b, ib);
    s.doc_mut().assembly.links.push(MotionLink { a, ia, b, ib, ratio, offset, coupling: None, lift: Vec::new() });
    let sol = resolve(s);
    Ok(sol_json(&sol, p))
}

/// A coupling length: a number (mm), an expression (kept as typed, so it follows the
/// parameters), or `{circle: [x, y, z], occurrence?}`: the round face there, by its radius.
fn size_param(s: &Session, p: &Value, key: &str, cmd: &str) -> Result<Size> {
    match p.get(key) {
        Some(Value::Number(n)) => n.as_f64().map(Size::Length).ok_or_else(|| bad(cmd, format!("`{key}` must be a finite length"))),
        Some(Value::String(t)) if t.len() <= 1000 => Ok(Size::Expr(t.trim().to_string())),
        Some(o @ Value::Object(m)) if m.contains_key("circle") => match origin_param(s, Some(o), cmd)? {
            o @ JointOrigin { snap: Snap::CircleCenter { .. }, .. } => Ok(Size::Circle(o)),
            _ => Err(bad(cmd, format!("`{key}`: give a point on the round face, not its centre"))),
        },
        _ => Err(bad(cmd, format!("`{key}` must be a length (mm or an expression) or a round face ({{circle: [x, y, z]}})"))),
    }
}

/// A joint's first value that turns (an angle), or that travels (a distance).
fn value_index(s: &Session, id: u64, turn: bool) -> Option<usize> {
    let j = s.doc.assembly.joints.iter().find(|j| j.id == id)?;
    (0..j.kind.dofs().len()).find(|i| j.kind.is_angle(*i) == turn)
}

/// The values a rack or a screw couples: a's turn and b's travel, or a's travel and b's turn.
fn turn_and_travel(s: &Session, a: u64, b: u64, cmd: &str) -> Result<(usize, usize)> {
    match (value_index(s, a, true), value_index(s, b, false), value_index(s, a, false), value_index(s, b, true)) {
        (Some(ia), Some(ib), ..) | (_, _, Some(ia), Some(ib)) => Ok((ia, ib)),
        _ => Err(bad(cmd, "couple a joint that turns (revolute) with one that slides (slider)")),
    }
}

/// Two different joints `a` and `b`.
fn joint_pair(s: &Session, p: &Value, cmd: &str) -> Result<(u64, u64)> {
    let (a, b) = (joint_id(s, p.get("a"), cmd)?, joint_id(s, p.get("b"), cmd)?);
    if a == b {
        return Err(bad(cmd, "couple two different joints"));
    }
    Ok((a, b))
}

fn reverse(p: &Value) -> bool {
    bool_(p, "reverse").unwrap_or(false)
}

fn gear(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.gear";
    let (a, b) = joint_pair(s, p, cmd)?;
    let turn = |id: u64| value_index(s, id, true).ok_or_else(|| bad(cmd, "gears need two joints that turn (revolute)"));
    let (ia, ib) = (turn(a)?, turn(b)?);
    let c = Coupling::Gear {
        ra: size_param(s, p, "ra", cmd)?,
        rb: size_param(s, p, "rb", cmd)?,
        internal: bool_(p, "internal").unwrap_or(false),
        reverse: reverse(p),
    };
    couple(s, p, cmd, MotionLink { a, ia, b, ib, ratio: 1.0, offset: 0.0, coupling: Some(c), lift: Vec::new() })
}

fn rack_pinion(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.rack_pinion";
    let (a, b) = joint_pair(s, p, cmd)?;
    let (ia, ib) = turn_and_travel(s, a, b, cmd)?;
    let c = Coupling::RackPinion { radius: size_param(s, p, "radius", cmd)?, reverse: reverse(p) };
    couple(s, p, cmd, MotionLink { a, ia, b, ib, ratio: 1.0, offset: 0.0, coupling: Some(c), lift: Vec::new() })
}

fn screw(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.screw";
    let (a, ia, b, ib) = match p.get("joint") {
        Some(v) => {
            let id = joint_id(s, Some(v), cmd)?;
            match (value_index(s, id, true), value_index(s, id, false)) {
                (Some(ia), Some(ib)) => (id, ia, id, ib),
                _ => return Err(bad(cmd, "a screw on one joint needs a joint that turns and slides (cylindrical); or give a and b")),
            }
        }
        None => {
            let (a, b) = joint_pair(s, p, cmd)?;
            let (ia, ib) = turn_and_travel(s, a, b, cmd)?;
            (a, ia, b, ib)
        }
    };
    let c = Coupling::Screw { pitch: size_param(s, p, "pitch", cmd)?, reverse: reverse(p) };
    couple(s, p, cmd, MotionLink { a, ia, b, ib, ratio: 1.0, offset: 0.0, coupling: Some(c), lift: Vec::new() })
}

fn cam_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.cam";
    let (a, b) = joint_pair(s, p, cmd)?;
    let kind = |id: u64| s.doc.assembly.joints.iter().find(|j| j.id == id).map(|j| j.kind);
    if kind(a) != Some(JointKind::Revolute) {
        return Err(bad(cmd, "the cam (a) turns on a revolute joint"));
    }
    let oscillating = match kind(b) {
        Some(JointKind::Slider) => false,
        Some(JointKind::Revolute) => true,
        _ => return Err(bad(cmd, "the follower (b) is a slider or a revolute joint")),
    };
    let follower = match str_(p, "follower").unwrap_or("knife") {
        "knife" => Follower::Knife,
        "roller" => Follower::Roller { radius: size_param(s, p, "radius", cmd)? },
        "flat" => Follower::Flat,
        _ => return Err(bad(cmd, "`follower` is knife, roller or flat")),
    };
    if oscillating && p.get("tip").is_none() && !matches!(follower, Follower::Roller { radius: Size::Circle(_) }) {
        return Err(bad(cmd, "an oscillating follower needs `tip`: its knife's point or its roller's centre ({circle: [x, y, z]} on the roller)"));
    }
    let tip = match p.get("tip") {
        Some(v) => Some(origin_param(s, Some(v), cmd)?),
        None => None,
    };
    let mut c = Cam { profile: cam_profile(s, p, cmd)?, follower, tip, reverse: false };
    let probe = MotionLink { a, ia: 0, b, ib: 0, ratio: 1.0, offset: 0.0, coupling: None, lift: Vec::new() };
    let st = s.model.state();
    c.reverse = cam::default_reverse(&s.doc, &st, &probe, &c).map_err(|e| bad(cmd, e.to_string()))? != bool_(p, "flip").unwrap_or(false);
    let offset = cam::follower_offset(&s.doc, &st, &probe, &c).map_err(|e| bad(cmd, e.to_string()))?;
    let reverse = c.reverse;
    let mut out = couple(s, p, cmd, MotionLink { coupling: Some(Coupling::Cam(c)), ..probe })?;
    let index = out["link"].as_u64().and_then(|i| usize::try_from(i).ok()).unwrap_or(usize::MAX);
    if let (Some(o), Some(l)) = (out.as_object_mut(), s.doc.assembly.links.get(index)) {
        o.remove("ratio");
        o.insert("lift".into(), lift_json(l, oscillating));
        o.insert("reverse".into(), json!(reverse));
        if !oscillating {
            o.insert("follower_offset".into(), json!(offset));
        }
    }
    Ok(out)
}

/// A cam's follower travel over a turn of the cam: b's lowest and highest values and their
/// difference, in degrees (`angle`) or mm.
fn lift_json(l: &MotionLink, angle: bool) -> Value {
    // A table that is not whole (a hand-edited file) moves nothing, and says so.
    if l.lift.len() != cam::CAM_SAMPLES {
        return Value::Null;
    }
    let unit = |v: f64| if angle { v.to_degrees() } else { v };
    let (lo, hi) = l.lift.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let (lo, hi) = (unit(lo + l.offset), unit(hi + l.offset));
    json!({"min": lo, "max": hi, "rise": hi - lo, "unit": if angle { "deg" } else { "mm" }})
}

/// A cam's profile: a sketch's closed profile (kept by its outer loop's curve ids, so the sketch's
/// other profiles may change), or picked edges of the cam's bodies (kept in the component's
/// coordinates with the edges' names).
fn cam_profile(s: &Session, p: &Value, cmd: &str) -> Result<CamProfile> {
    let v = p.get("profile").ok_or_else(|| bad(cmd, "`profile` is required: {sketch: id|name} or {edges: [[x, y, z], …]}"))?;
    if let Some(sk) = v.get("sketch") {
        let sketch = super::features::sketch_id(s, Some(sk), cmd, "profile.sketch")?;
        let st = s.model.state();
        let solved =
            st.sketches.iter().find(|x| x.feature == sketch).ok_or_else(|| bad(cmd, "that sketch is not evaluated (it is after the marker?)"))?;
        let picked = match v.get("index") {
            None => cam::sketch_profile(solved, &[]),
            Some(i) => i.as_u64().and_then(|i| usize::try_from(i).ok()).and_then(|i| solved.profiles.get(i)),
        };
        let profile = picked.ok_or_else(|| bad(cmd, "the sketch has no such closed profile (an open curve makes none)"))?;
        return Ok(CamProfile::Sketch { sketch, curves: profile.outer_curves.clone() });
    }
    let Some(Value::Array(pts)) = v.get("edges") else {
        return Err(bad(cmd, "`profile` is {sketch: id|name} or {edges: [[x, y, z], …]}"));
    };
    if pts.is_empty() || pts.len() > cam::MAX_PROFILE_EDGES {
        return Err(bad(cmd, format!("pick 1 to {} edges", cam::MAX_PROFILE_EDGES)));
    }
    let world: Vec<Vec3> = pts.iter().map(vec3).collect::<Option<_>>().ok_or_else(|| bad(cmd, "`edges` are finite [x, y, z] points"))?;
    let occurrence = match v.get("occurrence") {
        Some(o) => occurrence_id(s, Some(o), cmd)?,
        None => world.first().and_then(|w| occurrence_at(s, *w)).ok_or_else(|| bad(cmd, "no body there"))?,
    };
    let t = if occurrence == 0 { solidcraft_doc::IDENTITY } else { joints::occurrence_world(&s.doc, occurrence) };
    let inv = solidcraft_doc::mat_inverse(&t).ok_or_else(|| bad(cmd, "occurrence transform"))?;
    let picks: Vec<Vec3> = world.iter().map(|w| solidcraft_doc::apply_point(&inv, *w)).collect();
    let comp = s.doc.occurrences.iter().find(|o| o.id == occurrence).map(|o| o.component).unwrap_or(0);
    let st = s.model.state();
    let bodies: Vec<&solidcraft_doc::ModelBody> = st.bodies.iter().filter(|b| s.doc.model_body_component(b) == comp).collect();
    // Each pick's edge name on the body whose edges pass nearest it.
    let names = picks
        .iter()
        .map(|q| {
            let near = |b: &&solidcraft_doc::ModelBody| {
                b.mesh()
                    .edges
                    .iter()
                    .flat_map(|e| e.windows(2))
                    .filter_map(|w| Some(q.dist_to_segment(*w.first()?, *w.get(1)?)))
                    .fold(f64::MAX, f64::min)
            };
            bodies
                .iter()
                .min_by(|x, y| near(x).total_cmp(&near(y)))
                .and_then(|b| solidcraft_doc::naming::edge_names_at(b, &[*q]).into_iter().next())
                .unwrap_or_default()
        })
        .collect();
    Ok(CamProfile::Edges { occurrence, picks, names })
}

/// Add a coupling's motion link, or put it in place of link `link`: its ratio worked out from its
/// sizes, a new link's offset leaving b where it is (the teeth mesh as they are modelled).
fn couple(s: &mut Session, p: &Value, cmd: &str, mut l: MotionLink) -> Result<Value> {
    let links = &s.doc.assembly.links;
    let replace = match p.get("link") {
        None => None,
        Some(v) => {
            Some(v.as_u64().and_then(|i| usize::try_from(i).ok()).filter(|i| *i < links.len()).ok_or_else(|| bad(cmd, "no such motion link"))?)
        }
    };
    if replace.is_none() && links.len() >= joints::MAX_JOINTS {
        return Err(bad(cmd, "too many motion links"));
    }
    let others: Vec<&MotionLink> = links.iter().enumerate().filter(|(i, _)| Some(*i) != replace).map(|(_, k)| k).collect();
    let name = |id: u64| s.doc.assembly.joints.iter().find(|j| j.id == id).map(|j| j.name.clone()).unwrap_or_default();
    if others.iter().any(|k| (k.b, k.ib) == (l.b, l.ib)) {
        return Err(bad(cmd, format!("{} already follows a motion link: couple it the other way round (as a)", name(l.b))));
    }
    // The values that set a's, back through the links: b among them would make a cycle.
    let mut at = (l.a, l.ia);
    for _ in 0..=others.len() {
        if at == (l.b, l.ib) {
            return Err(bad(cmd, format!("{} already drives {} through motion links", name(l.b), name(l.a))));
        }
        match others.iter().find(|k| (k.b, k.ib) == at) {
            Some(k) => at = (k.a, k.ia),
            None => break,
        }
    }
    (l.ratio, l.lift) =
        joints::coupling_values(&s.doc, &s.model.state(), &l).unwrap_or(Ok((l.ratio, Vec::new()))).map_err(|e| bad(cmd, e.to_string()))?;
    let vals = joints::joint_values(&s.doc.assembly);
    let value = |id: u64, i: usize| vals.get(&id).and_then(|q| q.get(i)).copied().unwrap_or(0.0);
    // A link put in place of another keeps its offset, as a radius edited in the design does.
    l.offset = match replace.and_then(|i| links.get(i)) {
        Some(old) => old.offset,
        None => value(l.b, l.ib) - MotionLink { offset: 0.0, ..l.clone() }.follow(value(l.a, l.ia)),
    };
    if !l.offset.is_finite() {
        return Err(bad(cmd, "the coupling gives the driven joint no value where it is"));
    }
    let shown = l.ratio / ratio_scale(s, l.a, l.ia, l.b, l.ib);
    let doc = s.doc_mut();
    let index = match replace {
        Some(i) => {
            if let Some(slot) = doc.assembly.links.get_mut(i) {
                *slot = l;
            }
            i
        }
        None => {
            doc.assembly.links.push(l);
            doc.assembly.links.len() - 1
        }
    };
    let sol = resolve(s);
    let mut out = sol_json(&sol, p);
    out["link"] = json!(index);
    out["ratio"] = json!(shown);
    Ok(out)
}

fn is_angle(s: &Session, joint: u64, i: usize) -> bool {
    s.doc.assembly.joints.iter().find(|j| j.id == joint).is_some_and(|j| j.kind.is_angle(i))
}

/// A link's `ratio` is in b's units per a's (mm per degree, degrees per mm) and stored per
/// radian: the given one times this is the stored one.
fn ratio_scale(s: &Session, a: u64, ia: usize, b: u64, ib: usize) -> f64 {
    let unit = |angle: bool| if angle { 1f64.to_radians() } else { 1.0 };
    unit(is_angle(s, b, ib)) / unit(is_angle(s, a, ia))
}

fn limits(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.limits";
    let id = joint_id(s, p.get("joint"), cmd)?;
    let i = num(p, "index").unwrap_or(0.0).max(0.0) as usize;
    let kind = s.doc.assembly.joints.iter().find(|j| j.id == id).map(|j| j.kind).ok_or_else(|| bad(cmd, "joint"))?;
    if i >= kind.dofs().len() {
        return Err(bad(cmd, "no such value"));
    }
    let lim = match (p.get("min"), p.get("max")) {
        (Some(lo), Some(hi)) => Some((value(s, lo, kind.is_angle(i), cmd)?, value(s, hi, kind.is_angle(i), cmd)?)),
        (None, None) => None,
        _ => return Err(bad(cmd, "give both `min` and `max`, or neither to clear")),
    };
    let doc = s.doc_mut();
    let Some(j) = doc.assembly.joints.iter_mut().find(|j| j.id == id) else { return Err(bad(cmd, "joint")) };
    j.limits.resize(kind.dofs().len(), None);
    if let Some(slot) = j.limits.get_mut(i) {
        *slot = lim;
    }
    let mut q = j.values.clone();
    q.resize(kind.dofs().len(), 0.0);
    joints::clamp(j, &mut q);
    j.values = q;
    let sol = resolve(s);
    Ok(sol_json(&sol, p))
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.edit";
    let id = joint_id(s, p.get("joint"), cmd)?;
    let kind = match str_(p, "type") {
        Some(t) => Some(JointKind::parse(t).ok_or_else(|| bad(cmd, "unknown joint type"))?),
        None => None,
    };
    let offset = match p.get("offset") {
        Some(v) => Some(value(s, v, false, cmd)?),
        None => None,
    };
    let angle = match p.get("angle") {
        Some(v) => Some(value(s, v, true, cmd)?),
        None => None,
    };
    let doc = s.doc_mut();
    let Some(j) = doc.assembly.joints.iter_mut().find(|j| j.id == id) else { return Err(bad(cmd, "joint")) };
    if let Some(k) = kind {
        j.kind = k;
        j.values.resize(k.dofs().len(), 0.0);
        j.limits.resize(k.dofs().len(), None);
    }
    if let Some(o) = offset {
        j.offset = o;
    }
    if let Some(a) = angle {
        j.angle = a;
    }
    if let Some(f) = bool_(p, "flip") {
        j.flip = f;
    }
    if let Some(x) = bool_(p, "suppressed") {
        j.suppressed = x;
    }
    if let Some(n) = str_(p, "name").map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128) {
        j.name = n.to_string();
    }
    let sol = resolve(s);
    Ok(sol_json(&sol, p))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "joint.delete";
    if let Some(name) = str_(p, "origin") {
        let used = s.doc.assembly.joints.len();
        let doc = s.doc_mut();
        let before = doc.assembly.origins.len();
        doc.assembly.origins.retain(|o| o.name != name);
        if doc.assembly.origins.len() == before {
            return Err(bad(cmd, format!("no joint origin `{name}`")));
        }
        return Ok(json!({"deleted_origin": name, "joints": used}));
    }
    if let Some(i) = num(p, "link") {
        let i = i.max(0.0) as usize;
        let doc = s.doc_mut();
        if i >= doc.assembly.links.len() {
            return Err(bad(cmd, "no such motion link"));
        }
        doc.assembly.links.remove(i);
        return Ok(json!({"deleted_link": i}));
    }
    let id = joint_id(s, p.get("joint"), cmd)?;
    let doc = s.doc_mut();
    doc.assembly.joints.retain(|j| j.id != id);
    doc.assembly.links.retain(|l| l.a != id && l.b != id);
    let sol = resolve(s);
    let mut out = sol_json(&sol, p);
    out["deleted"] = json!(id);
    Ok(out)
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let st = s.model.state();
    let sol = joints::solve(&s.doc, &st);
    let vals = joints::joint_values(&s.doc.assembly);
    let js: Vec<Value> = s
        .doc
        .assembly
        .joints
        .iter()
        .map(|j| {
            let named = named_values(j, vals.get(&j.id).map(Vec::as_slice).unwrap_or_default());
            let world = |o: &JointOrigin| s.doc.origin_world(&st, o).ok().map(|m| json!({"origin": [m[3][0], m[3][1], m[3][2]], "z": [m[2][0], m[2][1], m[2][2]]}));
            json!({"id": j.id, "name": j.name, "type": j.kind, "a": j.a.occurrence, "b": j.b.occurrence, "values": named,
                "flip": j.flip, "offset": j.offset, "suppressed": j.suppressed, "visible": s.joint_visible(j.id), "frame_a": world(&j.a), "frame_b": world(&j.b)})
        })
        .collect();
    let links: Vec<Value> = s
        .doc
        .assembly
        .links
        .iter()
        .map(|l| {
            let angle = is_angle(s, l.b, l.ib);
            let offset = if angle { l.offset.to_degrees() } else { l.offset };
            let ratio = l.ratio / ratio_scale(s, l.a, l.ia, l.b, l.ib);
            let mut v =
                json!({"a": l.a, "ia": l.ia, "b": l.b, "ib": l.ib, "ratio": ratio, "offset": offset, "unit": if angle { "deg" } else { "mm" }});
            if let Some(c) = &l.coupling {
                v["coupling"] = json!(c);
                if let Some(Err(e)) = joints::coupling_values(&s.doc, &st, l) {
                    v["error"] = json!(e.to_string());
                }
            }
            // A cam's follower follows its table, not a ratio: its travel instead.
            if matches!(l.coupling, Some(Coupling::Cam(_)))
                && let Some(o) = v.as_object_mut()
            {
                o.remove("ratio");
                o.insert("lift".into(), lift_json(l, angle));
            }
            v
        })
        .collect();
    let origins: Vec<Value> = s.doc.assembly.origins.iter().map(|o| json!({"name": o.name, "occurrence": o.origin.occurrence})).collect();
    let mut out = sol_json(&sol, p);
    out["joints"] = json!(js);
    out["links"] = json!(links);
    out["origins"] = json!(origins);
    out["floating"] = json!(floating_chains(s, &sol));
    Ok(out)
}

fn solve_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let sol = resolve(s);
    Ok(sol_json(&sol, p))
}

fn interference(s: &mut Session, p: &Value) -> Result<Value> {
    let want = string_list(p, "bodies");
    let st = s.world_state();
    let bodies: Vec<&solidcraft_doc::ModelBody> =
        st.bodies.iter().filter(|b| want.is_empty() || want.contains(&b.name)).filter(|b| !b.body.is_mesh()).collect();
    // Prune by bounding boxes first (sort and sweep), so the cost follows the pairs that can
    // touch rather than the square of the body count.
    let boxes: Vec<Aabb3> = bodies.iter().map(|b| b.mesh().bounds()).collect();
    let (pairs, skipped) = overlapping_pairs(&boxes, MAX_INTERFERENCE_PAIRS);
    let (mut hits, mut unchecked) = (Vec::new(), Vec::new());
    for (i, j) in pairs {
        let (Some(a), Some(b)) = (bodies.get(i), bodies.get(j)) else { continue };
        let threshold = 1e-9 * a.body.size().max(1.0).powi(3);
        // A pin a hair off the axis of its hole: the crescent the walls share, which the
        // boolean can't intersect.
        if let Some(o) = solidcraft_kernel::coaxial_overlap(&a.body, &b.body) {
            if o.volume > threshold {
                hits.push(json!({"a": a.name, "b": b.name, "volume_mm3": o.volume, "area_mm2": o.area, "center": o.centroid,
                    "bbox": {"min": o.bbox.min, "max": o.bbox.max}, "near_coaxial": {"offset_mm": o.offset, "length_mm": o.length}}));
            }
            continue;
        }
        // A pair whose shared part can't be worked out is reported, not passed as clear.
        let (shared, report) =
            solidcraft_kernel::capture_boolean_diagnostics(|| solidcraft_kernel::boolean(&a.body, &b.body, solidcraft_kernel::BoolOp::Intersect));
        let mut skip = |reason: String| unchecked.push(json!({"a": a.name, "b": b.name, "reason": reason}));
        let shared = match shared {
            Ok(o) => o,
            Err(e) => {
                skip(e.to_string());
                continue;
            }
        };
        if let Some(reason) = report.unverified() {
            skip(reason);
        }
        let Some(o) = shared else { continue };
        let m = match solidcraft_kernel::measure(&o) {
            Ok(m) => m,
            Err(e) => {
                skip(e.to_string());
                continue;
            }
        };
        if m.volume > threshold {
            hits.push(json!({"a": a.name, "b": b.name, "volume_mm3": m.volume, "area_mm2": m.area, "center": m.centroid, "bbox": {"min": m.bbox.min, "max": m.bbox.max}}));
        }
    }
    Ok(json!({"interferences": hits, "count": hits.len(), "checked": bodies.len(), "unchecked": unchecked,
        "truncated": skipped > 0, "skipped_pairs": skipped}))
}

/// Most pairs with overlapping bounding boxes `inspect.interference` intersects in one call.
const MAX_INTERFERENCE_PAIRS: usize = 20_000;

/// Index pairs `(i, j)`, `i < j`, whose boxes overlap or touch (sort and sweep along X), at most
/// `limit` of them, plus how many more there were.
pub(crate) fn overlapping_pairs(boxes: &[Aabb3], limit: usize) -> (Vec<(usize, usize)>, usize) {
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|&a, &b| match (boxes.get(a), boxes.get(b)) {
        (Some(a), Some(b)) => a.min.x.total_cmp(&b.min.x),
        _ => std::cmp::Ordering::Equal,
    });
    let (mut pairs, mut skipped) = (Vec::new(), 0);
    for (k, &i) in order.iter().enumerate() {
        let Some(a) = boxes.get(i) else { continue };
        for &j in order.iter().skip(k + 1) {
            let Some(b) = boxes.get(j) else { continue };
            // Sorted by min.x: once one starts past `a`, all later ones do.
            if b.min.x > a.max.x {
                break;
            }
            if b.max.x < a.min.x || a.max.y < b.min.y || b.max.y < a.min.y || a.max.z < b.min.z || b.max.z < a.min.z {
                continue;
            }
            if pairs.len() < limit {
                pairs.push((i.min(j), i.max(j)));
            } else {
                skipped += 1;
            }
        }
    }
    pairs.sort_unstable();
    (pairs, skipped)
}

#[cfg(test)]
#[path = "joints_reach_tests.rs"]
mod reach_tests;
#[cfg(test)]
#[path = "joints_tests.rs"]
mod tests;

/// A point in the middle of the cylindrical face at `pick` (on its wall, halfway along).
fn cylinder_wall_point(b: &solidcraft_doc::kernel::Body, pick: Vec3) -> Option<Vec3> {
    let c = solidcraft_doc::kernel::cylinder_face_at(b, pick)?;
    let v = pick - c.axis_point;
    let along = v.dot(c.axis);
    let radial = (v - c.axis * along).normalized()?;
    Some(c.axis_point + c.axis * ((c.start + c.end) / 2.0) + radial * c.radius)
}
