//! Silhouettes of curved faces: the curves where the surface normal is perpendicular to the
//! direction of sight. Recognised cylinders and cones give exact lines and spheres an exact
//! circle; other surfaces are contoured on a chart grid, each point refined onto the curve,
//! and the deviation of the chords from the curve is measured.

use super::Accuracy;
use super::curve::C3;
use super::surface::{Analytic, S3};
use super::trim::Trim;
use crate::Vec3;

const TAU: f64 = std::f64::consts::TAU;

pub(crate) struct Sil {
    pub curve: C3,
    pub acc: Accuracy,
    /// Parameter ranges of `curve` that lie inside the face.
    pub ranges: Vec<[f64; 2]>,
}

fn perp(a: Vec3) -> Vec3 {
    a.any_perp().normalized().unwrap_or(Vec3::new(1., 0., 0.))
}

/// Axial extent of the face's boundary relative to `p` along `a`.
fn extent(trim: &Trim, p: Vec3, a: Vec3) -> (f64, f64) {
    trim.chords.iter().flat_map(|(x, y)| [*x, *y]).fold((f64::MAX, f64::MIN), |(lo, hi), q| {
        let t = (q - p).dot(a);
        (lo.min(t), hi.max(t))
    })
}

pub(crate) fn silhouettes(s: &S3, an: Analytic, trim: &Trim, d: Vec3, out: &mut Vec<Sil>) {
    let mut push = |curve: C3, acc: Accuracy| {
        let ranges = inside_ranges(&curve, s, trim);
        if !ranges.is_empty() {
            out.push(Sil { curve, acc, ranges });
        }
    };
    match an {
        Analytic::Plane => {}
        Analytic::Cylinder { p, a, r } => {
            let Some(m) = a.cross(d).normalized() else { return };
            let (t0, t1) = extent(trim, p, a);
            if !(t1 > t0) {
                return;
            }
            for side in [1., -1.] {
                let base = p + m * (r * side);
                push(C3::Line { a: base + a * t0, b: base + a * t1 }, Accuracy::Exact);
            }
        }
        Analytic::Cone { apex, a, half } => {
            let e1 = perp(a);
            let e2 = a.cross(e1);
            let (da, d1, d2) = (d.dot(a), d.dot(e1), d.dot(e2));
            let rho = (d1 * d1 + d2 * d2).sqrt();
            if rho <= 1e-12 {
                return;
            }
            let c = half.tan() * da / rho;
            if c.abs() > 1. {
                return;
            }
            let phid = d2.atan2(d1);
            let (h0, h1) = extent(trim, apex, a);
            let (t0, t1) = (h0.max(0.) / half.cos(), h1.max(0.) / half.cos());
            if !(t1 > t0) {
                return;
            }
            for sign in [1., -1.] {
                let phi = phid + sign * c.acos();
                let g = a * half.cos() + (e1 * phi.cos() + e2 * phi.sin()) * half.sin();
                push(C3::Line { a: apex + g * t0, b: apex + g * t1 }, Accuracy::Exact);
            }
        }
        Analytic::Sphere { c, r } => {
            let x = perp(d);
            let y = d.cross(x);
            push(C3::Arc { o: c, x, y, r: [r, r], range: [0., TAU] }, Accuracy::Exact);
        }
        Analytic::Torus { .. } | Analytic::Other => {
            let traced = if matches!(s, S3::Rev(_)) { s.rev_silhouette(d) } else { contour(s, trim, d) };
            for (curve, dev) in traced {
                let acc = if dev == 0. && matches!(curve, C3::Arc { .. }) { Accuracy::Exact } else { Accuracy::Sampled(dev) };
                push(curve, acc);
            }
        }
    }
}

/// Parameter ranges of `c` whose points are inside the face, transitions found by bisection.
pub(crate) fn inside_ranges(c: &C3, s: &S3, trim: &Trim) -> Vec<[f64; 2]> {
    let [t0, t1] = c.domain();
    let n = 256;
    let mut hint = None;
    let state = |t: f64, hint: &mut Option<[f64; 2]>| -> bool {
        let p = c.at(t);
        let uv = s.inverse(p, *hint);
        *hint = uv;
        uv.and_then(|uv| trim.contains(uv)).unwrap_or(false)
    };
    // Uniform samples, plus the parameters nearest to the face's boundary points: an in-face
    // interval is bounded by boundary crossings, so a narrow one between two uniform samples
    // still gets samples on both of its sides.
    let mut ts: Vec<f64> = (0..=n).map(|i| t0 + (t1 - t0) * i as f64 / n as f64).collect();
    let near = 0.02 * (trim.hi - trim.lo).len();
    ts.extend(trim.chords.iter().step_by(4).filter_map(|(q, _)| closest_param(c, *q).filter(|t| c.at(*t).dist(*q) <= near)));
    ts.retain(|t| *t >= t0 && *t <= t1);
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|a, b| (*a - *b).abs() <= 1e-12 * (t1 - t0));
    let mut hints = Vec::with_capacity(ts.len());
    let states: Vec<bool> = ts
        .iter()
        .map(|t| {
            let st = state(*t, &mut hint);
            hints.push(hint);
            st
        })
        .collect();
    let mut out = Vec::new();
    let mut start: Option<f64> = None;
    for i in 0..ts.len() {
        let (Some(&t), Some(&inside)) = (ts.get(i), states.get(i)) else { continue };
        let prev = i.checked_sub(1).and_then(|j| states.get(j).copied());
        if prev.is_some_and(|p| p != inside) {
            let (mut lo, mut hi) = (ts.get(i - 1).copied().unwrap_or(t), t);
            let near = hints.get(i).copied().flatten();
            for _ in 0..50 {
                let mid = 0.5 * (lo + hi);
                let mut h = near;
                if state(mid, &mut h) == inside {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let edge = 0.5 * (lo + hi);
            if inside {
                start = Some(edge);
            } else if let Some(s0) = start.take() {
                out.push([s0, edge]);
            }
        } else if i == 0 && inside {
            start = Some(t);
        }
    }
    if let Some(s0) = start {
        out.push([s0, t1]);
    }
    out.retain(|r| r[1] - r[0] > 1e-12 * (t1 - t0));
    out
}

/// The parameter of the point of `c` nearest to `p` (lines and arcs exactly, polylines at a
/// vertex), when `p` projects inside the curve's domain.
fn closest_param(c: &C3, p: Vec3) -> Option<f64> {
    match c {
        C3::Line { a, b } => {
            let e = *b - *a;
            let l2 = e.len2();
            (l2 > 0.).then(|| ((p - *a).dot(e) / l2).clamp(0., 1.))
        }
        C3::Arc { o, x, y, r, range } => {
            let w = p - *o;
            let t = (w.dot(*y) / r[1]).atan2(w.dot(*x) / r[0]);
            // The turn of t that falls in the range, if any.
            let k = ((range[0] - t) / TAU).ceil();
            let t = t + k * TAU;
            (t <= range[1]).then_some(t)
        }
        C3::Polyline(pts) => pts.iter().enumerate().min_by(|(_, a), (_, b)| a.dist(p).total_cmp(&b.dist(p))).map(|(i, _)| i as f64),
        C3::Spline { .. } => None,
    }
}

/// Zero set of `normal . d` on a chart grid over the face's chart box.
fn contour(s: &S3, trim: &Trim, d: Vec3) -> Vec<(C3, f64)> {
    let Some((lo, hi)) = trim.uv_bounds() else { return Vec::new() };
    let dom = s.domain();
    let lo = [lo[0].max(dom[0][0]), lo[1].max(dom[1][0])];
    let hi = [hi[0].min(dom[0][1]), hi[1].min(dom[1][1])];
    if !(hi[0] > lo[0] && hi[1] > lo[1]) {
        return Vec::new();
    }
    let g = 128usize;
    let uv = |i: usize, j: usize| [lo[0] + (hi[0] - lo[0]) * i as f64 / g as f64, lo[1] + (hi[1] - lo[1]) * j as f64 / g as f64];
    let f = |p: [f64; 2]| s.normal(p).map_or(f64::NAN, |n| n.dot(d));
    let vals: Vec<f64> = (0..=g).flat_map(|j| (0..=g).map(move |i| (i, j))).map(|(i, j)| f(uv(i, j))).collect();
    let val = |i: usize, j: usize| vals.get(j * (g + 1) + i).copied().unwrap_or(f64::NAN);
    let zero = |a: [f64; 2], b: [f64; 2], fa: f64| -> [f64; 2] {
        let (mut lo, mut hi, mut flo) = (a, b, fa);
        for _ in 0..50 {
            let m = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
            let fm = f(m);
            if fm.is_nan() {
                break;
            }
            if (fm > 0.) == (flo > 0.) {
                lo = m;
                flo = fm;
            } else {
                hi = m;
            }
        }
        [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])]
    };
    // Edge ids: horizontal (i,j)-(i+1,j) = 2k, vertical (i,j)-(i,j+1) = 2k+1, k = j*(g+1)+i.
    let mut points = std::collections::BTreeMap::new();
    let mut crossing = |id: usize, a: (usize, usize), b: (usize, usize)| -> Option<usize> {
        let (fa, fb) = (val(a.0, a.1), val(b.0, b.1));
        if fa.is_nan() || fb.is_nan() || (fa > 0.) == (fb > 0.) {
            return None;
        }
        points.entry(id).or_insert_with(|| zero(uv(a.0, a.1), uv(b.0, b.1), fa));
        Some(id)
    };
    let mut links: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for j in 0..g {
        for i in 0..g {
            let k = |i: usize, j: usize| j * (g + 1) + i;
            let e = [
                crossing(2 * k(i, j), (i, j), (i + 1, j)),
                crossing(2 * k(i + 1, j) + 1, (i + 1, j), (i + 1, j + 1)),
                crossing(2 * k(i, j + 1), (i, j + 1), (i + 1, j + 1)),
                crossing(2 * k(i, j) + 1, (i, j), (i, j + 1)),
            ];
            let found: Vec<usize> = e.iter().flatten().copied().collect();
            let pairs: Vec<(usize, usize)> = match found.as_slice() {
                [a, b] => vec![(*a, *b)],
                [a, b, c, dd] => {
                    let centre = f([0.5 * (uv(i, j)[0] + uv(i + 1, j)[0]), 0.5 * (uv(i, j)[1] + uv(i, j + 1)[1])]);
                    if (centre > 0.) == (val(i, j) > 0.) { vec![(*a, *b), (*c, *dd)] } else { vec![(*a, *dd), (*b, *c)] }
                }
                _ => Vec::new(),
            };
            for (a, b) in pairs {
                links.entry(a).or_default().push(b);
                links.entry(b).or_default().push(a);
            }
        }
    }
    // Chain the links into polylines.
    let mut used = std::collections::BTreeSet::new();
    let mut chains = Vec::new();
    let starts: Vec<usize> = links.iter().filter(|(_, v)| v.len() == 1).map(|(k, _)| *k).chain(links.keys().copied()).collect();
    for s0 in starts {
        if used.contains(&s0) {
            continue;
        }
        let mut chain = vec![s0];
        used.insert(s0);
        let mut cur = s0;
        while let Some(next) = links.get(&cur).and_then(|v| v.iter().find(|n| !used.contains(*n)).copied()) {
            used.insert(next);
            chain.push(next);
            cur = next;
        }
        // Close loops back to their start.
        if chain.len() > 2 && links.get(&cur).is_some_and(|v| v.contains(&s0)) {
            chain.push(s0);
        }
        if chain.len() >= 2 {
            chains.push(chain);
        }
    }
    let mut out = Vec::new();
    for chain in chains {
        let uvs: Vec<[f64; 2]> = chain.iter().filter_map(|id| points.get(id).copied()).collect();
        let pts: Vec<Vec3> = uvs.iter().map(|p| s.point(*p)).collect();
        // Deviation: refine each chord's midpoint onto the curve across the chord.
        let mut dev = 0f64;
        for (w, q) in uvs.windows(2).zip(pts.windows(2)) {
            let (Some(a), Some(b), Some(pa), Some(pb)) = (w.first(), w.get(1), q.first(), q.get(1)) else { continue };
            let m = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
            let n = [-(b[1] - a[1]), b[0] - a[0]];
            let (x, y) = ([m[0] - n[0], m[1] - n[1]], [m[0] + n[0], m[1] + n[1]]);
            let (fx, fy) = (f(x), f(y));
            if fx.is_nan() || fy.is_nan() || (fx > 0.) == (fy > 0.) {
                continue;
            }
            let exact = s.point(zero(x, y, fx));
            dev = dev.max(exact.dist_to_segment(*pa, *pb));
        }
        out.push((C3::Polyline(pts), dev));
    }
    out
}
