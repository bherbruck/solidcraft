//! Sketches made from a planar body face, so a face can be extruded like a profile (Extrude
//! with `face`, Press Pull). The face boundary is copied into a new sketch on the face: straight
//! edges become lines, circular edges circles or arcs, anything else a chain of lines.

use serde_json::{Value, json};
use solidcraft_doc::{EmbossFace, apply_point, mat_inverse, mat_mul};
use solidcraft_geom::{Mesh, Vec2, Vec3};

use crate::params::{bad, vec3};
use crate::{Result, Session};

fn point_triangle_dist(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f64 {
    let n = (b - a).cross(c - a);
    let Some(nn) = n.normalized() else { return f64::INFINITY };
    let h = (p - a).dot(nn);
    let q = p - nn * h;
    let inside = [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).cross(q - *u).dot(n) >= -1e-12);
    if inside { h.abs() } else { [(a, b), (b, c), (c, a)].iter().map(|(u, v)| p.dist_to_segment(*u, *v)).fold(f64::INFINITY, f64::min) }
}

/// The outward normal of the planar body face at `p`.
pub(super) fn face_normal(s: &Session, p: Vec3) -> Option<Vec3> {
    face_normal_in(s, p, None)
}

/// The outward normal of the planar face at `p` of `body` (any body when `None`).
pub(super) fn face_normal_in(s: &Session, p: Vec3, body: Option<&str>) -> Option<Vec3> {
    let (m, f, d, _) = face_at_in(s, p, body).ok().flatten()?;
    let tol = (m.bounds().diagonal() * 1e-3).max(1e-3);
    if d > tol * 10.0 {
        return None;
    }
    m.triangles
        .iter()
        .zip(&m.tri_face)
        .filter(|(_, tf)| **tf == f)
        .filter_map(|(t, _)| m.tri(t))
        .find_map(|[a, b, c]| (b - a).cross(c - a).normalized())
}

/// Is the body face at `p` planar (`None`: no face there)?
pub(super) fn face_is_planar(s: &Session, p: Vec3) -> Option<bool> {
    let (m, f, d, _) = face_at(s, p)?;
    let tol = (m.bounds().diagonal() * 1e-3).max(1e-3);
    if d > tol * 10.0 {
        return None;
    }
    let ns: Vec<Vec3> = m
        .triangles
        .iter()
        .zip(&m.tri_face)
        .filter(|(_, tf)| **tf == f)
        .filter_map(|(t, _)| m.tri(t))
        .filter_map(|[a, b, c]| (b - a).cross(c - a).normalized())
        .collect();
    let n0 = *ns.first()?;
    Some(ns.iter().all(|n| n.dot(n0) > 1.0 - 1e-6))
}

/// A face given to a command: a point `[x, y, z]` on it (or `{point}`), or its name as
/// `model.faces` lists it, looked up on `body` (any body without one). A name becomes a point on
/// that face, which the feature keeps like a picked one, so it also keeps the face's name.
/// The point is in the frame of the command's other picks: the component's own when
/// they were mapped into it, else the world's.
pub(super) fn face_ref(s: &Session, v: &Value, body: Option<&str>, cmd: &str) -> Result<Vec3> {
    if let Some(name) = v.as_str() {
        if name.is_empty() || name.len() > 512 {
            return Err(bad(cmd, "a face name is 1…512 characters"));
        }
        let st = if crate::frames::mapped() { s.model.state() } else { s.world_state() };
        return solidcraft_doc::naming::named_face_point(&st, body, name).map_err(|why| bad(cmd, why));
    }
    vec3(v).or_else(|| v.get("point").and_then(vec3)).ok_or_else(|| bad(cmd, "a face is a point [x, y, z] on it, or its name from model.faces"))
}

/// The body face under a point: (mesh, face index, distance, body).
fn face_at(s: &Session, p: Vec3) -> Option<(std::sync::Arc<Mesh>, u32, f64, String)> {
    face_at_in(s, p, None).ok().flatten()
}

type FacePick = (std::sync::Arc<Mesh>, u32, f64, String);

/// The face under a point of `only` (any body when `None`). Where faces of several bodies are
/// equally near (bodies touching flush), the pick is ambiguous: `Err` names those bodies.
fn face_at_in(s: &Session, p: Vec3, only: Option<&str>) -> std::result::Result<Option<FacePick>, Vec<String>> {
    let st = s.model.state();
    face_at_state(&st, p, only)
}

pub(super) fn face_at_state(st: &solidcraft_doc::ModelState, p: Vec3, only: Option<&str>) -> std::result::Result<Option<FacePick>, Vec<String>> {
    let mut best: Option<FacePick> = None;
    let mut tied: Vec<String> = Vec::new();
    for b in st.bodies.iter().filter(|b| only.is_none_or(|n| b.name == n)) {
        let m = b.mesh();
        let mut near: Option<(u32, f64)> = None;
        for (t, f) in m.triangles.iter().zip(&m.tri_face) {
            let Some([a, bb, c]) = m.tri(t) else { continue };
            let d = point_triangle_dist(p, a, bb, c);
            if near.is_none_or(|(_, nd)| d < nd) {
                near = Some((*f, d));
            }
        }
        if let Some((f, d)) = near {
            if best.as_ref().is_none_or(|x| d < x.2 - 1e-8) {
                best = Some((m.clone(), f, d, b.name.clone()));
                tied.clear();
            } else if best.as_ref().is_some_and(|x| (d - x.2).abs() <= 1e-8) {
                tied.push(b.name.clone());
            }
        }
    }
    match best {
        Some(b) if !tied.is_empty() => Err(std::iter::once(b.3).chain(tied).collect()),
        b => Ok(b),
    }
}

/// Circle through three points.
fn circumcircle(a: Vec2, b: Vec2, c: Vec2) -> Option<(Vec2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a2, b2, c2) = (a.len2(), b.len2(), c.len2());
    let o = Vec2::new((a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d, (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d);
    let r = o.dist(a);
    (o.is_finite() && r.is_finite() && r < 1e8).then_some((o, r))
}

fn xy(v: Vec2) -> Value {
    json!([v.x, v.y])
}

/// Coplanar faces share one linked profile sketch and one Extrude feature. Resolve every
/// pick before editing the document, so later picks never search an already modified body.
pub(super) fn sketch_of_faces(s: &mut Session, points: &[Vec3], cmd: &str, face_body: Option<&str>) -> Result<(u64, Vec<Vec2>, String)> {
    let first = points.first().copied().ok_or_else(|| bad(cmd, "faces must not be empty"))?;
    if points.len() > 1024 {
        return Err(bad(cmd, "faces exceed the 1024-face limit"));
    }
    let state = s.model.state();
    let mut picked = Vec::new();
    let mut common: Option<(String, solidcraft_geom::Plane)> = None;
    for &point in points {
        if !point.is_finite() {
            return Err(bad(cmd, "faces must be finite points"));
        }
        let (mesh, index, distance, name) = face_at_in(s, point, face_body)
            .map_err(|_| bad(cmd, "ambiguous face; specify face_body"))?
            .ok_or_else(|| bad(cmd, "no body face at a selected point"))?;
        let body = state.body(&name).ok_or_else(|| bad(cmd, "selected body disappeared"))?;
        let plane = body.body.planar_face(index as usize).ok_or_else(|| bad(cmd, "faces must be planar"))?;
        let tolerance = (body.body.size() * 1e-9).max(1e-8);
        if distance > (mesh.bounds().diagonal() * 1e-3).max(1e-3) * 10.0 {
            return Err(bad(cmd, "no body face at a selected point"));
        }
        if let Some((previous, base)) = &common {
            if *previous != name || base.normal().dot(plane.normal()) < 1.0 - 1e-9 || base.height(plane.origin).abs() > tolerance {
                return Err(bad(cmd, "faces must be coplanar, equally oriented faces of one body"));
            }
        } else {
            common = Some((name.clone(), plane));
        }
        if picked.iter().any(|(face, _)| *face == index as usize) {
            continue;
        }
        let inside = mesh
            .triangles
            .iter()
            .zip(&mesh.tri_face)
            .filter(|(_, face)| **face == index)
            .filter_map(|(triangle, _)| mesh.tri(triangle))
            .max_by(|[a, b, c], [x, y, z]| ((*b - *a).cross(*c - *a).len()).total_cmp(&((*y - *x).cross(*z - *x).len())))
            .map(|[a, b, c]| (a + b + c) / 3.0)
            .ok_or_else(|| bad(cmd, "degenerate face"))?;
        picked.push((index as usize, inside));
    }
    let (id, first_inside, _) = sketch_of_face(s, first, cmd, face_body)?;
    let (body, _) = common.ok_or_else(|| bad(cmd, "no faces selected"))?;
    let doc = s.placed_document();
    let source = state.body(&body).ok_or_else(|| bad(cmd, "source body disappeared"))?;
    let component = doc.model_body_component(source);
    let path = if component == s.active_component {
        s.active_occurrence_path.clone().unwrap_or_else(|| doc.first_path(component))
    } else {
        let placements = super::component::properties::placements(&doc, cmd)?;
        if placements.iter().filter(|(owner, _, _)| *owner == component).count() != 1 {
            return Err(bad(cmd, "activate the selected occurrence before extruding its faces"));
        }
        doc.first_path(component)
    };
    let placement = EmbossFace::component_frame(&doc, component, &path)?;
    let placement = doc.model_body_offset(source).map(|offset| mat_mul(&placement, offset)).unwrap_or(placement);
    let author_path = s.active_occurrence_path.clone().unwrap_or_else(|| doc.first_path(s.active_component));
    let author = EmbossFace::component_frame(&doc, s.active_component, &author_path)?;
    let inverse = mat_inverse(&author).ok_or_else(|| bad(cmd, "invalid face profile author frame"))?;
    let into_author = mat_mul(&inverse, &placement);
    drop(doc);
    let indices: Vec<_> = picked.iter().skip(1).map(|(index, _)| *index).collect();
    super::sketch_project::project_internal_faces(s, id, &body, &indices)?;
    s.refresh();
    let plane = s.model.state().sketch(id).map(|sketch| sketch.plane).ok_or_else(|| bad(cmd, "face profile sketch disappeared"))?;
    let mut selected = vec![first_inside];
    selected.extend(picked.iter().skip(1).map(|(_, point)| plane.to_local(apply_point(&into_author, *point))));
    Ok((id, selected, body))
}

/// Start a sketch on the planar face at `point` holding a copy of the face boundary. Returns
/// the sketch id, a point inside the face (sketch coordinates), and the picked body's name.
/// The sketch is left active.
pub(super) fn sketch_of_face(s: &mut Session, point: Vec3, cmd: &str, face_body: Option<&str>) -> Result<(u64, Vec2, String)> {
    let none = || match face_body {
        Some(b) => bad(cmd, format!("no face of `{b}` at `face`")),
        None => bad(cmd, "no body face at `face`"),
    };
    let (m, f, d, body) = face_at_in(s, point, face_body)
        .map_err(|names| bad(cmd, format!("faces of several bodies meet at `face` ({}): name one with `face_body`", names.join(", "))))?
        .ok_or_else(none)?;
    let tol = (m.bounds().diagonal() * 1e-3).max(1e-3);
    if d > tol * 10.0 {
        return Err(none());
    }
    let tris: Vec<[Vec3; 3]> = m.triangles.iter().zip(&m.tri_face).filter(|(_, tf)| **tf == f).filter_map(|(t, _)| m.tri(t)).collect();
    let normal = tris.iter().find_map(|[a, b, c]| (*b - *a).cross(*c - *a).normalized()).ok_or_else(|| bad(cmd, "degenerate face"))?;
    if tris.iter().any(|[a, b, c]| (*b - *a).cross(*c - *a).normalized().is_some_and(|n| n.dot(normal) < 1.0 - 1e-6)) {
        return Err(bad(cmd, "only planar faces can be extruded"));
    }
    // A point well inside the face: the centroid of its largest triangle.
    let inside = tris
        .iter()
        .max_by(|x, y| (x[1] - x[0]).cross(x[2] - x[0]).len().total_cmp(&(y[1] - y[0]).cross(y[2] - y[0]).len()))
        .map(|[a, b, c]| (*a + *b + *c) / 3.0)
        .unwrap_or(point);
    let edges: Vec<Vec<Vec3>> =
        m.face_edges(f).into_iter().filter(|e| !m.seams.get(*e).copied().unwrap_or(false)).filter_map(|e| m.edges.get(e).cloned()).collect();
    if edges.is_empty() || edges.len() > 5000 {
        return Err(bad(cmd, "the face has no usable boundary"));
    }
    let run = |s: &mut Session, id: &str, p: Value| -> Result<Value> {
        let spec = crate::find_command(id).ok_or_else(|| bad(cmd, id))?;
        (spec.run)(s, &p)
    };
    // Command frame normalization already made `point` definition-local. Capture takes
    // world coordinates; convert exactly once using the picked body's actual occurrence.
    let state = s.model.state();
    let doc = s.placed_document();
    let picked = state.body(&body).ok_or_else(|| bad(cmd, "picked body disappeared"))?;
    let component = doc.model_body_component(picked);
    let path = if component == s.active_component {
        s.active_occurrence_path.clone().unwrap_or_else(|| doc.first_path(component))
    } else {
        // The command mapper discarded the original placement when making this pick
        // definition-local. A repeated cross-owner source cannot be re-found safely.
        let placements = super::component::properties::placements(&doc, cmd)?;
        if placements.iter().filter(|(owner, _, _)| *owner == component).count() != 1 {
            return Err(bad(cmd, "activate the picked occurrence before generating a face profile from a repeated component"));
        }
        doc.first_path(component)
    };
    let placement = EmbossFace::component_frame(&doc, component, &path)?;
    let placement = doc.model_body_offset(picked).map(|offset| mat_mul(&placement, offset)).unwrap_or(placement);
    let world_inside = apply_point(&placement, inside);
    let world_body = doc.world_body_name(&state, &body, component, &path);
    let author_path = s.active_occurrence_path.clone().unwrap_or_else(|| doc.first_path(s.active_component));
    let author = EmbossFace::component_frame(&doc, s.active_component, &author_path)?;
    let author_inverse = mat_inverse(&author).ok_or_else(|| bad(cmd, "invalid face profile author frame"))?;
    let author_inside = apply_point(&author_inverse, world_inside);
    drop(doc);
    let r = super::sketch::create_face_profile(
        s,
        &json!({"plane": {"face": [world_inside.x, world_inside.y, world_inside.z], "body": world_body, "index": f, "path":path}, "project_edges": true}),
        &body,
        f as usize,
    )?;
    let sid = r.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "could not start a sketch on the face"))?;
    s.refresh();
    let plane = s.model.state().sketch(sid).map(|ss| ss.plane).ok_or_else(|| bad(cmd, "could not start a sketch on the face"))?;
    // The face's edges came in as linked projections: they follow the face when earlier
    // features change, so nothing is copied.
    if r.get("projected").is_some_and(|x| !x.is_null()) {
        return Ok((sid, plane.to_local(author_inside), body));
    }
    for e in &edges {
        let pts: Vec<Vec2> = e.iter().map(|p| plane.to_local(*p)).collect();
        let (Some(first), Some(last)) = (pts.first().copied(), pts.last().copied()) else { continue };
        let closed = pts.len() > 3 && first.dist(last) < tol;
        let circle = if pts.len() >= 4 {
            let n = pts.len() - usize::from(closed);
            let (a, b, c) = (pts.first(), pts.get(n / 3), pts.get(2 * n / 3));
            match (a, b, c) {
                (Some(a), Some(b), Some(c)) => circumcircle(*a, *b, *c).filter(|(o, r)| pts.iter().all(|q| (q.dist(*o) - r).abs() < tol)),
                _ => None,
            }
        } else {
            None
        };
        match circle {
            Some((o, r)) if closed => run(s, "sketch.circle.center", json!({"center": xy(o), "radius": r}))?,
            Some(_) => {
                let mid = pts.get(pts.len() / 2).copied().unwrap_or(first);
                run(s, "sketch.arc.three_point", json!({"start": xy(first), "end": xy(last), "through": xy(mid)}))?
            }
            None => {
                let mut list: Vec<Value> = Vec::new();
                for q in &pts {
                    // Drop points that repeat (a closed polyline ends where it starts).
                    if list.last().is_none_or(|l| l != &xy(*q)) {
                        list.push(xy(*q));
                    }
                }
                let closed_poly = list.len() > 2 && list.first() == list.last();
                if closed_poly {
                    list.pop();
                }
                run(s, "sketch.line", json!({"points": list, "closed": closed_poly}))?
            }
        };
    }
    Ok((sid, plane.to_local(author_inside), body))
}

#[cfg(test)]
#[path = "face_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "face_join_tests.rs"]
mod join_tests;
