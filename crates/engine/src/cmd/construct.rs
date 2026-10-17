//! SOLID ▸ CONSTRUCT: construction axes, points and the planes built from model geometry.
//!
//! References are given as `{face: [x,y,z]}`, `{edge: [x,y,z]}`, `{vertex: [x,y,z]}`,
//! `{point: [x,y,z]}`, `{plane: XY|name|{origin, normal}}`, `{axis: X|Y|Z|name}`,
//! `{construct_point: name}`, `{sketch, point}` or `{sketch, curve}`, or as a tagged
//! `GeoRef` (`{"type": "edge", "at": …}`). Picked faces and edges are stored with their
//! persistent names.

use serde_json::Value;
use solidcraft_doc::construct::{AxisDef, GeoRef, PointDef};
use solidcraft_doc::{FeatureKind, PlaneRef};

use super::CommandSpec;
use super::features::{add_feature, check_expr, plane_param, sketch_id};
use crate::params::{bad, req_expr, str_, vec3};
use crate::{Result, Session};

const REF: &str =
    "{face|edge|vertex|point: [x,y,z]} | {plane: XY|name|{origin, normal}} | {axis: X|Y|Z|name} | {construct_point: name} | {sketch, point|curve}";

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("construct.axis.through_cylinder", "Axis Through Cylinder/Cone/Torus", axis_face)
        .at("SOLID", "CONSTRUCT")
        .icon("axis_through_cylinder")
        .params("face: ref (a cylinder, cone or torus face, or a circular edge); name?"),
    CommandSpec::new("construct.axis.normal_to_face", "Axis Perpendicular To Face", axis_normal)
        .at("SOLID", "CONSTRUCT")
        .icon("axis_normal_to_face")
        .params("face: ref; point: ref (the axis goes through it, square to the face); name?"),
    CommandSpec::new("construct.axis.two_planes", "Axis Through Two Planes", axis_planes)
        .at("SOLID", "CONSTRUCT")
        .icon("axis_two_planes")
        .params("a, b: plane or planar face refs; name?"),
    CommandSpec::new("construct.axis.two_points", "Axis Through Two Points", axis_points)
        .at("SOLID", "CONSTRUCT")
        .icon("axis_two_points")
        .params("a, b: point refs (vertices, sketch points, points); name?"),
    CommandSpec::new("construct.axis.edge", "Axis Through Edge", axis_edge)
        .at("SOLID", "CONSTRUCT")
        .icon("axis_edge")
        .params("edge: ref (a straight edge or sketch line); name?"),
    CommandSpec::new("construct.point.vertex", "Point At Vertex", point_at)
        .at("SOLID", "CONSTRUCT")
        .icon("point_vertex")
        .params("point: ref (vertex, sketch point, point); name?"),
    CommandSpec::new("construct.point.two_edges", "Point Through Two Edges", point_edges)
        .at("SOLID", "CONSTRUCT")
        .icon("point_two_edges")
        .params("a, b: edge or line refs (where they meet, or come closest); name?"),
    CommandSpec::new("construct.point.three_planes", "Point Through Three Planes", point_planes)
        .at("SOLID", "CONSTRUCT")
        .icon("point_three_planes")
        .params("a, b, c: plane or planar face refs; name?"),
    CommandSpec::new("construct.point.center", "Point At Center Of Circle/Sphere/Torus", point_center)
        .at("SOLID", "CONSTRUCT")
        .icon("point_center")
        .params("of: ref (circular edge, sketch circle or arc, sphere or torus face); name?"),
    CommandSpec::new("construct.point.edge_plane", "Point At Edge And Plane", point_edge_plane)
        .at("SOLID", "CONSTRUCT")
        .icon("point_edge_plane")
        .params("edge: edge or line ref; plane: plane or planar face ref; name?"),
    CommandSpec::new("construct.point.along_path", "Point Along Path", point_along)
        .at("SOLID", "CONSTRUCT")
        .icon("point_along_path")
        .params("path: edge or sketch curve ref; t: expr, 0…1 along it (default 0.5); name?"),
    CommandSpec::new("construct.plane.midplane", "Midplane", plane_mid)
        .at("SOLID", "CONSTRUCT")
        .icon("plane_midplane")
        .params("a, b: plane or planar face refs (halfway between; bisecting when they meet at an angle); name?"),
    CommandSpec::new("construct.plane.two_edges", "Plane Through Two Edges", plane_edges)
        .at("SOLID", "CONSTRUCT")
        .icon("plane_two_edges")
        .params("a, b: straight edge or line refs in one plane; name?"),
    CommandSpec::new("construct.plane.three_points", "Plane Through Three Points", plane_points)
        .at("SOLID", "CONSTRUCT")
        .icon("plane_three_points")
        .params("a, b, c: point refs; name?"),
    CommandSpec::new("construct.plane.tangent", "Tangent Plane", plane_tangent)
        .at("SOLID", "CONSTRUCT")
        .icon("plane_tangent")
        .params("face: ref (cylinder, cone, sphere or torus); at?: [x,y,z] where it touches (default: the face pick); name?"),
    CommandSpec::new("construct.plane.along_path", "Plane Along Path", plane_along)
        .at("SOLID", "CONSTRUCT")
        .icon("plane_along_path")
        .params("path: edge or sketch curve ref; t: expr, 0…1 along it (default 0.5); name? The plane is square to the path there"),
    CommandSpec::new("construct.plane.perpendicular", "Perpendicular Plane", plane_perpendicular).at("SOLID", "CONSTRUCT").icon("plane_perpendicular").params(
        "plane: plane or planar face ref; line: straight edge, sketch line or axis ref; name? The plane holds the line and is square to the plane",
    ),
];

/// A reference from its JSON form.
fn geo(s: &Session, v: Option<&Value>, cmd: &str, what: &str) -> Result<GeoRef> {
    let v = v.ok_or_else(|| bad(cmd, format!("`{what}` is required: {REF}")))?;
    let p3 = |k: &str| v.get(k).and_then(vec3);
    let mut g = if v.get("type").is_some() {
        serde_json::from_value::<GeoRef>(v.clone()).map_err(|e| bad(cmd, format!("`{what}`: {e}")))?
    } else if let Some(at) = p3("face") {
        GeoRef::Face { at, name: None }
    } else if let Some(at) = p3("edge") {
        GeoRef::Edge { at, name: None }
    } else if let Some(at) = p3("vertex") {
        GeoRef::Vertex { at }
    } else if let Some(sk) = v.get("sketch") {
        let sketch = sketch_id(s, Some(sk), cmd, what)?;
        match (str_(v, "point"), str_(v, "curve")) {
            (Some(point), _) => GeoRef::SketchPoint { sketch, point: point.to_string() },
            (None, Some(curve)) => GeoRef::SketchCurve { sketch, curve: curve.to_string() },
            _ => return Err(bad(cmd, format!("`{what}`: a sketch reference needs `point` or `curve`"))),
        }
    } else if let Some(p) = p3("point") {
        GeoRef::Point { p }
    } else if v.get("plane").is_some() {
        GeoRef::Plane { plane: plane_param(s, v.get("plane"), cmd)? }
    } else if let Some(a) = str_(v, "axis") {
        if ["x", "y", "z"].contains(&a.to_ascii_lowercase().as_str()) {
            GeoRef::World { axis: a.to_ascii_uppercase() }
        } else {
            match s.doc.find_feature(a).map(|f| &f.kind) {
                Some(FeatureKind::ConstructionAxis { .. }) => GeoRef::Axis { name: a.to_string() },
                _ => return Err(bad(cmd, format!("no axis `{a}`"))),
            }
        }
    } else if let Some(n) = str_(v, "construct_point") {
        match s.doc.find_feature(n).map(|f| &f.kind) {
            Some(FeatureKind::ConstructionPoint { .. }) => GeoRef::ConstructPoint { name: n.to_string() },
            _ => return Err(bad(cmd, format!("no construction point `{n}`"))),
        }
    } else if v.as_str().is_some() {
        // A bare string: a plane.
        GeoRef::Plane { plane: plane_param(s, Some(v), cmd)? }
    } else {
        return Err(bad(cmd, format!("`{what}` must be {REF}")));
    };
    g.name_picks(&s.model.state());
    Ok(g)
}

fn t_param(s: &Session, p: &Value, cmd: &str) -> Result<solidcraft_doc::Expr> {
    let t = if p.get("t").is_some() { req_expr(cmd, p, "t")? } else { "0.5".into() };
    check_expr(s, &t, solidcraft_doc::expr::Kind::Unitless, cmd, "t")?;
    Ok(t)
}

fn axis(s: &mut Session, p: &Value, def: AxisDef) -> Result<Value> {
    add_feature(s, p, FeatureKind::ConstructionAxis { def })
}

fn point(s: &mut Session, p: &Value, def: PointDef) -> Result<Value> {
    add_feature(s, p, FeatureKind::ConstructionPoint { def })
}

fn plane(s: &mut Session, p: &Value, plane: PlaneRef) -> Result<Value> {
    add_feature(s, p, FeatureKind::ConstructionPlane { plane })
}

fn axis_face(s: &mut Session, p: &Value) -> Result<Value> {
    let face = geo(s, p.get("face"), "construct.axis.through_cylinder", "face")?;
    axis(s, p, AxisDef::ThroughFace { face })
}

fn axis_normal(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.axis.normal_to_face";
    let face = geo(s, p.get("face"), cmd, "face")?;
    let point = geo(s, p.get("point"), cmd, "point")?;
    axis(s, p, AxisDef::NormalToFace { face, point })
}

fn axis_planes(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.axis.two_planes";
    let (a, b) = (geo(s, p.get("a"), cmd, "a")?, geo(s, p.get("b"), cmd, "b")?);
    axis(s, p, AxisDef::TwoPlanes { a, b })
}

fn axis_points(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.axis.two_points";
    let (a, b) = (geo(s, p.get("a"), cmd, "a")?, geo(s, p.get("b"), cmd, "b")?);
    axis(s, p, AxisDef::TwoPoints { a, b })
}

fn axis_edge(s: &mut Session, p: &Value) -> Result<Value> {
    let edge = geo(s, p.get("edge"), "construct.axis.edge", "edge")?;
    axis(s, p, AxisDef::Edge { edge })
}

fn point_at(s: &mut Session, p: &Value) -> Result<Value> {
    let at = geo(s, p.get("point"), "construct.point.vertex", "point")?;
    point(s, p, PointDef::At { point: at })
}

fn point_edges(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.point.two_edges";
    let (a, b) = (geo(s, p.get("a"), cmd, "a")?, geo(s, p.get("b"), cmd, "b")?);
    point(s, p, PointDef::TwoEdges { a, b })
}

fn point_planes(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.point.three_planes";
    let (a, b, c) = (geo(s, p.get("a"), cmd, "a")?, geo(s, p.get("b"), cmd, "b")?, geo(s, p.get("c"), cmd, "c")?);
    point(s, p, PointDef::ThreePlanes { a, b, c })
}

fn point_center(s: &mut Session, p: &Value) -> Result<Value> {
    let of = geo(s, p.get("of"), "construct.point.center", "of")?;
    point(s, p, PointDef::Center { of })
}

fn point_edge_plane(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.point.edge_plane";
    let (edge, pl) = (geo(s, p.get("edge"), cmd, "edge")?, geo(s, p.get("plane"), cmd, "plane")?);
    point(s, p, PointDef::EdgeAndPlane { edge, plane: pl })
}

fn point_along(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.point.along_path";
    let path = geo(s, p.get("path"), cmd, "path")?;
    let t = t_param(s, p, cmd)?;
    point(s, p, PointDef::AlongPath { path, t })
}

fn plane_mid(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.plane.midplane";
    let (a, b) = (geo(s, p.get("a"), cmd, "a")?, geo(s, p.get("b"), cmd, "b")?);
    plane(s, p, PlaneRef::Midplane { a: Box::new(a), b: Box::new(b) })
}

fn plane_edges(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.plane.two_edges";
    let (a, b) = (geo(s, p.get("a"), cmd, "a")?, geo(s, p.get("b"), cmd, "b")?);
    plane(s, p, PlaneRef::TwoEdges { a: Box::new(a), b: Box::new(b) })
}

fn plane_points(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.plane.three_points";
    let (a, b, c) = (geo(s, p.get("a"), cmd, "a")?, geo(s, p.get("b"), cmd, "b")?, geo(s, p.get("c"), cmd, "c")?);
    plane(s, p, PlaneRef::ThreePoints { a: Box::new(a), b: Box::new(b), c: Box::new(c) })
}

fn plane_tangent(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.plane.tangent";
    let face = geo(s, p.get("face"), cmd, "face")?;
    let at = match (p.get("at").and_then(vec3), &face) {
        (Some(a), _) => a,
        (None, GeoRef::Face { at, .. }) => *at,
        _ => return Err(bad(cmd, "`at` is required: where the plane touches the face")),
    };
    plane(s, p, PlaneRef::Tangent { face: Box::new(face), at })
}

fn plane_along(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.plane.along_path";
    let path = geo(s, p.get("path"), cmd, "path")?;
    let t = t_param(s, p, cmd)?;
    plane(s, p, PlaneRef::AlongPath { path: Box::new(path), t })
}

fn plane_perpendicular(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "construct.plane.perpendicular";
    let (base, line) = (geo(s, p.get("plane"), cmd, "plane")?, geo(s, p.get("line"), cmd, "line")?);
    plane(s, p, PlaneRef::Perpendicular { plane: Box::new(base), line: Box::new(line) })
}
