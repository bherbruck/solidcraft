//! Solid feature commands: extrude, revolve, fillet, chamfer, primitives, combine, move.

use serde_json::{Value, json};
use solidcraft_doc::{
    AxisRef, Direction, Expr, Extent, FeatureKind, Operation, PointExpr, ProfileSel, RailFit, SweepOrientation, SweepRail, ThinSide, ThinWall,
    expr::Kind, point_expr,
};
use solidcraft_doc::{BodyName, FeatureName, SketchId};
use solidcraft_doc::{Dir, FacePick, Position};
use solidcraft_geom::Vec3;

use super::{CommandSpec, not_in_sketch};
use crate::params::{bad, bool_, expr, req_expr, str_, string_list, vec2, vec3};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("solid.extrude", "Extrude", extrude)
        .at("SOLID", "CREATE")
        .icon("extrude")
        .key("E")
        .params("distance: expr (or through_all: true); taper?: angle expr; sketch?: id|name (default: active or last sketch); profiles?: all | [index] | [{curves: [ids]}] (the profile whose outer loop is made of exactly these curves, or pieces of them; found again on every recompute, so it follows parameter changes; islands inside are its holes; an error when no profile or several match) | [{point:[x,y]}] (index and point picks are stored by their curves when that finds the same profile); face?: [x,y,z] (extrude a planar body face instead of a sketch profile); faces?: [[x,y,z],…] (coplanar faces of one body); face_body?: name (whose face, where faces of several bodies meet at the point); direction?: positive|negative|symmetric (distance is total thickness, split evenly around the start plane); convention?: total_distance|legacy_per_side (default total_distance; legacy preserves older symmetric extents); distance2?; start_offset?: expr (along the sketch normal, also with direction: negative); to?: [x,y,z] (To Object: a face, vertex or point; to_offset?: expr past it); from?: [x,y,z] (From Object: start at that face or point); operation?: new|join|cut|intersect|auto (cut into a body, otherwise join existing solids; new in an empty component); targets?: [body] (Join from a face, or from a sketch on a body's face, defaults to that face's body; explicit targets or participants override); name?; body_name?; thin?: true (Thin: a wall along sketch curves instead of the profiles) with curves?: [curve ids] (open chains and closed loops, joined where they meet end to end; default: every drawn curve of the sketch), thickness: expr, side?: center|inside|outside|two_sides (a closed loop's inside is the area it encloses, an open chain's the side it bends toward, a straight line's the left of its first curve as drawn; default center), thickness2?: expr (two_sides: the thickness outside); corners are mitred, crossing walls are one"),
    CommandSpec::new("solid.revolve", "Revolve", revolve)
        .at("SOLID", "CREATE")
        .icon("revolve")
        .params("axis: sketch line id | x | y (sketch axes) | X|Y|Z (world); angle?: expr (default 360 deg); angle2?: expr (Two Sides: the other side's angle); direction?: symmetric (angle split either side, independent of axis direction); convention?: sketch_normal|legacy_axis_start (default sketch_normal; legacy retains older angular starts); to?: [x,y,z] (To Object: turn until that face, vertex or point); sketch?, profiles? | face: [x,y,z] (a planar body face as the profile) and face_body?: name (whose face, where faces of several bodies meet), operation?, targets?, name?, body_name?"),
    CommandSpec::new("solid.sweep", "Sweep", sweep)
        .at("SOLID", "CREATE")
        .icon("sweep")
        .params("sketch: profile sketch; profiles?; path_sketch: sketch; path: [curve ids in order]; orientation?: perpendicular (default: square to the path, turning with it) | parallel (as drawn: the profile moves without turning; the path must keep crossing its plane) | [x,y,z] (square to the path, its turn about the path fixed by this direction); twist?: angle expr (the profile turns this much about the path over its length; within 4 turns); scale?: expr (the profile's scale at the end, linear along the path; 0.01…100); rails?: [{sketch?: sketch (default path_sketch), curves: [curve ids in order]}] (guide rails, one or two, each meeting every plane square to the path once along it, from the end nearest the path's start: one rail turns the profile so its point where the rail starts stays on the rail and scales it with the rail's distance from the path; two rails carry it between them, its points where they start staying on both; rails take the default orientation, no twist and no scale, and a path without sharp corners); rail_fit?: scale (default: the profile grows and shrinks evenly) | stretch (only across to the rail, between the rails); operation?, targets?, name?, body_name?"),
    CommandSpec::new("solid.loft", "Loft", loft).at("SOLID", "CREATE").icon("loft").params("sections: [{sketch, profiles?} | {sketch, point: id} (an apex)] (2 or more, in order); operation?, targets?, name?, body_name?"),
    CommandSpec::new("solid.box", "Box", prim_box).at("SOLID", "CREATE").icon("box").params("length, width, height: expr; corner?: [x,y,z] | center?: [x,y,z] (numbers in mm or length expressions; default: the origin); operation?, targets?: [body], name?, body_name?"),
    CommandSpec::new("solid.cylinder", "Cylinder", prim_cylinder).at("SOLID", "CREATE").icon("cylinder").params("radius | diameter, height: expr; base?: [x,y,z] (centre of the bottom: numbers in mm or length expressions; default: the origin); axis?: X|Y|Z|[x,y,z] (default Z); operation?, targets?: [body], name?, body_name?"),
    CommandSpec::new("solid.sphere", "Sphere", prim_sphere).at("SOLID", "CREATE").icon("sphere").params("radius | diameter: expr; center?: [x,y,z] (numbers in mm or length expressions; default: the origin); operation?, targets?: [body], name?, body_name?"),
    CommandSpec::new("solid.torus", "Torus", prim_torus).at("SOLID", "CREATE").icon("torus").params("major, minor: expr (radii); center?: [x,y,z] (numbers in mm or length expressions; default: the origin); operation?, targets?: [body], name?, body_name?"),
    CommandSpec::new("solid.fillet", "Fillet", fillet)
        .at("SOLID", "MODIFY")
        .icon("fillet")
        .key("F")
        .params("edges: [[x,y,z] point on edge | {body, index}]; radius: expr; body?; type?: constant|curvature (alias g2: curvature-continuous quintic between planar faces along straight edges, one perpendicular planar end face at each end; radius sets the equivalent circular setback, not constant curvature; exact fit only)|chord (radius is the width across)|variable (radius at the end nearest start, radius2 at the other; one end may be 0, where the round closes to the corner)|spline (the radius along the edge is a clamped B-spline from the end nearest start: radius is its first control radius, radii the rest, knots its knot vector from 0 to 1; one end may be 0); radius2?; start?: [x,y,z]; radii?: [expr, …]; knots?: [number, …]; mid?: [[position 0…1 from start, radius expr], …] (variable radii between the ends); smooth?: bool (smooth instead of straight between the radii); corner?: rolling|g2 (rolling, the default: the corners where three blended edges meet are pieces of the rolling ball's sphere, tangent to each blend; g2: a patch repeating each blend's surface up to the second order, constant-radius exact fillets only, refused where it cannot be built); fit?: exact|reduce|taper (exact, the default: a radius too large for the geometry around the edges is refused; reduce: where it doesn't fit it is reduced to the largest that does, one radius for edges sharing a face; the smallest used is returned as `radius_applied`, each edge's as `radii_applied: [{edge: [x,y,z], radius}]`, and noted in the feature's warning; taper: along each straight edge between planar faces where it doesn't fit, the radius follows the room there as a smooth spline, down to 0 where a face closes to a point, noted in the feature's warning)"),
    CommandSpec::new("solid.chamfer", "Chamfer", chamfer).at("SOLID", "MODIFY").icon("chamfer").params("edges: [[x,y,z] | {body, index}]; distance: expr; distance2?: expr (along the second face) | angle?: expr (from the first face); flip?: bool (which face is first: by default the one facing up most); body?"),
    CommandSpec::new("solid.combine", "Combine", combine).at("SOLID", "MODIFY").icon("combine").params("target: body; tools: [body]; operation?: join|cut|intersect; keep_tools?: bool; new_component?: bool (the result goes into a new component; component_name?)"),
    CommandSpec::new("solid.pattern.rectangular", "Rectangular Pattern", pattern_rect)
        .at("SOLID", "CREATE")
        .icon("pattern_rect")
        .params("features: [names] | bodies: [names] | components: [names]; dir1: [x,y,z]; count1; spacing1; dir2?, count2?, spacing2?; alignment?, alignment2?: left|center|right (default right; counts include the original); reversed?, reversed2?: bool (Flip)"),
    CommandSpec::new("solid.pattern.circular", "Circular Pattern", pattern_circ)
        .at("SOLID", "CREATE")
        .icon("pattern_circ")
        .params("features: [names] | bodies: [names] | components: [names]; axis: X|Y|Z | {origin, dir}; count; angle? (default 360 deg; partial arcs include both ends); alignment?: left|center|right (default right; original stays fixed); reversed?: bool (Flip; selects the extra side for even centered counts)"),
    CommandSpec::new("solid.pattern.path", "Pattern on Path", pattern_path)
        .at("SOLID", "CREATE")
        .icon("pattern_path")
        .params("features: [names] | bodies: [names]; path_sketch: sketch; path: [curve ids in order]; count; spacing (between instances) | distance (first to last); orientation?: identical|path; flip?: bool"),
    CommandSpec::new("solid.mirror", "Mirror", mirror).at("SOLID", "CREATE").icon("mirror").params("features: [names] | bodies: [names] (combine?: join with the original); plane: XY|XZ|YZ|construction plane name or id|{origin, normal}|{origin, x_dir, y_dir} (origin coordinates may be length expressions; the mirror follows them, or a construction plane)"),
    CommandSpec::new("solid.hole", "Hole", hole)
        .at("SOLID", "CREATE")
        .icon("hole")
        .key("H")
        .params("position: [x,y,z] on a face (numbers, or length expressions the hole follows) | sketch + points: [sketch point ids] (one hole each, perpendicular to the sketch, into the material on whichever side of the sketch plane it is) | sketch alone (every sketch point: standalone points and circle centres); flip?: bool (sketch points: drill to the other side); direction?: [x,y,z] (default: into the face, against its outward normal; one that leaves the face is mirrored into it; ignored with sketch points); diameter | thread (a tapped hole: the tap drill, major − pitch) | clearance: \"M6\" with fit?: close|normal|loose (ISO 273); depth? (default through all) | to?: [x,y,z] (To Object: drill to that face or point); type?: simple|drilled|counterbore|countersink; tip_angle?; cb_diameter?, cb_depth?; cs_diameter?, cs_angle?; thread?: \"M6\" (cosmetic)"),
    CommandSpec::new("solid.pipe", "Pipe", pipe)
        .at("SOLID", "CREATE")
        .icon("pipe")
        .params("path_sketch: sketch; path: [curve ids in order]; diameter: expr; wall?: expr (hollow); operation?, targets?, name?, body_name?"),
    CommandSpec::new("solid.bounding_solid", "Bounding Solid", bounding_solid)
        .at("SOLID", "CREATE")
        .icon("bounding_solid")
        .params("bodies?: [names] (default: all); margin?: expr; name?, body_name?"),
    CommandSpec::new("solid.scale", "Scale", scale)
        .at("SOLID", "MODIFY")
        .icon("scale")
        .params("bodies: [names]; factor: expr | factors: [x, y, z] exprs; origin?: [x,y,z]"),
    CommandSpec::new("face.delete", "Delete Face", delete_face)
        .at("SOLID", "MODIFY")
        .icon("delete_face")
        .params("faces: [[x,y,z] points on the faces, or face names from model.faces]; body?; heal?: bool (default true: the neighbouring planes close the gap, as when a rounded edge, hole or boss is taken off; false: the gap stays open and the faces left are a surface body)"),
    CommandSpec::new("solid.offset_face", "Offset Face", offset_face)
        .at("SOLID", "MODIFY")
        .icon("offset_face")
        .params("faces: [[x,y,z] points on planar faces, or face names from model.faces]; distance: expr (positive: outward); body?"),
    CommandSpec::new("solid.thread", "Thread", thread)
        .at("SOLID", "CREATE")
        .icon("thread")
        .params("face: [x,y,z] on a cylindrical face; designation?: ISO metric (\"M8\", \"M8x1\"; default: the size that fits); length?: expr (default: the whole face); modeled?: bool (default false). Cosmetic: the model is unchanged, model.threads lists it. Modeled: cuts the thread profile into the body"),
    CommandSpec::new("solid.shell", "Shell", shell)
        .at("SOLID", "MODIFY")
        .icon("shell")
        .params("faces: [[x,y,z] points on the faces to remove, or face names from model.faces]; thickness: expr; direction?: inside|outside|both; tangent_chain?: bool (also open faces joined smoothly to the picked ones); body?"),
    CommandSpec::new("solid.draft", "Draft", draft)
        .at("SOLID", "MODIFY")
        .icon("draft")
        .params("faces: [[x,y,z] points, or face names from model.faces]; angle: expr; neutral: XY|XZ|YZ|plane name|{origin, normal}; pull?: [x,y,z] (default: neutral plane normal); body?"),
    CommandSpec::new("construct.plane.offset", "Offset Plane", plane_offset)
        .at("SOLID", "CONSTRUCT")
        .icon("plane")
        .params("base: XY|XZ|YZ|plane name, or a planar face {face: [x,y,z], body?, index?, path?}; offset: expr; name?"),
    CommandSpec::new("construct.plane.at_angle", "Plane at Angle", plane_angle)
        .at("SOLID", "CONSTRUCT")
        .icon("plane_angle")
        .params("base: XY|XZ|YZ|plane name; axis: X|Y|Z|{origin, dir}; angle: expr; name?"),
    CommandSpec::new("solid.split_face", "Split Face", split_face)
        .at("SOLID", "MODIFY")
        .icon("split_face")
        .params(
            "faces: [[x,y,z] points on the faces to split, or face names from model.faces]; body?; and the splitting tool: plane: XY|XZ|YZ|plane name|{origin, normal} — or tool: {body, point} (a face of a body, its surface extended) — or sketch: id or name (its curves, along the sketch's normal)",
        ),
    CommandSpec::new("solid.split_body", "Split Body", split_body)
        .at("SOLID", "MODIFY")
        .icon("split_body")
        .params("body: name; plane: XY|XZ|YZ|plane name|{origin, normal} — or tool: {body, point} (a face of a body, its surface extended) — or sketch: id, line: curve id (a straight sketch line, along the sketch's normal)"),
    CommandSpec::new("solid.press_pull", "Press Pull", press_pull)
        .at("SOLID", "MODIFY")
        .icon("presspull")
        .key("Q")
        .params("face: [x,y,z] and distance: expr (positive adds material, negative cuts), or edges: [[x,y,z]…] and distance (a fillet)"),
    CommandSpec::new("solid.move", "Move/Copy", move_bodies).at("SOLID", "MODIFY").icon("move").key("M").enabled(not_in_sketch).params("bodies: [names]; translate?: [x,y,z] (exprs or numbers); axis?: [x,y,z]; angle?: expr; copy?: bool (default false: keeps the original bodies and transforms a new copy instead of moving them in place)"),
];

/// Where an extrude starts and which way is positive: a point inside the (first) profile or
/// face, and the unit normal.
fn extrude_base(s: &Session, p: &Value) -> Option<(Vec3, Vec3)> {
    if let Some(fp) = p.get("face").or_else(|| p.get("faces").and_then(Value::as_array).and_then(|faces| faces.first())).and_then(vec3) {
        return Some((fp, super::face::face_normal_in(s, fp, str_(p, "face_body"))?));
    }
    let sid = feature_sketch(s, p, "Extrude").ok()?;
    let st = s.model.state();
    let ss = st.sketch(sid)?;
    if bool_(p, "thin").unwrap_or(false) {
        let thin = thin_param(s, p, sid, "solid.extrude").ok()?;
        let q = thin_regions(s, ss, &thin).ok()?.first()?.interior_point();
        return Some((ss.plane.to_world(q), ss.plane.normal()));
    }
    let ps = &ss.profiles;
    let q = match profiles(p, "Extrude").ok()? {
        ProfileSel::All => ps.first()?.region.interior_point(),
        ProfileSel::Indices { indices } => ps.get(*indices.first()?)?.region.interior_point(),
        ProfileSel::Points { points } => *points.first()?,
        selection @ (ProfileSel::Curves { .. } | ProfileSel::OffsetBoundaries { .. } | ProfileSel::WholeBoundaries { .. }) => {
            let indices = solidcraft_doc::profile_selection_indices(ss, &selection).ok()?;
            ps.get(*indices.first()?)?.region.interior_point()
        }
    };
    Some((ss.plane.to_world(q), ss.plane.normal()))
}

/// How far an extrude with these parameters goes from its profile along the profile's normal
/// (negative: the other way). To an object, the target's height, on whichever side it is: the
/// extrude goes there and its `direction` does not count (#1633). Otherwise its distance, turned
/// round for the negative direction.
pub fn extrude_reach(s: &Session, p: &Value) -> Option<f64> {
    if let Some(t) = p.get("to").and_then(vec3) {
        let (base, n) = extrude_base(s, p)?;
        return Some((t - base).dot(n)).filter(|h| h.is_finite());
    }
    let d = s.doc.eval(&expr(p, "distance")?, Kind::Length).ok()?;
    let dir = str_(p, "direction").map(str::to_ascii_lowercase);
    Some(if matches!(dir.as_deref(), Some("negative" | "reverse" | "flip")) { -d } else { d })
}

/// How far past the profile [`auto_operation`] looks for material it starts in.
const START_PROBE_MM: f64 = 0.1;

/// What `operation: "auto"` means for an extrude with these parameters: `cut` when it goes
/// into a body, otherwise `join` when the active component has a solid. Keeping an
/// addition separate requires an explicit `new` operation.
///
/// "Goes into a body" is decided where the extrude starts: just past the profile in the
/// direction it grows, or halfway along it. With the halfway point alone, a cut dragged far
/// past a body's far side read as a Join (#1257).
pub fn auto_operation(s: &Session, p: &Value) -> Option<&'static str> {
    auto_operation_in(s, p, true)
}

fn auto_operation_in(s: &Session, p: &Value, world_faces: bool) -> Option<&'static str> {
    let symmetric = p.get("to").is_none() && str_(p, "direction").is_some_and(|d| d.eq_ignore_ascii_case("symmetric"));
    // Imported face groups can belong to a child of an empty active component.
    // Probe in the same coordinate frame as the face picks.
    let grouped = p.get("faces").and_then(Value::as_array).and_then(|faces| faces.first()).and_then(vec3);
    let st = if grouped.is_some() && world_faces { s.world_state() } else { s.model.state() };
    let (base, n) = if let Some(point) = grouped {
        let (mesh, index, distance, name) = super::face::face_at_state(&st, point, str_(p, "face_body")).ok().flatten()?;
        if distance > (mesh.bounds().diagonal() * 1e-3).max(1e-3) * 10.0 {
            return None;
        }
        let normal = st.body(&name)?.body.planar_face(index as usize)?.normal();
        (point, normal)
    } else {
        extrude_base(s, p)?
    };
    let d = if grouped.is_some() && p.get("to").is_some() { (p.get("to").and_then(vec3)? - base).dot(n) } else { extrude_reach(s, p)? };
    if st.bodies.is_empty() || !d.is_finite() {
        return Some("new");
    }
    // Match the evaluator's implicit participants: a grouped cut from an empty
    // assembly component considers every body; otherwise keep the active scope.
    // Explicit participants override either default.
    let parts = string_list(p, "participants");
    let implicit_all = grouped.is_some() && !st.bodies.iter().any(|body| s.doc.model_body_component(body) == s.active_component);
    let bodies: Vec<_> = st
        .bodies
        .iter()
        .filter(|b| if parts.is_empty() { implicit_all || s.doc.model_body_component(b) == s.active_component } else { parts.contains(&b.name) })
        .collect();
    if bodies.is_empty() {
        return Some("new");
    }
    let inside = |q: Vec3| bodies.iter().any(|b| b.mesh().contains(q));
    // A step into whatever is right in front of the profile, short of the far end.
    let near = |d: f64| d.signum() * (d.abs() * 0.5).min(START_PROBE_MM);
    let into = |d: f64| inside(base + n * near(d)) || inside(base + n * (d * 0.5));
    if symmetric {
        return Some(if into(d.abs()) || into(-d.abs()) { "cut" } else { "join" });
    }
    if into(d) {
        Some("cut")
    } else {
        // A profile can meet material along its side, or only around part of a
        // screw boss. A single point behind its centre cannot establish contact.
        // Default additions to Join and let the boolean merge the actual geometry.
        Some("join")
    }
}

pub(super) fn operation(p: &Value, cmd: &str) -> Result<Operation> {
    match str_(p, "operation") {
        None => Ok(Operation::NewBody),
        Some(o) => Operation::parse(o).ok_or_else(|| bad(cmd, format!("unknown operation `{o}` (new, join, cut, intersect)"))),
    }
}

/// The sketch a feature uses: `sketch` param, the active sketch, or the last sketch.
pub(super) fn feature_sketch(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    if let Some(v) = p.get("sketch") {
        let key = match v {
            Value::Number(n) => n.to_string(),
            Value::String(x) => x.clone(),
            _ => return Err(bad(cmd, "`sketch` must be an id or name")),
        };
        let f = s.doc.find_feature(&key).ok_or_else(|| bad(cmd, format!("no sketch `{key}`")))?;
        return match f.kind {
            FeatureKind::Sketch { .. } => Ok(f.id),
            _ => Err(bad(cmd, format!("`{key}` is not a sketch"))),
        };
    }
    if let Some(a) = s.active_sketch {
        return Ok(a);
    }
    let limit = s.doc.marker.unwrap_or(usize::MAX);
    s.doc
        .features
        .iter()
        .take(limit)
        .rev()
        .find(|f| matches!(f.kind, FeatureKind::Sketch { .. }))
        .map(|f| f.id)
        .ok_or_else(|| bad(cmd, "there is no sketch to use (create one first)"))
}

/// Profiles picked by index or point are stored by the curve ids of each profile's outer loop
/// as the sketch is now solved, so they follow the geometry when parameters move it. A pick
/// whose curves would not find that same profile again (two profiles bounded by the same
/// curves) stays as given. Curve picks are checked: one that names no profile, or several, is
/// refused now rather than failing later.
pub(super) fn stable_profiles(s: &Session, kind: &mut FeatureKind) -> Result<()> {
    use solidcraft_doc::sketch::profile_at_point;
    let st = s.model.state();
    for (sketch, sel) in kind.profile_sels_mut() {
        let Some(ss) = st.sketch(sketch) else { continue };
        let indices = match sel {
            ProfileSel::Indices { .. } => solidcraft_doc::profile_selection_indices(ss, sel)?,
            ProfileSel::Points { points } => {
                let Some(indices) = points.iter().map(|point| profile_at_point(&ss.profiles, *point)).collect::<Option<Vec<_>>>() else { continue };
                indices
            }
            ProfileSel::Curves { loops } => {
                // The command's ordinary curve lists are sets, including repeated
                // caller IDs. Saved graph-span multisets retain their legacy meaning.
                // Reserved offset roles never use this ordinary normalization.
                let mut normalized = loops.clone();
                let reserved = ss
                    .sketch
                    .offsets
                    .iter()
                    .filter_map(|offset| offset.topology.as_ref())
                    .flat_map(|topology| topology.roles.iter().map(|role| role.output.as_str()))
                    .collect::<std::collections::BTreeSet<_>>();
                for curves in &mut normalized {
                    let dynamic = curves.iter().any(|curve| reserved.contains(curve.as_str()));
                    if !dynamic {
                        let mut seen = std::collections::BTreeSet::new();
                        curves.retain(|curve| seen.insert(curve.clone()));
                    }
                }
                let selection = ProfileSel::Curves { loops: normalized };
                solidcraft_doc::profile_selection_indices(ss, &selection)?;
                *sel = selection;
                continue;
            }
            ProfileSel::OffsetBoundaries { .. } | ProfileSel::WholeBoundaries { .. } => {
                // Explicit identities retain the caller's semantics. Validate through the
                // same resolver as recomputation, including reserved-role precedence.
                solidcraft_doc::profile_selection_indices(ss, sel)?;
                continue;
            }
            ProfileSel::All => continue,
        };
        let captured = solidcraft_doc::capture_profile_selection(ss, &indices)?;
        *sel = match captured {
            ProfileSel::OffsetBoundaries { associations }
                if associations.iter().any(|id| {
                    s.doc
                        .sketch(sketch)
                        .is_ok_and(|raw| !raw.offsets.iter().filter_map(|offset| offset.topology.as_ref()).any(|topology| topology.id == *id))
                }) =>
            {
                // Legacy upgrade descriptors belong to the solved copy. Persist the
                // complete original role IDs instead of an unsaved association ID.
                let loops = associations
                    .iter()
                    .map(|id| {
                        ss.sketch
                            .offsets
                            .iter()
                            .filter_map(|offset| offset.topology.as_ref())
                            .find(|topology| topology.id == *id)
                            .map(|topology| topology.roles.iter().map(|role| role.output.clone()).collect())
                            .ok_or_else(|| EngineError::Other(format!("offset association `{id}` changed during profile capture")))
                    })
                    .collect::<Result<Vec<_>>>()?;
                ProfileSel::Curves { loops }
            }
            captured => captured,
        };
    }
    Ok(())
}

pub(super) fn profiles(p: &Value, cmd: &str) -> Result<ProfileSel> {
    let Some(v) = p.get("profiles") else { return Ok(ProfileSel::All) };
    match v {
        Value::Object(fields) => {
            if fields.get("type").and_then(Value::as_str) == Some("whole_boundaries") {
                return solidcraft_doc::whole_profile_selection_from_value(v).map_err(|error| bad(cmd, error.to_string()));
            }
            let associations = fields
                .get("offset_boundaries")
                .or_else(|| (fields.get("type").and_then(Value::as_str) == Some("offset_boundaries")).then(|| fields.get("associations")).flatten())
                .and_then(Value::as_array)
                .ok_or_else(|| bad(cmd, "profile object must provide `offset_boundaries: [association ids]`"))?;
            if associations.is_empty() || associations.len() > 128 {
                return Err(bad(cmd, "offset profile selection needs 1 to 128 association ids"));
            }
            let associations = associations
                .iter()
                .map(|id| {
                    id.as_str()
                        .filter(|id| !id.is_empty() && id.len() <= 512)
                        .map(str::to_string)
                        .ok_or_else(|| bad(cmd, "offset association ids must be nonempty strings of at most 512 bytes"))
                })
                .collect::<Result<Vec<_>>>()?;
            if associations.iter().collect::<std::collections::BTreeSet<_>>().len() != associations.len() {
                return Err(bad(cmd, "offset association ids must be unique"));
            }
            Ok(ProfileSel::OffsetBoundaries { associations })
        }
        Value::String(s) if s.eq_ignore_ascii_case("all") => Ok(ProfileSel::All),
        Value::Number(n) => Ok(ProfileSel::Indices { indices: vec![n.as_u64().ok_or_else(|| bad(cmd, "profile index"))? as usize] }),
        Value::Array(a) if a.is_empty() => Err(bad(cmd, "`profiles` is empty")),
        Value::Array(a) if a.len() > 10_000 => Err(bad(cmd, "too many profiles")),
        Value::Array(a) => {
            if a.iter().all(Value::is_u64) {
                return Ok(ProfileSel::Indices { indices: a.iter().filter_map(Value::as_u64).map(|x| x as usize).collect() });
            }
            if a.iter().all(|x| x.get("point").is_some()) {
                let pts =
                    a.iter().map(|x| x.get("point").and_then(vec2).ok_or_else(|| bad(cmd, "profile point must be [x, y]"))).collect::<Result<_>>()?;
                return Ok(ProfileSel::Points { points: pts });
            }
            // [[ids]], [{curves: [...]}] or recipe-style [{loops: [{outer: true, curves: [...]}]}].
            let mut loops = Vec::new();
            for x in a {
                let ids: Vec<String> = match x {
                    Value::Array(ids) => ids.iter().filter_map(|i| i.as_str().map(str::to_string)).collect(),
                    Value::Object(o) => {
                        if let Some(c) = o.get("curves") {
                            string_list(&json!({"c": c}), "c")
                        } else if let Some(ls) = o.get("loops").and_then(Value::as_array) {
                            let outer = ls.iter().find(|l| l.get("outer").and_then(Value::as_bool).unwrap_or(true)).or(ls.first());
                            outer.map(|l| string_list(l, "curves")).unwrap_or_default()
                        } else {
                            Vec::new()
                        }
                    }
                    _ => Vec::new(),
                };
                if ids.is_empty() {
                    return Err(bad(cmd, "each profile must be an index, a list of curve ids, {curves}, {loops} or {point}"));
                }
                loops.push(ids);
            }
            Ok(ProfileSel::Curves { loops })
        }
        _ => Err(bad(cmd, "`profiles` must be \"all\" or a list")),
    }
}

pub(super) fn check_expr(s: &Session, e: &str, kind: Kind, cmd: &str, what: &str) -> Result<()> {
    s.doc.eval(e, kind).map(|_| ()).map_err(|err| bad(cmd, format!("{what}: {err}")))
}

/// Add a feature, evaluate, and report its result (an evaluation error fails the command).
pub(super) fn add_feature(s: &mut Session, p: &Value, kind: FeatureKind) -> Result<Value> {
    add_feature_in(s, p, kind, true)
}

/// `add_feature`; `world` is false when `kind` is already in the active component's frame.
fn add_feature_in(s: &mut Session, p: &Value, kind: FeatureKind, world: bool) -> Result<Value> {
    let name = str_(p, "name");
    // The bodies before the feature, to say which ones it changed or removed.
    let before = s.model.state();
    // Commands speak world coordinates; the feature is authored in the active component.
    let mut kind = kind;
    if world {
        super::component::to_active_frame(s, &mut kind);
    }
    // Bare lengths typed in an inch (or cm…) design mean that unit.
    if s.doc.units.trim() != "mm" {
        let doc = s.doc.clone();
        for (_, e, k) in kind.inputs_mut() {
            *e = doc.with_design_unit(e, k).into();
        }
    }
    // Profiles picked by index are kept by the curves around them: a recompute can renumber a
    // sketch's profiles (a face sketch's projected outline rebuilt), never the curves.
    stable_profiles(s, &mut kind)?;
    // Blends remember where their edges sat, to find them again after upstream edits.
    let (refs, names) = match &kind {
        FeatureKind::Fillet { edges, body, .. } | FeatureKind::Chamfer { edges, body, .. } => {
            let st = s.model.state();
            (
                solidcraft_doc::edge_refs(&st, body, &solidcraft_doc::plain(edges)),
                solidcraft_doc::edge_names_for(&st, body, &solidcraft_doc::plain(edges)),
            )
        }
        // Sheet metal edges are named by the sheet model.
        FeatureKind::SheetFlange { edges, .. } => {
            let points: Vec<_> = edges.iter().map(|e| e.point).collect();
            (Vec::new(), solidcraft_doc::sheet::sheet_edge_names(&s.model.state(), &solidcraft_doc::plain(&points)))
        }
        FeatureKind::SheetHem { edges, .. } => (Vec::new(), solidcraft_doc::sheet::sheet_edge_names(&s.model.state(), &solidcraft_doc::plain(edges))),
        // A conversion's bend edges are edges of the body it converts.
        FeatureKind::SheetConvert { body, bends, .. } if !bends.is_empty() => {
            (Vec::new(), solidcraft_doc::sheet::bend_edge_names(&s.model.state(), body, &solidcraft_doc::plain(bends)))
        }
        _ => (Vec::new(), Vec::new()),
    };
    let face_names = {
        let pts = kind.face_points();
        if let FeatureKind::Emboss { face: Some(face), .. } = &kind {
            let state = s.model.state();
            let body = state.body(&face.body).ok_or_else(|| EngineError::Other("selected cylinder body no longer exists".into()))?;
            let frame = face.body_frame(&s.doc, body).map_err(|e| EngineError::Other(e.to_string()))?;
            let inverse = solidcraft_doc::mat_inverse(&frame).ok_or_else(|| EngineError::Other("invalid cylinder placement".into()))?;
            let at = solidcraft_doc::apply_point(&solidcraft_doc::mat_mul(&inverse, &s.active_component_frame()), face.at.0);
            let mut scoped = solidcraft_doc::ModelState::default();
            scoped.bodies.push(body.clone());
            let names = solidcraft_doc::naming::face_names_at(&scoped, &[at]);
            if names.len() != 1 || names.first().is_none_or(|name| name.is_empty()) {
                return Err(EngineError::Other("selected cylinder face has no persistent identity".into()));
            }
            names
        } else if pts.is_empty() {
            Vec::new()
        } else {
            solidcraft_doc::face_point_names(&s.doc, &s.model.state(), &kind, s.active_component)
        }
    };
    // Primitives have no `targets` of their own: their `targets` are the bodies taking part (#259).
    let primitive = matches!(kind, FeatureKind::Box { .. } | FeatureKind::Cylinder { .. } | FeatureKind::Sphere { .. } | FeatureKind::Torus { .. });
    let (participants, key) = match string_list(p, "participants") {
        v if v.is_empty() && primitive => (string_list(p, "targets"), "targets"),
        v => (v, "participants"),
    };
    if participants.len() > 10_000 || participants.iter().any(|b| b != "*" && s.model.state().body(b).is_none()) {
        return Err(EngineError::Other(format!("`{key}` must list existing bodies")));
    }
    // A cut or intersect made where the active component has no body of its own (a sketch in
    // the assembly's root, cutting through the parts, #1234) would have nothing to work on: it
    // takes part in every body instead, which it changes only where it meets them.
    let participants = if participants.is_empty()
        && !primitive
        && (matches!(str_(p, "operation").and_then(Operation::parse), Some(Operation::Cut | Operation::Intersect))
            || p.get("faces").is_some() && matches!(&kind, FeatureKind::Extrude { operation: Operation::Cut | Operation::Intersect, .. }))
        && {
            let st = s.model.state();
            !st.bodies.is_empty() && !st.bodies.iter().any(|b| s.doc.model_body_component(b) == s.active_component)
        } {
        vec!["*".to_string()]
    } else {
        participants
    };
    if matches!(kind, FeatureKind::ConstructionPlane { .. })
        && let Some(n) = name
    {
        check_plane_name(n, "construct.plane")?;
    }
    let id = s.doc_mut().add_feature(kind, name)?;
    if let Some(f) = s.doc_mut().feature_mut(id) {
        f.participants = participants;
        f.edge_refs = refs;
        f.edge_names = names;
        f.face_names = face_names;
    }
    s.doc_mut().name_feature_inputs(id);
    let comp = s.active_component;
    if comp != 0
        && let Some(f) = s.doc_mut().feature_mut(id)
    {
        f.component = comp;
    }
    let mut names = string_list(p, "body_names");
    if let Some(b) = str_(p, "body_name") {
        names.insert(0, b.to_string());
    }
    if !names.is_empty()
        && let Some(f) = s.doc_mut().feature_mut(id)
    {
        f.body_names = names.clone();
    }
    s.active_sketch = None;
    s.refresh();
    // A feature that makes more bodies than it was given names (a multi-profile extrude with one
    // `body_name`) has the rest numbered after the first by the evaluator: `Ribs`, `Ribs (2)`,
    // `Ribs (3)`… (#266, #373). They are stored on the feature, so recomputes and saved designs
    // keep them even when the profiles are later picked in another order.
    if names.first().is_some_and(|n| !n.trim().is_empty()) {
        let made: Vec<String> = s.model.state().bodies.iter().filter(|b| b.feature == id).map(|b| b.name.clone()).collect();
        if made.len() > names.len()
            && let Some(f) = s.doc_mut().feature_mut(id)
        {
            f.body_names = made;
        }
    }
    let r = s.model.result(id).cloned();
    if let Some(e) = r.as_ref().and_then(|r| r.error.clone()) {
        return Err(EngineError::Other(e));
    }
    let st = s.model.state();
    let bodies: Vec<&str> = st.bodies.iter().map(|b| b.name.as_str()).collect();
    // The bodies this feature made, apart from the ones it changed or that were there (#374).
    let created: Vec<&str> = st.bodies.iter().filter(|b| b.feature == id).map(|b| b.name.as_str()).collect();
    // Bodies that were there before and now have other geometry, and bodies that are gone.
    let old: std::collections::HashMap<&str, &solidcraft_doc::kernel::Body> = before.bodies.iter().map(|b| (b.name.as_str(), &b.body)).collect();
    let now: std::collections::HashSet<&str> = st.bodies.iter().map(|b| b.name.as_str()).collect();
    let changed: Vec<&str> =
        st.bodies.iter().filter(|b| b.feature != id && old.get(b.name.as_str()).is_some_and(|o| !o.same(&b.body))).map(|b| b.name.as_str()).collect();
    let removed: Vec<&str> = before.bodies.iter().map(|b| b.name.as_str()).filter(|n| !now.contains(n)).collect();
    let fname = s.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default();
    // A warning (a Boolean that needed a retry, a reference found again elsewhere) is part of
    // the reply: a script or an agent sees it without asking the timeline (#616).
    let warning = r.as_ref().and_then(|r| r.warning.clone());
    Ok(json!({"feature": id, "name": fname, "bodies": bodies, "created_bodies": created, "changed_bodies": changed,
        "removed_bodies": removed, "warning": warning, "ms": r.map(|r| r.ms)}))
}

fn extrude(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.extrude";
    let grouped = if let Some(value) = p.get("faces") {
        if p.get("face").is_some() || p.get("sketch").is_some() || p.get("profiles").is_some() {
            return Err(bad(cmd, "faces cannot be combined with face, sketch or profiles"));
        }
        let faces = value
            .as_array()
            .filter(|faces| !faces.is_empty() && faces.len() <= 1024)
            .ok_or_else(|| bad(cmd, "faces must contain between 1 and 1024 points"))?;
        let points = faces.iter().map(|point| vec3(point).ok_or_else(|| bad(cmd, "faces must be points [x,y,z]"))).collect::<Result<Vec<_>>>()?;
        let (id, points, body) = super::face::sketch_of_faces(s, &points, cmd, str_(p, "face_body"))?;
        Some((id, Some(ProfileSel::Points { points }), Some(body)))
    } else {
        None
    };
    // A planar body face extrudes like a profile: its boundary is copied into a sketch on it.
    let (sketch, face_profile, face_body) = if let Some(grouped) = grouped {
        grouped
    } else {
        match p.get("face").and_then(vec3) {
            Some(fp) => {
                let (id, inside, body) = super::face::sketch_of_face(s, fp, cmd, str_(p, "face_body"))?;
                (id, Some(ProfileSel::Points { points: vec![inside] }), Some(body))
            }
            None => (feature_sketch(s, p, cmd)?, None, None),
        }
    };
    let extent = extent_param(s, p, cmd)?;
    let thin = match bool_(p, "thin") {
        Some(true) => {
            if face_profile.is_some() || p.get("profiles").is_some() {
                return Err(bad(cmd, "a thin extrude follows `curves`; it takes no `profiles` or `face`"));
            }
            Some(thin_param(s, p, sketch, cmd)?)
        }
        _ if p.get("thin").is_some_and(|v| !v.is_boolean()) => return Err(bad(cmd, "`thin` must be true or false")),
        _ => None,
    };
    let operation = match str_(p, "operation") {
        Some(o) if o.eq_ignore_ascii_case("auto") => Operation::parse(auto_operation_in(s, p, false).unwrap_or("new")).unwrap_or(Operation::NewBody),
        _ => operation(p, cmd)?,
    };
    let mut targets = string_list(p, "targets");
    if operation == Operation::Join && targets.is_empty() && string_list(p, "participants").is_empty() {
        // A face's outward extension joins that body, and so does a profile sketched on a body's
        // face. Assembly neighbours can touch only an edge of the tool: trying to join them
        // first fails, or absorbs unrelated parts (#226). A profile drawn past the face can lie
        // on a neighbour's face too: a rib on a bracket's side welded in the motor the bracket
        // holds, a join that ran for minutes and failed (#1349).
        // Store the resolved target so recomputes and reopening keep the same scope.
        targets.extend(face_body.or_else(|| sketch_face_body(s, sketch)));
    }
    let kind = FeatureKind::Extrude {
        sketch: SketchId(sketch),
        profiles: match face_profile {
            Some(f) => f,
            None if thin.is_some() => ProfileSel::All,
            None => profiles(p, cmd)?,
        },
        extent,
        operation,
        targets: targets.into_iter().map(BodyName).collect(),
        thin,
    };
    add_feature(s, p, kind)
}

/// The body whose face `sketch` is attached to (on the face itself, not offset from it), when
/// it is a body of the active component.
fn sketch_face_body(s: &Session, sketch: u64) -> Option<String> {
    let FeatureKind::Sketch { plane: solidcraft_doc::PlaneRef::Face { anchor: Some(anchor), .. }, .. } = &s.doc.feature(sketch)?.kind else {
        return None;
    };
    let st = s.model.state();
    let body = st.body(&anchor.target.body)?;
    (s.doc.model_body_component(body) == s.active_component).then(|| body.name.clone())
}

/// A thin extrude's wall from its parameters: the sketch's curves it follows (every drawn
/// curve when none are given), its thickness and which side of them it lies on.
fn thin_param(s: &Session, p: &Value, sketch: u64, cmd: &str) -> Result<ThinWall> {
    let st = s.model.state();
    let ss = st.sketch(sketch).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?;
    let mut curves = string_list(p, "curves");
    if curves.is_empty() {
        curves =
            ss.sketch.curves.iter().filter(|c| !c.construction && !c.centerline && !c.infinite && c.link.is_none()).map(|c| c.id.clone()).collect();
    }
    if curves.is_empty() || curves.len() > 1000 {
        return Err(bad(cmd, "a thin extrude needs 1…1000 sketch curves (`curves`)"));
    }
    if let Some(c) = curves.iter().find(|c| ss.sketch.curve_index(c).is_none()) {
        return Err(bad(cmd, format!("no curve `{c}` in the sketch")));
    }
    let thickness = req_expr(cmd, p, "thickness")?;
    check_expr(s, &thickness, Kind::Length, cmd, "thickness")?;
    let side = match str_(p, "side") {
        None => ThinSide::Center,
        Some(o) => ThinSide::parse(o).ok_or_else(|| bad(cmd, format!("unknown side `{o}` (center, inside, outside or two_sides)")))?,
    };
    let thickness2 = expr(p, "thickness2");
    match (&thickness2, side) {
        (Some(t), ThinSide::TwoSides) => check_expr(s, t, Kind::Length, cmd, "thickness2")?,
        (None, ThinSide::TwoSides) => return Err(bad(cmd, "`side: two_sides` needs `thickness2` (the thickness outside)")),
        (Some(_), _) => return Err(bad(cmd, "`thickness2` goes with `side: two_sides`")),
        (None, _) => {}
    }
    let thin = ThinWall { curves, thickness, side, thickness2 };
    // Refuse a wall that can't be built now, with the reason, rather than a failed feature.
    thin_regions(s, ss, &thin).map_err(|e| bad(cmd, e))?;
    Ok(thin)
}

/// The regions of a thin wall in its (solved) sketch, with the current parameter values.
fn thin_regions(s: &Session, ss: &solidcraft_doc::SolvedSketch, thin: &ThinWall) -> std::result::Result<Vec<solidcraft_geom::Region2>, String> {
    let length = |e: &str| s.doc.eval(&s.doc.with_design_unit(e, Kind::Length), Kind::Length).map_err(|e| e.to_string());
    let t = length(&thin.thickness)?;
    let (inside, outside) = match thin.side {
        ThinSide::Center => (t / 2.0, t / 2.0),
        ThinSide::Inside => (t, 0.0),
        ThinSide::Outside => (0.0, t),
        ThinSide::TwoSides => (t, length(thin.thickness2.as_deref().unwrap_or("0"))?),
    };
    let curves: Vec<Vec<solidcraft_geom::Seg2>> = thin
        .curves
        .iter()
        .filter_map(|id| ss.sketch.curve_index(id))
        .map(|ci| ss.sketch.exact_segs(ci).unwrap_or_else(|| ss.sketch.segs(ci)))
        .collect();
    solidcraft_sketch::thin_wall_regions(&curves, inside, outside).map_err(|e| format!("thin wall: {e}"))
}

/// An extrude's extent from its parameters: distance (or through all, or to an object), the
/// side(s), a second distance, a start offset, a taper, and from an object.
pub(super) fn extent_param(s: &Session, p: &Value, cmd: &str) -> Result<Extent> {
    let through_all = bool_(p, "through_all").unwrap_or(false) || str_(p, "extent").is_some_and(|e| e.eq_ignore_ascii_case("through_all"));
    let to_object = p.get("to").is_some();
    if to_object && p.get("to").and_then(vec3).is_none() {
        return Err(bad(cmd, "`to` must be a point [x, y, z] on the face, vertex or point to extrude to"));
    }
    if p.get("from").is_some() && p.get("from").and_then(vec3).is_none() {
        return Err(bad(cmd, "`from` must be a point [x, y, z] on the face or point to start from"));
    }
    let distance = if through_all || to_object { expr(p, "distance").unwrap_or_else(|| "0".into()) } else { req_expr(cmd, p, "distance")? };
    check_expr(s, &distance, Kind::Length, cmd, "distance")?;
    let taper = expr(p, "taper");
    if let Some(t) = &taper {
        check_expr(s, t, Kind::Angle, cmd, "taper")?;
    }
    let direction = match str_(p, "direction").map(str::to_ascii_lowercase).as_deref() {
        None | Some("positive") | Some("one_side") => Direction::Positive,
        Some("negative") | Some("reverse") | Some("flip") => Direction::Negative,
        Some("symmetric") => Direction::Symmetric,
        Some(o) => return Err(bad(cmd, format!("unknown direction `{o}`"))),
    };
    let distance2 = expr(p, "distance2");
    if let Some(d) = &distance2 {
        check_expr(s, d, Kind::Length, cmd, "distance2")?;
    }
    let start_offset = expr(p, "start_offset");
    if let Some(d) = &start_offset {
        check_expr(s, d, Kind::Length, cmd, "start_offset")?;
    }
    let convention = match p.get("convention") {
        // Surface Extrude shares Extent but retains its existing per-side distance semantics.
        None if cmd == "surface.extrude" => solidcraft_doc::ExtrudeConvention::LegacyPerSide,
        None => solidcraft_doc::ExtrudeConvention::TotalDistance,
        Some(value) => serde_json::from_value(value.clone()).map_err(|_| bad(cmd, "`convention` must be total_distance or legacy_per_side"))?,
    };
    Ok(Extent {
        distance,
        convention,
        direction,
        distance2,
        start_offset,
        through_all,
        taper,
        to: p.get("to").and_then(vec3).map(FacePick),
        to_offset: expr(p, "to_offset"),
        from: p.get("from").and_then(vec3).map(FacePick),
    })
}

fn revolve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.revolve";
    // A planar body face revolves like a profile (its edges projected into a sketch on it).
    let (sketch, face_profile) = match p.get("face").and_then(vec3) {
        Some(fp) => {
            let (id, inside, _) = super::face::sketch_of_face(s, fp, cmd, str_(p, "face_body"))?;
            s.active_sketch = None;
            (id, Some(ProfileSel::Points { points: vec![inside] }))
        }
        None => (feature_sketch(s, p, cmd)?, None),
    };
    let axis = axis_param(p, cmd)?;
    let angle = expr(p, "angle").unwrap_or_else(|| "360 deg".into());
    check_expr(s, &angle, Kind::Angle, cmd, "angle")?;
    let profiles = match face_profile {
        Some(f) => f,
        None => profiles(p, cmd)?,
    };
    let angle2 = expr(p, "angle2");
    if let Some(a) = &angle2 {
        check_expr(s, a, Kind::Angle, cmd, "angle2")?;
    }
    let to = match p.get("to") {
        Some(v) => Some(vec3(v).ok_or_else(|| bad(cmd, "`to` must be a point [x, y, z] to turn to"))?),
        None => None,
    };
    let symmetric =
        matches!(str_(p, "direction").map(str::to_ascii_lowercase).as_deref(), Some("symmetric")) || bool_(p, "symmetric").unwrap_or(false);
    let convention = match p.get("convention") {
        None => solidcraft_doc::RevolveConvention::SketchNormal,
        Some(value) => serde_json::from_value(value.clone()).map_err(|_| bad(cmd, "`convention` must be sketch_normal or legacy_axis_start"))?,
    };
    let kind = FeatureKind::Revolve {
        sketch: SketchId(sketch),
        profiles,
        axis,
        angle,
        operation: operation(p, cmd)?,
        targets: string_list(p, "targets").into_iter().map(BodyName).collect(),
        angle2,
        symmetric,
        convention,
        to: to.map(FacePick),
    };
    add_feature(s, p, kind)
}

/// A revolve axis from its parameter: a sketch line id, the sketch's x or y axis, a world axis,
/// or {origin, dir}.
pub(super) fn axis_param(p: &Value, cmd: &str) -> Result<AxisRef> {
    if let Some(o) = p.get("axis").filter(|v| v.is_object()) {
        let origin = o.get("origin").and_then(vec3).unwrap_or(Vec3::ZERO);
        let dir = o.get("dir").and_then(vec3).ok_or_else(|| bad(cmd, "axis needs `dir`"))?;
        return Ok(AxisRef::Line { origin: Position(origin), dir: Dir(dir) });
    }
    match str_(p, "axis") {
        Some(a @ ("X" | "Y" | "Z")) => Ok(AxisRef::World { axis: a.into() }),
        Some(a @ ("x" | "y")) => Ok(AxisRef::SketchAxis { axis: a.into() }),
        Some(id) => Ok(AxisRef::SketchLine { curve: id.into() }),
        None => Err(bad(cmd, "`axis` must be a sketch line id, x, y (sketch axes), X, Y, Z or {origin, dir}")),
    }
}

fn radius_expr(s: &Session, p: &Value, cmd: &str) -> Result<Expr> {
    match (expr(p, "radius"), expr(p, "diameter")) {
        (Some(r), _) => Ok(r),
        (None, Some(d)) => {
            let [d] = length_operands(s, [(&d, &typed_length_unit(s))]);
            Ok(format!("({d}) / 2").into())
        }
        _ => Err(bad(cmd, "needs `radius` or `diameter`")),
    }
}

/// The unit a bare length typed into a command means: the design's units. (A point's bare
/// coordinates, and a bare number already stored in a feature, are millimetres.)
pub(super) fn typed_length_unit(s: &Session) -> String {
    solidcraft_doc::expr::length_unit_or_mm(&s.doc.units).to_string()
}

/// The lengths a command works one expression out from (a radius from a diameter, a pitch from
/// a height), each with the unit a bare number means where it came from. Bare numbers get that
/// unit written in, so the expression keeps the meaning they had: inside a larger expression a
/// bare number is read in the design's units of the moment, and changing them would resize the
/// part (#716). Next to a unit-less expression that is not a bare number (`2 * 5`, a unit-less
/// parameter), which is read in the design's units itself, they stay bare and are read with it,
/// as before.
pub(super) fn length_operands<const N: usize>(s: &Session, operands: [(&str, &str); N]) -> [String; N] {
    use solidcraft_doc::expr::is_bare_number;
    let follows = operands.iter().any(|(e, _)| !is_bare_number(e) && s.doc.eval(e, Kind::Unitless).is_ok());
    operands.map(|(e, unit)| if !follows && is_bare_number(e) { format!("{} {unit}", e.trim()) } else { e.to_string() })
}

/// A placement point: `[x, y, z]` (or `{x, y, z}`) of numbers (mm) or length expressions;
/// the origin when it is not given. Anything else is refused (never read as the origin).
pub(super) fn point_param(s: &Session, p: &Value, key: &str, cmd: &str) -> Result<Option<PointExpr>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    let want = || bad(cmd, format!("`{key}` must be a point [x, y, z] of numbers or length expressions"));
    let coords: Vec<&Value> = match v {
        Value::Array(a) if a.len() == 3 => a.iter().collect(),
        Value::Object(o) => ["x", "y", "z"].iter().map(|k| o.get(*k)).collect::<Option<Vec<_>>>().ok_or_else(want)?,
        _ => return Err(want()),
    };
    let mut out = PointExpr::default();
    for (slot, (c, axis)) in out.iter_mut().zip(coords.into_iter().zip(["x", "y", "z"])) {
        let e = expr(&json!({ "v": c }), "v").ok_or_else(want)?;
        let x = s.doc.eval(&e, Kind::Length).map_err(|err| bad(cmd, format!("{key} {axis}: {err}")))?;
        if !(x.is_finite() && x.abs() < 1e9) {
            return Err(bad(cmd, format!("{key} {axis} is out of range")));
        }
        *slot = e;
    }
    Ok(Some(out))
}

/// An axis direction: X|Y|Z or a non-zero `[x, y, z]`; `Z` when it is not given.
pub(super) fn axis_dir_param(p: &Value, cmd: &str) -> Result<Vec3> {
    match p.get("axis") {
        None => Ok(Vec3::Z),
        Some(Value::String(a)) => match a.trim().to_ascii_uppercase().as_str() {
            "X" => Ok(Vec3::X),
            "Y" => Ok(Vec3::Y),
            "Z" => Ok(Vec3::Z),
            _ => Err(bad(cmd, "`axis` must be X, Y, Z or [x, y, z]")),
        },
        Some(v) => vec3(v).and_then(|a| a.normalized()).ok_or_else(|| bad(cmd, "`axis` must be X, Y, Z or a non-zero [x, y, z]")),
    }
}

fn prim_box(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.box";
    let (l, w, h) = (req_expr(cmd, p, "length")?, req_expr(cmd, p, "width")?, req_expr(cmd, p, "height")?);
    for (e, n) in [(&l, "length"), (&w, "width"), (&h, "height")] {
        check_expr(s, e, Kind::Length, cmd, n)?;
    }
    let corner = match (point_param(s, p, "corner", cmd)?, point_param(s, p, "center", cmd)?) {
        (Some(c), _) => c,
        // The corner half a size back from the centre (numbers stay numbers, in mm). The centre's
        // bare numbers are millimetres, the sizes' the design's units.
        (None, Some([cx, cy, cz])) => {
            let unit = typed_length_unit(s);
            let mm_per_unit = solidcraft_doc::expr::unit_info(&unit).map_or(1.0, |(_, k)| k);
            let back = |c: Expr, size: &Expr| -> Expr {
                match (c.parse::<f64>(), size.parse::<f64>()) {
                    (Ok(a), Ok(b)) => format!("{}", a - b * mm_per_unit / 2.0).into(),
                    _ => {
                        let [c, size] = length_operands(s, [(&c, "mm"), (size, &unit)]);
                        format!("({c}) - ({size}) / 2").into()
                    }
                }
            };
            [back(cx, &l), back(cy, &w), back(cz, &h)]
        }
        _ => point_expr(Vec3::ZERO),
    };
    add_feature(s, p, FeatureKind::Box { corner, length: l, width: w, height: h, operation: operation(p, cmd)? })
}

fn prim_cylinder(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.cylinder";
    let radius = radius_expr(s, p, cmd)?;
    let height = req_expr(cmd, p, "height")?;
    check_expr(s, &radius, Kind::Length, cmd, "radius")?;
    check_expr(s, &height, Kind::Length, cmd, "height")?;
    let base = point_param(s, p, "base", cmd)?.unwrap_or_else(|| point_expr(Vec3::ZERO));
    let axis = axis_dir_param(p, cmd)?;
    add_feature(s, p, FeatureKind::Cylinder { base, axis: Dir(axis), radius, height, operation: operation(p, cmd)? })
}

fn prim_sphere(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.sphere";
    let radius = radius_expr(s, p, cmd)?;
    check_expr(s, &radius, Kind::Length, cmd, "radius")?;
    let center = point_param(s, p, "center", cmd)?.unwrap_or_else(|| point_expr(Vec3::ZERO));
    add_feature(s, p, FeatureKind::Sphere { center, radius, operation: operation(p, cmd)? })
}

fn prim_torus(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.torus";
    let (major, minor) = (req_expr(cmd, p, "major")?, req_expr(cmd, p, "minor")?);
    check_expr(s, &major, Kind::Length, cmd, "major")?;
    check_expr(s, &minor, Kind::Length, cmd, "minor")?;
    let center = point_param(s, p, "center", cmd)?.unwrap_or_else(|| point_expr(Vec3::ZERO));
    add_feature(s, p, FeatureKind::Torus { center, major, minor, operation: operation(p, cmd)? })
}

/// Edge references → points on the edges.
fn edge_points(s: &Session, p: &Value, cmd: &str) -> Result<Vec<Vec3>> {
    let a = p.get("edges").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`edges` must be a list"))?;
    if a.is_empty() || a.len() > 1000 {
        return Err(bad(cmd, "select 1…1000 edges"));
    }
    let st = s.model.state();
    // A picked point must lie on an edge (within the distance the kernel re-finds an edge from)
    // of the body the feature names, or of any body: a pick far from every edge would round
    // whichever edge is nearest (#783). Each body's edges are worked out once.
    let named = str_(p, "body").filter(|n| st.body(n).is_some());
    let mut candidates: Option<Vec<(f64, Vec<solidcraft_kernel::EdgeInfo>)>> = None;
    let mut out = Vec::new();
    for e in a {
        if let Some(pt) = vec3(e).or_else(|| e.get("point").and_then(vec3)) {
            let bodies = candidates.get_or_insert_with(|| {
                // Mesh bodies have no edges to pick: rounding one is refused later, saying why.
                st.bodies
                    .iter()
                    .filter(|b| named.is_none_or(|n| b.name == n) && !b.body.is_mesh())
                    .filter_map(|b| {
                        let size = b.body.size();
                        b.body.edges((size * 2e-3).max(1e-3)).ok().map(|edges| (size, edges))
                    })
                    .collect()
            });
            let mut nearest = f64::INFINITY;
            let on_edge = bodies.iter().any(|(size, edges)| {
                let d = edges
                    .iter()
                    .flat_map(|e| e.points.windows(2))
                    .filter_map(|w| Some(pt.dist_to_segment(*w.first()?, *w.get(1)?)))
                    .fold(f64::INFINITY, f64::min);
                nearest = nearest.min(d);
                d <= size * 0.05 + 1e-3
            });
            if !on_edge && !bodies.is_empty() {
                let far = if nearest.is_finite() { format!("the nearest edge is {nearest:.3} mm away") } else { "there is no edge".to_string() };
                return Err(bad(cmd, format!("no edge at [{}, {}, {}]: {far}", pt.x, pt.y, pt.z)));
            }
            out.push(pt);
            continue;
        }
        let body = e.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "an edge is a point [x,y,z], {point} or {body, index}"))?;
        let idx = e.get("index").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "edge `index` must be a number"))? as usize;
        let b = st.body(body).ok_or_else(|| bad(cmd, format!("no body `{body}`")))?;
        let tol = (b.body.size() * 2e-3).max(1e-3);
        let edges = b.body.edges(tol)?;
        out.push(edges.get(idx).ok_or_else(|| bad(cmd, format!("{body} has no edge {idx}")))?.mid);
    }
    Ok(out)
}

/// Press Pull: a face moves along its normal (an extrude that joins outward or cuts inward); an
/// edge is rounded (a fillet).
fn press_pull(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.press_pull";
    let distance = req_expr(cmd, p, "distance")?;
    if p.get("edges").is_some() {
        return fillet(s, &json!({"edges": p.get("edges"), "radius": distance}));
    }
    let face = p.get("face").ok_or_else(|| bad(cmd, "needs `face` or `edges`"))?;
    let d = s.doc.eval(&distance, Kind::Length).map_err(|e| bad(cmd, format!("distance: {e}")))?;
    if d.abs() < 1e-9 {
        return Err(bad(cmd, "the distance must not be zero"));
    }
    // A fillet's or chamfer's face: its size changes by the distance (the feature is edited).
    if let Some(fp) = vec3(face)
        && super::face::face_is_planar(s, fp) == Some(false)
    {
        let name = solidcraft_doc::naming::face_names_at(&s.model.state(), &[fp]).into_iter().next().unwrap_or_default();
        let fid = name.strip_prefix('F').and_then(|r| r.split(':').next()).and_then(|x| x.parse::<u64>().ok());
        if name.contains(":blend") {
            let f = fid.and_then(|id| s.doc.feature(id)).cloned().ok_or_else(|| bad(cmd, "the blend's feature is gone"))?;
            let (key, old) = match &f.kind {
                FeatureKind::Fillet { radius, .. } => ("radius", radius.clone()),
                FeatureKind::Chamfer { distance, .. } => ("distance", distance.clone()),
                _ => return Err(bad(cmd, "that face is not a fillet's or a chamfer's")),
            };
            // A bare number stored in the feature is millimetres; the distance's, the design's units.
            let [old, more] = length_operands(s, [(&old, "mm"), (&distance, &typed_length_unit(s))]);
            let new = format!("({old}) + ({more})");
            let r = super::find_command("timeline.edit").ok_or_else(|| bad(cmd, "timeline.edit"))?;
            let out = (r.run)(s, &json!({"feature": f.id, "set": {key: new}}))?;
            return Ok(json!({"feature": f.id, "edited": f.name, key: new, "recomputed": out.get("recomputed"), "restored": out.get("restored")}));
        }
        // Another curved face moves along its normal.
        let r = super::find_command("solid.offset_face").ok_or_else(|| bad(cmd, "solid.offset_face"))?;
        return (r.run)(s, &json!({"faces": [face], "distance": distance}));
    }
    // Pushed in: the extrude cuts the other way by as much (a bare number stays one, so it keeps
    // being read as a bare number typed into the extrude).
    let flipped = match distance.trim().parse::<f64>() {
        Ok(v) if solidcraft_doc::expr::is_bare_number(&distance) => Expr::new(format!("{}", -v)),
        _ => Expr::new(format!("-({distance})")),
    };
    let (dist, dir, op) = if d > 0.0 { (distance, "positive", "join") } else { (flipped, "negative", "cut") };
    extrude(s, &json!({"face": face, "distance": dist, "direction": dir, "operation": op}))
}

fn fillet(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.fillet";
    let edges = edge_points(s, p, cmd)?;
    let radius = req_expr(cmd, p, "radius")?;
    check_expr(s, &radius, Kind::Length, cmd, "radius")?;
    let style = match str_(p, "type").map(str::to_ascii_lowercase).as_deref() {
        None | Some("constant") | Some("rolling_ball") => solidcraft_doc::FilletStyle::Constant,
        Some("chord") | Some("chord_length") => solidcraft_doc::FilletStyle::Chord,
        Some("curvature") | Some("g2") => solidcraft_doc::FilletStyle::Curvature,
        Some("variable") => {
            let radius2 = req_expr(cmd, p, "radius2")?;
            check_expr(s, &radius2, Kind::Length, cmd, "radius2")?;
            let start =
                p.get("start").and_then(vec3).ok_or_else(|| bad(cmd, "a variable fillet needs `start`: [x,y,z] near the end that takes `radius`"))?;
            let mut mid = Vec::new();
            for m in p.get("mid").and_then(Value::as_array).into_iter().flatten().take(100) {
                let (Some(t), Some(r)) = (m.get(0).and_then(Value::as_f64), m.get(1)) else {
                    return Err(bad(cmd, "`mid` entries are [position, radius]"));
                };
                if !(t.is_finite() && t > 0.0 && t < 1.0) {
                    return Err(bad(cmd, "`mid` positions lie between 0 and 1 (from the start end)"));
                }
                let r = match r {
                    Value::String(x) => x.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => return Err(bad(cmd, "a `mid` radius is a number or an expression")),
                };
                check_expr(s, &r, Kind::Length, cmd, "mid radius")?;
                mid.push((t, r.into()));
            }
            let smooth = p.get("smooth").and_then(Value::as_bool).unwrap_or(false);
            solidcraft_doc::FilletStyle::Variable { radius2, start: Position(start), mid, smooth }
        }
        Some("spline") => {
            let start = p
                .get("start")
                .and_then(vec3)
                .ok_or_else(|| bad(cmd, "a spline fillet needs `start`: [x,y,z] near the end its positions start from"))?;
            let knots: Vec<f64> = p.get("knots").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).collect();
            let mut radii = Vec::new();
            for r in p.get("radii").and_then(Value::as_array).into_iter().flatten().take(100) {
                let r = match r {
                    Value::String(x) => x.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => return Err(bad(cmd, "a control radius is a number or an expression")),
                };
                check_expr(s, &r, Kind::Length, cmd, "control radius")?;
                radii.push(Expr::from(r));
            }
            // All control radii (`radius` and `radii`) plus the degree (1…3) plus one.
            if radii.is_empty() || !(radii.len() + 3..=radii.len() + 5).contains(&knots.len()) {
                return Err(bad(
                    cmd,
                    "a spline fillet needs `radii` (the control radii after `radius`) and `knots` (as many as all control radii, the degree 1…3 and one more)",
                ));
            }
            solidcraft_doc::FilletStyle::Spline { start: Position(start), knots, radii }
        }
        Some(o) => return Err(bad(cmd, format!("unknown fillet type `{o}` (constant, chord, variable, spline or curvature)"))),
    };
    let fit = match str_(p, "fit").map(str::to_ascii_lowercase).as_deref() {
        None | Some("exact") => solidcraft_doc::FilletFit::Exact,
        Some("reduce") => solidcraft_doc::FilletFit::Reduce,
        Some("taper") => solidcraft_doc::FilletFit::Taper,
        Some(o) => return Err(bad(cmd, format!("unknown fit `{o}` (exact, reduce or taper)"))),
    };
    if !fit.is_exact() && !style.is_constant() {
        return Err(bad(cmd, "`fit: reduce` and `fit: taper` take a constant radius"));
    }
    // What fills the corners where three blended edges meet.
    let corner = match str_(p, "corner").map(str::to_ascii_lowercase).as_deref() {
        None | Some("rolling") => solidcraft_doc::FilletCorner::Rolling,
        Some("g2") => solidcraft_doc::FilletCorner::G2,
        Some(o) => return Err(bad(cmd, format!("unknown corner `{o}` (rolling or g2)"))),
    };
    if !corner.is_rolling() && (!style.is_constant() || !fit.is_exact()) {
        return Err(bad(cmd, "`corner: g2` is built for a constant-radius fillet (`type: constant`, `fit: exact`) only"));
    }
    let mut out = add_feature(
        s,
        p,
        FeatureKind::Fillet {
            edges: edges.into_iter().map(Position).collect(),
            radius,
            body: str_(p, "body").map(BodyName::from),
            style,
            fit,
            corner,
        },
    )?;
    // The radii the fillet used, when `fit: reduce` reduced it.
    if let Some(id) = out.get("feature").and_then(Value::as_u64)
        && let Some(state) = s.model.result(id).map(|res| &res.state)
        && let Some(r) = state.fitted_radius(id)
    {
        out["radius_applied"] = serde_json::json!(r);
        let each: Vec<Value> =
            state.fitted_edges(id).unwrap_or_default().into_iter().map(|(p, r)| serde_json::json!({"edge": [p.x, p.y, p.z], "radius": r})).collect();
        out["radii_applied"] = Value::Array(each);
    }
    Ok(out)
}

fn chamfer(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.chamfer";
    let edges = edge_points(s, p, cmd)?;
    let distance = req_expr(cmd, p, "distance")?;
    check_expr(s, &distance, Kind::Length, cmd, "distance")?;
    let (distance2, angle) = (expr(p, "distance2"), expr(p, "angle"));
    if let Some(d) = &distance2 {
        check_expr(s, d, Kind::Length, cmd, "distance2")?;
    }
    if let Some(a) = &angle {
        check_expr(s, a, Kind::Angle, cmd, "angle")?;
    }
    if distance2.is_some() && angle.is_some() {
        return Err(bad(cmd, "give `distance2` or `angle`, not both"));
    }
    let flip = bool_(p, "flip").unwrap_or(false);
    add_feature(
        s,
        p,
        FeatureKind::Chamfer {
            edges: edges.into_iter().map(Position).collect(),
            distance,
            distance2,
            angle,
            flip,
            body: str_(p, "body").map(BodyName::from),
        },
    )
}

fn combine(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.combine";
    let target = str_(p, "target").ok_or_else(|| bad(cmd, "`target` must be a body name"))?.to_string();
    let tools = string_list(p, "tools");
    if tools.is_empty() {
        return Err(bad(cmd, "`tools` must list bodies"));
    }
    let op = match str_(p, "operation") {
        None => Operation::Join,
        Some(o) => Operation::parse(o).filter(|o| *o != Operation::NewBody).ok_or_else(|| bad(cmd, "operation must be join, cut or intersect"))?,
    };
    let mut out = add_feature(
        s,
        p,
        FeatureKind::Combine {
            target: target.clone().into(),
            tools: tools.into_iter().map(BodyName).collect(),
            operation: op,
            keep_tools: bool_(p, "keep_tools").unwrap_or(false),
        },
    )?;
    // New Component: the result goes into a component of its own.
    if bool_(p, "new_component").unwrap_or(false) {
        let name = str_(p, "component_name").map(str::to_string).unwrap_or_else(|| format!("{target} Combined"));
        let c = s.execute("component.create", &json!({"name": name, "activate": false}))?;
        super::component::transfer_constructor_result(s, &json!({"bodies": [target], "component": c["component"]}))?;
        out["component"] = c["component"].clone();
    }
    Ok(out)
}

fn move_bodies(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.move";
    let bodies = string_list(p, "bodies");
    if bodies.is_empty() {
        return Err(bad(cmd, "`bodies` must list bodies"));
    }
    let t = match p.get("translate") {
        Some(Value::Array(a)) if a.len() == 3 => {
            let mut out: [Expr; 3] = Default::default();
            for (i, v) in a.iter().enumerate() {
                let e = expr(&json!({"v": v}), "v").ok_or_else(|| bad(cmd, "translate entries must be numbers or expressions"))?;
                check_expr(s, &e, Kind::Length, cmd, "translate")?;
                if let Some(slot) = out.get_mut(i) {
                    *slot = e;
                }
            }
            out
        }
        None => ["0".into(), "0".into(), "0".into()],
        _ => return Err(bad(cmd, "`translate` must be [x, y, z]")),
    };
    let angle = expr(p, "angle");
    if let Some(a) = &angle {
        check_expr(s, a, Kind::Angle, cmd, "angle")?;
    }
    add_feature(
        s,
        p,
        FeatureKind::Move {
            bodies: bodies.into_iter().map(BodyName).collect(),
            translate: t,
            rotate_axis: p.get("axis").and_then(vec3).map(Dir),
            angle,
            copy: bool_(p, "copy").unwrap_or(false),
        },
    )
}

fn source_features(s: &Session, p: &Value, cmd: &str) -> Result<Vec<String>> {
    let names = string_list(p, "features");
    if names.is_empty() {
        return Err(bad(cmd, "`features` must list features to copy"));
    }
    for n in &names {
        if s.doc.find_feature(n).is_none() {
            return Err(bad(cmd, format!("no feature `{n}`")));
        }
    }
    Ok(names)
}

fn axis_line(p: &Value, cmd: &str) -> Result<(Vec3, Vec3)> {
    match p.get("axis") {
        Some(Value::String(a)) => match a.to_ascii_uppercase().as_str() {
            "X" => Ok((Vec3::ZERO, Vec3::X)),
            "Y" => Ok((Vec3::ZERO, Vec3::Y)),
            "Z" => Ok((Vec3::ZERO, Vec3::Z)),
            _ => Err(bad(cmd, "axis must be X, Y, Z or {origin, dir}")),
        },
        Some(o @ Value::Object(_)) => {
            let origin = match o.get("origin") {
                None => Vec3::ZERO,
                Some(v) => vec3(v).ok_or_else(|| {
                    bad(cmd, "axis.origin must contain three finite numeric coordinates in millimetres; expressions are not supported")
                })?,
            };
            let dir = o.get("dir").and_then(vec3).and_then(|d| d.normalized()).ok_or_else(|| bad(cmd, "axis needs a non-zero `dir`"))?;
            Ok((origin, dir))
        }
        _ => Err(bad(cmd, "`axis` is required")),
    }
}

/// A pattern feature, or for `components`, new occurrences of them at each placement.
fn finish_pattern(s: &mut Session, p: &Value, features: Vec<String>, pattern: solidcraft_doc::PatternKind, bodies: Vec<String>) -> Result<Value> {
    let comps = string_list(p, "components");
    if comps.is_empty() {
        return add_feature(
            s,
            p,
            FeatureKind::Pattern {
                features: features.into_iter().map(FeatureName).collect(),
                pattern,
                bodies: bodies.into_iter().map(BodyName).collect(),
            },
        );
    }
    let cmd = "pattern (components)";
    let mats = solidcraft_doc::pattern_matrices(&s.doc, &s.model.state(), &pattern)?;
    let mut made = Vec::new();
    for c in &comps {
        let id = s.doc.find_component(c).filter(|id| *id != 0).ok_or_else(|| bad(cmd, format!("no component `{c}`")))?;
        let src = s.doc.occurrence_of(id).cloned().ok_or_else(|| bad(cmd, format!("component `{c}` has no occurrence")))?;
        for m in &mats {
            made.push(s.doc_mut().add_occurrence(id, src.parent, solidcraft_doc::mat_mul(m, &src.transform))?);
        }
    }
    Ok(json!({"occurrences": made}))
}

/// What a pattern copies: features (replayed), bodies (`bodies: [names]`), or components
/// (`components: [names]`, placed again as occurrences).
fn pattern_sources(s: &Session, p: &Value, cmd: &str) -> Result<(Vec<String>, Vec<String>)> {
    if !string_list(p, "components").is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let bodies = string_list(p, "bodies");
    if bodies.is_empty() {
        return Ok((source_features(s, p, cmd)?, Vec::new()));
    }
    let st = s.model.state();
    if let Some(b) = bodies.iter().find(|b| st.body(b).is_none()) {
        return Err(bad(cmd, format!("no body `{b}`")));
    }
    Ok((Vec::new(), bodies))
}

fn pattern_rect(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.pattern.rectangular";
    let (features, bodies) = pattern_sources(s, p, cmd)?;
    let dir1 = p.get("dir1").and_then(vec3).ok_or_else(|| bad(cmd, "`dir1` must be [x, y, z]"))?;
    let count1 = req_expr(cmd, p, "count1")?;
    let spacing1 = req_expr(cmd, p, "spacing1")?;
    check_expr(s, &count1, Kind::Unitless, cmd, "count1")?;
    check_expr(s, &spacing1, Kind::Length, cmd, "spacing1")?;
    let dir2 = p.get("dir2").and_then(vec3);
    let (count2, spacing2) = (expr(p, "count2"), expr(p, "spacing2"));
    if let Some(c) = &count2 {
        check_expr(s, c, Kind::Unitless, cmd, "count2")?;
    }
    if let Some(c) = &spacing2 {
        check_expr(s, c, Kind::Length, cmd, "spacing2")?;
    }
    let pattern = solidcraft_doc::PatternKind::Rectangular {
        dir1: Dir(dir1),
        count1,
        spacing1,
        dir2: dir2.map(Dir),
        count2,
        spacing2,
        alignment: super::alignment_param(p, "alignment", cmd)?,
        alignment2: super::alignment_param(p, "alignment2", cmd)?,
        reversed: super::reversed_param(p, "reversed", cmd)?,
        reversed2: super::reversed_param(p, "reversed2", cmd)?,
    };
    finish_pattern(s, p, features, pattern, bodies)
}

fn pattern_circ(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.pattern.circular";
    let (features, bodies) = pattern_sources(s, p, cmd)?;
    let (origin, axis) = axis_line(p, cmd)?;
    let count = req_expr(cmd, p, "count")?;
    check_expr(s, &count, Kind::Unitless, cmd, "count")?;
    let angle = expr(p, "angle").unwrap_or_else(|| "360 deg".into());
    check_expr(s, &angle, Kind::Angle, cmd, "angle")?;
    let pattern = solidcraft_doc::PatternKind::Circular {
        origin: Position(origin),
        axis: Dir(axis),
        count,
        angle,
        alignment: super::alignment_param(p, "alignment", cmd)?,
        reversed: super::reversed_param(p, "reversed", cmd)?,
    };
    finish_pattern(s, p, features, pattern, bodies)
}

fn pattern_path(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.pattern.path";
    let (features, bodies) = pattern_sources(s, p, cmd)?;
    let path_sketch = sketch_id(s, p.get("path_sketch").or_else(|| p.get("sketch")), cmd, "path_sketch")?;
    let path = string_list(p, "path");
    if path.is_empty() || path.len() > 1000 {
        return Err(bad(cmd, "`path` must list 1…1000 curve ids"));
    }
    let count = req_expr(cmd, p, "count")?;
    check_expr(s, &count, Kind::Unitless, cmd, "count")?;
    let (spacing, extent) = match (expr(p, "spacing"), expr(p, "distance")) {
        (Some(x), _) => (x, false),
        (None, Some(x)) => (x, true),
        (None, None) => return Err(bad(cmd, "give `spacing` (between instances) or `distance` (first to last)")),
    };
    check_expr(s, &spacing, Kind::Length, cmd, "spacing")?;
    let orient = match str_(p, "orientation") {
        None | Some("identical") => false,
        Some("path" | "path_direction" | "along_path") => true,
        Some(o) => return Err(bad(cmd, format!("unknown orientation `{o}` (identical or path)"))),
    };
    let flip = bool_(p, "flip").unwrap_or(false);
    let pattern = solidcraft_doc::PatternKind::Path { path_sketch: SketchId(path_sketch), path, count, spacing, extent, flip, orient };
    finish_pattern(s, p, features, pattern, bodies)
}

fn mirror(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.mirror";
    let bodies = string_list(p, "bodies");
    let features = if bodies.is_empty() { source_features(s, p, cmd)? } else { Vec::new() };
    let plane = plane_param(s, p.get("plane"), cmd)?;
    let combine = bool_(p, "combine").unwrap_or(false);
    let (bodies, placements) = placed_bodies(s, bodies, cmd)?;
    add_feature(
        s,
        p,
        FeatureKind::Mirror {
            features: features.into_iter().map(FeatureName).collect(),
            plane,
            bodies: bodies.into_iter().map(BodyName).collect(),
            combine,
            placements,
        },
    )
}

/// Bodies picked where the viewport shows them: each world name ("Body1", or "Body1 (Occ:2)"
/// for a further instance) becomes its component's body name, with the occurrence path of the
/// placement it was picked at. Names stay as they are (and no placement is kept) while every
/// component is in place, or when one of them is not a body.
fn placed_bodies(s: &Session, names: Vec<String>, cmd: &str) -> Result<(Vec<String>, Vec<Vec<u64>>)> {
    if names.is_empty() || !crate::frames::placed(s) {
        return Ok((names, Vec::new()));
    }
    let frames = s.doc.body_frames_with_source(&s.model.state());
    let mut picked = Vec::with_capacity(names.len());
    for name in &names {
        let Some((_, own, _, path, _)) = frames.iter().find(|(world, ..)| world == name) else { return Ok((names, Vec::new())) };
        if path.iter().any(|id| s.pending_moves.contains_key(id)) {
            return Err(bad(cmd, format!("`{name}` has a position that is not captured yet: Capture Position (component.capture_position) first")));
        }
        picked.push((own.clone(), path.clone()));
    }
    Ok(picked.into_iter().unzip())
}

/// The other name of an origin plane (`Top` for `XY`, `XY` for `Top`...), if `name` is one.
pub(super) fn origin_plane_alias(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_uppercase().as_str() {
        "XY" => Some("Top"),
        "TOP" => Some("XY"),
        "XZ" => Some("Front"),
        "FRONT" => Some("XZ"),
        "YZ" => Some("Right"),
        "RIGHT" => Some("YZ"),
        _ => None,
    }
}

/// A construction plane may not take an origin plane's name (`XY`, `Top`...): a plane
/// parameter could then mean either.
pub(super) fn check_plane_name(name: &str, cmd: &str) -> Result<()> {
    if solidcraft_geom::Plane::named(name.trim()).is_some() {
        return Err(bad(
            cmd,
            format!("`{}` is reserved for an origin plane (XY, XZ, YZ, Top, Front, Right); give the construction plane another name", name.trim()),
        ));
    }
    Ok(())
}

/// A plane reference from a parameter: an origin plane, a construction plane (by name or
/// feature id; kept by name, so the feature follows the plane when it changes), or an
/// explicit plane `{origin, x_dir, y_dir}` / `{origin, normal}`. Every command that takes a
/// plane reads it here.
pub(super) fn plane_param(s: &Session, v: Option<&Value>, cmd: &str) -> Result<solidcraft_doc::PlaneRef> {
    use solidcraft_doc::PlaneRef;
    const WANT: &str = "XY, XZ, YZ, a construction plane name or id, or {origin, normal} (origin coordinates may be length expressions)";
    let construction = |r: &str| match s.doc.find_feature(r) {
        Some(f) if matches!(f.kind, FeatureKind::ConstructionPlane { .. }) => Ok(PlaneRef::Construction { name: f.name.clone().into() }),
        Some(f) => Err(bad(cmd, format!("`{}` is not a construction plane (a plane is {WANT})", f.name))),
        None => Err(bad(cmd, format!("no plane `{r}` (a plane is {WANT})"))),
    };
    match v {
        Some(Value::String(n)) if solidcraft_geom::Plane::named(n.trim()).is_some() => {
            if let Some(f) = s.doc.find_feature(n.trim())
                && matches!(f.kind, FeatureKind::ConstructionPlane { .. })
            {
                return Err(bad(
                    cmd,
                    format!(
                        "plane `{}` is ambiguous: an origin-plane alias and a construction plane; use `{}` for the origin plane, construction plane feature id {}, or an explicit {{origin, normal}} plane",
                        n.trim(),
                        origin_plane_alias(n).unwrap_or("XY"),
                        f.id
                    ),
                ));
            }
            Ok(PlaneRef::Origin { name: n.trim().to_ascii_uppercase() })
        }
        Some(Value::String(n)) => construction(n.trim()),
        Some(Value::Number(id)) => match id.as_u64() {
            Some(id) => construction(&id.to_string()),
            None => Err(bad(cmd, format!("`plane` must be {WANT}"))),
        },
        Some(o @ Value::Object(_)) => {
            // The origin's coordinates are numbers or length expressions; expressions are kept,
            // so the plane follows its parameters.
            let origin = match o.get("origin") {
                None => point_expr(Vec3::ZERO),
                Some(_) => point_param(s, o, "origin", cmd)?.unwrap_or_else(|| point_expr(Vec3::ZERO)),
            };
            let at = |e: &str| s.doc.eval(e, Kind::Length).unwrap_or(0.0);
            let here = Vec3::new(at(&origin[0]), at(&origin[1]), at(&origin[2]));
            let pl = match (o.get("x_dir").and_then(vec3), o.get("y_dir").and_then(vec3), o.get("normal").and_then(vec3)) {
                (Some(x), Some(y), _) => solidcraft_geom::Plane::new(here, x, y),
                (_, _, Some(n)) => solidcraft_geom::Plane::from_normal(here, n),
                _ => None,
            };
            let plane = pl.ok_or_else(|| bad(cmd, "plane needs x_dir and y_dir, or normal"))?;
            if origin.iter().all(|e| solidcraft_doc::expr::is_literal(e)) {
                return Ok(PlaneRef::Custom { plane });
            }
            Ok(PlaneRef::Placed { origin, x_dir: plane.x, y_dir: plane.y })
        }
        _ => Err(bad(cmd, format!("`plane` must be {WANT}"))),
    }
}

fn plane_offset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.plane.offset";
    let d = req_expr(cmd, p, "offset")?;
    check_expr(s, &d, Kind::Length, cmd, "offset")?;
    // A planar body face: attached exactly, as a sketch on that face is, so the plane follows it.
    if let Some(face) = p.get("base").filter(|b| b.get("face").is_some()) {
        let base = super::sketch::face::capture(s, face, cmd)?;
        super::sketch::face::capture_positions(s, &base)?;
        let kind = FeatureKind::ConstructionPlane { plane: solidcraft_doc::PlaneRef::Offset { base: Box::new(base), distance: d } };
        // The capture is already in the authoring frame.
        return add_feature_in(s, p, kind, false);
    }
    let base = plane_param(s, p.get("base"), cmd)?;
    add_feature(s, p, FeatureKind::ConstructionPlane { plane: solidcraft_doc::PlaneRef::Offset { base: Box::new(base), distance: d } })
}

fn plane_angle(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.plane.at_angle";
    let base = plane_param(s, p.get("base"), cmd)?;
    let (axis_origin, axis_dir) = axis_line(p, cmd)?;
    let angle = req_expr(cmd, p, "angle")?;
    check_expr(s, &angle, Kind::Angle, cmd, "angle")?;
    add_feature(
        s,
        p,
        FeatureKind::ConstructionPlane { plane: solidcraft_doc::PlaneRef::AtAngle { base: Box::new(base), axis_origin, axis_dir, angle } },
    )
}

fn split_face(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.split_face";
    let faces = face_points(s, p, cmd)?;
    let body = str_(p, "body").map(str::to_string);
    let xy = || solidcraft_doc::PlaneRef::Origin { name: "XY".into() };
    let kind = if let Some(t) = p.get("tool").filter(|t| !t.is_null()) {
        let tb = t.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`tool.body` must name a body"))?.to_string();
        let point = t.get("point").and_then(crate::params::vec3).ok_or_else(|| bad(cmd, "`tool.point` must be [x, y, z] on the tool face"))?;
        FeatureKind::SplitFace {
            faces: faces.into_iter().map(FacePick).collect(),
            body: body.map(BodyName),
            plane: xy(),
            tool: Some(solidcraft_doc::FaceAt { body: BodyName(tb), point: Position(point) }),
            sketch: None,
        }
    } else if p.get("sketch").is_some_and(|v| !v.is_null()) {
        let sketch = sketch_id(s, p.get("sketch"), cmd, "sketch")?;
        FeatureKind::SplitFace {
            faces: faces.into_iter().map(FacePick).collect(),
            body: body.map(BodyName),
            plane: xy(),
            tool: None,
            sketch: Some(SketchId(sketch)),
        }
    } else {
        let plane = plane_param(s, p.get("plane"), cmd)?;
        FeatureKind::SplitFace { faces: faces.into_iter().map(FacePick).collect(), body: body.map(BodyName), plane, tool: None, sketch: None }
    };
    add_feature(s, p, kind)
}

fn split_body(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.split_body";
    let body = str_(p, "body").ok_or_else(|| bad(cmd, "`body` is required"))?.to_string();
    if s.model.state().body(&body).is_none() {
        return Err(bad(cmd, format!("no body `{body}`")));
    }
    // A straight sketch line as the tool: `sketch` and `line` (#1552).
    if p.get("sketch").is_some_and(|v| !v.is_null()) || p.get("line").is_some_and(|v| !v.is_null()) {
        let sketch = p.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`sketch` must be a sketch feature id"))?;
        let line = str_(p, "line").ok_or_else(|| bad(cmd, "`line` must name a line of the sketch"))?.to_string();
        let st = s.model.state();
        if st.sketch(sketch).is_none() {
            return Err(bad(cmd, format!("no sketch {sketch}")));
        }
        solidcraft_doc::sketch_line_plane(&st, sketch, &line).map_err(|e| bad(cmd, e.to_string()))?;
        return add_feature(
            s,
            p,
            FeatureKind::Split {
                body: BodyName(body),
                plane: solidcraft_doc::PlaneRef::Origin { name: "XY".into() },
                tool: None,
                sketch: Some(solidcraft_doc::SketchId(sketch)),
                line: Some(line),
            },
        );
    }
    // A face as the tool: `tool: {body, point}` (its surface extended); else a plane.
    if let Some(t) = p.get("tool").filter(|t| !t.is_null()) {
        let tb = t.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`tool.body` must name a body"))?.to_string();
        let point = t.get("point").and_then(crate::params::vec3).ok_or_else(|| bad(cmd, "`tool.point` must be [x, y, z] on the tool face"))?;
        let st = s.model.state();
        let tbody = st.body(&tb).ok_or_else(|| bad(cmd, format!("no body `{tb}`")))?;
        if solidcraft_doc::appearance::face_index_at(tbody, point).is_none() {
            return Err(bad(cmd, "`tool.point` is not on a face of the tool body"));
        }
        return add_feature(
            s,
            p,
            FeatureKind::Split {
                body: BodyName(body),
                plane: solidcraft_doc::PlaneRef::Origin { name: "XY".into() },
                tool: Some(solidcraft_doc::FaceAt { body: BodyName(tb), point: Position(point) }),
                sketch: None,
                line: None,
            },
        );
    }
    let plane = plane_param(s, p.get("plane"), cmd)?;
    add_feature(s, p, FeatureKind::Split { body: BodyName(body), plane, tool: None, sketch: None, line: None })
}

/// ISO 273 clearance hole diameters (close, normal, loose fit) for metric bolts.
fn clearance_diameter(size: &str, fit: &str) -> Option<f64> {
    const TABLE: [(f64, f64, f64, f64); 21] = [
        (1.6, 1.7, 1.8, 2.0),
        (2.0, 2.2, 2.4, 2.6),
        (2.5, 2.7, 2.9, 3.1),
        (3.0, 3.2, 3.4, 3.6),
        (4.0, 4.3, 4.5, 4.8),
        (5.0, 5.3, 5.5, 5.8),
        (6.0, 6.4, 6.6, 7.0),
        (8.0, 8.4, 9.0, 10.0),
        (10.0, 10.5, 11.0, 12.0),
        (12.0, 13.0, 13.5, 14.5),
        (14.0, 15.0, 15.5, 16.5),
        (16.0, 17.0, 17.5, 18.5),
        (20.0, 21.0, 22.0, 24.0),
        (24.0, 25.0, 26.0, 28.0),
        (30.0, 31.0, 33.0, 35.0),
        (36.0, 37.0, 39.0, 42.0),
        (42.0, 43.0, 45.0, 48.0),
        (48.0, 50.0, 52.0, 56.0),
        (56.0, 58.0, 62.0, 66.0),
        (60.0, 62.0, 66.0, 70.0),
        (64.0, 66.0, 70.0, 74.0),
    ];
    let t = size.trim().trim_start_matches(['M', 'm']);
    let d: f64 = t.split(['x', 'X']).next()?.trim().parse().ok()?;
    let row = TABLE.iter().find(|r| (r.0 - d).abs() < 1e-9)?;
    match fit {
        "close" | "fine" => Some(row.1),
        "normal" | "medium" => Some(row.2),
        "loose" | "coarse" => Some(row.3),
        _ => None,
    }
}

fn hole(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.hole";
    // At sketch points: the holes go perpendicular to the sketch and follow its points.
    let points = match (p.get("sketch"), p.get("points"), p.get("position")) {
        (Some(_), Some(_), _) | (Some(_), None, None) => {
            let sketch = feature_sketch(s, p, cmd)?;
            // No points named: every sketch point (standalone points and circle centres, not
            // line or arc ends, not the origin).
            let ids = if p.get("points").is_some() { string_list(p, "points") } else { sketch_hole_points(s, sketch) };
            if ids.is_empty() || ids.len() > 10_000 {
                return Err(bad(cmd, "`points` must list 1…10000 sketch point ids"));
            }
            let st = s.model.state();
            let ss = st.sketch(sketch).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?;
            for id in &ids {
                if !ss.sketch.points.iter().any(|q| &q.id == id) {
                    return Err(bad(cmd, format!("no point `{id}` in the sketch")));
                }
            }
            let n = ss.plane.normal();
            let first =
                ids.first().and_then(|id| ss.sketch.points.iter().find(|q| &q.id == id)).map(|q| ss.plane.to_world(q.pos)).unwrap_or_default();
            let flip = p.get("flip").and_then(Value::as_bool).unwrap_or(false);
            Some((solidcraft_doc::SketchPoints { sketch: SketchId(sketch), ids, flip }, first, -n))
        }
        _ => None,
    };
    let thread = str_(p, "thread").map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if let Some(t) = &thread
        && solidcraft_doc::parse_metric_thread(t).is_none()
    {
        return Err(bad(cmd, format!("unknown thread `{t}` (ISO metric, like M6 or M6x0.75)")));
    }
    // A position given as length expressions (#1678), say `["leg_h*0.4", "width/2", "t"]`: kept,
    // so the hole follows the parameters; drilled where they evaluate now.
    let at: Option<solidcraft_doc::PointExpr> = match p.get("position").and_then(Value::as_array) {
        Some(a) if points.is_none() && a.len() == 3 && a.iter().any(Value::is_string) => {
            let mut coords = Vec::with_capacity(3);
            for v in a {
                let e = match v {
                    Value::String(x) => x.trim().to_string(),
                    Value::Number(n) => n.to_string(),
                    _ => return Err(bad(cmd, "`position` must be [x, y, z]: numbers or length expressions")),
                };
                check_expr(s, &e, Kind::Length, cmd, "position")?;
                coords.push(solidcraft_doc::Expr::new(e));
            }
            let [x, y, z]: [solidcraft_doc::Expr; 3] = coords.try_into().map_err(|_| bad(cmd, "`position` must be [x, y, z]"))?;
            Some([x, y, z])
        }
        _ => None,
    };
    let at_value = match &at {
        Some([x, y, z]) => {
            let c = |e: &solidcraft_doc::Expr| s.doc.eval(e, Kind::Length).map_err(|e| bad(cmd, format!("`position`: {e}")));
            Some(Vec3::new(c(x)?, c(y)?, c(z)?)).filter(|v| v.is_finite())
        }
        None => None,
    };
    let position = match (&points, at_value.or_else(|| p.get("position").and_then(vec3))) {
        (Some((_, first, _)), None) => *first,
        (_, Some(x)) => x,
        (None, None) => return Err(bad(cmd, "`position` must be [x, y, z], numbers or length expressions (or give `sketch` and `points`)")),
    };
    let direction = match (p.get("direction").and_then(vec3), &points) {
        (None, Some((_, _, d))) => *d,
        (d, _) => match d {
            Some(d) => d.normalized().ok_or_else(|| bad(cmd, "`direction` must be non-zero"))?,
            None => {
                // Into the face the position lies on: against the outward normal of the face
                // nearest it (planar or curved), as it faces out of its own body.
                let st = s.model.state();
                solidcraft_doc::hole_direction_at(&st, position).ok_or_else(|| bad(cmd, "no face at `position`; give `direction`"))?
            }
        },
    };
    // The diameter: given, or the tap drill of the thread (major − pitch), or an ISO 273
    // clearance hole for a bolt size (close, normal or loose fit).
    let diameter = match (expr(p, "diameter"), &thread, str_(p, "clearance")) {
        (Some(d), _, _) => d,
        (None, _, Some(c)) => {
            let fit = str_(p, "fit").unwrap_or("normal").to_ascii_lowercase();
            let d = clearance_diameter(c, &fit).ok_or_else(|| bad(cmd, format!("no clearance hole for `{c}` ({fit} fit): M1.6 to M64")))?;
            Expr::new(format!("{d} mm"))
        }
        (None, Some(t), None) => {
            let (major, pitch) = solidcraft_doc::parse_metric_thread(t).ok_or_else(|| bad(cmd, "thread"))?;
            Expr::new(format!("{} mm", ((major - pitch) * 1000.0).round() / 1000.0))
        }
        (None, None, None) => return Err(bad(cmd, "needs `diameter`, `thread` (a tapped hole) or `clearance` (a bolt size)")),
    };
    check_expr(s, &diameter, Kind::Length, cmd, "diameter")?;
    let depth = expr(p, "depth");
    if let Some(d) = &depth {
        check_expr(s, d, Kind::Length, cmd, "depth")?;
    }
    let get = |k: &str| -> Result<Expr> {
        let e = req_expr(cmd, p, k)?;
        Ok(e)
    };
    let kind = match str_(p, "type").unwrap_or("simple") {
        "simple" => match expr(p, "tip_angle") {
            Some(t) if depth.is_some() => solidcraft_doc::HoleKind::Drilled { tip_angle: t },
            _ => solidcraft_doc::HoleKind::Simple,
        },
        "drilled" => solidcraft_doc::HoleKind::Drilled { tip_angle: expr(p, "tip_angle").unwrap_or_else(|| "118 deg".into()) },
        "counterbore" => solidcraft_doc::HoleKind::Counterbore { cb_diameter: get("cb_diameter")?, cb_depth: get("cb_depth")? },
        "countersink" => solidcraft_doc::HoleKind::Countersink {
            cs_diameter: get("cs_diameter")?,
            cs_angle: expr(p, "cs_angle").unwrap_or_else(|| "90 deg".into()),
        },
        other => return Err(bad(cmd, format!("unknown hole type `{other}`"))),
    };
    let to = match p.get("to") {
        Some(v) => Some(vec3(v).ok_or_else(|| bad(cmd, "`to` must be a point [x, y, z] on the face to drill to"))?),
        None => None,
    };
    // A hole picked on a body of another component (a moved one, shown where it is placed) cuts
    // that body: the pick is already in that body's frame, and the component's own bodies alone
    // would leave nothing to cut.
    let mut p = p.clone();
    if points.is_none()
        && p.get("participants").is_none()
        && let Some(body) = body_of_other_component_at(s, position)
    {
        p["participants"] = json!([body]);
    }
    add_feature(
        s,
        &p,
        FeatureKind::Hole {
            position: FacePick(position),
            at,
            direction: Dir(direction),
            diameter,
            depth,
            hole: kind,
            points: points.map(|x| x.0),
            thread,
            to: to.map(FacePick),
        },
    )
}

/// The body whose face is nearest `p` (within its pick tolerance), when it belongs to another
/// component than the active one.
fn body_of_other_component_at(s: &Session, p: Vec3) -> Option<String> {
    let st = s.model.state();
    let (_, body) = st
        .bodies
        .iter()
        .filter(|b| !b.body.is_mesh())
        .filter_map(|b| {
            let (d, _) = solidcraft_doc::appearance::nearest_face(b, p)?;
            (d <= solidcraft_doc::appearance::pick_tolerance(b)).then_some((d, b))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    (s.doc.model_body_component(body) != s.active_component).then(|| body.name.clone())
}

/// The points of a sketch a hole drills at when only the sketch is given.
fn sketch_hole_points(s: &Session, sketch: u64) -> Vec<String> {
    let st = s.model.state();
    let Some(ss) = st.sketch(sketch) else { return Vec::new() };
    let mut ends: Vec<usize> = Vec::new();
    for c in &ss.sketch.curves {
        match &c.kind {
            solidcraft_sketch::CurveKind::Circle { .. } => {}
            solidcraft_sketch::CurveKind::Arc { a, b, .. } | solidcraft_sketch::CurveKind::Line { a, b } => ends.extend([*a, *b]),
            solidcraft_sketch::CurveKind::Ellipse { m, .. } => ends.push(*m),
            solidcraft_sketch::CurveKind::Spline { pts, .. } => ends.extend(pts.iter().copied()),
            solidcraft_sketch::CurveKind::Conic { a, b, apex, .. } => ends.extend([*a, *b, *apex]),
        }
    }
    ss.sketch.points.iter().enumerate().filter(|(i, p)| p.id != "origin" && !ends.contains(i)).map(|(_, p)| p.id.clone()).collect()
}

fn face_points(s: &Session, p: &Value, cmd: &str) -> Result<Vec<Vec3>> {
    let a = p.get("faces").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`faces` must be a list of points or face names"))?;
    if a.is_empty() || a.len() > 1000 {
        return Err(bad(cmd, "select 1…1000 faces"));
    }
    a.iter().map(|v| super::face::face_ref(s, v, str_(p, "body"), cmd)).collect()
}

fn shell(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.shell";
    let faces = face_points(s, p, cmd)?;
    let thickness = req_expr(cmd, p, "thickness")?;
    check_expr(s, &thickness, Kind::Length, cmd, "thickness")?;
    let direction = str_(p, "direction").map(str::to_ascii_lowercase).unwrap_or_default();
    if !matches!(direction.as_str(), "" | "inside" | "outside" | "both") {
        return Err(bad(cmd, "`direction` must be inside, outside or both"));
    }
    add_feature(
        s,
        p,
        FeatureKind::Shell {
            faces: faces.into_iter().map(FacePick).collect(),
            thickness,
            body: str_(p, "body").map(BodyName::from),
            direction: if direction == "inside" { String::new() } else { direction },
            tangent_chain: bool_(p, "tangent_chain").unwrap_or(false),
        },
    )
}

fn draft(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.draft";
    let faces = face_points(s, p, cmd)?;
    let angle = req_expr(cmd, p, "angle")?;
    check_expr(s, &angle, Kind::Angle, cmd, "angle")?;
    let neutral = plane_param(s, p.get("neutral"), cmd)?;
    let (vals, _) = s.doc.param_values();
    let pl = s.doc.resolve_plane_in(&vals, &neutral, 0, Some(&s.model.state()))?;
    let pull = p.get("pull").and_then(vec3).unwrap_or_else(|| pl.normal());
    add_feature(
        s,
        p,
        FeatureKind::Draft {
            faces: faces.into_iter().map(FacePick).collect(),
            angle,
            neutral,
            pull: Dir(pull),
            body: str_(p, "body").map(BodyName::from),
        },
    )
}

pub(super) fn sketch_id(s: &Session, v: Option<&Value>, cmd: &str, what: &str) -> Result<u64> {
    let key = match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, format!("`{what}` must be a sketch id or name"))),
    };
    match s.doc.find_feature(&key) {
        Some(f) if matches!(f.kind, FeatureKind::Sketch { .. }) => Ok(f.id),
        _ => Err(bad(cmd, format!("no sketch `{key}`"))),
    }
}

fn sweep(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.sweep";
    let sketch = feature_sketch(s, p, cmd)?;
    let path_sketch = sketch_id(s, p.get("path_sketch"), cmd, "path_sketch")?;
    let path = string_list(p, "path");
    if path.is_empty() {
        return Err(bad(cmd, "`path` must list curve ids"));
    }
    let (orientation, twist, scale) = sweep_options(s, p, cmd)?;
    let (rails, rail_fit) = sweep_rails(s, p, cmd, path_sketch)?;
    add_feature(
        s,
        p,
        FeatureKind::Sweep {
            sketch: SketchId(sketch),
            profiles: profiles(p, cmd)?,
            path_sketch: SketchId(path_sketch),
            path,
            orientation,
            twist,
            scale,
            rails,
            rail_fit,
            operation: operation(p, cmd)?,
            targets: string_list(p, "targets").into_iter().map(BodyName).collect(),
        },
    )
}

/// A sweep's `orientation`, `twist` and `scale` (solid.sweep and surface.sweep), checked: an
/// orientation the sweep knows, a twist within four turns, an end scale within 0.01…100.
pub(super) fn sweep_options(s: &Session, p: &Value, cmd: &str) -> Result<(SweepOrientation, Option<Expr>, Option<Expr>)> {
    let orientation = match p.get("orientation") {
        None | Some(Value::Null) => SweepOrientation::Perpendicular,
        Some(Value::String(o)) if o.eq_ignore_ascii_case("perpendicular") => SweepOrientation::Perpendicular,
        Some(Value::String(o)) if o.eq_ignore_ascii_case("parallel") => SweepOrientation::Parallel,
        Some(v) => {
            let d = vec3(v)
                .or_else(|| v.get("direction").and_then(vec3))
                .ok_or_else(|| bad(cmd, "`orientation` must be perpendicular, parallel or a direction [x, y, z]"))?;
            if !(d.len() > 1e-9) {
                return Err(bad(cmd, "the `orientation` direction must not be zero"));
            }
            SweepOrientation::Direction(Dir(d))
        }
    };
    let twist = match p.get("twist") {
        None | Some(Value::Null) => None,
        Some(_) => {
            let t = expr(p, "twist").ok_or_else(|| bad(cmd, "`twist` must be an angle or an expression"))?;
            let v = s.doc.eval(&t, Kind::Angle).map_err(|e| bad(cmd, format!("twist: {e}")))?;
            if !(v.is_finite() && v.abs() <= solidcraft_kernel::SweepControl::MAX_TWIST) {
                return Err(bad(cmd, "`twist` must be within four turns (1440 deg)"));
            }
            Some(t)
        }
    };
    let scale = match p.get("scale") {
        None | Some(Value::Null) => None,
        Some(_) => {
            let k = expr(p, "scale").ok_or_else(|| bad(cmd, "`scale` must be a number or an expression"))?;
            let v = s.doc.eval(&k, Kind::Unitless).map_err(|e| bad(cmd, format!("scale: {e}")))?;
            let range = solidcraft_kernel::SweepControl::MIN_SCALE..=solidcraft_kernel::SweepControl::MAX_SCALE;
            if !(v.is_finite() && range.contains(&v)) {
                return Err(bad(cmd, "`scale` (the end scale) must be between 0.01 and 100"));
            }
            Some(k)
        }
    };
    Ok((orientation, twist, scale))
}

/// A sweep's guide `rails` and `rail_fit` (solid.sweep and surface.sweep), checked: one or two
/// rails, each a sketch (default: the path's) and 1…1000 of its curves; a fit of scale or
/// stretch; and no orientation, twist or scale besides, since the rails set those.
pub(super) fn sweep_rails(s: &Session, p: &Value, cmd: &str, path_sketch: u64) -> Result<(Vec<SweepRail>, RailFit)> {
    let rail_fit = match p.get("rail_fit") {
        None | Some(Value::Null) => RailFit::Scale,
        Some(Value::String(f)) if f.eq_ignore_ascii_case("scale") => RailFit::Scale,
        Some(Value::String(f)) if f.eq_ignore_ascii_case("stretch") => RailFit::Stretch,
        Some(_) => return Err(bad(cmd, "`rail_fit` must be scale or stretch")),
    };
    let given = match p.get("rails") {
        None | Some(Value::Null) => return Ok((Vec::new(), rail_fit)),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(bad(cmd, "`rails` must list one or two rails: [{sketch?, curves: [ids]}]")),
    };
    if !(1..=2).contains(&given.len()) {
        return Err(bad(cmd, "a sweep takes one or two guide rails"));
    }
    for key in ["orientation", "twist", "scale"] {
        if p.get(key).is_some_and(|v| !v.is_null()) {
            return Err(bad(cmd, format!("guide rails set the profile's turn and size along the path: leave out `{key}`")));
        }
    }
    let mut rails = Vec::with_capacity(given.len());
    for rail in given {
        let sketch = match rail.get("sketch") {
            None | Some(Value::Null) => path_sketch,
            Some(_) => sketch_id(s, rail.get("sketch"), cmd, "rails[].sketch")?,
        };
        let curves = string_list(rail, "curves");
        if curves.is_empty() || curves.len() > 1000 {
            return Err(bad(cmd, "each rail's `curves` must list 1…1000 curve ids in order"));
        }
        rails.push(SweepRail { sketch: SketchId(sketch), curves });
    }
    Ok((rails, rail_fit))
}

fn loft(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.loft";
    let list = p.get("sections").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`sections` must be a list"))?;
    if list.len() < 2 || list.len() > 50 {
        return Err(bad(cmd, "a loft needs 2…50 sections"));
    }
    let mut sections = Vec::new();
    for sec in list {
        let sketch = sketch_id(s, sec.get("sketch"), cmd, "sections[].sketch")?;
        // A sketch point closes the loft to an apex.
        if let Some(pt) = str_(sec, "point") {
            let st = s.model.state();
            if !st.sketch(sketch).is_some_and(|ss| ss.sketch.points.iter().any(|q| q.id == pt)) {
                return Err(bad(cmd, format!("no point `{pt}` in the section's sketch")));
            }
            sections.push(solidcraft_doc::LoftSection {
                sketch: SketchId(sketch),
                profiles: solidcraft_doc::ProfileSel::All,
                point: Some(pt.to_string()),
            });
            continue;
        }
        sections.push(solidcraft_doc::LoftSection { sketch: SketchId(sketch), profiles: profiles(sec, cmd)?, point: None });
    }
    add_feature(
        s,
        p,
        FeatureKind::Loft { sections, operation: operation(p, cmd)?, targets: string_list(p, "targets").into_iter().map(BodyName).collect() },
    )
}

fn thread(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.thread";
    let face = p
        .get("face")
        .and_then(|v| vec3(v).or_else(|| v.get("point").and_then(vec3)))
        .ok_or_else(|| bad(cmd, "`face` must be a point [x, y, z] on a cylindrical face"))?;
    let designation = str_(p, "designation").map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if let Some(t) = &designation
        && solidcraft_doc::parse_metric_thread(t).is_none()
    {
        return Err(bad(cmd, format!("unknown thread `{t}` (ISO metric, like M8 or M8x1)")));
    }
    let length = expr(p, "length");
    if let Some(l) = &length {
        check_expr(s, l, Kind::Length, cmd, "length")?;
    }
    let modeled = p.get("modeled").and_then(Value::as_bool).unwrap_or(false);
    let v = add_feature(s, p, FeatureKind::Thread { face: FacePick(face), designation, length, modeled })?;
    let st = s.model.state();
    let t = v.get("feature").and_then(Value::as_u64).and_then(|id| st.threads.iter().find(|t| t.feature == id).cloned());
    Ok(json!({"feature": v.get("feature"), "name": v.get("name"), "thread": t}))
}

fn pipe(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.pipe";
    let path_sketch = sketch_id(s, p.get("path_sketch"), cmd, "path_sketch")?;
    let path = string_list(p, "path");
    if path.is_empty() {
        return Err(bad(cmd, "`path` must list curve ids"));
    }
    let diameter = req_expr(cmd, p, "diameter")?;
    check_expr(s, &diameter, Kind::Length, cmd, "diameter")?;
    let wall = expr(p, "wall");
    if let Some(w) = &wall {
        check_expr(s, w, Kind::Length, cmd, "wall")?;
    }
    add_feature(
        s,
        p,
        FeatureKind::Pipe {
            path_sketch: SketchId(path_sketch),
            path,
            diameter,
            wall,
            operation: operation(p, cmd)?,
            targets: string_list(p, "targets").into_iter().map(BodyName).collect(),
        },
    )
}

fn bounding_solid(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.bounding_solid";
    let margin = expr(p, "margin").unwrap_or_else(|| "0".into());
    check_expr(s, &margin, Kind::Length, cmd, "margin")?;
    add_feature(s, p, FeatureKind::BoundingSolid { bodies: string_list(p, "bodies").into_iter().map(BodyName).collect(), margin })
}

fn scale(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.scale";
    let bodies = string_list(p, "bodies");
    if bodies.is_empty() {
        return Err(bad(cmd, "`bodies` must list bodies"));
    }
    let factors = match p.get("factors").and_then(Value::as_array) {
        Some(a) if a.len() == 3 => {
            let e: Vec<String> = a.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect();
            for x in &e {
                check_expr(s, x, Kind::Unitless, cmd, "factors")?;
            }
            Some([e[0].clone().into(), e[1].clone().into(), e[2].clone().into()])
        }
        Some(_) => return Err(bad(cmd, "`factors` must have three values")),
        None => None,
    };
    let factor = match (&factors, expr(p, "factor")) {
        (_, Some(f)) => f,
        (Some(_), None) => "1".into(),
        (None, None) => return Err(bad(cmd, "give `factor` or `factors`")),
    };
    check_expr(s, &factor, Kind::Unitless, cmd, "factor")?;
    let origin = p.get("origin").and_then(vec3).unwrap_or(Vec3::ZERO);
    add_feature(s, p, FeatureKind::Scale { bodies: bodies.into_iter().map(BodyName).collect(), origin: Position(origin), factor, factors })
}

fn delete_face(s: &mut Session, p: &Value) -> Result<Value> {
    let faces = face_points(s, p, "face.delete")?;
    let heal = bool_(p, "heal").unwrap_or(true);
    add_feature(s, p, FeatureKind::DeleteFace { faces: faces.into_iter().map(FacePick).collect(), body: str_(p, "body").map(BodyName::from), heal })
}

fn offset_face(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "solid.offset_face";
    let faces = face_points(s, p, cmd)?;
    let distance = req_expr(cmd, p, "distance")?;
    check_expr(s, &distance, Kind::Length, cmd, "distance")?;
    add_feature(
        s,
        p,
        FeatureKind::OffsetFace { faces: faces.into_iter().map(FacePick).collect(), distance, body: str_(p, "body").map(BodyName::from) },
    )
}

#[cfg(test)]
#[path = "patterns_tests.rs"]
mod patterns_tests;

#[cfg(test)]
#[path = "prism_local_tests.rs"]
mod prism_local_tests;

#[cfg(test)]
#[path = "revolve_convention_tests.rs"]
mod revolve_convention_tests;

#[cfg(test)]
#[path = "sweep_options_tests.rs"]
mod sweep_options_tests;

#[cfg(test)]
#[path = "sweep_rails_tests.rs"]
mod sweep_rails_tests;

#[cfg(test)]
#[path = "offset_topology_tests.rs"]
mod offset_topology_tests;

#[cfg(test)]
#[path = "offset_identity_tests.rs"]
mod offset_identity_tests;

#[cfg(test)]
#[path = "offset_edit_guard_tests.rs"]
mod offset_edit_guard_tests;

#[cfg(test)]
#[path = "offset_plane_face_tests.rs"]
mod offset_plane_face_tests;

#[cfg(test)]
#[path = "lineage_face_tests.rs"]
mod lineage_face_tests;
#[cfg(test)]
#[path = "stacked_face_tests.rs"]
mod stacked_face_tests;

#[cfg(test)]
#[path = "solid_move_copy_tests.rs"]
mod solid_move_copy_tests;

#[cfg(test)]
#[path = "move_in_sketch_tests.rs"]
mod move_in_sketch_tests;

#[cfg(test)]
#[path = "thin_extrude_tests.rs"]
mod thin_extrude_tests;

#[cfg(test)]
#[path = "extrude_convention_tests.rs"]
mod extrude_convention_tests;
/// `solid.fillet corner?: rolling|g2`: the default keeps the rolling ball's sphere corner, `g2`
/// is accepted and refuses (item 10's patch is built but not sewn in yet, see `corner_g2`), and an
/// unknown word is refused.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod corner_tests {
    use serde_json::{Value, json};

    use crate::Session;

    fn run(s: &mut Session, id: &str, p: &Value) -> Value {
        s.execute(id, p).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    /// A 20 mm box with its three edges at one corner, filleted by 4 with `corner`.
    fn cornered(corner: Option<&str>) -> (Session, Value) {
        let mut s = Session::default();
        run(&mut s, "solid.box", &json!({"length": 20, "width": 20, "height": 20, "body_name": "B"}));
        let mut p = json!({"edges": [[10, 0, 0], [0, 10, 0], [0, 0, 10]], "radius": 4});
        if let Some(c) = corner {
            p["corner"] = json!(c);
        }
        let reply = run(&mut s, "solid.fillet", &p);
        (s, reply)
    }

    #[test]
    fn the_default_corner_is_the_rolling_ball_and_g2_refuses_clearly() {
        let (default, reply) = cornered(None);
        assert_eq!(reply["warning"], json!(null), "{reply}");
        let (rolling, _) = cornered(Some("rolling"));
        let ball = default.world_state().body("B").unwrap().body.clone();
        let same = rolling.world_state().body("B").unwrap().body.clone();
        // `corner: rolling` is the default: the same body, to the last bit of its area.
        assert_eq!(solidcraft_kernel::measure(&ball).unwrap().area, solidcraft_kernel::measure(&same).unwrap().area);
        assert!(solidcraft_kernel::measure(&ball).unwrap().volume > 7000.0);
        // `corner: g2`: accepted, and refused with a clear message until the patch sews in.
        let mut s = Session::default();
        run(&mut s, "solid.box", &json!({"length": 20, "width": 20, "height": 20, "body_name": "B"}));
        let err =
            s.execute("solid.fillet", &json!({"edges": [[10, 0, 0], [0, 10, 0], [0, 0, 10]], "radius": 4, "corner": "g2"})).unwrap_err().to_string();
        eprintln!("corner g2: {err}");
        assert!(err.contains("not supported yet:") && err.contains("G2 corner"), "{err}");
        // An unknown word, and a style the G2 corner has no patch for, are refused as parameters.
        assert!(s.execute("solid.fillet", &json!({"edges": [[10, 0, 0]], "radius": 4, "corner": "round"})).is_err());
        assert!(s.execute("solid.fillet", &json!({"edges": [[10, 0, 0]], "radius": 4, "corner": "g2", "type": "chord"})).is_err());
        assert!(s.execute("solid.fillet", &json!({"edges": [[10, 0, 0]], "radius": 4, "corner": "g2", "fit": "reduce"})).is_err());
    }
}
#[cfg(test)]
#[path = "fillet_connected_rounds_tests.rs"]
mod fillet_connected_rounds_tests;
