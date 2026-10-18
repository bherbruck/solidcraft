//! Split Face: divide chosen faces of a body along where a tool meets them (a plane, another
//! face's surface, or sketch curves swept along the sketch normal), leaving the solid itself
//! unchanged. Edges the split line ends on are cut in every face that uses them.
//!
//! The tool is a signed field (which side of the tool a point is on). Its zero set on a face is
//! traced on the face's triangulation, every crossing is placed exactly on the surface by
//! bisection in the face's parameters, and the line ends exactly on the boundary edges.

use std::collections::HashMap;

use mt::{BoundedCurve, InnerSpace, MetricSpace, ParametricCurve, ParametricSurface, ParametricSurface3D, SearchNearestParameter};
use solidcraft_geom::{Plane, Seg2, Vec2, Vec3};
use truck_meshalgo::tessellation::RobustMeshableShape;
use truck_modeling as mt;

use crate::body::{Body, from_p3, p3};
use crate::record::Rebuild;
use crate::{KernelError, Result, guard};

/// What splits the faces.
#[derive(Clone, Debug)]
pub enum SplitTool<'a> {
    /// A plane (unbounded).
    Plane(Plane),
    /// The surface of another body's face (`face` in that body's face order), extended.
    Face { body: &'a Body, face: usize },
    /// Sketch curves (chains of segments in `plane`) swept along `along` (none: the plane's
    /// normal); each curve splits on its own, an open one extended past its ends. A single line
    /// splits exactly (a plane), and so does a single arc swept along the normal (a cylinder);
    /// longer chains, splines and arcs swept aslant follow the curve.
    Curves { plane: Plane, curves: Vec<Vec<Seg2>>, along: Option<Vec3> },
}

/// Which side of the tool a point is on (zero on the tool).
enum Field {
    Plane {
        o: Vec3,
        n: Vec3,
    },
    Surface(mt::Surface),
    /// A circular cylinder (radius `r` about the line through `o` along unit `axis`); `out` is
    /// +1 when the tool's normal points away from the axis.
    Cylinder {
        o: Vec3,
        axis: Vec3,
        r: f64,
        out: f64,
    },
    /// A chain of points in `plane`, swept along `along` (the plane's normal when none). A
    /// closed chain is positive inside it; an open one on its left, its end segments extended.
    Curve {
        plane: Plane,
        pts: Vec<Vec2>,
        closed: bool,
        along: Option<Vec3>,
    },
}

impl Field {
    fn eval(&self, p: Vec3) -> f64 {
        match self {
            Field::Plane { o, n } => (p - *o).dot(*n),
            Field::Surface(s) => match s.search_nearest_parameter(p3(p), None, 50) {
                Some((u, v)) => {
                    let q = from_p3(s.subs(u, v));
                    let n = s.normal(u, v);
                    (p - q).dot(Vec3::new(n.x, n.y, n.z))
                }
                None => f64::NAN,
            },
            Field::Cylinder { o, axis, r, out } => {
                let v = p - *o;
                let radial = v - *axis * v.dot(*axis);
                *out * (radial.len() - *r)
            }
            Field::Curve { plane, pts, closed, along } => {
                // Where p's line along the sweep direction meets the plane.
                let p = match along {
                    Some(d) => p - *d * ((p - plane.origin).dot(plane.normal()) / d.dot(plane.normal())),
                    None => p,
                };
                let q = plane.to_local(p);
                let q = Vec2::new(q.x, q.y);
                if *closed {
                    // Distance to the loop, positive inside it.
                    let d = (0..pts.len().saturating_sub(1))
                        .filter_map(|i| Some((*pts.get(i)?, *pts.get(i + 1)?)))
                        .map(|(a, b)| {
                            let d = b - a;
                            let l2 = d.dot(d);
                            let t = if l2 > 0.0 { ((q - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
                            (q - (a + d * t)).len()
                        })
                        .fold(f64::INFINITY, f64::min);
                    let uv: Vec<(f64, f64)> = pts.iter().map(|p| (p.x, p.y)).collect();
                    return if inside(&uv, (q.x, q.y)) { d } else { -d };
                }
                // Signed distance to the polyline, its end segments extended as lines.
                let n = pts.len();
                let mut best = (f64::INFINITY, 0.0);
                for i in 0..n.saturating_sub(1) {
                    let (Some(&a), Some(&b)) = (pts.get(i), pts.get(i + 1)) else { continue };
                    let d = b - a;
                    let l2 = d.dot(d);
                    if !(l2 > 0.0) {
                        continue;
                    }
                    let mut t = (q - a).dot(d) / l2;
                    if i > 0 {
                        t = t.max(0.0);
                    }
                    if i + 2 < n {
                        t = t.min(1.0);
                    }
                    let c = a + d * t;
                    let dist = (q - c).len();
                    if dist < best.0 {
                        let side = d.x * (q.y - a.y) - d.y * (q.x - a.x);
                        best = (dist, if side >= 0.0 { dist } else { -dist });
                    }
                }
                best.1
            }
        }
    }
}

/// A crossing of the split line with a face boundary edge.
#[derive(Clone)]
struct Hit {
    edge: mt::Edge,
    t: f64,
    p: mt::Point3,
}

/// One traced split line on a face: points (exact) with their surface parameters, and whether
/// it closes on itself.
struct Chain {
    pts: Vec<(mt::Point3, (f64, f64))>,
    closed: bool,
}

/// Mesh tolerance for tracing a face.
fn trace_tol(face: &mt::Face, size: f64) -> f64 {
    let _ = face;
    trace_tol_of(size)
}

/// Mesh tolerance for tracing on a body of this size.
fn trace_tol_of(size: f64) -> f64 {
    (size * 2e-3).max(1e-4)
}

/// Trace the zero set of `g` over a face: chains of exact points.
fn trace(face: &mt::Face, g: &Field, size: f64) -> Result<Vec<Chain>> {
    if let Some(chains) = trace_flat(face, g, size) {
        return Ok(chains);
    }
    let tol = trace_tol(face, size);
    let shell: mt::Shell = vec![face.clone()].into();
    let meshed = shell.robust_triangulation(tol);
    let Some(mf) = meshed.face_iter().next() else { return Ok(Vec::new()) };
    let Some(pm) = mf.surface() else { return Err(KernelError::Failed("the face could not be meshed".into())) };
    let surface = face.surface();
    let (pos, uvs) = (pm.positions(), pm.uv_coords());
    // The face's triangles subdivided in parameter space, finely enough that a small split line
    // (a hole's circle on a big face) crosses some triangle sides. Corners shared between
    // triangles are found by their (rounded) parameters.
    let step = size / 100.0;
    let mut ids: HashMap<(i64, i64), usize> = HashMap::new();
    let mut corner_uv: Vec<(f64, f64)> = Vec::new();
    let (mut umin, mut umax, mut vmin, mut vmax) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for uv in uvs {
        umin = umin.min(uv.x);
        umax = umax.max(uv.x);
        vmin = vmin.min(uv.y);
        vmax = vmax.max(uv.y);
    }
    let qu = ((umax - umin).abs() * 1e-9).max(1e-12);
    let qv = ((vmax - vmin).abs() * 1e-9).max(1e-12);
    let mut corner = |uv: (f64, f64)| -> usize {
        let k = ((uv.0 / qu).round() as i64, (uv.1 / qv).round() as i64);
        *ids.entry(k).or_insert_with(|| {
            corner_uv.push(uv);
            corner_uv.len() - 1
        })
    };
    let mut base: Vec<[((f64, f64), mt::Point3); 3]> = Vec::new();
    for t in pm.faces().triangle_iter() {
        let mut c = [((0.0, 0.0), mt::Point3::new(0.0, 0.0, 0.0)); 3];
        for (k, v) in t.iter().enumerate() {
            let Some(uv) = v.uv.and_then(|i| uvs.get(i)) else { return Err(KernelError::Failed("mesh without parameters".into())) };
            let p = pos.get(v.pos).copied().unwrap_or(mt::Point3::new(0.0, 0.0, 0.0));
            c[k] = ((uv.x, uv.y), p);
        }
        base.push(c);
    }
    // One division for every triangle, so shared sides divide alike.
    let longest = base.iter().flat_map(|c| (0..3).map(move |k| c[k].1.distance(c[(k + 1) % 3].1))).fold(0.0, f64::max);
    let n = ((longest / step).ceil() as usize).clamp(1, 48);
    let mut tris: Vec<[(usize, (f64, f64)); 3]> = Vec::new();
    for c in base {
        let at = |i: usize, j: usize| {
            let (a, b, cc) = (c[0].0, c[1].0, c[2].0);
            let (s, t) = (i as f64 / n as f64, j as f64 / n as f64);
            (a.0 + (b.0 - a.0) * s + (cc.0 - a.0) * t, a.1 + (b.1 - a.1) * s + (cc.1 - a.1) * t)
        };
        for i in 0..n {
            for j in 0..n - i {
                let (p0, p1, p2) = (at(i, j), at(i + 1, j), at(i, j + 1));
                tris.push([(corner(p0), p0), (corner(p1), p1), (corner(p2), p2)]);
                if i + j + 1 < n {
                    let p3 = at(i + 1, j + 1);
                    tris.push([(corner(p1), p1), (corner(p3), p3), (corner(p2), p2)]);
                }
            }
        }
        if tris.len() > 4_000_000 {
            return Err(KernelError::Failed("the face is too finely divided to trace".into()));
        }
    }
    let gv: Vec<f64> = corner_uv.iter().map(|uv| g.eval(from_p3(surface.subs(uv.0, uv.1)))).collect();
    if gv.iter().any(|x| !x.is_finite()) {
        return Err(KernelError::Failed("the splitting tool cannot be evaluated on the face".into()));
    }
    let side = |i: usize| gv.get(i).copied().unwrap_or(0.0) >= 0.0;
    // Mesh edges crossed by the zero set, and which triangles link them.
    let key = |a: usize, b: usize| (a.min(b), a.max(b));
    let mut node_uv: HashMap<(usize, usize), ((f64, f64), (f64, f64))> = HashMap::new();
    let mut links: HashMap<(usize, usize), Vec<(usize, usize)>> = HashMap::new();
    let mut edge_tris: HashMap<(usize, usize), usize> = HashMap::new();
    for t in &tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *edge_tris.entry(key(a.0, b.0)).or_default() += 1;
        }
        let crossed: Vec<(usize, usize)> = (0..3)
            .filter_map(|k| {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                (side(a.0) != side(b.0)).then(|| {
                    let kk = key(a.0, b.0);
                    let (ua, ub) = if a.0 < b.0 { (a.1, b.1) } else { (b.1, a.1) };
                    node_uv.entry(kk).or_insert((ua, ub));
                    kk
                })
            })
            .collect();
        if let [x, y] = crossed[..] {
            links.entry(x).or_default().push(y);
            links.entry(y).or_default().push(x);
        }
    }
    // Exact point on a crossed mesh edge: bisection along its parameter segment.
    let exact = |k: (usize, usize)| -> Option<(mt::Point3, (f64, f64))> {
        let (ua, ub) = *node_uv.get(&k)?;
        let f = |s: f64| {
            let uv = (ua.0 + (ub.0 - ua.0) * s, ua.1 + (ub.1 - ua.1) * s);
            (g.eval(from_p3(surface.subs(uv.0, uv.1))), uv)
        };
        let (mut lo, mut hi) = (0.0, 1.0);
        let s0 = f(0.0).0 >= 0.0;
        for _ in 0..48 {
            let mid = 0.5 * (lo + hi);
            if (f(mid).0 >= 0.0) == s0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let (val, uv) = f(0.5 * (lo + hi));
        // A side whose ends were on the same side after all (a corner exactly on the tool).
        if !(val.abs() <= size * 1e-7) {
            return None;
        }
        Some((surface.subs(uv.0, uv.1), uv))
    };
    // Walk the links into chains.
    let mut seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut chains = Vec::new();
    let mut nodes: Vec<(usize, usize)> = links.keys().copied().collect();
    nodes.sort();
    // Open chains start at mesh boundary edges (one triangle).
    nodes.sort_by_key(|k| edge_tris.get(k).copied().unwrap_or(0) != 1);
    for start in nodes {
        if seen.contains(&start) {
            continue;
        }
        let mut path = vec![start];
        seen.insert(start);
        let mut cur = start;
        let closed;
        loop {
            let next = links.get(&cur).into_iter().flatten().find(|n| !seen.contains(*n)).copied();
            match next {
                Some(n) => {
                    seen.insert(n);
                    path.push(n);
                    cur = n;
                }
                None => {
                    closed = path.len() > 2 && links.get(&cur).is_some_and(|l| l.contains(&start));
                    break;
                }
            }
            if path.len() > 1_000_000 {
                return Err(KernelError::Failed("split line too long".into()));
            }
        }
        // Points about a step apart (the ends kept).
        let all: Vec<_> = path.iter().filter_map(|k| exact(*k)).collect();
        let mut pts: Vec<(mt::Point3, (f64, f64))> = Vec::new();
        for (i, p) in all.iter().enumerate() {
            let keep = i == 0 || i + 1 == all.len() || pts.last().is_none_or(|l| l.0.distance(p.0) >= step * 0.5);
            if keep {
                pts.push(*p);
            }
        }
        if pts.len() >= 2 {
            chains.push(Chain { pts, closed });
        }
    }
    Ok(chains)
}

/// A plane across a flat face (a plane, or a B-spline surface that is one) with straight edges,
/// exactly: the line the two planes meet in, inside the face between pairs of its crossings with
/// the boundary, taken in order along the line. Which side a corner is on is decided once, at
/// the corner, so a plane through a corner crosses the boundary there once or not at all, as it
/// should. `None` (traced on a mesh instead) for any other face or tool, and where an edge lies
/// in the plane.
fn trace_flat(face: &mt::Face, g: &Field, size: f64) -> Option<Vec<Chain>> {
    let Field::Plane { o, n } = g else { return None };
    let surface = face.surface();
    let Field::Plane { n: m, .. } = surface_field(&surface) else { return None };
    let along = m.cross(*n).normalized()?;
    let tol = size * 1e-7 + 1e-9;
    let mut hits: Vec<mt::Point3> = Vec::new();
    for e in face.boundaries().iter().flat_map(|w| w.edge_iter()) {
        let (a, b) = (from_p3(e.front().point()), from_p3(e.back().point()));
        let (ga, gb) = ((a - *o).dot(*n), (b - *o).dot(*n));
        // Straight: a line, or a curve (a B-spline of degree one) whose points all lie on its
        // chord; such an edge crosses the plane once at most.
        let straight = matches!(e.curve(), mt::Curve::Line(_)) || {
            let c = e.curve();
            let (t0, t1) = c.range_tuple();
            (1..8).all(|k| from_p3(c.subs(t0 + (t1 - t0) * k as f64 / 8.0)).dist_to_segment(a, b) <= tol)
        };
        if !straight || (ga.abs() <= tol && gb.abs() <= tol) || !ga.is_finite() || !gb.is_finite() {
            return None;
        }
        if (ga >= 0.0) != (gb >= 0.0) {
            hits.push(p3(a + (b - a) * (ga / (ga - gb)).clamp(0.0, 1.0)));
        }
    }
    if !hits.len().is_multiple_of(2) {
        return None;
    }
    hits.sort_by(|a, b| from_p3(*a).dot(along).total_cmp(&from_p3(*b).dot(along)));
    let uv = |p: mt::Point3| surface.search_nearest_parameter(p, None, 50);
    let mut chains = Vec::new();
    for pair in hits.chunks(2) {
        let [a, b] = pair else { return None };
        // A corner the plane only touches: no line.
        if a.distance(*b) <= tol {
            continue;
        }
        chains.push(Chain { pts: vec![(*a, uv(*a)?), (*b, uv(*b)?)], closed: false });
    }
    Some(chains)
}

/// Where the zero set crosses a boundary edge of the face (exact, by bisection on the curve).
fn boundary_hits(face: &mt::Face, g: &Field) -> Vec<Hit> {
    let mut out = Vec::new();
    for w in face.absolute_boundaries() {
        for e in w.edge_iter() {
            let c = e.curve();
            let (t0, t1) = c.range_tuple();
            let n = 64;
            let val = |t: f64| g.eval(from_p3(c.subs(t)));
            let mut prev = (t0, val(t0));
            for k in 1..=n {
                let t = t0 + (t1 - t0) * k as f64 / n as f64;
                let cur = (t, val(t));
                if (prev.1 >= 0.0) != (cur.1 >= 0.0) {
                    let (mut lo, mut hi) = (prev.0, cur.0);
                    let s0 = prev.1 >= 0.0;
                    for _ in 0..60 {
                        let mid = 0.5 * (lo + hi);
                        if (val(mid) >= 0.0) == s0 {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    let t = 0.5 * (lo + hi);
                    out.push(Hit { edge: e.clone(), t, p: c.subs(t) });
                }
                prev = cur;
            }
        }
    }
    out
}

/// The exact boundary crossings each open chain runs between (`None` for closed chains and
/// for chains whose ends have no crossing of their own).
///
/// A chain ends where the zero set crosses the boundary of the face's mesh, whose sides are
/// chords up to `mesh_tol` inside a curved boundary: the true crossing lies further along the
/// zero set, by the chord's sag over the sine of the angle the two meet at. A fixed snap
/// distance below the sag drops splits depending on where the mesh's vertices fall (a wall
/// crossing a disc's rim 0.06 mm past the last chord on a 50 mm part). Ends are matched within
/// a few sags (crossings at 15° or steeper), one crossing to one end, nearest pairs first, so
/// a wider reach never lets two ends share a crossing.
fn chain_ends(chains: &[Chain], hits: &[Hit], mesh_tol: f64) -> Vec<Option<(Hit, Hit)>> {
    let reach = 4.0 * mesh_tol;
    // (distance, end slot: chain * 2 + 0 for the first point, 1 for the last; hit index)
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (c, ch) in chains.iter().enumerate() {
        if ch.closed {
            continue;
        }
        for (k, end) in [ch.pts.first(), ch.pts.last()].into_iter().enumerate() {
            let Some(&(p, _)) = end else { continue };
            for (h, hit) in hits.iter().enumerate() {
                let d = hit.p.distance(p);
                if d <= reach {
                    pairs.push((d, c * 2 + k, h));
                }
            }
        }
    }
    pairs.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut slot: Vec<Option<usize>> = vec![None; chains.len() * 2];
    let mut taken = vec![false; hits.len()];
    for (_, end, h) in pairs {
        if slot.get(end).is_some_and(Option::is_none) && taken.get(h).is_some_and(|t| !*t) {
            if let Some(s) = slot.get_mut(end) {
                *s = Some(h);
            }
            if let Some(t) = taken.get_mut(h) {
                *t = true;
            }
        }
    }
    (0..chains.len())
        .map(|c| match (slot.get(c * 2).copied().flatten(), slot.get(c * 2 + 1).copied().flatten()) {
            (Some(a), Some(b)) => Some((hits.get(a)?.clone(), hits.get(b)?.clone())),
            _ => None,
        })
        .collect()
}

/// A curve through points: a line when they are collinear, else a cubic through every point
/// (Catmull-Rom tangents; `closed` when the last point repeats the first).
pub(crate) fn fit(pts: &[mt::Point3], scale: f64) -> Result<mt::Curve> {
    let (Some(a), Some(b)) = (pts.first(), pts.last()) else { return Err(KernelError::Failed("empty split line".into())) };
    let d = b - a;
    let len = d.magnitude();
    let straight = len > 0.0 && pts.iter().all(|p| (p - a).cross(d).magnitude() / len <= scale * 1e-9);
    if straight {
        return Ok(mt::Curve::Line(mt::Line(*a, *b)));
    }
    let m = pts.len().saturating_sub(1);
    if m < 1 {
        return Err(KernelError::Failed("split line too short".into()));
    }
    let closed = a.distance(*b) <= scale * 1e-9 && m >= 3;
    let at = |i: isize| -> mt::Point3 {
        let i = if closed { i.rem_euclid(m as isize) as usize } else { i.clamp(0, m as isize) as usize };
        pts.get(i).copied().unwrap_or(*a)
    };
    let mut ctrl = vec![*a];
    let mut knots = vec![0.0; 4];
    for i in 0..m as isize {
        let (p0, p1) = (at(i), at(i + 1));
        let t0 = (at(i + 1) - at(i - 1)) / 6.0;
        let t1 = (at(i + 2) - at(i)) / 6.0;
        ctrl.extend([p0 + t0, p1 - t1, p1]);
        let k = (i + 1) as f64;
        knots.extend(if i + 1 == m as isize { [k; 4].to_vec() } else { [k; 3].to_vec() });
    }
    let bsp = mt::BSplineCurve::try_new(mt::KnotVec::from(knots), ctrl).map_err(|e| KernelError::Failed(format!("split line: {e}")))?;
    Ok(mt::Curve::BSplineCurve(bsp))
}

/// The curve of a split line: an exact circular arc when a cylinder meets a plane square to its
/// axis (a sketch arc splitting a planar face), else [`fit`].
fn fit_on(g: &Field, face: &mt::Face, pts: &[mt::Point3], scale: f64) -> Result<mt::Curve> {
    if let (Field::Cylinder { axis, .. }, mt::Surface::Plane(pl)) = (g, face.surface())
        && let (Some(a), Some(b), Some(m)) = (pts.first(), pts.last(), pts.get(pts.len() / 2))
        && a.distance(*b) > scale * 1e-9
    {
        let n = pl.normal();
        if n.cross(crate::body::v3(*axis)).magnitude() < 1e-9 {
            let (va, vb) = (mt::Vertex::new(*a), mt::Vertex::new(*b));
            let e = guard("split arc", || Ok(mt::builder::circle_arc(&va, &vb, *m)))?;
            return Ok(e.curve());
        }
    }
    fit(pts, scale)
}

/// Cut an edge at a parameter (none when the point is already a vertex): the vertex and the
/// pieces in the edge's absolute direction.
fn cut_edge(e: &mt::Edge, t: f64, p: mt::Point3, tol: f64) -> Result<(mt::Vertex, Option<(mt::Edge, mt::Edge)>)> {
    let abs = if e.orientation() { e.clone() } else { e.inverse() };
    if abs.front().point().distance2(p) <= tol * tol {
        return Ok((abs.front().clone(), None));
    }
    if abs.back().point().distance2(p) <= tol * tol {
        return Ok((abs.back().clone(), None));
    }
    let v = mt::Vertex::new(p);
    // `t` is on the edge as it was: when an earlier crossing already cut it (a split line
    // crossing it twice), the parameter is found again on the piece.
    let cut = abs.cut_with_parameter(&v, t).or_else(|| {
        use mt::SearchNearestParameter;
        let t = abs.curve().search_nearest_parameter(p, None, 100)?;
        abs.cut_with_parameter(&v, t)
    });
    let (a, b) = cut.ok_or_else(|| KernelError::Failed("cannot cut an edge at the split line".into()))?;
    Ok((v, Some((a, b))))
}

/// Replace cut edges (absolute edge → its two pieces) in a face's loops.
fn recut(face: &mt::Face, cuts: &[(mt::Edge, (mt::Edge, mt::Edge))]) -> mt::Face {
    let touches = face.absolute_boundaries().iter().flat_map(|w| w.edge_iter()).any(|e| cuts.iter().any(|(c, _)| c.id() == e.id()));
    if !touches {
        return face.clone();
    }
    let wires: Vec<mt::Wire> = face
        .absolute_boundaries()
        .iter()
        .map(|w| {
            let mut out: Vec<mt::Edge> = Vec::new();
            for e in w.edge_iter() {
                expand(e, cuts, &mut out, 0);
            }
            out.into()
        })
        .collect();
    let mut f = mt::Face::new_unchecked(wires, face.surface());
    if !face.orientation() {
        f.invert();
    }
    f
}

/// `e` as the pieces it was cut into, in its direction; a piece cut again (a line crossing an
/// edge twice) as its own pieces.
fn expand(e: &mt::Edge, cuts: &[(mt::Edge, (mt::Edge, mt::Edge))], out: &mut Vec<mt::Edge>, depth: usize) {
    match cuts.iter().find(|(c, _)| c.id() == e.id()).filter(|_| depth < 64) {
        Some((c, (a, b))) if c.orientation() == e.orientation() => {
            expand(a, cuts, out, depth + 1);
            expand(b, cuts, out, depth + 1);
        }
        Some((_, (a, b))) => {
            expand(&b.inverse(), cuts, out, depth + 1);
            expand(&a.inverse(), cuts, out, depth + 1);
        }
        None => out.push(e.clone()),
    }
}

/// The parameter of the point of `c` nearest `q`, searched from the nearest of points along it (a
/// long arc's search from its middle can settle on the far side).
fn nearest(c: &mt::Curve, q: mt::Point3) -> Option<f64> {
    let (t0, t1) = c.range_tuple();
    let hint = (0..=16).map(|k| t0 + (t1 - t0) * k as f64 / 16.0).min_by(|x, y| c.subs(*x).distance2(q).total_cmp(&c.subs(*y).distance2(q)));
    c.search_nearest_parameter(q, hint, 100)
}

/// `body` with its edges cut at the given points where they pass through them (not at their
/// ends): the faces either side keep the same region, bounded by the pieces. Used to give two
/// faces that cover the same region the same vertices before they are glued (#674).
pub(crate) fn split_edges_at(body: &Body, points: &[Vec3]) -> Result<Body> {
    let size = body.size();
    let tol = size * 1e-7 + 1e-9;
    guard("split edges", || {
        let mut cuts: Vec<(mt::Edge, (mt::Edge, mt::Edge))> = Vec::new();
        let mut rec = Rebuild::default();
        // Each current piece and the body edge it is a piece of.
        let mut current: Vec<(mt::Edge, mt::Edge)> = body
            .solid
            .edge_iter()
            .map(|e| {
                let abs = if e.orientation() { e.clone() } else { e.inverse() };
                (abs.clone(), abs)
            })
            .collect();
        for p in points {
            let q = p3(*p);
            let hit = current.iter().position(|(e, _)| {
                if e.front().point().distance(q) <= tol * 10.0 || e.back().point().distance(q) <= tol * 10.0 {
                    return false;
                }
                let c = e.curve();
                let (t0, t1) = c.range_tuple();
                nearest(&c, q).is_some_and(|t| t > t0.min(t1) && t < t0.max(t1) && c.subs(t).distance(q) <= tol * 10.0)
            });
            let Some(k) = hit else { continue };
            let (e, root) = current.swap_remove(k);
            let Some(t) = nearest(&e.curve(), q) else { continue };
            let (_, cut) = cut_edge(&e, t, q, tol)?;
            if let Some((a, b)) = cut {
                rec.piece(&a, &root);
                rec.piece(&b, &root);
                current.push((a.clone(), root.clone()));
                current.push((b.clone(), root.clone()));
                cuts.push((e, (a, b)));
            } else {
                current.push((e, root));
            }
        }
        if cuts.is_empty() {
            return Ok(body.clone());
        }
        let shells: Vec<mt::Shell> = body
            .solid
            .boundaries()
            .iter()
            .map(|sh| {
                sh.face_iter()
                    .map(|f| {
                        let nf = recut(f, &cuts);
                        rec.whole(&nf, f);
                        nf
                    })
                    .collect::<Vec<_>>()
                    .into()
            })
            .collect();
        let solid = mt::Solid::new_unchecked(shells);
        let split = Body {
            solid: std::sync::Arc::new(solid),
            size: 0.0,
            volume: Some(std::sync::Arc::new(std::sync::Mutex::new(None))),
            coarse_missing: None,
            mesh: None,
            color: body.color,
            paint: None,
            splits: body.splits.clone(),
            face_sources: None,
            history: None,
            display_of: None,
            regularization_protected: true,
        };
        Ok(crate::record::rebuilt_body(split, &[&body.solid], &rec))
    })
}

/// A face rebuilt from loops (absolute) on the same surface and orientation as `like`.
fn face_like(like: &mt::Face, wires: Vec<mt::Wire>) -> mt::Face {
    let mut f = mt::Face::new_unchecked(wires, like.surface());
    if !like.orientation() {
        f.invert();
    }
    f
}

/// Polygon of a loop in the surface's parameters (for inside tests).
fn loop_uv(s: &mt::Surface, w: &mt::Wire) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut hint = None;
    for e in w.edge_iter() {
        let c = e.oriented_curve();
        let (t0, t1) = c.range_tuple();
        for k in 0..8 {
            let p = c.subs(t0 + (t1 - t0) * k as f64 / 8.0);
            if let Some(uv) = s.search_nearest_parameter(p, hint, 50) {
                hint = Some(uv);
                out.push(uv);
            }
        }
    }
    out
}

fn inside(poly: &[(f64, f64)], q: (f64, f64)) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
        if (a.1 > q.1) != (b.1 > q.1) && q.0 < a.0 + (b.0 - a.0) * (q.1 - a.1) / (b.1 - a.1) {
            c = !c;
        }
    }
    c
}

/// Split one face (index `fi` in `faces`) by one field; `faces` is updated (the face replaced
/// by its pieces, boundary edges cut in every face).
/// Returns how many lines split it and how many faces now stand in its place (at `fi`). The
/// pieces of cut edges are recorded in `rec`.
fn split_one(faces: &mut Vec<mt::Face>, fi: usize, g: &Field, size: f64, new_edges: &mut Vec<mt::Edge>, rec: &mut Rebuild) -> Result<(usize, usize)> {
    let Some(face) = faces.get(fi).cloned() else { return Ok((0, 1)) };
    let chains = trace(&face, g, size)?;
    if chains.is_empty() {
        return Ok((0, 1));
    }
    let tol = size * 1e-7 + 1e-9;
    let hits = boundary_hits(&face, g);
    let ends = chain_ends(&chains, &hits, trace_tol(&face, size));
    // Pieces of the original face as they are split.
    let mut pieces: Vec<mt::Face> = vec![face.clone()];
    let mut all_cuts: Vec<(mt::Edge, (mt::Edge, mt::Edge))> = Vec::new();
    let mut made = 0;
    for (ch, end) in chains.into_iter().zip(ends) {
        if ch.closed {
            // A closed line inside the face: a new face inside it and a hole in the piece that holds it.
            let mut pts: Vec<mt::Point3> = ch.pts.iter().map(|x| x.0).collect();
            if let Some(first) = pts.first().copied() {
                pts.push(first);
            }
            let curve = fit(&pts, size)?;
            let Some(&(p0, uv0)) = ch.pts.first() else { continue };
            let v = mt::Vertex::new(p0);
            let e = mt::Edge::new_unchecked(&v, &v, curve);
            new_edges.push(e.clone());
            let uv: Vec<(f64, f64)> = ch.pts.iter().map(|x| x.1).collect();
            let area: f64 = (0..uv.len())
                .map(|i| {
                    let (a, b) = (uv[i], uv[(i + 1) % uv.len()]);
                    a.0 * b.1 - b.0 * a.1
                })
                .sum();
            let s = face.surface();
            let Some(pi) = pieces.iter().position(|f| {
                let ws = f.absolute_boundaries();
                ws.first().is_some_and(|o| inside(&loop_uv(&s, o), uv0)) && !ws.iter().skip(1).any(|h| inside(&loop_uv(&s, h), uv0))
            }) else {
                continue;
            };
            let Some(host) = pieces.get(pi).cloned() else { continue };
            // The new loop counter-clockwise in parameters (an outer loop); its reverse a hole.
            let ccw: mt::Wire = if area > 0.0 { vec![e.clone()].into() } else { vec![e.inverse()].into() };
            let hole: mt::Wire = ccw.inverse();
            let ws = host.absolute_boundaries().clone();
            let mut outer_ws = vec![];
            let mut inner_ws = vec![ccw.clone()];
            for (k, w) in ws.iter().enumerate() {
                let q = loop_uv(&s, w).first().copied();
                if k > 0 && q.is_some_and(|q| inside(&uv, q)) {
                    inner_ws.push(w.clone());
                } else {
                    outer_ws.push(w.clone());
                }
            }
            outer_ws.push(hole);
            let a = face_like(&host, outer_ws);
            let b = face_like(&host, inner_ws);
            pieces.splice(pi..=pi, [a, b]);
            made += 1;
            continue;
        }
        // An open line: it must end on the boundary at two crossings.
        let Some((ha, hb)) = end else { continue };
        // Cut the boundary edges (an edge may already have been cut by an earlier line).
        let vert = |h: &Hit, cuts: &mut Vec<(mt::Edge, (mt::Edge, mt::Edge))>| -> Result<mt::Vertex> {
            // The current edge holding the hit: the original, or a piece of it.
            let mut e = h.edge.clone();
            for (c, (x, y)) in cuts.iter() {
                if c.id() == e.id() || c.id() == e.inverse().id() {
                    // The piece the hit lies on (a cut line's pieces are parametrised afresh,
                    // so by where the point is, not by the original parameter).
                    let off = |pc: &mt::Edge| {
                        use mt::SearchNearestParameter;
                        let cc = pc.curve();
                        let (t0, t1) = cc.range_tuple();
                        let t = cc.search_nearest_parameter(h.p, None, 100).unwrap_or(h.t).clamp(t0.min(t1), t0.max(t1));
                        cc.subs(t).distance(h.p)
                    };
                    e = if off(x) <= off(y) { x.clone() } else { y.clone() };
                }
            }
            let (v, cut) = cut_edge(&e, h.t, h.p, tol)?;
            if let Some(pair) = cut {
                let abs = if e.orientation() { e.clone() } else { e.inverse() };
                cuts.push((abs, pair));
            }
            Ok(v)
        };
        let va = vert(&ha, &mut all_cuts)?;
        let vb = vert(&hb, &mut all_cuts)?;
        if va.id() == vb.id() {
            continue;
        }
        pieces = pieces.iter().map(|f| recut(f, &all_cuts)).collect();
        let mut pts: Vec<mt::Point3> = ch.pts.iter().map(|x| x.0).collect();
        if let Some(f) = pts.first_mut() {
            *f = va.point();
        }
        if let Some(l) = pts.last_mut() {
            *l = vb.point();
        }
        // A line from one loop of a piece to another (into a hole): the two loops become one,
        // joined by the line run both ways; a later line divides it.
        let in_loop = |w: &mt::Wire, v: &mt::Vertex| w.edge_iter().any(|e| e.front().id() == v.id());
        if let Some(pi) = pieces.iter().position(|f| {
            let ws = f.absolute_boundaries();
            ws.iter().any(|w| in_loop(w, &va)) && ws.iter().any(|w| in_loop(w, &vb)) && !ws.iter().any(|w| in_loop(w, &va) && in_loop(w, &vb))
        }) && let Some(host) = pieces.get(pi).cloned()
        {
            let ws = host.absolute_boundaries().clone();
            let (Some(wa_i), Some(wb_i)) = (ws.iter().position(|w| in_loop(w, &va)), ws.iter().position(|w| in_loop(w, &vb))) else { continue };
            let from = |w: &mt::Wire, v: &mt::Vertex| -> Vec<mt::Edge> {
                let edges: Vec<mt::Edge> = w.edge_iter().cloned().collect();
                let k = edges.iter().position(|e| e.front().id() == v.id()).unwrap_or(0);
                edges.iter().skip(k).chain(edges.iter().take(k)).cloned().collect()
            };
            let (Some(w1), Some(w2)) = (ws.get(wa_i), ws.get(wb_i)) else { continue };
            let slit = mt::Edge::new_unchecked(&va, &vb, fit_on(g, &face, &pts, size)?);
            new_edges.push(slit.clone());
            let mut joined = from(w1, &va);
            joined.push(slit.clone());
            joined.extend(from(w2, &vb));
            joined.push(slit.inverse());
            // The outer loop first.
            let others = ws.iter().enumerate().filter(|(k, _)| *k != wa_i && *k != wb_i).map(|(_, w)| w.clone());
            let joined: mt::Wire = joined.into();
            let loops: Vec<mt::Wire> = if wa_i == 0 || wb_i == 0 {
                std::iter::once(joined).chain(others).collect()
            } else {
                others.chain(std::iter::once(joined)).collect()
            };
            pieces.splice(pi..=pi, [face_like(&host, loops)]);
            continue;
        }
        // The piece whose loop passes both vertices.
        let Some(pi) = pieces.iter().position(|f| {
            f.absolute_boundaries().iter().any(|w| {
                let ids: Vec<_> = w.edge_iter().map(|e| e.front().id()).collect();
                ids.contains(&va.id()) && ids.contains(&vb.id())
            })
        }) else {
            continue;
        };
        let Some(host) = pieces.get(pi).cloned() else { continue };
        let ws = host.absolute_boundaries().clone();
        let Some(wi) = ws.iter().position(|w| {
            let ids: Vec<_> = w.edge_iter().map(|e| e.front().id()).collect();
            ids.contains(&va.id()) && ids.contains(&vb.id())
        }) else {
            continue;
        };
        let Some(w) = ws.get(wi) else { continue };
        let edges: Vec<mt::Edge> = w.edge_iter().cloned().collect();
        let n = edges.len();
        let (Some(ia), Some(ib)) = (edges.iter().position(|e| e.front().id() == va.id()), edges.iter().position(|e| e.front().id() == vb.id()))
        else {
            continue;
        };
        let split = mt::Edge::new_unchecked(&va, &vb, fit_on(g, &face, &pts, size)?);
        new_edges.push(split.clone());
        let run = |from: usize, to: usize| -> Vec<mt::Edge> {
            let mut out = Vec::new();
            let mut i = from;
            while i != to {
                if let Some(e) = edges.get(i) {
                    out.push(e.clone());
                }
                i = (i + 1) % n;
            }
            out
        };
        // Piece A: a → b along the loop, back along the split. Piece B: b → a, then the split.
        let mut wa = run(ia, ib);
        wa.push(split.inverse());
        let mut wb = run(ib, ia);
        wb.push(split.clone());
        let (wa, wb): (mt::Wire, mt::Wire) = (wa.into(), wb.into());
        if !wa.is_closed() || !wb.is_closed() {
            return Err(KernelError::Failed("split loops do not close".into()));
        }
        // Other loops go to the piece on their side.
        let s = face.surface();
        let (pa_poly, pb_poly) = (loop_uv(&s, &wa), loop_uv(&s, &wb));
        let (mut la, mut lb) = (vec![wa], vec![wb]);
        for (k, other) in ws.iter().enumerate() {
            if k == wi {
                continue;
            }
            let q = loop_uv(&s, other).first().copied();
            match q {
                Some(q) if inside(&pa_poly, q) => la.push(other.clone()),
                Some(q) if inside(&pb_poly, q) => lb.push(other.clone()),
                _ => return Err(KernelError::Failed("a hole meets the split line".into())),
            }
        }
        let (a, b) = (face_like(&host, la), face_like(&host, lb));
        pieces.splice(pi..=pi, [a, b]);
        made += 1;
    }
    if made == 0 {
        return Ok((0, 1));
    }
    // In the order they were cut: a piece cut again is a piece of the edge it came from.
    for (c, (a, b)) in &all_cuts {
        rec.piece(a, c);
        rec.piece(b, c);
    }
    let count = pieces.len();
    // Neighbours take the cut edges.
    let rest: Vec<mt::Face> = faces.iter().enumerate().filter(|(k, _)| *k != fi).map(|(_, f)| recut(f, &all_cuts)).collect();
    let mut out = Vec::with_capacity(rest.len() + pieces.len());
    out.extend(rest.iter().take(fi).cloned());
    out.extend(pieces);
    out.extend(rest.iter().skip(fi).cloned());
    *faces = out;
    Ok((made, count))
}

/// The plane or circular cylinder a (B-spline) surface is, as a field with the surface's
/// normal; other surfaces as they are.
fn surface_field(s: &mt::Surface) -> Field {
    let (Some((u0, u1)), Some((v0, v1))) = s.try_range_tuple() else { return Field::Surface(s.clone()) };
    if !matches!(s, mt::Surface::Plane(_) | mt::Surface::BSplineSurface(_) | mt::Surface::NurbsSurface(_)) {
        return Field::Surface(s.clone());
    }
    let at = |a: f64, b: f64| from_p3(s.subs(u0 + (u1 - u0) * a, v0 + (v1 - v0) * b));
    let nrm = |a: f64, b: f64| {
        let n = s.normal(u0 + (u1 - u0) * a, v0 + (v1 - v0) * b);
        Vec3::new(n.x, n.y, n.z)
    };
    let grid: Vec<(f64, f64)> = (0..=6).flat_map(|i| (0..=6).map(move |j| (i as f64 / 6.0, j as f64 / 6.0))).collect();
    let size = grid.iter().map(|(a, b)| at(*a, *b).dist(at(0.0, 0.0))).fold(1e-9, f64::max);
    let tol = size * 1e-7;
    // A plane: one normal, every point on it.
    let n0 = nrm(0.5, 0.5);
    let o = at(0.5, 0.5);
    if grid.iter().all(|(a, b)| (at(*a, *b) - o).dot(n0).abs() <= tol) {
        return Field::Plane { o, n: n0 };
    }
    // A cylinder: straight along one parameter, a circle along the other.
    for along_v in [true, false] {
        let pt = |t: f64, w: f64| if along_v { at(t, w) } else { at(w, t) };
        let d = pt(0.0, 1.0) - pt(0.0, 0.0);
        let Some(axis) = d.normalized() else { continue };
        let straight = (0..=6).all(|k| {
            let t = k as f64 / 6.0;
            (0..=4).all(|m| (pt(t, m as f64 / 4.0) - pt(t, 0.0) - d * (m as f64 / 4.0)).len() <= tol)
        });
        if !straight {
            continue;
        }
        let prof: Vec<Vec3> = (0..=8).map(|k| pt(k as f64 / 8.0, 0.0)).collect();
        let (a, b, c) = (prof[0], prof[3], prof[6]);
        let (ab, ac) = (b - a, c - a);
        let nn = ab.cross(ac);
        let n2 = nn.dot(nn);
        if !(n2 > 1e-24) {
            continue;
        }
        let centre = a + (nn.cross(ab) * ac.dot(ac) + ac.cross(nn) * ab.dot(ab)) * (0.5 / n2);
        let r = (a - centre).len();
        if prof.iter().all(|p| ((*p - centre).len() - r).abs() <= tol) && nn.normalized().is_some_and(|nu| nu.cross(axis).len() < 1e-6) {
            let mid = pt(0.5, 0.5);
            let v = mid - centre;
            let radial = v - axis * v.dot(axis);
            let out = if nrm(0.5, 0.5).dot(radial) >= 0.0 { 1.0 } else { -1.0 };
            return Field::Cylinder { o: centre, axis, r, out };
        }
    }
    Field::Surface(s.clone())
}

/// A plane or circular cylinder behind a surface (for offsetting).
pub(crate) enum Analytic {
    Plane { n: Vec3 },
    Cylinder { o: Vec3, axis: Vec3, r: f64, out: f64 },
}

pub(crate) fn analytic_field(s: &mt::Surface) -> Option<Analytic> {
    match surface_field(s) {
        Field::Plane { n, .. } => Some(Analytic::Plane { n }),
        Field::Cylinder { o, axis, r, out } => Some(Analytic::Cylinder { o, axis, r, out }),
        _ => None,
    }
}

/// Which side of a tool a point is on.
pub(crate) fn side_of_point(tool: &SplitTool, p: Vec3) -> Result<f64> {
    Ok(ToolSide::of(tool)?.at(p))
}

/// Which side of a tool points are on, the tool's field built once for many points.
pub(crate) struct ToolSide(Field);

impl ToolSide {
    pub(crate) fn of(tool: &SplitTool) -> Result<ToolSide> {
        let mut f = fields(tool)?;
        match (f.pop(), f.is_empty()) {
            (Some(g), true) => Ok(ToolSide(g)),
            _ => Err(KernelError::Invalid("one tool at a time".into())),
        }
    }

    /// The side of `p`: its sign says which, zero on the tool (NaN where it can't be told).
    pub(crate) fn at(&self, p: Vec3) -> f64 {
        self.0.eval(p)
    }
}

/// Which side of a tool a face lies on (its field well inside it).
pub(crate) fn face_side(face: &mt::Face, tool: &SplitTool, size: f64) -> Result<f64> {
    let f = fields(tool)?;
    let [g] = f.as_slice() else { return Err(KernelError::Invalid("one tool at a time".into())) };
    side_of(face, g, size)
}

fn fields(tool: &SplitTool) -> Result<Vec<Field>> {
    Ok(match tool {
        SplitTool::Plane(p) => vec![Field::Plane { o: p.origin, n: p.normal() }],
        SplitTool::Face { body: other, face } => {
            other.require_brep("a split tool")?;
            let f = other.solid.face_iter().nth(*face).ok_or_else(|| KernelError::Invalid(format!("the tool body has no face {face}")))?;
            vec![surface_field(&f.oriented_surface())]
        }
        SplitTool::Curves { plane, curves, along } => {
            let n = plane.normal();
            // The sweep direction, turned to the plane's side (a direction along the plane sweeps
            // nothing).
            let along = match along {
                None => None,
                Some(d) => {
                    let d = d
                        .normalized()
                        .filter(|d| d.is_finite())
                        .ok_or_else(|| KernelError::Invalid("the projection direction must be a nonzero vector".into()))?;
                    if d.dot(n).abs() < 1e-6 {
                        return Err(KernelError::Invalid("the projection direction runs along the sketch plane".into()));
                    }
                    let d = if d.dot(n) < 0.0 { -d } else { d };
                    (d.cross(n).len() > 1e-12).then_some(d)
                }
            };
            let sweep = along.unwrap_or(n);
            let mut out = Vec::new();
            for c in curves {
                out.push(match c.as_slice() {
                    [Seg2::Line { a, b }] => {
                        let (wa, wb) = (plane.to_world(*a), plane.to_world(*b));
                        let n = sweep.cross(wb - wa).normalized().ok_or_else(|| KernelError::Invalid("a zero-length split line".into()))?;
                        Field::Plane { o: wa, n }
                    }
                    [Seg2::Arc { center, radius, .. }] if along.is_none() && radius.is_finite() && *radius > 0.0 => {
                        Field::Cylinder { o: plane.to_world(*center), axis: n, r: *radius, out: 1.0 }
                    }
                    segs => {
                        let mut pts: Vec<Vec2> = Vec::new();
                        for sg in segs {
                            for p in sg.polyline(1e-3) {
                                if pts.last().is_none_or(|q| (*q - p).len() > 1e-9) {
                                    pts.push(p);
                                }
                            }
                        }
                        if pts.len() < 2 || pts.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) || pts.len() > 1_000_000 {
                            return Err(KernelError::Invalid("split curves need finite segments".into()));
                        }
                        let span = pts.iter().fold(0.0f64, |m, p| m.max(p.len()));
                        let closed = pts.len() > 3 && pts.first().zip(pts.last()).is_some_and(|(a, b)| (*a - *b).len() <= span * 1e-9 + 1e-9);
                        Field::Curve { plane: *plane, pts, closed, along }
                    }
                });
            }
            out
        }
    })
}

/// The body's shells with the chosen faces split by each field in turn, the new edges, and
/// how many lines split something.
/// What an imprint made: the shells' faces, for each face the body face it came from, the new
/// edges, and how many lines split each chosen face.
pub(crate) struct Imprint {
    shells: Vec<Vec<mt::Face>>,
    origin: Vec<usize>,
    new_edges: Vec<mt::Edge>,
    made: HashMap<usize, usize>,
    /// The faces and edges made from the body's (each face over the same region as the body
    /// face it came from, or a piece of it; cut edges' pieces).
    rec: Rebuild,
}

fn imprint(body: &Body, faces: &[usize], fields: &[Field], size: f64) -> Result<Imprint> {
    let mut out = Imprint { shells: Vec::new(), origin: Vec::new(), new_edges: Vec::new(), made: HashMap::new(), rec: Rebuild::default() };
    let body_faces: Vec<&mt::Face> = body.solid.face_iter().collect();
    let mut base = 0;
    for sh in body.solid.boundaries() {
        let mut fs: Vec<mt::Face> = sh.face_iter().cloned().collect();
        // Which faces (by position) are to be split: the chosen ones and, later, their pieces;
        // and the body face each came from.
        let mut target: Vec<bool> = (0..fs.len()).map(|k| faces.contains(&(base + k))).collect();
        let mut origin: Vec<usize> = (0..fs.len()).map(|k| base + k).collect();
        base += fs.len();
        for g in fields {
            let mut k = 0;
            while k < fs.len() {
                if !target.get(k).copied().unwrap_or(false) {
                    k += 1;
                    continue;
                }
                let src = origin.get(k).copied().unwrap_or(0);
                let (m, pieces) = split_one(&mut fs, k, g, size, &mut out.new_edges, &mut out.rec)?;
                *out.made.entry(src).or_default() += m;
                // The pieces stand at k..k + pieces; positions after shift.
                target = target.iter().take(k).copied().chain(std::iter::repeat_n(true, pieces)).chain(target.iter().skip(k + 1).copied()).collect();
                origin = origin.iter().take(k).copied().chain(std::iter::repeat_n(src, pieces)).chain(origin.iter().skip(k + 1).copied()).collect();
                k += pieces;
            }
        }
        for (f, src) in fs.iter().zip(&origin) {
            if let Some(from) = body_faces.get(*src) {
                out.rec.whole(f, from);
            }
        }
        out.shells.push(fs);
        out.origin.extend(origin);
    }
    Ok(out)
}

/// Split the given faces (indices in `body`'s face order) where the tool meets them. The solid
/// is unchanged. A listed face the tool misses is an error naming it. The result's history is
/// over `[body]`: each piece of a split face is Split from it, every other face Kept.
pub fn split_faces(body: &Body, faces: &[usize], tool: &SplitTool) -> Result<Body> {
    split_faces_with_map(body, faces, tool).map(|(b, _)| b)
}

/// Like [`split_faces`], with the body face each face of the result came from (in face order).
pub fn split_faces_with_map(body: &Body, faces: &[usize], tool: &SplitTool) -> Result<(Body, Vec<usize>)> {
    let (b, imp) = split_any(body, faces, tool)?;
    // The new edges, remembered so that export keeps the pieces apart.
    let mut lines: Vec<Vec<Vec3>> = body.splits.as_ref().map(|l| l.to_vec()).unwrap_or_default();
    lines.extend(imp.new_edges.iter().filter_map(|e| {
        let c = e.curve();
        let (t0, t1) = c.range_tuple();
        let n = if matches!(c, mt::Curve::Line(_)) { 1 } else { 64 };
        let pts: Vec<Vec3> = (0..=n).map(|k| from_p3(c.subs(t0 + (t1 - t0) * k as f64 / n as f64))).collect();
        pts.iter().all(|p| p.is_finite()).then_some(pts)
    }));
    let b = b.with_split_lines(Some(std::sync::Arc::new(lines)));
    if let Some(missed) = faces.iter().find(|f| imp.made.get(f).copied().unwrap_or(0) == 0) {
        return Err(KernelError::Invalid(format!("the tool does not cross face {missed}")));
    }
    Ok((b, imp.origin))
}

/// Split where the tool meets the listed faces (missing some is fine; missing all is an error).
pub(crate) fn split_any(body: &Body, faces: &[usize], tool: &SplitTool) -> Result<(Body, Imprint)> {
    body.require_brep("split face")?;
    let size = body.size();
    let fields = fields(tool)?;
    let total = body.solid.face_iter().count();
    if faces.iter().any(|f| *f >= total) {
        return Err(KernelError::Invalid("no such face".into()));
    }
    guard("split face", || {
        let mut imp = imprint(body, faces, &fields, size)?;
        if imp.made.values().all(|m| *m == 0) {
            return Err(KernelError::Invalid("the tool does not cross the selected faces".into()));
        }
        let shells = std::mem::take(&mut imp.shells);
        let solid = mt::Solid::new_unchecked(shells.into_iter().map(mt::Shell::from).collect());
        let split = Body {
            solid: std::sync::Arc::new(solid),
            size: 0.0,
            volume: Some(std::sync::Arc::new(std::sync::Mutex::new(None))),
            coarse_missing: None,
            mesh: None,
            color: body.color,
            paint: None,
            splits: body.splits.clone(),
            face_sources: None,
            history: None,
            display_of: None,
            regularization_protected: true,
        };
        Ok((crate::record::rebuilt_body(split, &[&body.solid], &imp.rec), imp))
    })
}

/// The field at a point well inside a face (a mesh triangle's centre where the field is
/// largest; for a plane, the face's corner farthest from it).
fn side_of(face: &mt::Face, g: &Field, size: f64) -> Result<f64> {
    // A face the plane does not cross is on the side of whichever corner lies off it (the
    // farthest, against rounding); a face with every corner on the plane is meshed.
    if let Field::Plane { .. } = g {
        let far = face
            .boundaries()
            .iter()
            .flat_map(|w| w.vertex_iter())
            .map(|v| g.eval(from_p3(v.point())))
            .fold(0.0, |b: f64, x| if x.abs() > b.abs() { x } else { b });
        if far.abs() > size * 1e-6 {
            return Ok(far);
        }
    }
    let shell: mt::Shell = vec![face.clone()].into();
    let meshed = shell.robust_triangulation((size * 2e-3).max(1e-4));
    let pm = meshed.face_iter().next().and_then(|f| f.surface()).ok_or_else(|| KernelError::Failed("a face could not be meshed".into()))?;
    let pos = pm.positions();
    let mut best: f64 = 0.0;
    for t in pm.faces().triangle_iter() {
        let c = t.iter().filter_map(|v| pos.get(v.pos)).fold(Vec3::ZERO, |a, p| a + from_p3(*p) / 3.0);
        let x = g.eval(c);
        if x.is_finite() && x.abs() > best.abs() {
            best = x;
        }
    }
    Ok(best)
}

/// Loops from edges (each used once), chained end to start; edges are turned round as needed.
fn chain_loops(edges: &[mt::Edge]) -> Result<Vec<Vec<mt::Edge>>> {
    let mut pool: Vec<mt::Edge> = edges.to_vec();
    let mut out = Vec::new();
    while let Some(first) = pool.pop() {
        let start = first.front().id();
        let mut at = first.back().clone();
        let mut lp = vec![first];
        while at.id() != start {
            let Some(k) = pool.iter().position(|e| e.front().id() == at.id() || e.back().id() == at.id()) else {
                return Err(KernelError::Failed("the section through the body does not close".into()));
            };
            let e = pool.remove(k);
            let e = if e.front().id() == at.id() { e } else { e.inverse() };
            at = e.back().clone();
            lp.push(e);
            if lp.len() > 1_000_000 {
                return Err(KernelError::Failed("section loop too long".into()));
            }
        }
        out.push(lp);
    }
    Ok(out)
}

/// The regions of a body cut by a tool: for each face (in face order), the region it lies in,
/// and how many there are. Faces are in one region when they meet at an edge the tool does not
/// run along; an edge along the tool (a split line) parts them.
pub(crate) fn regions(split: &Body, tool: &SplitTool) -> Result<(Vec<usize>, usize)> {
    let fields = fields(tool)?;
    let tol = trace_tol_of(split.size());
    let faces: Vec<&mt::Face> = split.solid.face_iter().collect();
    let mut along_tool: HashMap<mt::EdgeID, bool> = HashMap::new();
    let mut by_edge: HashMap<mt::EdgeID, Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        for e in f.boundaries().iter().flat_map(|w| w.edge_iter()) {
            by_edge.entry(e.id()).or_default().push(i);
            along_tool.entry(e.id()).or_insert_with(|| {
                let c = e.curve();
                let (t0, t1) = c.range_tuple();
                let at: Vec<Vec3> = [0.25, 0.5, 0.75].iter().map(|s| from_p3(c.subs(t0 + (t1 - t0) * s))).collect();
                fields.iter().any(|g| at.iter().all(|p| g.eval(*p).abs() <= tol))
            });
        }
    }
    let mut region = vec![usize::MAX; faces.len()];
    let mut count = 0;
    for s in 0..faces.len() {
        if region.get(s) != Some(&usize::MAX) {
            continue;
        }
        if let Some(r) = region.get_mut(s) {
            *r = count;
        }
        let mut stack = vec![s];
        while let Some(i) = stack.pop() {
            let Some(f) = faces.get(i) else { continue };
            for e in f.boundaries().iter().flat_map(|w| w.edge_iter()) {
                if along_tool.get(&e.id()).copied().unwrap_or(false) {
                    continue;
                }
                for &j in by_edge.get(&e.id()).into_iter().flatten() {
                    if let Some(r) = region.get_mut(j)
                        && *r == usize::MAX
                    {
                        *r = count;
                        stack.push(j);
                    }
                }
            }
        }
        count += 1;
    }
    Ok((region, count))
}

/// How many fields a tool is (a sketch tool: one per curve).
pub(crate) fn field_count(tool: &SplitTool) -> Result<usize> {
    Ok(fields(tool)?.len())
}

/// Faces grouped by shared edges (in face order).
fn components(faces: Vec<mt::Face>) -> Vec<Vec<mt::Face>> {
    let n = faces.len();
    let mut by_edge: HashMap<mt::EdgeID, Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        for e in f.boundaries().iter().flat_map(|w| w.edge_iter()) {
            by_edge.entry(e.id()).or_default().push(i);
        }
    }
    let mut comp = vec![usize::MAX; n];
    let mut count = 0;
    for s in 0..n {
        if comp[s] != usize::MAX {
            continue;
        }
        comp[s] = count;
        let mut stack = vec![s];
        while let Some(i) = stack.pop() {
            let Some(f) = faces.get(i) else { continue };
            for e in f.boundaries().iter().flat_map(|w| w.edge_iter()) {
                for &j in by_edge.get(&e.id()).into_iter().flatten() {
                    if comp[j] == usize::MAX {
                        comp[j] = count;
                        stack.push(j);
                    }
                }
            }
        }
        count += 1;
    }
    let mut out: Vec<Vec<mt::Face>> = vec![Vec::new(); count];
    for (f, c) in faces.into_iter().zip(comp) {
        if let Some(v) = out.get_mut(c) {
            v.push(f);
        }
    }
    out
}

/// Split a body into the parts on either side of a tool: a plane, or another face's surface
/// (extended). Faces are divided where the tool meets them and every part is closed with the
/// piece of the tool's surface inside the body; parts come in face order, those on the tool's
/// back first. A body the tool misses comes back whole. Each part's history is over `[body]`:
/// its faces the body's (or pieces of them), the section new.
pub fn split_body(body: &Body, tool: &SplitTool) -> Result<Vec<Body>> {
    body.require_brep("split body")?;
    if body.solid.boundaries().len() != 1 {
        return Err(KernelError::Invalid("not supported yet: splitting a body with voids".into()));
    }
    let size = body.size();
    let fields = fields(tool)?;
    let [g] = fields.as_slice() else { return Err(KernelError::Invalid("a body is split by a plane or a face".into())) };
    // The tool surface, with the field's normal.
    // The tool's whole surface for the section (a sheet that is part of a plane or cylinder
    // is extended to all of it), with the field's normal.
    let surface = match g {
        Field::Plane { o, n } => {
            let x = (if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y }).cross(*n).normalized().unwrap_or(Vec3::X);
            let y = n.cross(x);
            let o = p3(*o);
            mt::Surface::Plane(mt::Plane::new(o, o + crate::body::v3(x), o + crate::body::v3(y)))
        }
        Field::Cylinder { o, axis, r, out } => {
            // A line along the axis at radius r, long enough for the body, revolved.
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for v in body.solid.vertex_iter() {
                let t = (from_p3(v.point()) - *o).dot(*axis);
                lo = lo.min(t);
                hi = hi.max(t);
            }
            let (lo, hi) = (lo - size, hi + size);
            let x = (if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y }).cross(*axis).normalized().unwrap_or(Vec3::X);
            let (a, b) = (*o + *axis * lo + x * *r, *o + *axis * hi + x * *r);
            let line = mt::Curve::Line(mt::Line(p3(a), p3(b)));
            crate::step_in::geom::revolved_oriented(line, p3(*o), crate::body::v3(*axis), 0.5, crate::body::v3(x * *out))
        }
        Field::Surface(s) => s.clone(),
        Field::Curve { .. } => return Err(KernelError::Invalid("a body is split by a plane or a face".into())),
    };
    let total = body.solid.face_iter().count();
    let all: Vec<usize> = (0..total).collect();
    guard("split body", || {
        let imp = imprint(body, &all, &fields, size)?;
        let new_edges = imp.new_edges;
        if imp.made.values().all(|m| *m == 0) || new_edges.is_empty() {
            return Ok(vec![crate::boolean_record::operand(body, 0, &crate::record::counts(&[body]))]);
        }
        let mut rec = imp.rec;
        let faces: Vec<mt::Face> = imp.shells.into_iter().flatten().collect();
        // The section: loops of the new edges on the tool surface, outer loops counter-clockwise
        // about its normal and holes inside them.
        let loops = chain_loops(&new_edges)?;
        let polys: Vec<Vec<(f64, f64)>> = loops.iter().map(|l| loop_uv(&surface, &l.clone().into())).collect();
        let signed = |p: &[(f64, f64)]| {
            (0..p.len())
                .map(|i| {
                    let (a, b) = (p[i], p[(i + 1) % p.len()]);
                    a.0 * b.1 - b.0 * a.1
                })
                .sum::<f64>()
        };
        let depth: Vec<usize> = (0..loops.len())
            .map(|i| {
                let q = polys.get(i).and_then(|p| p.first()).copied().unwrap_or((0.0, 0.0));
                (0..loops.len()).filter(|&j| j != i && polys.get(j).is_some_and(|p| inside(p, q))).count()
            })
            .collect();
        let wire = |i: usize, ccw: bool| -> mt::Wire {
            let w: mt::Wire = loops.get(i).cloned().unwrap_or_default().into();
            let is_ccw = polys.get(i).is_some_and(|p| signed(p) > 0.0);
            if is_ccw == ccw { w } else { w.inverse() }
        };
        let mut section = Vec::new();
        // On a surface of revolution, loops that run once round it (a cylinder through a box:
        // its circles at the top and bottom) bound bands: taken in order along the profile,
        // each pair is one face, the lower loop running back round and the upper forward.
        let mut wrapping = vec![false; loops.len()];
        if let mt::Surface::RevolutedCurve(rc) = &surface {
            let (o, axis) = (rc.entity().origin(), rc.entity().axis().normalize());
            let x0 = if axis.x.abs() < 0.9 { mt::Vector3::unit_x() } else { mt::Vector3::unit_y() };
            let ex = (x0 - axis * x0.dot(axis)).normalize();
            let ey = axis.cross(ex);
            let tau = std::f64::consts::TAU;
            let samples = |w: &mt::Wire| -> Vec<mt::Point3> {
                w.edge_iter()
                    .flat_map(|e| {
                        let c = e.oriented_curve();
                        let (t0, t1) = c.range_tuple();
                        (0..32).map(move |k| c.subs(t0 + (t1 - t0) * k as f64 / 32.0)).collect::<Vec<_>>()
                    })
                    .collect()
            };
            let mut bands: Vec<(f64, usize)> = Vec::new();
            for (i, l) in loops.iter().enumerate() {
                let pts = samples(&l.clone().into());
                let ang = |p: &mt::Point3| (p - o).dot(ey).atan2((p - o).dot(ex));
                let turn: f64 = (0..pts.len())
                    .map(|k| {
                        let d = ang(&pts[(k + 1) % pts.len()]) - ang(&pts[k]);
                        d - tau * (d / tau).round()
                    })
                    .sum();
                if (turn.abs() - tau).abs() < 0.1 {
                    let along = pts.iter().map(|p| (p - o).dot(axis)).sum::<f64>() / pts.len().max(1) as f64;
                    bands.push((along, i));
                    if let Some(w) = wrapping.get_mut(i) {
                        *w = true;
                    }
                }
            }
            bands.sort_by(|a, b| a.0.total_cmp(&b.0));
            // A loop runs so that the band (towards `toward` along the axis) is on its left about
            // the surface normal.
            let oriented = |i: usize, toward: f64| -> mt::Wire {
                let w: mt::Wire = loops.get(i).cloned().unwrap_or_default().into();
                let pts = samples(&w);
                let (Some(p), Some(q)) = (pts.first(), pts.get(1)) else { return w };
                let n = surface.search_nearest_parameter(*p, None, 50).map(|(u, v)| surface.normal(u, v)).unwrap_or(axis);
                let left = n.cross(q - p);
                if left.dot(axis) * toward >= 0.0 { w } else { w.inverse() }
            };
            for pair in bands.chunks(2) {
                let [(_, lo), (_, hi)] = pair else { return Err(KernelError::Failed("an unmatched band edge in the section".into())) };
                section.push(mt::Face::new_unchecked(vec![oriented(*lo, 1.0), oriented(*hi, -1.0)], surface.clone()));
            }
        }
        for (i, d) in depth.iter().enumerate() {
            if d % 2 != 0 || wrapping.get(i).copied().unwrap_or(false) {
                continue;
            }
            let mut ws = vec![wire(i, true)];
            for (j, dj) in depth.iter().enumerate() {
                let q = polys.get(j).and_then(|p| p.first()).copied().unwrap_or((0.0, 0.0));
                if *dj == d + 1 && polys.get(i).is_some_and(|p| inside(p, q)) {
                    ws.push(wire(j, false));
                }
            }
            section.push(mt::Face::new_unchecked(ws, surface.clone()));
        }
        // Each face goes to the side its inside is on.
        let (mut back, mut front) = (Vec::new(), Vec::new());
        for f in faces {
            let x = side_of(&f, g, size)?;
            if x.abs() <= size * 1e-9 {
                return Err(KernelError::Invalid("not supported yet: the tool lies along a face of the body".into()));
            }
            if x < 0.0 {
                back.push(f);
            } else {
                front.push(f);
            }
        }
        // The section is new: it comes from nothing of the body.
        for f in &section {
            rec.made(f, Vec::new());
        }
        // The section closes the back side facing the tool's normal, the front side facing away.
        back.extend(section.iter().cloned());
        front.extend(section.iter().map(|f| f.inverse()));
        let mut parts = Vec::new();
        for side in [back, front] {
            for comp in components(side) {
                let shell = mt::Shell::from(comp);
                if shell.shell_condition() != mt::ShellCondition::Closed {
                    return Err(KernelError::Failed("a part of the split does not close".into()));
                }
                parts.push(crate::record::rebuilt(mt::Solid::new_unchecked(vec![shell]), &[&body.solid], &rec)?.with_color(body.color));
            }
        }
        Ok(parts)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{box_solid, cylinder, measure};

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs().max(1e-9)
    }

    fn top(b: &Body, z: f64) -> usize {
        b.faces(0.01).unwrap().iter().position(|f| (f.centroid.z - z).abs() < 1e-6 && f.plane_normal.is_some()).unwrap()
    }

    #[test]
    fn plane_splits_a_box_top() {
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 20.0, 30.0)).unwrap();
        let m0 = measure(&b).unwrap();
        let pl = Plane::new(Vec3::new(4.0, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap();
        let s = split_faces(&b, &[top(&b, 30.0)], &SplitTool::Plane(pl)).unwrap();
        assert_eq!(s.face_count(), 7);
        let m = measure(&s).unwrap();
        assert!(rel(m.volume, m0.volume) < 1e-9 && rel(m.area, m0.area) < 1e-9);
        let mut tops: Vec<f64> = s.faces(0.01).unwrap().iter().filter(|f| (f.centroid.z - 30.0).abs() < 1e-6).map(|f| f.area).collect();
        tops.sort_by(f64::total_cmp);
        assert!((tops[0] - 80.0).abs() < 1e-6 && (tops[1] - 120.0).abs() < 1e-6, "{tops:?}");
        // The side faces the line ends on gained a vertex but kept their area.
        assert!(s.faces(0.01).unwrap().iter().all(|f| f.area > 0.0));
        // Later features work on the split body: a hole through the split top.
        let pin = cylinder(Vec3::new(5.0, 10.0, -1.0), Vec3::Z, 2.0, 40.0).unwrap();
        let holed = crate::boolean(&s, &pin, crate::BoolOp::Cut).unwrap().unwrap();
        let v = measure(&holed).unwrap().volume;
        assert!(rel(v, 6000.0 - std::f64::consts::PI * 4.0 * 30.0) < 1e-3, "{v}");
    }

    /// Faces after a STEP round trip.
    fn round_trip_faces(b: &Body) -> usize {
        let ex = crate::ExportBody { name: "B".into(), body: b, color: None };
        let text = crate::step_export_bodies(&[ex], &crate::StepHeader::default()).unwrap();
        let imp = crate::step_import(&text).unwrap();
        imp.bodies.iter().map(|x| x.body.face_count()).sum()
    }

    #[test]
    fn export_keeps_faces_split_on_purpose() {
        // A box top split by a plane, and a cylinder's side split by a plane across its axis.
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 20.0, 30.0)).unwrap();
        let pl = Plane::new(Vec3::new(4.0, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap();
        let s = split_faces(&b, &[top(&b, 30.0)], &SplitTool::Plane(pl)).unwrap();
        assert_eq!(s.face_count(), 7);
        assert_eq!(round_trip_faces(&s), 7);
        let c = cylinder(Vec3::ZERO, Vec3::Z, 10.0, 20.0).unwrap();
        let side = c.faces(0.01).unwrap().iter().position(|f| f.plane_normal.is_none()).unwrap();
        let mid = Plane::new(Vec3::new(0.0, 0.0, 5.0), Vec3::X, Vec3::Y).unwrap();
        let cs = split_faces(&c, &[side], &SplitTool::Plane(mid)).unwrap();
        let n = cs.face_count();
        assert!(n > c.face_count());
        // The pieces stay apart; the cylinder's incidental seam pieces still join as before.
        assert_eq!(round_trip_faces(&cs), round_trip_faces(&c) + 1);
        // Without the record the export joins the top's pieces again (as it does for pieces a
        // boolean left).
        assert_eq!(round_trip_faces(&s.clone().with_split_lines(None)), 6);
        // A split after a split keeps both lines.
        let pl2 = Plane::new(Vec3::new(0.0, 12.0, 0.0), Vec3::X, Vec3::Z).unwrap();
        let tops: Vec<usize> =
            s.faces(0.01).unwrap().iter().enumerate().filter(|(_, f)| (f.centroid.z - 30.0).abs() < 1e-6).map(|(i, _)| i).collect();
        let s2 = split_faces(&s, &tops, &SplitTool::Plane(pl2)).unwrap();
        assert_eq!(s2.face_count(), 9);
        assert_eq!(round_trip_faces(&s2), 9);
    }

    #[test]
    fn sketch_curve_splits_a_cylinder_side_and_a_circle_splits_a_top() {
        let c = cylinder(Vec3::ZERO, Vec3::Z, 10.0, 20.0).unwrap();
        let m0 = measure(&c).unwrap();
        let side = c.faces(0.01).unwrap().iter().position(|f| f.plane_normal.is_none()).unwrap();
        // A horizontal line seen from the front (XZ sketch) at z = 5, swept along y.
        let xz = Plane { y: Vec3::Z, ..Plane::XZ };
        let tool = SplitTool::Curves { plane: xz, curves: vec![vec![Seg2::Line { a: Vec2::new(-20.0, 5.0), b: Vec2::new(20.0, 5.0) }]], along: None };
        let s = split_faces(&c, &[side], &tool).unwrap();
        let m = measure(&s).unwrap();
        // (Measured on meshes: curved faces agree to about 1e-4.)
        assert!(rel(m.volume, m0.volume) < 2e-4 && rel(m.area, m0.area) < 2e-4, "{} {} {} {}", m.volume, m0.volume, m.area, m0.area);
        // The half side face splits at z = 5: a quarter and three quarters of its area.
        let mut sides: Vec<f64> = s.faces(0.01).unwrap().iter().filter(|f| f.plane_normal.is_none()).map(|f| f.area).collect();
        sides.sort_by(f64::total_cmp);
        let half = std::f64::consts::PI * 10.0 * 20.0;
        assert_eq!(sides.len(), 3);
        assert!(rel(sides[0], half / 4.0) < 1e-3 && rel(sides[1], half * 0.75) < 1e-3 && rel(sides[2], half) < 1e-3, "{sides:?}");
        assert!(s.face_count() > c.face_count());
        // A smaller cylinder's side splits the top in a ring and a disc.
        let pin = cylinder(Vec3::ZERO, Vec3::Z, 4.0, 40.0).unwrap();
        let pin_side = pin.faces(0.01).unwrap().iter().position(|f| f.plane_normal.is_none()).unwrap();
        let s = split_faces(&c, &[top(&c, 20.0)], &SplitTool::Face { body: &pin, face: pin_side }).unwrap();
        let m = measure(&s).unwrap();
        assert!(rel(m.volume, m0.volume) < 2e-4 && rel(m.area, m0.area) < 2e-4, "{} {}", m.area, m0.area);
        let mut tops: Vec<f64> = s.faces(0.01).unwrap().iter().filter(|f| (f.centroid.z - 20.0).abs() < 1e-6).map(|f| f.area).collect();
        tops.sort_by(f64::total_cmp);
        let pi = std::f64::consts::PI;
        // (Face areas come from meshes: within their sag, which depends on where seams lie.)
        assert!(rel(tops[0], pi * 16.0) < 2e-3 && rel(tops[1], pi * 84.0) < 2e-3, "{tops:?}");
    }

    /// A plane through a hole in a plate's top: the top splits in two, each piece holding
    /// half of the hole's rim.
    #[test]
    fn plane_through_a_hole_splits_the_face() {
        let plate = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
        let pin = cylinder(Vec3::new(20.0, 15.0, -1.0), Vec3::Z, 5.0, 12.0).unwrap();
        let b = crate::boolean(&plate, &pin, crate::BoolOp::Cut).unwrap().unwrap();
        let m0 = measure(&b).unwrap();
        let pl = Plane::new(Vec3::new(20.0, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap();
        let s = split_faces(&b, &[top(&b, 10.0)], &SplitTool::Plane(pl)).unwrap();
        let m = measure(&s).unwrap();
        assert!(rel(m.volume, m0.volume) < 1e-4 && rel(m.area, m0.area) < 1e-4);
        let tops: Vec<f64> = s.faces(0.01).unwrap().iter().filter(|f| (f.centroid.z - 10.0).abs() < 1e-6).map(|f| f.area).collect();
        let half = (1200.0 - std::f64::consts::PI * 25.0) / 2.0;
        assert_eq!(tops.len(), 2, "{tops:?}");
        assert!(tops.iter().all(|a| rel(*a, half) < 1e-3), "{tops:?}");
    }

    fn volumes(parts: &[Body]) -> Vec<f64> {
        let mut v: Vec<f64> = parts.iter().map(|b| measure(b).unwrap().volume).collect();
        v.sort_by(f64::total_cmp);
        v
    }

    #[test]
    fn split_body_by_plane_and_by_a_face() {
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 20.0, 30.0)).unwrap();
        let pl = Plane::new(Vec3::new(4.0, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap();
        let v = volumes(&split_body(&b, &SplitTool::Plane(pl)).unwrap());
        assert!(v.len() == 2 && rel(v[0], 2400.0) < 1e-9 && rel(v[1], 3600.0) < 1e-9, "{v:?}");
        // A plate with a hole split through the hole: two equal halves.
        let plate = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
        let pin = cylinder(Vec3::new(20.0, 15.0, -1.0), Vec3::Z, 5.0, 12.0).unwrap();
        let holed = crate::boolean(&plate, &pin, crate::BoolOp::Cut).unwrap().unwrap();
        let pl = Plane::new(Vec3::new(20.0, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap();
        let v = volumes(&split_body(&holed, &SplitTool::Plane(pl)).unwrap());
        let half = (12000.0 - std::f64::consts::PI * 250.0) / 2.0;
        assert!(v.len() == 2 && rel(v[0], half) < 1e-4 && rel(v[1], half) < 1e-4, "{v:?}");
        // A box split by a cylinder's side: the core and the rest.
        let tool = cylinder(Vec3::new(5.0, 10.0, -5.0), Vec3::Z, 4.0, 40.0).unwrap();
        let side = tool.faces(0.01).unwrap().iter().position(|f| f.plane_normal.is_none()).unwrap();
        let v = volumes(&split_body(&b, &SplitTool::Face { body: &tool, face: side }).unwrap());
        let core = std::f64::consts::PI * 16.0 * 30.0;
        assert!(v.len() == 2 && rel(v[0], core) < 1e-3 && rel(v[1], 6000.0 - core) < 1e-3, "{v:?}");
        // A plane that misses gives the body back.
        let far = Plane::new(Vec3::new(50.0, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap();
        assert_eq!(split_body(&b, &SplitTool::Plane(far)).unwrap().len(), 1);
    }

    /// Sketch arcs split exactly (a circular edge), a listed face the tool misses is named, and
    /// the map gives each new face its source.
    #[test]
    fn sketch_arc_splits_exactly_and_misses_are_named() {
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 20.0, 30.0)).unwrap();
        let t = top(&b, 30.0);
        let arc = Seg2::Arc { center: Vec2::new(0.0, 0.0), radius: 6.0, start: 0.0, sweep: std::f64::consts::FRAC_PI_2 };
        let plane = Plane { origin: Vec3::new(0.0, 0.0, 30.0), ..Plane::XY };
        let tool = SplitTool::Curves { plane, curves: vec![vec![arc]], along: None };
        let (s, map) = split_faces_with_map(&b, &[t], &tool).unwrap();
        assert_eq!(map.len(), s.face_count());
        assert_eq!(map.iter().filter(|o| **o == t).count(), 2);
        let mut tops: Vec<f64> = s.faces(0.001).unwrap().iter().filter(|f| (f.centroid.z - 30.0).abs() < 1e-6).map(|f| f.area).collect();
        tops.sort_by(f64::total_cmp);
        let quarter = std::f64::consts::PI * 36.0 / 4.0;
        // (Face areas come from meshes.)
        assert!(rel(tops[0], quarter) < 1e-3 && rel(tops[1], 200.0 - quarter) < 1e-3, "{tops:?}");
        assert!(s.solid.edge_iter().any(|e| matches!(e.curve(), mt::Curve::NurbsCurve(_))));
        // A listed face the tool misses is named: the plane z = 10 crosses the sides, not the top.
        let mid = Plane { origin: Vec3::new(0.0, 0.0, 10.0), ..Plane::XY };
        let side = b.faces(0.01).unwrap().iter().position(|f| (f.centroid.x - 10.0).abs() < 1e-6).unwrap();
        assert!(split_faces(&b, &[side], &SplitTool::Plane(mid)).is_ok());
        let e = split_faces(&b, &[side, t], &SplitTool::Plane(mid)).unwrap_err().to_string();
        assert!(e.contains(&format!("face {t}")), "{e}");
    }

    #[test]
    fn a_tool_that_misses_is_an_error() {
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 20.0, 30.0)).unwrap();
        let pl = Plane::new(Vec3::new(50.0, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap();
        assert!(split_faces(&b, &[0], &SplitTool::Plane(pl)).is_err());
        assert!(split_faces(&b, &[99], &SplitTool::Plane(Plane::XY)).is_err());
    }
}
