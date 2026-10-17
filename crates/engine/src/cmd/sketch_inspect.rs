//! Inspection: curvature combs and minimum radius on sketch curves, model edges and faces;
//! centre of mass of bodies.

use serde_json::{Value, json};
use solidcraft_geom::{Seg2, Vec2, Vec3};

use super::CommandSpec;
use crate::params::{bad, num, string_list, vec3};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("inspect.curvature_comb", "Curvature Comb Analysis", curvature_comb)
        .at("SKETCH", "INSPECT")
        .icon("curvature_comb")
        .noundo()
        .params("curves?: [sketch curve ids] (sketch?: id|name, default active) | edges?: [[x,y,z] points on body edges] | faces?: [[x,y,z] points on faces] with count?: isoparametric curves each way (default 4; combs along the face's iso grid); density?: teeth per curve (default 40), scale?: comb length per unit curvature (default auto)"),
    CommandSpec::new("inspect.minimum_radius", "Minimum Radius Analysis", minimum_radius)
        .at("SKETCH", "INSPECT")
        .icon("min_radius")
        .noundo()
        .params("curves?: [sketch curve ids] (sketch?) | edges?: [[x,y,z]…] | faces?: [[x,y,z]…] (default: every face of every body); the smallest radius of curvature of each and where it is"),
    CommandSpec::new("inspect.isocurve", "Isocurve Analysis", isocurves)
        .at("SKETCH", "INSPECT")
        .icon("iso_analysis")
        .noundo()
        .params("faces: [[x,y,z]…] points on faces; count?: curves each way (default 8): the faces' isoparametric curves"),
    CommandSpec::new("inspect.center_of_mass", "Center of Mass", center_of_mass)
        .at("SKETCH", "INSPECT")
        .icon("center_of_mass")
        .noundo()
        .params("bodies?: [names] (default all): the centre of mass (uniform density) of each and of all together"),
];

/// Curvature (signed, 1/mm) and unit normal of a segment at `t`.
fn seg_curvature(s: &Seg2, t: f64) -> (f64, Vec2) {
    let d1 = s.derivative(t);
    let h = 1e-5;
    let (ta, tb) = ((t - h).max(0.0), (t + h).min(1.0));
    let d2 = (s.derivative(tb) - s.derivative(ta)) / (tb - ta).max(1e-12);
    let l = d1.len().max(1e-300);
    let k = d1.cross(d2) / (l * l * l);
    (k, d1.perp() / l)
}

/// Teeth along a 2D curve: (foot, curvature, normal).
fn teeth_2d(segs: &[Seg2], density: usize) -> Vec<(Vec2, f64, Vec2)> {
    let per = density.div_ceil(segs.len().max(1)).max(2);
    let mut out = Vec::new();
    for s in segs {
        for i in 0..=per {
            let t = i as f64 / per as f64;
            let (k, n) = seg_curvature(s, t);
            out.push((s.point_at(t), k, n));
        }
    }
    out
}

/// Teeth along a 3D polyline (discrete curvature from three consecutive points).
fn teeth_3d(pts: &[Vec3]) -> Vec<(Vec3, f64, Vec3)> {
    let mut out = Vec::new();
    for w in pts.windows(3) {
        let (a, b, c) = (w[0], w[1], w[2]);
        let (ab, bc, ca) = (b - a, c - b, a - c);
        let area2 = ab.cross(bc).len();
        let den = ab.len() * bc.len() * ca.len();
        let k = if den > 1e-300 { 2.0 * area2 / den } else { 0.0 };
        // Towards the centre of curvature.
        let n = (a + c - b * 2.0).normalized().unwrap_or(Vec3::ZERO);
        out.push((b, k, n));
    }
    out
}

fn curvature_comb(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.curvature_comb";
    let density = num(p, "density").map(|d| d.clamp(2.0, 2000.0) as usize).unwrap_or(40);
    let st = s.model.state();
    let mut combs: Vec<(String, Vec<(Vec3, f64, Vec3)>)> = Vec::new();
    let curves = string_list(p, "curves");
    if !curves.is_empty() {
        let id = match p.get("sketch") {
            Some(Value::Number(n)) => n.as_u64(),
            Some(Value::String(x)) => s.doc.find_feature(x).map(|f| f.id),
            _ => s.active_sketch,
        }
        .ok_or_else(|| bad(cmd, "no such sketch (and none is active)"))?;
        let ss = st.sketch(id).ok_or_else(|| EngineError::Other("that sketch is not evaluated".into()))?;
        for c in curves.iter().take(1000) {
            let ci = ss.sketch.curve_index(c).ok_or_else(|| bad(cmd, format!("unknown curve `{c}`")))?;
            let segs = ss.sketch.exact_segs(ci).unwrap_or_else(|| ss.sketch.segs(ci));
            let teeth = teeth_2d(&segs, density).into_iter().map(|(q, k, n)| (ss.plane.to_world(q), k, ss.plane.dir_to_world(n))).collect();
            combs.push((c.clone(), teeth));
        }
    }
    if let Some(edges) = p.get("edges").and_then(Value::as_array) {
        for e in edges.iter().take(1000) {
            let at = vec3(e).ok_or_else(|| bad(cmd, "`edges` must be points [x,y,z]"))?;
            let mut best: Option<(f64, Vec<Vec3>)> = None;
            for b in &st.bodies {
                let m = b.mesh();
                for (i, pts) in m.edges.iter().enumerate() {
                    if m.seams.get(i).copied().unwrap_or(false) {
                        continue;
                    }
                    let d = pts.windows(2).map(|w| at.dist_to_segment(w[0], w[1])).fold(f64::INFINITY, f64::min);
                    if best.as_ref().is_none_or(|x| d < x.0) {
                        best = Some((d, pts.clone()));
                    }
                }
            }
            let (_, pts) = best.ok_or_else(|| bad(cmd, "no edge there"))?;
            combs.push((format!("edge@{},{},{}", at.x, at.y, at.z), teeth_3d(&pts)));
        }
    }
    if let Some(faces) = p.get("faces") {
        let faces = faces.as_array().filter(|a| !a.is_empty()).ok_or_else(|| bad(cmd, "`faces` must list points [x,y,z]"))?;
        let count = match p.get("count") {
            None | Some(Value::Null) => 4,
            Some(_) => {
                num(p, "count").filter(|c| c.is_finite()).map(|c| c.clamp(1.0, 50.0) as usize).ok_or_else(|| bad(cmd, "`count` must be a number"))?
            }
        };
        for v in faces.iter().take(100) {
            let at = vec3(v).ok_or_else(|| bad(cmd, "`faces` must list points [x,y,z]"))?;
            let (bi, f) = face_at(&st, at).ok_or_else(|| bad(cmd, "no face there"))?;
            let m = st.bodies.get(bi).map(|b| b.mesh()).ok_or_else(|| bad(cmd, "that face's body is not evaluated"))?;
            for (k, w) in solidcraft_doc::iso_grid(&m, f, at, count).iter().enumerate() {
                combs.push((format!("face@{},{},{}#{k}", at.x, at.y, at.z), teeth_3d(w)));
            }
        }
    }
    if combs.is_empty() {
        return Err(bad(cmd, "give `curves`, `edges` or `faces`"));
    }
    // One scale for all combs: the longest tooth is a fifth of the largest curve's size.
    let kmax = combs.iter().flat_map(|c| c.1.iter().map(|t| t.1.abs())).fold(0.0_f64, f64::max);
    let size = combs.iter().flat_map(|c| c.1.iter().map(|t| t.0)).fold(
        (Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY)),
        |(lo, hi), q| (lo.min(q), hi.max(q)),
    );
    let diag = (size.1 - size.0).len();
    let scale = num(p, "scale").filter(|x| *x > 0.0).unwrap_or(if kmax > 1e-12 && diag.is_finite() { 0.2 * diag / kmax } else { 1.0 });
    let out: Vec<Value> = combs
        .iter()
        .map(|(id, teeth)| {
            let lines: Vec<[[f64; 3]; 2]> = teeth.iter().map(|(q, k, n)| [(*q).into(), (*q - *n * (k * scale)).into()]).collect();
            let kmax_c = teeth.iter().map(|t| t.1.abs()).fold(0.0_f64, f64::max);
            json!({
                "curve": id,
                "teeth": lines,
                "envelope": lines.iter().map(|l| l[1]).collect::<Vec<_>>(),
                "max_curvature": kmax_c,
                "min_radius": if kmax_c > 1e-12 { Value::from(1.0 / kmax_c) } else { Value::Null },
            })
        })
        .collect();
    Ok(json!({"combs": out, "scale": scale}))
}

/// Smallest radius along teeth: (radius, where), or None when straight.
fn min_of(teeth: &[(Vec3, f64, Vec3)]) -> Option<(f64, Vec3)> {
    teeth.iter().filter(|t| t.1.abs() > 1e-9).max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|t| (1.0 / t.1.abs(), t.0))
}

/// The face of a body at `at` (nearest triangle): (body index, face index).
fn face_at(st: &solidcraft_doc::ModelState, at: Vec3) -> Option<(usize, u32)> {
    let mut best: Option<(f64, usize, u32)> = None;
    for (bi, b) in st.bodies.iter().enumerate() {
        let m = b.mesh();
        for (ti, t) in m.triangles.iter().enumerate() {
            let [a, bb, c] = t.map(|i| m.positions.get(i as usize).copied().unwrap_or_default());
            let d = at.dist(closest_on_triangle(at, a, bb, c));
            if best.is_none_or(|x| d < x.0) {
                best = Some((d, bi, m.tri_face.get(ti).copied().unwrap_or(0)));
            }
        }
    }
    best.filter(|b| b.0 < 1.0).map(|b| (b.1, b.2))
}

fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    // Project onto the plane, then clamp to the edges if outside.
    let n = (b - a).cross(c - a);
    let nn = n.dot(n);
    if nn < 1e-300 {
        return a;
    }
    let q = p - n * ((p - a).dot(n) / nn);
    let inside = |u: Vec3, v: Vec3| (v - u).cross(q - u).dot(n) >= 0.0;
    if inside(a, b) && inside(b, c) && inside(c, a) {
        return q;
    }
    let seg = |u: Vec3, v: Vec3| {
        let d = v - u;
        let t = if d.dot(d) > 1e-300 { ((p - u).dot(d) / d.dot(d)).clamp(0.0, 1.0) } else { 0.0 };
        u + d * t
    };
    [seg(a, b), seg(b, c), seg(c, a)].into_iter().min_by(|x, y| x.dist(p).total_cmp(&y.dist(p))).unwrap_or(a)
}

/// Smallest radius of curvature over a face of a mesh, from the turn of the vertex normals
/// along each triangle side: (radius, where).
fn face_min_radius(m: &solidcraft_geom::Mesh, face: u32) -> Option<(f64, Vec3)> {
    let mut best: Option<(f64, Vec3)> = None;
    for (ti, t) in m.triangles.iter().enumerate() {
        if m.tri_face.get(ti) != Some(&face) {
            continue;
        }
        for k in 0..3 {
            let (i, j) = (t[k] as usize, t[(k + 1) % 3] as usize);
            let (Some(pi), Some(pj), Some(ni), Some(nj)) = (m.positions.get(i), m.positions.get(j), m.normals.get(i), m.normals.get(j)) else {
                continue;
            };
            let d = *pj - *pi;
            let dd = d.dot(d);
            if dd < 1e-18 {
                continue;
            }
            let kappa = ((*nj - *ni).dot(d) / dd).abs();
            if kappa > 1e-6 && best.is_none_or(|b| 1.0 / kappa < b.0) {
                best = Some((1.0 / kappa, (*pi + *pj) * 0.5));
            }
        }
    }
    best
}

fn minimum_radius(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.minimum_radius";
    let st = s.model.state();
    let mut items: Vec<(String, Option<(f64, Vec3)>)> = Vec::new();
    let curves = string_list(p, "curves");
    if !curves.is_empty() {
        let id = match p.get("sketch") {
            Some(Value::Number(n)) => n.as_u64(),
            Some(Value::String(x)) => s.doc.find_feature(x).map(|f| f.id),
            _ => s.active_sketch,
        }
        .ok_or_else(|| bad(cmd, "no such sketch (and none is active)"))?;
        let ss = st.sketch(id).ok_or_else(|| EngineError::Other("that sketch is not evaluated".into()))?;
        for c in curves.iter().take(1000) {
            let ci = ss.sketch.curve_index(c).ok_or_else(|| bad(cmd, format!("unknown curve `{c}`")))?;
            let segs = ss.sketch.exact_segs(ci).unwrap_or_else(|| ss.sketch.segs(ci));
            let teeth: Vec<(Vec3, f64, Vec3)> =
                teeth_2d(&segs, 400).into_iter().map(|(q, k, n)| (ss.plane.to_world(q), k, ss.plane.dir_to_world(n))).collect();
            items.push((c.clone(), min_of(&teeth)));
        }
    }
    if let Some(edges) = p.get("edges").and_then(Value::as_array) {
        for e in edges.iter().take(1000) {
            let at = vec3(e).ok_or_else(|| bad(cmd, "`edges` must be points [x,y,z]"))?;
            let mut best: Option<(f64, Vec<Vec3>)> = None;
            for b in &st.bodies {
                let m = b.mesh();
                for (i, pts) in m.edges.iter().enumerate() {
                    if m.seams.get(i).copied().unwrap_or(false) {
                        continue;
                    }
                    let d = pts.windows(2).map(|w| at.dist_to_segment(w[0], w[1])).fold(f64::INFINITY, f64::min);
                    if best.as_ref().is_none_or(|x| d < x.0) {
                        best = Some((d, pts.clone()));
                    }
                }
            }
            let (_, pts) = best.ok_or_else(|| bad(cmd, "no edge there"))?;
            items.push((format!("edge@{},{},{}", at.x, at.y, at.z), min_of(&teeth_3d(&pts))));
        }
    }
    let faces: Vec<Vec3> = match p.get("faces").and_then(Value::as_array) {
        Some(a) => a.iter().take(1000).map(|v| vec3(v).ok_or_else(|| bad(cmd, "`faces` must be points [x,y,z]"))).collect::<Result<_>>()?,
        None => Vec::new(),
    };
    for at in &faces {
        let (bi, f) = face_at(&st, *at).ok_or_else(|| bad(cmd, "no face there"))?;
        let m = st.bodies.get(bi).map(|b| b.mesh()).ok_or_else(|| bad(cmd, "body"))?;
        items.push((format!("face@{},{},{}", at.x, at.y, at.z), face_min_radius(&m, f)));
    }
    if items.is_empty() {
        // Every face of every body.
        for b in &st.bodies {
            let m = b.mesh();
            let n = m.tri_face.iter().copied().max().map(|x| x + 1).unwrap_or(0);
            for f in 0..n.min(100_000) {
                items.push((format!("{}:face{f}", b.name), face_min_radius(&m, f)));
            }
        }
    }
    let overall = items.iter().filter_map(|i| i.1).min_by(|a, b| a.0.total_cmp(&b.0));
    let fmt = |x: Option<(f64, Vec3)>| match x {
        Some((r, at)) => json!({"min_radius": r, "at": [at.x, at.y, at.z]}),
        None => json!({"min_radius": null}),
    };
    Ok(json!({
        "items": items.iter().map(|(id, x)| { let mut v = fmt(*x); v["item"] = json!(id); v }).collect::<Vec<_>>(),
        "min_radius": overall.map(|o| o.0),
        "at": overall.map(|o| [o.1.x, o.1.y, o.1.z]),
    }))
}

fn isocurves(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.isocurve";
    let count = num(p, "count").map(|c| c.clamp(1.0, 100.0) as usize).unwrap_or(8);
    let faces = p.get("faces").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`faces` must list points [x,y,z]"))?;
    if faces.is_empty() {
        return Err(bad(cmd, "`faces` is empty"));
    }
    let st = s.model.state();
    let mut out = Vec::new();
    for v in faces.iter().take(100) {
        let at = vec3(v).ok_or_else(|| bad(cmd, "`faces` must list points [x,y,z]"))?;
        let (bi, f) = face_at(&st, at).ok_or_else(|| bad(cmd, "no face there"))?;
        let m = st.bodies.get(bi).map(|b| b.mesh()).ok_or_else(|| bad(cmd, "body"))?;
        for w in solidcraft_doc::iso_grid(&m, f, at, count) {
            out.push(w.iter().map(|q| [q.x, q.y, q.z]).collect::<Vec<_>>());
        }
    }
    Ok(json!({"curves": out}))
}

fn center_of_mass(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.center_of_mass";
    let names = string_list(p, "bodies");
    let st = s.model.state();
    let mut out = Vec::new();
    let (mut vol, mut moment) = (0.0, Vec3::ZERO);
    for b in st.bodies.iter().filter(|b| names.is_empty() || names.contains(&b.name)) {
        let m = b.mesh().measure();
        if !(m.volume.is_finite() && m.centroid.is_finite()) {
            continue;
        }
        vol += m.volume;
        moment += m.centroid * m.volume;
        out.push(json!({"body": b.name, "center": [m.centroid.x, m.centroid.y, m.centroid.z], "volume": m.volume}));
    }
    if out.is_empty() {
        return Err(bad(cmd, if names.is_empty() { "there are no bodies" } else { "no such bodies" }));
    }
    let c = if vol.abs() > 1e-300 { moment / vol } else { Vec3::ZERO };
    Ok(json!({"center": [c.x, c.y, c.z], "volume": vol, "bodies": out}))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Session;

    #[test]
    fn curvature_combs() {
        let mut s = Session::default();
        s.execute("sketch.create", &json!({"plane": "XY"})).unwrap();
        let c = s.execute("sketch.circle.center", &json!({"center": [0, 0], "radius": 5})).unwrap()["curves"][0].clone();
        let l = s.execute("sketch.line", &json!({"points": [[0, 20], [10, 20]]})).unwrap()["curves"][0].clone();
        let r = s.execute("inspect.curvature_comb", &json!({"curves": [c, l], "density": 20})).unwrap();
        let circle = &r["combs"][0];
        assert!((circle["min_radius"].as_f64().unwrap() - 5.0).abs() < 1e-3, "{circle}");
        assert!(r["combs"][1]["min_radius"].is_null(), "a line is straight");
        // Circle teeth stand out on the convex side, away from the centre.
        let t = &circle["teeth"][3];
        let (a, b) = ((t[0][0].as_f64().unwrap(), t[0][1].as_f64().unwrap()), (t[1][0].as_f64().unwrap(), t[1][1].as_f64().unwrap()));
        assert!(b.0 * b.0 + b.1 * b.1 > a.0 * a.0 + a.1 * a.1);
        s.execute("sketch.finish", &json!({})).unwrap();
        s.execute("solid.cylinder", &json!({"base": [50, 0, 0], "radius": 8, "height": 10})).unwrap();
        let r = s.execute("inspect.curvature_comb", &json!({"edges": [[58, 0, 10]]})).unwrap();
        assert!((r["combs"][0]["min_radius"].as_f64().unwrap() - 8.0).abs() < 0.5, "{r}");
        assert!(s.execute("inspect.curvature_comb", &json!({})).is_err());
        // Combs along the side face's isocurves: rings of radius 8 and straight lines along it.
        let r = s.execute("inspect.curvature_comb", &json!({"faces": [[58, 0, 5]], "count": 3})).unwrap();
        let combs = r["combs"].as_array().unwrap();
        assert!(combs.len() >= 6, "{}", combs.len());
        let rings = combs.iter().filter(|c| c["min_radius"].as_f64().is_some_and(|m| (m - 8.0).abs() < 0.5)).count();
        let straight = combs.iter().filter(|c| c["min_radius"].is_null() || c["min_radius"].as_f64().is_some_and(|m| m > 1e3)).count();
        assert!(rings >= 3 && straight >= 3, "{rings} rings, {straight} straight: {r}");
        assert!(s.execute("inspect.curvature_comb", &json!({"faces": []})).is_err());
        assert!(s.execute("inspect.curvature_comb", &json!({"faces": [[58, 0, 5]], "count": "x"})).is_err());
    }

    #[test]
    fn minimum_radius_and_center_of_mass() {
        let mut s = Session::default();
        s.execute("sketch.create", &json!({"plane": "XY"})).unwrap();
        let e = s.execute("sketch.ellipse", &json!({"center": [0, 0], "major": [10, 0], "minor_radius": 4})).unwrap()["curves"][0].clone();
        // An ellipse's tightest bend is at the major axis ends: b²/a.
        let r = s.execute("inspect.minimum_radius", &json!({"curves": [e]})).unwrap();
        assert!((r["min_radius"].as_f64().unwrap() - 1.6).abs() < 1e-3, "{r}");
        assert!((r["at"][0].as_f64().unwrap().abs() - 10.0).abs() < 0.1, "{r}");
        s.execute("sketch.finish", &json!({})).unwrap();
        s.execute("solid.cylinder", &json!({"base": [50, 0, 0], "radius": 8, "height": 10})).unwrap();
        // The cylinder's side face: radius 8 (the mesh estimate is close).
        let r = s.execute("inspect.minimum_radius", &json!({"faces": [[58, 0, 5]]})).unwrap();
        assert!((r["min_radius"].as_f64().unwrap() - 8.0).abs() < 0.4, "{r}");
        let r = s.execute("inspect.minimum_radius", &json!({})).unwrap();
        assert!((r["min_radius"].as_f64().unwrap() - 8.0).abs() < 0.4, "{r}");
        // Centre of mass: a box beside the cylinder.
        s.execute("solid.box", &json!({"corner": [0, 0, 0], "length": 10, "width": 10, "height": 10})).unwrap();
        let r = s.execute("inspect.center_of_mass", &json!({})).unwrap();
        let (vb, vc) = (1000.0, std::f64::consts::PI * 640.0);
        let want = (5.0 * vb + 50.0 * vc) / (vb + vc);
        assert!((r["center"][0].as_f64().unwrap() - want).abs() < 0.1, "{r}");
        assert!((r["center"][2].as_f64().unwrap() - 5.0).abs() < 0.05, "{r}");
        assert!(s.execute("inspect.center_of_mass", &json!({"bodies": ["nope"]})).is_err());
        // Isocurves on the cylinder's side: rings around it and lines along it.
        let r = s.execute("inspect.isocurve", &json!({"faces": [[58, 0, 5]], "count": 4})).unwrap();
        let curves = r["curves"].as_array().unwrap();
        let ring =
            |c: &Value| c.as_array().unwrap().iter().all(|q| ((q[0].as_f64().unwrap() - 50.0).hypot(q[1].as_f64().unwrap()) - 8.0).abs() < 0.2);
        assert!(curves.len() >= 8, "{}", curves.len());
        assert!(curves.iter().all(ring), "every curve lies on the cylinder");
        let flat = |c: &Value| {
            let z: Vec<f64> = c.as_array().unwrap().iter().map(|q| q[2].as_f64().unwrap()).collect();
            z.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - z.iter().cloned().fold(f64::INFINITY, f64::min) < 1e-6
        };
        assert!(curves.iter().filter(|c| flat(c)).count() >= 4, "rings at constant height");
        assert!(s.execute("inspect.isocurve", &json!({"faces": []})).is_err());
    }
}
