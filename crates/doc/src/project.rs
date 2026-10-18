//! Resolving sketch links against the model: projecting edges, faces, body silhouettes, other
//! sketches and the origin onto a sketch plane, and intersecting bodies with it.
//!
//! Model geometry is found again on every recompute: a face or an edge by the persistent name
//! its link keeps, anything else (and a link saved without a name) from the reference point
//! stored in the link. An edge of a solid lying in a plane parallel to the sketch's comes in as
//! its exact curve (a line, an arc, the Bézier pieces of a free-form curve such as a letter
//! cut from a font), so a profile drawn round it meets the solid's faces exactly. Anything
//! else is fitted to its polyline from the tessellation, which lies exactly on the curve:
//! lines and circular arcs are recovered exactly, the rest (an ellipse from a tilted circle,
//! a spline edge running out of the plane) becomes a chain of lines.

use std::collections::BTreeMap;

use solidcraft_geom::{Mesh, Plane, Seg2, Vec2, Vec3};
use solidcraft_sketch::{CurveKind, LinkGeom, LinkKind, LinkSource, Sketch};

use crate::eval::ModelState;
use crate::expr::Value;
use crate::{DocError, Document, PlaneRef, Result};

/// Half length of the line an axis or plane projects to.
const AXIS_HALF: f64 = 100.0;
/// Points in one fitted chain at most (longer chains are thinned).
const MAX_CHAIN: usize = 4000;

#[path = "project_order.rs"]
mod order;

/// A resolved link: its geometry in sketch coordinates and the (possibly moved) reference.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub geom: Vec<LinkGeom>,
    /// 3D curves (world coordinates).
    pub wires: Vec<Vec<Vec3>>,
    pub source: LinkSource,
    /// The reference was not exactly where it was and was re-found on the nearest geometry.
    pub moved: bool,
    /// Exact face selected during this resolution, before its closest point is
    /// projected onto an edge shared with another face. Computed metadata only.
    pub face_index: Option<usize>,
    /// The edge an `Edge` reference resolved to (its index in the body's mesh), whose
    /// persistent name the link keeps ([`resolved_edge_name`]). Computed metadata only.
    pub edge_index: Option<usize>,
}

/// Resolutions of body-based references, keyed by the reference, the sketch plane and the
/// identity of every body's display mesh: re-resolving an unchanged model (every sketch edit
/// does) is free.
static CACHE: std::sync::Mutex<Vec<(String, Resolved)>> = std::sync::Mutex::new(Vec::new());
const CACHE_SIZE: usize = 64;

fn cache_key(st: &ModelState, plane: &Plane, kind: LinkKind, src: &LinkSource) -> Option<String> {
    let body_based = matches!(
        src,
        LinkSource::Edge { .. }
            | LinkSource::Face { .. }
            | LinkSource::Body { .. }
            | LinkSource::Vertex { .. }
            | LinkSource::Intersection { .. }
            | LinkSource::Iso { .. }
            | LinkSource::Spun { .. }
    );
    if !body_based {
        return None;
    }
    let bodies: Vec<String> = st
        .bodies
        .iter()
        .map(|b| {
            let m = b.mesh();
            format!("{}@{:p}/{}/{:?}", b.name, std::sync::Arc::as_ptr(&m), m.triangles.len(), m.bounds())
        })
        .collect();
    Some(format!("{kind:?}|{src:?}|{plane:?}|{}", bodies.join(";")))
}

/// Resolve a link source against the model state before the sketch.
pub fn resolve(doc: &Document, vals: &BTreeMap<String, Value>, st: &ModelState, plane: &Plane, kind: LinkKind, src: &LinkSource) -> Result<Resolved> {
    let key = cache_key(st, plane, kind, src);
    if let Some(k) = &key
        && let Ok(c) = CACHE.lock()
        && let Some((_, r)) = c.iter().find(|(x, _)| x == k)
    {
        return Ok(r.clone());
    }
    let r = resolve_uncached(doc, vals, st, plane, kind, src)?;
    if let Some(k) = key
        && let Ok(mut c) = CACHE.lock()
    {
        c.insert(0, (k, r.clone()));
        c.truncate(CACHE_SIZE);
    }
    Ok(r)
}

fn resolve_uncached(
    doc: &Document,
    vals: &BTreeMap<String, Value>,
    st: &ModelState,
    plane: &Plane,
    kind: LinkKind,
    src: &LinkSource,
) -> Result<Resolved> {
    let lost = |what: &str| DocError::Invalid(format!("{what} no longer exists"));
    let mut moved = false;
    let mut face_index = None;
    let mut edge_index = None;
    let mut source = src.clone();
    let mut wires: Vec<Vec<Vec3>> = Vec::new();
    let geom = match src {
        LinkSource::Edge { body, at } => {
            let (bi, mesh) = body_mesh(st, body, *at)?;
            let (e, q, d) = nearest_edge(&mesh, *at).ok_or_else(|| lost("the edge"))?;
            let size = mesh.bounds().diagonal().max(1e-9);
            if d > size * 0.25 {
                return Err(lost("the edge"));
            }
            moved = d > (size * 2e-3).max(1e-3);
            let name = st.bodies.get(bi).map(|b| b.name.clone()).unwrap_or_else(|| body.clone());
            source = LinkSource::Edge { body: name, at: q };
            edge_index = Some(e);
            let exact = st.bodies.get(bi).and_then(|b| exact_edges(b, plane, &[e]).pop().flatten());
            edge_geom(plane, kind, exact, mesh.edges.get(e).cloned().unwrap_or_default(), &mut wires)
        }
        LinkSource::Face { body, at } => {
            let (bi, mesh) = body_mesh(st, body, *at)?;
            let (f, q, d) = nearest_face(&mesh, *at).ok_or_else(|| lost("the face"))?;
            face_index = Some(usize::try_from(f).map_err(|_| DocError::Invalid("face index exceeds platform limit".into()))?);
            let size = mesh.bounds().diagonal().max(1e-9);
            if d > size * 0.25 {
                return Err(lost("the face"));
            }
            moved = d > (size * 2e-3).max(1e-3);
            let name = st.bodies.get(bi).map(|b| b.name.clone()).unwrap_or_else(|| body.clone());
            source = LinkSource::Face { body: name, at: q };
            match kind {
                LinkKind::Intersect => section(plane, &mesh, Some(f)),
                _ => {
                    let target = st.bodies.get(bi).ok_or_else(|| lost("the projection body"))?;
                    let g = projected_edges(target, &mesh, plane, &projection_face_edges(target, &mesh, f, plane)?);
                    if certified_planar_construction(target, f as usize) { merge_family_lines(g)? } else { g }
                }
            }
        }
        LinkSource::Body { body } => {
            let b = st.body(body).ok_or_else(|| lost(&format!("body {body}")))?;
            let mesh = b.mesh();
            match kind {
                LinkKind::Intersect => section(plane, &mesh, None),
                _ => silhouette(plane, &mesh),
            }
        }
        LinkSource::Vertex { body, at } => {
            let (bi, mesh) = body_mesh(st, body, *at)?;
            let size = mesh.bounds().diagonal().max(1e-9);
            let v = mesh
                .edges
                .iter()
                .flat_map(|e| [e.first().copied(), e.last().copied()])
                .flatten()
                .min_by(|a, b| a.dist(*at).total_cmp(&b.dist(*at)))
                .ok_or_else(|| lost("the vertex"))?;
            let d = v.dist(*at);
            if d > size * 0.25 {
                return Err(lost("the vertex"));
            }
            moved = d > (size * 2e-3).max(1e-3);
            let name = st.bodies.get(bi).map(|b| b.name.clone()).unwrap_or_else(|| body.clone());
            source = LinkSource::Vertex { body: name, at: v };
            vec![LinkGeom::Point(plane.to_local(v))]
        }
        LinkSource::SketchCurve { sketch, curve } => {
            let ss = st.sketch(*sketch).ok_or_else(|| lost(&format!("sketch {sketch}")))?;
            let ci = ss.sketch.curve_index(curve).ok_or_else(|| lost(&format!("sketch curve {curve}")))?;
            match kind {
                LinkKind::Intersect => {
                    let pts: Vec<Vec3> = ss.sketch.segs(ci).iter().flat_map(|s| s.polyline(1e-3)).map(|p| ss.plane.to_world(p)).collect();
                    section_of_polyline(plane, &pts)
                }
                LinkKind::Include => {
                    let pts: Vec<Vec3> = ss.sketch.polyline(ci).iter().map(|p| ss.plane.to_world(*p)).collect();
                    if off_plane(plane, &pts) {
                        wires.push(pts);
                        Vec::new()
                    } else {
                        project_sketch_curve(&ss.sketch, &ss.plane, ci, plane)
                    }
                }
                _ => project_sketch_curve(&ss.sketch, &ss.plane, ci, plane),
            }
        }
        LinkSource::OnSurface { sketch, curve, body, at } => {
            let ss = st.sketch(*sketch).ok_or_else(|| lost(&format!("sketch {sketch}")))?;
            let ci = ss.sketch.curve_index(curve).ok_or_else(|| lost(&format!("sketch curve {curve}")))?;
            let pts: Vec<Vec3> = ss.sketch.polyline(ci).iter().map(|p| ss.plane.to_world(*p)).collect();
            let (bi, mesh) = body_mesh(st, body, *at)?;
            let (f, q, d) = nearest_face(&mesh, *at).ok_or_else(|| lost("the face"))?;
            let size = mesh.bounds().diagonal().max(1e-9);
            if d > size * 0.25 {
                return Err(lost("the face"));
            }
            let name = st.bodies.get(bi).map(|b| b.name.clone()).unwrap_or_else(|| body.clone());
            source = LinkSource::OnSurface { sketch: *sketch, curve: curve.clone(), body: name, at: q };
            wires = crate::project3d::onto_face(&mesh, f, &pts, plane.normal());
            Vec::new()
        }
        LinkSource::Intersection { a, b } => {
            let (ma, fa) = crate::project3d::mesh_of(st, a).ok_or_else(|| lost(&a.describe()))?;
            let (mb, fb) = crate::project3d::mesh_of(st, b).ok_or_else(|| lost(&b.describe()))?;
            wires = crate::project3d::mesh_intersection(&ma, fa, &mb, fb);
            Vec::new()
        }
        LinkSource::Iso { body, at, dir } => {
            let (bi, mesh) = body_mesh(st, body, *at)?;
            let (f, q, d) = nearest_face(&mesh, *at).ok_or_else(|| lost("the face"))?;
            let size = mesh.bounds().diagonal().max(1e-9);
            if d > size * 0.25 {
                return Err(lost("the face"));
            }
            let name = st.bodies.get(bi).map(|b| b.name.clone()).unwrap_or_else(|| body.clone());
            source = LinkSource::Iso { body: name, at: q, dir: dir.clone() };
            wires = crate::project3d::iso_curve(&mesh, f, q, dir);
            Vec::new()
        }
        LinkSource::Spun { body, origin, dir } => {
            let b = st.body(body).ok_or_else(|| lost(&format!("body {body}")))?;
            let pts = crate::project3d::spun_outline(&b.mesh(), *origin, *dir, plane);
            let mut g = Vec::new();
            fit_chain(&pts, Some(b.mesh().bounds().diagonal().max(1e-9) * 2e-3), &mut g);
            g
        }
        LinkSource::SketchPoint { sketch, point } => {
            let ss = st.sketch(*sketch).ok_or_else(|| lost(&format!("sketch {sketch}")))?;
            let pi = ss.sketch.resolve_point(point).ok_or_else(|| lost(&format!("sketch point {point}")))?;
            let p = ss.sketch.point(pi).unwrap_or_default();
            vec![LinkGeom::Point(plane.to_local(ss.plane.to_world(p)))]
        }
        LinkSource::Origin => vec![LinkGeom::Point(plane.to_local(Vec3::ZERO))],
        LinkSource::Axis { name } => {
            let d = match name.to_ascii_uppercase().as_str() {
                "X" => Vec3::X,
                "Y" => Vec3::Y,
                "Z" => Vec3::Z,
                _ => return Err(DocError::Unknown(format!("axis `{name}`"))),
            };
            let o = plane.to_local(Vec3::ZERO);
            let d2 = Vec2::new(d.dot(plane.x), d.dot(plane.y));
            match d2.normalized() {
                Some(u) if d2.len() > 1e-9 => vec![LinkGeom::Line(o - u * AXIS_HALF, o + u * AXIS_HALF)],
                _ => vec![LinkGeom::Point(o)],
            }
        }
        LinkSource::SketchAxis { name } => {
            vec![solidcraft_sketch::sketch_axis_geom(name).ok_or_else(|| DocError::Unknown(format!("sketch axis `{name}`")))?]
        }
        LinkSource::Text { text, at, height, angle, font, style, .. } if style.path.is_none() => {
            solidcraft_sketch::text_geometry_styled(font.as_deref(), text, *at, *height, *angle, style, None)?
        }
        // Its path is a curve of its own sketch: set with the sketch (`Sketch::text_source_geometry`).
        LinkSource::Text { .. } => return Err(DocError::Invalid("a text on a path is set with its sketch".into())),
        LinkSource::Plane { name } => {
            let pr = if Plane::named(name).is_some() {
                PlaneRef::Origin { name: name.clone() }
            } else {
                PlaneRef::Construction { name: name.clone().into() }
            };
            let other = doc.resolve_plane_in(vals, &pr, 0, Some(st)).map_err(|_| lost(&format!("plane {name}")))?;
            plane_trace(plane, &other).into_iter().collect()
        }
    };
    if geom.is_empty() && wires.is_empty() {
        return Err(DocError::Invalid(format!("the {} projects to nothing on this sketch plane", src.describe())));
    }
    Ok(Resolved { geom: merge_split_circles(geom), wires, source, moved, face_index, edge_index })
}

/// Whether a curve (world points) leaves the sketch plane: included, it then stays 3D.
fn off_plane(plane: &Plane, pts: &[Vec3]) -> bool {
    let size = pts.iter().fold(1.0_f64, |m, p| m.max(p.len()));
    pts.iter().any(|p| plane.height(*p).abs() > size * 1e-7)
}

/// What a body edge (its points) gives a sketch: its section with the sketch plane, itself as a
/// 3D curve (into `wires`) when it is included and leaves the plane, else its projection.
/// An edge (its `exact` curve when read, and its polyline `pts`) projected for a link of `kind`.
fn edge_geom(plane: &Plane, kind: LinkKind, exact: Option<Vec<LinkGeom>>, pts: Vec<Vec3>, wires: &mut Vec<Vec<Vec3>>) -> Vec<LinkGeom> {
    match kind {
        LinkKind::Intersect => section_of_polyline(plane, &pts),
        LinkKind::Include if off_plane(plane, &pts) => {
            wires.push(pts);
            Vec::new()
        }
        _ => exact.unwrap_or_else(|| fit_world(plane, &pts)),
    }
}

/// Edges `edges` of `body` (indices into its mesh's `edges`) on `plane` as their exact curves:
/// lines, circles and arcs, and the Bézier pieces of a free-form curve. `None` for an edge that
/// doesn't lie in a plane parallel to `plane` or isn't read exactly (a mesh body's): it is
/// fitted to its polyline instead.
fn exact_edges(body: &crate::ModelBody, plane: &Plane, edges: &[usize]) -> Vec<Option<Vec<LinkGeom>>> {
    body.body.planar_edges(plane, edges).into_iter().map(|segs| segs?.iter().map(link_geom).collect()).collect()
}

/// `edges` of `body` projected on `plane`: each exactly ([`exact_edges`]) or fitted to its
/// polyline in `mesh`.
fn projected_edges(body: &crate::ModelBody, mesh: &Mesh, plane: &Plane, edges: &[usize]) -> Vec<LinkGeom> {
    let mut out = Vec::new();
    for (e, exact) in edges.iter().zip(exact_edges(body, plane, edges)) {
        match exact {
            Some(g) => out.extend(g),
            None => {
                if let Some(pts) = mesh.edges.get(*e) {
                    out.extend(fit_world(plane, pts));
                }
            }
        }
    }
    out
}

/// A boundary segment as the link geometry that is the same curve.
fn link_geom(s: &Seg2) -> Option<LinkGeom> {
    Some(match *s {
        Seg2::Line { a, b } => LinkGeom::Line(a, b),
        Seg2::Arc { center, radius, sweep, .. } if sweep.abs() >= std::f64::consts::TAU - 1e-9 => LinkGeom::Circle(center, radius),
        Seg2::Arc { center, radius, start, sweep } => {
            let at = |t: f64| center + Vec2::new(t.cos(), t.sin()) * radius;
            // Counter-clockwise from `a` to `b`.
            let (a, b) = if sweep > 0. { (at(start), at(start + sweep)) } else { (at(start + sweep), at(start)) };
            LinkGeom::Arc { c: center, a, b }
        }
        Seg2::Conic { a, apex, b, w } => LinkGeom::Conic { a, apex, b, rho: w / (1. + w) },
        Seg2::Cubic { p0, p1, p2, p3 } => LinkGeom::Spline { pts: vec![p0, p1, p2, p3], control: true, degree: 3, periodic: false },
        Seg2::Bezier { n, p } => LinkGeom::Spline { pts: p.get(..=usize::from(n))?.to_vec(), control: true, degree: n, periodic: false },
    })
}

/// The persistent name of the edge an `Edge` reference resolved to, for its link to keep and
/// be found by from then on. None on a mesh body, whose edges have no names.
pub fn resolved_edge_name(st: &ModelState, r: &Resolved) -> Option<String> {
    let (LinkSource::Edge { body, .. }, Some(index)) = (&r.source, r.edge_index) else { return None };
    let body = st.body(body).filter(|body| !body.body.is_mesh())?;
    crate::naming::edge_names(body).get(index).filter(|name| !name.is_empty()).cloned()
}

/// Re-resolve every link of a sketch (in place). Returns warnings for lost, moved or rebuilt
/// links; lost links keep their last geometry.
fn lost(reference: &str) -> DocError {
    DocError::Unknown(format!("{reference} no longer exists"))
}

/// The edge a link's stored name finds, and the body it is on: the link's body, or, when no
/// body has that name any more (it was joined into another), the body on which the name finds
/// the edge nearest the link's point.
fn locate_named_edge(st: &ModelState, body: &str, name: &str, at: Vec3) -> Option<(String, crate::naming::EdgeHit)> {
    let hit = |b: &crate::ModelBody| crate::naming::locate_edge(b, name, at).ok().map(|hit| (b.name.clone(), hit));
    match st.body(body) {
        Some(b) => hit(b),
        None => st.bodies.iter().filter(|b| !b.body.is_mesh()).filter_map(hit).min_by(|a, b| a.1.point.dist(at).total_cmp(&b.1.point.dist(at))),
    }
}

/// An edge found by its persistent name: edge `index` of `body`, with the link's point `near`
/// moved onto it.
fn resolve_named_edge(st: &ModelState, plane: &Plane, kind: LinkKind, body: &str, index: usize, near: Vec3) -> Result<Resolved> {
    let target = st.body(body).ok_or_else(|| lost("named projection body"))?;
    let pts = target.mesh().edges.get(index).cloned().unwrap_or_default();
    let at = pts
        .windows(2)
        .filter_map(|w| Some(closest_on_segment(near, *w.first()?, *w.get(1)?)))
        .min_by(|a, b| a.dist(near).total_cmp(&b.dist(near)))
        .ok_or_else(|| lost("named projection edge"))?;
    let mut wires = Vec::new();
    let exact = exact_edges(target, plane, &[index]).pop().flatten();
    let geom = edge_geom(plane, kind, exact, pts, &mut wires);
    if geom.is_empty() && wires.is_empty() {
        return Err(DocError::Invalid("named edge projects to nothing on this sketch plane".into()));
    }
    let source = LinkSource::Edge { body: body.into(), at };
    Ok(Resolved { geom: merge_split_circles(geom), wires, source, moved: false, face_index: None, edge_index: Some(index) })
}

fn resolve_named_face(st: &ModelState, plane: &Plane, kind: LinkKind, body: &str, index: usize, at: Vec3) -> Result<Resolved> {
    let target = st.body(body).ok_or_else(|| lost("named projection body"))?;
    let mesh = target.mesh();
    if mesh.triangles.len() > 2_000_000 || mesh.edges.len() > 2_000_000 {
        return Err(DocError::Invalid("named face projection work limit exceeded".into()));
    }
    let face_index = Some(index);
    let index = u32::try_from(index).map_err(|_| DocError::Invalid("named projection face index exceeds limit".into()))?;
    let geom = if kind == LinkKind::Intersect {
        section(plane, &mesh, Some(index))
    } else {
        let geom = projected_edges(target, &mesh, plane, &projection_face_edges(target, &mesh, index, plane)?);
        let geom = merge_split_circles(geom);
        if certified_planar_construction(target, index as usize) { merge_family_lines(geom)? } else { geom }
    };
    if geom.is_empty() {
        return Err(DocError::Invalid("named face projects to nothing on this sketch plane".into()));
    }
    Ok(Resolved { geom, wires: Vec::new(), source: LinkSource::Face { body: body.into(), at }, moved: false, face_index, edge_index: None })
}

fn projection_face_edges(body: &crate::ModelBody, mesh: &Mesh, index: u32, plane: &Plane) -> Result<Vec<usize>> {
    let shell_plane = body
        .body
        .face_sources()
        .and_then(|sources| sources.get(index as usize))
        .is_some_and(|source| matches!(source, Some(solidcraft_kernel::FaceSource::ShellPlanar { .. })));
    if shell_plane { order::raw_face_edges(mesh, index, plane) } else { order::face_edges(mesh, index, plane) }
}

fn certified_planar_construction(body: &crate::ModelBody, index: usize) -> bool {
    body.body.face_sources().and_then(|sources| sources.get(index)).is_some_and(|source| {
        matches!(
            source,
            Some(
                solidcraft_kernel::FaceSource::ShellPlanar { producer: Some(_), .. }
                    | solidcraft_kernel::FaceSource::Box { producer: Some(_), .. }
                    | solidcraft_kernel::FaceSource::CylinderCap { .. }
                    | solidcraft_kernel::FaceSource::CertifiedExtrudeSide { plane: Some(_), .. }
                    | solidcraft_kernel::FaceSource::CertifiedExtrudeCap { .. }
            )
        )
    })
}

pub fn resolve_anchored_face(doc: &Document, st: &ModelState, plane: &Plane, kind: LinkKind, anchor: &crate::FaceAnchor) -> Result<Resolved> {
    let body = st.body(&anchor.target.body).ok_or_else(|| lost("anchored projection body"))?;
    let indices = doc.configuration_face_indices(body, &anchor.target)?;
    resolve_face_indices(st, plane, kind, &body.name, &indices)
}

/// Project explicitly selected current face indices, without claiming persistent provenance.
/// Callers retaining a link must supply their own replay identity policy.
pub fn resolve_face_indices(st: &ModelState, plane: &Plane, kind: LinkKind, body_name: &str, indices: &[usize]) -> Result<Resolved> {
    let body = st.body(body_name).ok_or_else(|| lost("projection body"))?;
    let mesh = body.mesh();
    if mesh.triangles.len() > 2_000_000 || indices.len() > 10_000 {
        return Err(DocError::Invalid("anchored projection work limit exceeded".into()));
    }
    let work = mesh.triangles.len().saturating_add(mesh.edges.len()).saturating_mul(indices.len());
    if work > 10_000_000 {
        return Err(DocError::Invalid("anchored projection aggregate work limit exceeded".into()));
    }
    let selected: std::collections::BTreeSet<_> = indices.iter().copied().collect();
    let mut geometry = Vec::new();
    if kind == LinkKind::Intersect {
        for index in indices {
            let index = u32::try_from(*index).map_err(|_| DocError::Invalid("anchored face index exceeds format bounds".into()))?;
            geometry.extend(section(plane, &mesh, Some(index)));
        }
    } else {
        let edges = if let [index] = indices {
            projection_face_edges(
                body,
                &mesh,
                u32::try_from(*index).map_err(|_| DocError::Invalid("face index exceeds format bounds".into()))?,
                plane,
            )?
        } else {
            order::family_edges(&mesh, &selected, plane)?
        };
        geometry.extend(projected_edges(body, &mesh, plane, &edges));
    }
    if kind != LinkKind::Intersect && (indices.len() > 1 || indices.first().is_some_and(|index| certified_planar_construction(body, *index))) {
        geometry = merge_family_lines(geometry)?;
    }
    let point = mesh
        .triangles
        .iter()
        .zip(&mesh.tri_face)
        .find_map(|(triangle, face)| selected.contains(&(*face as usize)).then(|| mesh.tri(triangle)).flatten().map(|[a, b, c]| (a + b + c) / 3.0))
        .ok_or_else(|| lost("anchored projection face"))?;
    if geometry.is_empty() {
        return Err(lost("anchored projection boundaries"));
    }
    Ok(Resolved {
        geom: merge_split_circles(geometry),
        wires: Vec::new(),
        source: LinkSource::Face { body: body.name.clone(), at: point },
        moved: false,
        face_index: if indices.len() == 1 { indices.first().copied() } else { None },
        edge_index: None,
    })
}

pub fn refresh_face_anchor_links(
    doc: &Document,
    vals: &BTreeMap<String, Value>,
    st: &ModelState,
    plane: &Plane,
    sk: &mut Sketch,
    anchor: &crate::FaceAnchor,
) -> Vec<String> {
    refresh_links_in(doc, vals, st, plane, sk, Some(anchor))
}

/// Remove degree-two straight stations introduced by coplanar face subdivision.
/// The walk uses endpoint incidence; geometric sorting does not imply adjacency.
fn merge_family_lines(geom: Vec<LinkGeom>) -> Result<Vec<LinkGeom>> {
    if geom.len().saturating_mul(geom.len()).saturating_mul(3) > 10_000_000 {
        return Err(DocError::Invalid("face family straight-boundary work limit exceeded".into()));
    }
    let ends = |curve: &LinkGeom| match curve {
        LinkGeom::Line(a, b) | LinkGeom::Arc { a, b, .. } | LinkGeom::Conic { a, b, .. } => Some((*a, *b)),
        LinkGeom::Spline { pts, periodic: false, .. } => pts.first().zip(pts.last()).map(|(a, b)| (*a, *b)),
        _ => None,
    };
    let scale = geom.iter().filter_map(&ends).flat_map(|(a, b)| [a.len(), b.len()]).fold(1., f64::max);
    let tol = scale * f64::EPSILON * 512.;
    let mut used = vec![false; geom.len()];
    let mut result = Vec::new();
    for (index, curve) in geom.iter().enumerate() {
        if used.get(index).copied().unwrap_or(true) {
            continue;
        }
        if let Some(slot) = used.get_mut(index) {
            *slot = true;
        }
        let LinkGeom::Line(a, b) = curve else {
            result.push(curve.clone());
            continue;
        };
        let mut endpoints = [*a, *b];
        for side in 0..2 {
            let mut current = index;
            for _ in 0..geom.len() {
                let Some(point) = endpoints.get(side).copied() else { break };
                let mut incidence = geom
                    .iter()
                    .enumerate()
                    .filter_map(|(other, curve)| ends(curve).filter(|(a, b)| a.dist(point) <= tol || b.dist(point) <= tol).map(|_| other));
                let (Some(one), Some(two), None) = (incidence.next(), incidence.next(), incidence.next()) else { break };
                let other = if one == current {
                    two
                } else if two == current {
                    one
                } else {
                    break;
                };
                if used.get(other).copied().unwrap_or(true) {
                    break;
                }
                let Some(LinkGeom::Line(a, b)) = geom.get(other) else { break };
                let next = if a.dist(point) <= tol { *b } else { *a };
                let Some(opposite) = endpoints.get(1 - side).copied() else { break };
                let (Some(incoming), Some(outgoing)) = ((point - opposite).normalized(), (next - point).normalized()) else { break };
                if incoming.dot(outgoing) <= 0. || incoming.cross(outgoing).abs() > f64::EPSILON * 512. {
                    break;
                }
                if let Some(slot) = used.get_mut(other) {
                    *slot = true;
                }
                if let Some(slot) = endpoints.get_mut(side) {
                    *slot = next;
                }
                current = other;
            }
        }
        if let (Some(a), Some(b)) = (endpoints.first(), endpoints.get(1)) {
            result.push(LinkGeom::Line(*a, *b));
        }
    }
    Ok(result)
}

pub fn refresh_links(doc: &Document, vals: &BTreeMap<String, Value>, st: &ModelState, plane: &Plane, sk: &mut Sketch) -> Vec<String> {
    refresh_links_in(doc, vals, st, plane, sk, None)
}

fn refresh_links_in(
    doc: &Document,
    vals: &BTreeMap<String, Value>,
    st: &ModelState,
    plane: &Plane,
    sk: &mut Sketch,
    anchor: Option<&crate::FaceAnchor>,
) -> Vec<String> {
    let mut warn = Vec::new();
    let links = sk.links.clone();
    for l in &links {
        let mut source = l.source.clone();
        let mut named_face = None;
        if !l.auto_face
            && let (Some(name), LinkSource::Face { body, at }) = (&l.face_name, &mut source)
        {
            if let Some((index, p, split)) = st.body(body).and_then(|target| crate::naming::face_on_body(target, name, *at)) {
                let stem = crate::naming::strip_piece(name);
                let removed_piece = split
                    && name != stem
                    && st.body(body).is_some_and(|target| {
                        (certified_planar_construction(target, index) || target.body.face_was_coalesced(index))
                            && crate::naming::face_has_name(target, index, stem)
                    });
                if removed_piece {
                    sk.set_link_lost(&l.id, true);
                    warn.push(format!("projected face piece `{name}` no longer exists; last geometry retained"));
                    continue;
                }
                *at = p;
                named_face = Some((body.clone(), index, p));
                if split {
                    warn.push(format!("projected face `{name}` was split; using the nearest piece"));
                }
            } else {
                sk.set_link_lost(&l.id, true);
                warn.push(format!("projected face `{name}` no longer exists"));
                continue;
            }
        }
        // An edge the link keeps by name is that edge of its body, wherever it went. When no
        // edge has the name any more the link is lost: no other edge is taken for it.
        let mut named_edge = None;
        if let (Some(name), LinkSource::Edge { body, at }) = (&l.edge_name, &source) {
            match locate_named_edge(st, body, name, *at) {
                Some((body, hit)) => {
                    warn.extend(hit.note(name).map(|note| format!("projected {note}")));
                    named_edge = Some((body, hit.edge, *at));
                }
                None => {
                    sk.set_link_lost(&l.id, true);
                    warn.push(format!("projected reference lost: edge `{name}` no longer exists; last geometry retained"));
                    continue;
                }
            }
        }
        let resolved = if matches!(source, LinkSource::Text { .. }) {
            // A text needs nothing outside its sketch; one on a path follows a curve of it.
            sk.text_source_geometry(&source).map_err(DocError::from).and_then(|geom| {
                if geom.is_empty() {
                    return Err(DocError::Invalid(format!("the {} makes no outline", source.describe())));
                }
                Ok(Resolved { geom, wires: Vec::new(), source: source.clone(), moved: false, face_index: None, edge_index: None })
            })
        } else if l.auto_face {
            anchor
                .filter(|anchor| {
                    l.face_name.as_ref() == Some(&anchor.target.name) && matches!(&l.source,LinkSource::Face {body,..} if *body==anchor.target.body)
                })
                .ok_or_else(|| lost("exact auto-projected attachment"))
                .and_then(|anchor| resolve_anchored_face(doc, st, plane, l.kind, anchor))
        } else if let Some((body, index, at)) = named_face {
            resolve_named_face(st, plane, l.kind, &body, index, at)
        } else if let Some((body, index, at)) = named_edge {
            resolve_named_edge(st, plane, l.kind, &body, index, at)
        } else {
            resolve(doc, vals, st, plane, l.kind, &source)
        };
        match resolved {
            Ok(r) => {
                // An edge link saved without a name takes the name of the edge it was found
                // on. A sketch that is edited stores it; evaluation leaves the design as it is.
                if l.edge_name.is_none()
                    && let Some(name) = resolved_edge_name(st, &r)
                    && let Some(link) = sk.links.iter_mut().find(|link| link.id == l.id)
                {
                    link.edge_name = Some(name);
                }
                sk.set_link_source(&l.id, r.source);
                let _ = sk.set_link_wires(&l.id, &r.wires);
                match sk.update_link(&l.id, &r.geom) {
                    Ok(true) => warn.push(format!("projected {} changed shape and was rebuilt", l.source.describe())),
                    Ok(false) if r.moved => warn.push(format!("projected {} was re-found on the nearest geometry", l.source.describe())),
                    Ok(false) => {}
                    Err(e) => {
                        sk.set_link_lost(&l.id, true);
                        warn.push(format!("projected {}: {e}", l.source.describe()));
                    }
                }
            }
            Err(e) => {
                sk.set_link_lost(&l.id, true);
                warn.push(format!("projected reference lost: {e}"));
            }
        }
    }
    warn
}

// ---------------------------------------------------------------------------------------------
// Model lookups

/// The body by name (or, when it is gone or unnamed, the one with geometry nearest `at`) and
/// its display mesh.
fn body_mesh(st: &ModelState, name: &str, at: Vec3) -> Result<(usize, std::sync::Arc<Mesh>)> {
    if let Some(i) = st.bodies.iter().position(|b| b.name == name)
        && let Some(b) = st.bodies.get(i)
    {
        return Ok((i, b.mesh()));
    }
    let mut best: Option<(usize, f64)> = None;
    for (i, b) in st.bodies.iter().enumerate() {
        let m = b.mesh();
        if let Some((_, _, d)) = nearest_face(&m, at)
            && best.is_none_or(|(_, bd)| d < bd)
        {
            best = Some((i, d));
        }
    }
    let (i, _) = best.ok_or_else(|| DocError::Invalid(format!("body {name} no longer exists")))?;
    let b = st.bodies.get(i).ok_or_else(|| DocError::Invalid(format!("body {name} no longer exists")))?;
    Ok((i, b.mesh()))
}

fn closest_on_segment(p: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let ab = b - a;
    let l2 = ab.len2();
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    a + ab * t
}

/// Nearest non-seam edge: (index, closest point, distance).
pub(crate) fn nearest_edge(m: &Mesh, p: Vec3) -> Option<(usize, Vec3, f64)> {
    let mut best: Option<(usize, Vec3, f64)> = None;
    for (i, e) in m.edges.iter().enumerate() {
        if m.seams.get(i).copied().unwrap_or(false) {
            continue;
        }
        for w in e.windows(2) {
            let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
            let q = closest_on_segment(p, *a, *b);
            let d = q.dist(p);
            if best.is_none_or(|(_, _, bd)| d < bd) {
                best = Some((i, q, d));
            }
        }
    }
    best
}

pub(crate) fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let n = (b - a).cross(c - a);
    let Some(nn) = n.normalized() else { return closest_on_segment(p, a, b) };
    let q = p - nn * (p - a).dot(nn);
    let inside = (b - a).cross(q - a).dot(n) >= 0.0 && (c - b).cross(q - b).dot(n) >= 0.0 && (a - c).cross(q - c).dot(n) >= 0.0;
    if inside {
        return q;
    }
    [closest_on_segment(p, a, b), closest_on_segment(p, b, c), closest_on_segment(p, c, a)]
        .into_iter()
        .min_by(|x, y| x.dist(p).total_cmp(&y.dist(p)))
        .unwrap_or(a)
}

/// Nearest face: (B-rep face index, closest point, distance).
pub(crate) fn nearest_face(m: &Mesh, p: Vec3) -> Option<(u32, Vec3, f64)> {
    let mut best: Option<(u32, Vec3, f64)> = None;
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        let Some([a, b, c]) = m.tri(t) else { continue };
        let q = closest_on_triangle(p, a, b, c);
        let d = q.dist(p);
        if best.is_none_or(|(_, _, bd)| d < bd) {
            best = Some((*f, q, d));
        }
    }
    best
}

/// A full circle split by a seam comes out as two arcs between the same two points, but some
/// meshes give the same half twice. That is a circle too: turn the duplicate into the other half
/// so re-reading an unchanged face always gives the same two complementary arcs.
fn merge_split_circles(geom: Vec<LinkGeom>) -> Vec<LinkGeom> {
    const TOL: f64 = 1e-6;
    let near = |p: Vec2, q: Vec2| p.dist(q) <= TOL * (1.0 + p.len().max(q.len()));
    let mut out = geom.clone();
    for i in 0..geom.len() {
        let Some(LinkGeom::Arc { c, a, b }) = geom.get(i).cloned() else { continue };
        for j in i + 1..geom.len() {
            if let Some(LinkGeom::Arc { c: c2, a: a2, b: b2 }) = geom.get(j).cloned()
                && near(c, c2)
                && near(a, a2)
                && near(b, b2)
                && let Some(slot) = out.get_mut(j)
            {
                *slot = LinkGeom::Arc { c, a: b, b: a };
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Projection

fn project_sketch_curve(sk: &Sketch, from: &Plane, ci: usize, to: &Plane) -> Vec<LinkGeom> {
    let map = |p: Vec2| to.to_local(from.to_world(p));
    let parallel = from.normal().cross(to.normal()).len() < 1e-9;
    let flip = from.normal().dot(to.normal()) < 0.0;
    match sk.curves.get(ci).map(|c| &c.kind) {
        Some(CurveKind::Line { a, b }) => match (sk.point(*a), sk.point(*b)) {
            (Some(a), Some(b)) => {
                let (a, b) = (map(a), map(b));
                if a.dist(b) > 1e-9 { vec![LinkGeom::Line(a, b)] } else { vec![LinkGeom::Point(a)] }
            }
            _ => Vec::new(),
        },
        Some(CurveKind::Circle { c, r }) if parallel => sk.point(*c).map(|c| vec![LinkGeom::Circle(map(c), *r)]).unwrap_or_default(),
        Some(CurveKind::Arc { c, a, b }) if parallel => match (sk.point(*c), sk.point(*a), sk.point(*b)) {
            (Some(c), Some(a), Some(b)) => {
                let (c, a, b) = (map(c), map(a), map(b));
                vec![if flip { LinkGeom::Arc { c, a: b, b: a } } else { LinkGeom::Arc { c, a, b } }]
            }
            _ => Vec::new(),
        },
        // Free-form curves in a parallel plane map exactly through their defining points.
        Some(CurveKind::Ellipse { c, m, r }) if parallel => match (sk.point(*c), sk.point(*m)) {
            (Some(c), Some(m)) => vec![LinkGeom::Ellipse { c: map(c), major: map(m), minor: *r }],
            _ => Vec::new(),
        },
        Some(CurveKind::Spline { pts, control, degree, periodic }) if parallel => {
            let p: Option<Vec<Vec2>> = pts.iter().map(|q| sk.point(*q).map(map)).collect();
            p.map(|pts| vec![LinkGeom::Spline { pts, control: *control, degree: *degree, periodic: *periodic }]).unwrap_or_default()
        }
        Some(CurveKind::Conic { a, b, apex, rho }) if parallel => match (sk.point(*a), sk.point(*apex), sk.point(*b)) {
            (Some(a), Some(x), Some(b)) => vec![LinkGeom::Conic { a: map(a), apex: map(x), b: map(b), rho: *rho }],
            _ => Vec::new(),
        },
        Some(_) => {
            let pts: Vec<Vec3> = sk.polyline(ci).iter().map(|p| from.to_world(*p)).collect();
            fit_world(to, &pts)
        }
        None => Vec::new(),
    }
}

/// A circle in space (centre, unit normal, radius) through the polyline's points, and whether
/// the polyline closes.
fn circle3(pts: &[Vec3]) -> Option<(Vec3, Vec3, f64, bool)> {
    let n = pts.len();
    if n < 4 {
        return None;
    }
    let closed = pts.first()?.dist(*pts.last()?) <= 1e-9 * pts.first()?.len().max(1.0);
    let (a, b, c) = if closed { (*pts.first()?, *pts.get(n / 3)?, *pts.get(2 * n / 3)?) } else { (*pts.first()?, *pts.get(n / 2)?, *pts.last()?) };
    let (ab, ac) = (b - a, c - a);
    let nrm = ab.cross(ac);
    let l2 = nrm.len2();
    if l2 < 1e-24 {
        return None;
    }
    // Circumcentre of the triangle a, b, c.
    let centre = a + (nrm.cross(ab) * ac.len2() + ac.cross(nrm) * ab.len2()) / (2.0 * l2);
    let r = centre.dist(a);
    let un = nrm.normalized()?;
    let tol = 1e-7 * r.max(1.0);
    pts.iter().all(|p| (p.dist(centre) - r).abs() < tol && (*p - centre).dot(un).abs() < tol).then_some((centre, un, r, closed))
}

/// A circle seen at a slant is an ellipse, exactly: a full circle becomes an ellipse; an arc
/// becomes conics (rational quadratic Béziers of at most 90 degrees each — parallel projection
/// keeps their weights).
fn tilted_circle(plane: &Plane, pts: &[Vec3]) -> Option<Vec<LinkGeom>> {
    let (c, n, r, closed) = circle3(pts)?;
    let sn = plane.normal();
    let cosang = n.dot(sn).abs();
    if !(1e-9..=1.0 - 1e-12).contains(&cosang) {
        return None;
    }
    if closed {
        let u = n.cross(sn).normalized()?;
        return Some(vec![LinkGeom::Ellipse { c: plane.to_local(c), major: plane.to_local(c + u * r), minor: r * cosang }]);
    }
    // Sweep from the first point along the polyline's direction.
    let p0 = *pts.first()?;
    let e1 = (p0 - c).normalized()?;
    let e2 = n.cross(e1);
    let ang = |p: Vec3| (p - c).dot(e2).atan2((p - c).dot(e1));
    let mut sweep = 0.0;
    for w in pts.windows(2) {
        let mut d = ang(w[1]) - ang(w[0]);
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        sweep += d;
    }
    let k = ((sweep.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize).clamp(1, 8);
    let h = sweep / k as f64;
    let at = |t: f64| c + (e1 * t.cos() + e2 * t.sin()) * r;
    let w = (h.abs() / 2.0).cos();
    let rho = w / (1.0 + w);
    let mut out = Vec::new();
    for i in 0..k {
        let (t0, t1) = (h * i as f64, h * (i + 1) as f64);
        let apex = c + (e1 * ((t0 + t1) / 2.0).cos() + e2 * ((t0 + t1) / 2.0).sin()) * (r / w);
        out.push(LinkGeom::Conic { a: plane.to_local(at(t0)), apex: plane.to_local(apex), b: plane.to_local(at(t1)), rho });
    }
    Some(out)
}

/// A smooth chain that fitting broke into many short lines is better as one fit spline (a
/// projected spline edge or elliptical arc): when a spline through a few of its points stays
/// within `tol` of all of them.
fn as_spline(pts: &[Vec2], tol: f64) -> Option<LinkGeom> {
    let n = pts.len();
    if n < 4 {
        return None;
    }
    // No sharp corners.
    for w in pts.windows(3) {
        let (Some(u), Some(v)) = ((w[1] - w[0]).normalized(), (w[2] - w[1]).normalized()) else { continue };
        if u.dot(v) < 30f64.to_radians().cos() {
            return None;
        }
    }
    for k in [8usize, 16, 32, 64] {
        let step = (n - 1) as f64 / k as f64;
        let mut fit: Vec<Vec2> = (0..=k).map(|i| pts[((i as f64 * step).round() as usize).min(n - 1)]).collect();
        fit.dedup_by(|a, b| a.dist(*b) < 1e-12);
        if fit.len() < 2 {
            return None;
        }
        let poly = solidcraft_sketch::spline_polyline(&fit, false, 3);
        let ok = pts.iter().all(|q| {
            poly.windows(2)
                .map(|w| {
                    let (a, b) = (w[0], w[1]);
                    let d = b - a;
                    let t = if d.len2() > 0.0 { ((*q - a).dot(d) / d.len2()).clamp(0.0, 1.0) } else { 0.0 };
                    q.dist(a + d * t)
                })
                .fold(f64::INFINITY, f64::min)
                <= tol
        });
        if ok {
            return Some(LinkGeom::Spline { pts: fit, control: false, degree: 3, periodic: false });
        }
    }
    None
}

/// The line where another plane crosses the sketch plane (None when parallel).
fn plane_trace(sk: &Plane, other: &Plane) -> Option<LinkGeom> {
    let m = other.normal();
    let (a, b) = (sk.x.dot(m), sk.y.dot(m));
    let n2 = a * a + b * b;
    if n2 < 1e-12 {
        return None;
    }
    let c = (other.origin - sk.origin).dot(m);
    let p0 = Vec2::new(a, b) * (c / n2);
    let u = Vec2::new(-b, a).normalized()?;
    Some(LinkGeom::Line(p0 - u * AXIS_HALF, p0 + u * AXIS_HALF))
}

/// Project a world polyline and fit lines and arcs to it.
fn fit_world(plane: &Plane, pts: &[Vec3]) -> Vec<LinkGeom> {
    if let Some(e) = tilted_circle(plane, pts) {
        return e;
    }
    let p2: Vec<Vec2> = pts.iter().map(|p| plane.to_local(*p)).collect();
    let mut out = Vec::new();
    fit_chain(&p2, None, &mut out);
    // Many little lines from one smooth edge: a spline instead.
    if out.len() > 4 && out.iter().all(|g| matches!(g, LinkGeom::Line(..))) {
        let ext = p2.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.x).min(p.y), hi.max(p.x).max(p.y)));
        if let Some(sp) = as_spline(&p2, (ext.1 - ext.0).abs().max(1e-9) * 1e-4) {
            return vec![sp];
        }
    }
    out
}

/// Where a world polyline crosses the plane.
fn section_of_polyline(plane: &Plane, pts: &[Vec3]) -> Vec<LinkGeom> {
    let mut out: Vec<LinkGeom> = Vec::new();
    for w in pts.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        let (ha, hb) = (plane.height(*a), plane.height(*b));
        if (ha > 0.0) != (hb > 0.0) || ha == 0.0 {
            let t = if (ha - hb).abs() < 1e-15 { 0.0 } else { ha / (ha - hb) };
            let p = plane.to_local(a.lerp(*b, t));
            if !out.iter().any(|g| matches!(g, LinkGeom::Point(q) if q.dist(p) < 1e-7)) {
                out.push(LinkGeom::Point(p));
            }
        }
    }
    out
}

/// The outline of a body seen along the sketch normal: mesh edges between triangles facing the
/// sketch and triangles facing away (or edge-on), projected and fitted.
fn silhouette(plane: &Plane, m: &Mesh) -> Vec<LinkGeom> {
    let n = plane.normal();
    let size = m.bounds().diagonal().max(1e-9);
    let q = size * 1e-7;
    let key = |p: Vec3| ((p.x / q).round() as i64, (p.y / q).round() as i64, (p.z / q).round() as i64);
    // Undirected mesh edge → facing flags of the triangles using it.
    let mut edges: BTreeMap<((i64, i64, i64), (i64, i64, i64)), (Vec3, Vec3, Vec<bool>)> = BTreeMap::new();
    for t in &m.triangles {
        let Some([a, b, c]) = m.tri(t) else { continue };
        let fnrm = (b - a).cross(c - a);
        let Some(un) = fnrm.normalized() else { continue };
        let front = un.dot(n) > 1e-6;
        for (p, r) in [(a, b), (b, c), (c, a)] {
            let (kp, kr) = (key(p), key(r));
            let k = if kp <= kr { (kp, kr) } else { (kr, kp) };
            edges.entry(k).or_insert_with(|| (p, r, Vec::new())).2.push(front);
        }
    }
    let segs: Vec<(Vec2, Vec2)> = edges
        .values()
        .filter(|(_, _, f)| {
            let fronts = f.iter().filter(|x| **x).count();
            fronts > 0 && fronts < f.len() || (f.len() == 1 && fronts == 1)
        })
        .map(|(a, b, _)| (plane.to_local(*a), plane.to_local(*b)))
        .collect();
    fit_segments(&segs, size)
}

/// Section of a mesh (or one of its faces) with the sketch plane.
pub(crate) fn section(plane: &Plane, m: &Mesh, face: Option<u32>) -> Vec<LinkGeom> {
    let size = m.bounds().diagonal().max(1e-9);
    let q = size * 1e-7;
    let key = |p: Vec3| ((p.x / q).round() as i64, (p.y / q).round() as i64, (p.z / q).round() as i64);
    // Surface normal at each mesh vertex where the surface is smooth (None at creases), over
    // all faces and per face.
    type Key = (i64, i64, i64);
    let mut normals: BTreeMap<Key, Option<Vec3>> = BTreeMap::new();
    let mut face_normals: BTreeMap<(u32, Key), Option<Vec3>> = BTreeMap::new();
    // Faces on either side of each mesh edge.
    let mut edge_faces: BTreeMap<(Key, Key), Vec<u32>> = BTreeMap::new();
    let merge = |e: &mut Option<Vec3>, n: Vec3| {
        if e.is_some_and(|o| o.dot(n) < 1.0 - 1e-6) {
            *e = None;
        }
    };
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        for k in t {
            let (Some(p), Some(n)) = (m.positions.get(*k as usize), m.normals.get(*k as usize)) else { continue };
            let Some(n) = n.normalized() else { continue };
            normals.entry(key(*p)).and_modify(|e| merge(e, n)).or_insert(Some(n));
            face_normals.entry((*f, key(*p))).and_modify(|e| merge(e, n)).or_insert(Some(n));
        }
        for (i, j) in [(0usize, 1usize), (1, 2), (2, 0)] {
            let (Some(a), Some(b)) = (t.get(i).and_then(|k| m.positions.get(*k as usize)), t.get(j).and_then(|k| m.positions.get(*k as usize)))
            else {
                continue;
            };
            let (ka, kb) = (key(*a), key(*b));
            edge_faces.entry(if ka <= kb { (ka, kb) } else { (kb, ka) }).or_default().push(*f);
        }
    }
    // Where the plane crosses a mesh edge: on a cubic through the end points whose end
    // tangents lie in the surface's tangent planes (so sections of curved faces land on the
    // surface, not on the chord), computed once per edge so neighbours agree.
    let exact: std::cell::RefCell<Vec<Vec2>> = std::cell::RefCell::new(Vec::new());
    let crossing = |a: Vec3, b: Vec3| -> Option<Vec3> {
        let (a, b) = if key(a) <= key(b) { (a, b) } else { (b, a) };
        let (ha, hb) = (plane.height(a), plane.height(b));
        if (ha > 0.0) == (hb > 0.0) {
            return None;
        }
        let d = b - a;
        // An edge inside one face takes that face's normals (its ends may lie on the face's
        // boundary, a crease for the whole mesh); an edge between faces, the merged ones, so
        // both faces agree on the crossing.
        let inside = edge_faces.get(&(key(a), key(b))).and_then(|fs| fs.first().filter(|f| fs.len() == 2 && fs.iter().all(|x| x == *f)).copied());
        let normal = |p: Vec3| match inside {
            Some(f) => face_normals.get(&(f, key(p))).copied().flatten(),
            None => normals.get(&key(p)).copied().flatten(),
        };
        // Tangent length: the arc length of a circle through both ends with these normals.
        let (na, nb) = (normal(a), normal(b));
        let l = match (na, nb) {
            (Some(x), Some(y)) => {
                let half = 0.5 * x.dot(y).clamp(-1.0, 1.0).acos();
                if half > 1e-9 { d.len() * half / half.sin() } else { d.len() }
            }
            _ => d.len(),
        };
        let tangent = |n: Option<Vec3>| -> Vec3 {
            match n {
                Some(n) => (d - n * d.dot(n)).normalized().map(|t| t * l).unwrap_or(d),
                None => d,
            }
        };
        // Along a ruling (same normal at both ends: planes, cylinder and cone generators) the
        // edge lies on the surface and the straight crossing is exact.
        if let (Some(x), Some(y)) = (na, nb)
            && x.dot(y) > 1.0 - 1e-12
        {
            let p = a.lerp(b, ha / (ha - hb));
            exact.borrow_mut().push(plane.to_local(p));
            return Some(p);
        }
        let (ta, tb) = (tangent(na), tangent(nb));
        let h = |t: f64| {
            let (t2, t3) = (t * t, t * t * t);
            a * (2.0 * t3 - 3.0 * t2 + 1.0) + ta * (t3 - 2.0 * t2 + t) + b * (-2.0 * t3 + 3.0 * t2) + tb * (t3 - t2)
        };
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if (plane.height(h(mid)) > 0.0) == (ha > 0.0) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(h(0.5 * (lo + hi)))
    };
    let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        if face.is_some_and(|x| x != *f) {
            continue;
        }
        let Some(tri) = m.tri(t) else { continue };
        let mut pts: Vec<Vec3> = Vec::new();
        for (i, j) in [(0usize, 1usize), (1, 2), (2, 0)] {
            let (Some(&pi), Some(&pj)) = (tri.get(i), tri.get(j)) else { continue };
            if let Some(x) = crossing(pi, pj) {
                pts.push(x);
            }
        }
        if let [a, b] = pts[..] {
            let (a, b) = (plane.to_local(a), plane.to_local(b));
            if a.dist(b) > size * 1e-9 {
                segs.push((a, b));
            }
        }
    }
    let exact = exact.into_inner();
    let loose = size * 2e-3;
    fit_segments(&segs, size).into_iter().map(|g| refine_round(g, &exact, loose)).collect()
}

/// Re-fit a circle or arc through the exact section points near it, when there are enough.
fn refine_round(g: LinkGeom, exact: &[Vec2], tol: f64) -> LinkGeom {
    let (c, r) = match g {
        LinkGeom::Circle(c, r) => (c, r),
        LinkGeom::Arc { c, a, .. } => (c, c.dist(a)),
        _ => return g,
    };
    let near: Vec<Vec2> = exact.iter().filter(|p| (p.dist(c) - r).abs() <= tol).copied().collect();
    // Arcs: only points within the arc's sweep.
    let near: Vec<Vec2> = match g {
        LinkGeom::Arc { c, a, b } => {
            let (s0, sw) = ((a - c).angle(), ((b - c).angle() - (a - c).angle()).rem_euclid(std::f64::consts::TAU));
            near.into_iter().filter(|p| ((*p - c).angle() - s0).rem_euclid(std::f64::consts::TAU) <= sw + 1e-9).collect()
        }
        _ => near,
    };
    let Some((c2, r2)) = (near.len() >= 4).then(|| ls_circle(&near)).flatten() else { return g };
    if c2.dist(c) > tol || (r2 - r).abs() > tol || near.iter().any(|p| (p.dist(c2) - r2).abs() > tol * 1e-3) {
        return g;
    }
    let on = |q: Vec2| c2 + (q - c).normalized().unwrap_or(Vec2::X) * r2;
    match g {
        LinkGeom::Arc { a, b, .. } => LinkGeom::Arc { c: c2, a: on(a), b: on(b) },
        _ => LinkGeom::Circle(c2, r2),
    }
}

/// Chain loose 2D segments into polylines (merging coincident ends) and fit each chain.
fn fit_segments(segs: &[(Vec2, Vec2)], size: f64) -> Vec<LinkGeom> {
    // Mesh chords stray from the surface by up to the display tolerance (1/1000 of the size).
    let loose = Some(size * 2e-3);
    let q = size * 1e-7;
    let key = |p: Vec2| ((p.x / q).round() as i64, (p.y / q).round() as i64);
    let mut nodes: BTreeMap<(i64, i64), (Vec2, Vec<usize>)> = BTreeMap::new();
    let mut uniq: Vec<((i64, i64), (i64, i64))> = Vec::new();
    for (a, b) in segs {
        let (ka, kb) = (key(*a), key(*b));
        if ka == kb {
            continue;
        }
        let k = if ka <= kb { (ka, kb) } else { (kb, ka) };
        if uniq.contains(&k) {
            continue;
        }
        let si = uniq.len();
        uniq.push(k);
        nodes.entry(ka).or_insert_with(|| (*a, Vec::new())).1.push(si);
        nodes.entry(kb).or_insert_with(|| (*b, Vec::new())).1.push(si);
        if uniq.len() > 200_000 {
            break;
        }
    }
    let mut used = vec![false; uniq.len()];
    let mut out = Vec::new();
    let walk = |start: (i64, i64), used: &mut Vec<bool>| -> Vec<Vec2> {
        let mut chain = vec![nodes.get(&start).map(|n| n.0).unwrap_or_default()];
        let mut cur = start;
        while let Some((_, adj)) = nodes.get(&cur) {
            let next = adj.iter().copied().find(|s| !used.get(*s).copied().unwrap_or(true));
            let Some(s) = next else { break };
            if let Some(u) = used.get_mut(s) {
                *u = true;
            }
            let Some(&(ka, kb)) = uniq.get(s) else { break };
            cur = if ka == cur { kb } else { ka };
            chain.push(nodes.get(&cur).map(|n| n.0).unwrap_or_default());
            // Stop at junctions so each chain is a simple path.
            if nodes.get(&cur).is_some_and(|n| n.1.len() != 2) {
                break;
            }
        }
        chain
    };
    // Open chains start at ends and junctions; what is left are closed loops.
    let starts: Vec<(i64, i64)> = nodes.iter().filter(|(_, n)| n.1.len() != 2).map(|(k, _)| *k).collect();
    for s in starts {
        while nodes.get(&s).is_some_and(|n| n.1.iter().any(|e| !used.get(*e).copied().unwrap_or(true))) {
            let c = walk(s, &mut used);
            fit_chain(&c, loose, &mut out);
        }
    }
    let keys: Vec<(i64, i64)> = nodes.keys().copied().collect();
    for s in keys {
        while nodes.get(&s).is_some_and(|n| n.1.iter().any(|e| !used.get(*e).copied().unwrap_or(true))) {
            let c = walk(s, &mut used);
            fit_chain(&c, loose, &mut out);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Fitting

fn circumcircle(a: Vec2, b: Vec2, c: Vec2) -> Option<(Vec2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-18 {
        return None;
    }
    let (a2, b2, c2) = (a.len2(), b.len2(), c.len2());
    let o = Vec2::new((a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d, (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d);
    let r = o.dist(a);
    (o.is_finite() && r.is_finite() && r < 1e7).then_some((o, r))
}

/// Least-squares circle (algebraic fit, in coordinates centred on the points' mean).
fn ls_circle(p: &[Vec2]) -> Option<(Vec2, f64)> {
    let n = p.len() as f64;
    if p.len() < 3 {
        return None;
    }
    let m = p.iter().fold(Vec2::ZERO, |a, q| a + *q) / n;
    // x² + y² + D x + E y + F = 0 → normal equations.
    let (mut a, mut r) = ([[0.0f64; 3]; 3], [0.0f64; 3]);
    for q in p {
        let (x, y) = (q.x - m.x, q.y - m.y);
        let row = [x, y, 1.0];
        let rhs = -(x * x + y * y);
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += row[i] * row[j];
            }
            r[i] += row[i] * rhs;
        }
    }
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d0 = det(a);
    if d0.abs() < 1e-30 {
        return None;
    }
    let mut sol = [0.0; 3];
    for (k, s) in sol.iter_mut().enumerate() {
        let mut mk = a;
        for i in 0..3 {
            mk[i][k] = r[i];
        }
        *s = det(mk) / d0;
    }
    let c = Vec2::new(-sol[0] / 2.0, -sol[1] / 2.0);
    let rr = c.len2() - sol[2];
    let c = c + m;
    (rr > 0.0 && c.is_finite() && rr.sqrt() < 1e7).then(|| (c, rr.sqrt()))
}

fn fits_line(p: &[Vec2], tol: f64) -> bool {
    let (Some(a), Some(b)) = (p.first(), p.last()) else { return false };
    let d = *b - *a;
    let l = d.len();
    if l <= tol {
        return false;
    }
    let u = d / l;
    let mut last = -tol;
    p.iter().all(|q| {
        let t = (*q - *a).dot(u);
        let ok = u.cross(*q - *a).abs() <= tol && t >= last - tol && t <= l + tol;
        last = t;
        ok
    })
}

/// Circle through the chain (centre, radius, signed sweep from the first to the last point).
/// `exact`: the points lie on the curve (edge tessellation), else they are approximate
/// (mesh sections and silhouettes) and the circle is a least-squares fit.
fn fits_circle(p: &[Vec2], tol: f64, exact: bool) -> Option<(Vec2, f64, f64)> {
    let n = p.len();
    if n < 3 || (!exact && n < 5) {
        return None;
    }
    let closed = p.first()?.dist(*p.last()?) <= tol;
    let (c, r) = if exact {
        let (i1, i2) = if closed { (n / 3, 2 * n / 3) } else { (n / 2, n - 1) };
        circumcircle(*p.first()?, *p.get(i1)?, *p.get(i2)?)?
    } else {
        ls_circle(p)?
    };
    if r > 1e6 || p.iter().any(|q| (q.dist(c) - r).abs() > tol) {
        return None;
    }
    let mut sweep = 0.0;
    for w in p.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        let mut d = (*b - c).angle() - (*a - c).angle();
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        // Chords of a tessellated circle are short; points jumping around the circle (a
        // polygon inscribed in it) are not an arc.
        if d.abs() > 1.0 || (sweep != 0.0 && d * sweep < 0.0) {
            return None;
        }
        sweep += d;
    }
    Some((c, r, sweep))
}

fn arc_geom(p: &[Vec2], c: Vec2, r: f64, sweep: f64, tol: f64) -> Option<LinkGeom> {
    let (a, b) = (*p.first()?, *p.last()?);
    if sweep.abs() >= std::f64::consts::TAU - 1e-3 || a.dist(b) <= tol {
        return Some(LinkGeom::Circle(c, r));
    }
    // Ends exactly on the circle.
    let on = |q: Vec2| c + (q - c).normalized().unwrap_or(Vec2::X) * r;
    let (a, b) = (on(a), on(b));
    Some(if sweep > 0.0 { LinkGeom::Arc { c, a, b } } else { LinkGeom::Arc { c, a: b, b: a } })
}

/// Fit lines and circular arcs to a 2D polyline: the whole chain when it is one line or arc,
/// else greedy maximal pieces. `loose`: the points are approximate (within `loose` mm), and
/// sharp corners always split the chain.
pub(crate) fn fit_chain(raw: &[Vec2], loose: Option<f64>, out: &mut Vec<LinkGeom>) {
    let mut p: Vec<Vec2> = Vec::with_capacity(raw.len());
    let scale = raw.iter().fold(0.0_f64, |m, q| m.max(q.x.abs()).max(q.y.abs())).max(1.0);
    let ext = match (
        raw.iter().map(|q| q.x).reduce(f64::min),
        raw.iter().map(|q| q.x).reduce(f64::max),
        raw.iter().map(|q| q.y).reduce(f64::min),
        raw.iter().map(|q| q.y).reduce(f64::max),
    ) {
        (Some(x0), Some(x1), Some(y0), Some(y1)) => (x1 - x0).max(y1 - y0),
        _ => return,
    };
    let exact = loose.is_none();
    let tol = loose.unwrap_or(1e-6 * scale.max(ext)).max(1e-9);
    for q in raw {
        if !q.is_finite() {
            return;
        }
        if p.last().is_none_or(|l| l.dist(*q) > tol * 1e-3) {
            p.push(*q);
        }
    }
    if p.len() > MAX_CHAIN {
        let step = p.len().div_ceil(MAX_CHAIN);
        let last = p.last().copied();
        p = p.iter().step_by(step).copied().collect();
        if let Some(l) = last
            && p.last() != Some(&l)
        {
            p.push(l);
        }
    }
    if p.len() < 2 || ext <= tol {
        return;
    }
    if !exact {
        // Split at corners (turns sharper than 30 degrees); a closed chain starts at one.
        let n = p.len();
        let closed = n > 3 && p.first().zip(p.last()).is_some_and(|(a, b)| a.dist(*b) <= tol);
        let turn = |i: usize| -> f64 {
            let prev = if i == 0 { if closed { n.saturating_sub(2) } else { return 0.0 } } else { i - 1 };
            let next = if i + 1 >= n { if closed { 1 } else { return 0.0 } } else { i + 1 };
            match (p.get(prev), p.get(i), p.get(next)) {
                (Some(a), Some(b), Some(c)) => match ((*b - *a).normalized(), (*c - *b).normalized()) {
                    (Some(u), Some(v)) => u.cross(v).atan2(u.dot(v)).abs(),
                    _ => 0.0,
                },
                _ => 0.0,
            }
        };
        let corners: Vec<usize> = (0..n).filter(|i| turn(*i) > 30f64.to_radians()).collect();
        let cut = |a: usize, b: usize, out: &mut Vec<LinkGeom>| {
            if let Some(s) = p.get(a..=b) {
                fit_piece(s, tol, exact, out);
            }
        };
        if closed && let Some(&c0) = corners.first() {
            // Rotate so the loop starts and ends at a corner.
            let mut q: Vec<Vec2> = p.get(c0..n.saturating_sub(1)).unwrap_or_default().to_vec();
            q.extend(p.get(..=c0).unwrap_or_default());
            let m = q.len();
            let shift = |i: usize| (i + n - 1 - c0) % (n - 1);
            let mut cs: Vec<usize> = corners.iter().map(|c| shift(*c)).filter(|c| *c > 0 && *c < m - 1).collect();
            cs.sort_unstable();
            let mut start = 0;
            for c in cs.into_iter().chain([m - 1]) {
                if let Some(s) = q.get(start..=c) {
                    fit_piece(s, tol, exact, out);
                }
                start = c;
            }
            return;
        }
        let mut start = 0;
        for c in corners.into_iter().filter(|c| *c > 0 && *c < n - 1).chain([n - 1]) {
            cut(start, c, out);
            start = c;
        }
        return;
    }
    fit_piece(&p, tol, exact, out);
}

fn fit_piece(p: &[Vec2], tol: f64, exact: bool, out: &mut Vec<LinkGeom>) {
    if p.len() < 2 {
        return;
    }
    if fits_line(p, tol) {
        if let (Some(a), Some(b)) = (p.first(), p.last()) {
            out.push(LinkGeom::Line(*a, *b));
        }
        return;
    }
    if let Some((c, r, sw)) = fits_circle(p, tol, exact)
        && let Some(g) = arc_geom(p, c, r, sw, tol)
    {
        out.push(g);
        return;
    }
    // Greedy pieces.
    let mut i = 0;
    let n = p.len();
    let mut guard = 0;
    while i + 1 < n && guard < MAX_CHAIN * 2 {
        guard += 1;
        let mut jl = i + 1;
        while jl + 1 < n && p.get(i..=jl + 1).is_some_and(|s| fits_line(s, tol)) {
            jl += 1;
        }
        let mut jc = i + 1;
        let mut arc = None;
        while jc + 1 < n {
            match p.get(i..=jc + 1).and_then(|s| fits_circle(s, tol, exact)) {
                Some(f) => {
                    arc = Some(f);
                    jc += 1;
                }
                None => break,
            }
        }
        if jc >= jl + 2
            && jc - i >= 3
            && let Some((c, r, sw)) = arc
            && let Some(g) = p.get(i..=jc).and_then(|s| arc_geom(s, c, r, sw, tol))
        {
            out.push(g);
            i = jc;
        } else {
            if let (Some(a), Some(b)) = (p.get(i), p.get(jl)) {
                out.push(LinkGeom::Line(*a, *b));
            }
            i = jl;
        }
    }
}
