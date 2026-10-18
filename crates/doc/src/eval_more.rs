//! Evaluation of Emboss, Rib/Web, Replace Face, Align and Remove (features built from the
//! kernel's extrude, boolean, offset and transform operations).

use super::*;
use crate::EmbossFace;
use crate::apply_point;
use crate::sheet;
use solidcraft_geom::{Aabb3, Loop2, Seg2};

pub(super) fn eval(doc: &Document, vals: &BTreeMap<String, Value>, f: &Feature, st: &mut ModelState) -> Result<()> {
    match &f.kind {
        FeatureKind::Emboss { sketch, profiles, depth, deboss, targets, face } => {
            let ss = st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch}")))?;
            let regions = select_profiles(ss, profiles)?;
            let d = val(vals, depth, Kind::Length)?;
            if !(d > 0.0) {
                return Err(DocError::Invalid("the emboss depth must be positive".into()));
            }
            if let Some(face) = face {
                if !targets.is_empty() && (targets.len() != 1 || targets.first() != Some(&face.body)) {
                    return Err(DocError::Invalid("cylinder emboss must keep its selected target body".into()));
                }
                let i =
                    st.bodies.iter().position(|b| b.name == face.body).ok_or_else(|| DocError::Unknown(format!("emboss body `{}`", face.body)))?;
                let mb = st.bodies.get(i).cloned().ok_or_else(|| DocError::Invalid("emboss body".into()))?;
                let body_frame = face.body_frame(doc, &mb)?;
                let inverse = crate::mat_inverse(&body_frame).ok_or_else(|| DocError::Invalid("invalid cylinder body frame".into()))?;
                let sketch_owner = doc.feature(ss.feature).map(|f| f.component).unwrap_or(0);
                let author_frame = EmbossFace::component_frame(doc, f.component, &face.author_path)?;
                let sketch_frame = face.sketch_frame(doc, f.component, sketch_owner, &mb)?;
                let sketch_transform = crate::mat_mul(&inverse, &sketch_frame);
                let plane = Plane::new(
                    crate::apply_point(&sketch_transform, ss.plane.origin),
                    crate::apply_vector(&sketch_transform, ss.plane.x),
                    crate::apply_vector(&sketch_transform, ss.plane.y),
                )
                .ok_or_else(|| DocError::Invalid("invalid cylinder sketch frame".into()))?;
                let mut at = crate::apply_point(&crate::mat_mul(&inverse, &author_frame), face.at.0);
                let name = f
                    .face_names
                    .first()
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| DocError::Invalid("selected cylinder face has no persistent identity".into()))?;
                {
                    let mut scoped = ModelState::default();
                    scoped.bodies.push(mb.clone());
                    let (picked, split) = crate::naming::point_on_face(&scoped, name, at)
                        .ok_or_else(|| DocError::Invalid("selected cylinder face no longer exists".into()))?;
                    if split {
                        return Err(DocError::Invalid("selected cylinder face was split".into()));
                    }
                    let (index, _, _) = crate::project::nearest_face(&mb.mesh(), picked)
                        .ok_or_else(|| DocError::Invalid("selected cylinder face cannot be resolved".into()))?;
                    if !crate::naming::face_has_name(&mb, index as usize, name) {
                        return Err(DocError::Invalid("selected cylinder face is ambiguous".into()));
                    }
                    at = kernel::cylinder_emboss_anchor(&mb.body, index as usize, picked)?;
                }
                let result = kernel::emboss_cylinder_at(&mb.body, at, &plane, &regions, if *deboss { -d } else { d })?;
                if let Some(slot) = st.bodies.get_mut(i) {
                    *slot = ModelBody::new(mb.name, result, mb.feature);
                }
                return Ok(());
            }
            let (a, b) = if *deboss { (-d, 0.0) } else { (0.0, d) };
            let tools = kernel::extrude(&ss.plane, &regions, a, b)?;
            if st.bodies.is_empty() {
                return Err(DocError::Invalid("there is no body to emboss".into()));
            }
            apply_op(st, f, tools, if *deboss { Operation::Cut } else { Operation::Join }, &crate::plain(targets))
        }
        FeatureKind::Rib { sketch, curves, thickness, depth, flip, web, normal } => {
            let ss = st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch}")))?.clone();
            let t = val(vals, thickness, Kind::Length)?;
            if !(t > 0.0) {
                return Err(DocError::Invalid("the rib thickness must be positive".into()));
            }
            let depth = match depth {
                Some(e) => Some(val(vals, e, Kind::Length)?)
                    .filter(|d| *d > 0.0)
                    .ok_or_else(|| DocError::Invalid("the rib depth must be positive".into()))
                    .map(Some)?,
                None => None,
            };
            let groups: Vec<Vec<String>> = if *web { curves.iter().map(|c| vec![c.clone()]).collect() } else { vec![curves.clone()] };
            if groups.is_empty() || curves.is_empty() || curves.len() > 1000 {
                return Err(DocError::Invalid("a rib needs 1…1000 sketch curves".into()));
            }
            let mut tools = Vec::new();
            for g in &groups {
                tools.push(if *normal { rib_tool_normal(st, &ss, g, t, depth, *flip)? } else { rib_tool(st, &ss, g, t, depth, *flip)? });
            }
            apply_op(st, f, tools, Operation::Join, &[])
        }
        FeatureKind::ReplaceFace { faces, target, body } => {
            let tp = doc.resolve_plane_in(vals, target, 0, Some(st))?;
            let tn = tp.normal();
            let i = body_at(st, body, &crate::plain(faces))?;
            let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("body".into())) };
            // Each face moved is a step of the body's lineage, as Offset Face's is: without it no face of
            // the result could take a sketch's attachment.
            let mut b = mb.traced();
            for p in faces {
                let n = planar_normal_at(&b.body, p.0)
                    .ok_or_else(|| DocError::Invalid("not supported yet: replacing a face that is not planar".into()))?;
                if n.dot(tn).abs() < 1.0 - 1e-9 {
                    return Err(DocError::Invalid("not supported yet: a target that is not parallel to the face".into()));
                }
                let d = (tp.origin - p.0).dot(n);
                if d.abs() > 1e-9 {
                    b = Traced::step(kernel::offset_faces(&b.body, &[p.0], d)?, &[&b]);
                }
            }
            if let Some(slot) = st.bodies.get_mut(i) {
                *slot = ModelBody::traced_new(mb.name, b, mb.feature);
            }
            Ok(())
        }
        FeatureKind::Align { bodies, from, to, from_normal, to_normal, flip } => {
            let mut m = translation(Vec3::ZERO);
            if let (Some(a), Some(b)) = (from_normal.and_then(|n| n.normalized()), to_normal.and_then(|n| n.normalized())) {
                // Turn `a` onto −b (faces meet) or b (flipped), about the `from` point.
                let want = if *flip { b } else { b * -1.0 };
                let axis = a.cross(want);
                let ang = axis.len().atan2(a.dot(want));
                let axis = axis.normalized().unwrap_or_else(|| a.any_perp());
                if ang.abs() > 1e-12 {
                    m = rotation(from.0, axis, ang);
                }
            }
            let d = to.0 - from.0;
            m[3][0] += d.x;
            m[3][1] += d.y;
            m[3][2] += d.z;
            if bodies.is_empty() {
                return Err(DocError::Invalid("no bodies to align".into()));
            }
            for n in bodies {
                let i = st.bodies.iter().position(|b| &b.name == n).ok_or_else(|| DocError::Unknown(format!("body `{n}`")))?;
                if let Some(mb) = st.bodies.get_mut(i) {
                    let nb = kernel::transform_matrix(&mb.body, m)?;
                    *mb = ModelBody::new(mb.name.clone(), nb, mb.feature);
                }
            }
            Ok(())
        }
        FeatureKind::Remove { bodies } => {
            if bodies.is_empty() {
                return Err(DocError::Invalid("no bodies to remove".into()));
            }
            for n in bodies {
                if st.body(n).is_none() {
                    return Err(DocError::Unknown(format!("body `{n}`")));
                }
            }
            st.bodies.retain(|b| !bodies.iter().any(|t| *t == b.name));
            Ok(())
        }
        _ => Err(DocError::Invalid(format!("{} is not evaluated here", f.name))),
    }
}

/// Unit outward normal of the planar face through `p`: of the planar faces whose plane runs
/// through it, the one nearest it. A face elsewhere in such a plane (the wall of a notch, the end
/// of a rib, in line with a rim) is no pick of `p`, though its plane runs through it as exactly.
fn planar_normal_at(b: &Body, p: Vec3) -> Option<Vec3> {
    let tol = (b.size() * 2e-3).max(1e-3);
    let faces = b.faces(tol).ok()?;
    let mesh = b.tessellate(tol).ok()?;
    let mut near = vec![f64::INFINITY; faces.len()];
    for (t, f) in mesh.triangles.iter().zip(&mesh.tri_face) {
        if let (Some([u, v, w]), Some(d)) = (mesh.tri(t), near.get_mut(*f as usize)) {
            *d = d.min(crate::appearance::closest_on_triangle(p, u, v, w).dist(p));
        }
    }
    faces
        .iter()
        .filter_map(|f| f.plane_normal.map(|n| (n, ((p - f.centroid).dot(n)).abs(), near.get(f.index).copied().unwrap_or(f64::INFINITY))))
        .filter(|(_, d, _)| *d < tol * 5.0)
        .min_by(|a, b| a.2.total_cmp(&b.2).then(a.1.total_cmp(&b.1)))
        .map(|(n, _, _)| n)
}

/// The curves of a chain as one polyline in sketch coordinates, in order.
fn chain_points(ss: &SolvedSketch, ids: &[String]) -> Result<Vec<Vec2>> {
    let mut segs: Vec<Seg2> = Vec::new();
    for id in ids {
        let ci = ss.sketch.curve_index(id).ok_or_else(|| DocError::Unknown(format!("curve `{id}`")))?;
        segs.extend(ss.sketch.segs(ci));
    }
    let Some(first) = segs.first().copied() else { return Err(DocError::Invalid("no curves".into())) };
    // Orient the first segment toward the second, then chain.
    let mut chain = vec![first];
    if let Some(n) = segs.get(1)
        && (first.start().dist(n.start()) < 1e-6 || first.start().dist(n.end()) < 1e-6)
    {
        chain[0] = first.reversed();
    }
    for s in segs.iter().skip(1) {
        let prev = chain.last().map(Seg2::end).unwrap_or_default();
        let s = if s.end().dist(prev) < s.start().dist(prev) { s.reversed() } else { *s };
        if s.start().dist(prev) > 1e-5 {
            return Err(DocError::Invalid("the rib curves are not connected end to end".into()));
        }
        chain.push(s);
    }
    let mut pts = vec![chain[0].start()];
    for s in &chain {
        let n = if matches!(s, Seg2::Line { .. }) { 1 } else { 24 };
        for k in 1..=n {
            pts.push(s.point_at(k as f64 / n as f64));
        }
    }
    Ok(pts)
}

/// A rib wall: the band swept from the curves across the plane by `depth` (or to the body),
/// `t` thick about the sketch plane.
fn rib_tool(st: &ModelState, ss: &SolvedSketch, ids: &[String], t: f64, depth: Option<f64>, flip: bool) -> Result<Body> {
    let pts = chain_points(ss, ids)?;
    let (Some(a), Some(b)) = (pts.first().copied(), pts.last().copied()) else { return Err(DocError::Invalid("empty rib curve".into())) };
    let chord = (b - a).normalized().ok_or_else(|| DocError::Invalid("a closed or zero-length rib curve".into()))?;
    let mid = pts.get(pts.len() / 2).copied().unwrap_or(a);
    // Toward the material: the side of the curve where the bodies are (unless flipped).
    let mut n = chord.perp();
    let reach = st.bodies.iter().map(|b| b.mesh().bounds()).fold(0.0f64, |acc, bb| acc.max((bb.max - bb.min).len()));
    let probe = |side: Vec2| {
        let mut hits = 0;
        for k in 1..=20 {
            let q = ss.plane.to_world(mid + side * (reach * k as f64 / 20.0));
            if st.bodies.iter().any(|b| b.mesh().contains(q)) {
                hits += 1;
            }
        }
        hits
    };
    if probe(n * -1.0) > probe(n) {
        n = n * -1.0;
    }
    if flip {
        n = n * -1.0;
    }
    // From each point of the curves, walk across the plane until the body begins (or `depth`);
    // walls that reach the body go a little into it so they merge with it.
    let inside = |q: Vec2| {
        let w = ss.plane.to_world(q);
        st.bodies.iter().any(|b| b.mesh().contains(w))
    };
    let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    let samples = ((len / (t.min(len) * 0.25).max(len / 400.0)).ceil() as usize).clamp(2, 400);
    let along: Vec<Vec2> = (0..=samples).map(|k| point_on_polyline(&pts, len * k as f64 / samples as f64)).collect();
    let limit = depth.unwrap_or(reach * 2.0 + 1.0);
    let step = (limit.min(reach * 2.0 + 1.0) / 400.0).max(1e-3);
    let overlap = (len / samples as f64 * 2.0).max(step * 2.0).min(t);
    let mut far = Vec::with_capacity(along.len());
    for q in &along {
        let mut hit = None;
        let mut k = 1;
        while (k as f64) * step <= limit + step && k < 100_000 {
            if inside(*q + n * (k as f64 * step)) {
                // Refine between the last outside and the first inside sample.
                let (mut lo, mut hi) = ((k - 1) as f64 * step, k as f64 * step);
                for _ in 0..30 {
                    let m = 0.5 * (lo + hi);
                    if inside(*q + n * m) { hi = m } else { lo = m }
                }
                hit = Some(hi);
                break;
            }
            k += 1;
        }
        let h = match (hit, depth) {
            (Some(h), Some(d)) if h >= d => d,
            (Some(h), _) => h + overlap,
            (None, Some(d)) => d,
            (None, None) => return Err(DocError::Invalid("the rib curves do not face a body (give a depth)".into())),
        };
        far.push(*q + n * h);
    }
    let mut poly = along.clone();
    poly.extend(far.into_iter().rev());
    let mut lp = Loop2::polygon(&poly);
    if lp.signed_area() < 0.0 {
        lp = lp.reversed();
    }
    let region = Region2 { outer: lp, holes: Vec::new() };
    let tools = kernel::extrude(&ss.plane, &[region], -t / 2.0, t / 2.0)?;
    tools.into_iter().next().ok_or_else(|| DocError::Invalid("the rib made no solid".into()))
}

/// A rib wall along the sketch normal: the curves widened to `t` in the sketch plane, run from
/// the sketch toward the body by `depth` (or until the whole wall reaches the body).
fn rib_tool_normal(st: &ModelState, ss: &SolvedSketch, ids: &[String], t: f64, depth: Option<f64>, flip: bool) -> Result<Body> {
    let pts = chain_points(ss, ids)?;
    let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    if !(len > 1e-9) {
        return Err(DocError::Invalid("a zero-length rib curve".into()));
    }
    let samples = ((len / (t * 0.5).max(len / 400.0)).ceil() as usize).clamp(2, 400);
    let along: Vec<Vec3> = (0..=samples).map(|k| ss.plane.to_world(point_on_polyline(&pts, len * k as f64 / samples as f64))).collect();
    let normal = ss.plane.normal();
    let reach = st.bodies.iter().map(|b| b.mesh().bounds()).fold(0.0f64, |acc, bb| acc.max((bb.max - bb.min).len()));
    let inside = |q: Vec3| st.bodies.iter().any(|b| b.mesh().contains(q));
    // Where the body begins from each point of the curves, along `sign` × the sketch normal.
    let limit = reach * 2.0 + 1.0;
    let step = (limit / 400.0).max(1e-3);
    let hits = |sign: f64| -> Vec<f64> {
        let mut out = Vec::new();
        for q in &along {
            let Some(k) = (1..=400).find(|k| inside(*q + normal * (sign * *k as f64 * step))) else { continue };
            let (mut lo, mut hi) = ((k - 1) as f64 * step, k as f64 * step);
            for _ in 0..30 {
                let m = 0.5 * (lo + hi);
                if inside(*q + normal * (sign * m)) { hi = m } else { lo = m }
            }
            out.push(hi);
        }
        out
    };
    // Toward the material: the side of the sketch where the curves meet the bodies (unless flipped).
    let (front, back) = (hits(1.0), hits(-1.0));
    let mut sign = if back.len() > front.len() { -1.0 } else { 1.0 };
    let mut reached = if sign > 0.0 { front } else { back };
    if flip {
        sign = -sign;
        reached = hits(sign);
    }
    let h = match depth {
        Some(d) => d,
        None => {
            let far = reached.iter().copied().fold(f64::NAN, f64::max);
            if !far.is_finite() {
                return Err(DocError::Invalid("the rib curves do not face a body along the sketch normal (give a depth)".into()));
            }
            far + (t * 0.5).max(step)
        }
    };
    // The curves widened to `t`: each point offset by half the thickness along the miter of its
    // neighbours, both sides joined into one band.
    let half = t / 2.0;
    let mut left = Vec::with_capacity(pts.len());
    let mut right = Vec::with_capacity(pts.len());
    for (i, p) in pts.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|j| pts.get(j)).and_then(|q| (*p - *q).normalized());
        let next = pts.get(i + 1).and_then(|q| (*q - *p).normalized());
        let (n, scale) = match (prev, next) {
            (Some(a), Some(b)) => {
                let m = (a.perp() + b.perp()).normalized().unwrap_or(a.perp());
                (m, 1.0 / m.dot(a.perp()).max(0.25))
            }
            (Some(a), None) | (None, Some(a)) => (a.perp(), 1.0),
            (None, None) => continue,
        };
        left.push(*p + n * (half * scale));
        right.push(*p - n * (half * scale));
    }
    left.extend(right.into_iter().rev());
    let mut lp = Loop2::polygon(&left);
    if lp.signed_area() < 0.0 {
        lp = lp.reversed();
    }
    let region = Region2 { outer: lp, holes: Vec::new() };
    let (a, b) = if sign > 0.0 { (0.0, h) } else { (-h, 0.0) };
    let tools = kernel::extrude(&ss.plane, &[region], a, b)?;
    tools.into_iter().next().ok_or_else(|| DocError::Invalid("the rib made no solid".into()))
}

/// The point `d` along a polyline.
fn point_on_polyline(pts: &[Vec2], d: f64) -> Vec2 {
    let mut left = d.max(0.0);
    for w in pts.windows(2) {
        let l = w[0].dist(w[1]);
        if left <= l && l > 0.0 {
            return w[0].lerp(w[1], left / l);
        }
        left -= l;
    }
    pts.last().copied().unwrap_or_default()
}

/// Rebuild a sheet body's solid from its model.
fn replace_sheet_body(mb: &mut ModelBody, body: Body) {
    let mut next = ModelBody::new(mb.name.clone(), body, mb.feature);
    next.component = mb.component;
    next.transferred = mb.transferred;
    next.source_name = mb.source_name.clone();
    next.source_transform = mb.source_transform;
    next.imported = mb.imported.clone();
    *mb = next;
}

/// Put a converted sheet in the place of the body it was made from.
fn put_sheet(st: &mut ModelState, sh: sheet::SheetBody) -> Result<()> {
    let solid = sh.solid()?;
    if let Some(b) = st.bodies.iter_mut().find(|b| b.name == sh.body) {
        replace_sheet_body(b, solid);
    }
    st.sheets.retain(|s| s.body != sh.body);
    st.sheets.push(sh);
    Ok(())
}

fn rebuild_sheet(st: &mut ModelState, idx: usize) -> Result<()> {
    let sheet = st.sheets.get(idx).cloned().ok_or_else(|| DocError::Invalid("sheet".into()))?;
    let solid = sheet.solid()?;
    let i = st.bodies.iter().position(|b| b.name == sheet.body).ok_or_else(|| DocError::Unknown(format!("body `{}`", sheet.body)))?;
    if let Some(mb) = st.bodies.get_mut(i) {
        replace_sheet_body(mb, solid);
    }
    Ok(())
}

/// The sheet a feature works on: by body name, else the one with an edge nearest the pick.
fn sheet_for(st: &ModelState, body: &Option<crate::BodyName>, pick: Option<Vec3>) -> Result<usize> {
    if let Some(n) = body {
        return st.sheets.iter().position(|s| &s.body == n).ok_or_else(|| DocError::Invalid(format!("`{n}` is not a sheet metal body")));
    }
    let p = pick.ok_or_else(|| DocError::Invalid("no sheet metal body".into()))?;
    st.sheets
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.edge_at(p).map(|e| (i, e.4)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
        .ok_or_else(|| DocError::Invalid("there is no sheet metal body".into()))
}

/// Whether a point touches sheet `i`'s material: inside it, on one of its faces, or within its
/// display tolerance of one (picks land on the display mesh). A sheet with no mesh to test
/// against counts as touched.
fn on_sheet_material(st: &ModelState, i: usize, p: Vec3) -> bool {
    let Some(mb) = st.sheets.get(i).and_then(|sh| st.body(&sh.body)) else { return true };
    let mesh = mb.mesh();
    if mesh.triangles.is_empty() {
        return true;
    }
    // Twice the display mesh's chord tolerance (see `ModelBody::mesh`), and a quarter of the
    // thickness: on a large thin sheet the first steps from a point on its face clean through
    // it, so a point on the face would touch nothing (#685).
    let step = 2.0 * (mb.body.size() * 1e-3).max(1e-3);
    let thin = st.sheets.get(i).map_or(step, |sh| (sh.t * 0.25).min(step));
    let offsets = [-step, -thin, 0.0, thin, step];
    offsets.iter().any(|&dx| offsets.iter().any(|&dy| offsets.iter().any(|&dz| mesh.contains(p + Vec3::new(dx, dy, dz)))))
}

/// A region mirrored in the sketch x axis (y → −y), exactly: free-form curves through their
/// control points.
fn mirror_y(r: &Region2) -> Region2 {
    let f = |p: Vec2| Vec2::new(p.x, -p.y);
    let m = |l: &Loop2| {
        let segs: Vec<Seg2> = l
            .segs
            .iter()
            .map(|s| match *s {
                Seg2::Arc { center, radius, start, sweep } => Seg2::Arc { center: f(center), radius, start: -start, sweep: -sweep },
                other => sheet::map_controls(&other, f),
            })
            .collect();
        Loop2 { segs }.ccw()
    };
    Region2 { outer: m(&r.outer), holes: r.holes.iter().map(|h| m(h).reversed()).collect() }
}

pub(super) fn sheet_eval(
    doc: &Document,
    vals: &BTreeMap<String, Value>,
    f: &Feature,
    st: &mut ModelState,
    warning: &mut Option<String>,
) -> Result<()> {
    use crate::sheet;
    match &f.kind {
        FeatureKind::SheetBase { sketch, profiles, rule, flip } => {
            let ss = st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch}")))?.clone();
            let regions = solidcraft_sketch::merge_regions(&select_profiles(&ss, profiles)?);
            let r = doc.sheet_rule(rule.as_deref());
            let rv = doc.rule_values(vals, &r)?;
            for (k, region) in regions.into_iter().enumerate() {
                let (plane, region) = if *flip {
                    (Plane { origin: ss.plane.origin, x: ss.plane.x, y: ss.plane.y * -1.0 }, mirror_y(&region))
                } else {
                    (ss.plane, region)
                };
                let name = new_name(st, f, k);
                let mut sh = sheet::base_flange(&name, &r.name, &rv, plane, region);
                // The sketch's lines name the base panel's edges.
                sh.base_curves = ss
                    .sketch
                    .curves
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| !c.construction)
                    .flat_map(|(ci, c)| {
                        ss.sketch.segs(ci).into_iter().filter_map(move |sg| match sg {
                            Seg2::Line { a, b } => Some((c.id.clone(), a, b)),
                            _ => None,
                        })
                    })
                    .map(|(id, a, b)| if *flip { (id, Vec2::new(a.x, -a.y), Vec2::new(b.x, -b.y)) } else { (id, a, b) })
                    .collect();
                let solid = sh.solid()?;
                st.bodies.push(ModelBody::new(name, solid, f.id));
                st.sheets.push(sh);
            }
            Ok(())
        }
        FeatureKind::SheetContour { sketch, curves, distance, rule, flip, reverse } => {
            let ss = st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch}")))?.clone();
            for id in curves {
                let ci = ss.sketch.curve_index(id).ok_or_else(|| DocError::Unknown(format!("curve `{id}`")))?;
                if !ss.sketch.segs(ci).iter().all(|s| matches!(s, Seg2::Line { .. })) {
                    return Err(DocError::Invalid("not supported yet: arcs in a contour flange (use lines; the bends are made for you)".into()));
                }
            }
            let pts = chain_points(&ss, curves)?;
            let w = val(vals, distance, Kind::Length)?;
            if !(w > 0.0) {
                return Err(DocError::Invalid("the contour flange distance must be positive".into()));
            }
            let r = doc.sheet_rule(rule.as_deref());
            let rv = doc.rule_values(vals, &r)?;
            let plane = if *reverse { Plane { origin: ss.plane.origin, x: ss.plane.x, y: ss.plane.y * -1.0 } } else { ss.plane };
            let pts: Vec<Vec2> = if *reverse { pts.iter().map(|p| Vec2::new(p.x, -p.y)).collect() } else { pts };
            // Mirroring the sketch swaps the material side too.
            let name = new_name(st, f, 0);
            let sh = sheet::contour_flange(&name, &r.name, &rv, &plane, &pts, w, *flip != *reverse)?;
            let solid = sh.solid()?;
            st.bodies.push(ModelBody::new(name, solid, f.id));
            st.sheets.push(sh);
            Ok(())
        }
        FeatureKind::SheetFlange { edges, height, angle, radius, position, flip, body, corner_relief_shape, corner_relief } => {
            let i = sheet_for(st, body, edges.first().map(|e| e.point.0))?;
            let (h, a) = (val(vals, height, Kind::Length)?, val(vals, angle, Kind::Angle)?);
            // Each edge's own height and angle, else the feature's.
            let mut own = Vec::with_capacity(edges.len());
            for e in edges {
                let height = match &e.height {
                    Some(x) => val(vals, x, Kind::Length)?,
                    None => h,
                };
                let angle = match &e.angle {
                    Some(x) => val(vals, x, Kind::Angle)?,
                    None => a,
                };
                own.push((height, angle));
            }
            let rr = match radius {
                Some(e) => val(vals, e, Kind::Length)?,
                None => {
                    let rule = st.sheets.get(i).map(|s| s.rule.clone()).unwrap_or_default();
                    doc.rule_values(vals, &doc.sheet_rule(Some(&rule)))?.bend_radius
                }
            };
            // The corner relief these flanges ask for in place of the rule's.
            let relief = sheet::ReliefOverride {
                shape: match corner_relief_shape {
                    Some(name) => Some(
                        sheet::ReliefShape::parse(name)
                            .ok_or_else(|| DocError::Invalid(format!("unknown corner relief shape `{name}` (notch, round, rectangular or tear)")))?,
                    ),
                    None => None,
                },
                size: match corner_relief {
                    Some(e) => Some(val(vals, e, Kind::Length)?),
                    None => None,
                },
            };
            let sh = st.sheets.get_mut(i).ok_or_else(|| DocError::Invalid("sheet".into()))?;
            if sh.flat {
                return Err(DocError::Invalid("refold the sheet before adding flanges".into()));
            }
            let points: Vec<Vec3> = edges.iter().map(|e| e.point.0).collect();
            let each: Vec<sheet::EdgeFlange> = sheet_picks(sh, f, &points, warning)
                .into_iter()
                .zip(own)
                .map(|(pick, (height, angle))| sheet::EdgeFlange { pick, height, angle })
                .collect();
            let n0 = sh.flanges.len();
            let (_, notes) = sh.add_flanges_each(&each, rr, position, *flip, relief)?;
            if !notes.is_empty() {
                let notes = notes.join("; ");
                *warning = Some(match warning.take() {
                    Some(w) => format!("{w}; {notes}"),
                    None => notes,
                });
            }
            tag_flanges(sh, n0, f.id);
            rebuild_sheet(st, i)
        }
        FeatureKind::SheetPerforate { diameter, pitch, margin, pattern, body } => {
            // Without a body name the field goes on the newest sheet, as the command's own body lookup does.
            let i = match body {
                Some(_) => sheet_for(st, body, None)?,
                None => st.sheets.len().checked_sub(1).ok_or_else(|| DocError::Invalid("there is no sheet metal body".into()))?,
            };
            let d = val(vals, diameter, Kind::Length)?;
            let pitch = val(vals, pitch, Kind::Length)?;
            let margin = match margin {
                Some(e) => val(vals, e, Kind::Length)?,
                None => d,
            };
            let square = match pattern.as_deref() {
                None | Some("staggered") => false,
                Some("square") => true,
                Some(o) => return Err(DocError::Invalid(format!("perforation pattern {o:?}: use \"staggered\" or \"square\""))),
            };
            if !(d > 0.0 && pitch > d && margin >= 0.0) {
                return Err(DocError::Invalid(format!(
                    "perforation needs diameter > 0, pitch > diameter and margin >= 0 (diameter {d}, pitch {pitch}, margin {margin})"
                )));
            }
            let sh = st.sheets.get_mut(i).ok_or_else(|| DocError::Invalid("sheet".into()))?;
            sh.perforations.push(crate::sheet::SheetPerforation { feature: f.id, diameter: d, pitch, margin, square });
            Ok(())
        }
        FeatureKind::SheetHem { edges, length, gap, flip, body } => {
            let i = sheet_for(st, body, edges.first().copied().map(Vec3::from))?;
            let l = val(vals, length, Kind::Length)?;
            let g = match gap {
                Some(e) => val(vals, e, Kind::Length)?,
                None => {
                    let rule = st.sheets.get(i).map(|s| s.rule.clone()).unwrap_or_default();
                    doc.rule_values(vals, &doc.sheet_rule(Some(&rule)))?.hem_gap
                }
            };
            let sh = st.sheets.get_mut(i).ok_or_else(|| DocError::Invalid("sheet".into()))?;
            let picks = sheet_picks(sh, f, &crate::plain(edges), warning);
            let n0 = sh.flanges.len();
            for e in &picks {
                sh.add_hem(*e, l, g, *flip)?;
            }
            tag_flanges(sh, n0, f.id);
            rebuild_sheet(st, i)
        }
        FeatureKind::SheetFold { sketch, curve, a, b, angle, radius, position, flip, fixed, body } => {
            // The sketch line, where it is now.
            let (a, b) = match (sketch, curve) {
                (Some(sk), Some(c)) => {
                    let ss = st.sketch(sk.0).ok_or_else(|| DocError::Unknown(format!("sketch {sk}")))?;
                    let ci = ss.sketch.curve_index(c).ok_or_else(|| DocError::Unknown(format!("curve `{c}`")))?;
                    match ss.sketch.segs(ci).as_slice() {
                        [Seg2::Line { a, b }] => (ss.plane.to_world(*a), ss.plane.to_world(*b)),
                        _ => return Err(DocError::Invalid("the fold line must be a sketch line".into())),
                    }
                }
                _ => (a.0, b.0),
            };
            let i = sheet_for(st, body, Some((a + b) * 0.5))?;
            let ang = val(vals, angle, Kind::Angle)?;
            let rr = match radius {
                Some(e) => val(vals, e, Kind::Length)?,
                None => {
                    let rule = st.sheets.get(i).map(|s| s.rule.clone()).unwrap_or_default();
                    doc.rule_values(vals, &doc.sheet_rule(Some(&rule)))?.bend_radius
                }
            };
            // A fixed point that touches no material at all (e.g. far above the sheet) still picks
            // the side it lies on, so a stored fold keeps recomputing; it says so (#371).
            if let Some(p) = fixed
                && !on_sheet_material(st, i, p.0)
            {
                *warning = Some(format!(
                    "{} at ({:.3}, {:.3}, {:.3}); the side of the fold line it lies on stays",
                    sheet::FOLD_FIXED_OFF_SHEET,
                    p.x,
                    p.y,
                    p.z
                ));
            }
            let sh = st.sheets.get_mut(i).ok_or_else(|| DocError::Invalid("sheet".into()))?;
            if sh.flat {
                return Err(DocError::Invalid("refold the sheet before folding it".into()));
            }
            let n0 = sh.flanges.len();
            sh.add_fold(a, b, ang, rr, position, *flip, fixed.map(Vec3::from), warning)?;
            tag_flanges(sh, n0, f.id);
            rebuild_sheet(st, i)
        }
        FeatureKind::SheetUnfold { body, refold } => {
            let i = match body {
                Some(_) => sheet_for(st, body, None)?,
                None => st.sheets.iter().rposition(|s| s.flat == *refold).ok_or_else(|| {
                    DocError::Invalid(if *refold { "no unfolded sheet to refold".into() } else { "no sheet metal body to unfold".into() })
                })?,
            };
            if let Some(sh) = st.sheets.get_mut(i) {
                sh.flat = !*refold;
            }
            rebuild_sheet(st, i)
        }
        // With bend edges: a solid or shelled body whose faces become the sheet.
        FeatureKind::SheetConvert { body, face, rule, bends } if !bends.is_empty() => {
            let mb = st.body(body).cloned().ok_or_else(|| DocError::Unknown(format!("body `{body}`")))?;
            let r = doc.sheet_rule(rule.as_deref());
            let rv = doc.rule_values(vals, &r)?;
            // The bend edges by name first; a name that is gone keeps its point, with a warning.
            let mut picks = Vec::with_capacity(bends.len());
            for (i, p) in bends.iter().enumerate() {
                let name = f.edge_names.get(i).filter(|n| !n.is_empty());
                picks.push(match name.map(|n| (n, crate::naming::locate_edge(&mb, n, p.0))) {
                    Some((n, Ok(hit))) => {
                        if let Some(note) = hit.note(n) {
                            append_warning(warning, note);
                        }
                        hit.point
                    }
                    Some((n, Err(_))) => {
                        append_warning(warning, format!("edge `{n}` no longer exists; using the edge at the picked point"));
                        p.0
                    }
                    None => p.0,
                });
            }
            let (mut sh, notes) = sheet::convert_body(&mb, &r.name, &rv, face.0, &picks)?;
            for note in notes {
                append_warning(warning, note);
            }
            tag_flanges(&mut sh, 0, f.id);
            put_sheet(st, sh)
        }
        FeatureKind::SheetConvert { body, face, rule, .. } => {
            let mb = st.body(body).cloned().ok_or_else(|| DocError::Unknown(format!("body `{body}`")))?;
            let n = planar_normal_at(&mb.body, face.0).ok_or_else(|| DocError::Invalid("pick a planar face of the plate".into()))?;
            // Thickness: the body's extent along the face normal.
            let extent = |mesh: &solidcraft_geom::Mesh| {
                mesh.positions.iter().map(|p| (*p - face.0).dot(n)).fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)))
            };
            // The tolerance (the mesh's chords, and how far off the face's plane a point of it may
            // be) is at most a tenth of the thickness, not only a thousandth of the size. On a long
            // thin plate that thousandth passes the thickness: the far side's triangles counted as
            // this face's (no outline was left), and a hole of a smaller radius than that fell out.
            let mut tol = (mb.body.size() * 1e-3).max(1e-3);
            let mut mesh = mb.body.tessellate(tol)?;
            let (mut lo, mut hi) = extent(&mesh);
            if hi - lo > 0.0 && tol > (hi - lo) * 0.1 {
                tol = ((hi - lo) * 0.1).max(1e-6);
                mesh = mb.body.tessellate(tol)?;
                (lo, hi) = extent(&mesh);
            }
            let t = hi - lo;
            // The face outline: boundary edges of the triangles on that face's plane, where it runs
            // along a round wall an arc of the wall's circle (not chords: the plate check below
            // compares its area with the exact volume).
            let plane = Plane::from_normal(face.0 + n * lo, n * -1.0).ok_or_else(|| DocError::Invalid("face".into()))?;
            // Recesses in one face (an engraved label) become pockets; the outline is the other face's.
            let recesses = plate_recesses(&mb.body, &mesh, &plane, face.0, n, (lo, hi), tol)?;
            let mut walls = std::collections::HashMap::new();
            let mut wall = |f: u32| *walls.entry(f).or_insert_with(|| sheet::round_wall(&mb.body, &plane, n, f));
            let mut loops: Vec<Loop2> =
                face_outline(&mesh, &plane, face.0, n, recesses.level, tol).iter().map(|l| sheet::outline_loop(l, &mut wall, tol * 1e-2)).collect();
            loops.sort_by(|a, b| b.signed_area().abs().total_cmp(&a.signed_area().abs()));
            let mut it = loops.into_iter();
            let outer = it.next().ok_or_else(|| DocError::Invalid("no outline on that face".into()))?.ccw();
            let region = Region2 { outer, holes: it.map(|l| l.ccw().reversed()).collect() };
            let area = region.area().abs();
            let vol = kernel::measure(&mb.body)?.volume + recesses.volume;
            if (area * t - vol).abs() > 1e-3 * vol.max(1.0) {
                return Err(DocError::Invalid(
                    "not supported yet: converting bodies that are not a plate of even thickness without bend edges (`bends`)".into(),
                ));
            }
            let r = doc.sheet_rule(rule.as_deref());
            let mut rv = doc.rule_values(vals, &r)?;
            rv.thickness = t;
            let mut sh = sheet::base_flange(body, &r.name, &rv, plane, Region2 { outer: region.outer.clone(), holes: Vec::new() });
            sh.holes = region.holes.iter().map(|h| Region2 { outer: h.ccw(), holes: Vec::new() }).collect();
            let mut work = sheet::FlatWork::new();
            for (pocket, z0, z1) in recesses.pockets {
                sh.record_pocket(pocket, z0, z1, &mut work)?;
            }
            put_sheet(st, sh)
        }
        _ => Err(DocError::Invalid(format!("{} is not a sheet metal feature", f.name))),
    }
}

fn poly_area(p: &[Vec2]) -> f64 {
    let m = p.len();
    (0..m).filter_map(|i| Some(p.get(i)?.cross(*p.get((i + 1) % m)?))).sum::<f64>() / 2.0
}

/// Current unmodified extrusion caps, including placement, certify the tool's depth.
/// Tessellation can drift in thickness at distant coordinates and cannot certify a slab.
fn sheet_tool_span(body: &Body, source: &Plane) -> Option<(f64, f64, f64, Option<(Plane, Region2)>)> {
    use kernel::FaceSource;
    #[derive(PartialEq)]
    enum Signature<'a> {
        Raw(&'a Region2),
        Certified(&'a [String], &'a [Vec<String>]),
    }
    let sources = body.face_sources()?;
    if sources.len() != body.face_count() || sources.len() > 16_384 {
        return None;
    }
    let n = source.normal();
    let mut family = None;
    let mut caps = [None, None];
    let mut projection_roundoff: f64 = 0.0;
    let mut footprint = None;
    for (index, entry) in sources.iter().enumerate() {
        let (group, cap) = match entry.as_ref()? {
            FaceSource::ExtrudeSide { .. } => (None, None),
            FaceSource::ExtrudeCap { high, region, plane } => (None, Some((*high, *plane, Signature::Raw(region)))),
            FaceSource::CertifiedExtrudeSide { producer, sketch, .. } => (Some((*producer, *sketch)), None),
            FaceSource::CertifiedExtrudeCap { producer, sketch, high, outer, holes, plane } => {
                (Some((*producer, *sketch)), Some((*high, *plane, Signature::Certified(outer, holes))))
            }
            _ => return None,
        };
        match family {
            None => family = Some(group),
            Some(previous) if previous != group => return None,
            _ => {}
        }
        if let Some((high, plane, signature)) = cap {
            if high && let Signature::Raw(region) = &signature {
                footprint = Some((plane, (*region).clone()));
            }
            let valid = |p: &Plane| {
                p.origin.is_finite()
                    && p.x.is_finite()
                    && p.y.is_finite()
                    && (p.x.len2() - 1.0).abs() <= 1e-10
                    && (p.y.len2() - 1.0).abs() <= 1e-10
                    && p.x.dot(p.y).abs() <= 1e-10
                    && p.normal().cross(n).len() <= 1e-10
            };
            let current = body.planar_face(index)?;
            if !valid(&plane) || !valid(&current) {
                return None;
            }
            // Bound floating point projection separately from model geometry tolerance.
            let roundoff = 128.0 * f64::EPSILON * (plane.origin.len() + current.origin.len() + source.origin.len()).max(1.0);
            // The cap's current point lies off the recorded plane by the noise of evaluating
            // its edges, which its fitted tilt shows across the distance between the two points
            // (8e-12 over 2.4 mm on a 45° flange's drill, #1255; none on axis-aligned caps).
            let noise = current.normal().cross(n).len() * (current.origin - plane.origin).len();
            if roundoff > 1e-7 || (current.origin - plane.origin).dot(n).abs() > roundoff + noise {
                return None;
            }
            projection_roundoff = projection_roundoff.max(roundoff);
            let distance = (plane.origin - source.origin).dot(n);
            if !distance.is_finite() || caps[usize::from(high)].is_some() {
                return None;
            }
            caps[usize::from(high)] = Some((distance, signature));
        }
    }
    let [Some((low, low_signature)), Some((high, high_signature))] = caps else { return None };
    (low_signature == high_signature && low != high).then_some((low.min(high), low.max(high), projection_roundoff, footprint))
}

/// Exact prismatic machining, retained in sheet material/depth metadata.
pub(super) fn sheet_cut(
    f: &Feature,
    st: &mut ModelState,
    tools: &[Body],
    targets: &[String],
    constructor: Option<&ExtrudeConstructor>,
    warning: &mut Option<String>,
) -> Result<Vec<String>> {
    let FeatureKind::Extrude { sketch, profiles, operation, extent, .. } = &f.kind else { return Ok(Vec::new()) };
    if extent.taper.is_some() || !any_eligible_sheet(f, st, targets) {
        return Ok(Vec::new());
    }
    let Some(ss) = st.sketch(sketch.0).cloned() else { return Ok(Vec::new()) };
    let regions = solidcraft_sketch::merge_regions(&select_profiles(&ss, profiles)?);
    // The outlines of the sketch's text: the sheet remembers where its letters went.
    let text: Vec<Seg2> = ss
        .sketch
        .links
        .iter()
        .filter(|l| matches!(l.source, solidcraft_sketch::LinkSource::Text { .. }))
        .flat_map(|l| ss.sketch.link_curves(&l.id))
        .flat_map(|ci| ss.sketch.exact_segs(ci).unwrap_or_else(|| ss.sketch.segs(ci)))
        .collect();
    sheet_prism_cut(f, st, &ss.plane, &regions, *operation, tools, targets, constructor, extent.to.is_some(), &text, warning)
}

/// Whether a cut of `f` with these targets (else its participants, else its component's
/// bodies) machines the sheet `original`.
fn sheet_eligible(f: &Feature, st: &ModelState, targets: &[String], original: &crate::sheet::SheetBody) -> bool {
    let targets = if targets.is_empty() { &f.participants } else { targets };
    let all = targets.len() == 1 && targets.first().is_some_and(|t| t == "*");
    all || if targets.is_empty() {
        st.bodies.iter().any(|b| b.name == original.body && (b.component == f.component || b.feature == f.id))
    } else {
        targets.contains(&original.body)
    }
}

/// Whether a cut of `f` with these targets machines any sheet metal body.
fn any_eligible_sheet(f: &Feature, st: &ModelState, targets: &[String]) -> bool {
    st.sheets.iter().any(|original| sheet_eligible(f, st, targets, original))
}

/// A Box or Cylinder cut (or intersect) whose tool is a straight prism along the normal of
/// the sheet it meets: machined like the equivalent extrude of its footprint, so the sheet
/// keeps it as a flat cut-out or pocket. A hole's drill (`drill`) may also be square to a
/// folded panel of the sheet, a flange's. Returns the sheet bodies it handled; none when the
/// primitive is not such a prism (the generic preflight then refuses real 3D machining).
pub(super) fn sheet_primitive_cut(
    f: &Feature,
    st: &mut ModelState,
    tool: &Body,
    prism: PrimitivePrism,
    operation: Operation,
    drill: bool,
    warning: &mut Option<String>,
) -> Result<Vec<String>> {
    if !matches!(operation, Operation::Cut | Operation::Intersect) || !any_eligible_sheet(f, st, &[]) {
        return Ok(Vec::new());
    }
    // The first sheet the primitive meets decides the machining direction. Sheets clear of it
    // by their boxes are passed over; the one sheet left needs no boolean to show it is met (that
    // boolean came out empty against a sheet carrying a label's cut-outs, and the hole was then
    // cut into the solid without a place in the flat pattern).
    let tool_box = body_box(tool);
    let near: Vec<&sheet::SheetBody> = st
        .sheets
        .iter()
        .filter(|sh| sheet_eligible(f, st, &[], sh) && st.body(&sh.body).is_some_and(|mb| !boxes_apart(&mesh_box(mb), &tool_box)))
        .collect();
    let mut normal = None;
    for sh in &near {
        if let Some(mb) = st.body(&sh.body)
            && (near.len() == 1 || kernel::boolean(&mb.body, tool, kernel::BoolOp::Intersect)?.is_some())
        {
            normal = Some(match prism {
                PrimitivePrism::Cylinder { axis, .. }
                    if drill && !sh.flat && Plane::from_normal(Vec3::ZERO, axis).is_some_and(|p| !sh.parallel_panels(&p).is_empty()) =>
                {
                    axis
                }
                _ => sh.frame.normal(),
            });
            break;
        }
    }
    let Some(normal) = normal else { return Ok(Vec::new()) };
    let Some((plane, region, interval)) = prism.along(normal) else { return Ok(Vec::new()) };
    let tools = kernel::extrude(&plane, std::slice::from_ref(&region), interval.0, interval.1)?;
    sheet_prism_cut(f, st, &plane, std::slice::from_ref(&region), operation, &tools, &[], None, false, &[], warning)
}

/// A hole's straight drill along a panel of a sheet (into its rim), kept as a bore of each
/// panel it meets (see [`crate::sheet::SheetBore`]), so the sheet still unfolds. Returns the
/// sheet bodies it machined; None, with the model as it was, when the drill is not a plain
/// cylinder or a sheet it meets can't keep it that way (it runs into a bend, or crosses a
/// panel at an angle).
pub(super) fn sheet_bore_cut(f: &Feature, st: &mut ModelState, prism: PrimitivePrism) -> Result<Option<Vec<String>>> {
    let PrimitivePrism::Cylinder { base, axis, radius, height } = prism else { return Ok(None) };
    let mut plans = Vec::new();
    for original in &st.sheets {
        if !sheet_eligible(f, st, &[], original) {
            continue;
        }
        let mut sh = original.clone();
        match sh.add_bore(base, axis, radius, height) {
            Ok(true) => {}
            Ok(false) => continue,
            // Refused here, the hole keeps the message of the machining it was first tried as.
            Err(DocError::Invalid(_)) => return Ok(None),
            Err(e) => return Err(e),
        }
        let body = sh.solid()?;
        plans.push((sh, body));
    }
    if plans.is_empty() {
        return Ok(None);
    }
    let mut handled = Vec::new();
    for (sh, body) in plans {
        if let Some(mb) = st.bodies.iter_mut().find(|b| b.name == sh.body) {
            replace_sheet_body(mb, body);
        }
        handled.push(sh.body.clone());
        if let Some(slot) = st.sheets.iter_mut().find(|s| s.body == sh.body) {
            *slot = sh;
        }
    }
    Ok(Some(handled))
}

/// The most hole tools machined in one pass by [`sheet_drill_cuts`].
const MAX_DRILLS_AT_ONCE: usize = 1000;

/// A hole feature's straight drills (`prisms`, all along one axis), machined into the sheets in
/// one pass for each depth they share: one rebuild of the sheet and of its flat pattern for all
/// of them, rather than one per drill, each on a sheet machined by the ones before.
/// The drills are the cut-outs and pockets [`sheet_primitive_cut`] makes of them one at a time.
/// None when they can't all be taken this way (they are not all cylinders along one axis that
/// every sheet they may machine takes square, or a sheet keeps one some other way): the caller
/// then puts the model back and drills them one by one.
pub(super) fn sheet_drill_cuts(
    f: &Feature,
    st: &mut ModelState,
    prisms: &[PrimitivePrism],
    warning: &mut Option<String>,
) -> Result<Option<Vec<String>>> {
    if prisms.len() < 2 || prisms.len() > MAX_DRILLS_AT_ONCE || !any_eligible_sheet(f, st, &[]) {
        return Ok(None);
    }
    let Some(PrimitivePrism::Cylinder { base: base0, axis: axis0, .. }) = prisms.first().copied() else { return Ok(None) };
    let Some(axis) = axis0.normalized() else { return Ok(None) };
    let Some(plane) = Plane::from_normal(base0, axis) else { return Ok(None) };
    // Every sheet the drills may machine takes them square: along its normal, or (folded) square
    // to one of its panels, as a single drill would be taken.
    for sh in st.sheets.iter().filter(|sh| sheet_eligible(f, st, &[], sh)) {
        let square = sh.frame.normal().cross(axis).len() <= 1e-12 || (!sh.flat && !sh.parallel_panels(&plane).is_empty());
        if !square {
            return Ok(None);
        }
    }
    // Each drill in the one plane: its circle, and its depth interval along the axis.
    let mut groups: Vec<((f64, f64), Vec<Region2>)> = Vec::new();
    for prism in prisms {
        let PrimitivePrism::Cylinder { base, axis: a, radius, height } = *prism else { return Ok(None) };
        let Some(a) = a.normalized() else { return Ok(None) };
        if a.cross(axis).len() > 1e-12 || ![base.x, base.y, base.z, radius, height].iter().all(|v| v.is_finite()) || !(radius > 0.0) {
            return Ok(None);
        }
        let (sense, at) = (a.dot(axis).signum(), (base - base0).dot(axis));
        let (e0, e1) = (at + sense * height.min(0.0), at + sense * height.max(0.0));
        let interval = (e0.min(e1), e0.max(e1));
        let circle = Region2 { outer: Loop2::circle(plane.to_local(base), radius), holes: vec![] };
        let same = |(lo, hi): (f64, f64)| (lo - interval.0).abs() <= 1e-9 * (1.0 + lo.abs()) && (hi - interval.1).abs() <= 1e-9 * (1.0 + hi.abs());
        match groups.iter_mut().find(|(i, _)| same(*i)) {
            Some((_, regions)) => regions.push(circle),
            None => groups.push((interval, vec![circle])),
        }
    }
    let mut handled: Vec<String> = Vec::new();
    for ((lo, hi), regions) in groups {
        let tools = kernel::extrude(&plane, &regions, lo, hi)?;
        let done = sheet_prism_cut(f, st, &plane, &regions, Operation::Cut, &tools, &[], None, false, &[], warning)?;
        if done.is_empty() {
            return Ok(None);
        }
        for name in done {
            if !handled.contains(&name) {
                handled.push(name);
            }
        }
    }
    Ok(Some(handled))
}

/// A primitive tool's exact geometry, for [`sheet_primitive_cut`].
#[derive(Clone, Copy, Debug)]
pub(super) enum PrimitivePrism {
    /// An axis-aligned box between two corners.
    Box { lo: Vec3, hi: Vec3 },
    /// A cylinder: base centre, unit axis, radius and signed height.
    Cylinder { base: Vec3, axis: Vec3, radius: f64, height: f64 },
}

impl PrimitivePrism {
    /// The sketch-like plane, footprint region and depth interval of this tool as a prism along
    /// `normal` (either sense), or None when it is not one.
    fn along(&self, normal: Vec3) -> Option<(Plane, Region2, (f64, f64))> {
        let n = normal.normalized()?;
        match *self {
            PrimitivePrism::Box { lo, hi } => {
                let (lo, hi) = (lo.min(hi), lo.max(hi));
                let s = hi - lo;
                // Plane axes (x, y) and the depth axis, as a right-handed frame.
                let (x, y, depth) = if n.cross(Vec3::Z).len() <= 1e-12 {
                    (Vec3::X, Vec3::Y, s.z)
                } else if n.cross(Vec3::X).len() <= 1e-12 {
                    (Vec3::Y, Vec3::Z, s.x)
                } else if n.cross(Vec3::Y).len() <= 1e-12 {
                    (Vec3::Z, Vec3::X, s.y)
                } else {
                    return None;
                };
                let plane = Plane::new(lo, x, y)?;
                let (w, h) = (s.dot(x), s.dot(y));
                let outer = Loop2::polygon(&[Vec2::ZERO, Vec2::new(w, 0.0), Vec2::new(w, h), Vec2::new(0.0, h)]);
                Some((plane, Region2 { outer, holes: vec![] }, (0.0, depth)))
            }
            PrimitivePrism::Cylinder { base, axis, radius, height } => {
                let axis = axis.normalized()?;
                if axis.cross(n).len() > 1e-12 {
                    return None;
                }
                let plane = Plane::from_normal(base, axis)?;
                Some((plane, Region2 { outer: Loop2::circle(Vec2::ZERO, radius), holes: vec![] }, (height.min(0.0), height.max(0.0))))
            }
        }
    }
}

/// The most outline segments of a sketch's text a cut tells letters by, and the most text regions
/// a sheet keeps (see [`crate::sheet::SheetBody::labels`]).
const MAX_TEXT_CURVES: usize = 20_000;
const MAX_LABELS: usize = 1000;

/// `text` is the outlines of the text in the cut's sketch (none for a primitive's cut).
#[allow(clippy::too_many_arguments)]
fn sheet_prism_cut(
    f: &Feature,
    st: &mut ModelState,
    plane: &Plane,
    regions: &[Region2],
    operation: Operation,
    tools: &[Body],
    targets: &[String],
    constructor: Option<&ExtrudeConstructor>,
    footprints_bound: bool,
    text: &[Seg2],
    warning: &mut Option<String>,
) -> Result<Vec<String>> {
    let n = plane.normal();
    let mut work = sheet::FlatWork::new();
    let faces = tools.iter().fold(0usize, |count, body| count.saturating_add(body.face_count()));
    if tools.len() > 1000 || faces > 16_384 {
        return Err(DocError::Invalid("sheet machining tools exceed the bounded geometry budget".into()));
    }
    work.charge(faces.saturating_mul(5))?;
    let mut span = (f64::MAX, f64::MIN);
    let mut cap_roundoff: f64 = 0.0;
    let mut current_footprints = Vec::new();
    for tool in tools {
        let Some((lo, hi, roundoff, footprint)) = sheet_tool_span(tool, plane) else {
            // Modified tools need a current geometric prism certificate. The generic
            // sheet preflight permits actual no-change, otherwise refuses stale metadata.
            return Ok(Vec::new());
        };
        if span.0 != f64::MAX && ((span.0 - lo).abs() > cap_roundoff + roundoff || (span.1 - hi).abs() > cap_roundoff + roundoff) {
            // Different profile tools cannot be represented as one filled depth interval.
            return Ok(Vec::new());
        }
        span = (span.0.min(lo), span.1.max(hi));
        cap_roundoff = cap_roundoff.max(roundoff);
        if let Some(footprint) = footprint {
            current_footprints.push(footprint);
        }
    }
    // Only untrimmed Distance tools may use their exact authored scalar interval.
    // The cap certificate still qualifies their current physical slab above.
    let selected_edges =
        regions.iter().flat_map(|r| std::iter::once(&r.outer).chain(&r.holes)).map(|l| l.segs.len()).fold(0usize, usize::saturating_add);
    work.charge(faces.saturating_mul(selected_edges).saturating_mul(16))?;
    let nominal = constructor.filter(|c| c.plane == *plane).and_then(|c| {
        let placed_basis = Plane::new(c.plane.origin, c.plane.x, c.plane.y)?;
        (current_footprints.len() == tools.len()
            && current_footprints.iter().all(|(plane, r)| plane.x == placed_basis.x && plane.y == placed_basis.y && regions.contains(r)))
        .then_some(c.interval)
    });
    // The regions of the cut that are letters, with their counters.
    let letters: Vec<&Region2> = if operation == Operation::Cut && !text.is_empty() && text.len() <= MAX_TEXT_CURVES {
        regions.iter().filter(|r| sheet::loop_lies_on(&r.outer, text)).collect()
    } else {
        Vec::new()
    };
    // Boxes around the tools: a tool clear of a bend by its box needs no boolean to show it (a
    // label's letters against every bend and flange of a tray).
    let tool_boxes: Vec<Option<Aabb3>> = tools.iter().map(body_box).collect();
    let mut plans = Vec::new();
    let mut notes = Vec::new();
    'sheets: for original in &st.sheets {
        if !sheet_eligible(f, st, targets, original) {
            continue;
        }
        let Some(mb) = st.body(&original.body) else {
            continue;
        };
        let mut sh = original.clone();
        if operation == Operation::Intersect && (!sh.flat && !sh.flanges.is_empty()) {
            super::sheet_boolean_unchanged(&mb.body, tools, operation)?;
            plans.push((sh.body.clone(), Some((sh, mb.body.clone()))));
            continue;
        }
        // No 3D boolean of each tool with the whole folded sheet first: the flat machining below
        // is exact, and a cut that machines nothing there is checked against the solid after it.
        // (That boolean cost each label hundreds of milliseconds, and against a sheet already
        // carrying a label's cut-outs it came out empty: the next cut was dropped.)
        for bends in sh.unmodeled_solids(plane)? {
            let bends_box = body_box(&bends);
            for (tool, tool_box) in tools.iter().zip(&tool_boxes) {
                if boxes_apart(&bends_box, tool_box) {
                    continue;
                }
                if kernel::boolean(&bends, tool, kernel::BoolOp::Intersect)?.is_some() {
                    return Err(DocError::Invalid(
                        "this machining intersects a folded bend or a nonparallel panel; unfold the sheet before machining that material".into(),
                    ));
                }
            }
        }
        let candidates = if sh.flat {
            if sh.frame.normal().cross(n).len() > 1e-6 { Vec::new() } else { sh.flat_regions()?.into_iter().map(|r| (None, r)).collect() }
        } else {
            sh.material_panels(Some(plane), &mut work)?
        };
        let mut modeled = false;
        let mut labels = Vec::new();
        for (pi, panel) in candidates {
            let w = if sh.flat { sh.world(None) } else { sh.world(pi) };
            let zorigin = (apply_point(&w, Vec3::ZERO) - plane.origin).dot(n);
            let sign = crate::apply_vector(&w, Vec3::Z).dot(n);
            if sign.abs() < 1e-6 {
                continue;
            }
            let (a, b) = ((span.0 - zorigin) / sign, (span.1 - zorigin) / sign);
            let roundoff = (cap_roundoff
                + 128.0 * f64::EPSILON * (apply_point(&w, Vec3::ZERO).len() + plane.origin.len() + span.0.abs() + span.1.abs() + sh.t).max(1.0))
                / sign.abs();
            let nominal_depth = if apply_point(&w, Vec3::ZERO) == plane.origin
                && let Some((lo, hi)) = nominal
                && (span.0 - lo).abs() <= roundoff
                && (span.1 - hi).abs() <= roundoff
            {
                // Equality of the two frame origins proves zero origin displacement;
                // use authored distances, never an inferred endpoint coincidence.
                let normal = crate::apply_vector(&w, Vec3::X).cross(crate::apply_vector(&w, Vec3::Y));
                let exact_sign = if normal == n {
                    1.0
                } else if normal == -n {
                    -1.0
                } else {
                    sign
                };
                Some((lo / exact_sign, hi / exact_sign))
            } else {
                None
            };
            if nominal_depth.is_none() && roundoff > 1e-7 {
                super::sheet_boolean_unchanged(&mb.body, tools, operation)?;
                plans.push((original.body.clone(), Some((original.clone(), mb.body.clone()))));
                continue 'sheets;
            }
            let (a, b) = nominal_depth.unwrap_or((a, b));
            let (z0, z1) = (a.min(b).max(0.0), a.max(b).min(sh.t));
            // Sub-resolution layers must not be silently erased by endpoint snapping.
            let depth_resolved = ![z0, sh.t - z1, z1 - z0].into_iter().any(|d| d > 0.0 && d < 1e-6);
            labels.extend(letters.iter().filter_map(|r| sh.region_to_flat(pi, plane, r).ok()));
            let mapped = if footprints_bound && current_footprints.len() == tools.len() {
                current_footprints.iter().map(|(plane, r)| sh.region_to_flat(pi, plane, r)).collect::<Result<Vec<_>>>()?
            } else {
                regions.iter().map(|r| sh.region_to_flat(pi, plane, r)).collect::<Result<Vec<_>>>()?
            };
            if operation == Operation::Intersect {
                if z1 <= z0 {
                    sh.holes.push(panel);
                    modeled = true;
                    continue;
                }
                let tool = sheet::material_union(mapped, &mut work)?;
                let retained = sheet::material_boolean(std::slice::from_ref(&panel), &tool, kernel::BoolOp::Intersect, &mut work)?;
                let removed = sheet::material_boolean(std::slice::from_ref(&panel), &retained, kernel::BoolOp::Cut, &mut work)?;
                if removed.is_empty() && !sh.material_outside_depth(&panel, z0, z1, &mut work)? {
                    modeled = true;
                    continue;
                }
                if !retained.is_empty() && !depth_resolved {
                    return Err(DocError::Invalid(
                        "sheet machining depth is below the supported 0.000001 mm extrusion resolution; increase the layer depth".into(),
                    ));
                }
                sh.holes.extend(removed);
                // Preserve original stock thickness and prior machining. Intersection
                // removes the complementary layers above/below every surviving region.
                for region in retained {
                    if z0 > 0.0 {
                        sh.record_pocket(region.clone(), 0.0, z0, &mut work)?;
                    }
                    if z1 < sh.t {
                        sh.record_pocket(region, z1, sh.t, &mut work)?;
                    }
                }
                modeled = true;
            } else if z1 > z0 {
                // Each tool meets only the panel's cut-outs near it: arranging every one of a
                // perforation's holes against every new hole would grow with their square (#732).
                let near = sheet::NearHoles::new(&panel)?;
                for flat in mapped {
                    let local = near.around(&flat)?;
                    let clipped =
                        sheet::material_boolean(std::slice::from_ref(&local), std::slice::from_ref(&flat), kernel::BoolOp::Intersect, &mut work)?;
                    if !clipped.is_empty() && !depth_resolved {
                        return Err(DocError::Invalid(
                            "sheet machining depth is below the supported 0.000001 mm extrusion resolution; increase the layer depth".into(),
                        ));
                    }
                    if !clipped.is_empty() && z0 <= 1e-7 && z1 >= sh.t - 1e-7 {
                        // Keep a single original tool boundary across structural seams.
                        // Re-clipping notch polygons creates coincident seam endpoints.
                        if !sh.holes.contains(&flat) {
                            sh.holes.push(flat);
                        }
                        modeled = true;
                    } else {
                        for region in clipped {
                            sh.record_pocket(region, z0, z1, &mut work)?;
                            modeled = true;
                        }
                    }
                }
            }
        }
        if modeled {
            for flat in labels {
                if !sh.labels.contains(&flat) && sh.labels.len() < MAX_LABELS {
                    sh.labels.push(flat);
                }
            }
        }
        if !modeled {
            super::sheet_boolean_unchanged(&mb.body, tools, operation)?;
            plans.push((sh.body.clone(), Some((sh, mb.body.clone()))));
        } else {
            let material = sh.flat_material()?;
            if material.is_empty() {
                // Empty exact material removes geometry and metadata together. Budget or
                // topology errors propagate; they are never mistaken for an empty blank.
                plans.push((sh.body.clone(), None));
            } else {
                // Every piece the cut leaves stays; the ones it parts from the base are named.
                notes.extend(parted_note(&sh, &material, original));
                let body = sh.solid()?;
                plans.push((sh.body.clone(), Some((sh, body))));
            }
        }
    }
    let handled = plans.iter().map(|(name, _)| name.clone()).collect();
    if !notes.is_empty() {
        append_warning(warning, notes.join("; "));
    }
    for (name, result) in plans {
        match result {
            Some((sh, body)) => {
                if let Some(mb) = st.bodies.iter_mut().find(|b| b.name == name) {
                    replace_sheet_body(mb, body);
                }
                if let Some(target) = st.sheets.iter_mut().find(|s| s.body == name) {
                    *target = sh;
                }
            }
            None => {
                st.bodies.retain(|b| b.name != name);
                st.sheets.retain(|s| s.body != name);
                st.threads.retain(|t| t.body != name);
            }
        }
    }
    Ok(handled)
}

/// A box around `body`, grown past its mesh's deflection from the exact faces (and a hair
/// more): bodies whose boxes are apart do not meet. None when it can't be worked out.
fn body_box(body: &Body) -> Option<Aabb3> {
    let tol = (body.size() * 1e-3).max(1e-4);
    padded_box(&body.tessellate(tol).ok()?, tol)
}

/// [`body_box`] of a model body, from the mesh it keeps.
fn mesh_box(mb: &ModelBody) -> Option<Aabb3> {
    padded_box(&mb.mesh(), mb.display_tol())
}

fn padded_box(mesh: &solidcraft_geom::Mesh, tol: f64) -> Option<Aabb3> {
    let b = mesh.bounds();
    let pad = 2.0 * tol + 1e-6;
    let pad = Vec3::new(pad, pad, pad);
    (!b.is_empty() && b.min.is_finite() && b.max.is_finite()).then(|| Aabb3 { min: b.min - pad, max: b.max + pad })
}

/// Whether two boxes are known to be apart (an unknown box is never apart).
fn boxes_apart(a: &Option<Aabb3>, b: &Option<Aabb3>) -> bool {
    let (Some(a), Some(b)) = (a, b) else { return false };
    a.max.x < b.min.x || b.max.x < a.min.x || a.max.y < b.min.y || b.max.y < a.min.y || a.max.z < b.min.z || b.max.z < a.min.z
}

/// A cut's note on the pieces it parted from the base of `sh` (see
/// [`sheet::SheetBody::parted_from_base`]; `material` is the sheet's flat material after the cut,
/// `before` the sheet before it): how many, their area and where they lie in the flat pattern.
/// The pieces stay, as every piece a cut leaves does. None when nothing is parted, and when the
/// pieces can't be worked out: the note is no reason for a cut to fail.
fn parted_note(sh: &sheet::SheetBody, material: &[Region2], before: &sheet::SheetBody) -> Option<String> {
    let pieces = sh.parted_from_base(material, before).ok().filter(|p| !p.is_empty())?;
    let area: f64 = pieces.iter().map(Region2::area).sum();
    let at: Vec<String> = pieces
        .iter()
        .take(4)
        .map(|r| {
            let c = r.interior_point();
            format!("({:.1}, {:.1})", c.x, c.y)
        })
        .collect();
    let at = at.join(", ");
    let body = &sh.body;
    Some(match pieces.len() {
        1 => format!("1 piece of `{body}` is no longer joined to its base ({area:.2} mm², near {at} in the flat pattern); it is kept"),
        n => {
            let more = if n > 4 { format!(" and {} more", n - 4) } else { String::new() };
            format!(
                "{n} pieces of `{body}` are no longer joined to its base ({area:.2} mm² in all, near {at}{more} in the flat pattern); they are kept"
            )
        }
    })
}

/// Outline loops (in `plane` coordinates) of the mesh triangles lying in the plane at distance
/// `level` from `at` along `n`: the edges used by one of them only, chained.
fn face_loops(mesh: &solidcraft_geom::Mesh, plane: &Plane, at: Vec3, n: Vec3, level: f64, tol: f64) -> Vec<Vec<Vec2>> {
    face_outline(mesh, plane, at, n, level, tol).into_iter().map(|l| l.into_iter().map(|(p, _)| p).collect()).collect()
}

/// [`face_loops`] with each point's B-rep face across the outline from it to the next point (the
/// wall the outline runs along there): the face off the plane that meets both points.
fn face_outline(mesh: &solidcraft_geom::Mesh, plane: &Plane, at: Vec3, n: Vec3, level: f64, tol: f64) -> Vec<Vec<(Vec2, Option<u32>)>> {
    let on_plane = |t: &[u32; 3]| t.iter().all(|k| mesh.positions.get(*k as usize).is_some_and(|p| ((*p - at).dot(n) - level).abs() < tol));
    let on: Vec<[u32; 3]> = mesh.triangles.iter().copied().filter(on_plane).collect();
    let pos = |k: u32| mesh.positions.get(k as usize).copied().unwrap_or_default();
    let q = |p: Vec3| ((p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64, (p.z * 1e6).round() as i64);
    // The faces off the plane at each point on it, by position to a millionth of a millimetre
    // either way (a neighbouring face's mesh may round a shared point differently).
    let mut touch: std::collections::HashMap<(i64, i64, i64), Vec<u32>> = Default::default();
    for (t, f) in mesh.triangles.iter().zip(&mesh.tri_face).filter(|(t, _)| !on_plane(t)) {
        for p in t.iter().map(|k| pos(*k)).filter(|p| ((*p - at).dot(n) - level).abs() < tol) {
            let faces = touch.entry(q(p)).or_default();
            if !faces.contains(f) {
                faces.push(*f);
            }
        }
    }
    let near = |p: Vec3| {
        let (x, y, z) = q(p);
        let cells = (-1..=1).flat_map(|i| (-1..=1).flat_map(move |j| (-1..=1).map(move |k| (x + i, y + j, z + k))));
        let mut faces: Vec<u32> = cells.filter_map(|c| touch.get(&c)).flatten().copied().collect();
        faces.sort_unstable();
        faces.dedup();
        faces
    };
    let across = |a: Vec3, b: Vec3| {
        let at_b = near(b);
        near(a).into_iter().find(|f| at_b.contains(f))
    };
    chain_loops(mesh, plane, &on, across)
}

/// The loops (in `plane` coordinates) that bound the triangles `on` of the mesh.
fn boundary_loops(mesh: &solidcraft_geom::Mesh, plane: &Plane, on: &[[u32; 3]]) -> Vec<Vec<Vec2>> {
    chain_loops(mesh, plane, on, |_, _| None).into_iter().map(|l| l.into_iter().map(|(p, _)| p).collect()).collect()
}

/// The loops (in `plane` coordinates) that bound the triangles `on` of the mesh, each point with
/// `across(it, next)`: the face the outline runs along from it to the next point.
fn chain_loops(
    mesh: &solidcraft_geom::Mesh,
    plane: &Plane,
    on: &[[u32; 3]],
    across: impl Fn(Vec3, Vec3) -> Option<u32>,
) -> Vec<Vec<(Vec2, Option<u32>)>> {
    // Boundary edges by position (meshes may repeat vertices per triangle).
    let pos = |k: u32| mesh.positions.get(k as usize).copied().unwrap_or_default();
    let q = |p: Vec3| ((p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64, (p.z * 1e6).round() as i64);
    let key = |a: Vec3, b: Vec3| {
        let (ka, kb) = (q(a), q(b));
        (ka.min(kb), ka.max(kb))
    };
    let mut count: std::collections::HashMap<((i64, i64, i64), (i64, i64, i64)), i32> = Default::default();
    let edges: Vec<(Vec3, Vec3)> = on.iter().flat_map(|t| [(pos(t[0]), pos(t[1])), (pos(t[1]), pos(t[2])), (pos(t[2]), pos(t[0]))]).collect();
    for (a, b) in &edges {
        *count.entry(key(*a, *b)).or_insert(0) += 1;
    }
    let mut boundary: Vec<(Vec3, Vec3)> = edges.into_iter().filter(|(a, b)| count.get(&key(*a, *b)) == Some(&1)).collect();
    // Chain into loops (plane coordinates).
    let mut loops: Vec<Vec<(Vec2, Option<u32>)>> = Vec::new();
    for _ in 0..10_000 {
        let Some((s0, mut cur)) = boundary.pop() else { break };
        let mut lp = vec![(s0, across(s0, cur))];
        for _ in 0..boundary.len() + 1 {
            if q(cur) == q(s0) {
                break;
            }
            match boundary.iter().position(|(a, _)| q(*a) == q(cur)) {
                Some(k) => {
                    let b = boundary.remove(k).1;
                    lp.push((cur, across(cur, b)));
                    cur = b;
                }
                None => {
                    lp.push((cur, None));
                    break;
                }
            }
        }
        let raw: Vec<(Vec2, Option<u32>)> = lp.iter().map(|(p, f)| (plane.to_local(*p), *f)).collect();
        // Drop points in the middle of straight runs.
        let m = raw.len();
        let pts: Vec<(Vec2, Option<u32>)> = (0..m)
            .filter_map(|i| {
                let (p, c, n) = (raw.get((i + m - 1) % m)?.0, *raw.get(i)?, raw.get((i + 1) % m)?.0);
                ((c.0 - p).cross(n - c.0).abs() > 1e-9 * (c.0 - p).len().max(1e-9) * (n - c.0).len().max(1e-9)).then_some(c)
            })
            .collect();
        if pts.len() >= 3 {
            loops.push(pts);
        }
    }
    loops
}

/// What the recesses of a plate (an engraved label, a counterbore) take out of it, for Convert
/// to Sheet Metal: each becomes a pocket of the sheet, from its floor to the face it opens on.
struct Recesses {
    /// The level (along the picked face's normal) of the plate's face without recesses, whose
    /// outline the sheet takes.
    level: f64,
    /// Pockets in flat coordinates, with their depth interval in the sheet (0 to its thickness).
    pockets: Vec<(Region2, f64, f64)>,
    /// The material they take out.
    volume: f64,
}

/// A planar face of the mesh parallel to the plate: its lowest and highest level, whether its
/// outward normal is the picked face's (it faces up), its area, a point on it, and its
/// triangles' centres and areas.
struct LevelFace {
    lo: f64,
    hi: f64,
    up: bool,
    area: f64,
    at: Vec3,
    tris: Vec<(Vec3, f64)>,
}

/// The recesses of a plate lying between levels `lo` and `hi` along `n` (measured from `at`),
/// whose sheet is built in `plane`. A recess's floor is a face parallel to the plate inside its
/// thickness, lying in an opening of the face it is cut into, and its walls are square to the
/// plate (walls that are not show as material the plate's volume doesn't account for). A body
/// that is no plate once its recesses are filled has none, so the plate check refuses it as
/// before: a bent part, whose legs' faces lie inside its extent. Recesses are taken in one face
/// only, up to half the thickness deep, and no larger than the face they are cut into (a plate
/// raised in places is no plate of the raised thickness).
fn plate_recesses(body: &Body, mesh: &Mesh, plane: &Plane, at: Vec3, n: Vec3, (lo, hi): (f64, f64), tol: f64) -> Result<Recesses> {
    let t = hi - lo;
    let eps = t * 1e-3;
    let level = |p: Vec3| (p - at).dot(n);
    let mut faces: BTreeMap<u32, Option<LevelFace>> = BTreeMap::new();
    for (tri, f) in mesh.triangles.iter().zip(&mesh.tri_face) {
        let Some([a, b, c]) = mesh.tri(tri) else { continue };
        let cr = (b - a).cross(c - a);
        let len = cr.len();
        if !(len > 1e-12) {
            continue;
        }
        let up = cr.dot(n) > 0.0;
        let entry = faces.entry(*f).or_insert(Some(LevelFace { lo: f64::MAX, hi: f64::MIN, up, area: 0.0, at: a, tris: Vec::new() }));
        match entry {
            Some(face) if face.up == up && cr.dot(n).abs() >= len * (1.0 - 1e-6) => {
                for p in [a, b, c] {
                    face.lo = face.lo.min(level(p));
                    face.hi = face.hi.max(level(p));
                }
                face.area += len / 2.0;
                face.tris.push(((a + b + c) / 3.0, len / 2.0));
            }
            _ => *entry = None,
        }
    }
    // The floors (face index, level, the face), and the areas of the plate's two faces (at `lo`
    // and at `hi`).
    let mut floors: Vec<(usize, f64, &LevelFace)> = Vec::new();
    let (mut bottom, mut top) = (0.0, 0.0);
    for (f, face) in faces.iter().filter_map(|(f, face)| Some((*f as usize, face.as_ref()?))) {
        let d = (face.lo + face.hi) / 2.0;
        if face.hi - face.lo > eps {
            continue;
        } else if (d - hi).abs() <= eps && face.up {
            top += face.area;
        } else if (d - lo).abs() <= eps && !face.up {
            bottom += face.area;
        } else if d > lo + eps && d < hi - eps {
            floors.push((f, d, face));
        }
    }
    let none = || Recesses { level: lo, pockets: Vec::new(), volume: 0.0 };
    if floors.is_empty() {
        return Ok(none());
    }
    // A floor is a recess's where (most of) it lies in an opening of the face it is cut into:
    // inside that face's outline and in a hole of it, not on an island in the hole. (Its
    // triangles are those at its level within `eps`: `tol` may exceed a recess's depth.)
    let loops_at = |level: f64| -> Vec<Loop2> { face_loops(mesh, plane, at, n, level, eps).into_iter().map(|l| Loop2::polygon(&l)).collect() };
    let (top_loops, bottom_loops) = (loops_at(hi), loops_at(lo));
    floors.retain(|(_, _, face)| {
        let loops = if face.up { &top_loops } else { &bottom_loops };
        let open = |p: Vec2| {
            let k = loops.iter().filter(|l| l.contains(p)).count();
            k >= 2 && k.is_multiple_of(2)
        };
        face.tris.iter().filter(|(c, _)| open(plane.to_local(*c))).map(|(_, a)| a).sum::<f64>() > face.area / 2.0
    });
    let Some(&(_, _, first)) = floors.first() else { return Ok(none()) };
    // Filled, the recesses leave a plate: its outline as large at `lo` as at `hi` once their
    // openings count, and its volume with theirs the outline times the thickness. Else (a bent
    // part) they are none.
    let depth_in = |d: f64, up: bool| if up { hi - d } else { d - lo };
    let opened = |up: bool| floors.iter().filter(|f| f.2.up == up).map(|f| f.2.area).sum::<f64>();
    let outline = bottom + opened(false);
    let removed: f64 = floors.iter().map(|f| f.2.area * depth_in(f.1, f.2.up)).sum();
    let volume = kernel::measure(body)?.volume;
    if (outline - top - opened(true)).abs() > 1e-3 * outline || (outline * t - volume - removed).abs() > 1e-3 * volume.max(1.0) {
        return Ok(none());
    }
    let pt = |p: Vec3| format!("({:.3}, {:.3}, {:.3})", p.x, p.y, p.z);
    let up = first.up;
    if let Some(other) = floors.iter().find(|f| f.2.up != up) {
        return Err(DocError::Invalid(format!(
            "not supported yet: converting a plate with recesses in both faces (at {} and {}); recesses in one face become pockets",
            pt(first.at),
            pt(other.2.at)
        )));
    }
    let depth_of = |d: f64| depth_in(d, up);
    if let Some(deep) = floors.iter().find(|f| depth_of(f.1) > t / 2.0 + eps) {
        return Err(DocError::Invalid(format!(
            "not supported yet: the recess at {} is {:.3} mm deep, more than half the plate's {:.3} mm; shallower recesses (an engraving, a label) become pockets",
            pt(deep.2.at),
            depth_of(deep.1),
            t
        )));
    }
    let face_area = if up { top } else { bottom };
    if floors.iter().map(|f| f.2.area).sum::<f64>() > face_area {
        return Err(DocError::Invalid(format!(
            "not supported yet: converting a plate whose recesses are larger than the face they are in (a plate thinner than {t:.3} mm, raised in places)"
        )));
    }
    // The floors' outlines, from a mesh fine enough to mark them by.
    let fine = body.tessellate((t * 1e-2).min(tol).max(1e-4))?;
    floors.sort_by(|a, b| depth_of(a.1).total_cmp(&depth_of(b.1)));
    // Floors at one depth are one pocket where they meet.
    let mut groups: Vec<(f64, Vec<Region2>)> = Vec::new();
    for (f, d, ..) in &floors {
        let on: Vec<[u32; 3]> = fine.triangles.iter().zip(&fine.tri_face).filter(|(_, g)| **g as usize == *f).map(|(tri, _)| *tri).collect();
        let mut loops = boundary_loops(&fine, plane, &on);
        loops.sort_by(|a, b| poly_area(b).abs().total_cmp(&poly_area(a).abs()));
        let mut it = loops.into_iter();
        let Some(outline) = it.next() else { continue };
        let region = Region2 { outer: Loop2::polygon(&outline).ccw(), holes: it.map(|l| Loop2::polygon(&l).ccw().reversed()).collect() };
        match groups.last_mut() {
            Some((depth, regions)) if depth_of(*d) - *depth <= eps => regions.push(region),
            _ => groups.push((depth_of(*d), vec![region])),
        }
    }
    // The sheet's z starts from the picked face when its plane faces away from that face.
    let from_zero = up == (plane.normal().dot(n) < 0.0);
    let mut work = sheet::FlatWork::new();
    let (mut pockets, mut volume) = (Vec::new(), 0.0);
    for (depth, regions) in groups {
        let (z0, z1) = if from_zero { (0.0, depth) } else { (t - depth, t) };
        for region in sheet::material_union(regions, &mut work)? {
            volume += region.area().abs() * depth;
            pockets.push((region, z0, z1));
        }
    }
    Ok(Recesses { level: if up { lo } else { hi }, pockets, volume })
}

/// The body a placed plastic feature joins: by name, else the one under the point.
fn body_under(st: &ModelState, body: &Option<crate::BodyName>, p: Vec3) -> Result<String> {
    let i = body_at(st, body, &[p])?;
    let b = st.bodies.get(i).ok_or_else(|| DocError::Invalid("there is no body".into()))?;
    let tolerance = (b.body.size() * 1e-5).max(1e-4);
    if crate::project::nearest_face(&b.mesh(), p).is_none_or(|(_, _, distance)| distance > tolerance) {
        return Err(DocError::Invalid("the plastic feature's placement is no longer on its body face".into()));
    }
    Ok(b.name.clone())
}

/// Axes with z along `z` (and x along `x` when given).
fn local_frame(z: Vec3, x: Option<Vec3>) -> Result<(Vec3, Vec3, Vec3)> {
    let z = z.normalized().ok_or_else(|| DocError::Invalid("direction".into()))?;
    let x = x.and_then(|x| (x - z * x.dot(z)).normalized()).unwrap_or_else(|| z.any_perp());
    let y = z.cross(x);
    Ok((x, y, z))
}

/// Offset a closed polygon to its left by `d` (mitred corners).
fn offset_left(p: &[Vec2], d: f64) -> Vec<Vec2> {
    let m = p.len();
    let line = |i: usize| -> Option<(Vec2, Vec2)> {
        let (a, b) = (*p.get(i)?, *p.get((i + 1) % m)?);
        let t = (b - a).normalized()?;
        Some((a + Vec2::new(-t.y, t.x) * d, t))
    };
    (0..m)
        .filter_map(|i| {
            let (p0, d0) = line((i + m - 1) % m)?;
            let (p1, d1) = line(i)?;
            let den = d0.cross(d1);
            if den.abs() < 1e-12 {
                return Some(p1);
            }
            Some(p0 + d0 * ((p1 - p0).cross(d1) / den))
        })
        .collect()
}

pub(super) fn plastic_eval(doc: &Document, vals: &BTreeMap<String, Value>, f: &Feature, st: &mut ModelState) -> Result<()> {
    let len = |e: &str| val(vals, e, Kind::Length);
    // The plastic rule of the body a feature goes on gives its defaults (draft, clearance).
    let rule = |target: &str| -> Result<Option<crate::plastic::PlasticValues>> {
        doc.plastic_rule_for(target).map(|r| doc.plastic_values(vals, &r)).transpose()
    };
    // Tools start a little inside the material so they overlap it instead of touching a face.
    match &f.kind {
        FeatureKind::Boss {
            position,
            direction,
            diameter,
            height,
            hole_diameter,
            hole_depth,
            draft,
            fillet,
            ribs,
            rib_thickness,
            rib_length,
            rib_offset,
            body,
        } => {
            let target = body_under(st, body, position.0)?;
            let (r0, h) = (len(diameter)? / 2.0, len(height)?);
            if !(r0 > 0.0 && h > 0.0) {
                return Err(DocError::Invalid("the boss diameter and height must be positive".into()));
            }
            let dr = match draft {
                Some(e) => val(vals, e, Kind::Angle)?,
                None => rule(&target)?.map_or(0.0, |r| r.draft),
            };
            let rf = match fillet {
                Some(e) => len(e)?.max(0.0),
                None => 0.0,
            };
            let r_top = r0 - h * dr.tan();
            if r_top <= 0.0 {
                return Err(DocError::Invalid("the draft closes the boss before its top".into()));
            }
            let delta = (h * 0.05).clamp(1e-3, 0.5);
            let (x, _, z) = local_frame(direction.0, None)?;
            // The post: a (tapered) cylinder from a little inside the face.
            let base = Plane::new(position.0 - z * delta, x, z.cross(x)).ok_or_else(|| DocError::Invalid("boss frame".into()))?;
            let r_in = r0 + delta * dr.tan();
            let disc = Region2 { outer: Loop2::circle(Vec2::ZERO, r_in), holes: Vec::new() };
            let post = if dr.abs() > 1e-12 {
                vec![kernel::extrude_tapered(&base, &disc, h + delta, 1.0, -dr)?]
            } else {
                kernel::extrude(&base, &[disc], 0.0, h + delta)?
            };
            let mut tools: Vec<Traced> = post.into_iter().map(|b| crate::lineage::part_tool(b, "post")).collect();
            if let Some(n) = ribs {
                let n = val(vals, n, Kind::Unitless)?.round();
                if !(0.0..=64.0).contains(&n) {
                    return Err(DocError::Invalid("0…64 ribs".into()));
                }
                let t = match rib_thickness {
                    Some(e) => len(e)?,
                    None => r0 * 0.3,
                };
                let l = match rib_length {
                    Some(e) => len(e)?,
                    None => r0,
                };
                let off = match rib_offset {
                    Some(e) => len(e)?,
                    None => 0.0,
                };
                let hr = h - off;
                if n > 0.0 && !(t > 0.0 && l > 0.0 && hr > 0.0) {
                    return Err(DocError::Invalid("ribs need a positive thickness, length and height".into()));
                }
                for k in 0..n as usize {
                    let a = std::f64::consts::TAU * k as f64 / n;
                    let (u, v) = (x * a.cos() + z.cross(x) * a.sin(), z.cross(x) * a.cos() - x * a.sin());
                    let rp = Plane::new(position.0, u, v).ok_or_else(|| DocError::Invalid("rib frame".into()))?;
                    let rect = Region2 {
                        outer: Loop2::polygon(&[
                            Vec2::new(r0 * 0.5, -t / 2.0),
                            Vec2::new(r0 + l, -t / 2.0),
                            Vec2::new(r0 + l, t / 2.0),
                            Vec2::new(r0 * 0.5, t / 2.0),
                        ]),
                        holes: Vec::new(),
                    };
                    tools.extend(kernel::extrude(&rp, &[rect], -delta, hr)?.into_iter().map(|b| crate::lineage::part_tool(b, &format!("rib{k}"))));
                }
            }
            apply_traced(st, f, tools, Operation::Join, std::slice::from_ref(&target))?;
            // The root fillet: rounds the circle where the post meets the face.
            if rf > 1e-9 {
                let i = st.bodies.iter().position(|b| b.name == target).ok_or_else(|| DocError::Invalid("boss body".into()))?;
                let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("boss body".into())) };
                let mut warn = None;
                let edges = resolve_edges(&mb.body, &[position.0 + x * r0], &[], &mut warn)?;
                let nb = mb.traced().then(kernel::fillet(&mb.body, &edges, rf)?);
                if let Some(slot) = st.bodies.get_mut(i) {
                    *slot = ModelBody::traced_new(mb.name, nb, mb.feature);
                }
            }
            if let Some(hd) = hole_diameter {
                let rh = len(hd)? / 2.0;
                let depth = match hole_depth {
                    Some(e) => len(e)?,
                    None => h,
                };
                if !(rh > 0.0 && rh < r_top && depth > 0.0) {
                    return Err(DocError::Invalid("the boss hole must fit inside the boss".into()));
                }
                let hp = Plane::new(position.0, x, z.cross(x)).ok_or_else(|| DocError::Invalid("hole frame".into()))?;
                let tool = kernel::extrude(&hp, &[Region2 { outer: Loop2::circle(Vec2::ZERO, rh), holes: Vec::new() }], h - depth, h + delta)?;
                let tool = tool.into_iter().map(|b| crate::lineage::part_tool(b, "hole")).collect();
                apply_traced(st, f, tool, Operation::Cut, std::slice::from_ref(&target))?;
            }
            Ok(())
        }
        FeatureKind::Lip { face, width, height, groove, gap, outside, body } => {
            let target = body_under(st, body, face.0)?;
            let mb = st.body(&target).cloned().ok_or_else(|| DocError::Invalid("body".into()))?;
            let n = planar_normal_at(&mb.body, face.0).ok_or_else(|| DocError::Invalid("pick the planar rim face".into()))?;
            let (w, h) = (len(width)?, len(height)?);
            let g = match gap {
                Some(e) => len(e)?,
                None if *groove => rule(&target)?.map_or(0.0, |r| r.clearance),
                None => 0.0,
            };
            if !(w > 0.0 && h > 0.0 && g >= 0.0) {
                return Err(DocError::Invalid("the lip width and height must be positive".into()));
            }
            let plane = Plane::from_normal(face.0, n).ok_or_else(|| DocError::Invalid("rim plane".into()))?;
            let tol = (mb.body.size() * 1e-3).max(1e-3);
            let mesh = mb.body.tessellate(tol)?;
            let mut loops = face_loops(&mesh, &plane, face.0, n, 0.0, tol);
            loops.sort_by(|a, b| poly_area(b).abs().total_cmp(&poly_area(a).abs()));
            let ccw = |l: &[Vec2]| if poly_area(l) < 0.0 { l.iter().rev().copied().collect::<Vec<_>>() } else { l.to_vec() };
            // The inner edge is an opening (the cavity under a rim): not the foot of a boss or a
            // wall standing on the face, whose side the band would run against.
            let opening = |l: &[Vec2]| {
                let eps = (tol * 4.0).max(1e-3);
                l.iter().zip(l.iter().cycle().skip(1)).filter_map(|(a, b)| Some(((*a + *b) * 0.5, (*b - *a).normalized()?))).take(3).any(|(m, t)| {
                    // Just over the face, inside the loop (to the left of a ccw loop's edges).
                    let q = plane.to_world(m + Vec2::new(-t.y, t.x) * eps) + n * eps;
                    !mesh.contains(q)
                })
            };
            let outer = loops.first().map(|l| ccw(l));
            let openings: Vec<Vec<Vec2>> = loops.iter().skip(1).map(|l| ccw(l)).filter(|l| opening(l)).collect();
            // A rim runs round one opening, the cavity; smaller ones (a screw hole) may sit in it.
            // Openings alike in size (a plate's holes) leave no edge that is the rim's.
            let alike = match openings.as_slice() {
                [a, b, ..] => poly_area(b).abs() > 0.5 * poly_area(a).abs(),
                _ => false,
            };
            let (Some(outer), Some(inner), false) = (outer, openings.first().cloned(), alike) else {
                let what = match (loops.len(), openings.len()) {
                    (0 | 1, _) => "pick the rim face of a shelled body (a face with an inner and an outer edge)".to_string(),
                    (_, 0) => "pick the rim face of a shelled body: this face's inner edges are where other features stand on it, not an opening"
                        .to_string(),
                    (_, k) => format!("pick the rim face of a shelled body (round one opening): this face has {k} openings alike in size"),
                };
                return Err(DocError::Invalid(what));
            };
            let bw = if *groove { w + g } else { w };
            // A lip: the band raised from the rim's own faces (its walls continue the walls
            // below, which a join can't do: they would coincide).
            if !*groove
                && let Ok(b) = kernel::raise_band(&mb.body, face.0, w, h, *outside)
                && let Some(slot) = st.bodies.iter_mut().find(|x| x.name == target)
            {
                *slot = ModelBody::traced_new(target.clone(), Traced::step(b, &[&mb.traced()]), slot.feature);
                return Ok(());
            }
            // The band along the chosen edge, inside the rim face. A groove's cutter reaches past
            // the edge into the free side (the cavity, or outside), so no wall coincides.
            let free = if *groove { bw } else { 0.0 };
            let band = if *outside {
                Region2 {
                    outer: Loop2::polygon(&offset_left(&outer, -free)).ccw(),
                    holes: vec![Loop2::polygon(&offset_left(&outer, bw)).ccw().reversed()],
                }
            } else {
                Region2 {
                    outer: Loop2::polygon(&offset_left(&inner, -bw)).ccw(),
                    holes: vec![Loop2::polygon(&offset_left(&inner, free)).ccw().reversed()],
                }
            };
            let delta = (h * 0.05).clamp(1e-3, 0.5);
            if *groove {
                let tool = kernel::extrude(&plane, &[band], -(h + g), delta)?.into_iter().map(|b| crate::lineage::part_tool(b, "groove")).collect();
                apply_traced(st, f, tool, Operation::Cut, std::slice::from_ref(&target))
            } else {
                let tool = kernel::extrude(&plane, &[band], -delta, h)?.into_iter().map(|b| crate::lineage::part_tool(b, "lip")).collect();
                apply_traced(st, f, tool, Operation::Join, std::slice::from_ref(&target))
            }
        }
        FeatureKind::SnapFit { position, direction, hook, length, thickness, width, catch_depth, catch_length, body } => {
            let target = body_under(st, body, position.0)?;
            let (l, t, w, cd, cl) = (len(length)?, len(thickness)?, len(width)?, len(catch_depth)?, len(catch_length)?);
            if !(l > 0.0 && t > 0.0 && w > 0.0 && cd > 0.0 && cl > 0.0 && cl < l) {
                return Err(DocError::Invalid("the snap fit's sizes must be positive and the catch shorter than the arm".into()));
            }
            let (x, y, z) = local_frame(direction.0, Some(hook.0))?;
            let delta = (l * 0.05).clamp(1e-3, 0.5);
            // Side profile (x = toward the catch, y = up the arm), extruded across the width.
            let pts =
                [Vec2::new(-t, -delta), Vec2::new(0.0, -delta), Vec2::new(0.0, l - cl), Vec2::new(cd, l - cl), Vec2::new(0.0, l), Vec2::new(-t, l)];
            let o = position.0 - y * (w / 2.0);
            let pl = Plane::new(o, x, z).ok_or_else(|| DocError::Invalid("snap fit frame".into()))?;
            let mut lp = Loop2::polygon(&pts);
            if lp.signed_area() < 0.0 {
                lp = lp.reversed();
            }
            let sgn = if pl.normal().dot(y) >= 0.0 { 1.0 } else { -1.0 };
            let (lo, hi) = if sgn > 0.0 { (0.0, w) } else { (-w, 0.0) };
            let tool = kernel::extrude(&pl, &[Region2 { outer: lp, holes: Vec::new() }], lo, hi)?;
            let tool = tool.into_iter().map(|b| crate::lineage::part_tool(b, "arm")).collect();
            apply_traced(st, f, tool, Operation::Join, std::slice::from_ref(&target))
        }
        FeatureKind::Rest { position, direction, along, width, length, height, draft, thickness, body } => {
            let target = body_under(st, body, position.0)?;
            let (w, h) = (len(width)?, len(height)?);
            let l = length.as_deref().map(len).transpose()?;
            let dr = match draft {
                Some(e) => val(vals, e, Kind::Angle)?,
                None => rule(&target)?.map_or(0.0, |r| r.draft),
            };
            let t = thickness.as_deref().map(len).transpose()?;
            if !(w > 0.0 && h > 0.0 && l.is_none_or(|l| l > 0.0) && t.is_none_or(|t| t > 0.0 && 2.0 * t < w.min(l.unwrap_or(w)))) {
                return Err(DocError::Invalid("the rest's sizes must be positive and its wall thinner than half its width".into()));
            }
            if !(dr.abs() < 1.0) {
                return Err(DocError::Invalid("the rest's draft must be under 57°".into()));
            }
            let shrink = h * dr.tan();
            let closes = match t {
                Some(t) => 2.0 * shrink >= t,
                None => 2.0 * shrink >= w.min(l.unwrap_or(w)),
            };
            if closes {
                return Err(DocError::Invalid("the draft closes the rest before its top".into()));
            }
            let delta = (h * 0.05).clamp(1e-3, 0.5);
            let (x, y, z) = local_frame(direction.0, along.map(Vec3::from))?;
            // Sized at the face: the tool starts a little inside, grown by the draft over that.
            let e = delta * dr.tan();
            let shape = |grow: f64| match l {
                Some(l) => Loop2::polygon(&[
                    Vec2::new(-l / 2.0 - grow, -w / 2.0 - grow),
                    Vec2::new(l / 2.0 + grow, -w / 2.0 - grow),
                    Vec2::new(l / 2.0 + grow, w / 2.0 + grow),
                    Vec2::new(-l / 2.0 - grow, w / 2.0 + grow),
                ]),
                None => Loop2::circle(Vec2::ZERO, w / 2.0 + grow),
            };
            let holes = match t {
                Some(t) => vec![shape(-t - e).reversed()],
                None => Vec::new(),
            };
            let region = Region2 { outer: shape(e), holes };
            let base = Plane::new(position.0 - z * delta, x, y).ok_or_else(|| DocError::Invalid("rest frame".into()))?;
            let tool = if dr.abs() > 1e-12 {
                vec![kernel::extrude_tapered(&base, &region, h + delta, 1.0, -dr)?]
            } else {
                kernel::extrude(&base, &[region], 0.0, h + delta)?
            };
            let tool = tool.into_iter().map(|b| crate::lineage::part_tool(b, "rest")).collect();
            apply_traced(st, f, tool, Operation::Join, std::slice::from_ref(&target))
        }
        _ => Err(DocError::Invalid(format!("{} is not a plastic feature", f.name))),
    }
}

/// Edge picks of a sheet feature, moved onto their named edges (a name that is gone keeps the
/// point, with a warning).
fn sheet_picks(sh: &crate::sheet::SheetBody, f: &Feature, edges: &[Vec3], warning: &mut Option<String>) -> Vec<Vec3> {
    let mut gone = Vec::new();
    let out = edges
        .iter()
        .enumerate()
        .map(|(i, p)| match f.edge_names.get(i).filter(|n| !n.is_empty()) {
            Some(n) => sh.edge_by_name(n, *p).unwrap_or_else(|| {
                gone.push(n.clone());
                *p
            }),
            None => *p,
        })
        .collect();
    if !gone.is_empty() {
        *warning = Some(format!("sheet edge(s) {} no longer exist; using the edges at the picked points", gone.join(", ")));
    }
    out
}

/// Tag the flanges a feature just added (`F<feature>.<k>`).
fn tag_flanges(sh: &mut crate::sheet::SheetBody, from: usize, feature: u64) {
    for (k, fl) in sh.flanges.iter_mut().enumerate().skip(from) {
        fl.tag = format!("F{feature}.{}", k - from);
    }
}
