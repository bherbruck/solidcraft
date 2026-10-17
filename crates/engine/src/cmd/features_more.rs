//! Rib, Web, Emboss, Replace Face, Align and Remove.

use serde_json::Value;
use solidcraft_doc::FeatureKind;
use solidcraft_doc::expr::Kind;
use solidcraft_doc::{BodyName, SketchId};
use solidcraft_doc::{Dir, FacePick, Position};
use solidcraft_geom::Vec3;

use super::CommandSpec;
use super::features::{
    add_feature, axis_dir_param, check_expr, feature_sketch, length_operands, plane_param, point_param, profiles, typed_length_unit,
};
use crate::params::{bad, bool_, expr, req_expr, str_, string_list, vec3};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("solid.boundary_fill", "Boundary Fill", boundary_fill).at("SOLID", "CREATE").icon("boundary_fill").params(
        "tools: [bodies]; cells: [[x,y,z] a point inside each cell to fill (a cell: inside some tools, outside the rest)]; operation?: new|join|cut|intersect; remove_tools?: bool",
    ),
    CommandSpec::new("solid.coil", "Coil", coil).at("SOLID", "CREATE").icon("coil").params(
        "diameter; two of revolutions (or turns), height, pitch; section_size; section?: circular|square; section_position?: inside|center|outside; \
         base?: [x,y,z] (numbers in mm or length expressions; default: the origin); axis?: X|Y|Z|[x,y,z] (default Z); start_angle?; clockwise?: bool; operation?, targets?, name?, body_name?",
    ),
    CommandSpec::new("solid.rib", "Rib", rib)
        .at("SOLID", "CREATE")
        .icon("rib")
        .params("sketch, curves: [open curve ids, in order]; thickness (across the sketch plane, or across the curves when normal); depth? (default: to the next face); flip?: bool (fill to the other side); direction?: in_plane|normal (grow across the sketch plane, the default, or along its normal)"),
    CommandSpec::new("solid.web", "Web", web)
        .at("SOLID", "CREATE")
        .icon("web")
        .params("sketch, curves: [open curve ids] (one wall each); thickness; depth? (default: to the next face); flip?: bool; direction?: in_plane|normal"),
    CommandSpec::new("solid.emboss", "Emboss", emboss)
        .at("SOLID", "CREATE")
        .icon("emboss")
        .params("sketch, profiles?; depth; mode?: emboss|deboss; targets?; face?: world point [x,y,z] selecting a planar or complete exterior cylindrical wall (inferred when exactly one supported tangent cylinder matches the selected profiles)"),
    CommandSpec::new("solid.replace_face", "Replace Face", replace_face)
        .at("SOLID", "MODIFY")
        .icon("replace_face")
        .params("faces: [[x,y,z] points on planar faces, or face names from model.faces]; target: plane (XY|XZ|YZ|construction plane|{origin, normal}) | target_face: [x,y,z] on a parallel planar face; body?"),
    CommandSpec::new("solid.align", "Align", align)
        .at("SOLID", "MODIFY")
        .icon("align")
        .params("bodies: [names]; from: [x,y,z] | {snap: vertex|edge_mid|face_center|circle_center, at: [x,y,z]} | from_face: [x,y,z]; to: (the same) | to_face: [x,y,z] (faces also turn to meet); flip?: bool"),
    CommandSpec::new("solid.remove", "Remove", remove).at("SOLID", "MODIFY").icon("delete").params("bodies: [names] (removed from here on in the timeline)"),
];

fn coil(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.coil";
    let base = point_param(s, p, "base", cmd)?.unwrap_or_else(|| solidcraft_doc::point_expr(Vec3::ZERO));
    let axis = axis_dir_param(p, cmd)?;
    let diameter = req_expr(cmd, p, "diameter")?;
    check_expr(s, &diameter, Kind::Length, cmd, "diameter")?;
    let turns = expr(p, "revolutions").or_else(|| expr(p, "turns"));
    let (height, pitch) = (expr(p, "height"), expr(p, "pitch"));
    // The pitch or the revolutions from the height keep the units the lengths were given in.
    let unit = typed_length_unit(s);
    let (turns, pitch) = match (turns, height, pitch) {
        (Some(n), _, Some(pt)) => (n, pt),
        (Some(n), Some(h), None) => {
            let [h] = length_operands(s, [(&h, &unit)]);
            (n.clone(), format!("({h}) / ({n})").into())
        }
        (None, Some(h), Some(pt)) => {
            let [h, by] = length_operands(s, [(&h, &unit), (&pt, &unit)]);
            (format!("({h}) / ({by})").into(), pt)
        }
        _ => return Err(bad(cmd, "give two of `revolutions`, `height` and `pitch`")),
    };
    check_expr(s, &turns, Kind::Unitless, cmd, "revolutions")?;
    check_expr(s, &pitch, Kind::Length, cmd, "pitch")?;
    let section_size = req_expr(cmd, p, "section_size")?;
    check_expr(s, &section_size, Kind::Length, cmd, "section_size")?;
    let section = match str_(p, "section").map(str::to_ascii_lowercase).as_deref() {
        None | Some("circular" | "circle") => solidcraft_doc::CoilSection::Circular,
        Some("square") => solidcraft_doc::CoilSection::Square,
        Some(o) => return Err(bad(cmd, format!("unknown section `{o}` (circular or square)"))),
    };
    let position = match str_(p, "section_position").map(str::to_ascii_lowercase).as_deref() {
        None | Some("center" | "on" | "on_center") => 0,
        Some("inside") => -1,
        Some("outside") => 1,
        Some(o) => return Err(bad(cmd, format!("unknown section position `{o}` (inside, center or outside)"))),
    };
    let start_angle = expr(p, "start_angle");
    if let Some(a) = &start_angle {
        check_expr(s, a, Kind::Angle, cmd, "start_angle")?;
    }
    let clockwise = bool_(p, "clockwise").unwrap_or(false);
    let operation = super::features::operation(p, cmd)?;
    add_feature(
        s,
        p,
        FeatureKind::Coil {
            base,
            axis: Dir(axis),
            diameter,
            pitch,
            turns,
            section_size,
            section,
            position,
            start_angle,
            clockwise,
            operation,
            targets: string_list(p, "targets").into_iter().map(BodyName).collect(),
        },
    )
}

fn curves_param(p: &Value, cmd: &str) -> Result<Vec<String>> {
    let c = string_list(p, "curves");
    if c.is_empty() || c.len() > 1000 {
        return Err(bad(cmd, "`curves` must list 1…1000 sketch curve ids"));
    }
    Ok(c)
}

fn rib_like(s: &mut Session, p: &Value, cmd: &str, web: bool) -> Result<Value> {
    let sketch = feature_sketch(s, p, cmd)?;
    let curves = curves_param(p, cmd)?;
    {
        let st = s.model.state();
        let ss = st.sketch(sketch).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?;
        if let Some(c) = curves.iter().find(|c| ss.sketch.curve_index(c).is_none()) {
            return Err(bad(cmd, format!("no curve `{c}` in the sketch")));
        }
    }
    let thickness = req_expr(cmd, p, "thickness")?;
    check_expr(s, &thickness, Kind::Length, cmd, "thickness")?;
    let depth = expr(p, "depth");
    if let Some(d) = &depth {
        check_expr(s, d, Kind::Length, cmd, "depth")?;
    }
    let flip = bool_(p, "flip").unwrap_or(false);
    let normal = match str_(p, "direction").map(str::to_ascii_lowercase).as_deref() {
        None | Some("in_plane" | "plane") => false,
        Some("normal") => true,
        Some(o) => return Err(bad(cmd, format!("unknown direction `{o}` (in_plane or normal)"))),
    };
    add_feature(s, p, FeatureKind::Rib { sketch: SketchId(sketch), curves, thickness, depth, flip, web, normal })
}

fn rib(s: &mut Session, p: &Value) -> Result<Value> {
    rib_like(s, p, "solid.rib", false)
}

fn web(s: &mut Session, p: &Value) -> Result<Value> {
    rib_like(s, p, "solid.web", true)
}

fn emboss(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.emboss";
    let sketch = feature_sketch(s, p, cmd)?;
    let profiles = profiles(p, cmd)?;
    let depth = req_expr(cmd, p, "depth")?;
    check_expr(s, &depth, Kind::Length, cmd, "depth")?;
    let deboss = match str_(p, "mode").map(str::to_ascii_lowercase).as_deref() {
        None | Some("emboss") => false,
        Some("deboss") => true,
        Some(m) => return Err(bad(cmd, format!("unknown mode `{m}` (emboss or deboss)"))),
    };
    let mut targets = string_list(p, "targets");
    let face = if let Some(value) = p.get("face") {
        let local = vec3(value).ok_or_else(|| bad(cmd, "`face` must be a finite point [x,y,z]"))?;
        let at = solidcraft_doc::apply_point(&s.active_component_frame(), local);
        let state = s.world_state();
        let mut candidates = Vec::new();
        // The nearest face off the point: (distance, body), for the error when none holds it.
        let mut nearest: Option<(f64, &str)> = None;
        for body in &state.bodies {
            let definition = body.definition_name();
            if !targets.is_empty() && !targets.iter().any(|n| n == definition || n == &body.name) {
                continue;
            }
            let mesh = body.mesh();
            let closest = mesh
                .triangles
                .iter()
                .zip(&mesh.tri_face)
                .filter_map(|(tri, face)| {
                    let [a, b, c] = mesh.tri(tri)?;
                    let n = (b - a).cross(c - a).normalized()?;
                    let q = at - n * (at - a).dot(n);
                    let inside = [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).cross(q - *u).dot(n) >= -1e-9);
                    inside.then_some(((at - a).dot(n).abs(), *face as usize))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match closest {
                Some((distance, index)) if distance < (body.body.size() * 1e-3).max(1e-3) => candidates.push((body, index)),
                Some((distance, _)) if nearest.is_none_or(|(d, _)| distance < d) => nearest = Some((distance, body.name.as_str())),
                _ => {}
            }
        }
        // #392: say what is wrong with the point, not only that it picks no single face.
        let point = format!("[{}, {}, {}]", local.x, local.y, local.z);
        match (candidates.as_slice(), nearest) {
            ([], Some((d, body))) => {
                return Err(bad(
                    cmd,
                    format!("`face` {point} lies on no face: the nearest, on `{body}`, is {d:.3} mm away; give a point on the face"),
                ));
            }
            ([], None) => return Err(bad(cmd, format!("`face` {point} lies on no face of the target bodies"))),
            ([_], _) => {}
            (more, _) => {
                let names: Vec<&str> = more.iter().map(|(b, _)| b.name.as_str()).collect();
                return Err(bad(
                    cmd,
                    format!("`face` {point} lies on faces of {} bodies ({}): name one in `targets`", names.len(), names.join(", ")),
                ));
            }
        }
        let (selected, index) = candidates.first().copied().ok_or_else(|| bad(cmd, "no target face"))?;
        let name = selected.definition_name();
        let body = &selected.body;
        match solidcraft_kernel::face_surface(body, index) {
            Some(solidcraft_kernel::FaceSurface::Plane { .. }) => {
                targets = vec![name.to_owned()];
                None
            }
            Some(solidcraft_kernel::FaceSurface::Cylinder { .. }) => {
                if targets.len() > 1 {
                    return Err(bad(cmd, "cylinder emboss needs one selected target body"));
                }
                // Viewport picks lie on tessellated chords. Keep their axial and
                // angular position, and certify their distance before storing an
                // exact surface anchor for later replay.
                let at = solidcraft_kernel::cylinder_emboss_anchor(body, index, at).map_err(|e| bad(cmd, e.to_string()))?;
                let face = emboss_selection(s, selected, at)?;
                check_emboss_positions(s, sketch, selected, &face)?;
                targets = vec![name.to_owned()];
                Some(face)
            }
            _ => return Err(bad(cmd, "only planar faces and complete exterior cylindrical walls support emboss")),
        }
    } else {
        let inferred = infer_emboss_selection(s, sketch, &profiles, &depth, deboss, &targets)?;
        if let Some(face) = &inferred {
            targets = vec![face.body.0.clone()];
        }
        inferred
    };
    add_feature(
        s,
        p,
        FeatureKind::Emboss { sketch: SketchId(sketch), profiles, depth, deboss, targets: targets.into_iter().map(BodyName).collect(), face },
    )
}

fn emboss_selection(s: &Session, selected: &solidcraft_doc::ModelBody, at: Vec3) -> Result<solidcraft_doc::EmbossFace> {
    let cmd = "solid.emboss";
    let name = selected.definition_name();
    let original = s.model.state();
    let owner = s.doc.model_body_component(selected);
    let paths: Vec<_> = s
        .doc
        .placements()
        .into_iter()
        .filter(|(comp, path, _)| *comp == owner && s.doc.world_body_name(&original, name, owner, path) == selected.name)
        .map(|(_, path, _)| path)
        .collect();
    if paths.len() != 1 {
        return Err(bad(cmd, "selected cylinder placement is ambiguous"));
    }
    let path = paths.into_iter().next().ok_or_else(|| bad(cmd, "selected cylinder placement"))?;
    let author_path = s.active_occurrence_path.clone().unwrap_or_else(|| s.doc.first_path(s.active_component));
    if path.len() > 64 || author_path.len() > 64 {
        return Err(bad(cmd, "cylinder placement is too deep"));
    }
    let inverse = solidcraft_doc::mat_inverse(&s.active_component_frame()).ok_or_else(|| bad(cmd, "invalid active frame"))?;
    Ok(solidcraft_doc::EmbossFace { body: name.into(), at: FacePick(solidcraft_doc::apply_point(&inverse, at)), path, author_path })
}

fn check_emboss_positions(s: &Session, sketch: u64, body: &solidcraft_doc::ModelBody, face: &solidcraft_doc::EmbossFace) -> Result<()> {
    let cmd = "solid.emboss";
    let owner = s.doc.model_body_component(body);
    let sketch_owner = s.doc.feature(sketch).map(|f| f.component).ok_or_else(|| bad(cmd, "profile sketch no longer exists"))?;
    let sketch_path = if sketch_owner == owner {
        face.path.clone()
    } else if sketch_owner == s.active_component {
        face.author_path.clone()
    } else {
        s.doc.first_path(sketch_owner)
    };
    if face.path.iter().chain(&face.author_path).chain(&sketch_path).any(|id| s.pending_moves.contains_key(id)) {
        return Err(bad(cmd, "capture pending source, author and profile positions with Capture Position before cylinder emboss"));
    }
    Ok(())
}

fn infer_emboss_selection(
    s: &Session,
    sketch: u64,
    profiles: &solidcraft_doc::ProfileSel,
    depth: &str,
    deboss: bool,
    targets: &[String],
) -> Result<Option<solidcraft_doc::EmbossFace>> {
    let cmd = "solid.emboss";
    let original = s.model.state();
    let ss = original.sketch(sketch).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?;
    let regions = solidcraft_doc::select_profiles(ss, profiles).map_err(|e| bad(cmd, e.to_string()))?;
    let sketch_owner = s.doc.feature(sketch).map(|f| f.component).ok_or_else(|| bad(cmd, "profile sketch no longer exists"))?;
    let d = s.doc.eval(&s.doc.with_design_unit(depth, Kind::Length), Kind::Length).map_err(|e| bad(cmd, e.to_string()))?;
    let world = s.world_state();
    let placed = s.placed_document();
    let mut chosen = None;
    let mut faces = 0usize;
    for selected in &world.bodies {
        let name = selected.definition_name();
        if !targets.is_empty() && !targets.iter().any(|n| n == name || n == &selected.name) {
            continue;
        }
        let raw = original.body(name).ok_or_else(|| bad(cmd, "target definition no longer exists"))?;
        faces = faces.saturating_add(raw.body.face_count());
        if faces > 50_000 {
            return Err(bad(cmd, "cylinder target search exceeds face work limit"));
        }
        if !solidcraft_kernel::has_cylinder_emboss_wall(&raw.body).map_err(|e| bad(cmd, e.to_string()))? {
            continue;
        }
        let mut face = emboss_selection(s, selected, Vec3::ZERO)?;
        let body_frame = face.body_frame(&placed, raw).map_err(|e| bad(cmd, e.to_string()))?;
        let inverse = solidcraft_doc::mat_inverse(&body_frame).ok_or_else(|| bad(cmd, "invalid cylinder body frame"))?;
        let displayed = world.sketch(sketch).ok_or_else(|| bad(cmd, "the profile sketch is not displayed"))?;
        let plane = solidcraft_geom::Plane::new(
            solidcraft_doc::apply_point(&inverse, displayed.plane.origin),
            solidcraft_doc::apply_vector(&inverse, displayed.plane.x),
            solidcraft_doc::apply_vector(&inverse, displayed.plane.y),
        )
        .ok_or_else(|| bad(cmd, "invalid cylinder sketch frame"))?;
        if let Some((_, at)) =
            solidcraft_kernel::infer_cylinder_emboss(&raw.body, &plane, &regions, if deboss { -d } else { d }).map_err(|e| bad(cmd, e.to_string()))?
        {
            face.sketch_frame(&placed, s.active_component, sketch_owner, raw).map_err(|e| bad(cmd, e.to_string()))?;
            check_emboss_positions(s, sketch, selected, &face)?;
            let author_inverse = solidcraft_doc::mat_inverse(&s.active_component_frame()).ok_or_else(|| bad(cmd, "invalid author frame"))?;
            face.at = FacePick(solidcraft_doc::apply_point(&solidcraft_doc::mat_mul(&author_inverse, &body_frame), at));
            if chosen.is_some() {
                return Err(bad(cmd, "multiple tangent cylinders match the selected profiles; select a target face or body"));
            }
            chosen = Some(face);
        }
    }
    Ok(chosen)
}
fn face_points(s: &Session, p: &Value, k: &str, cmd: &str) -> Result<Vec<Vec3>> {
    let a = p.get(k).and_then(Value::as_array).ok_or_else(|| bad(cmd, format!("`{k}` must list points [x, y, z] or face names")))?;
    if a.is_empty() || a.len() > 1000 {
        return Err(bad(cmd, format!("`{k}` must list 1…1000 points")));
    }
    a.iter().map(|v| super::face::face_ref(s, v, str_(p, "body"), cmd)).collect()
}

fn replace_face(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.replace_face";
    let faces = face_points(s, p, "faces", cmd)?;
    for f in &faces {
        if super::face::face_normal(s, *f).is_none() {
            return Err(bad(cmd, "not supported yet: replacing a face that is not planar"));
        }
    }
    let target = match p.get("target_face").and_then(vec3) {
        Some(tp) => {
            let n = super::face::face_normal(s, tp).ok_or_else(|| bad(cmd, "the target face must be planar"))?;
            let plane = solidcraft_geom::Plane::from_normal(tp, n).ok_or_else(|| bad(cmd, "the target face's plane"))?;
            solidcraft_doc::PlaneRef::Custom { plane }
        }
        None => plane_param(s, p.get("target"), cmd)?,
    };
    let body = str_(p, "body").map(str::to_string);
    add_feature(s, p, FeatureKind::ReplaceFace { faces: faces.into_iter().map(FacePick).collect(), target, body: body.map(BodyName) })
}

fn align(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.align";
    let bodies = string_list(p, "bodies");
    if bodies.is_empty() {
        return Err(bad(cmd, "`bodies` must list the bodies to move"));
    }
    {
        let st = s.model.state();
        if let Some(b) = bodies.iter().find(|b| st.body(b).is_none()) {
            return Err(bad(cmd, format!("no body `{b}`")));
        }
    }
    let pick = |s: &Session, pk: &str, fk: &str| -> Result<(Vec3, Option<Vec3>)> {
        if let Some(fp) = p.get(fk).and_then(vec3) {
            let n = super::face::face_normal(s, fp).ok_or_else(|| bad(cmd, format!("`{fk}` must be on a planar face")))?;
            return Ok((fp, Some(n)));
        }
        // A snap point: {snap: vertex|edge_mid|face_center|circle_center, at: [x,y,z]}.
        if let Some(o) = p.get(pk).filter(|v| v.is_object()) {
            let at = o.get("at").and_then(vec3).ok_or_else(|| bad(cmd, format!("`{pk}.at` must be [x, y, z]")))?;
            let kind = o.get("snap").and_then(Value::as_str).unwrap_or("vertex");
            return snap_point(s, kind, at).map(|q| (q, None)).ok_or_else(|| bad(cmd, format!("no {kind} to snap to near `{pk}`")));
        }
        p.get(pk).and_then(vec3).map(|v| (v, None)).ok_or_else(|| bad(cmd, format!("give `{pk}` or `{fk}` as [x, y, z]")))
    };
    let (from, from_normal) = pick(s, "from", "from_face")?;
    let (to, to_normal) = pick(s, "to", "to_face")?;
    let (from_normal, to_normal) = match (from_normal, to_normal) {
        (Some(a), Some(b)) => (Some(a), Some(b)),
        _ => (None, None),
    };
    let flip = bool_(p, "flip").unwrap_or(false);
    add_feature(
        s,
        p,
        FeatureKind::Align {
            bodies: bodies.into_iter().map(BodyName).collect(),
            from: Position(from),
            to: Position(to),
            from_normal: from_normal.map(Dir),
            to_normal: to_normal.map(Dir),
            flip,
        },
    )
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.remove";
    let bodies = string_list(p, "bodies");
    if bodies.is_empty() {
        return Err(bad(cmd, "`bodies` must list bodies"));
    }
    let st = s.model.state();
    if let Some(b) = bodies.iter().find(|b| st.body(b).is_none()) {
        return Err(bad(cmd, format!("no body `{b}`")));
    }
    add_feature(s, p, FeatureKind::Remove { bodies: bodies.into_iter().map(BodyName).collect() })
}

#[cfg(test)]
#[path = "features_more_tests.rs"]
mod tests;

/// A snap point near `at` on the model: the nearest vertex, edge midpoint, planar face's centre
/// or circular edge's centre (the end of a cylindrical face).
pub(super) fn snap_point(s: &Session, kind: &str, at: Vec3) -> Option<Vec3> {
    let st = s.world_state();
    let near = |pts: Vec<Vec3>| pts.into_iter().min_by(|a, b| a.dist(at).total_cmp(&b.dist(at)));
    match kind {
        "vertex" => near(
            st.bodies
                .iter()
                .flat_map(|b| b.mesh().edges.iter().flat_map(|e| [e.first().copied(), e.last().copied()]).flatten().collect::<Vec<_>>())
                .collect(),
        ),
        "edge_mid" => {
            let d = |e: &solidcraft_doc::kernel::EdgeInfo| e.points.windows(2).map(|w| at.dist_to_segment(w[0], w[1])).fold(f64::MAX, f64::min);
            st.bodies
                .iter()
                .filter_map(|b| b.body.edges((b.body.size() * 2e-3).max(1e-3)).ok())
                .flatten()
                .min_by(|a, b| d(a).total_cmp(&d(b)))
                .map(|e| e.mid)
        }
        "face_center" => st.bodies.iter().find_map(|b| {
            let i = solidcraft_doc::appearance::face_index_at(b, at)?;
            b.body.faces((b.body.size() * 2e-3).max(1e-3)).ok()?.into_iter().find(|f| f.index == i).map(|f| f.centroid)
        }),
        "circle_center" => st.bodies.iter().find_map(|b| {
            let c = solidcraft_doc::kernel::cylinder_face_at(&b.body, at)?;
            let along = (at - c.axis_point).dot(c.axis);
            let end = if (along - c.start).abs() <= (c.end - along).abs() { c.start } else { c.end };
            Some(c.axis_point + c.axis * end)
        }),
        _ => None,
    }
}

fn boundary_fill(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.boundary_fill";
    let tools = string_list(p, "tools");
    let st = s.model.state();
    if tools.is_empty() || tools.len() > 64 || tools.iter().any(|t| st.body(t).is_none()) {
        return Err(bad(cmd, "`tools` must list 1…64 bodies"));
    }
    let cells: Vec<Vec3> = p.get("cells").and_then(Value::as_array).map(|a| a.iter().filter_map(vec3).collect()).unwrap_or_default();
    if cells.is_empty() || cells.len() > 1000 {
        return Err(bad(cmd, "`cells` must list 1…1000 points [x, y, z], one inside each cell"));
    }
    let operation = super::features::operation(p, cmd)?;
    add_feature(
        s,
        p,
        FeatureKind::BoundaryFill {
            tools: tools.into_iter().map(BodyName).collect(),
            cells: cells.into_iter().map(Position).collect(),
            operation,
            remove_tools: bool_(p, "remove_tools").unwrap_or(false),
        },
    )
}
