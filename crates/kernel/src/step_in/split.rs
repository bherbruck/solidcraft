//! Splitting band faces that wrap all the way around a closed, non-periodic parameter direction.
//!
//! A STEP face may run once around a closed surface and close its loop with a seam edge used
//! twice (a pipe segment of a torus, a revolved B-spline band). Truck meshes faces in parameter
//! space and only treats the rotation of a revolved surface as periodic, so such a face is cut
//! in two along the parameter line opposite its seam; the ring edges it crosses are cut in every
//! face that uses them.

use std::collections::HashMap;

use mt::{BoundedCurve, BoundedSurface, InnerSpace, Invertible, MetricSpace, ParametricCurve, ParametricSurface, SearchNearestParameter};
use truck_modeling as mt;

use super::geom::{Frame, R, conic_arc};

/// Samples per edge when looking for the split crossing.
const SAMPLES: usize = 48;

/// Which surface parameter wraps around.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dir {
    U,
    V,
}

fn uv_of(s: &mt::Surface, p: mt::Point3) -> Option<(f64, f64)> {
    s.search_nearest_parameter(p, None, 100).filter(|(u, v)| u.is_finite() && v.is_finite())
}

fn coord(d: Dir, uv: (f64, f64)) -> f64 {
    match d {
        Dir::U => uv.0,
        Dir::V => uv.1,
    }
}

/// The direction the split line runs along (the one that does not wrap).
fn along(d: Dir) -> Dir {
    match d {
        Dir::U => Dir::V,
        Dir::V => Dir::U,
    }
}

/// The closed, non-periodic direction of a surface with its range.
fn closed_dirs(s: &mt::Surface) -> Vec<(Dir, f64, f64)> {
    match s {
        mt::Surface::RevolutedCurve(p) => {
            let c = p.entity().entity_curve();
            let (t0, t1) = c.range_tuple();
            if c.front().distance(c.back()) < 1e-7 * (1.0 + c.front().to_homogeneous().truncate().magnitude()) {
                vec![(Dir::U, t0, t1)]
            } else {
                vec![]
            }
        }
        mt::Surface::BSplineSurface(_) | mt::Surface::NurbsSurface(_) => {
            let ((u0, u1), (v0, v1)) = match s {
                mt::Surface::BSplineSurface(b) => b.range_tuple(),
                mt::Surface::NurbsSurface(n) => n.range_tuple(),
                _ => return vec![],
            };
            let size = (0..=4)
                .flat_map(|i| (0..=4).map(move |j| (i, j)))
                .map(|(i, j)| s.subs(u0 + (u1 - u0) * i as f64 / 4.0, v0 + (v1 - v0) * j as f64 / 4.0).to_homogeneous().truncate().magnitude())
                .fold(1.0, f64::max);
            let tol = size * 1e-7;
            let mut out = Vec::new();
            if (0..=8).all(|k| {
                let v = v0 + (v1 - v0) * k as f64 / 8.0;
                s.subs(u0, v).distance(s.subs(u1, v)) < tol
            }) {
                out.push((Dir::U, u0, u1));
            }
            if (0..=8).all(|k| {
                let u = u0 + (u1 - u0) * k as f64 / 8.0;
                s.subs(u, v0).distance(s.subs(u, v1)) < tol
            }) {
                out.push((Dir::V, v0, v1));
            }
            out
        }
        mt::Surface::Plane(_) => vec![],
    }
}

fn samples(c: &mt::Curve, n: usize) -> Vec<(f64, mt::Point3)> {
    let (t0, t1) = c.range_tuple();
    (0..=n).map(|i| t0 + (t1 - t0) * i as f64 / n as f64).map(|t| (t, c.subs(t))).collect()
}

/// Where a ring chain crosses `w = mid`: (edge index in the chain, parameter on the edge's
/// absolute curve, point).
fn crossing(s: &mt::Surface, d: Dir, chain: &[mt::Edge], mid: f64, period: f64) -> R<(usize, f64, mt::Point3)> {
    for (k, e) in chain.iter().enumerate() {
        let c = e.curve();
        let pts = samples(&c, SAMPLES);
        let ws: Vec<Option<f64>> = pts.iter().map(|(_, p)| uv_of(s, *p).map(|uv| coord(d, uv))).collect();
        for i in 0..SAMPLES {
            let (Some(Some(wa)), Some(Some(wb)), Some((ta, _)), Some((tb, _))) = (ws.get(i), ws.get(i + 1), pts.get(i), pts.get(i + 1)) else {
                continue;
            };
            if (wb - wa).abs() > period * 0.5 || (wa - mid) * (wb - mid) > 0.0 {
                continue;
            }
            // Bisection on the curve parameter.
            let (mut lo, mut hi, mut flo) = (*ta, *tb, wa - mid);
            for _ in 0..60 {
                let m = 0.5 * (lo + hi);
                let Some(w) = uv_of(s, c.subs(m)).map(|uv| coord(d, uv)) else { break };
                let f = w - mid;
                if (f - flo).abs() > period * 0.5 {
                    break;
                }
                if f * flo <= 0.0 {
                    hi = m;
                } else {
                    lo = m;
                    flo = f;
                }
            }
            let t = 0.5 * (lo + hi);
            return Ok((k, t, c.subs(t)));
        }
    }
    Err("no crossing with the split line".into())
}

/// Iso-parameter edge curve `w = mid` from `q1` to `q2`. `tol` is the file's point-on-surface
/// tolerance: a ring edge is written to it and may run that far off the surface the split line
/// lies on, so the line is matched to the surface points below `q1` and `q2`, not to them.
fn iso_curve(s: &mt::Surface, d: Dir, mid: f64, q1: mt::Point3, q2: mt::Point3, seam: &[mt::Point3], tol: f64) -> R<mt::Curve> {
    match s {
        mt::Surface::RevolutedCurve(p) => {
            // A circle around the axis, turning opposite to the seam (which runs from the q2
            // side to the q1 side).
            let (o, a) = (p.entity().origin(), p.entity().axis());
            let az = |x: mt::Point3, f: &Frame| {
                let v = x - f.o;
                v.dot(f.y).atan2(v.dot(f.x))
            };
            let c = o + a * (q1 - o).dot(a);
            let r = (q1 - c).magnitude();
            if !(r > 1e-9) {
                return Err("split line on the axis".into());
            }
            let x = (q1 - c) / r;
            let f = Frame { o: c, x, y: a.cross(x), z: a };
            let mut seam_turn = 0.0;
            for w in seam.windows(2) {
                if let (Some(p0), Some(p1)) = (w.first(), w.get(1)) {
                    let d = az(*p1, &f) - az(*p0, &f);
                    seam_turn += (d + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                }
            }
            let sweep = -seam_turn;
            if sweep.abs() < 1e-9 {
                return Err("degenerate split line".into());
            }
            let f = if sweep > 0.0 { f } else { Frame { o: c, x, y: -a.cross(x), z: -a } };
            let arc = conic_arc(&f, r, r, 0.0, sweep.abs());
            if arc.back().distance(q2) > (1e-6 * (1.0 + r)).max(tol) {
                return Err("split line does not reach the other ring".into());
            }
            Ok(mt::Curve::NurbsCurve(arc))
        }
        mt::Surface::BSplineSurface(_) | mt::Surface::NurbsSurface(_) => {
            let hom: mt::BSplineSurface<mt::Vector4> = match s {
                mt::Surface::BSplineSurface(b) => mt::BSplineSurface::new(
                    b.knot_vecs().clone(),
                    b.control_points().iter().map(|row| row.iter().map(|p| p.to_homogeneous()).collect()).collect(),
                ),
                mt::Surface::NurbsSurface(n) => n.non_rationalized().clone(),
                _ => return Err("unsupported surface".into()),
            };
            let iso = mt::Curve::NurbsCurve(iso_bsp(&hom, mid, d == Dir::V)?);
            let (t0, t1) = iso.range_tuple();
            // Where the split line comes nearest `p`: the point on the surface below `p` (the
            // line runs on the surface, `p` only within the file's tolerance of it), looked for
            // from its own surface parameter and from a scan of the whole line.
            let near = |p: mt::Point3| {
                let foot = uv_of(s, p).filter(|uv| s.subs(uv.0, uv.1).distance(p) <= tol);
                let target = foot.map_or(p, |uv| s.subs(uv.0, uv.1));
                let mut best = (t0, f64::INFINITY);
                for i in 0..=256 {
                    let t = t0 + (t1 - t0) * i as f64 / 256.0;
                    let dd = iso.subs(t).distance(target);
                    if dd < best.1 {
                        best = (t, dd);
                    }
                }
                let seeds = foot.map(|uv| coord(along(d), uv)).into_iter().chain(iso.search_nearest_parameter(target, best.0, 64));
                for t in seeds {
                    let t = t.clamp(t0, t1);
                    let dd = iso.subs(t).distance(target);
                    if dd <= best.1 {
                        best = (t, dd);
                    }
                }
                best
            };
            let ((ta, da), (tb, db)) = (near(q1), near(q2));
            let scale = 1.0 + q1.to_homogeneous().truncate().magnitude();
            if da > 1e-5 * scale || db > 1e-5 * scale || (ta - tb).abs() < 1e-12 {
                return Err("split line does not reach the rings".into());
            }
            let (lo, hi) = if ta < tb { (ta, tb) } else { (tb, ta) };
            let mut c = iso;
            let eps = (t1 - t0) * 1e-9;
            if lo > t0 + eps {
                c = mt::Cut::cut(&mut c, lo);
            }
            if hi < t1 - eps {
                let _ = mt::Cut::cut(&mut c, hi);
            }
            if ta > tb {
                c.invert();
            }
            Ok(c)
        }
        mt::Surface::Plane(_) => Err("planes do not wrap".into()),
    }
}

/// The iso-curve of a homogeneous B-spline surface at `v = w` (`along_u`) or `u = w`.
pub(crate) fn iso_bsp(b: &mt::BSplineSurface<mt::Vector4>, w: f64, along_u: bool) -> R<mt::NurbsCurve<mt::Vector4>> {
    let (ku, kv) = b.knot_vecs();
    let ctrl = b.control_points();
    let nu = ctrl.len();
    let nv = ctrl.first().map(Vec::len).unwrap_or(0);
    let zero = mt::Vector4::new(0.0, 0.0, 0.0, 0.0);
    let pts: Vec<mt::Vector4> = if along_u {
        let deg = kv.len().checked_sub(nv + 1).ok_or("bad knot vector")?;
        let basis = kv.try_bspline_basis_functions(deg, w).map_err(|e| e.to_string())?;
        ctrl.iter().map(|row| row.iter().zip(&basis).fold(zero, |acc, (p, f)| acc + *p * *f)).collect()
    } else {
        let deg = ku.len().checked_sub(nu + 1).ok_or("bad knot vector")?;
        let basis = ku.try_bspline_basis_functions(deg, w).map_err(|e| e.to_string())?;
        (0..nv).map(|j| ctrl.iter().zip(&basis).fold(zero, |acc, (row, f)| acc + row.get(j).copied().unwrap_or(zero) * *f)).collect()
    };
    let knots = if along_u { ku.clone() } else { kv.clone() };
    mt::BSplineCurve::try_new(knots, pts).map(mt::NurbsCurve::new).map_err(|e| e.to_string())
}

type Cut = (mt::Edge, (mt::Edge, mt::Edge));

/// Split a chain at parameter `t` of its edge `k` (absolute curve): the edges before and after
/// the split point, the edge cut (unless the point is an existing vertex) and the split vertex.
fn split_chain(chain: &[mt::Edge], k: usize, t: f64, q: mt::Point3) -> R<(Vec<mt::Edge>, Vec<mt::Edge>, Option<Cut>, mt::Vertex)> {
    let e = chain.get(k).ok_or("bad crossing")?;
    let (t0, t1) = e.curve().range_tuple();
    let eps = (t1 - t0).abs() * 1e-7;
    let at_front = (t - t0).abs() <= eps;
    let at_back = (t - t1).abs() <= eps;
    if at_front || at_back {
        // The split point is a vertex already: split the chain between edges.
        let v = if at_front { e.absolute_front().clone() } else { e.absolute_back().clone() };
        let first_end = at_front != e.orientation(); // the point is this edge's end in loop order
        let cut_at = if first_end { k + 1 } else { k };
        let before = chain.iter().take(cut_at).cloned().collect();
        let after = chain.iter().skip(cut_at).cloned().collect();
        return Ok((before, after, None, v));
    }
    let v = mt::Vertex::new(q);
    let (a, b) = e.cut_with_parameter(&v, t).ok_or("cannot cut a ring edge at the split line")?;
    let mut before: Vec<mt::Edge> = chain.iter().take(k).cloned().collect();
    before.push(a.clone());
    let mut after = vec![b.clone()];
    after.extend(chain.iter().skip(k + 1).cloned());
    Ok((before, after, Some((e.clone(), (a, b))), v))
}

struct Plan {
    /// Edge cuts: (edge as used in the band's loop, its two pieces in that loop's order).
    cuts: Vec<Cut>,
    faces: (mt::Face, mt::Face),
}

fn plan(face: &mt::Face, tol: f64) -> R<Option<Plan>> {
    let s = face.surface();
    let wires = face.absolute_boundaries().clone();
    let mut uses: HashMap<mt::EdgeID, usize> = HashMap::new();
    for e in wires.iter().flat_map(|w| w.edge_iter()) {
        *uses.entry(e.id()).or_default() += 1;
    }
    let is_seam = |e: &mt::Edge| uses.get(&e.id()).copied().unwrap_or(0) > 1;
    if !wires.iter().flat_map(|w| w.edge_iter()).any(is_seam) {
        return Ok(None);
    }
    // The closed direction whose boundary carries the seam.
    let seam_pts: Vec<mt::Point3> =
        wires.iter().flat_map(|w| w.edge_iter()).filter(|e| is_seam(e)).flat_map(|e| samples(&e.curve(), 4).into_iter().map(|x| x.1)).collect();
    let Some((d, w0, w1)) = closed_dirs(&s).into_iter().find(|(d, w0, w1)| {
        let p = w1 - w0;
        seam_pts.iter().all(|q| uv_of(&s, *q).map(|uv| coord(*d, uv)).is_some_and(|w| (w - w0).abs().min((w - w1).abs()) < p * 1e-4))
    }) else {
        return Ok(None);
    };
    let (period, mid) = (w1 - w0, 0.5 * (w0 + w1));
    let outer_idx = wires.iter().position(|w| w.edge_iter().any(is_seam)).ok_or("no seam loop")?;
    if wires.iter().enumerate().any(|(i, w)| i != outer_idx && w.edge_iter().any(is_seam)) {
        return Err("seam edges in several loops".into());
    }
    let outer: Vec<mt::Edge> = wires.get(outer_idx).ok_or("no seam loop")?.edge_iter().cloned().collect();
    let n = outer.len();
    let start = (0..n)
        .find(|&i| outer.get(i).is_some_and(is_seam) && outer.get((i + n - 1) % n).is_some_and(|e| !is_seam(e)))
        .ok_or("loop made only of seam edges")?;
    let rot: Vec<mt::Edge> = (0..n).filter_map(|i| outer.get((start + i) % n).cloned()).collect();
    let mut chains: Vec<(bool, Vec<mt::Edge>)> = Vec::new();
    for e in rot {
        let seam = is_seam(&e);
        match chains.last_mut() {
            Some((s0, c)) if *s0 == seam => c.push(e),
            _ => chains.push((seam, vec![e])),
        }
    }
    // A cone's tip: the seam runs from the ring to the apex and straight back.
    if let [(true, sm), (false, ring)] = chains.as_slice() {
        return apex_plan(face, &s, (d, mid, period), sm, ring, &wires, outer_idx, tol).map(Some);
    }
    let [(true, s1), (false, r1), (true, s2), (false, r2)] = chains.as_slice() else {
        return Err(format!("unsupported seam loop layout ({} runs)", chains.len()));
    };
    let s2_ids: Vec<mt::EdgeID> = s2.iter().rev().map(mt::Edge::id).collect();
    if s1.iter().map(mt::Edge::id).collect::<Vec<_>>() != s2_ids {
        return Err("seam edges are not used back and forth".into());
    }
    let (k1, t1, q1) = crossing(&s, d, r1, mid, period)?;
    let (k2, t2, q2) = crossing(&s, d, r2, mid, period)?;
    let seam_line: Vec<mt::Point3> = s1.iter().flat_map(|e| samples(&e.oriented_curve(), 16).into_iter().map(|x| x.1)).collect();
    let iso = iso_curve(&s, d, mid, q1, q2, &seam_line, tol)?;
    let (r1a, r1b, c1, v1) = split_chain(r1, k1, t1, q1)?;
    let (r2a, r2b, c2, v2) = split_chain(r2, k2, t2, q2)?;
    let e_split = mt::Edge::new_unchecked(&v1, &v2, iso);
    // Face A: split, rest of ring 2, seam, start of ring 1. Face B: the other half.
    let mut wa: Vec<mt::Edge> = vec![e_split.clone()];
    wa.extend(r2b);
    wa.extend(s1.iter().cloned());
    wa.extend(r1a.iter().cloned());
    let mut wb: Vec<mt::Edge> = vec![e_split.inverse()];
    wb.extend(r1b);
    wb.extend(s2.iter().cloned());
    wb.extend(r2a);
    let wire_a: mt::Wire = wa.into();
    let wire_b: mt::Wire = wb.into();
    if !wire_a.is_closed() || !wire_b.is_closed() {
        return Err("split loops do not close".into());
    }
    // Holes go to the half that holds them.
    let side_a = r1a
        .iter()
        .flat_map(|e| samples(&e.oriented_curve(), 4))
        .filter_map(|(_, p)| uv_of(&s, p).map(|uv| coord(d, uv)))
        .find(|w| (w - mid).abs() > period * 1e-3)
        .map(|w| w < mid)
        .ok_or("cannot place the split halves")?;
    let (mut ha, mut hb) = (vec![wire_a], vec![wire_b]);
    for (i, w) in wires.iter().enumerate() {
        if i == outer_idx {
            continue;
        }
        let ws: Vec<f64> = w.edge_iter().flat_map(|e| samples(&e.curve(), 4)).filter_map(|(_, p)| uv_of(&s, p).map(|uv| coord(d, uv))).collect();
        if ws.iter().all(|x| (*x < mid) == side_a) {
            ha.push(w.clone());
        } else if ws.iter().all(|x| (*x < mid) != side_a) {
            hb.push(w.clone());
        } else {
            return Err("a hole crosses the split line".into());
        }
    }
    let mut fa = mt::Face::new_unchecked(ha, s.clone());
    let mut fb = mt::Face::new_unchecked(hb, s);
    if !face.orientation() {
        fa.invert();
        fb.invert();
    }
    Ok(Some(Plan { cuts: c1.into_iter().chain(c2).collect(), faces: (fa, fb) }))
}

/// Split a face closing at an apex (loop: seam out to the apex, seam back, the ring) along the
/// iso-line opposite the seam, from the ring to the apex.
fn apex_plan(
    face: &mt::Face,
    s: &mt::Surface,
    (d, mid, period): (Dir, f64, f64),
    sm: &[mt::Edge],
    ring: &[mt::Edge],
    wires: &[mt::Wire],
    outer_idx: usize,
    tol: f64,
) -> R<Plan> {
    let h = sm.len() / 2;
    if !sm.len().is_multiple_of(2) || h == 0 {
        return Err("seam edges are not used back and forth".into());
    }
    let (out, back) = (sm.get(..h).ok_or("seam")?, sm.get(h..).ok_or("seam")?);
    if out.iter().map(mt::Edge::id).collect::<Vec<_>>() != back.iter().rev().map(mt::Edge::id).collect::<Vec<_>>() {
        return Err("seam edges are not used back and forth".into());
    }
    let apex = out.last().ok_or("seam")?.back().clone();
    let (k, t, q) = crossing(s, d, ring, mid, period)?;
    let seam_line: Vec<mt::Point3> = out.iter().flat_map(|e| samples(&e.oriented_curve(), 16).into_iter().map(|x| x.1)).collect();
    let iso = iso_curve(s, d, mid, q, apex.point(), &seam_line, tol)?;
    let (ra, rb, c, vq) = split_chain(ring, k, t, q)?;
    let e = mt::Edge::new_unchecked(&vq, &apex, iso);
    // Face A: out to the apex, down the split line, the ring's second part; face B the rest.
    let mut wa: Vec<mt::Edge> = out.to_vec();
    wa.push(e.inverse());
    wa.extend(rb.iter().cloned());
    let mut wb: Vec<mt::Edge> = vec![e];
    wb.extend(back.iter().cloned());
    wb.extend(ra);
    let (wire_a, wire_b): (mt::Wire, mt::Wire) = (wa.into(), wb.into());
    if !wire_a.is_closed() || !wire_b.is_closed() {
        return Err("split loops do not close".into());
    }
    let side_a = rb
        .iter()
        .flat_map(|e| samples(&e.oriented_curve(), 4))
        .filter_map(|(_, p)| uv_of(s, p).map(|uv| coord(d, uv)))
        .find(|w| (w - mid).abs() > period * 1e-3)
        .map(|w| w < mid)
        .ok_or("cannot place the split halves")?;
    let (mut ha, mut hb) = (vec![wire_a], vec![wire_b]);
    for (i, w) in wires.iter().enumerate() {
        if i == outer_idx {
            continue;
        }
        let ws: Vec<f64> = w.edge_iter().flat_map(|e| samples(&e.curve(), 4)).filter_map(|(_, p)| uv_of(s, p).map(|uv| coord(d, uv))).collect();
        if ws.iter().all(|x| (*x < mid) == side_a) {
            ha.push(w.clone());
        } else if ws.iter().all(|x| (*x < mid) != side_a) {
            hb.push(w.clone());
        } else {
            return Err("a hole crosses the split line".into());
        }
    }
    let mut fa = mt::Face::new_unchecked(ha, s.clone());
    let mut fb = mt::Face::new_unchecked(hb, s.clone());
    if !face.orientation() {
        fa.invert();
        fb.invert();
    }
    Ok(Plan { cuts: c.into_iter().collect(), faces: (fa, fb) })
}

/// Replace cut edges in a face's loops.
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
                match cuts.iter().find(|(c, _)| c.id() == e.id()) {
                    Some((c, (a, b))) if c.orientation() == e.orientation() => out.extend([a.clone(), b.clone()]),
                    Some((_, (a, b))) => out.extend([b.inverse(), a.inverse()]),
                    None => out.push(e.clone()),
                }
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

/// Split every band face of a shell (see the module docs). Faces that cannot be split stay as
/// they are and are reported.
///
/// `tol` is the file's point-on-surface tolerance (`step_in::mod::Units::tol`): 10x the file's
/// declared uncertainty, floored at 1 µm and clamped to [1e-4, 1] mm. A ring edge is written to
/// within this distance of the surface it bounds, so it is how far `iso_curve` may accept a
/// crossing point as lying on the surface.
pub(crate) fn split_bands(mut faces: Vec<mt::Face>, origin: &mut Vec<u64>, warnings: &mut Vec<String>, tol: f64) -> Vec<mt::Face> {
    let n = faces.len();
    for i in 0..n {
        let Some(face) = faces.get(i).cloned() else { continue };
        match crate::guard("step band split", || plan(&face, tol).map_err(crate::KernelError::Failed)) {
            Ok(Some(p)) => {
                faces = faces.iter().map(|f| recut(f, &p.cuts)).collect();
                if let Some(slot) = faces.get_mut(i) {
                    *slot = p.faces.0;
                }
                faces.push(p.faces.1);
                // The second piece comes from the same file face.
                let o = origin.get(i).copied().unwrap_or(0);
                origin.push(o);
            }
            Ok(None) => {}
            Err(e) => warnings
                .push(format!("face wrapping around a closed surface kept whole: {}", e.to_string().trim_start_matches("the operation failed: "))),
        }
    }
    faces
}
