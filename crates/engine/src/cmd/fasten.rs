//! Fill hole stacks with fasteners: for each stack (`inspect.hole_stacks`), a rivet, bolt,
//! screw or pin of a family, sized to pass the narrowest hole and long enough for the grip,
//! seated on the stack's first hole with a rigid joint, with washers and a nut where asked.
//!
//! Everything is worked out first (sizes, lengths, the faces and the hole walls to joint to);
//! then each distinct part is built once and placed again as further occurrences, and the
//! joints are solved once at the end.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use solidcraft_doc::joints::{JointOrigin, Snap};

use super::CommandSpec;
use super::hole_stacks::{Layer, Options, SlotShape, Stack};
use super::{joints, parts};
use crate::params::{bad, bool_, num, str_, vec3};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("parts.fasten", "Fill Hole Stacks", fasten).icon("fastener").enabled(super::not_in_sketch).params(
    "family: a fastener family (parts.library: built-in or a library's); stack?: index | stack object, stacks?: [index | stack object | {index, head_side?}] | \"all\" \
     (default all; indices and \"all\" are inspect.hole_stacks with this command's bodies?, diameters?, slots?, touching?, axis_tolerance?; a stack object is one of its results, \
     with head_side? of its own); size? (default the largest that passes the narrowest hole and has a length); \
     rule?: \"grip\" (default: a library family's shortest length whose grip range holds the grip, else the shortest length at least the grip, plus the nut's height and \
     two pitches with a nut) | \"thread\" (a screw into a tapped far layer: sized to pass the other layers, the shortest length at least their grip plus engagement?: mm, \
     default 1.5 × d); head_side?: start | end | {body} (default start: the head on the stack's start face; {body}: on the face of that body's layer); \
     washer?: true | family | {family, size?}; washer_side?: head | nut | both (default both with a nut, else head); \
     nut?: true | family | {family, size?} (on the far face; true: the built-in hex_nut and washer); \
     drawn?: \"set\" | \"unset\" (blind rivets; default set: the shank as long as the grip, with its formed tail beyond the far face); dry_run?: bool (only work out what goes where) \
     → placed: [{component, occurrence, joint, stack, head_side, size, length_mm, grip_mm, grip_range?, article?, drawn?, part, washers?: [occurrence], nut?: occurrence}], \
     unfilled: [{stack, reason}]. Each distinct part is one component, placed as occurrences. One undo step",
)];

/// Stacks filled by one command, at most.
const MAX_STACKS: usize = 2000;

/// A companion part (washer, nut): its family and size (none: the fastener's size).
#[derive(Clone, Debug)]
struct Companion {
    family: String,
    size: Option<String>,
}

fn companion(p: &Value, key: &str, builtin: &str, cmd: &str) -> Result<Option<Companion>> {
    match p.get(key) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some(Companion { family: builtin.into(), size: None })),
        Some(Value::String(f)) => Ok(Some(Companion { family: f.clone(), size: None })),
        Some(Value::Object(o)) => {
            let family = o.get("family").and_then(Value::as_str).ok_or_else(|| bad(cmd, format!("`{key}.family` is required")))?;
            let size = match o.get("size") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return Err(bad(cmd, format!("`{key}.size` must be text"))),
            };
            Ok(Some(Companion { family: family.into(), size }))
        }
        Some(_) => Err(bad(cmd, format!("`{key}` must be true, a family name or {{family, size?}}"))),
    }
}

/// A stack from its JSON (as `inspect.hole_stacks` gives it).
fn stack_from(v: &Value) -> Option<Stack> {
    let axis = v.get("axis").and_then(vec3)?.normalized()?;
    let start = v.get("start").and_then(vec3)?;
    let grip = v.get("grip_mm").and_then(Value::as_f64).filter(|g| g.is_finite() && *g > 0.0 && *g <= 10_000.0)?;
    let finite = |p: Vec3| p.x.is_finite() && p.y.is_finite() && p.z.is_finite() && p.len() < 1e9;
    if !finite(start) {
        return None;
    }
    let layers = v.get("layers")?.as_array()?;
    if layers.is_empty() || layers.len() > 64 {
        return None;
    }
    let layers = layers
        .iter()
        .map(|l| {
            let d = l.get("d_mm")?.as_f64().filter(|d| d.is_finite() && *d > 0.0 && *d <= 10_000.0)?;
            let t = l.get("t")?.as_array()?;
            let (t0, t1) = (t.first()?.as_f64()?, t.get(1)?.as_f64()?);
            if !(t0.is_finite() && t1.is_finite() && t0 <= t1) {
                return None;
            }
            let slot = match l.get("slot") {
                None | Some(Value::Null) => None,
                Some(sv) => Some(SlotShape {
                    length: sv.get("length_mm")?.as_f64().filter(|x| x.is_finite() && *x > 0.0)?,
                    along: sv.get("along").and_then(vec3)?.normalized()?,
                }),
            };
            Some(Layer { body: l.get("body")?.as_str()?.to_string(), d, t0, t1, slot })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Stack { axis, start, grip, layers })
}

/// Which face of a stack the head goes on.
#[derive(Clone, Debug, PartialEq)]
enum HeadSide {
    Start,
    End,
    /// The face of the layer of this body.
    Body(String),
}

fn head_side(v: &Value, cmd: &str) -> Result<HeadSide> {
    match v {
        Value::String(x) if x == "start" => Ok(HeadSide::Start),
        Value::String(x) if x == "end" => Ok(HeadSide::End),
        Value::Object(o) => match o.get("body").and_then(Value::as_str) {
            Some(b) => Ok(HeadSide::Body(b.to_string())),
            None => Err(bad(cmd, "`head_side` must be start, end or {body}")),
        },
        _ => Err(bad(cmd, "`head_side` must be start, end or {body}")),
    }
}

/// A stack to fill: what the result calls it, the stack, and a head side of its own.
type Target = (Value, Stack, Option<HeadSide>);

/// The stacks to fill.
fn targets(s: &Session, p: &Value, cmd: &str) -> Result<Vec<Target>> {
    let mut found: Option<Vec<Stack>> = None;
    let mut by_index = |s: &Session, i: usize| -> Result<Stack> {
        if found.is_none() {
            let o = Options::parse(cmd, p)?;
            found = Some(super::hole_stacks::stacks(&super::hole_stacks::holes(s, &o, cmd)?, &o).0);
        }
        found.as_ref().and_then(|f| f.get(i).cloned()).ok_or_else(|| bad(cmd, format!("no hole stack {i} (inspect.hole_stacks lists them)")))
    };
    let index = |n: &Value| n.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(cmd, "a stack index is a whole number"));
    let one = |s: &Session, v: &Value, at: usize, by_index: &mut dyn FnMut(&Session, usize) -> Result<Stack>| -> Result<Target> {
        let side = match v.get("head_side") {
            Some(h) => Some(head_side(h, cmd)?),
            None => None,
        };
        match v {
            Value::Number(_) => {
                let i = index(v)?;
                Ok((json!(i), by_index(s, i)?, None))
            }
            Value::Object(o) if o.contains_key("index") => {
                let i = index(o.get("index").unwrap_or(&Value::Null))?;
                Ok((json!(i), by_index(s, i)?, side))
            }
            Value::Object(_) => Ok((
                json!(at),
                stack_from(v).ok_or_else(|| bad(cmd, format!("stack {at} is not a hole stack (inspect.hole_stacks gives them)")))?,
                side,
            )),
            _ => Err(bad(cmd, "a stack is an index or a stack object from inspect.hole_stacks")),
        }
    };
    match (p.get("stack"), p.get("stacks")) {
        (Some(_), Some(_)) => Err(bad(cmd, "give `stack` or `stacks`, not both")),
        (Some(v), None) => Ok(vec![one(s, v, 0, &mut by_index)?]),
        (None, Some(Value::Array(list))) => {
            if list.len() > MAX_STACKS {
                return Err(bad(cmd, format!("at most {MAX_STACKS} stacks")));
            }
            list.iter().enumerate().map(|(i, v)| one(s, v, i, &mut by_index)).collect()
        }
        (None, None) | (None, Some(Value::String(_))) if p.get("stacks").is_none_or(|v| v == "all") => {
            let o = Options::parse(cmd, p)?;
            let all = super::hole_stacks::stacks(&super::hole_stacks::holes(s, &o, cmd)?, &o).0;
            if all.len() > MAX_STACKS {
                return Err(bad(cmd, format!("{} stacks: at most {MAX_STACKS} (pass `bodies` or `diameters`)", all.len())));
            }
            Ok(all.into_iter().enumerate().map(|(i, st)| (json!(i), st, None)).collect())
        }
        _ => Err(bad(cmd, "`stacks` must be a list or \"all\"")),
    }
}

/// How the length is chosen.
#[derive(Clone, Copy, Debug)]
enum Rule {
    /// The whole stack is clamped (a rivet, or a bolt with a nut).
    Grip,
    /// A screw into a tapped far layer: the other layers are clamped, and it reaches this far
    /// into the thread (none: 1.5 × d).
    Thread(Option<f64>),
}

/// The options of one fill.
struct Fill {
    family: String,
    size: Option<String>,
    rule: Rule,
    head_side: HeadSide,
    /// A washer under the head, and one under the nut.
    washer: (Option<Companion>, Option<Companion>),
    nut: Option<Companion>,
    /// Blind rivets drawn set on their stack's grip.
    set: bool,
    dry_run: bool,
}

impl Fill {
    fn parse(s: &Session, p: &Value, cmd: &str) -> Result<Self> {
        let family = str_(p, "family").ok_or_else(|| bad(cmd, "`family` is required (parts.library)"))?.to_string();
        parts::sizes(s, &family).map_err(|e| bad(cmd, e))?;
        let size = match p.get("size") {
            None | Some(Value::Null) => None,
            Some(Value::String(z)) => Some(z.clone()),
            Some(_) => return Err(bad(cmd, "`size` must be text (\"M5\", \"4.8\")")),
        };
        let engagement = match p.get("engagement") {
            None | Some(Value::Null) => None,
            Some(_) => Some(
                num(p, "engagement")
                    .filter(|e| e.is_finite() && *e > 0.0 && *e <= 2000.0)
                    .ok_or_else(|| bad(cmd, "`engagement` must be 0…2000 mm"))?,
            ),
        };
        let rule = match str_(p, "rule") {
            None | Some("grip") if engagement.is_none() => Rule::Grip,
            None | Some("grip") => return Err(bad(cmd, "`engagement` goes with rule \"thread\"")),
            Some("thread") => Rule::Thread(engagement),
            Some(r) => return Err(bad(cmd, format!("not supported yet: rule `{r}` (\"grip\" or \"thread\")"))),
        };
        let head_side = match p.get("head_side") {
            None | Some(Value::Null) => HeadSide::Start,
            Some(v) => head_side(v, cmd)?,
        };
        let nut = companion(p, "nut", "hex_nut", cmd)?;
        if nut.is_some() && matches!(rule, Rule::Thread(_)) {
            return Err(bad(cmd, "rule \"thread\" screws into the far layer: it takes no nut"));
        }
        let washer = companion(p, "washer", "washer", cmd)?;
        let side = str_(p, "washer_side");
        let washer = match (washer, side) {
            (None, Some(_)) => return Err(bad(cmd, "`washer_side` needs `washer`")),
            (None, None) => (None, None),
            (Some(w), None | Some("both")) => (Some(w.clone()), nut.is_some().then_some(w)),
            (Some(w), Some("head")) => (Some(w), None),
            (Some(_), Some("nut")) if nut.is_none() => return Err(bad(cmd, "`washer_side: nut` needs `nut`")),
            (Some(w), Some("nut")) => (None, Some(w)),
            (Some(_), Some(o)) => return Err(bad(cmd, format!("`washer_side` must be head, nut or both, not `{o}`"))),
        };
        let dry_run = match p.get("dry_run") {
            None | Some(Value::Null) => false,
            Some(_) => bool_(p, "dry_run").ok_or_else(|| bad(cmd, "`dry_run` must be true or false"))?,
        };
        let rivet = parts::blind_rivet(s, &family);
        let set = match str_(p, "drawn") {
            None => rivet,
            Some("unset") => false,
            Some("set") if rivet => true,
            Some("set") => return Err(bad(cmd, "`drawn: set` is for blind rivets")),
            Some(d) => return Err(bad(cmd, format!("`drawn` must be set or unset, not `{d}`"))),
        };
        Ok(Self { family, size, rule, head_side, washer, nut, set, dry_run })
    }
}

/// A part to place: family, size and length (none for a nut or washer), and the grip a blind
/// rivet is drawn set on.
#[derive(Clone, Debug, PartialEq)]
struct Part {
    family: String,
    size: String,
    length: Option<f64>,
    set: Option<f64>,
}

/// What to place in one stack, worked out before anything is built.
struct Plan {
    size: String,
    length: f64,
    grip: Option<[f64; 2]>,
    article: Option<String>,
    head_washer: Option<(Part, f64)>,
    nut_washer: Option<(Part, f64)>,
    nut: Option<(Part, f64)>,
    /// The grip a blind rivet is drawn set on (the stack's and its washers').
    set: Option<f64>,
}

/// Size and length for a stack whose head goes on `head` (the far layer is `far`).
fn plan(s: &Session, st: &Stack, far: &Layer, f: &Fill) -> std::result::Result<Plan, String> {
    let family = f.family.as_str();
    let thread = matches!(f.rule, Rule::Thread(_));
    // The holes the shank passes: all of them, or with a thread all but the far layer's.
    let passes: Vec<&Layer> = st.layers.iter().filter(|l| !thread || *l != far).collect();
    let narrowest = passes.iter().map(|l| l.d).fold(f64::INFINITY, f64::min);
    let sizes = parts::sizes(s, family)?;
    let candidates: Vec<&(String, f64)> = match &f.size {
        Some(z) => {
            let c = sizes.iter().find(|(n, _)| n.eq_ignore_ascii_case(z.trim())).ok_or_else(|| format!("no size `{z}` in `{family}`"))?;
            if c.1 > narrowest + 1e-6 {
                return Err(format!("{family} {} (d {} mm) does not pass a {:.3} mm hole", c.0, c.1, narrowest));
            }
            vec![c]
        }
        None => {
            let mut fit: Vec<&(String, f64)> = sizes.iter().filter(|(_, d)| *d <= narrowest + 1e-6).collect();
            fit.sort_by(|a, b| b.1.total_cmp(&a.1));
            if fit.is_empty() {
                return Err(format!("no size of {family} passes a {narrowest:.3} mm hole"));
            }
            fit
        }
    };
    let mut why = String::new();
    for (name, d) in candidates {
        if thread && far.d >= d - 1e-6 {
            why = format!("the far layer's {:.3} mm hole passes {family} {name}: nothing to screw into (rule \"grip\" with a nut?)", far.d);
            continue;
        }
        let part = |c: &Option<Companion>| -> std::result::Result<Option<(Part, f64)>, String> {
            match c {
                None => Ok(None),
                Some(c) => {
                    let z = c.size.clone().unwrap_or_else(|| name.clone());
                    let h = parts::thickness(s, &c.family, &z)?;
                    Ok(Some((Part { family: c.family.clone(), size: z, length: None, set: None }, h)))
                }
            }
        };
        let (hw, nw, n) = match (part(&f.washer.0), part(&f.washer.1), part(&f.nut)) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                why = e;
                continue;
            }
        };
        let washers = hw.as_ref().map_or(0.0, |w| w.1) + nw.as_ref().map_or(0.0, |w| w.1);
        let (clamp, extra) = match f.rule {
            Rule::Grip => {
                let extra = n.as_ref().map_or(0.0, |(_, h)| h + 2.0 * parts::pitch(s, family, name).unwrap_or(0.0));
                (st.grip + washers, extra)
            }
            Rule::Thread(e) => (st.grip - (far.t1 - far.t0) + washers, e.unwrap_or(1.5 * d)),
        };
        match parts::length_for(s, family, name, clamp, extra, matches!(f.rule, Rule::Grip)) {
            Ok(c) => {
                let set = f.set.then(|| super::hole_stacks::round(clamp));
                return Ok(Plan {
                    size: name.clone(),
                    length: c.length,
                    grip: c.grip,
                    article: c.article,
                    head_washer: hw,
                    nut_washer: nw,
                    nut: n,
                    set,
                });
            }
            Err(e) => why = e,
        }
    }
    Err(why)
}

/// A unit vector across `axis`, the way to a hole's rim.
fn across(axis: Vec3) -> Vec3 {
    axis.any_perp().normalized().unwrap_or(Vec3::X)
}

/// One face of a stack a part seats on: the point on the axis there, the way out of the stack,
/// and the hole wall to joint to (none for a slot, which has no circle).
struct Face {
    at: Vec3,
    out: Vec3,
    hole: Option<JointOrigin>,
}

/// The occurrence that holds `body` (the root's bodies: 0), or the one nearest `at`.
fn host(s: &Session, body: &str, at: Vec3) -> Option<u64> {
    let st = s.model.state();
    if let Some(b) = st.bodies.iter().find(|b| b.name == body) {
        let comp = s.doc.model_body_component(b);
        if comp == 0 {
            return Some(0);
        }
        let mut occs = s.doc.occurrences.iter().filter(|o| o.component == comp);
        if let (Some(o), None) = (occs.next(), occs.next()) {
            return Some(o.id);
        }
    }
    joints::occurrence_at(s, at)
}

/// The face of `layer` at `at`, found before anything is placed in the stack (so a part already
/// in the hole is not taken for its wall). The joint takes the hole by a point on its wall just
/// in from the face, on the stack's own axis, whatever way the axis points. Where that point
/// gives no origin (it lies on the seam between the two faces of a cut cylinder), or one off
/// this face along the axis, a few other points round the hole are tried; failing those, the
/// first origin found is kept.
fn face(s: &Session, at: Vec3, out: Vec3, layer: &Layer, cmd: &str) -> std::result::Result<Face, String> {
    if layer.slot.is_some() {
        return Ok(Face { at, out, hole: None });
    }
    let inset = ((layer.t1 - layer.t0) * 0.25).clamp(1e-3, 0.5);
    let (u, v) = (across(out), out.cross(across(out)));
    // Along the axis, within a quarter of the layer of the face (holes in one stack may sit a
    // little apart across it, so only the distance along it counts).
    let near = ((layer.t1 - layer.t0) * 0.25).max(0.15);
    let (mut why, mut first) = (String::new(), None);
    for k in [0.0, 1.0, 3.0, 5.0, 7.0] {
        let a = k * std::f64::consts::FRAC_PI_4;
        let wall = at + (u * a.cos() + v * a.sin()) * (layer.d / 2.0) - out * inset;
        let Some(occ) = host(s, &layer.body, wall) else {
            return Err(format!("no body `{}` to seat on", layer.body));
        };
        match joints::origin(s, &json!({"occurrence": occ, "circle": wall}), cmd) {
            Ok(origin) => {
                let seat = solidcraft_doc::joints::resolve_origin(&s.doc, &s.model.state(), &origin).ok().map(|f| {
                    solidcraft_doc::apply_point(
                        &solidcraft_doc::joints::occurrence_world(&s.doc, origin.occurrence),
                        Vec3::new(f[3][0], f[3][1], f[3][2]),
                    )
                });
                if seat.is_some_and(|q| (q - at).dot(out).abs() <= near) {
                    return Ok(Face { at, out, hole: Some(origin) });
                }
                first.get_or_insert(origin);
            }
            Err(e) if why.is_empty() => why = format!("no hole wall of `{}` to seat on at {:?}: {e}", layer.body, super::hole_stacks::round3(wall)),
            Err(_) => {}
        }
    }
    first.map(|origin| Face { at, out, hole: Some(origin) }).ok_or(why)
}

/// Its placement with +z along `out` at `at`.
fn placement(at: Vec3, out: Vec3) -> solidcraft_doc::Mat {
    let mut m =
        solidcraft_doc::rigid(Vec3::ZERO, Vec3::ZERO, Vec3::Z.cross(out).normalized().unwrap_or(Vec3::X), Vec3::Z.dot(out).clamp(-1.0, 1.0).acos());
    m[3] = [at.x, at.y, at.z, 1.0];
    m
}

/// One stack's parts, ready to place.
struct Job {
    id: Value,
    grip: f64,
    head_end: bool,
    plan: Plan,
    head: Face,
    tail: Face,
}

/// The components placed so far, by part: a new one's first occurrence is used before another
/// is added.
#[derive(Default)]
struct Built {
    parts: Vec<(Part, u64, Option<u64>, String)>,
}

impl Built {
    /// Place `part` on face `f`, `offset` out from it, jointed to its hole: its occurrence and
    /// joint.
    fn place(&mut self, s: &mut Session, part: &Part, f: &Face, offset: f64) -> Result<(u64, Option<u64>)> {
        let i = match self.parts.iter().position(|(p, ..)| p == part) {
            Some(i) => i,
            None => {
                let (comp, fresh) = match parts::existing_part(s, &part.family, &part.size, part.length, part.set) {
                    Some(c) => (c, None),
                    None => {
                        let mut p = json!({"family": part.family, "size": part.size});
                        if let Some(l) = part.length {
                            p["length"] = json!(l);
                        }
                        if let Some(g) = part.set {
                            p["drawn"] = json!("set");
                            p["grip"] = json!(g);
                        }
                        let r = parts::insert_part(s, &p)?;
                        (r["component"].as_u64().unwrap_or(0), r["occurrence"].as_u64())
                    }
                };
                let name = s.doc.components.iter().find(|c| c.id == comp).map(|c| c.name.clone()).unwrap_or_default();
                self.parts.push((part.clone(), comp, fresh, name));
                self.parts.len() - 1
            }
        };
        let Some((_, comp, fresh, name)) = self.parts.get_mut(i) else { return Err(bad("parts.fasten", "part")) };
        let (comp, name) = (*comp, name.clone());
        let m = placement(f.at + f.out * offset, f.out);
        let occ = match fresh.take() {
            Some(o) => {
                if let Some(x) = s.doc_mut().occurrences.iter_mut().find(|x| x.id == o) {
                    x.transform = m;
                }
                o
            }
            None => {
                let parent = s.doc.occurrence_of(comp).map_or(s.active_component, |o| o.parent);
                s.doc_mut().add_occurrence(comp, parent, m)?
            }
        };
        let joint = match &f.hole {
            Some(a) => {
                let b = JointOrigin { occurrence: occ, snap: Snap::Point { point: Vec3::ZERO, z: Some(Vec3::Z * -1.0) } };
                Some(joints::add_rigid_unsolved(s, a.clone(), b, offset, format!("{name} seat"))?)
            }
            None => None,
        };
        Ok((occ, joint))
    }
}

fn fasten(s: &mut Session, p: &Value) -> Result<Value> {
    // The commands it runs push their own undo steps: drop them, so the whole fill (or a
    // failed one, which leaves the design as it was) is the one step `Session::execute` keeps.
    let depth = s.undo.len();
    // The parts it builds would solve every joint of the design on each of their commands:
    // the joints are solved once, at the end.
    let deferred = std::mem::replace(&mut s.defer_joints, true);
    let r = fill(s, p);
    s.defer_joints = deferred;
    s.undo.truncate(depth);
    r
}

fn fill(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parts.fasten";
    let f = Fill::parse(s, p, cmd)?;
    let targets = targets(s, p, cmd)?;
    // Work everything out before anything is placed.
    let (mut jobs, mut unfilled) = (Vec::new(), Vec::new());
    for (id, st, side) in targets {
        let (Some(first), Some(last)) = (st.layers.first(), st.layers.last()) else { continue };
        let head_end = match side.as_ref().unwrap_or(&f.head_side) {
            HeadSide::Start => false,
            HeadSide::End => true,
            HeadSide::Body(b) if first.body == *b => false,
            HeadSide::Body(b) if last.body == *b => true,
            HeadSide::Body(b) => {
                unfilled.push(json!({"stack": id, "reason": format!("neither outer layer is `{b}`")}));
                continue;
            }
        };
        let (near, far) = if head_end { (last, first) } else { (first, last) };
        let plan = match plan(s, &st, far, &f) {
            Ok(plan) => plan,
            Err(reason) => {
                unfilled.push(json!({"stack": id, "reason": reason}));
                continue;
            }
        };
        let (start, end) = (st.start, st.end());
        let faces = if head_end {
            face(s, end, st.axis, near, cmd).and_then(|h| Ok((h, face(s, start, st.axis * -1.0, far, cmd)?)))
        } else {
            face(s, start, st.axis * -1.0, near, cmd).and_then(|h| Ok((h, face(s, end, st.axis, far, cmd)?)))
        };
        match faces {
            Ok((head, tail)) => jobs.push(Job { id, grip: st.grip, head_end, plan, head, tail }),
            Err(reason) => unfilled.push(json!({"stack": id, "reason": reason})),
        }
    }
    let identity = |plan: &Plan, family: &str| plan.article.clone().unwrap_or_else(|| format!("{family}/{}x{}", plan.size, plan.length));
    let mut placed = Vec::new();
    let mut built = Built::default();
    for job in &jobs {
        let plan = &job.plan;
        let mut v = json!({
            "stack": job.id, "head_side": if job.head_end { "end" } else { "start" }, "size": plan.size, "length_mm": plan.length,
            "grip_mm": super::hole_stacks::round(job.grip), "part": identity(plan, &f.family),
        });
        if let Some(g) = plan.grip {
            v["grip_range"] = json!(g);
        }
        if let Some(a) = &plan.article {
            v["article"] = json!(a);
        }
        if plan.set.is_some() {
            v["drawn"] = json!("set");
        }
        if f.dry_run {
            placed.push(v);
            continue;
        }
        let mut washers = Vec::new();
        let mut under_head = 0.0;
        if let Some((w, h)) = &plan.head_washer {
            washers.push(built.place(s, w, &job.head, 0.0)?.0);
            under_head = *h;
        }
        let fastener = Part { family: f.family.clone(), size: plan.size.clone(), length: Some(plan.length), set: plan.set };
        let (occ, joint) = built.place(s, &fastener, &job.head, under_head)?;
        let mut under_nut = 0.0;
        if let Some((w, h)) = &plan.nut_washer {
            washers.push(built.place(s, w, &job.tail, 0.0)?.0);
            under_nut = *h;
        }
        if let Some((n, _)) = &plan.nut {
            v["nut"] = json!(built.place(s, n, &job.tail, under_nut)?.0);
        }
        let comp = s.doc.occurrences.iter().find(|o| o.id == occ).map_or(0, |o| o.component);
        v["component"] = json!(comp);
        v["occurrence"] = json!(occ);
        v["joint"] = json!(joint);
        v["name"] = json!(built.parts.iter().find(|(_, c, ..)| *c == comp).map(|(.., n)| n.clone()));
        if !washers.is_empty() {
            v["washers"] = json!(washers);
        }
        placed.push(v);
    }
    let mut out = json!({"placed": placed, "unfilled": unfilled});
    if f.dry_run {
        out["dry_run"] = json!(true);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "fasten_tests.rs"]
mod tests;
