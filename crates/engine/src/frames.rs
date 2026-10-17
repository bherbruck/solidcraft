//! Component frames for picks. The viewport shows the placed model (`Session::world_state`):
//! a component's bodies and sketches moved by its occurrence (and pasted instances moved by
//! theirs). Features are made and kept in their component's own frame. So a command's picked
//! points and directions, given in the world, are mapped into the frame of the body they lie on
//! (through the inverse of that body's placement), and points that create geometry into the
//! active component's frame, before the command runs. Instance body names ("Body1 (Occ:2)")
//! become the component's body name.
//!
//! Nothing changes while every placement is the identity (the common case, and every design
//! without moved components).

use serde_json::{Map, Value};
use solidcraft_doc::{Mat, apply_point, apply_vector, is_identity, map_point_expr, mat_inverse};
use solidcraft_geom::Vec3;

use crate::Session;

/// How a parameter is mapped.
#[derive(Clone, Copy, PartialEq)]
enum K {
    /// A point on a body (a pick): into that body's frame.
    Pick,
    /// A list of picks: `[[x,y,z]…]` or `[{point|face|edge: [x,y,z], body?}…]`.
    Picks,
    /// A point that places new geometry: into the active component's frame.
    Make,
    /// A direction: turned with the command's frame.
    Dir,
    /// An arbitrary axis object in the authoring frame: {origin, dir}.
    Axis,
    /// A plane object (`{face}`, `{origin, normal}`, `{origin, x_dir, y_dir}`) or a plane name.
    Plane,
    /// A body name, or a list of them.
    Body,
    /// A point (or a list of picks) for the sketch being made or edited: into that sketch's
    /// component frame, whichever component the geometry under it belongs to (a sketch in the
    /// root on a moved component's face finds the face where it is shown).
    Here,
    /// A plane object for a new sketch or canvas (or a mirror's copies): into the active
    /// component's frame.
    HerePlane,
}

use K::*;

/// The parameters each command takes in world coordinates.
fn table(id: &str) -> &'static [(&'static str, K)] {
    match id {
        "solid.extrude" => &[("face", Pick), ("face_body", Body), ("from", Make), ("to", Make), ("targets", Body), ("participants", Body)],
        "solid.revolve" => &[("face", Pick), ("axis", Axis), ("to", Make), ("targets", Body), ("participants", Body)],
        "solid.box" | "solid.sphere" => &[("corner", Make), ("center", Make)],
        "solid.cylinder" | "solid.coil" | "solid.torus" => &[("base", Make), ("center", Make), ("axis", Dir)],
        "solid.fillet" | "solid.chamfer" => &[("edges", Picks), ("body", Body)],
        "solid.hole" => &[("position", Pick), ("direction", Dir), ("body", Body)],
        "solid.scale" => &[("origin", Make), ("bodies", Body)],
        "solid.offset_face" | "solid.shell" => &[("faces", Picks), ("body", Body)],
        "solid.thread" => &[("face", Pick)],
        "solid.emboss" => &[("face", Make)],
        "solid.draft" => &[("faces", Picks), ("neutral", Plane), ("pull", Dir), ("body", Body)],
        "solid.press_pull" => &[("face", Pick), ("edges", Picks)],
        "solid.replace_face" => &[("faces", Picks), ("target_face", Pick), ("target", Plane), ("body", Body)],
        "solid.align" => &[("from", Pick), ("from_face", Pick), ("to", Pick), ("to_face", Pick), ("bodies", Body)],
        // A mirror keeps the placement its bodies were picked at (`cmd::features::placed_bodies`);
        // its plane is in the active component, where the copies go.
        "solid.mirror" => &[("plane", HerePlane)],
        "solid.split_body" => &[("plane", Plane), ("bodies", Body), ("body", Body)],
        "plastic.boss" | "plastic.snap_fit" | "plastic.rest" => {
            &[("position", Pick), ("direction", Dir), ("hook", Dir), ("along", Dir), ("body", Body)]
        }
        "plastic.lip" => &[("face", Pick), ("body", Body)],
        "sheet.convert" => &[("face", Pick), ("bends", Picks), ("body", Body)],
        "sheet.flange" | "sheet.hem" => &[("edges", Picks), ("body", Body)],
        "sheet.fold" => &[("points", Picks), ("fixed", Pick), ("body", Body)],
        "sheet.perforate" => &[("body", Body)],
        "sketch.create" | "sketch.redefine" => &[("plane", HerePlane)],
        "canvas.decal" => &[("face", Here)],
        "canvas.insert" => &[("plane", HerePlane), ("at", Here)],
        "sketch.project" | "sketch.intersect" | "sketch.include_3d" => &[("refs", Here), ("ref", Here)],
        "sketch.project_to_surface" | "sketch.isoparametric_curve" => &[("face", Here)],
        "sketch.intersection_curve" => &[("a", Here), ("b", Here)],
        "inspect.curvature_comb" | "inspect.minimum_radius" | "inspect.isocurve" => &[("edges", Picks), ("faces", Picks)],
        "parts.insert" | "parts.fastener" => &[("at", Pick), ("point", Make), ("direction", Dir)],
        "appearance.assign" => &[("faces", Picks), ("bodies", Body), ("body", Body)],
        "solid.move" => &[("bodies", Body), ("translate", Dir), ("axis", Dir), ("origin", Make)],
        "solid.combine" => &[("target", Body), ("tools", Body)],
        "solid.remove" | "solid.bounding_solid" | "material.assign" => &[("bodies", Body)],
        _ => &[],
    }
}

/// The world placement of bodies (name → frame), with moves not captured yet.
struct Frames {
    bodies: Vec<(String, String, Mat)>,
    active: Mat,
    /// The frame of the sketch being edited (its component), else the active component's.
    sketch: Mat,
}

fn frames(s: &Session) -> Frames {
    let st = s.model.state();
    let doc = s.placed_document();
    // World name, the component's own body name, and its frame.
    let bodies = doc.body_frames_with_source(&st).into_iter().map(|(world, own, _, _, m)| (world, own, m)).collect();
    let sketch_comp = s.active_sketch.and_then(|id| doc.feature(id)).map(|f| f.component).unwrap_or(s.active_component);
    let active = s.active_component_frame();
    Frames { bodies, active, sketch: if sketch_comp == s.active_component { active } else { doc.component_transform(sketch_comp) } }
}

/// Does world geometry or its instance naming differ from source components?
pub fn placed(s: &Session) -> bool {
    let mut components = std::collections::BTreeSet::new();
    !s.pending_moves.is_empty()
        || !s.doc.body_offsets.is_empty()
        || s.doc.occurrences.iter().any(|o| !is_identity(&o.transform) || !components.insert(o.component))
}

fn vec3(v: &Value) -> Option<Vec3> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let c = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite());
    Some(Vec3::new(c(0)?, c(1)?, c(2)?))
}

fn pt(p: Vec3) -> Value {
    serde_json::json!([p.x, p.y, p.z])
}

/// The frame of the body a world point lies on (the nearest), or of the named body.
fn frame_at(s: &Session, f: &Frames, p: Vec3, body: Option<&str>) -> Option<Mat> {
    if let Some(b) = body
        && let Some((_, _, m)) = f.bodies.iter().find(|(w, _, _)| w == b)
    {
        return Some(*m);
    }
    let world = s.world_state();
    let mut best: Option<(f64, Mat)> = None;
    for wb in &world.bodies {
        let Some((_, _, m)) = f.bodies.iter().find(|(w, _, _)| *w == wb.name) else { continue };
        let mesh = wb.mesh();
        let bb = mesh.bounds();
        let tol = (bb.diagonal() * 1e-3).max(1e-3);
        if p.x < bb.min.x - tol
            || p.y < bb.min.y - tol
            || p.z < bb.min.z - tol
            || p.x > bb.max.x + tol
            || p.y > bb.max.y + tol
            || p.z > bb.max.z + tol
        {
            continue;
        }
        let d = mesh.triangles.iter().filter_map(|t| mesh.tri(t)).map(|[a, b, c]| tri_dist(p, a, b, c)).fold(f64::INFINITY, f64::min);
        if d <= tol && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
            best = Some((d, *m));
        }
    }
    best.map(|(_, m)| m)
}

fn tri_dist(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f64 {
    let n = (b - a).cross(c - a);
    let Some(nn) = n.normalized() else { return p.dist_to_segment(a, b).min(p.dist_to_segment(b, c)) };
    let h = (p - a).dot(nn);
    let q = p - nn * h;
    let inside = [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).cross(q - *u).dot(n) >= -1e-12);
    if inside { h.abs() } else { [(a, b), (b, c), (c, a)].iter().map(|(u, v)| p.dist_to_segment(*u, *v)).fold(f64::INFINITY, f64::min) }
}

struct Mapper<'a> {
    s: &'a Session,
    f: Frames,
    /// The frame of the command's first pick (its directions turn with it).
    first: Option<Mat>,
    changed: bool,
    /// Picks go into the sketch's frame (`Here`), not the frame of the body under them.
    here: bool,
}

impl Mapper<'_> {
    fn pick(&mut self, v: &Value, body: Option<&str>) -> Option<Value> {
        let p = vec3(v)?;
        let m = if self.here { self.f.sketch } else { frame_at(self.s, &self.f, p, body).unwrap_or(self.f.active) };
        self.first.get_or_insert(m);
        self.local_point(&m, p)
    }

    fn local_point(&mut self, m: &Mat, p: Vec3) -> Option<Value> {
        if is_identity(m) {
            return None;
        }
        let inv = mat_inverse(m)?;
        self.changed = true;
        Some(pt(apply_point(&inv, p)))
    }

    fn make(&mut self, v: &Value) -> Option<Value> {
        let m = self.f.active;
        let Some(p) = vec3(v) else { return self.make_expr(&m, v) };
        self.local_point(&m, p)
    }

    /// A point with expression coordinates (`["w / 2", 0, 0]`): mapped as expressions.
    fn make_expr(&mut self, m: &Mat, v: &Value) -> Option<Value> {
        let a = v.as_array().filter(|a| a.len() == 3)?;
        let c = |i: usize| match a.get(i)? {
            Value::Number(n) => n.as_f64().filter(|x| x.is_finite()).map(|x| solidcraft_doc::Expr::new(format!("{x}"))),
            Value::String(e) => Some(solidcraft_doc::Expr::new(e.as_str())),
            _ => None,
        };
        let mut p = [c(0)?, c(1)?, c(2)?];
        if is_identity(m) {
            return None;
        }
        let inv = mat_inverse(m)?;
        map_point_expr(&inv, &mut p);
        self.changed = true;
        Some(serde_json::json!(p))
    }

    fn dir(&mut self, v: &Value) -> Option<Value> {
        let d = vec3(v)?;
        let m = self.first.unwrap_or(self.f.active);
        if is_identity(&m) {
            return None;
        }
        let inv = mat_inverse(&m)?;
        self.changed = true;
        Some(pt(apply_vector(&inv, d)))
    }

    /// A list of picks (points, or objects holding one).
    fn picks(&mut self, v: &Value, body: Option<&str>) -> Option<Value> {
        match v {
            Value::Array(a) if vec3(v).is_some() && a.iter().all(Value::is_number) => self.pick(v, body),
            Value::Array(a) => {
                let out: Vec<Value> = a.iter().map(|x| self.picks(x, body).unwrap_or_else(|| x.clone())).collect();
                Some(Value::Array(out))
            }
            Value::Object(o) => {
                let b = o.get("body").and_then(Value::as_str).map(str::to_string);
                let mut m = o.clone();
                for k in ["point", "face", "edge", "vertex", "at"] {
                    if let Some(x) = o.get(k)
                        && let Some(y) = self.pick(x, b.as_deref().or(body))
                    {
                        m.insert(k.into(), y);
                    }
                }
                if let Some(bn) = &b
                    && let Some(own) = self.own_name(bn)
                {
                    m.insert("body".into(), Value::String(own));
                }
                Some(Value::Object(m))
            }
            _ => None,
        }
    }

    fn plane(&mut self, v: &Value) -> Option<Value> {
        let o = v.as_object()?;
        let mut m = o.clone();
        if let Some(x) = o.get("face").and_then(|x| self.pick(x, None)) {
            m.insert("face".into(), x);
        }
        if let Some(x) = o.get("origin").and_then(|x| self.pick(x, None)) {
            m.insert("origin".into(), x);
        }
        for k in ["normal", "x_dir", "y_dir"] {
            if let Some(x) = o.get(k).and_then(|x| self.dir(x)) {
                m.insert(k.into(), x);
            }
        }
        Some(Value::Object(m))
    }

    /// The component's body name for a world (instance) body name.
    fn own_name(&mut self, world: &str) -> Option<String> {
        let own = self.f.bodies.iter().find(|(w, _, _)| w == world).map(|(_, own, _)| own.clone())?;
        (own != world).then(|| {
            self.changed = true;
            own
        })
    }

    fn body(&mut self, v: &Value) -> Option<Value> {
        match v {
            Value::String(n) => self.own_name(n).map(Value::String),
            Value::Array(a) => {
                let out: Vec<Value> = a.iter().map(|x| self.body(x).unwrap_or_else(|| x.clone())).collect();
                Some(Value::Array(out))
            }
            _ => None,
        }
    }
}

thread_local! {
    static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// The outermost command's coordinates were mapped here (its table lists them).
    static MAPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Did this module map the running command's coordinates? (Otherwise a new feature's geometry
/// goes into the active component's frame as a whole.)
pub fn mapped() -> bool {
    MAPPED.with(std::cell::Cell::get)
}

/// A command running inside another (its parameters are already in component frames).
pub struct Nested;

impl Nested {
    /// Enter command `id`; true when it is the outermost one.
    pub fn enter(id: &str) -> (Nested, bool) {
        let outer = DEPTH.with(|d| {
            let n = d.get();
            d.set(n + 1);
            n == 0
        });
        if outer {
            let inner = if id == "timeline.redefine" { None } else { Some(id) };
            MAPPED.with(|m| m.set(inner.is_none_or(|i| !table(i).is_empty())));
        }
        (Nested, outer)
    }
}

impl Drop for Nested {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// `params` with world picks mapped into component frames (`None`: nothing to change).
pub fn to_local(s: &Session, id: &str, params: &Value) -> Option<Value> {
    if id == "timeline.redefine" {
        let cmd = params.get("command")?.as_str()?;
        let key = params.get("feature").and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|id| id.to_string())))?;
        let feature = s.doc.find_feature(&key)?;
        // Edit Feature authors in the existing feature's frame, even when another
        // component is active. Body-relative picks still use the picked body's frame.
        let mut context = s.scratch();
        context.active_component = feature.component;
        if let solidcraft_doc::FeatureKind::Emboss { face: Some(face), .. } = &feature.kind {
            context.active_occurrence_path = (!face.author_path.is_empty()).then(|| face.author_path.clone());
        }
        let mut inner = to_local(&context, cmd, params.get("params")?)?;
        // An untouched explicit axis is displayed in world coordinates by Edit
        // Feature. Keep its exact saved values instead of an affine round trip.
        if cmd == "solid.revolve"
            && let solidcraft_doc::FeatureKind::Revolve { axis: solidcraft_doc::AxisRef::Line { origin, dir }, .. } = &feature.kind
        {
            let frame = context.active_component_frame();
            let displayed = serde_json::json!({"origin":pt(apply_point(&frame,origin.0)),"dir":pt(apply_vector(&frame,dir.0))});
            if params.get("params").and_then(|p| p.get("axis")) == Some(&displayed) {
                inner["axis"] = serde_json::json!({"origin":pt(origin.0),"dir":pt(dir.0)});
            }
        }
        let points = match (&feature.kind, cmd) {
            (solidcraft_doc::FeatureKind::Revolve { to, .. }, "solid.revolve") => [("to", *to), ("from", None)],
            (solidcraft_doc::FeatureKind::Extrude { extent, .. }, "solid.extrude") => [("to", extent.to), ("from", extent.from)],
            _ => [("to", None), ("from", None)],
        };
        for (key, point) in points {
            if let Some(point) = point {
                let displayed = pt(apply_point(&context.active_component_frame(), point.0));
                if params.get("params").and_then(|p| p.get(key)) == Some(&displayed) {
                    inner[key] = pt(point.0);
                }
            }
        }
        let mut out = params.clone();
        out["params"] = inner;
        return Some(out);
    }
    // Exact face capture retains the original world pick and occurrence identity.
    if matches!(id, "sketch.create" | "sketch.redefine") && params.get("plane").and_then(|plane| plane.get("face")).is_some() {
        return None;
    }
    let keys = table(id);
    if keys.is_empty() || !placed(s) {
        return None;
    }
    let obj = params.as_object()?;
    let mut command_frames = frames(s);
    if id == "sketch.redefine" {
        let key = params.get("sketch").and_then(|value| value.as_str().map(str::to_string).or_else(|| value.as_u64().map(|id| id.to_string())))?;
        let feature = s.doc.find_feature(&key)?;
        let path = crate::cmd::author_path_for_rebind(s, feature, params, id).ok()?;
        let frame = solidcraft_doc::EmbossFace::component_frame(&s.placed_document(), feature.component, &path).ok()?;
        command_frames.active = frame;
        command_frames.sketch = frame;
    }
    let mut m = Mapper { s, f: command_frames, first: None, changed: false, here: false };
    let body = obj.get("body").or_else(|| obj.get("face_body")).and_then(Value::as_str).map(str::to_string);
    let mut out: Map<String, Value> = obj.clone();
    // Picks first: their frame turns the directions.
    for pass in [true, false] {
        for (k, kind) in keys {
            let Some(v) = obj.get(*k) else { continue };
            let is_pick = matches!(kind, Pick | Picks | Plane | Here | HerePlane);
            if is_pick != pass {
                continue;
            }
            let new = match kind {
                Pick => m.pick(v, body.as_deref()),
                Picks => m.picks(v, body.as_deref()),
                Make => m.make(v),
                Dir => m.dir(v),
                Axis => {
                    if let Some(axis) = v.as_object() {
                        let mut axis = axis.clone();
                        if let Some(origin) = axis.get("origin").and_then(|o| m.make(o)) {
                            axis.insert("origin".into(), origin);
                        }
                        if let Some(dir) = axis.get("dir").and_then(vec3)
                            && !is_identity(&m.f.active)
                            && let Some(inv) = mat_inverse(&m.f.active)
                        {
                            axis.insert("dir".into(), pt(apply_vector(&inv, dir)));
                            m.changed = true;
                        }
                        Some(Value::Object(axis))
                    } else {
                        None
                    }
                }
                Plane => m.plane(v),
                Body => m.body(v),
                Here | HerePlane => {
                    // A new sketch or canvas goes into the active component.
                    let keep = m.f.sketch;
                    if *kind == HerePlane {
                        m.f.sketch = m.f.active;
                    }
                    m.here = true;
                    // Body names stay: the sketch's view of the model keeps them.
                    let r = if *kind == HerePlane { m.plane(v) } else { m.picks(v, None) };
                    m.here = false;
                    m.f.sketch = keep;
                    r
                }
            };
            if let Some(n) = new {
                out.insert((*k).to_string(), n);
            }
        }
    }
    m.changed.then_some(Value::Object(out))
}

#[cfg(test)]
#[path = "frames_tests.rs"]
mod tests;
