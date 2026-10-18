//! Faces on a closed surface whose boundary winds around it: a cylinder face bounded by two
//! circles and no seam edge, or one trimmed by a helix that turns several times (a modelled
//! thread, as other CAD systems write them to STEP). In the surface's unwrapped parameters such
//! a face repeats every period, and its loops do not close. It is cut along a parameter line
//! (the seam it never had) into one period: every loop, and every copy of it a whole number of
//! periods along, is clipped to the strip between the line and its next copy, and the parts of
//! the two lines inside the face join the pieces into closed loops. Both lines are one curve
//! on the surface, so their nodes are made once and shared: the face stays closed across it.

use solidcraft_geom::Vec3;
use solidcraft_geom::tess::EvalSurface;

use super::size::Rules;

/// One loop of a face: parameters (unwrapped along the loop) and the 3D node each stands for.
#[derive(Clone, Debug, Default)]
pub(crate) struct Loop {
    pub uv: Vec<[f64; 2]>,
    pub pts: Vec<Vec3>,
}

/// Most loop copies one face may be clipped into (a helix of this many turns).
const MAX_COPIES: i64 = 2048;
/// Most nodes a cut line may get between two crossings.
const MAX_SEAM_NODES: usize = 4096;

/// How far `l` drifts along `d` going once around it (a whole number of periods for a closed
/// loop: zero unless it winds around the surface).
pub(crate) fn drift(l: &Loop, d: usize, per: f64) -> f64 {
    let (Some(first), Some(last)) = (l.uv.first(), l.uv.last()) else { return 0.0 };
    let close = super::uv::unwrap(first[d], last[d], Some(per)) - last[d];
    last[d] - first[d] + close
}

/// Whether any loop winds around the closed direction `d`.
pub(crate) fn winds(loops: &[Loop], d: usize, per: f64) -> bool {
    loops.iter().any(|l| drift(l, d, per).abs() > per * 0.5)
}

/// A point where the boundary crosses a cut line: its place along the line, its 3D point, and
/// whether the face lies above it (towards larger parameters across the line) or below.
#[derive(Clone, Copy, Debug)]
struct Crossing {
    y: f64,
    p: Vec3,
    above: bool,
}

/// A boundary run across the strip: it enters through one cut line and leaves through one.
struct Piece {
    uv: Vec<[f64; 2]>,
    pts: Vec<Vec3>,
}

/// Cut a face whose loops wind around direction `d` (period `per`) into one period. The face
/// lies left of its loops in (u, v). `None` when the loops cannot be cut cleanly (the face would
/// be unbounded along the line, or the two lines disagree); the caller then fails as before.
pub(crate) fn cut(s: &EvalSurface, loops: &[Loop], d: usize, per: f64, rules: &Rules, h: f64) -> Option<Vec<Loop>> {
    if !(per > 0.0 && per.is_finite()) || loops.is_empty() {
        return None;
    }
    // Work in (x, y) with x along the closed direction; swapping (u, v) mirrors the plane, so the
    // loops are reversed to keep the face on their left.
    let swap = d == 1;
    let local: Vec<Loop> = loops
        .iter()
        .map(|l| {
            let mut uv: Vec<[f64; 2]> = l.uv.iter().map(|q| if swap { [q[1], q[0]] } else { *q }).collect();
            let mut pts = l.pts.clone();
            if swap {
                uv.reverse();
                pts.reverse();
            }
            Loop { uv, pts }
        })
        .collect();
    let x0 = cut_position(&local, per)?;
    let x1 = x0 + per;
    let mut closed: Vec<Loop> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();
    for l in &local {
        let n = l.uv.len();
        if n < 2 || l.pts.len() != n {
            return None;
        }
        let w = drift(l, 0, per);
        let (lo, hi) = l.uv.iter().fold((f64::MAX, f64::MIN), |(a, b), q| (a.min(q[0]), b.max(q[0])));
        if !(lo.is_finite() && hi.is_finite()) {
            return None;
        }
        if w.abs() <= per * 0.5 {
            // A closed loop: each copy along x that reaches into the strip.
            let k0 = ((x0 - hi) / per).floor() as i64;
            let k1 = ((x1 - lo) / per).ceil() as i64;
            if k1 - k0 > MAX_COPIES {
                return None;
            }
            for k in k0..=k1 {
                let shift = per * k as f64;
                let uv: Vec<[f64; 2]> = l.uv.iter().map(|q| [q[0] + shift, q[1]]).collect();
                if uv.iter().all(|q| q[0] > x0 && q[0] < x1) {
                    closed.push(Loop { uv, pts: l.pts.clone() });
                } else if uv.iter().any(|q| q[0] > x0 && q[0] < x1) || crosses(&uv, x0, x1) {
                    // Start at a node outside the strip, so every run inside is whole.
                    let start = uv.iter().position(|q| !(q[0] > x0 && q[0] < x1))?;
                    let idx: Vec<usize> = (0..=n).map(|i| (start + i) % n).collect();
                    let path: Vec<[f64; 2]> = idx.iter().filter_map(|&i| uv.get(i).copied()).collect();
                    let pts: Vec<Vec3> = idx.iter().filter_map(|&i| l.pts.get(i).copied()).collect();
                    clip(&path, &pts, x0, x1, &mut pieces);
                }
            }
        } else {
            // A loop winding around: the endless path it repeats along, over enough turns to
            // start and end outside the strip.
            let turns = (w / per).round();
            if turns.abs() != 1.0 {
                return None;
            }
            let reach = (hi - lo) / per;
            let span = (reach.ceil() as i64) + 2;
            if span > MAX_COPIES {
                return None;
            }
            // Copies j with lo + j·w ≤ x1 and hi + j·w ≥ x0, plus one each side.
            let (ja, jb) = if w > 0.0 {
                (((x0 - hi) / w).floor() as i64 - 1, ((x1 - lo) / w).ceil() as i64 + 1)
            } else {
                (((x1 - lo) / w).floor() as i64 - 1, ((x0 - hi) / w).ceil() as i64 + 1)
            };
            if jb - ja > MAX_COPIES {
                return None;
            }
            let mut path = Vec::new();
            let mut pts = Vec::new();
            for j in ja..=jb {
                let shift = w * j as f64;
                path.extend(l.uv.iter().map(|q| [q[0] + shift, q[1]]));
                pts.extend(l.pts.iter().copied());
            }
            let (Some(a), Some(b)) = (path.first(), path.last()) else { return None };
            if (a[0] > x0 && a[0] < x1) || (b[0] > x0 && b[0] < x1) {
                return None;
            }
            clip(&path, &pts, x0, x1, &mut pieces);
        }
    }
    // The crossings on each line, sorted along it; both lines must see the same ones.
    let mut left: Vec<(Crossing, usize, bool)> = Vec::new();
    let mut right: Vec<(Crossing, usize, bool)> = Vec::new();
    for (i, p) in pieces.iter().enumerate() {
        let (Some(a), Some(b), Some(pa), Some(pb)) = (p.uv.first(), p.uv.last(), p.pts.first(), p.pts.last()) else { return None };
        // Entering through the left line (moving +x): the face is above; through the right
        // line (moving −x): below. Leaving through the left (−x): below; the right (+x): above.
        let enter = Crossing { y: a[1], p: *pa, above: a[0] == x0 };
        let leave = Crossing { y: b[1], p: *pb, above: b[0] == x1 };
        if a[0] == x0 {
            left.push((enter, i, true))
        } else {
            right.push((enter, i, true))
        }
        if b[0] == x0 { left.push((leave, i, false)) } else { right.push((leave, i, false)) }
    }
    left.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
    right.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
    if left.len() != right.len() || !left.len().is_multiple_of(2) {
        return None;
    }
    let scale = loops.iter().flat_map(|l| l.uv.iter()).fold(1.0f64, |m, q| m.max(q[0].abs()).max(q[1].abs()));
    for (a, b) in left.iter().zip(&right) {
        if a.0.above != b.0.above || (a.0.y - b.0.y).abs() > 1e-7 * scale {
            return None;
        }
    }
    // The same crossing on both lines is one point.
    for (a, b) in left.clone().iter().zip(right.iter_mut()) {
        b.0.y = a.0.y;
        b.0.p = a.0.p;
    }
    for (c, i, start) in left.iter().chain(&right) {
        let p = pieces.get_mut(*i)?;
        let k = if *start { 0 } else { p.uv.len().checked_sub(1)? };
        if let (Some(q), Some(pt)) = (p.uv.get_mut(k), p.pts.get_mut(k)) {
            q[1] = c.y;
            *pt = c.p;
        }
    }
    // Inside the face along each line: from a crossing with the face above to the next one
    // up (which must have it below).
    let mut links: Vec<(usize, Vec<[f64; 2]>, Vec<Vec3>, usize)> = Vec::new();
    for pair in left.chunks(2).zip(right.chunks(2)) {
        let (lp, rp) = pair;
        let (Some(la), Some(lb), Some(ra), Some(rb)) = (lp.first(), lp.get(1), rp.first(), rp.get(1)) else { return None };
        if !(la.0.above && !lb.0.above) {
            return None;
        }
        // Nodes strictly between, made once for both lines.
        let inner = seam_nodes(s, swap, x0, la.0.y, lb.0.y, rules, h)?;
        // Left line, downwards: from the piece leaving at the top to the one entering at the bottom.
        let (top_leave, bottom_enter) = (lb.1, la.1);
        let down_uv: Vec<[f64; 2]> = inner.iter().rev().map(|(y, _)| [x0, *y]).collect();
        let down_pts: Vec<Vec3> = inner.iter().rev().map(|(_, p)| *p).collect();
        if lb.2 || !la.2 {
            return None;
        }
        links.push((top_leave, down_uv, down_pts, bottom_enter));
        // Right line, upwards: from the piece leaving at the bottom to the one entering at the top.
        if ra.2 || !rb.2 {
            return None;
        }
        let up_uv: Vec<[f64; 2]> = inner.iter().map(|(y, _)| [x1, *y]).collect();
        let up_pts: Vec<Vec3> = inner.iter().map(|(_, p)| *p).collect();
        links.push((ra.1, up_uv, up_pts, rb.1));
    }
    // Chain pieces through the links into closed loops.
    let mut next: Vec<Option<usize>> = vec![None; pieces.len()];
    let mut via: Vec<Option<usize>> = vec![None; pieces.len()];
    for (li, (from, _, _, to)) in links.iter().enumerate() {
        if next.get(*from).copied().flatten().is_some() {
            return None;
        }
        *next.get_mut(*from)? = Some(*to);
        *via.get_mut(*from)? = Some(li);
    }
    let mut used = vec![false; pieces.len()];
    for start in 0..pieces.len() {
        if used.get(start).copied().unwrap_or(true) {
            continue;
        }
        let mut out = Loop::default();
        let mut at = start;
        for _ in 0..=pieces.len() {
            if *used.get(at)? {
                break;
            }
            *used.get_mut(at)? = true;
            let p = pieces.get(at)?;
            out.uv.extend(&p.uv);
            out.pts.extend(&p.pts);
            let (_, luv, lpts, _) = links.get((*via.get(at)?)?)?;
            out.uv.extend(luv);
            out.pts.extend(lpts);
            at = (*next.get(at)?)?;
        }
        if at != start {
            return None;
        }
        closed.push(out);
    }
    // Back to (u, v).
    let out: Vec<Loop> = closed
        .into_iter()
        .map(|mut l| {
            if swap {
                l.uv = l.uv.iter().rev().map(|q| [q[1], q[0]]).collect();
                l.pts.reverse();
            }
            l
        })
        .collect();
    // The face must come out with positive area (left of its loops).
    let area: f64 = out
        .iter()
        .map(|l| {
            let n = l.uv.len();
            (0..n)
                .map(|i| {
                    let (a, b) = (l.uv[i], l.uv[(i + 1) % n]);
                    a[0] * b[1] - b[0] * a[1]
                })
                .sum::<f64>()
        })
        .sum();
    (area > 0.0).then_some(out)
}

/// Where to cut: the middle of the widest gap between the loops' nodes around the period, as
/// far from any node as can be.
fn cut_position(loops: &[Loop], per: f64) -> Option<f64> {
    let mut xs: Vec<f64> = loops.iter().flat_map(|l| l.uv.iter().map(|q| q[0].rem_euclid(per))).collect();
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    let mut best = (0.0, f64::NEG_INFINITY);
    for (i, x) in xs.iter().enumerate() {
        let next = xs.get(i + 1).copied().unwrap_or(xs[0] + per);
        if next - x > best.1 {
            best = (0.5 * (x + next), next - x);
        }
    }
    (best.1 > 0.0).then_some(best.0)
}

/// Whether a path jumps over the strip in one segment.
fn crosses(uv: &[[f64; 2]], x0: f64, x1: f64) -> bool {
    uv.windows(2).any(|w| (w[0][0] <= x0 && w[1][0] >= x1) || (w[1][0] <= x0 && w[0][0] >= x1))
}

/// The runs of an open path (starting and ending outside the strip) inside the strip, each
/// from the point where it enters to the one where it leaves. The crossing points lie on the
/// boundary's own straight 3D segments, so the neighbouring face's edge passes through them.
fn clip(path: &[[f64; 2]], pts: &[Vec3], x0: f64, x1: f64, out: &mut Vec<Piece>) {
    let inside = |q: &[f64; 2]| q[0] > x0 && q[0] < x1;
    let mut cur: Option<Piece> = None;
    for k in 0..path.len().saturating_sub(1) {
        let (Some(a), Some(b), Some(pa), Some(pb)) = (path.get(k), path.get(k + 1), pts.get(k), pts.get(k + 1)) else { break };
        let at = |x: f64| {
            let t = ((x - a[0]) / (b[0] - a[0])).clamp(0.0, 1.0);
            ([x, a[1] + (b[1] - a[1]) * t], *pa + (*pb - *pa) * t)
        };
        // Lines crossed by this segment, in order along it.
        let mut hits: Vec<f64> = [x0, x1].into_iter().filter(|x| (a[0] - x) * (b[0] - x) < 0.0).collect();
        if b[0] < a[0] {
            hits.reverse();
        }
        for x in hits {
            let (q, p) = at(x);
            match cur.take() {
                Some(mut piece) => {
                    piece.uv.push(q);
                    piece.pts.push(p);
                    out.push(piece);
                }
                None => cur = Some(Piece { uv: vec![q], pts: vec![p] }),
            }
        }
        if inside(b)
            && let Some(piece) = cur.as_mut()
        {
            piece.uv.push(*b);
            piece.pts.push(*pb);
        }
    }
}

/// Nodes along the cut line between two crossings (`y0` < `y1`, ends excluded): within the
/// tolerance, the side length and the facet angle of the surface along the line.
fn seam_nodes(s: &EvalSurface, swap: bool, x: f64, y0: f64, y1: f64, rules: &Rules, h: f64) -> Option<Vec<(f64, Vec3)>> {
    let uv = |y: f64| if swap { [y, x] } else { [x, y] };
    let mut ys = vec![y0, y1];
    let mut i = 0;
    while i + 1 < ys.len() {
        let (a, b) = (ys[i], ys[i + 1]);
        if ys.len() < MAX_SEAM_NODES && !super::uv::fine_between(s, rules, h, uv(a), uv(b)) && b - a > 1e-12 * (1.0 + a.abs()) {
            ys.insert(i + 1, 0.5 * (a + b));
        } else {
            i += 1;
        }
    }
    let inner = ys.get(1..ys.len() - 1)?;
    Some(inner.iter().map(|&y| (y, s.eval(uv(y)[0], uv(y)[1]).0)).collect())
}

/// Whether a face of `solid` lies on a B-spline surface that joins itself (a lofted tube).
/// Truck meshes a face in its surface's parameters and treats only a revolved surface's
/// rotation as periodic, so a face that runs across such a join comes out folded over itself or
/// with a hole in it. The reference mesh of such a body is made from the display plan,
/// which knows the join is periodic.
pub(crate) fn closes_on_itself(solid: &crate::body::Solid) -> bool {
    solid.face_iter().any(|f| matches!(super::surface::eval_surface(&f.surface()), Some(EvalSurface::Nurbs { closed, .. }) if closed[0] || closed[1]))
}

/// Whether a face of `solid` lies on a surface of revolution and has a boundary loop turning
/// more than once around its axis (a helical trim). Truck's mesher cannot mesh such a face (its
/// triangles cut across the solid), so the reference mesh is made from the display plan instead.
pub(crate) fn turns_more_than_once(solid: &crate::body::Solid) -> bool {
    use truck_meshalgo::prelude::*;
    use truck_modeling as mt;
    let tau = std::f64::consts::TAU;
    for face in solid.face_iter() {
        let mt::Surface::RevolutedCurve(proc) = face.surface() else { continue };
        let rev = proc.entity();
        let m = *proc.transform();
        let o = crate::body::from_p3(m.transform_point(rev.origin()));
        let a = m.transform_vector(rev.axis());
        let Some(k) = Vec3::new(a.x, a.y, a.z).normalized() else { continue };
        let e1 = k.cross(if k.x.abs() < 0.9 { Vec3::X } else { Vec3::Y }).normalized().unwrap_or(Vec3::X);
        let e2 = k.cross(e1);
        let angle = |p: mt::Point3| {
            let d = crate::body::from_p3(p) - o;
            d.dot(e2).atan2(d.dot(e1))
        };
        for wire in face.boundaries() {
            let (mut total, mut lo, mut hi) = (0.0f64, 0.0f64, 0.0f64);
            let mut prev: Option<f64> = None;
            for e in wire.edge_iter() {
                let c = e.oriented_curve();
                let (t0, t1) = c.range_tuple();
                // Samples dense enough that no step turns half way round.
                let mut n = 16usize;
                let steps = loop {
                    let xs: Vec<f64> = (0..=n).map(|i| angle(c.subs(t0 + (t1 - t0) * i as f64 / n as f64))).collect();
                    let ok = xs.windows(2).all(|w| ((w[1] - w[0] + std::f64::consts::PI).rem_euclid(tau) - std::f64::consts::PI).abs() < 1.0);
                    if ok || n >= 4096 {
                        break xs;
                    }
                    n *= 4;
                };
                for x in steps {
                    if let Some(p) = prev {
                        total += (x - p + std::f64::consts::PI).rem_euclid(tau) - std::f64::consts::PI;
                        lo = lo.min(total);
                        hi = hi.max(total);
                    }
                    prev = Some(x);
                }
            }
            if hi - lo > tau * 1.01 {
                return true;
            }
        }
    }
    false
}
