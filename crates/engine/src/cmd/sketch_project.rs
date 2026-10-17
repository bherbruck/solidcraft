//! Sketch projection: Project, Intersect and Include 3D Geometry bring model edges, faces,
//! bodies, other sketches and the origin into the active sketch as linked reference geometry
//! (fixed, drawn purple, updated when the upstream geometry changes). Break Link turns them into
//! normal geometry.

use serde_json::{Value, json};
use solidcraft_doc::project::resolve;
use solidcraft_doc::{Document, FeatureKind};
use solidcraft_geom::{Plane, Vec3};
use solidcraft_sketch::{ConstraintKind, LinkKind, LinkSource, Sketch};

use super::{CommandSpec, in_sketch};
use crate::params::{bad, bool_, str_, vec3};
use crate::{EngineError, Result, Session};

macro_rules! refs_doc {
    () => {
        "refs: [{edge: [x,y,z], body?} | {face: [x,y,z], body?} (its edge loops) | {body: name} (silhouette) | {vertex: [x,y,z], body?} | {sketch, curve} | {sketch, point} | \"origin\" | {axis: X|Y|Z} | {plane: XY|XZ|YZ|name}]"
    };
}

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("sketch.project", "Project", project_cmd).at("SKETCH", "CREATE").icon("project").key("P").enabled(in_sketch).params(concat!(refs_doc!(), created_doc!())),
    CommandSpec::new("sketch.intersect", "Intersect", intersect_cmd).at("SKETCH", "CREATE").icon("intersect").enabled(in_sketch).params(
        concat!("refs: [{body}, {face: [x,y,z], body?}, {edge: [x,y,z], body?}, {sketch, curve}]: section curves (or points) with the sketch plane, linked", created_doc!()),
    ),
    CommandSpec::new("sketch.include_3d", "Include 3D Geometry", include_cmd)
        .at("SKETCH", "CREATE")
        .icon("include")
        .enabled(in_sketch)
        .params(concat!("refs: like Project (edges, vertices, sketch curves and points); curves out of the sketch plane are flattened onto it", created_doc!())),
    CommandSpec::new("sketch.project_to_surface", "Project To Surface", project_to_surface)
        .at("SKETCH", "CREATE")
        .icon("project_surface")
        .enabled(in_sketch)
        .params(concat!("curves: [{sketch, curve}…] (curves of other sketches), face: [x,y,z] point on the target face, body?: projected along the active sketch's normal onto the face, as linked 3D curves", created_doc!())),
    CommandSpec::new("sketch.intersection_curve", "Intersection Curve", intersection_curve)
        .at("SKETCH", "CREATE")
        .icon("intersection_curve")
        .enabled(in_sketch)
        .params(concat!("a, b: {body} | {face: [x,y,z], body?}: where they meet, as linked 3D curves", created_doc!())),
    CommandSpec::new("sketch.spun_profile", "Spun Profile", spun_profile)
        .at("SKETCH", "CREATE")
        .icon("spun")
        .enabled(in_sketch)
        .params(concat!("body: name, axis: a line of the active sketch (curve id) | X|Y|Z: the body's outline revolved about the axis, in the sketch plane", created_doc!())),
    CommandSpec::new("sketch.isoparametric_curve", "Isoparametric Curve", iso_curve)
        .at("SKETCH", "CREATE")
        .icon("iso_curve")
        .enabled(in_sketch)
        .params(concat!("face: [x,y,z] point on the face (the curve passes through it), body?, direction?: u|v|[x,y,z] (default u): linked 3D curve", created_doc!())),
    CommandSpec::new("sketch.fit_curves_to_section", "Fit Curves to Mesh Section", fit_section)
        .at("SKETCH", "CREATE")
        .icon("fit_section")
        .enabled(in_sketch)
        .params(concat!("body: name (a mesh or solid body): its section with the sketch plane, fitted with lines and arcs, as normal (unlinked) geometry", created_doc!())),
    CommandSpec::new("sketch.break_link", "Break Link", break_link).icon("sketch_break")
        .enabled(in_sketch)
        .params("link?: link id (j1) | entities?: [curve or point ids]: linked geometry becomes normal sketch geometry"),
    CommandSpec::new("sketch.auto_project", "Auto-Project Edges on Reference", auto_project).icon("project")
        .noundo()
        .params("value?: bool (default toggles): project a face's edges when a sketch is created on it"),
];

/// Parse one reference.
fn source(s: &Session, v: &Value, cmd: &str) -> Result<LinkSource> {
    if let Some(name) = v.as_str() {
        return match name.to_ascii_lowercase().as_str() {
            "origin" => Ok(LinkSource::Origin),
            "x" | "y" | "z" => Ok(LinkSource::Axis { name: name.to_ascii_uppercase() }),
            _ if s.model.state().body(name).is_some() => Ok(LinkSource::Body { body: name.to_string() }),
            _ => Err(bad(cmd, format!("unknown reference `{name}`"))),
        };
    }
    let body = || str_(v, "body").unwrap_or("").to_string();
    if let Some(at) = v.get("edge").and_then(vec3) {
        return Ok(LinkSource::Edge { body: body(), at });
    }
    if let Some(at) = v.get("face").and_then(vec3) {
        return Ok(LinkSource::Face { body: body(), at });
    }
    if let Some(at) = v.get("vertex").and_then(vec3) {
        return Ok(LinkSource::Vertex { body: body(), at });
    }
    if let Some(sk) = v.get("sketch") {
        let key = match sk {
            Value::Number(n) => n.to_string(),
            Value::String(x) => x.clone(),
            _ => return Err(bad(cmd, "`sketch` must be an id or a name")),
        };
        let f = s.doc.find_feature(&key).ok_or_else(|| bad(cmd, format!("no sketch `{key}`")))?;
        if !matches!(f.kind, FeatureKind::Sketch { .. }) {
            return Err(bad(cmd, format!("`{key}` is not a sketch")));
        }
        if Some(f.id) == s.active_sketch {
            return Err(bad(cmd, "a sketch cannot project its own geometry"));
        }
        if let Some(c) = str_(v, "curve") {
            return Ok(LinkSource::SketchCurve { sketch: f.id, curve: c.to_string() });
        }
        if let Some(p) = str_(v, "point") {
            return Ok(LinkSource::SketchPoint { sketch: f.id, point: p.to_string() });
        }
        return Err(bad(cmd, "a sketch reference needs `curve` or `point`"));
    }
    if let Some(b) = str_(v, "body") {
        return Ok(LinkSource::Body { body: b.to_string() });
    }
    if let Some(a) = str_(v, "axis") {
        return Ok(LinkSource::Axis { name: a.to_ascii_uppercase() });
    }
    if let Some(p) = str_(v, "plane") {
        return Ok(LinkSource::Plane { name: p.to_string() });
    }
    if v.get("origin").is_some() {
        return Ok(LinkSource::Origin);
    }
    Err(bad(cmd, format!("cannot project {v}")))
}

/// The plane of a sketch feature, resolved against `st`: the model before the sketch as its
/// component sees it, which is what evaluation resolves it against. A construction plane built
/// from the model (through a sketch line, three points, between two faces…) exists only there.
fn sketch_plane(doc: &Document, id: u64, st: &solidcraft_doc::ModelState) -> Result<Plane> {
    let (vals, _) = doc.param_values();
    match doc.feature(id).map(|f| &f.kind) {
        Some(FeatureKind::Sketch { plane, .. }) => Ok(doc.resolve_plane_in(&vals, plane, 0, Some(st))?),
        _ => Err(EngineError::Other(format!("feature {id} is not a sketch"))),
    }
}

/// The model before sketch `id` in the frame its links and its plane are resolved in: the
/// attached face's frame for a sketch on a face, else its component's.
fn state_for_sketch<'a>(
    s: &Session,
    doc: &Document,
    id: u64,
    st: &'a solidcraft_doc::ModelState,
) -> std::result::Result<std::borrow::Cow<'a, solidcraft_doc::ModelState>, solidcraft_doc::DocError> {
    let comp = doc.feature(id).map(|f| f.component).unwrap_or(0);
    let attachment = doc.feature(id).and_then(|feature| match &feature.kind {
        solidcraft_doc::FeatureKind::Sketch { plane, .. } => super::sketch::face::anchor(plane),
        _ => None,
    });
    match attachment {
        Some(anchor) => solidcraft_doc::state_in_face_anchor_frame(doc, st, anchor, comp),
        None => Ok(solidcraft_doc::state_in_frame_at(doc, st, comp, owner_frame(s, doc, comp))),
    }
}

/// Keep candidate feature edits while overlaying uncommitted occurrence placements.
fn placed_candidate<'a>(s: &Session, doc: &'a Document) -> std::borrow::Cow<'a, Document> {
    if s.pending_moves.is_empty() {
        return std::borrow::Cow::Borrowed(doc);
    }
    let mut placed = doc.clone();
    for o in &mut placed.occurrences {
        if let Some(m) = s.pending_moves.get(&o.id) {
            o.transform = *m;
        }
    }
    std::borrow::Cow::Owned(placed)
}

fn owner_frame(s: &Session, doc: &Document, comp: u64) -> solidcraft_doc::Mat {
    if comp == s.active_component { s.active_component_frame() } else { doc.component_transform(comp) }
}

/// Re-resolve the links of a sketch being edited (`sk`, a copy of the stored sketch `id`)
/// against the model before it, so edits see current reference geometry. Returns warnings.
///
/// When a link moved, the rest of the sketch follows it as the evaluation solves it, keeping
/// the stored sides and turns, so the edit goes on from the sketch the model shows. Left where
/// it was stored beside a line that has moved past it, a dimensioned point would give the
/// edit's solve the other side to keep. A sketch that does not solve that way keeps its stored
/// geometry, with the links moved.
pub(super) fn refresh(s: &Session, doc: &Document, id: u64, sk: &mut Sketch) -> Vec<String> {
    if sk.links.is_empty() {
        return Vec::new();
    }
    let doc = placed_candidate(s, doc);
    let (vals, _) = doc.param_values();
    // A text's outlines are rebuilt from its numbers: the ones its expressions give come first
    // (an expression that cannot be evaluated leaves the text as it was).
    let _ = doc.apply_text_values(&vals, sk);
    let st = s.model.state_before(id);
    let attachment = doc.feature(id).and_then(|feature| match &feature.kind {
        solidcraft_doc::FeatureKind::Sketch { plane, .. } => super::sketch::face::anchor(plane),
        _ => None,
    });
    let Ok(st) = state_for_sketch(s, &doc, id, &st) else {
        return vec!["lost face attachment; reattach explicitly".into()];
    };
    let Ok(plane) = sketch_plane(&doc, id, &st) else { return Vec::new() };
    let warnings = match attachment {
        Some(anchor) => solidcraft_doc::project::refresh_face_anchor_links(&doc, &vals, &st, &plane, sk, anchor),
        None => solidcraft_doc::project::refresh_links(&doc, &vals, &st, &plane, sk),
    };
    if let Ok(stored) = doc.sketch(id)
        && sk.positions_in(stored).is_some()
    {
        let mut solved = sk.clone();
        let follows = solved.refresh_offsets().is_ok()
            && doc.solve_sketch(&vals, &mut solved, &stored.driving_values(), Some(stored)).is_ok_and(|report| report.keeps_orientation());
        if follows {
            *sk = solved;
        }
    }
    warnings
}

/// Add a link for `src` to sketch `id` in `doc` (not stored in the session). Returns the link id.
pub(super) fn add_link(s: &Session, doc: &Document, id: u64, sk: &mut Sketch, kind: LinkKind, src: LinkSource) -> Result<String> {
    let doc = placed_candidate(s, doc);
    let (vals, _) = doc.param_values();
    let st = s.model.state_before(id);
    // The model as the sketch's component sees it.
    let st = state_for_sketch(s, &doc, id, &st)?;
    let plane = sketch_plane(&doc, id, &st)?;
    let r = resolve(&doc, &vals, &st, &plane, kind, &src)?;
    let name = if let LinkSource::Face { body, at } = &r.source {
        st.body(body).and_then(|body| {
            if let Some(index) = r.face_index {
                solidcraft_doc::naming::face_names(body).get(index).filter(|name| !name.is_empty()).cloned()
            } else {
                solidcraft_doc::naming::face_name_at_body(body, *at)
            }
        })
    } else {
        None
    };
    // The link is the edge that was picked, not whichever is nearest the picked point later.
    let edge_name = solidcraft_doc::project::resolved_edge_name(&st, &r);
    let link = sk.add_link_with_wires(kind, r.source, &r.geom, &r.wires)?;
    if let Some(l) = sk.links.iter_mut().find(|l| l.id == link) {
        l.face_name = name;
        l.edge_name = edge_name;
    }
    Ok(link)
}

fn link_json(sk: &Sketch, link: &str) -> Value {
    let curves: Vec<String> = sk.link_curves(link).iter().filter_map(|c| sk.curves.get(*c).map(|c| c.id.clone())).collect();
    let points: Vec<String> = sk.link_points(link).iter().filter_map(|p| sk.points.get(*p).map(|p| p.id.clone())).collect();
    json!({"link": link, "curves": curves, "points": points})
}

fn run_links(s: &mut Session, p: &Value, cmd: &str, kind: LinkKind) -> Result<Value> {
    let refs: Vec<Value> = match p.get("refs").or_else(|| p.get("ref")) {
        Some(Value::Array(a)) => a.iter().take(1000).cloned().collect(),
        Some(v) => vec![v.clone()],
        None => return Err(bad(cmd, "`refs` must list what to project")),
    };
    if refs.is_empty() {
        return Err(bad(cmd, "`refs` is empty"));
    }
    let id = s.active_sketch.ok_or_else(|| EngineError::Other("no sketch is being edited".into()))?;
    let sources: Vec<LinkSource> = refs.iter().map(|v| source(s, v, cmd)).collect::<Result<_>>()?;
    let mut doc = (*s.doc).clone();
    let mut sk = doc.sketch(id)?.clone();
    refresh(s, &doc, id, &mut sk);
    let mut out = Vec::new();
    for src in sources {
        let what = src.describe();
        let l = add_link(s, &doc, id, &mut sk, kind, src).map_err(|e| bad(cmd, format!("{what}: {e}")))?;
        out.push(link_json(&sk, &l));
    }
    *doc.sketch_mut(id)? = sk;
    *s.doc_mut() = doc;
    Ok(json!({"links": out}))
}

fn project_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    run_links(s, p, "sketch.project", LinkKind::Project)
}

fn intersect_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    run_links(s, p, "sketch.intersect", LinkKind::Intersect)
}

fn include_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    run_links(s, p, "sketch.include_3d", LinkKind::Include)
}

/// Add one link from `src` to the active sketch and report it.
fn one_link(s: &mut Session, cmd: &str, kind: LinkKind, src: LinkSource) -> Result<Value> {
    let id = s.active_sketch.ok_or_else(|| EngineError::Other("no sketch is being edited".into()))?;
    let mut doc = (*s.doc).clone();
    let mut sk = doc.sketch(id)?.clone();
    refresh(s, &doc, id, &mut sk);
    let what = src.describe();
    let l = add_link(s, &doc, id, &mut sk, kind, src).map_err(|e| bad(cmd, format!("{what}: {e}")))?;
    let mut out = link_json(&sk, &l);
    let wires: Vec<String> = sk.link_wires(&l).iter().filter_map(|w| sk.wires.get(*w).map(|w| w.id.clone())).collect();
    out["wires"] = json!(wires);
    *doc.sketch_mut(id)? = sk;
    *s.doc_mut() = doc;
    Ok(out)
}

fn project_to_surface(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.project_to_surface";
    let at = p.get("face").and_then(vec3).ok_or_else(|| bad(cmd, "`face` must be a point [x,y,z] on the target face"))?;
    let body = str_(p, "body").unwrap_or("").to_string();
    let list = p.get("curves").and_then(Value::as_array).cloned().ok_or_else(|| bad(cmd, "`curves` must list {sketch, curve}"))?;
    if list.is_empty() || list.len() > 500 {
        return Err(bad(cmd, "`curves` must list 1…500 curves"));
    }
    let mut out = Vec::new();
    for v in list {
        let LinkSource::SketchCurve { sketch, curve } = source(s, &v, cmd)? else {
            return Err(bad(cmd, "each entry must be {sketch, curve}"));
        };
        out.push(one_link(s, cmd, LinkKind::Project, LinkSource::OnSurface { sketch, curve, body: body.clone(), at })?);
    }
    Ok(json!({"links": out}))
}

fn intersection_curve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.intersection_curve";
    let side = |k: &str| -> Result<LinkSource> {
        let v = p.get(k).ok_or_else(|| bad(cmd, format!("missing `{k}`")))?;
        match source(s, v, cmd)? {
            x @ (LinkSource::Body { .. } | LinkSource::Face { .. }) => Ok(x),
            _ => Err(bad(cmd, format!("`{k}` must be a body or a face"))),
        }
    };
    let (a, b) = (side("a")?, side("b")?);
    one_link(s, cmd, LinkKind::Include, LinkSource::Intersection { a: Box::new(a), b: Box::new(b) })
}

fn spun_profile(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.spun_profile";
    let body = str_(p, "body").ok_or_else(|| bad(cmd, "`body` must name a body"))?.to_string();
    let axis = str_(p, "axis").ok_or_else(|| bad(cmd, "`axis` must be a sketch line or X|Y|Z"))?;
    let (origin, dir) = match axis.to_ascii_uppercase().as_str() {
        "X" => (Vec3::ZERO, Vec3::X),
        "Y" => (Vec3::ZERO, Vec3::Y),
        "Z" => (Vec3::ZERO, Vec3::Z),
        _ => {
            let id = s.active_sketch.ok_or_else(|| EngineError::Other("no sketch is being edited".into()))?;
            let st = s.model.state();
            let ss = st.sketch(id).ok_or_else(|| EngineError::Other("the sketch is not evaluated".into()))?;
            let ci = ss.sketch.curve_index(axis).ok_or_else(|| bad(cmd, format!("unknown curve `{axis}`")))?;
            let Some(solidcraft_sketch::Shape::Line { a, b }) = ss.sketch.shape(ci) else { return Err(bad(cmd, "the axis must be a line")) };
            let (wa, wb) = (ss.plane.to_world(a), ss.plane.to_world(b));
            (wa, wb - wa)
        }
    };
    one_link(s, cmd, LinkKind::Project, LinkSource::Spun { body, origin, dir })
}

fn iso_curve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.isoparametric_curve";
    let at = p.get("face").and_then(vec3).ok_or_else(|| bad(cmd, "`face` must be a point [x,y,z] on the face"))?;
    let body = str_(p, "body").unwrap_or("").to_string();
    let dir = match p.get("direction") {
        None => "u".to_string(),
        Some(Value::String(d)) if d.eq_ignore_ascii_case("u") || d.eq_ignore_ascii_case("v") => d.to_ascii_lowercase(),
        Some(v) => {
            let d = vec3(v).ok_or_else(|| bad(cmd, "`direction` must be u, v or [x,y,z]"))?;
            format!("{},{},{}", d.x, d.y, d.z)
        }
    };
    one_link(s, cmd, LinkKind::Include, LinkSource::Iso { body, at, dir })
}

fn fit_section(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.fit_curves_to_section";
    let body = str_(p, "body").ok_or_else(|| bad(cmd, "`body` must name a body"))?.to_string();
    if s.model.state().body(&body).is_none() {
        return Err(bad(cmd, format!("no body `{body}`")));
    }
    let id = s.active_sketch.ok_or_else(|| EngineError::Other("no sketch is being edited".into()))?;
    let mut doc = (*s.doc).clone();
    let mut sk = doc.sketch(id)?.clone();
    let l = add_link(s, &doc, id, &mut sk, LinkKind::Intersect, LinkSource::Body { body }).map_err(|e| bad(cmd, e.to_string()))?;
    let curves: Vec<String> = sk.link_curves(&l).iter().filter_map(|c| sk.curves.get(*c).map(|c| c.id.clone())).collect();
    sk.break_link(&l)?;
    *doc.sketch_mut(id)? = sk;
    *s.doc_mut() = doc;
    Ok(json!({"curves": curves}))
}

fn break_link(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.break_link";
    let id = s.active_sketch.ok_or_else(|| EngineError::Other("no sketch is being edited".into()))?;
    let mut doc = (*s.doc).clone();
    let sk = doc.sketch_mut(id)?;
    let mut links: Vec<String> = Vec::new();
    if let Some(l) = str_(p, "link") {
        if sk.link(l).is_none() {
            return Err(bad(cmd, format!("no link `{l}`")));
        }
        links.push(l.to_string());
    }
    for e in crate::params::string_list(p, "entities") {
        let l = sk
            .curves
            .iter()
            .find(|c| c.id == e)
            .and_then(|c| c.link.clone())
            .or_else(|| sk.points.iter().find(|q| q.id == e).and_then(|q| q.link.clone()))
            .ok_or_else(|| bad(cmd, format!("`{e}` is not linked geometry")))?;
        if !links.contains(&l) {
            links.push(l);
        }
    }
    if links.is_empty() {
        return Err(bad(cmd, "give `link` or `entities`"));
    }
    for l in &links {
        sk.break_link(l)?;
    }
    *s.doc_mut() = doc;
    Ok(json!({"broken": links}))
}

fn auto_project(s: &mut Session, p: &Value) -> Result<Value> {
    s.auto_project = bool_(p, "value").unwrap_or(!s.auto_project);
    Ok(json!({"auto_project": s.auto_project}))
}

/// Point arguments that snap to model geometry while drawing, rewritten to sketch point ids:
/// `{"vertex": [x,y,z]}` projects the vertex (a linked point); `{"on_edge": [x,y,z]}` projects
/// the edge and makes a free point on it (coincident with the projection); `"mid:<line>"`
/// makes a point held at the line's midpoint (and `"vertex:x,y,z"` is the vertex form). Everything
/// else is left as it is. The links are added to the active sketch in the session's document.
pub(super) fn snap_points(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    let Some(id) = s.active_sketch else { return Ok(p.clone()) };
    let mut out = p.clone();
    let mut doc: Option<(Document, Sketch)> = None;
    let fix = |v: &mut Value, s: &Session, doc: &mut Option<(Document, Sketch)>| -> Result<()> {
        let (vertex, edge) = (v.get("vertex").and_then(vec3), v.get("on_edge").and_then(vec3));
        // String forms from interactive snapping: "vertex:x,y,z" and "mid:<line>".
        let vertex = vertex.or_else(|| {
            let t = v.as_str()?.strip_prefix("vertex:")?;
            let n: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            match n[..] {
                [x, y, z] if x.is_finite() && y.is_finite() && z.is_finite() => Some(Vec3::new(x, y, z)),
                _ => None,
            }
        });
        let mid = v.as_str().and_then(|x| x.strip_prefix("mid:")).map(str::to_string);
        // "on:<curve>:x,y": a point on a curve of the sketch near x,y.
        let on = v.as_str().and_then(|x| x.strip_prefix("on:")).and_then(|t| {
            let (c, xy) = t.rsplit_once(':')?;
            let n: Vec<f64> = xy.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            match n[..] {
                [x, y] if x.is_finite() && y.is_finite() => Some((c.to_string(), solidcraft_geom::Vec2::new(x, y))),
                _ => None,
            }
        });
        if vertex.is_none() && edge.is_none() && mid.is_none() && on.is_none() {
            return Ok(());
        }
        if doc.is_none() {
            let d = (*s.doc).clone();
            let mut sk = d.sketch(id)?.clone();
            refresh(s, &d, id, &mut sk);
            *doc = Some((d, sk));
        }
        let Some((d, sk)) = doc.as_mut() else { return Ok(()) };
        let body = str_(v, "body").unwrap_or("").to_string();
        let pid = if let Some((curve, near)) = on {
            let c = sk.curve_index(&curve).ok_or_else(|| bad(cmd, format!("unknown curve `{curve}`")))?;
            let at = match sk.shape(c) {
                Some(sh) => sh.project(near),
                None => near,
            };
            let pi = sk.add_point(at, None)?;
            sk.add_constraint(ConstraintKind::PointOnCurve { p: pi, c }, None)?;
            sk.points.get(pi).map(|q| q.id.clone()).unwrap_or_default()
        } else if let Some(line) = mid {
            // "mid:<line>": a new point at the line's midpoint, kept there.
            let l = sk.curve_index(&line).ok_or_else(|| bad(cmd, format!("unknown curve `{line}`")))?;
            let Some(solidcraft_sketch::Shape::Line { a, b }) = sk.shape(l) else { return Err(bad(cmd, format!("`{line}` is not a line"))) };
            let pi = sk.add_point((a + b) * 0.5, None)?;
            sk.add_constraint(ConstraintKind::Midpoint { p: pi, l }, None)?;
            sk.points.get(pi).map(|q| q.id.clone()).unwrap_or_default()
        } else if let Some(at) = vertex {
            let l = add_link(s, d, id, sk, LinkKind::Project, LinkSource::Vertex { body, at }).map_err(|e| bad(cmd, format!("vertex: {e}")))?;
            let pi = sk.link_points(&l).first().copied().ok_or_else(|| bad(cmd, "vertex projects to nothing"))?;
            sk.points.get(pi).map(|q| q.id.clone()).unwrap_or_default()
        } else if let Some(at) = edge {
            let l = add_link(s, d, id, sk, LinkKind::Project, LinkSource::Edge { body, at }).map_err(|e| bad(cmd, format!("edge: {e}")))?;
            let before = s.model.state_before(id);
            let plane = sketch_plane(d, id, &*state_for_sketch(s, d, id, &before)?)?;
            let q = plane.to_local(at);
            let ci = sk.link_curves(&l).first().copied().ok_or_else(|| bad(cmd, "the edge projects to a point"))?;
            let pi = sk.add_point(q, None)?;
            sk.add_constraint(ConstraintKind::PointOnCurve { p: pi, c: ci }, None)?;
            sk.points.get(pi).map(|q| q.id.clone()).unwrap_or_default()
        } else {
            return Ok(());
        };
        *v = Value::String(pid);
        Ok(())
    };
    if let Some(o) = out.as_object_mut() {
        for (_, v) in o.iter_mut() {
            match v {
                Value::Array(a) if a.len() <= 10_000 => {
                    for x in a.iter_mut() {
                        fix(x, s, &mut doc)?;
                    }
                }
                Value::Object(_) | Value::String(_) => fix(v, s, &mut doc)?,
                _ => {}
            }
        }
    }
    if let Some((mut d, sk)) = doc {
        *d.sketch_mut(id)? = sk;
        *s.doc_mut() = d;
    }
    Ok(out)
}

/// Private compatibility projection for generated face profiles. Named legacy links follow
/// edits, but cannot provide the construction certificate required by public attachments.
pub(super) fn project_internal_face(s: &mut Session, id: u64, body: &str, index: usize) -> Result<String> {
    let mut doc = (*s.doc).clone();
    let raw = s.model.state_before(id);
    let component = doc.feature(id).map(|feature| feature.component).unwrap_or(0);
    let state = solidcraft_doc::state_in_frame_at(&doc, &raw, component, owner_frame(s, &doc, component));
    let plane = sketch_plane(&doc, id, &state)?;
    let resolved = solidcraft_doc::project::resolve_face_indices(&state, &plane, LinkKind::Project, body, &[index])?;
    let face_name = state.body(body).and_then(|body| solidcraft_doc::naming::face_names(body).get(index).cloned());
    let mut sk = doc.sketch(id)?.clone();
    let link = sk.add_link_with_wires(LinkKind::Project, resolved.source, &resolved.geom, &resolved.wires)?;
    if let Some(entry) = sk.links.iter_mut().find(|entry| entry.id == link) {
        entry.face_name = face_name;
    }
    *doc.sketch_mut(id)? = sk;
    *s.doc_mut() = doc;
    Ok(link)
}

/// Project the edges of the face a new sketch was created on (Fusion's default).
pub(super) fn auto_project_face(s: &mut Session, id: u64, _at: Vec3) -> Result<Option<String>> {
    let mut doc = (*s.doc).clone();
    let Some(anchor) = doc
        .feature(id)
        .and_then(|feature| match &feature.kind {
            solidcraft_doc::FeatureKind::Sketch { plane, .. } => super::sketch::face::anchor(plane),
            _ => None,
        })
        .cloned()
    else {
        return Ok(None);
    };
    let raw = s.model.state_before(id);
    let author = doc.feature(id).map(|feature| feature.component).unwrap_or(0);
    let state = solidcraft_doc::state_in_face_anchor_frame(&doc, &raw, &anchor, author)?;
    let plane = doc.resolve_face_anchor(&anchor, author, &raw)?;
    let resolved = solidcraft_doc::project::resolve_anchored_face(&doc, &state, &plane, LinkKind::Project, &anchor)?;
    let mut sk = doc.sketch(id)?.clone();
    let link = sk.add_link_with_wires(LinkKind::Project, resolved.source, &resolved.geom, &resolved.wires)?;
    if let Some(link) = sk.links.iter_mut().find(|entry| entry.id == link) {
        link.face_name = Some(anchor.target.name.clone());
        link.auto_face = true;
    }
    *doc.sketch_mut(id)? = sk;
    *s.doc_mut() = doc;
    Ok(Some(link))
}

#[cfg(test)]
#[path = "sketch_project_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sketch_project_side_tests.rs"]
mod side_tests;

#[cfg(test)]
#[path = "sketch_project_edge_tests.rs"]
mod edge_tests;

#[cfg(test)]
#[path = "sketch_model_plane_tests.rs"]
mod model_plane_tests;
