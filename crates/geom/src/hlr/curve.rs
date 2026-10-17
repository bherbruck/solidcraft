//! Evaluation of portable B-rep curves: lines, circle and ellipse arcs, polynomial and rational
//! splines (fitted intersection records are ordinary splines with a stated fitting tolerance).

use super::{Accuracy, HlrError};
use crate::Vec3;
use crate::brep::{Curve, Frame};

#[derive(Clone, Debug)]
pub(crate) enum C3 {
    Line {
        a: Vec3,
        b: Vec3,
    },
    /// origin + x*rx*cos(t) + y*ry*sin(t), t in `range`.
    Arc {
        o: Vec3,
        x: Vec3,
        y: Vec3,
        r: [f64; 2],
        range: [f64; 2],
    },
    /// Homogeneous control points `[x*w, y*w, z*w, w]` (w = 1 for polynomial splines).
    Spline {
        degree: usize,
        knots: Vec<f64>,
        points: Vec<[f64; 4]>,
    },
    Polyline(Vec<Vec3>),
}

pub(crate) fn v3(p: [f64; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

pub(crate) fn frame_axes(f: &Frame) -> (Vec3, Vec3, Vec3) {
    let (x, y) = (v3(f.x_axis), v3(f.y_axis));
    (v3(f.origin), x, y)
}

impl C3 {
    pub fn new(c: &Curve) -> Result<(C3, Accuracy), HlrError> {
        Ok(match c {
            Curve::Line { start, end } => (C3::Line { a: v3(*start), b: v3(*end) }, Accuracy::Exact),
            Curve::CircleArc { frame, radius, angle } => {
                let (o, x, y) = frame_axes(frame);
                (C3::Arc { o, x, y, r: [*radius, *radius], range: *angle }, Accuracy::Exact)
            }
            Curve::EllipseArc { frame, radii, angle } => {
                let (o, x, y) = frame_axes(frame);
                (C3::Arc { o, x, y, r: *radii, range: *angle }, Accuracy::Exact)
            }
            Curve::Polyline { points } => (C3::Polyline(points.iter().copied().map(v3).collect()), Accuracy::Exact),
            Curve::Spline { knots, points } => (spline(knots, points.iter().map(|p| [p[0], p[1], p[2], 1.]).collect())?, Accuracy::Exact),
            Curve::RationalSpline { knots, homogeneous } => (spline(knots, homogeneous.clone())?, Accuracy::Exact),
            Curve::Intersection { knots, points, fitting_tolerance, .. } => {
                (spline(knots, points.iter().map(|p| [p[0], p[1], p[2], 1.]).collect())?, Accuracy::Bounded(*fitting_tolerance))
            }
        })
    }

    pub fn domain(&self) -> [f64; 2] {
        match self {
            C3::Line { .. } => [0., 1.],
            C3::Arc { range, .. } => *range,
            C3::Spline { degree, knots, points } => [knots.get(*degree).copied().unwrap_or(0.), knots.get(points.len()).copied().unwrap_or(1.)],
            C3::Polyline(p) => [0., p.len().saturating_sub(1) as f64],
        }
    }

    pub fn at(&self, t: f64) -> Vec3 {
        match self {
            C3::Line { a, b } => *a + (*b - *a) * t,
            C3::Arc { o, x, y, r, .. } => *o + *x * (r[0] * t.cos()) + *y * (r[1] * t.sin()),
            C3::Spline { degree, knots, points } => de_boor(*degree, knots, points, t),
            C3::Polyline(p) => {
                let n = p.len();
                if n == 0 {
                    return Vec3::ZERO;
                }
                let t = t.clamp(0., (n - 1) as f64);
                let i = (t.floor() as usize).min(n.saturating_sub(2));
                match (p.get(i), p.get(i + 1)) {
                    (Some(a), Some(b)) => a.lerp(*b, t - i as f64),
                    (Some(a), None) => *a,
                    _ => Vec3::ZERO,
                }
            }
        }
    }

    /// First derivative by a central difference scaled to the domain.
    pub fn d1(&self, t: f64) -> Vec3 {
        match self {
            C3::Line { a, b } => *b - *a,
            C3::Arc { x, y, r, .. } => *x * (-r[0] * t.sin()) + *y * (r[1] * t.cos()),
            _ => {
                let [t0, t1] = self.domain();
                let h = (t1 - t0) * 1e-6;
                let (a, b) = ((t - h).max(t0), (t + h).min(t1));
                if b - a <= 0. { Vec3::ZERO } else { (self.at(b) - self.at(a)) / (b - a) }
            }
        }
    }

    /// Parameters that split the curve into pieces each turning at most `max_turn` radians or
    /// spanning at most a quarter of the domain, plus all interior knots.
    pub fn breaks(&self) -> Vec<f64> {
        let [t0, t1] = self.domain();
        let mut out = vec![t0, t1];
        match self {
            C3::Spline { knots, .. } => out.extend(knots.iter().copied().filter(|k| *k > t0 && *k < t1)),
            C3::Polyline(p) => out.extend((1..p.len().saturating_sub(1)).map(|i| i as f64)),
            C3::Arc { .. } => {
                let n = ((t1 - t0) / (std::f64::consts::PI / 8.)).ceil().max(1.) as usize;
                out.extend((1..n).map(|i| t0 + (t1 - t0) * i as f64 / n as f64));
            }
            C3::Line { .. } => {}
        }
        out.sort_by(f64::total_cmp);
        out.dedup_by(|a, b| (*a - *b).abs() <= 1e-15 * (1. + b.abs()));
        out
    }
}

fn spline(knots: &[f64], points: Vec<[f64; 4]>) -> Result<C3, HlrError> {
    let degree = knots.len().checked_sub(points.len() + 1).ok_or(HlrError::Unsupported("spline knot count"))?;
    if degree == 0 || degree > 16 || points.is_empty() {
        return Err(HlrError::Unsupported("spline degree"));
    }
    Ok(C3::Spline { degree, knots: knots.to_vec(), points })
}

/// De Boor evaluation of a (rational) B-spline at `t`, clamped to the active domain.
pub(crate) fn de_boor(p: usize, knots: &[f64], ctrl: &[[f64; 4]], t: f64) -> Vec3 {
    let n = ctrl.len();
    let (Some(&lo), Some(&hi)) = (knots.get(p), knots.get(n)) else { return Vec3::ZERO };
    let t = t.clamp(lo, hi);
    // Span k with knots[k] <= t < knots[k+1], last nonempty span at the end.
    let mut k = p;
    while k + 1 < n && knots.get(k + 1).is_some_and(|u| *u <= t) {
        k += 1;
    }
    let mut d: Vec<[f64; 4]> = (0..=p).map(|j| ctrl.get(j + k - p).copied().unwrap_or([0.; 4])).collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let (a, b) = (knots.get(j + k - p).copied().unwrap_or(0.), knots.get(j + 1 + k - r).copied().unwrap_or(0.));
            let alpha = if b - a == 0. { 0. } else { (t - a) / (b - a) };
            let (prev, cur) = (d.get(j - 1).copied().unwrap_or([0.; 4]), d.get(j).copied().unwrap_or([0.; 4]));
            if let Some(slot) = d.get_mut(j) {
                for c in 0..4 {
                    slot[c] = (1. - alpha) * prev[c] + alpha * cur[c];
                }
            }
        }
    }
    let h = d.get(p).copied().unwrap_or([0.; 4]);
    if h[3] == 0. { Vec3::ZERO } else { Vec3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3]) }
}

/// The circle a curve lies on, if it is one: centre, unit normal, radius. Checked at many
/// parameters against `tol` (relative to the radius), so rational circle splines are recognised.
pub(crate) fn as_circle(c: &C3, tol: f64) -> Option<(Vec3, Vec3, f64)> {
    match c {
        C3::Arc { o, x, y, r, .. } if (r[0] - r[1]).abs() <= tol * r[0] => Some((*o, x.cross(*y).normalized()?, r[0])),
        C3::Spline { .. } => {
            let [t0, t1] = c.domain();
            let at = |f: f64| c.at(t0 + (t1 - t0) * f);
            let (a, b, m) = (at(0.), at(1.), at(0.5));
            // Circumcentre of a, m and a third point (b may equal a for a full circle).
            let third = if a.dist(b) < 1e-9 * (1. + a.len()) { at(0.25) } else { b };
            let (u, v) = (m - a, third - a);
            let n = u.cross(v);
            let n2 = n.len2();
            if n2 <= 1e-24 {
                return None;
            }
            let centre = a + (v * u.len2() - u * v.len2()).cross(n) * (1. / (2. * n2));
            let normal = n.normalized()?;
            let r = centre.dist(a);
            let ok = (0..=64).all(|i| {
                let p = at(i as f64 / 64.);
                ((p.dist(centre) - r).abs() <= tol * r) && ((p - centre).dot(normal).abs() <= tol * r)
            });
            ok.then_some((centre, normal, r))
        }
        _ => None,
    }
}
