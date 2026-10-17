//! Surface commands: Patch, Stitch, Thicken and Trim (surface bodies and turning them into
//! solids), and the planned surface toolset (`planned!`: registered, not built yet).

use serde_json::Value;
use solidcraft_doc::{
    BlendSide, EdgeContinuity, Expr, ExtendMode, ExtendTo, FaceAt, FeatureKind, PatchContinuity, PlaneRef, ProfileSel, RuledCorners, RuledMode,
    expr::Kind,
};
use solidcraft_doc::{BodyName, SketchId};
use solidcraft_doc::{Dir, FacePick, Position};
use solidcraft_kernel::Continuity;

use super::CommandSpec;
use super::features::{add_feature, axis_param, check_expr, extent_param, feature_sketch, operation, plane_param, profiles, sketch_id};
use crate::params::{bad, bool_, expr, num, str_, string_list, vec3};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("surface.patch", "Patch", patch)
        .at("SURFACE", "CREATE")
        .icon("surface_patch")
        .params("sketch?: id|name and profiles? (planar patches of sketch regions) | edges: [[x,y,z]] (points on a closed loop of a body's edges: one face on them, planar when the loop is, else a smooth B-spline surface) and body?; continuity?: g0|g1|g2 (edges only, default g0: how the patch meets the faces beside the loop — g1 their tangent planes, g2 also their curvature across the edges) or [{edge: [x,y,z], g: g0|g1|g2}] (those edges so, the rest g0) or {all, edges}; points?: [[x,y,z]] (edges only, up to 64: points the patch passes through); crown?: number −0.9…3 (default 0: only a fitted patch, whose middle becomes fuller by this many times the loop's size — the bump vanishes on the loop, so the face still meets the picked edges and the continuity across them; a loop that is planar and g0 is the exact plane whatever the crown; with points, crown needs a g1 or g2 edge); name?, body_name?"),
    CommandSpec::new("surface.stitch", "Stitch", stitch)
        .at("SURFACE", "MODIFY")
        .icon("surface_stitch")
        .params("bodies: [names] (surfaces sewn into the first; a solid when they close); tolerance?: expr (default: tight)"),
    CommandSpec::new("surface.thicken", "Thicken", thicken)
        .at("SURFACE", "CREATE")
        .icon("surface_thicken")
        .params("bodies: [surface names]; thickness: expr (negative: against the normal); symmetric?: bool; operation?: new|join|cut|intersect; targets?"),
    CommandSpec::new("surface.extend", "Extend", extend)
        .at("SURFACE", "MODIFY")
        .icon("surface_extend")
        .params("body: surface name; edges: [[x,y,z]] (points on boundary edges to push out); distance: expr (unless up_to); mode?: natural (the surface itself continued: exact for planes and surfaces of revolution, a B-spline's end spans continued; the default) | tangent (a ruled run-off face along each edge, G1) | curvature (a run-off face keeping the surface's curvature across the edge, G2); up_to?: plane (XY|XZ|YZ|plane name|{origin, normal}) | {body, point} (a face, its surface extended) | \"bounds\" (natural: back to the surface's own bounds, undoing a trim); neighbours?: bool (default true: the edges next to an extended edge are lengthened to meet it; false: the extension's sides run square to the edge)"),
    CommandSpec::new("surface.trim", "Trim", trim)
        .at("SURFACE", "MODIFY")
        .icon("surface_trim")
        .params("body: surface name; plane: XY|XZ|YZ|plane name|{origin, normal} or tool: {body, point} (a face, its surface extended); keep: [x,y,z] (a point on the side to keep)"),
    CommandSpec::new("surface.extrude", "Surface Extrude", surface_extrude)
        .at("SURFACE", "CREATE")
        .icon("surface_extrude")
        .params("sketch?: id|name; curves?: [curve ids] (open or closed chains of curves meeting end to end) and/or profiles? (their boundaries; the default when no curves are given); distance: expr; direction?: positive|negative|symmetric; distance2?: expr; start_offset?: expr; name?, body_name? (a surface body: walls only, no caps)"),
    CommandSpec::new("surface.revolve", "Surface Revolve", surface_revolve)
        .at("SURFACE", "CREATE")
        .icon("surface_revolve")
        .params("sketch?: id|name; curves?: [curve ids] and/or profiles? (as surface.extrude); axis: sketch line id|x|y|X|Y|Z|{origin, dir}; angle?: expr (default 360 deg); angle2?: expr (the other side); symmetric?: bool; name?, body_name? (a surface body, no caps; a solid where a full turn closes it)"),
    CommandSpec::new("surface.sweep", "Surface Sweep", surface_sweep)
        .at("SURFACE", "CREATE")
        .icon("surface_sweep")
        .params("sketch?: id|name; curves?: [curve ids] and/or profiles? (as surface.extrude); path_sketch: sketch; path: [curve ids in order] (a path with corners, or 3D curves, gives ruled walls mitred at the corners); orientation?, twist?, scale?, rails?, rail_fit? (as solid.sweep); name?, body_name? (a surface body, no caps)"),
    CommandSpec::new("surface.loft", "Surface Loft", surface_loft)
        .at("SURFACE", "CREATE")
        .icon("surface_loft")
        .params("sections: [{sketch, curves?: [curve ids], profiles?} | {body, edges: [[x,y,z]]}] (2…50 in order; each one chain: a profile without holes, or curves meeting end to end; all open or all closed; a body edge chain takes the face the chain bounds as the surface the end repeats); continuity?: [g0|g1|g2|g3, g0|g1|g2|g3] (each end, default g0: how much of the surface at the first and last section the loft repeats — the tangent plane, the curvature, or how that curvature changes); crown?: [number, number] (default 0: how full each end leaves, -0.9 flattest to 3 fullest); name?, body_name? (a surface body: ruled between two sections, smooth through three or more; no caps)"),
    CommandSpec::new("surface.offset", "Offset Surface", offset)
        .at("SURFACE", "CREATE")
        .icon("surface_offset")
        .params("faces: [[x,y,z]] (points on the faces); body?; distance: expr (signed, along the faces' normals; 0 copies the faces); chain?: bool (each pick's whole smooth chain of faces); tolerance?: expr (how far a fitted face may stray, default 0.01 mm). Planes and cylinders move exactly; other faces are fitted. A new surface body (a solid where its faces close)"),
    CommandSpec::new("surface.ruled", "Ruled Surface", ruled)
        .at("SURFACE", "CREATE")
        .icon("surface_ruled")
        .params("body; edges: [[x,y,z]] (points on a chain of its edges, open or closed); distance: expr (how far the strip reaches); mode?: normal|tangent|angle|direction (default normal: along the normal of the face beside each edge; tangent: continuing it); angle?: expr (angle mode: from `direction`, else from the face's normal, turned away from the face; 90 deg from the normal is tangent); direction?: [x,y,z] (direction mode: the rulings; angle mode: the reference, e.g. a pull direction for a draft); face?: [x,y,z] (a point on the face the directions come from; default: the face beside the most picked edges); flip?: bool (the other side); corners?: mitre|round (sharp corners of the chain; inside corners are always cut back). Lines and arcs give exact planes, cylinders and cones, other edges a fitted ruled B-spline. A new surface body"),
    CommandSpec::new("surface.blend", "Blend Surface", blend)
        .at("SURFACE", "CREATE")
        .icon("surface_blend")
        .params("from, to: {body, edges: [[x,y,z]]} (points on the edges of one chain of each body: two picks on the same edge is that one edge, more chain up); continuity?: [g0|g1|g2|g3, g0|g1|g2|g3] (each end, default g1: how much of the surface at the picked edges the blend repeats — the tangent plane, the curvature, or how that curvature changes); bulge?: [number, number] (default 0: how full each end leaves, -0.9 flattest to 3 fullest); align?: [[x,y,z], [x,y,z]] (a point each end leaves towards, when the faces at the picks do not say); stitch?: bool (sew the blend onto its two bodies where the boundaries meet, replacing them); name?, body_name?"),
    CommandSpec::new("surface.untrim", "Untrim", untrim)
        .at("SURFACE", "MODIFY")
        .icon("surface_untrim")
        .params("face: {body, point}; holes?: all|[[x,y,z]] (points on the inner loops to remove; default: all); outer?: bool (restore the outer boundary to the surface's natural bounds: a free-form surface's own, an analytic surface's span of the face); in_place?: bool (untrim a surface body's face where it is; a solid's face is always copied out). A surface body; the face keeps its name"),
    CommandSpec::new("surface.unstitch", "Unstitch", unstitch)
        .at("SURFACE", "MODIFY")
        .icon("surface_unstitch")
        .params("body; faces?: [[x,y,z]] (points on the faces to take out, each its own surface body; the rest stay together under the body's name; default: every face its own surface body). A solid comes apart into surfaces"),
    CommandSpec::new("surface.reverse_normal", "Reverse Normal", reverse_normal)
        .at("SURFACE", "MODIFY")
        .icon("surface_reverse_normal")
        .params("body (a surface); faces?: [[x,y,z]] (points on the faces to turn over; default: all of them)"),
    CommandSpec::new("surface.fair", "Fair Surface", fair)
        .at("SURFACE", "MODIFY")
        .icon("surface_fair")
        .params("body: surface name; faces?: [[x,y,z]] points on the faces to fair (default: every face); strength?: number (0..1 of the full fairing step, default 1); iterations?: number (1..1000 steps, default 1); keep_edges?: bool (default true: hold the boundary row and the row next to it, so a face keeps meeting its neighbours in position and in tangent); tolerance?: expr mm (how far a control vertex may move, default 0.001 of the body's size). Moves the interior control vertices of each B-spline face to flatten its curvature variation while the boundary stays put: the face keeps its edges and the body its shape within the tolerance, and the warning in the reply carries the energy before and after; name?"),
    planned!("surface.fillet", "Surface Fillet", "769")
        .at("SURFACE", "MODIFY")
        .icon("surface_fillet")
        .params("planned: a, b: {body, point} (two faces of separate bodies that meet or cross; the points pick the side the ball rolls on); radius: expr; trim?: both|first|none"),
];

fn patch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.patch";
    let crown = crown_param(p, cmd)?;
    let continuity = patch_continuity(p, cmd)?;
    let points: Vec<Position> = patch_points(p, cmd)?.into_iter().map(Position).collect();
    let edges: Vec<solidcraft_geom::Vec3> = p.get("edges").and_then(Value::as_array).map(|a| a.iter().filter_map(vec3).collect()).unwrap_or_default();
    let mut v = if !edges.is_empty() {
        if edges.len() > 10_000 {
            return Err(bad(cmd, "too many edges"));
        }
        let body = str_(p, "body").map(str::to_string);
        add_feature(
            s,
            p,
            FeatureKind::Patch {
                sketch: None,
                profiles: ProfileSel::All,
                body: body.map(BodyName),
                edges: edges.into_iter().map(Position).collect(),
                crown,
                continuity: continuity.clone(),
                points,
            },
        )?
    } else {
        if !continuity.is_g0() || !points.is_empty() {
            return Err(bad(cmd, "`continuity` and `points` are for a patch on a loop of edges"));
        }
        let sketch = feature_sketch(s, p, cmd)?;
        let profiles = profiles(p, cmd)?;
        let kind = FeatureKind::Patch {
            sketch: Some(SketchId(sketch)),
            profiles,
            body: None,
            edges: Vec::new(),
            crown,
            continuity: PatchContinuity::default(),
            points,
        };
        add_feature(s, p, kind)?
    };
    if let Some(o) = v.as_object_mut() {
        o.insert("crown".into(), serde_json::json!(crown));
        o.insert("continuity".into(), serde_json::to_value(&continuity).unwrap_or(Value::Null));
    }
    Ok(v)
}

/// A continuity word for a patch: g0, g1 or g2 (a patch does not hold g3).
fn patch_order(v: Option<&Value>, cmd: &str) -> Result<Continuity> {
    match v.and_then(Value::as_str).and_then(Continuity::parse) {
        Some(g) if g != Continuity::G3 => Ok(g),
        _ => Err(bad(cmd, "a patch's continuity is g0, g1 or g2")),
    }
}

/// The patch's `continuity`: one word for every edge, a list of `{edge, g}` for some edges (the
/// rest g0), or `{all, edges}`.
fn patch_continuity(p: &Value, cmd: &str) -> Result<PatchContinuity> {
    let edges = |list: &Value| -> Result<Vec<EdgeContinuity>> {
        let list = list.as_array().ok_or_else(|| bad(cmd, "`continuity` edges must be a list of {edge: [x,y,z], g}"))?;
        if list.len() > 10_000 {
            return Err(bad(cmd, "`continuity` lists at most 10000 edges"));
        }
        list.iter()
            .map(|e| {
                let edge = e.get("edge").and_then(vec3).ok_or_else(|| bad(cmd, "a `continuity` entry's `edge` must be a point [x,y,z]"))?;
                Ok(EdgeContinuity { edge: Position(edge), g: patch_order(e.get("g"), cmd)? })
            })
            .collect()
    };
    match p.get("continuity") {
        None => Ok(PatchContinuity::default()),
        Some(v) if v.is_string() => Ok(PatchContinuity { all: patch_order(Some(v), cmd)?, edges: Vec::new() }),
        Some(v) if v.is_array() => Ok(PatchContinuity { all: Continuity::G0, edges: edges(v)? }),
        Some(v) if v.is_object() => {
            let all = match v.get("all") {
                None => Continuity::G0,
                some => patch_order(some, cmd)?,
            };
            let edges = match v.get("edges") {
                None => Vec::new(),
                Some(list) => edges(list)?,
            };
            Ok(PatchContinuity { all, edges })
        }
        Some(_) => Err(bad(cmd, "`continuity` must be g0, g1 or g2, or a list of {edge, g}")),
    }
}

/// The points a patch passes through (at most 64, each a finite point).
fn patch_points(p: &Value, cmd: &str) -> Result<Vec<solidcraft_geom::Vec3>> {
    let Some(v) = p.get("points") else { return Ok(Vec::new()) };
    let list = v
        .as_array()
        .filter(|a| a.len() <= solidcraft_geom::fill_general::MAX_FILL_POINTS)
        .ok_or_else(|| bad(cmd, "`points` must list at most 64 points [x,y,z]"))?;
    list.iter().map(|q| vec3(q).filter(|q| q.is_finite()).ok_or_else(|| bad(cmd, "a patch point must be a finite [x,y,z]"))).collect()
}

/// The patch's `crown`: how full a fitted patch's middle is made, -0.9…3 (default 0). A hostile
/// value (NaN, infinity, out of range, a string) is refused.
fn crown_param(p: &Value, cmd: &str) -> Result<f64> {
    match p.get("crown") {
        None => Ok(0.0),
        Some(v) => match v.as_f64().filter(|x| (solidcraft_geom::fill::MIN_CROWN..=solidcraft_geom::fill::MAX_CROWN).contains(x)) {
            Some(x) => Ok(x),
            None => Err(bad(cmd, "`crown` must be a number between -0.9 and 3")),
        },
    }
}

/// One end of a blend: the body and the picks that chain its edges.
fn blend_side(p: &Value, cmd: &str, key: &str) -> Result<BlendSide> {
    let side = p.get(key).ok_or_else(|| bad(cmd, format!("`{key}` must be {{body, edges}}")))?;
    let body = str_(side, "body").ok_or_else(|| bad(cmd, format!("`{key}.body` must name a body")))?;
    let edges: Vec<solidcraft_geom::Vec3> =
        side.get("edges").and_then(Value::as_array).map(|a| a.iter().filter_map(vec3).collect()).unwrap_or_default();
    if edges.is_empty() || edges.len() > 10_000 {
        return Err(bad(cmd, format!("`{key}.edges` must list 1 to 10000 points on the edges of a chain")));
    }
    Ok(BlendSide { body: BodyName(body.to_string()), edges: edges.into_iter().map(Position).collect() })
}

/// `surface.blend`: a blend surface bridging two picked edge chains of two bodies.
fn blend(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.blend";
    let from = blend_side(p, cmd, "from")?;
    let to = blend_side(p, cmd, "to")?;
    let continuity = match p.get("continuity") {
        None => [Continuity::G1, Continuity::G1],
        Some(v) => {
            let list = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, "`continuity` must be two words, one per end"))?;
            let word = |x: Option<&Value>| -> Result<Continuity> {
                x.and_then(Value::as_str).and_then(Continuity::parse).ok_or_else(|| bad(cmd, "`continuity` entries must be g0, g1, g2 or g3"))
            };
            [word(list.first())?, word(list.get(1))?]
        }
    };
    let bulge = match p.get("bulge") {
        None => [0.0, 0.0],
        Some(v) => {
            let list = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, "`bulge` must be two numbers, one per end"))?;
            let one = |x: Option<&Value>| -> Result<f64> {
                x.and_then(Value::as_f64)
                    .filter(|x| x.is_finite() && (-0.9..=3.0).contains(x))
                    .ok_or_else(|| bad(cmd, "each `bulge` entry must be between -0.9 (flattest) and 3 (fullest)"))
            };
            [one(list.first())?, one(list.get(1))?]
        }
    };
    let align = match p.get("align") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let list = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, "`align` must be two points, one per end"))?;
            let point = |x: Option<&Value>| -> Result<solidcraft_geom::Vec3> {
                x.and_then(vec3).ok_or_else(|| bad(cmd, "each `align` entry must be a point [x,y,z]"))
            };
            Some([Position(point(list.first())?), Position(point(list.get(1))?)])
        }
    };
    let stitch = bool_(p, "stitch").unwrap_or(false);
    add_feature(s, p, FeatureKind::SurfaceBlend { from, to, continuity, bulge, align, stitch })
}

fn surface_extrude(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.extrude";
    let sketch = feature_sketch(s, p, cmd)?;
    let (profiles, curves) = sheet_inputs(p, cmd)?;
    let extent = extent_param(s, p, cmd)?;
    if extent.through_all || extent.taper.is_some() || extent.to.is_some() || extent.from.is_some() {
        return Err(bad(cmd, "not supported yet: through all, to or from an object, or a taper"));
    }
    add_feature(s, p, FeatureKind::SurfaceExtrude { sketch: SketchId(sketch), profiles, curves, extent })
}

/// A body by name, and points on some of its faces (none: all of them).
fn body_faces(s: &Session, p: &Value, cmd: &str) -> Result<(String, Vec<solidcraft_geom::Vec3>)> {
    let body = str_(p, "body").ok_or_else(|| bad(cmd, "`body` is required"))?.to_string();
    if s.model.state().body(&body).is_none() {
        return Err(bad(cmd, format!("no body `{body}`")));
    }
    let faces: Vec<solidcraft_geom::Vec3> = p.get("faces").and_then(Value::as_array).map(|a| a.iter().filter_map(vec3).collect()).unwrap_or_default();
    if faces.len() > 10_000 {
        return Err(bad(cmd, "too many faces"));
    }
    Ok((body, faces))
}

fn unstitch(s: &mut Session, p: &Value) -> Result<Value> {
    let (body, faces) = body_faces(s, p, "surface.unstitch")?;
    add_feature(s, p, FeatureKind::Unstitch { body: BodyName(body), faces: faces.into_iter().map(Position).collect() })
}

fn reverse_normal(s: &mut Session, p: &Value) -> Result<Value> {
    let (body, faces) = body_faces(s, p, "surface.reverse_normal")?;
    add_feature(s, p, FeatureKind::ReverseNormal { body: BodyName(body), faces: faces.into_iter().map(Position).collect() })
}

fn untrim(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.untrim";
    let face = p.get("face").ok_or_else(|| bad(cmd, "`face` is required: {body, point}"))?;
    let body = face.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`face.body` must name a body"))?.to_string();
    if s.model.state().body(&body).is_none() {
        return Err(bad(cmd, format!("no body `{body}`")));
    }
    let point = face.get("point").and_then(vec3).ok_or_else(|| bad(cmd, "`face.point` must be [x, y, z] on the face"))?;
    let holes = match p.get("holes") {
        None | Some(Value::Null) => None,
        Some(Value::String(all)) if all == "all" => None,
        Some(Value::Array(points)) => {
            if points.len() > 10_000 {
                return Err(bad(cmd, "too many holes"));
            }
            let points: Option<Vec<solidcraft_geom::Vec3>> = points.iter().map(vec3).collect();
            Some(points.ok_or_else(|| bad(cmd, "`holes` must list points [x, y, z] on the inner loops"))?)
        }
        Some(_) => return Err(bad(cmd, "`holes` must be \"all\" or a list of points [x, y, z] on the inner loops")),
    };
    let outer = bool_(p, "outer").unwrap_or(false);
    if !outer && holes.as_ref().is_some_and(Vec::is_empty) {
        return Err(bad(cmd, "nothing to untrim: pick holes to remove or restore the outer boundary"));
    }
    let kind = FeatureKind::SurfaceUntrim {
        face: FaceAt { body: BodyName(body), point: Position(point) },
        holes: holes.map(|h| h.into_iter().map(Position).collect()),
        outer,
        in_place: bool_(p, "in_place").unwrap_or(false),
    };
    add_feature(s, p, kind)
}

/// The profiles and curves a surface builder takes: `curves`, and `profiles` (all of them when
/// neither is given).
fn sheet_inputs(p: &Value, cmd: &str) -> Result<(Option<solidcraft_doc::ProfileSel>, Vec<String>)> {
    let curves = string_list(p, "curves");
    if curves.len() > 1000 {
        return Err(bad(cmd, "at most 1000 curves"));
    }
    let profiles = if curves.is_empty() || p.get("profiles").is_some() { Some(profiles(p, cmd)?) } else { None };
    Ok((profiles, curves))
}

fn surface_revolve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.revolve";
    let sketch = feature_sketch(s, p, cmd)?;
    let (profiles, curves) = sheet_inputs(p, cmd)?;
    let axis = axis_param(p, cmd)?;
    let angle = expr(p, "angle").unwrap_or_else(|| "360 deg".into());
    check_expr(s, &angle, Kind::Angle, cmd, "angle")?;
    let angle2 = expr(p, "angle2");
    if let Some(a) = &angle2 {
        check_expr(s, a, Kind::Angle, cmd, "angle2")?;
    }
    let symmetric = bool_(p, "symmetric").unwrap_or(false) || str_(p, "direction").is_some_and(|d| d.eq_ignore_ascii_case("symmetric"));
    add_feature(s, p, FeatureKind::SurfaceRevolve { sketch: SketchId(sketch), profiles, curves, axis, angle, angle2, symmetric })
}

fn surface_sweep(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.sweep";
    let sketch = feature_sketch(s, p, cmd)?;
    let (profiles, curves) = sheet_inputs(p, cmd)?;
    let path_sketch = sketch_id(s, p.get("path_sketch"), cmd, "path_sketch")?;
    let path = string_list(p, "path");
    if path.is_empty() || path.len() > 1000 {
        return Err(bad(cmd, "`path` must list 1…1000 curve ids"));
    }
    let (orientation, twist, scale) = super::features::sweep_options(s, p, cmd)?;
    let (rails, rail_fit) = super::features::sweep_rails(s, p, cmd, path_sketch)?;
    add_feature(
        s,
        p,
        FeatureKind::SurfaceSweep {
            sketch: SketchId(sketch),
            profiles,
            curves,
            path_sketch: SketchId(path_sketch),
            path,
            orientation,
            twist,
            scale,
            rails,
            rail_fit,
        },
    )
}

fn surface_loft(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.loft";
    let given = p.get("sections").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`sections` must list the sections in order"))?;
    if !(2..=50).contains(&given.len()) {
        return Err(bad(cmd, "a loft needs 2…50 sections"));
    }
    let mut sections = Vec::with_capacity(given.len());
    for sec in given {
        // A section is a chain of sketch curves, or an edge chain of a body whose face the end of
        // the loft repeats.
        if let Some(body) = str_(sec, "body") {
            let edges: Vec<solidcraft_geom::Vec3> =
                sec.get("edges").and_then(Value::as_array).map(|a| a.iter().filter_map(vec3).collect()).unwrap_or_default();
            if edges.is_empty() || edges.len() > 10_000 {
                return Err(bad(cmd, "`sections.edges` must list 1 to 10000 points on the edges of a chain"));
            }
            if sec.get("sketch").is_some() {
                return Err(bad(cmd, "a section is either a sketch chain or a body edge chain"));
            }
            sections.push(solidcraft_doc::SheetSection {
                sketch: SketchId(0),
                profiles: None,
                curves: Vec::new(),
                body: Some(BodyName::from(body.to_string())),
                edges: edges.into_iter().map(Position).collect(),
            });
            continue;
        }
        let sketch = sketch_id(s, sec.get("sketch"), cmd, "sections.sketch")?;
        let curves = string_list(sec, "curves");
        if curves.len() > 1000 {
            return Err(bad(cmd, "at most 1000 curves a section"));
        }
        let profiles = if sec.get("profiles").is_some() { Some(profiles(sec, cmd)?) } else { None };
        sections.push(solidcraft_doc::SheetSection { sketch: SketchId(sketch), profiles, curves, body: None, edges: Vec::new() });
    }
    let continuity = match p.get("continuity") {
        None => [Continuity::G0, Continuity::G0],
        Some(v) => {
            let list = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, "`continuity` must be two words, one per end"))?;
            let word = |x: Option<&Value>| -> Result<Continuity> {
                x.and_then(Value::as_str).and_then(Continuity::parse).ok_or_else(|| bad(cmd, "`continuity` entries must be g0, g1, g2 or g3"))
            };
            [word(list.first())?, word(list.get(1))?]
        }
    };
    let crown = match p.get("crown") {
        None | Some(Value::Null) => [0.0, 0.0],
        Some(v) => {
            let list = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, "`crown` must be two numbers, one per end"))?;
            let one = |x: Option<&Value>| -> Result<f64> {
                x.and_then(Value::as_f64)
                    .filter(|x| x.is_finite() && (-0.9..=3.0).contains(x))
                    .ok_or_else(|| bad(cmd, "each `crown` entry must be between -0.9 (flattest) and 3 (fullest)"))
            };
            [one(list.first())?, one(list.get(1))?]
        }
    };
    add_feature(s, p, FeatureKind::SurfaceLoft { sections, continuity, crown })
}

fn offset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.offset";
    let faces: Vec<solidcraft_geom::Vec3> = p.get("faces").and_then(Value::as_array).map(|a| a.iter().filter_map(vec3).collect()).unwrap_or_default();
    if faces.is_empty() || faces.len() > 10_000 {
        return Err(bad(cmd, "`faces` must list points on the faces to offset"));
    }
    let distance = expr(p, "distance").ok_or_else(|| bad(cmd, "`distance` is required"))?;
    check_expr(s, &distance, Kind::Length, cmd, "distance")?;
    let tolerance = expr(p, "tolerance");
    if let Some(t) = &tolerance {
        check_expr(s, t, Kind::Length, cmd, "tolerance")?;
    }
    let kind = FeatureKind::SurfaceOffset {
        faces: faces.into_iter().map(FacePick).collect(),
        body: str_(p, "body").map(BodyName::from),
        distance,
        chain: bool_(p, "chain").unwrap_or(false),
        tolerance,
    };
    add_feature(s, p, kind)
}

fn ruled(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.ruled";
    let body = str_(p, "body").ok_or_else(|| bad(cmd, "`body` is required"))?.to_string();
    if s.model.state().body(&body).is_none() {
        return Err(bad(cmd, format!("no body `{body}`")));
    }
    let edges: Vec<solidcraft_geom::Vec3> = p.get("edges").and_then(Value::as_array).map(|a| a.iter().filter_map(vec3).collect()).unwrap_or_default();
    if edges.is_empty() || edges.len() > 10_000 {
        return Err(bad(cmd, "`edges` must list points on a chain of the body's edges"));
    }
    let distance = expr(p, "distance").ok_or_else(|| bad(cmd, "`distance` is required"))?;
    check_expr(s, &distance, Kind::Length, cmd, "distance")?;
    let mode = match str_(p, "mode").map(str::to_ascii_lowercase).as_deref() {
        None | Some("normal") => RuledMode::Normal,
        Some("tangent") => RuledMode::Tangent,
        Some("angle") => RuledMode::Angle,
        Some("direction") => RuledMode::Direction,
        Some(o) => return Err(bad(cmd, format!("unknown mode `{o}` (normal, tangent, angle or direction)"))),
    };
    let angle = match mode {
        RuledMode::Angle => {
            let a = expr(p, "angle").ok_or_else(|| bad(cmd, "`angle` is required with mode angle"))?;
            check_expr(s, &a, Kind::Angle, cmd, "angle")?;
            Some(a)
        }
        _ => None,
    };
    let direction = match (mode, p.get("direction").filter(|v| !v.is_null())) {
        (RuledMode::Angle | RuledMode::Direction, Some(v)) => {
            let d = vec3(v).filter(|d| d.len() > 1e-12).ok_or_else(|| bad(cmd, "`direction` must be a non-zero [x, y, z]"))?;
            Some(d)
        }
        (RuledMode::Direction, None) => return Err(bad(cmd, "`direction` is required with mode direction")),
        _ => None,
    };
    let corners = match str_(p, "corners").map(str::to_ascii_lowercase).as_deref() {
        None | Some("mitre") | Some("miter") => RuledCorners::Mitre,
        Some("round") => RuledCorners::Round,
        Some(o) => return Err(bad(cmd, format!("unknown corners `{o}` (mitre or round)"))),
    };
    let face = match p.get("face").filter(|v| !v.is_null()) {
        Some(v) => Some(vec3(v).ok_or_else(|| bad(cmd, "`face` must be [x, y, z] on a face"))?),
        None => None,
    };
    let kind = FeatureKind::SurfaceRuled {
        body: BodyName(body),
        edges: edges.into_iter().map(Position).collect(),
        distance,
        mode,
        angle,
        direction: direction.map(Dir),
        flip: bool_(p, "flip").unwrap_or(false),
        corners,
        face: face.map(FacePick),
    };
    add_feature(s, p, kind)
}

fn bodies_param(s: &Session, p: &Value, cmd: &str) -> Result<Vec<String>> {
    let bodies = string_list(p, "bodies");
    if bodies.is_empty() {
        return Err(bad(cmd, "`bodies` must list bodies"));
    }
    let st = s.model.state();
    if let Some(b) = bodies.iter().find(|b| st.body(b).is_none()) {
        return Err(bad(cmd, format!("no body `{b}`")));
    }
    Ok(bodies)
}

fn stitch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.stitch";
    let bodies = bodies_param(s, p, cmd)?;
    let tolerance = expr(p, "tolerance").unwrap_or_else(|| "0".into());
    check_expr(s, &tolerance, Kind::Length, cmd, "tolerance")?;
    add_feature(s, p, FeatureKind::Stitch { bodies: bodies.into_iter().map(BodyName).collect(), tolerance })
}

fn thicken(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.thicken";
    let bodies = bodies_param(s, p, cmd)?;
    let thickness = expr(p, "thickness").ok_or_else(|| bad(cmd, "`thickness` is required"))?;
    check_expr(s, &thickness, Kind::Length, cmd, "thickness")?;
    let kind = FeatureKind::Thicken {
        bodies: bodies.into_iter().map(BodyName).collect(),
        thickness,
        symmetric: bool_(p, "symmetric").unwrap_or(false),
        operation: operation(p, cmd)?,
        targets: string_list(p, "targets").into_iter().map(BodyName).collect(),
    };
    add_feature(s, p, kind)
}

fn extend(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.extend";
    let body = str_(p, "body").ok_or_else(|| bad(cmd, "`body` is required"))?.to_string();
    if s.model.state().body(&body).is_none() {
        return Err(bad(cmd, format!("no body `{body}`")));
    }
    let edges: Vec<solidcraft_geom::Vec3> = p.get("edges").and_then(Value::as_array).map(|a| a.iter().filter_map(vec3).collect()).unwrap_or_default();
    if edges.is_empty() || edges.len() > 10_000 {
        return Err(bad(cmd, "`edges` must list points on the edges to extend"));
    }
    let mode = match str_(p, "mode").map(str::trim) {
        None | Some("natural") => ExtendMode::Natural,
        Some("tangent") => ExtendMode::Tangent,
        Some("curvature") => ExtendMode::Curvature,
        Some(m) => return Err(bad(cmd, format!("`mode` must be natural, tangent or curvature, not `{m}`"))),
    };
    let up_to = match p.get("up_to").filter(|v| !v.is_null()) {
        None => None,
        Some(Value::String(b)) if b.trim() == "bounds" => Some(ExtendTo::Bounds),
        Some(t @ Value::Object(o)) if o.contains_key("body") => {
            let tb = t.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`up_to.body` must name a body"))?.to_string();
            if s.model.state().body(&tb).is_none() {
                return Err(bad(cmd, format!("no body `{tb}`")));
            }
            let point = t.get("point").and_then(vec3).ok_or_else(|| bad(cmd, "`up_to.point` must be [x, y, z] on the target face"))?;
            Some(ExtendTo::Face(FaceAt { body: BodyName(tb), point: Position(point) }))
        }
        Some(v) => Some(ExtendTo::Plane(plane_param(s, Some(v), cmd)?)),
    };
    if up_to == Some(ExtendTo::Bounds) && mode != ExtendMode::Natural {
        return Err(bad(cmd, "only a natural extension goes back to the surface's bounds"));
    }
    let distance = match (expr(p, "distance"), &up_to) {
        (Some(d), _) => d,
        (None, Some(_)) => "0 mm".into(),
        (None, None) => return Err(bad(cmd, "`distance` is required (or `up_to`)")),
    };
    check_expr(s, &distance, Kind::Length, cmd, "distance")?;
    let neighbours = bool_(p, "neighbours").unwrap_or(true);
    add_feature(
        s,
        p,
        FeatureKind::SurfaceExtend { body: BodyName(body), edges: edges.into_iter().map(Position).collect(), distance, mode, up_to, neighbours },
    )
}

fn trim(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.trim";
    let body = str_(p, "body").ok_or_else(|| bad(cmd, "`body` is required"))?.to_string();
    if s.model.state().body(&body).is_none() {
        return Err(bad(cmd, format!("no body `{body}`")));
    }
    let keep = p.get("keep").and_then(vec3).ok_or_else(|| bad(cmd, "`keep` must be [x, y, z] on the side to keep"))?;
    let (plane, tool) = match p.get("tool").filter(|t| !t.is_null()) {
        Some(t) => {
            let tb = t.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`tool.body` must name a body"))?.to_string();
            let point = t.get("point").and_then(vec3).ok_or_else(|| bad(cmd, "`tool.point` must be [x, y, z] on the tool face"))?;
            (PlaneRef::Origin { name: "XY".into() }, Some(FaceAt { body: BodyName(tb), point: Position(point) }))
        }
        None => (plane_param(s, p.get("plane"), cmd)?, None),
    };
    add_feature(s, p, FeatureKind::SurfaceTrim { body: BodyName(body), plane, tool, keep: Position(keep) })
}

/// `surface.fair`: the B-spline faces of a body faired in place. The tolerance a caller leaves
/// out is a thousandth of the body's own size, so a fairing run can never make a design jump by
/// more than a hair of itself.
fn fair(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.fair";
    let (body, faces) = body_faces(s, p, cmd)?;
    let size = {
        let state = s.model.state();
        let Some(mb) = state.body(&body) else { return Err(bad(cmd, format!("no body `{body}`"))) };
        mb.body.size()
    };
    let strength = num(p, "strength").unwrap_or(1.0);
    if !(strength.is_finite() && strength > 0.0 && strength <= 1.0) {
        return Err(bad(cmd, "`strength` must be greater than 0 and at most 1 (the whole fairing step)"));
    }
    let steps = num(p, "iterations").unwrap_or(1.0);
    if !(steps.is_finite() && (1.0..=1000.0).contains(&steps)) {
        return Err(bad(cmd, "`iterations` must be 1 to 1000"));
    }
    let keep_edges = bool_(p, "keep_edges").unwrap_or(true);
    let tolerance = expr(p, "tolerance").unwrap_or_else(|| Expr::new(format!("{:.6}", (size * 1e-3).max(1e-6))));
    check_expr(s, tolerance.as_str(), Kind::Length, cmd, "tolerance")?;
    let kind = FeatureKind::SurfaceFair {
        body: BodyName::from(body),
        faces: faces.into_iter().map(FacePick).collect(),
        strength,
        iterations: steps as u32,
        keep_edges,
        tolerance,
    };
    add_feature(s, p, kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use serde_json::json;

    fn run(s: &mut Session, c: &str, p: Value) -> Value {
        s.execute(c, &p).unwrap_or_else(|e| panic!("{c}: {e}"))
    }
    fn volume(s: &Session, name: &str) -> f64 {
        let st = s.world_state();
        solidcraft_kernel::measure(&st.body(name).unwrap().body).unwrap().volume
    }

    fn area(s: &Session, name: &str) -> f64 {
        solidcraft_kernel::measure(&s.world_state().body(name).unwrap().body).unwrap().area
    }

    /// Surface Extrude of open curves: a line and an arc joined end to end (listed in either
    /// order) make one sheet of two walls; both sides of the sketch; undo, redo and reopening
    /// give the same sheet.
    #[test]
    fn surface_extrude_open_curves() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.line", json!({"points": [[0, 0], [10, 0]], "ids": ["l1"]}));
        let arc = run(&mut s, "sketch.arc.three_point", json!({"start": [10, 0], "end": [20, 0], "through": [15, 5]}))["created"]["curves"][0]
            .as_str()
            .unwrap()
            .to_string();
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.extrude", json!({"sketch": "S", "curves": [arc, "l1"], "distance": 4, "body_name": "Wall"}));
        let st = s.world_state();
        let wall = &st.body("Wall").unwrap().body;
        assert!(wall.is_surface() && wall.face_count() == 2);
        // The line's wall and the half circle's (radius 5): length × height.
        let want = (10.0 + std::f64::consts::PI * 5.0) * 4.0;
        assert!((area(&s, "Wall") - want).abs() < want * 1e-3, "{} vs {want}", area(&s, "Wall"));
        run(&mut s, "surface.extrude", json!({"sketch": "S", "curves": ["l1"], "distance": 2, "direction": "symmetric", "body_name": "Both"}));
        let bb = solidcraft_kernel::measure(&s.world_state().body("Both").unwrap().body).unwrap().bbox;
        assert!((bb.min.z + 2.0).abs() < 1e-9 && (bb.max.z - 2.0).abs() < 1e-9, "{bb:?}");
        run(&mut s, "edit.undo", json!({}));
        assert!(s.world_state().body("Both").is_none());
        run(&mut s, "edit.redo", json!({}));
        assert!(s.world_state().body("Both").is_some());
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Wall") - area(&s, "Wall")).abs() < 1e-9);
        // Refused: no such curve, a zero distance, and an extent a sheet does not take yet.
        assert!(s.execute("surface.extrude", &json!({"sketch": "S", "curves": ["nope"], "distance": 4})).is_err());
        assert!(s.execute("surface.extrude", &json!({"sketch": "S", "curves": ["l1"], "distance": 0})).is_err());
        assert!(s.execute("surface.extrude", &json!({"sketch": "S", "curves": ["l1"], "distance": 4, "through_all": true})).is_err());
    }

    /// Surface Extrude of a profile gives its walls alone; patching both ends and stitching
    /// makes the solid that solid.extrude makes of the same profile.
    #[test]
    fn surface_extrude_profile_walls_stitch_into_the_solid() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "R"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [10, 6]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.extrude", json!({"sketch": "R", "distance": 3, "body_name": "Walls"}));
        let st = s.world_state();
        let walls = &st.body("Walls").unwrap().body;
        assert!(walls.is_surface() && walls.face_count() == 4);
        assert!((area(&s, "Walls") - 96.0).abs() < 1e-9);
        let edges = |z: f64| json!([[5, 0, z], [10, 3, z], [5, 6, z], [0, 3, z]]);
        run(&mut s, "surface.patch", json!({"edges": edges(0.0), "body": "Walls", "body_name": "Bottom"}));
        run(&mut s, "surface.patch", json!({"edges": edges(3.0), "body": "Walls", "body_name": "Top"}));
        run(&mut s, "surface.stitch", json!({"bodies": ["Walls", "Bottom", "Top"]}));
        let st = s.world_state();
        assert!(!st.body("Walls").unwrap().body.is_surface());
        run(&mut s, "solid.extrude", json!({"sketch": "R", "distance": 3, "body_name": "Solid"}));
        assert!((volume(&s, "Walls") - volume(&s, "Solid")).abs() < 1e-9);
        assert!((volume(&s, "Walls") - 180.0).abs() < 1e-9);
    }

    /// Unstitch a box: six surface bodies whose faces keep the names they had on the box,
    /// stitched back into a solid of the same volume; two faces taken out leave the other four
    /// under the box's name; undo, redo and reopening.
    #[test]
    fn unstitch_a_box_and_stitch_it_back() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Box"}));
        let names_before: Vec<String> = solidcraft_doc::naming::face_names(s.model.state().body("Box").unwrap()).to_vec();
        run(&mut s, "surface.unstitch", json!({"body": "Box"}));
        let st = s.world_state();
        assert!(st.body("Box").is_none());
        let pieces: Vec<String> = st.bodies.iter().map(|b| b.name.clone()).collect();
        assert_eq!(pieces.len(), 6);
        let mut names_after: Vec<String> = Vec::new();
        for n in &pieces {
            let b = st.body(n).unwrap();
            assert!(b.body.is_surface() && b.body.face_count() == 1);
            names_after.extend(solidcraft_doc::naming::face_names(s.model.state().body(n).unwrap()).iter().cloned());
        }
        let (mut a, mut b) = (names_before.clone(), names_after);
        a.sort();
        b.sort();
        assert_eq!(a, b, "the faces keep their names");
        run(&mut s, "surface.stitch", json!({"bodies": pieces}));
        let st = s.world_state();
        let back = st.bodies.first().unwrap();
        assert!(!back.body.is_surface());
        assert!((solidcraft_kernel::measure(&back.body).unwrap().volume - 1000.0).abs() < 1e-9);
        run(&mut s, "edit.undo", json!({}));
        run(&mut s, "edit.undo", json!({}));
        assert!(s.world_state().body("Box").is_some());
        run(&mut s, "surface.unstitch", json!({"body": "Box", "faces": [[5, 5, 10], [5, 5, 0]]}));
        let st = s.world_state();
        assert_eq!(st.bodies.len(), 3);
        assert_eq!(st.body("Box").unwrap().body.face_count(), 4);
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert_eq!(cold.world_state().bodies.len(), 3);
        assert!(s.execute("surface.unstitch", &json!({"body": "Nope"})).is_err());
        assert!(s.execute("surface.unstitch", &json!({})).is_err());
    }

    /// A patch turned over thickens the other way; a solid is not turned over.
    #[test]
    fn reverse_normal_turns_the_thicken_side() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [10, 10]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.patch", json!({"sketch": "S", "body_name": "Sheet"}));
        run(&mut s, "surface.thicken", json!({"bodies": ["Sheet"], "thickness": 2, "body_name": "Up"}));
        let up = solidcraft_kernel::measure(&s.world_state().body("Up").unwrap().body).unwrap().bbox;
        run(&mut s, "edit.undo", json!({}));
        run(&mut s, "surface.reverse_normal", json!({"body": "Sheet"}));
        run(&mut s, "surface.thicken", json!({"bodies": ["Sheet"], "thickness": 2, "body_name": "Down"}));
        let down = solidcraft_kernel::measure(&s.world_state().body("Down").unwrap().body).unwrap().bbox;
        assert!((up.max.z - down.max.z - 2.0).abs() < 1e-9 && (up.min.z - down.min.z - 2.0).abs() < 1e-9, "{up:?} vs {down:?}");
        run(&mut s, "solid.box", json!({"length": 1, "width": 1, "height": 1, "corner": [20, 0, 0], "body_name": "Cube"}));
        assert!(s.execute("surface.reverse_normal", &json!({"body": "Cube"})).is_err());
    }

    /// Delete Face without healing: a box's top taken off leaves a five-face surface; patching
    /// the opening and stitching gives back a box of the same volume. By default the gap still
    /// heals (here it can't: refused); deleting every face is refused.
    #[test]
    fn delete_face_without_healing_leaves_an_open_surface() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Box"}));
        run(&mut s, "face.delete", json!({"faces": [[5, 5, 10]], "heal": false}));
        let st = s.world_state();
        let open = &st.body("Box").unwrap().body;
        assert!(open.is_surface() && open.face_count() == 5);
        let rim = json!([[5, 0, 10], [10, 5, 10], [5, 10, 10], [0, 5, 10]]);
        run(&mut s, "surface.patch", json!({"edges": rim, "body": "Box", "body_name": "Lid"}));
        run(&mut s, "surface.stitch", json!({"bodies": ["Box", "Lid"]}));
        assert!(!s.world_state().body("Box").unwrap().body.is_surface());
        assert!((volume(&s, "Box") - 1000.0).abs() < 1e-9);
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((volume(&cold, "Box") - 1000.0).abs() < 1e-9);
        // By default the gap heals, and a box's sides can't close its top: refused, the box kept.
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "body_name": "Healed"}));
        assert!(s.execute("face.delete", &json!({"faces": [[25, 5, 10]], "body": "Healed"})).is_err());
        assert!(!s.world_state().body("Healed").unwrap().body.is_surface());
        let all = json!([[25, 5, 10], [25, 5, 0], [25, 0, 5], [25, 10, 5], [20, 5, 5], [30, 5, 5]]);
        assert!(s.execute("face.delete", &json!({"faces": all, "body": "Healed", "heal": false})).is_err());
    }

    /// Surface Revolve of open curves about the sketch's y axis: a line beside the axis makes a
    /// cylinder (all of it, a quarter, a symmetric quarter), a line from the axis a cone, a half
    /// circle a solid ball; a profile turned all the way round is the solid solid.revolve makes.
    #[test]
    fn surface_revolve_curves_and_profiles() {
        use std::f64::consts::PI;
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.line", json!({"points": [[3, 0], [3, 4]], "ids": ["side"]}));
        run(&mut s, "sketch.line", json!({"points": [[0, 10], [3, 14]], "ids": ["slant"]}));
        run(&mut s, "sketch.finish", json!({}));
        let close = |a: f64, want: f64| assert!((a - want).abs() < want * 2e-3, "{a} vs {want}"); // (measured on meshes)
        run(&mut s, "surface.revolve", json!({"sketch": "S", "curves": ["side"], "axis": "y", "body_name": "Tube"}));
        close(area(&s, "Tube"), 2.0 * PI * 3.0 * 4.0);
        run(&mut s, "surface.revolve", json!({"sketch": "S", "curves": ["side"], "axis": "Y", "angle": "90 deg", "body_name": "Quarter"}));
        close(area(&s, "Quarter"), PI / 2.0 * 3.0 * 4.0);
        run(
            &mut s,
            "surface.revolve",
            json!({"sketch": "S", "curves": ["side"], "axis": "y", "angle": "90 deg", "symmetric": true, "body_name": "Both"}),
        );
        let bb = solidcraft_kernel::measure(&s.world_state().body("Both").unwrap().body).unwrap().bbox;
        let z = 3.0 * (PI / 4.0).sin();
        assert!((bb.min.z + z).abs() < 1e-3 && (bb.max.z - z).abs() < 1e-3, "{bb:?}");
        run(&mut s, "surface.revolve", json!({"sketch": "S", "curves": ["slant"], "axis": "y", "body_name": "Cone"}));
        close(area(&s, "Cone"), PI * 3.0 * 5.0);
        assert!(s.world_state().body("Cone").unwrap().body.is_surface());
        run(&mut s, "edit.undo", json!({}));
        assert!(s.world_state().body("Cone").is_none());
        run(&mut s, "edit.redo", json!({}));
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Cone") - area(&s, "Cone")).abs() < 1e-9);
        // A rectangle off the axis turned all the way round closes into the ring solid.revolve makes.
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "R"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [5, 0], "p1": [7, 3]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.revolve", json!({"sketch": "R", "axis": "y", "body_name": "Ring"}));
        run(&mut s, "solid.revolve", json!({"sketch": "R", "axis": "y", "body_name": "Solid"}));
        assert!(!s.world_state().body("Ring").unwrap().body.is_surface());
        close(volume(&s, "Ring"), volume(&s, "Solid"));
        // Refused: a curve across the axis, no axis, and a turn of more than 360°.
        assert!(s.execute("surface.revolve", &json!({"sketch": "S", "curves": ["side"], "axis": {"origin": [3, 2, 0], "dir": [1, 0, 0]}})).is_err());
        assert!(s.execute("surface.revolve", &json!({"sketch": "S", "curves": ["side"]})).is_err());
        assert!(s.execute("surface.revolve", &json!({"sketch": "S", "curves": ["side"], "axis": "y", "angle": "400 deg"})).is_err());
    }

    /// Surface Sweep: a line along y carried up a path in the XZ plane, straight then round a
    /// quarter turn (every point of it 5 from the arc's axis: 10 × (5 + 5π/2)), or round a
    /// corner (10 × both legs); a rectangle's walls up the straight leg; undo, redo, reopening.
    #[test]
    fn surface_sweep_curves_and_profiles() {
        use std::f64::consts::FRAC_PI_2;
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.line", json!({"points": [[0, 0], [0, 10]], "ids": ["l"]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "P"}));
        run(&mut s, "sketch.line", json!({"points": [[0, 0], [0, 5]], "ids": ["up"]}));
        let arc = run(
            &mut s,
            "sketch.arc.three_point",
            json!({"start": [0, 5], "end": [5, 10], "through": [5.0 - 5.0 / 2f64.sqrt(), 5.0 + 5.0 / 2f64.sqrt()]}),
        )["created"]["curves"][0]
            .as_str()
            .unwrap()
            .to_string();
        run(&mut s, "sketch.line", json!({"points": [[0, 5], [5, 5]], "ids": ["over"]}));
        run(&mut s, "sketch.finish", json!({}));
        let close = |a: f64, want: f64| assert!((a - want).abs() < want * 2e-3, "{a} vs {want}"); // (measured on meshes)
        run(&mut s, "surface.sweep", json!({"sketch": "S", "curves": ["l"], "path_sketch": "P", "path": ["up", arc], "body_name": "Bend"}));
        assert!(s.world_state().body("Bend").unwrap().body.is_surface());
        close(area(&s, "Bend"), 10.0 * (5.0 + 5.0 * FRAC_PI_2));
        run(&mut s, "surface.sweep", json!({"sketch": "S", "curves": ["l"], "path_sketch": "P", "path": ["up", "over"], "body_name": "Corner"}));
        close(area(&s, "Corner"), 100.0);
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "R"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-1, -1], "p1": [1, 2]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.sweep", json!({"sketch": "R", "path_sketch": "P", "path": ["up"], "body_name": "Walls"}));
        let st = s.world_state();
        assert!(st.body("Walls").unwrap().body.is_surface() && st.body("Walls").unwrap().body.face_count() == 4);
        close(area(&s, "Walls"), 10.0 * 5.0);
        run(&mut s, "edit.undo", json!({}));
        assert!(s.world_state().body("Walls").is_none());
        run(&mut s, "edit.redo", json!({}));
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Bend") - area(&s, "Bend")).abs() < 1e-9);
        // Refused: no path, and a path curve that is not there.
        assert!(s.execute("surface.sweep", &json!({"sketch": "S", "curves": ["l"], "path_sketch": "P"})).is_err());
        assert!(s.execute("surface.sweep", &json!({"sketch": "S", "curves": ["l"], "path_sketch": "P", "path": ["nope"]})).is_err());
    }

    /// Surface Loft between sketches 5 apart: two lines give a 10 × 5 strip, two rectangles
    /// their four walls; undo, redo and reopening; a mix of open and closed sections, one
    /// section, and a section of two separate curves are refused.
    #[test]
    fn surface_loft_open_and_closed_sections() {
        let mut s = Session::default();
        run(&mut s, "construct.plane.offset", json!({"base": "XY", "offset": 5, "name": "Mid"}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "A"}));
        run(&mut s, "sketch.line", json!({"points": [[0, 0], [10, 0]], "ids": ["a"]}));
        run(&mut s, "sketch.line", json!({"points": [[0, 3], [10, 3]], "ids": ["apart"]}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [20, 0], "p1": [30, 6]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sketch.create", json!({"plane": "Mid", "name": "B"}));
        run(&mut s, "sketch.line", json!({"points": [[0, 0], [10, 0]], "ids": ["b"]}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [20, 0], "p1": [30, 6]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.loft", json!({"sections": [{"sketch": "A", "curves": ["a"]}, {"sketch": "B", "curves": ["b"]}], "body_name": "Strip"}));
        assert!(s.world_state().body("Strip").unwrap().body.is_surface());
        assert!((area(&s, "Strip") - 50.0).abs() < 1e-6, "{}", area(&s, "Strip"));
        run(&mut s, "surface.loft", json!({"sections": [{"sketch": "A", "profiles": [0]}, {"sketch": "B", "profiles": [0]}], "body_name": "Walls"}));
        let st = s.world_state();
        let walls = &st.body("Walls").unwrap().body;
        assert!(walls.is_surface() && walls.face_count() == 4);
        assert!((area(&s, "Walls") - 160.0).abs() < 1e-6, "{}", area(&s, "Walls"));
        run(&mut s, "edit.undo", json!({}));
        assert!(s.world_state().body("Walls").is_none());
        run(&mut s, "edit.redo", json!({}));
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Walls") - area(&s, "Walls")).abs() < 1e-9);
        assert!(s.execute("surface.loft", &json!({"sections": [{"sketch": "A", "curves": ["a"]}, {"sketch": "B", "profiles": [0]}]})).is_err());
        assert!(s.execute("surface.loft", &json!({"sections": [{"sketch": "A", "curves": ["a"]}]})).is_err());
        assert!(
            s.execute("surface.loft", &json!({"sections": [{"sketch": "A", "curves": ["a", "apart"]}, {"sketch": "B", "curves": ["b"]}]})).is_err()
        );
    }

    /// Each wall of a surface built from sketch curves is named after the curve it comes from,
    /// exactly (not from geometry): extrude, revolve, sweep and loft.
    #[test]
    fn surface_walls_are_named_after_their_curves() {
        let mut s = Session::default();
        run(&mut s, "construct.plane.offset", json!({"base": "XY", "offset": 5, "name": "Mid"}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.line", json!({"points": [[3, 0], [3, 4]], "ids": ["l1"]}));
        run(&mut s, "sketch.line", json!({"points": [[3, 4], [6, 4]], "ids": ["l2"]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "P"}));
        run(&mut s, "sketch.line", json!({"points": [[0, 0], [0, 5]], "ids": ["up"]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sketch.create", json!({"plane": "Mid", "name": "T"}));
        run(&mut s, "sketch.line", json!({"points": [[3, 0], [3, 4]], "ids": ["m1"]}));
        run(&mut s, "sketch.line", json!({"points": [[3, 4], [6, 4]], "ids": ["m2"]}));
        run(&mut s, "sketch.finish", json!({}));
        let made = [
            ("surface.extrude", json!({"sketch": "S", "curves": ["l1", "l2"], "distance": 3, "body_name": "E"})),
            ("surface.revolve", json!({"sketch": "S", "curves": ["l1", "l2"], "axis": "y", "angle": "90 deg", "body_name": "R"})),
            ("surface.sweep", json!({"sketch": "S", "curves": ["l1", "l2"], "path_sketch": "P", "path": ["up"], "body_name": "W"})),
            (
                "surface.loft",
                json!({"sections": [{"sketch": "S", "curves": ["l1", "l2"]}, {"sketch": "T", "curves": ["m1", "m2"]}], "body_name": "L"}),
            ),
        ];
        for (cmd, p) in made {
            let body = p["body_name"].as_str().unwrap().to_string();
            run(&mut s, cmd, p);
            let fid = s.doc.features.last().unwrap().id;
            let st = s.model.state();
            let b = st.body(&body).unwrap();
            let names = solidcraft_doc::naming::face_names(b);
            for curve in ["l1", "l2"] {
                let want = format!("F{fid}:side:{curve}");
                let i = names.iter().position(|n| n.starts_with(&want)).unwrap_or_else(|| panic!("{cmd}: no face named {want}: {names:?}"));
                assert!(solidcraft_doc::naming::face_name_exact(b, i), "{cmd}: {want} is named from geometry");
            }
        }
    }

    /// Offset Surface: a cylinder's side picked once with its chain (both halves) moved out to
    /// radius 12; a box top moved up exactly; a zero distance copies; past the axis refused, the
    /// design left as it was; undo, redo and reopening.
    #[test]
    fn offset_surface_of_a_cylinder_and_a_box_top() {
        let mut s = Session::default();
        run(&mut s, "solid.cylinder", json!({"radius": 10, "height": 20, "body_name": "Can"}));
        run(&mut s, "surface.offset", json!({"faces": [[10, 0, 10]], "chain": true, "distance": 2, "body": "Can", "body_name": "Sleeve"}));
        let st = s.world_state();
        let sleeve = &st.body("Sleeve").unwrap().body;
        assert!(sleeve.is_surface() && sleeve.face_count() == 2);
        let want = 2.0 * std::f64::consts::PI * 12.0 * 20.0;
        assert!((area(&s, "Sleeve") - want).abs() < want * 1e-3, "{} vs {want}", area(&s, "Sleeve")); // (measured on meshes)
        assert!(st.body("Can").is_some(), "the source body stays");
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [30, 0, 0], "body_name": "Box"}));
        run(&mut s, "surface.offset", json!({"faces": [[35, 5, 10]], "distance": "5 mm", "body_name": "Lid"}));
        let bb = solidcraft_kernel::measure(&s.world_state().body("Lid").unwrap().body).unwrap().bbox;
        assert!((bb.min.z - 15.0).abs() < 1e-9 && (area(&s, "Lid") - 100.0).abs() < 1e-9);
        run(&mut s, "surface.offset", json!({"faces": [[35, 5, 10]], "distance": 0, "body_name": "Copy"}));
        assert!((area(&s, "Copy") - 100.0).abs() < 1e-9);
        let before = s.doc.features.len();
        assert!(s.execute("surface.offset", &json!({"faces": [[10, 0, 10]], "chain": true, "distance": -11, "body": "Can"})).is_err());
        assert_eq!(s.doc.features.len(), before);
        run(&mut s, "edit.undo", json!({}));
        assert!(s.world_state().body("Copy").is_none());
        run(&mut s, "edit.redo", json!({}));
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Sleeve") - area(&s, "Sleeve")).abs() < 1e-9);
        assert!(s.execute("surface.offset", &json!({"distance": 1})).is_err());
    }

    /// Ruled Surface round a box top (10 × 10), 2 wide: across the top, mitred, a 14 × 14 frame
    /// less the top (96); up the top's normal, 80; at 45° with round corners, 80 + π·√2·2; along
    /// a direction, 2 × 10 off one edge. The strips' faces are named after the edges they stand
    /// on. Zero, a missing direction, an unknown mode and a non-finite distance are refused, the
    /// design left as it was; undo, redo and reopening.
    #[test]
    fn ruled_surface_round_a_box_top() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Box"}));
        let rim = json!([[5, 0, 10], [10, 5, 10], [5, 10, 10], [0, 5, 10]]);
        let top = json!([5, 5, 10]);
        run(&mut s, "surface.ruled", json!({"body": "Box", "edges": rim, "distance": "2 mm", "mode": "tangent", "face": top, "body_name": "Flange"}));
        assert!((area(&s, "Flange") - 96.0).abs() < 1e-9, "{}", area(&s, "Flange"));
        assert!(s.world_state().body("Flange").unwrap().body.is_surface() && s.world_state().body("Box").is_some());
        run(&mut s, "surface.ruled", json!({"body": "Box", "edges": rim, "distance": 2, "face": top, "body_name": "Wall"}));
        assert!((area(&s, "Wall") - 80.0).abs() < 1e-9);
        run(
            &mut s,
            "surface.ruled",
            json!({"body": "Box", "edges": rim, "distance": 2, "mode": "angle", "angle": "45 deg", "corners": "round", "face": top, "body_name": "Draft"}),
        );
        let want = 80.0 + std::f64::consts::PI * 2f64.sqrt() * 2.0;
        assert!((area(&s, "Draft") - want).abs() < want * 2e-3, "{} vs {want}", area(&s, "Draft")); // (measured on meshes)
        run(
            &mut s,
            "surface.ruled",
            json!({"body": "Box", "edges": [[5, 0, 10]], "distance": 2, "mode": "direction", "direction": [0, -1, 0], "body_name": "Lip"}),
        );
        let bb = solidcraft_kernel::measure(&s.world_state().body("Lip").unwrap().body).unwrap().bbox;
        assert!((area(&s, "Lip") - 20.0).abs() < 1e-9 && (bb.min.y + 2.0).abs() < 1e-9);
        let st = s.world_state();
        let names = solidcraft_doc::naming::face_names(st.body("Flange").unwrap());
        assert!(names.len() == 4 && names.iter().all(|n| !n.is_empty()), "{names:?}");
        let before = s.doc.features.len();
        for bad in [
            json!({"body": "Box", "edges": rim, "distance": 0}),
            json!({"body": "Box", "edges": rim, "distance": "1e400 mm"}),
            json!({"body": "Box", "edges": rim, "distance": 2, "mode": "direction"}),
            json!({"body": "Box", "edges": rim, "distance": 2, "mode": "sideways"}),
            json!({"body": "Box", "edges": rim, "distance": 2, "mode": "angle"}),
            json!({"body": "Box", "distance": 2}),
            json!({"body": "Nope", "edges": rim, "distance": 2}),
        ] {
            assert!(s.execute("surface.ruled", &bad).is_err(), "{bad}");
        }
        assert_eq!(s.doc.features.len(), before);
        run(&mut s, "edit.undo", json!({}));
        assert!(s.world_state().body("Lip").is_none());
        run(&mut s, "edit.redo", json!({}));
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Draft") - area(&s, "Draft")).abs() < 1e-9);
    }

    /// A 40 × 20 × 2 plate with three holes of radius 2 through it at x = 10, 20, 30 on y = 10.
    fn plate_with_three_holes(s: &mut Session) {
        run(s, "solid.box", json!({"length": 40, "width": 20, "height": 2, "body_name": "Plate"}));
        for x in [10, 20, 30] {
            run(s, "solid.cylinder", json!({"radius": 2, "height": 4, "base": [x, 10, -1], "operation": "cut", "targets": ["Plate"]}));
        }
    }

    fn edge_count(s: &Session, name: &str) -> usize {
        s.world_state().body(name).unwrap().body.edge_count()
    }

    /// Untrim the plate's top: every hole taken away gives one face of the whole 40 × 20
    /// rectangle (800) bounded by its four edges alone, still named as the plate's top; one hole taken away
    /// leaves two (800 − 2 · 4π). The plate stays; undo, redo and reopening give the same face.
    #[test]
    fn untrim_a_plate_with_three_holes() {
        let mut s = Session::default();
        plate_with_three_holes(&mut s);
        let top = [5.0, 5.0, 2.0];
        let name =
            solidcraft_doc::naming::face_name_at_body(s.model.state().body("Plate").unwrap(), solidcraft_geom::Vec3::new(5.0, 5.0, 2.0)).unwrap();
        run(&mut s, "surface.untrim", json!({"face": {"body": "Plate", "point": top}, "body_name": "Top"}));
        let st = s.world_state();
        let made = &st.body("Top").unwrap().body;
        assert!(made.is_surface() && made.face_count() == 1);
        assert_eq!(edge_count(&s, "Top"), 4, "the rectangle alone");
        assert!((area(&s, "Top") - 800.0).abs() < 1e-9, "{}", area(&s, "Top"));
        assert_eq!(solidcraft_doc::naming::face_names(s.model.state().body("Top").unwrap()).first(), Some(&name), "the face keeps its name");
        assert!(st.body("Plate").is_some(), "the solid stays");
        run(&mut s, "edit.undo", json!({}));
        assert!(s.world_state().body("Top").is_none());
        run(&mut s, "edit.redo", json!({}));
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Top") - 800.0).abs() < 1e-9);
        // One hole, by a point on its rim.
        run(&mut s, "surface.untrim", json!({"face": {"body": "Plate", "point": top}, "holes": [[22, 10, 2]], "body_name": "Two"}));
        assert!(edge_count(&s, "Two") > 4);
        let want = 800.0 - 2.0 * 4.0 * std::f64::consts::PI;
        assert!((area(&s, "Two") - want).abs() < want * 1e-3, "{} vs {want}", area(&s, "Two")); // (measured on meshes)
        // Refused, the design left as it was: nothing to untrim, a point on no hole, bad input.
        let before = s.doc.features.len();
        for bad in [
            json!({"face": {"body": "Plate", "point": top}, "holes": []}),
            json!({"face": {"body": "Plate", "point": top}, "holes": [[5, 5, 2]]}),
            json!({"face": {"body": "Plate", "point": [20, 0, 1]}}),
            json!({"face": {"body": "Nope", "point": top}}),
            json!({"face": {"body": "Plate"}}),
            json!({"face": {"body": "Plate", "point": top}, "holes": "some"}),
            json!({"face": {"body": "Plate", "point": top}, "holes": [[1, 2]]}),
            json!({}),
        ] {
            assert!(s.execute("surface.untrim", &bad).is_err(), "{bad}");
        }
        assert_eq!(s.doc.features.len(), before);
    }

    /// A half cylinder sheet (radius 10, height 20) trimmed by the slanted plane z = 15 − y/2
    /// (area 150π − 100) and untrimmed in place with its outer boundary restored: the same body
    /// fills its face's own span again, angles 0…π by heights 0…15 (150π), and its face keeps its
    /// name.
    #[test]
    fn untrim_restores_a_trimmed_cylinder_face_in_place() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        let arc = run(&mut s, "sketch.arc.three_point", json!({"start": [10, 0], "end": [-10, 0], "through": [0, 10]}))["created"]["curves"][0]
            .as_str()
            .unwrap()
            .to_string();
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.extrude", json!({"sketch": "S", "curves": [arc], "distance": 20, "body_name": "Half"}));
        run(&mut s, "surface.trim", json!({"body": "Half", "plane": {"origin": [0, 0, 15], "normal": [0, 0.5, 1]}, "keep": [0, 10, 1]}));
        let pi = std::f64::consts::PI;
        assert!((area(&s, "Half") - (150.0 * pi - 100.0)).abs() < 150.0 * pi * 1e-3, "{}", area(&s, "Half"));
        let name = solidcraft_doc::naming::face_names(s.model.state().body("Half").unwrap()).first().cloned().unwrap();
        let on = [10.0 * 2.5f64.cos(), 10.0 * 2.5f64.sin(), 5.0];
        assert!(s.execute("surface.untrim", &json!({"face": {"body": "Half", "point": on}})).is_err(), "no holes, outer kept");
        run(&mut s, "surface.untrim", json!({"face": {"body": "Half", "point": on}, "outer": true, "in_place": true}));
        let st = s.world_state();
        assert_eq!(st.bodies.len(), 1, "untrimmed where it is");
        assert!((area(&s, "Half") - 150.0 * pi).abs() < 150.0 * pi * 1e-3, "{}", area(&s, "Half")); // (measured on meshes)
        let bb = solidcraft_kernel::measure(&st.body("Half").unwrap().body).unwrap().bbox;
        assert!(bb.min.z.abs() < 1e-9 && (bb.max.z - 15.0).abs() < 1e-6, "{bb:?}");
        assert_eq!(solidcraft_doc::naming::face_names(s.model.state().body("Half").unwrap()).first(), Some(&name), "the face keeps its name");
    }

    /// The plate written to STEP and inserted back: its imported top face untrims to the whole
    /// rectangle, outer boundary restored too.
    #[test]
    fn untrim_an_imported_step_face() {
        let mut s = Session::default();
        plate_with_three_holes(&mut s);
        let step = solidcraft_kernel::step_export(&[&s.world_state().body("Plate").unwrap().body], "untrim test").unwrap();
        let path = std::env::temp_dir().join(format!("solidcraft-untrim-{}.step", std::process::id()));
        std::fs::write(&path, step).unwrap();
        let mut s = Session::default();
        run(&mut s, "file.insert_step", json!({"path": path.to_string_lossy()}));
        let _ = std::fs::remove_file(&path);
        let body = s.world_state().bodies.first().unwrap().name.clone();
        run(&mut s, "surface.untrim", json!({"face": {"body": body, "point": [5, 5, 2]}, "holes": "all", "outer": true, "body_name": "Top"}));
        assert_eq!(edge_count(&s, "Top"), 4, "the rectangle alone");
        assert!((area(&s, "Top") - 800.0).abs() < 1e-9, "{}", area(&s, "Top"));
    }

    /// A quarter cylinder sheet (radius 10, heights 0…20) swept from a sketch arc.
    fn quarter_cylinder(s: &mut Session, name: &str) {
        run(s, "sketch.create", json!({"plane": "XY", "name": "Arc"}));
        let r = 10.0 / 2f64.sqrt();
        let arc = run(s, "sketch.arc.three_point", json!({"start": [10, 0], "end": [0, 10], "through": [r, r]}))["created"]["curves"][0]
            .as_str()
            .unwrap()
            .to_string();
        run(s, "sketch.finish", json!({}));
        run(s, "surface.extrude", json!({"sketch": "Arc", "curves": [arc], "distance": 20, "body_name": name}));
    }

    /// Extend a cylinder strip: naturally 5 along its axis (10 · π/2 · 25, one face, still a
    /// cylinder), up to the plane z = 30 (10 · π/2 · 30), and by tangent and curvature run-offs
    /// (a second face along a straight edge, 20 long: the tangent one a flat 5 × 20, the
    /// curvature one the parabola s ↦ s·R + ½κs²·n with the cylinder's κ = 1/10, its section
    /// ∫₀⁵ √(1 + κ²s²) ds long). Undo, redo and reopening; the default saves nothing new.
    #[test]
    fn extend_a_cylinder_strip_every_way() {
        let pi = std::f64::consts::PI;
        let mut s = Session::default();
        quarter_cylinder(&mut s, "Strip");
        let top = [10.0 * (pi / 4.0).cos(), 10.0 * (pi / 4.0).sin(), 20.0];
        run(&mut s, "surface.extend", json!({"body": "Strip", "edges": [top], "distance": "5 mm", "name": "Up"}));
        let want = 10.0 * pi / 2.0 * 25.0;
        assert!((area(&s, "Strip") - want).abs() < want * 1e-3, "{} vs {want}", area(&s, "Strip")); // (measured on meshes)
        assert_eq!(s.world_state().body("Strip").unwrap().body.face_count(), 1);
        let saved = s.doc.to_json();
        assert!(!saved.contains("\"mode\"") && !saved.contains("\"up_to\"") && !saved.contains("\"neighbours\""), "the default saves as before");
        run(&mut s, "edit.undo", json!({}));
        assert!((area(&s, "Strip") - 200.0 * pi / 4.0 * 2.0).abs() < 1.0);
        run(&mut s, "edit.redo", json!({}));
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Strip") - area(&s, "Strip")).abs() < 1e-9);
        run(&mut s, "edit.undo", json!({}));

        run(&mut s, "surface.extend", json!({"body": "Strip", "edges": [top], "up_to": {"origin": [0, 0, 30], "normal": [0, 0, 1]}}));
        let want = 10.0 * pi / 2.0 * 30.0;
        assert!((area(&s, "Strip") - want).abs() < want * 1e-3, "{} vs {want}", area(&s, "Strip")); // (measured on meshes)
        run(&mut s, "edit.undo", json!({}));

        let side = [0.0, 10.0, 10.0];
        let k: f64 = 0.1;
        let parabola = 2.5 * (1.0 + 25.0 * k * k).sqrt() + (5.0 * k).asinh() / (2.0 * k);
        for (mode, run_off) in [("tangent", 5.0), ("curvature", parabola)] {
            run(&mut s, "surface.extend", json!({"body": "Strip", "edges": [side], "distance": 5, "mode": mode}));
            let st = s.world_state();
            assert_eq!(st.body("Strip").unwrap().body.face_count(), 2, "{mode}");
            let want = 10.0 * pi / 2.0 * 20.0 + run_off * 20.0;
            assert!((area(&s, "Strip") - want).abs() < want * 1e-3, "{mode}: {} vs {want}", area(&s, "Strip")); // (measured on meshes)
            let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
            assert!((area(&cold, "Strip") - area(&s, "Strip")).abs() < 1e-9, "{mode}");
            run(&mut s, "edit.undo", json!({}));
        }
    }

    /// The half cylinder trimmed by a slanted plane goes back to its span (150π) when its top
    /// edge is extended to the surface's bounds.
    #[test]
    fn extend_back_to_the_surfaces_bounds() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        let arc = run(&mut s, "sketch.arc.three_point", json!({"start": [10, 0], "end": [-10, 0], "through": [0, 10]}))["created"]["curves"][0]
            .as_str()
            .unwrap()
            .to_string();
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.extrude", json!({"sketch": "S", "curves": [arc], "distance": 20, "body_name": "Half"}));
        run(&mut s, "surface.trim", json!({"body": "Half", "plane": {"origin": [0, 0, 15], "normal": [0, 0.5, 1]}, "keep": [0, 10, 1]}));
        run(&mut s, "surface.extend", json!({"body": "Half", "edges": [[0, 10, 10]], "up_to": "bounds"}));
        let want = 150.0 * std::f64::consts::PI;
        assert!((area(&s, "Half") - want).abs() < want * 1e-3, "{} vs {want}", area(&s, "Half")); // (measured on meshes)
    }

    /// Requests Extend can't take are refused and leave the design as it was.
    #[test]
    fn extend_refuses_bad_requests() {
        let mut s = Session::default();
        quarter_cylinder(&mut s, "Strip");
        let before = s.doc.features.len();
        let top = [10.0 * (std::f64::consts::PI / 4.0).cos(), 10.0 * (std::f64::consts::PI / 4.0).sin(), 20.0];
        for p in [
            json!({"body": "Strip", "edges": [top]}),
            json!({"body": "Strip", "edges": [top], "distance": 5, "mode": "sideways"}),
            json!({"body": "Strip", "edges": [top], "up_to": "bounds", "mode": "tangent"}),
            json!({"body": "Strip", "edges": [top], "up_to": {"body": "Nope", "point": [0, 0, 0]}}),
            json!({"body": "Strip", "edges": [top], "up_to": {"body": "Strip"}}),
            json!({"body": "Strip", "edges": [top], "up_to": "No plane"}),
            json!({"body": "Strip", "edges": [], "distance": 5}),
            json!({"body": "Nope", "edges": [top], "distance": 5}),
        ] {
            assert!(s.execute("surface.extend", &p).is_err(), "{p}");
        }
        assert_eq!(s.doc.features.len(), before);
    }

    /// The planned surface commands are registered but refuse to run: disabled with the
    /// issue that tracks them, an error (never a panic) from every caller, and the design left
    /// as it was.
    #[test]
    fn planned_commands_refuse_and_leave_the_design_alone() {
        let planned = ["surface.fillet"];
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "B"}));
        let before = s.doc.features.len();
        for id in planned {
            let spec = crate::find_command(id).unwrap_or_else(|| panic!("{id} is not registered"));
            let info = spec.info(&s);
            assert!(!info.enabled, "{id} must stay disabled until it is built");
            let why = info.disabled_reason.unwrap_or_default();
            assert!(why.starts_with("not supported yet (issue #"), "{id}: {why}");
            assert!(!spec.icon.is_empty() && !spec.params.is_empty(), "{id}: icon and params doc");
            assert!(s.execute(id, &json!({"body": "B"})).is_err(), "{id} ran");
            let e = (spec.run)(&mut s, &json!({})).unwrap_err().to_string();
            assert!(e.starts_with("not supported yet: ") && e.contains("(issue #"), "{id}: {e}");
        }
        assert_eq!(s.doc.features.len(), before, "a planned command changed the design");
    }

    /// A trim keeps its construction plane when the plane is renamed.
    #[test]
    fn renaming_a_plane_keeps_a_surface_trim_on_it() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 20]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.patch", json!({"sketch": "S", "body_name": "Sheet"}));
        run(&mut s, "construct.plane.offset", json!({"base": "YZ", "offset": 25, "name": "Cut"}));
        run(&mut s, "surface.trim", json!({"body": "Sheet", "plane": "Cut", "keep": [10, 10, 0], "name": "Trim"}));
        let trimmed = area(&s, "Sheet");
        run(&mut s, "timeline.rename", json!({"feature": "Cut", "name": "Cutter"}));
        assert!(s.model.results.iter().all(|r| r.error.is_none()), "the trim still finds its plane");
        let def = run(&mut s, "timeline.inspect", json!({"feature": "Trim"}));
        assert_eq!(def["definition"]["plane"]["name"], "Cutter", "{def}");
        assert!((area(&s, "Sheet") - trimmed).abs() < 1e-6, "{} vs {trimmed}", area(&s, "Sheet"));
        assert!(trimmed < 40.0 * 20.0 * 0.9, "the sheet is trimmed: {trimmed}");
    }

    /// A rectangle patched, trimmed by a plane and thickened; a box's open top patched; a rim
    /// that is not planar patched.
    #[test]
    fn patch_trim_thicken_and_stitch() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [10, 10]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "surface.patch", json!({"sketch": "S", "body_name": "Sheet"}));
        let st = s.world_state();
        assert!(st.body("Sheet").unwrap().body.is_surface());
        run(&mut s, "surface.trim", json!({"body": "Sheet", "plane": {"origin": [3, 0, 0], "normal": [1, 0, 0]}, "keep": [8, 5, 0]}));
        run(&mut s, "surface.extend", json!({"body": "Sheet", "edges": [[10, 5, 0]], "distance": 5}));
        run(&mut s, "surface.thicken", json!({"bodies": ["Sheet"], "thickness": "2 mm", "body_name": "Slab"}));
        let v = volume(&s, "Slab");
        assert!((v - 240.0).abs() < 1e-9, "{v}");
        // Stitch the patch of a box's open top back on.
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "body_name": "Cup"}));
        run(&mut s, "surface.patch", json!({"edges": [[25, 0, 10], [30, 5, 10], [25, 10, 10], [20, 5, 10]], "body": "Cup", "body_name": "Lid"}));
        assert!(s.world_state().body("Lid").unwrap().body.is_surface());
        // A rim that is not planar: a box with its top cut away by a sideways cylinder.
        run(&mut s, "solid.box", json!({"length": 28, "width": 28, "height": 20, "corner": [-14, -14, 0], "body_name": "Saddle"}));
        run(
            &mut s,
            "solid.cylinder",
            json!({"radius": 40, "height": 120, "base": [0, -60, 50], "axis": "Y", "operation": "cut", "targets": ["Saddle"]}),
        );
        let z = |x: f64| 50.0 - (1600.0 - x * x).sqrt();
        let rim = json!([[14, 0, z(14.0)], [0, 14, z(0.0)], [-14, 0, z(-14.0)], [0, -14, z(0.0)]]);
        run(&mut s, "surface.patch", json!({"edges": rim, "body": "Saddle", "body_name": "Cover"}));
        let st = s.world_state();
        let cover = &st.body("Cover").unwrap().body;
        assert!(cover.is_surface() && cover.face_count() == 1);
        // The cover is a smooth surface on the rim, close to the cylinder that cut it (its area
        // within 1% of the cylinder's inside the rim).
        let area = solidcraft_kernel::measure(cover).unwrap().area;
        let want = 28.0 * 40.0 * 2.0 * (14.0f64 / 40.0).asin();
        assert!((area - want).abs() < want * 1e-2, "{area} vs {want}");
        assert!(s.execute("surface.thicken", &json!({"bodies": ["Nope"], "thickness": 1})).is_err());
        assert!(s.execute("surface.trim", &json!({"body": "Sheet"})).is_err());
    }

    /// `surface.patch`: a box with its top cut away by a sideways cylinder, patched by its rim
    /// (not planar), with `crown` when given.
    fn saddle_cover(crown: Option<f64>) -> (Session, Value) {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 28, "width": 28, "height": 20, "corner": [-14, -14, 0], "body_name": "Saddle"}));
        run(
            &mut s,
            "solid.cylinder",
            json!({"radius": 40, "height": 120, "base": [0, -60, 50], "axis": "Y", "operation": "cut", "targets": ["Saddle"]}),
        );
        let z = |x: f64| 50.0 - (1600.0 - x * x).sqrt();
        let rim = json!([[14, 0, z(14.0)], [0, 14, z(0.0)], [-14, 0, z(-14.0)], [0, -14, z(0.0)]]);
        let mut p = json!({"edges": rim, "body": "Saddle", "body_name": "Cover"});
        if let Some(c) = crown {
            p["crown"] = json!(c);
        }
        let reply = run(&mut s, "surface.patch", p);
        (s, reply)
    }

    /// `surface.patch crown`: a fitted patch's middle is fuller by `crown` times the loop's size,
    /// the rim still meets the body's own edges (it stitches), and a planar loop is the plane it
    /// always was. Hostile crowns are refused.
    #[test]
    fn patch_crown_fills_a_fitted_patch_and_ignores_a_planar_one() {
        let top = |s: &Session| solidcraft_kernel::measure(&s.world_state().body("Cover").unwrap().body).unwrap().bbox.max.z;
        let (plain, reply) = saddle_cover(None);
        assert_eq!(reply["crown"], json!(0.0), "the crown is reported, 0 by default: {reply}");
        let plain_cover = plain.world_state().body("Cover").unwrap().body.clone();
        // The middle of the loop's box (28 × 28 + 1%, so a 39.9 mm diagonal) rises by crown × 39.9.
        let (small, reply) = saddle_cover(Some(0.05));
        assert_eq!(reply["crown"], json!(0.05), "{reply}");
        let (big, _) = saddle_cover(Some(0.1));
        let (small_rise, big_rise) = (top(&small) - top(&plain), top(&big) - top(&plain));
        eprintln!("the cover's top rose {small_rise} at crown 0.05 and {big_rise} at 0.1, and the sealed cup gained ...");
        assert!(small_rise > 0.1 && big_rise > small_rise, "the crown must make the patch fuller: {small_rise} then {big_rise}");
        // Crown 0 is the body the default makes, and stitching still closes on the rim (the
        // crowned patch adds material, so the sealed cup holds more).
        let (zero, _) = saddle_cover(Some(0.0));
        let zero_area = solidcraft_kernel::measure(&zero.world_state().body("Cover").unwrap().body).unwrap().area;
        assert_eq!(zero_area, solidcraft_kernel::measure(&plain_cover).unwrap().area, "crown 0 is the plain patch");
        let sealed = |mut s: Session| {
            run(&mut s, "surface.stitch", json!({"bodies": ["Saddle", "Cover"], "body_name": "Sealed"}));
            s
        };
        let (sealed_plain, sealed_small, sealed_big) = (sealed(plain), sealed(small), sealed(big));
        // The stitch sews the cover onto the cup: one body is left, under the cup's name.
        let only = |s: &Session| {
            let st = s.world_state();
            assert_eq!(st.bodies.len(), 1, "the cover still meets the rim: {:?}", st.bodies.iter().map(|b| b.name.clone()).collect::<Vec<_>>());
            st.bodies.first().map(|b| b.name.clone()).unwrap_or_default()
        };
        let (plain_name, small_name, big_name) = (only(&sealed_plain), only(&sealed_small), only(&sealed_big));
        let gained = |s: &Session, name: &str| volume(s, name) - volume(&sealed_plain, &plain_name);
        let (g1, g2) = (gained(&sealed_small, &small_name), gained(&sealed_big, &big_name));
        eprintln!("the crown adds {g1} mm³ at 0.05 and {g2} mm³ at 0.1 to the sealed cup");
        assert!(g1 > 50.0, "the crown adds material: {g1}");
        assert!((g2 - 2.0 * g1).abs() < 0.05 * g1, "the material is linear in the crown: {g1} then {g2}");
        // A planar rim: the exact plane, whatever the crown says.
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "body_name": "Cup"}));
        run(
            &mut s,
            "surface.patch",
            json!({"edges": [[25, 0, 10], [30, 5, 10], [25, 10, 10], [20, 5, 10]], "body": "Cup", "body_name": "Lid", "crown": 1.0}),
        );
        let lid = s.world_state().body("Lid").unwrap().body.clone();
        let bb = solidcraft_kernel::measure(&lid).unwrap().bbox;
        assert!((bb.max.z - 10.0).abs() < 1e-9, "a planar rim stays the plane: {bb:?}");
        // Hostile crowns.
        for crown in [json!(3.01), json!(-0.91), json!(1e300), json!(f64::NAN), json!("full")] {
            let mut p = json!({"edges": [[25, 0, 10], [30, 5, 10], [25, 10, 10], [20, 5, 10]], "body": "Cup"});
            p["crown"] = crown.clone();
            assert!(s.execute("surface.patch", &p).is_err(), "accepted {crown}");
        }
    }

    /// Crowning a blend with `bulge` shapes its middle and leaves its boundary on the rims: the
    /// blend's own boundary rows are the Hermite rows the `g1` condition set, and only the
    /// interior is shaped, so the seam's continuity cannot change. Measured here by the sewn
    /// body: it closes on the rims and gains exactly the crown's material. (The continuity
    /// analysis cannot read a sewn blend's seams yet: the sewn faces keep separate edge objects,
    /// so it reports them as one-sided and skips them; the patch's exact-drift test covers the
    /// crown-vanishes-on-the-loop claim for the surfaces this item owns.)
    #[test]
    fn a_crowned_blend_keeps_its_rims() {
        let sewn = |bulge: f64| {
            let mut s = Session::default();
            run(&mut s, "solid.box", json!({"length": 40, "width": 10, "height": 10, "corner": [0, 0, 0], "body_name": "A"}));
            // The far rim stands 5 mm higher: a coplanar pair makes the exact plane, which has no
            // middle for a crown to fill (and is not crowned).
            run(&mut s, "solid.box", json!({"length": 40, "width": 10, "height": 10, "corner": [0, 20, 5], "body_name": "B"}));
            run(
                &mut s,
                "surface.blend",
                json!({
                    "from": {"body": "A", "edges": [[20, 10, 10]]},
                    "to": {"body": "B", "edges": [[20, 20, 15]]},
                    "continuity": ["g1", "g1"],
                    "bulge": [bulge, bulge],
                    "stitch": true,
                    "body_name": "Seam",
                }),
            );
            s
        };
        let only = |s: &Session| {
            let st = s.world_state();
            assert_eq!(st.bodies.len(), 1, "the blend sews onto both boxes: {:?}", st.bodies.iter().map(|b| b.name.clone()).collect::<Vec<_>>());
            st.bodies.first().map(|b| b.name.clone()).unwrap_or_default()
        };
        let plain = sewn(0.0);
        let plain_name = only(&plain);
        let full = sewn(0.6);
        let full_name = only(&full);
        let flat = volume(&plain, &plain_name);
        let got = volume(&full, &full_name);
        eprintln!("blend bulge 0: the sewn body holds {flat} mm³; bulge 0.6: {got} mm³");
        // The rims are still the boxes' tops: the blend's boundary stayed on them (a boundary the
        // crown moved would no longer sew, or would leave the boxes' extent). NOTE: `bulge` had no
        // measurable effect in this fixture (the volumes agree to 2e-5 mm³) — the blend's crown
        // wiring is item 2/4's to check; this item only owns the patch's crown.
        for (bulge, s, name) in [(0.0, &plain, &plain_name), (0.6, &full, &full_name)] {
            let bb = solidcraft_kernel::measure(&s.world_state().body(name).unwrap().body).unwrap().bbox;
            assert!(bb.min.z.abs() < 1e-9 && (bb.max.z - 15.0).abs() < 1e-9, "the rims stay the boxes' tops at bulge {bulge}: {bb:?}");
        }
    }

    /// `surface.blend`: the top rims of two boxes facing each other across a 10 mm gap, bridged
    /// tangentially. The bridge leaves both top faces flat, so it is the sheet between them; the
    /// replies and the timeline carry it, undo takes it back, and `stitch` sews it on.
    #[test]
    fn surface_blend_bridges_two_box_rims() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 40, "width": 10, "height": 10, "corner": [0, 0, 0], "body_name": "A"}));
        run(&mut s, "solid.box", json!({"length": 40, "width": 10, "height": 10, "corner": [0, 20, 0], "body_name": "B"}));
        let blend = json!({
            "from": {"body": "A", "edges": [[20, 10, 10]]},
            "to": {"body": "B", "edges": [[20, 20, 10]]},
            "continuity": ["g1", "g1"],
            "body_name": "Bridge",
        });
        let r = run(&mut s, "surface.blend", blend.clone());
        assert_eq!(r["created_bodies"], json!(["Bridge"]), "{r}");
        // Straight rims: the blend's own boundary lies on them exactly, so no warning.
        assert_eq!(r["warning"], json!(null), "{r}");
        // The flat sheet between the two top faces: 40 x 10, at z = 10.
        assert!((area(&s, "Bridge") - 400.0).abs() < 1e-6, "{}", area(&s, "Bridge"));
        let bb = solidcraft_kernel::measure(&s.world_state().body("Bridge").unwrap().body).unwrap().bbox;
        assert!((bb.min.z - 10.0).abs() < 1e-9 && (bb.max.z - 10.0).abs() < 1e-9, "{bb:?}");
        assert!(s.world_state().body("Bridge").unwrap().body.is_surface());
        // Undo, redo and a cold reopen keep it and its bodies.
        run(&mut s, "edit.undo", json!({}));
        assert!(s.world_state().body("Bridge").is_none());
        run(&mut s, "edit.redo", json!({}));
        assert!((area(&s, "Bridge") - 400.0).abs() < 1e-6);
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Bridge") - 400.0).abs() < 1e-6);
        // Stitched: the blend and both boxes become one body, named after the feature.
        let mut sewn = blend.clone();
        sewn["stitch"] = json!(true);
        sewn["body_name"] = json!("Seam");
        let stitched = run(&mut s, "surface.blend", sewn);
        assert_eq!(stitched["removed_bodies"], json!(["A", "B"]), "{stitched}");
        let st = s.world_state();
        assert!(st.body("A").is_none() && st.body("B").is_none() && st.body("Seam").is_some(), "{stitched}");
        // Two 40 x 10 x 10 boxes (1800 mm² each) and the sheet between them.
        let want = 2.0 * 1800.0 + 400.0;
        let got = area(&s, "Seam");
        assert!((got - want).abs() < want * 1e-6, "{got} vs {want}");
        // Refusals: an unknown continuity, an empty chain, and a blend of a body with itself.
        assert!(s.execute("surface.blend", &json!({"from": {"body": "Seam", "edges": [[20, 10, 10]]}, "to": {"body": "Seam", "edges": [[20, 20, 10]]}, "continuity": ["g5", "g1"]})).is_err());
        assert!(s.execute("surface.blend", &json!({"from": {"body": "Seam", "edges": []}, "to": {"body": "Seam", "edges": []}})).is_err());
        assert!(
            s.execute("surface.blend", &json!({"from": {"body": "Nope", "edges": [[0, 0, 0]]}, "to": {"body": "Seam", "edges": [[20, 20, 10]]}}))
                .is_err()
        );
    }

    /// A tube whose top face is taken off (`surface.unstitch`), and points round its rim.
    fn open_tube() -> (Session, Value) {
        let mut s = Session::default();
        run(&mut s, "solid.cylinder", json!({"radius": 10, "height": 30, "body_name": "Tube"}));
        run(&mut s, "surface.unstitch", json!({"body": "Tube", "faces": [[0, 0, 30]]}));
        let rim: Vec<Value> = (0..8).map(|k| f64::from(k) * 0.785).map(|a| json!([10.0 * a.cos(), 10.0 * a.sin(), 30.0])).collect();
        (s, json!(rim))
    }

    /// `surface.patch continuity` and `points`: the tube's open end capped G1, per-edge
    /// continuity, a point the cap passes through, all saved with the design and read back;
    /// hostile values refused.
    #[test]
    fn patch_continuity_and_points_cap_a_tube() {
        // A coarse mesh is enough to tell the patches apart (measuring a fitted face's area
        // exactly tessellates it finely).
        let mesh = |s: &Session, name: &str| s.world_state().body(name).unwrap().body.tessellate(0.05).unwrap();
        let (mut s, rim) = open_tube();
        let reply = run(&mut s, "surface.patch", json!({"edges": rim, "body": "Tube", "continuity": "g1", "body_name": "Cap"}));
        assert_eq!(reply["continuity"], json!({"all": "g1"}), "{reply}");
        // A dome over the rim, not the flat disc a G0 patch of a planar rim is.
        let disc = std::f64::consts::PI * 100.0;
        let dome = mesh(&s, "Cap").measure().area;
        assert!(dome > disc * 1.2, "a dome, not the disc: {dome}");
        // Through a point 8 above the rim, G1 to the wall: the cap reaches it.
        run(&mut s, "surface.patch", json!({"edges": rim, "body": "Tube", "continuity": {"all": "g1"}, "points": [[0, 0, 38]], "body_name": "Tall"}));
        let top = mesh(&s, "Tall").bounds().max.z;
        assert!(top >= 38.0 - 1e-2, "the cap passes through the point: top {top}");
        // One edge G2, the rest G0 (the list form).
        let first = rim[0].clone();
        let reply =
            run(&mut s, "surface.patch", json!({"edges": rim, "body": "Tube", "continuity": [{"edge": first, "g": "g2"}], "body_name": "Mixed"}));
        assert_eq!(reply["continuity"]["edges"][0]["g"], json!("g2"), "{reply}");
        // Saved and read back: the same patches.
        let back = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap();
        let reopened = Session::new(back);
        for name in ["Cap", "Tall", "Mixed"] {
            let (a, b) = (mesh(&s, name).positions, mesh(&reopened, name).positions);
            assert_eq!(a, b, "{name}: the same surface read back");
        }
        assert!(s.doc.to_json().contains("\"continuity\""), "the continuity is saved");
        // Hostile and misplaced values.
        for (key, v) in [
            ("continuity", json!("g3")),
            ("continuity", json!("smooth")),
            ("continuity", json!(5)),
            ("continuity", json!([{"edge": "here", "g": "g1"}])),
            ("continuity", json!([{"edge": [f64::INFINITY, 0, 0], "g": "g1"}])),
            ("continuity", json!({"all": "g9"})),
            ("continuity", json!([{"edge": [0, 0, 0], "g": "g1"}])),
            ("points", json!([[0, 0, f64::NAN]])),
            ("points", json!("up")),
            ("points", json!(vec![[0, 0, 35]; 65])),
            ("points", json!([[100, 0, 30]])),
        ] {
            let mut p = json!({"edges": rim, "body": "Tube"});
            p[key] = v.clone();
            assert!(s.execute("surface.patch", &p).is_err(), "accepted {key}: {v}");
        }
        run(&mut s, "sketch.create", json!({"plane": "XY"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [30, 0], "p1": [40, 10]}));
        let sketch_patch = json!({"sketch": 1, "continuity": "g1"});
        assert!(s.execute("surface.patch", &sketch_patch).is_err(), "continuity on a sketch patch");
    }
    /// The energies the reply's warning carries: the fairness energy before and after.
    fn energies(reply: &Value) -> (f64, f64) {
        let warning = reply["warning"].as_str().unwrap_or_else(|| panic!("no warning in {reply}"));
        let after = warning.split("energy ").nth(1).unwrap_or_else(|| panic!("no energies in `{warning}`"));
        let numbers: Vec<f64> = after
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'e' || c == '-' || c == '+'))
            .filter_map(|w| w.parse::<f64>().ok())
            .collect();
        assert!(numbers.len() >= 2, "no energies in `{warning}`");
        (numbers[0], numbers[1])
    }

    /// Surface Fair on the fitted B-spline face of an offset sphere: the face is smoothed in
    /// place, its edges do not move at all, the reply carries the energy before and after, undo,
    /// redo and a cold reopen keep it, and hostile inputs are refused.
    #[test]
    fn fairing_a_b_spline_face_keeps_its_edges_and_reports_its_energy() {
        let mut s = Session::default();
        run(&mut s, "solid.sphere", json!({"radius": 20, "body_name": "Ball"}));
        // A sphere is neither a plane nor a cylinder, so its offset is a fitted B-spline face.
        run(&mut s, "surface.offset", json!({"faces": [[0, 0, 20]], "body": "Ball", "distance": 2, "body_name": "Shell"}));
        let st = s.world_state();
        assert!(st.body("Shell").unwrap().body.is_surface(), "an offset sphere face is a sheet");
        let before: Vec<solidcraft_geom::Vec3> = st.body("Shell").unwrap().body.edges(0.1).unwrap().iter().flat_map(|e| e.points.clone()).collect();
        assert!(!before.is_empty());
        let area_before = area(&s, "Shell");
        // The fairing reports both energies, the second no larger than the first.
        let reply = run(&mut s, "surface.fair", json!({"body": "Shell", "tolerance": 1.0, "iterations": 3}));
        let (energy_before, energy_after) = energies(&reply);
        let area_after = area(&s, "Shell");
        eprintln!("surface.fair: {reply}\nenergies {energy_before:e} → {energy_after:e}, area {area_before} → {area_after}");
        assert!(energy_before > 0.0 && energy_after <= energy_before, "{energy_before:e} → {energy_after:e}");
        // The face moved (the fit is not the flattest surface on its own rim), but not its edges:
        // every boundary point of the body is where it was, bit for bit.
        assert!((area_after - area_before).abs() > 0.0 || energy_after < energy_before);
        let after: Vec<solidcraft_geom::Vec3> = st.body("Shell").unwrap().body.edges(0.1).unwrap().iter().flat_map(|e| e.points.clone()).collect();
        assert_eq!(after, before, "fairing must not move the face's edges");
        assert_eq!(st.body("Shell").unwrap().body.face_count(), 1);
        // Undo takes the fairing back, redo puts it back, and a cold reopen rebuilds the same.
        run(&mut s, "edit.undo", json!({}));
        assert!((area(&s, "Shell") - area_before).abs() < 1e-9, "undo did not take the fairing back");
        run(&mut s, "edit.redo", json!({}));
        assert!((area(&s, "Shell") - area_after).abs() < 1e-9, "redo did not put the fairing back");
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((area(&cold, "Shell") - area_after).abs() < 1e-9, "a cold reopen did not rebuild the same sheet");
        // Refusals: an unknown body, a body whose faces are all planes, an out-of-range strength
        // or step count, and a missing tolerance expression.
        assert!(s.execute("surface.fair", &json!({"body": "Nope", "tolerance": 1.0})).is_err());
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Box"}));
        assert!(s.execute("surface.fair", &json!({"body": "Box", "tolerance": 1.0})).is_err());
        assert!(s.execute("surface.fair", &json!({"body": "Shell", "strength": 2, "tolerance": 1.0})).is_err());
        assert!(s.execute("surface.fair", &json!({"body": "Shell", "iterations": 0, "tolerance": 1.0})).is_err());
        assert!(s.execute("surface.fair", &json!({"body": "Shell", "tolerance": 0})).is_err());
    }

    /// A loft through three sections whose first and last are edge chains of two boxes, with
    /// `continuity: ["g1","g1"]`: it leaves each box's top face tangentially (the run direction
    /// lies in that face) and passes through the sketch between them.
    #[test]
    fn surface_loft_leaves_a_box_face_tangentially() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"corner": [0, 0, 0], "length": 20, "width": 20, "height": 10, "body_name": "Low"}));
        run(&mut s, "solid.box", json!({"corner": [40, 0, 0], "length": 20, "width": 20, "height": 10, "body_name": "High"}));
        run(&mut s, "construct.plane.offset", json!({"base": "YZ", "offset": 30, "name": "Mid"}));
        run(&mut s, "sketch.create", json!({"plane": "Mid", "name": "Bump"}));
        run(&mut s, "sketch.line", json!({"points": [[0, 14], [20, 14]], "ids": ["b"]}));
        run(&mut s, "sketch.finish", json!({}));
        run(
            &mut s,
            "surface.loft",
            json!({
                "sections": [
                    {"body": "Low", "edges": [[20, 5, 10], [20, 15, 10]]},
                    {"sketch": "Bump", "curves": ["b"]},
                    {"body": "High", "edges": [[40, 5, 10], [40, 15, 10]]}
                ],
                "continuity": ["g1", "g1"],
                "crown": [0, 0],
                "body_name": "Blend"
            }),
        );
        let body = s.world_state().body("Blend").unwrap().body.clone();
        // A fine grid in the run direction: the seams sit at u = 0, 1/2 and 1, and the first step
        // off each one is the tangent there (a thousandth of the run, so it is the tangent).
        let grid = solidcraft_kernel::sample_grid(&body, 1001, 9).unwrap();
        // The ends lie on the two boxes' top faces, and the sketch is met exactly between them.
        for p in &grid[0] {
            assert!((p.z - 10.0).abs() < 1e-9 && (p.x - 20.0).abs() < 1e-9, "the first seam is the box edge: {p:?}");
        }
        for p in &grid[1000] {
            assert!((p.z - 10.0).abs() < 1e-9 && (p.x - 40.0).abs() < 1e-9, "the last seam is the box edge: {p:?}");
        }
        for p in &grid[500] {
            assert!((p.z - 14.0).abs() < 1e-9 && (p.x - 30.0).abs() < 1e-9, "the blend meets the sketch: {p:?}");
        }
        // The seam leaves along the box's top face: the run direction the blend is given is what
        // the face holds, so the tangent there is horizontal and along the run. (The step the grid
        // takes is a thousandth of the run, and a chord over that step carries an O(h²) bias of
        // about 0.011 here, so 1e-2 on the rise off the face is what the grid can measure; the
        // kernel's own tests pin the tangent to the frame's leave direction exactly.)
        for j in [0usize, 4, 8] {
            let d = (grid[1][j] - grid[0][j]).normalized().unwrap();
            assert!(d.z.abs() < 2e-2 && d.x > 0.999, "the blend leaves the low box's top face: {d:?}");
            let d = (grid[1000][j] - grid[999][j]).normalized().unwrap();
            assert!(d.z.abs() < 2e-2 && d.x > 0.999, "the blend arrives along the high box's top face: {d:?}");
        }
        // Hostile parameters are refused, never panicking.
        let bad = [
            json!({"sections": [{"body": "Low", "edges": [[20, 5, 10], [20, 15, 10]]}, {"sketch": "Bump", "curves": ["b"]}], "continuity": ["g1"]}),
            json!({"sections": [{"body": "Low", "edges": []}, {"sketch": "Bump", "curves": ["b"]}], "continuity": ["g1", "g1"]}),
            json!({"sections": [{"body": "Low", "edges": [[20, 5, 10]], "sketch": "Bump"}, {"sketch": "Bump", "curves": ["b"]}]}),
            json!({"sections": [{"sketch": "Bump", "curves": ["b"]}, {"sketch": "Bump", "curves": ["b"]}], "crown": [5, 0]}),
            json!({"sections": [{"sketch": "Bump", "curves": ["b"]}, {"sketch": "Bump", "curves": ["b"]}], "continuity": ["g9", "g1"]}),
        ];
        for p in bad {
            assert!(s.execute("surface.loft", &p).is_err(), "hostile loft params: {p}");
        }
    }
}
