//! Whole-span certificates for analytic planar prism boundaries.
//! Samples propose a circle; only positive rational Bernstein bounds accept it.
use crate::body::from_p3;
use solidcraft_geom::{Plane, Seg2, Vec2};
use truck_modeling::{self as mt, BoundedCurve, Invertible, ParametricCurve};

const MAX_DEGREE: usize = 32;
/// How far an arc read off a curve may end from the curve's end (mm): half the gap a profile
/// loop may have between segments and still close (`crate::build`).
const ARC_END_TOL: f64 = 5e-7;
const MAX_CONTROLS: usize = 4096;
type H = [f64; 3];

fn charge(work: &mut usize, n: usize) -> Option<()> {
    *work = work.checked_sub(n)?;
    Some(())
}

fn preflight(c: &mt::Curve, work: &mut usize) -> Option<()> {
    charge(work, 1)?;
    let (knots, controls) = match c {
        mt::Curve::Line(_) => return Some(()),
        mt::Curve::BSplineCurve(b) => (b.knot_vec(), b.control_points().len()),
        mt::Curve::NurbsCurve(b) => (b.knot_vec(), b.control_points().len()),
        _ => return None,
    };
    let degree = knots.len().checked_sub(controls + 1)?;
    if !(1..=MAX_DEGREE).contains(&degree) || controls > MAX_CONTROLS || knots.len() > 8192 {
        return None;
    }
    if knots.iter().any(|t| !t.is_finite()) || knots.windows(2).any(|w| w[1] < w[0]) {
        return None;
    }
    // Charge decomposition before its cloning / knot insertion allocations.
    charge(work, controls.checked_mul(knots.len())?.checked_mul(degree + 1)?)
}

fn controls(c: &mt::Curve, plane: &Plane, h: f64, tol: f64) -> Option<(Vec<Vec<H>>, f64)> {
    let spans: Vec<Vec<mt::Vector4>> = match c {
        mt::Curve::BSplineCurve(b) => {
            b.bezier_decomposition().into_iter().map(|s| s.control_points().iter().map(|p| mt::Vector4::new(p.x, p.y, p.z, 1.)).collect()).collect()
        }
        mt::Curve::NurbsCurve(b) => b.non_rationalized().bezier_decomposition().into_iter().map(|s| s.control_points().clone()).collect(),
        _ => return None,
    };
    if spans.is_empty() || spans.len() > 256 || spans.iter().map(Vec::len).sum::<usize>() > 8192 {
        return None;
    }
    let mut error: f64 = 0.;
    let mut out = Vec::new();
    for s in spans {
        if !(2..=MAX_DEGREE + 1).contains(&s.len()) {
            return None;
        }
        let max_w = s.iter().map(|p| p.w).fold(0., f64::max);
        if !max_w.is_finite() || max_w <= 0. {
            return None;
        }
        let mut span = Vec::new();
        for p in s {
            if ![p.x, p.y, p.z, p.w].iter().all(|x| x.is_finite()) || p.w / max_w < 1e-12 {
                return None;
            }
            let v = from_p3(mt::Point3::new(p.x / p.w, p.y / p.w, p.z / p.w));
            let local = plane.to_local(v);
            let magnitude = v.x.abs() + v.y.abs() + v.z.abs() + plane.origin.len();
            let e = magnitude * f64::EPSILON * 256.;
            if !local.x.is_finite() || !local.y.is_finite() || (plane.height(v) - h).abs() + e > tol {
                return None;
            }
            error = error.max(e);
            let w = p.w / max_w;
            span.push([local.x * w, local.y * w, w]);
        }
        out.push(span);
    }
    (error < tol * 0.1).then_some((out, error))
}

fn product(a: &[f64], b: &[f64], work: &mut usize) -> Option<Vec<f64>> {
    charge(work, a.len().checked_mul(b.len())?)?;
    let (m, n) = (a.len() - 1, b.len() - 1);
    let choose = |degree: usize| {
        let mut v = vec![1.; degree + 1];
        for i in 1..=degree {
            v[i] = v[i - 1] * (degree + 1 - i) as f64 / i as f64;
        }
        v
    };
    let (cm, cn, ct) = (choose(m), choose(n), choose(m + n));
    let mut out = vec![0.; m + n + 1];
    for (i, ai) in a.iter().enumerate() {
        for (j, bj) in b.iter().enumerate() {
            out[i + j] += ai * bj * cm[i] * cn[j] / ct[i + j];
        }
    }
    out.iter().all(|x| x.is_finite()).then_some(out)
}

fn derivative(a: &[f64]) -> Vec<f64> {
    let n = (a.len() - 1) as f64;
    a.windows(2).map(|w| (w[1] - w[0]) * n).collect()
}

fn determinant(a: &[f64], b: &[f64], work: &mut usize) -> Option<(Vec<f64>, f64)> {
    let (ab, ba) = (product(a, &derivative(b), work)?, product(b, &derivative(a), work)?);
    let max = |v: &[f64]| v.iter().map(|x| x.abs()).fold(0., f64::max);
    // Bernstein product weights are positive and sum to one. Bound absolute
    // summands from the inputs, never from a possibly cancelled output sum.
    let scale = max(a) * max(&derivative(b)) + max(b) * max(&derivative(a));
    let error = scale * f64::EPSILON * 128. * (a.len() * a.len()) as f64;
    Some((ab.iter().zip(&ba).map(|(x, y)| x - y).collect(), error))
}

fn split(c: &[H]) -> (Vec<H>, Vec<H>) {
    let mut q = c.to_vec();
    let mut left = vec![q[0]];
    let mut right = vec![q[q.len() - 1]];
    while q.len() > 1 {
        q = q.windows(2).map(|p| std::array::from_fn(|i| (p[0][i] + p[1][i]) * 0.5)).collect();
        left.push(q[0]);
        right.push(q[q.len() - 1]);
    }
    right.reverse();
    (left, right)
}

fn point(p: H) -> Vec2 {
    Vec2::new(p[0] / p[2], p[1] / p[2])
}

fn line_span(c: &[H], a: Vec2, b: Vec2, tol: f64, error: f64, work: &mut usize) -> Option<()> {
    let d = b - a;
    let length = d.len();
    if length <= tol {
        return None;
    }
    let d = d / length;
    let mut projected = Vec::new();
    let weights: Vec<_> = c.iter().map(|p| p[2]).collect();
    for p in c {
        let v = point(*p) - a;
        let t = v.dot(d);
        if (v.x * d.y - v.y * d.x).abs() + error > tol || t < -tol + error || t > length + tol - error {
            return None;
        }
        projected.push(t * p[2]);
    }
    // W*T' - T*W' proves oriented traversal, rather than merely collinearity.
    let (der, mut err) = determinant(&weights, &projected, work)?;
    err += error * (c.len() * c.len()) as f64 * 32.;
    if !der.iter().all(|d| *d > err) {
        return None;
    }
    Some(())
}

fn circle_span(c: &[H], center: Vec2, radius: f64, tol: f64, source_error: f64, depth: usize, work: &mut usize) -> Option<f64> {
    charge(work, c.len())?;
    let x: Vec<_> = c.iter().map(|p| p[0] - center.x * p[2]).collect();
    let y: Vec<_> = c.iter().map(|p| p[1] - center.y * p[2]).collect();
    let w: Vec<_> = c.iter().map(|p| p[2]).collect();
    let min_w = w.iter().copied().fold(f64::INFINITY, f64::min);
    if min_w < 1e-12 {
        return None;
    }
    let (xx, yy, ww) = (product(&x, &x, work)?, product(&y, &y, work)?, product(&w, &w, work)?);
    let max = |v: &[f64]| v.iter().map(|x| x.abs()).fold(0., f64::max);
    let scale = max(&x).powi(2) + max(&y).powi(2) + radius * radius * max(&w).powi(2);
    let coordinate_error = source_error + (center.len() + radius) * f64::EPSILON * (depth + 1) as f64 * 128. * c.len() as f64;
    let roundoff = scale * f64::EPSILON * 256. * (c.len() * c.len()) as f64;
    let residual = xx.iter().zip(&yy).zip(&ww).map(|((a, b), w)| (a + b - radius * radius * w).abs()).fold(0., f64::max);
    let radial_tol = tol - coordinate_error;
    if radial_tol <= 0. || (residual + roundoff) / (min_w * min_w) > radial_tol * (2. * radius - radial_tol) {
        return None;
    }
    let (angular, mut error) = determinant(&x, &y, work)?;
    let extent = x.iter().chain(&y).map(|v| v.abs()).fold(0., f64::max);
    error += coordinate_error * extent * (c.len() * c.len()) as f64 * 32.;
    let positive = angular.iter().all(|v| *v > error);
    let negative = angular.iter().all(|v| *v < -error);
    let a = point(c[0]) - center;
    let b = point(c[c.len() - 1]) - center;
    let mid = a + b;
    // One open angular half-plane plus a derivative sign rules out hidden turns.
    let half_plane = mid.len() > radius * 1e-8 && c.iter().all(|p| (point(*p) - center).dot(mid) > coordinate_error * mid.len());
    if (positive || negative) && half_plane {
        let cross = a.x * b.y - a.y * b.x;
        let sweep = cross.atan2(a.dot(b));
        if (positive && sweep > 0.) || (negative && sweep < 0.) {
            return Some(sweep);
        }
        return None;
    }
    if depth >= 12 {
        return None;
    }
    let (left, right) = split(c);
    let a = circle_span(&left, center, radius, tol, source_error, depth + 1, work)?;
    let b = circle_span(&right, center, radius, tol, source_error, depth + 1, work)?;
    (a.signum() == b.signum()).then_some(a + b)
}

/// [`segment`], except that a spline that is neither a line nor a circular arc comes back as lines
/// and arcs that stay within `fit` of it (the walls of a body swept along a spline). `None` where
/// [`segment`] refuses for another reason: not in the plane, not finite, over the work budget,
/// or a rational spline.
pub(super) fn segment_fitted(e: &mt::Edge, plane: &Plane, tol: f64, fit: f64, work: &mut usize) -> Option<Vec<Seg2>> {
    if let Some(exact) = segment(e, plane, tol, work) {
        return Some(exact);
    }
    if !tol.is_finite() || tol <= 0. || !fit.is_finite() || fit <= 0. {
        return None;
    }
    let mut c = e.curve();
    preflight(&c, work)?;
    if !e.orientation() {
        c.invert();
    }
    let (t0, t1) = c.range_tuple();
    if !(t0.is_finite() && t1.is_finite() && t1 > t0) {
        return None;
    }
    let (a3, b3) = (from_p3(c.subs(t0)), from_p3(c.subs(t1)));
    if !a3.is_finite() || !b3.is_finite() || a3.dist(from_p3(e.front().point())) > tol || b3.dist(from_p3(e.back().point())) > tol {
        return None;
    }
    let (spans, _) = controls(&c, plane, plane.height(a3), tol)?;
    let mut out = Vec::new();
    for span in spans {
        if span.iter().any(|p| (p[2] - 1.).abs() > 1e-9) {
            return None;
        }
        charge(work, 64)?;
        let points: Vec<Vec2> = span.iter().map(|p| point(*p)).collect();
        out.extend(solidcraft_geom::bezier(&points)?.fit_arcs(fit)?);
        if out.len() > MAX_CONTROLS {
            return None;
        }
    }
    Some(out)
}

pub(super) fn segment(e: &mt::Edge, plane: &Plane, tol: f64, work: &mut usize) -> Option<Vec<Seg2>> {
    if !tol.is_finite() || tol <= 0. {
        return None;
    }
    let mut c = e.curve();
    preflight(&c, work)?;
    if !e.orientation() {
        c.invert();
    }
    let (t0, t1) = c.range_tuple();
    if !t0.is_finite() || !t1.is_finite() || t1 <= t0 {
        return None;
    }
    let (a3, b3) = (from_p3(c.subs(t0)), from_p3(c.subs(t1)));
    let (a, b) = (plane.to_local(a3), plane.to_local(b3));
    if !a3.is_finite()
        || !b3.is_finite()
        || ![a.x, a.y, b.x, b.y].iter().all(|v| v.is_finite())
        || a3.dist(from_p3(e.front().point())) > tol
        || b3.dist(from_p3(e.back().point())) > tol
    {
        return None;
    }
    if matches!(c, mt::Curve::Line(_)) {
        let error = (a3.len() + b3.len() + plane.origin.len()) * f64::EPSILON * 256.;
        return (error < tol * 0.1 && a.dist(b) > tol && (plane.height(a3) - plane.height(b3)).abs() + error <= tol)
            .then_some(vec![Seg2::Line { a, b }]);
    }
    let (spans, error) = controls(&c, plane, plane.height(a3), tol)?;
    if spans.windows(2).any(|pair| point(pair[0][pair[0].len() - 1]).dist(point(pair[1][0])) + 2. * error > tol)
        || point(spans.first()?.first().copied()?).dist(a) + error > tol
        || point(spans.last()?.last().copied()?).dist(b) + error > tol
    {
        return None;
    }
    if a.dist(b) > tol && spans.iter().all(|s| line_span(s, a, b, tol, error, work).is_some()) {
        return Some(vec![Seg2::Line { a, b }]);
    }
    arc(&c, &spans, plane, a, b, tol, error, work).or_else(|| free_form(&spans, a, b, tol))
}

/// One Bézier span that is neither a line nor an arc, as the exact free-form segment it is
/// (a font's quadratic or cubic, a conic), its ends `a` and `b`. `None` for several spans (a
/// curve that may wind about more than once), degrees past [`solidcraft_geom::MAX_BEZIER_DEGREE`],
/// rational spans other than conics, and control polygons with no area (a curve doubling back
/// along a line).
fn free_form(spans: &[Vec<H>], a: Vec2, b: Vec2, tol: f64) -> Option<Vec<Seg2>> {
    let [span] = spans else { return None };
    let pts: Vec<Vec2> = span.iter().map(|p| point(*p)).collect();
    let (first, last) = (*pts.first()?, *pts.last()?);
    if first.dist(a) > tol || last.dist(b) > tol || first.dist(last) <= tol {
        return None;
    }
    // Twice the area the control polygon spans off the chord: none is a line, maybe doubling back.
    let chord = last - first;
    let off = pts.iter().map(|p| chord.cross(*p - first).abs()).fold(0., f64::max) / chord.len();
    if !(off > tol) {
        return None;
    }
    let rational = span.iter().any(|p| (p[2] - 1.).abs() > 1e-12);
    let seg = match (pts.len(), rational) {
        (_, false) => solidcraft_geom::bezier(&pts)?,
        (3, true) => {
            // Weights normalised to 1 at the ends: the middle one is the conic's.
            let (w0, w1, w2) = (span[0][2], span[1][2], span[2][2]);
            let w = w1 / (w0 * w2).sqrt();
            if !(w.is_finite() && w > 1e-9) {
                return None;
            }
            Seg2::Conic { a: first, apex: pts[1], b: last, w }
        }
        _ => return None,
    };
    // The exact ends of the edge (the span's agree within the tolerance).
    let seg = match seg {
        Seg2::Cubic { p1, p2, .. } => Seg2::Cubic { p0: a, p1, p2, p3: b },
        Seg2::Conic { apex, w, .. } => Seg2::Conic { a, apex, b, w },
        Seg2::Bezier { n, mut p } => {
            *p.first_mut()? = a;
            *p.get_mut(n as usize)? = b;
            Seg2::Bezier { n, p }
        }
        other => other,
    };
    Some(vec![seg])
}

/// The spline `c` (its Bézier `spans` in the plane, from `a` to `b`) as a circular arc, when
/// whole-span proofs show it is one.
#[allow(clippy::too_many_arguments)]
fn arc(c: &mt::Curve, spans: &[Vec<H>], plane: &Plane, a: Vec2, b: Vec2, tol: f64, error: f64, work: &mut usize) -> Option<Vec<Seg2>> {
    let (t0, t1) = c.range_tuple();
    // Samples are only a candidate proposal. Whole-span implicit and oriented
    // angular proofs below are required before converting any spline to an arc.
    let q = plane.to_local(from_p3(c.subs(t0 + (t1 - t0) * 5. / 16.))) - a;
    let r = plane.to_local(from_p3(c.subs(t0 + (t1 - t0) * 11. / 16.))) - a;
    let det = 2. * (q.x * r.y - q.y * r.x);
    if !det.is_finite() || det.abs() < 1e-300 {
        return None;
    }
    let offset = Vec2::new((q.dot(q) * r.y - r.dot(r) * q.y) / det, (q.x * r.dot(r) - r.x * q.dot(q)) / det);
    let (center, radius) = (a + offset, offset.len());
    if !radius.is_finite() || radius <= tol || !center.x.is_finite() || !center.y.is_finite() {
        return None;
    }
    let mut sweep: f64 = 0.;
    for span in spans {
        let ds = circle_span(span, center, radius, tol, error, 0, work)?;
        if sweep != 0. && sweep.signum() != ds.signum() {
            return None;
        }
        sweep += ds;
    }
    let start = (a - center).angle();
    let end = center + Vec2::new((start + sweep).cos(), (start + sweep).sin()) * radius;
    // The arc ends where the curve does, as closely as a profile loop must close: within `tol`
    // only (which grows with the model), a curve that is merely near a circle (a font's) would
    // come back as an arc ending off the next segment's start, and the loop would not close.
    if end.dist(b) + error > tol.min(ARC_END_TOL) {
        return None;
    }
    let closed = a.dist(b) < tol;
    let angular_tol = tol / radius;
    if sweep.abs() > std::f64::consts::TAU + angular_tol || sweep.abs() < angular_tol {
        return None;
    }
    if closed {
        if (sweep.abs() - std::f64::consts::TAU).abs() > angular_tol {
            return None;
        }
        let h = sweep.signum() * std::f64::consts::PI;
        Some(vec![Seg2::Arc { center, radius, start, sweep: h }, Seg2::Arc { center, radius, start: start + h, sweep: h }])
    } else {
        Some(vec![Seg2::Arc { center, radius, start, sweep }])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use mt::builder;

    fn choose(n: usize, k: usize) -> f64 {
        (0..k).fold(1., |v, i| v * (n - i) as f64 / (i + 1) as f64)
    }
    fn bernstein(power: &[f64], degree: usize) -> Vec<f64> {
        (0..=degree).map(|i| (0..=i.min(power.len() - 1)).map(|k| power[k] * choose(i, k) / choose(degree, k)).sum()).collect()
    }
    fn multiply(a: &[f64], b: &[f64]) -> Vec<f64> {
        let mut out = vec![0.; a.len() + b.len() - 1];
        for (i, x) in a.iter().enumerate() {
            for (j, y) in b.iter().enumerate() {
                out[i + j] += x * y;
            }
        }
        out
    }
    fn trap() -> Vec<f64> {
        let mut p = vec![1.];
        for k in 0..=16 {
            p = multiply(&p, &[-(k as f64) / 16., 1.]);
        }
        let mid = (0..=16).map(|k| 1. / 32. - k as f64 / 16.).product::<f64>();
        p.iter_mut().for_each(|v| *v /= mid);
        p
    }
    fn edge(x: &[f64], y: &[f64], w: &[f64], degree: usize) -> mt::Edge {
        let (x, y, w) = (bernstein(x, degree), bernstein(y, degree), bernstein(w, degree));
        let controls = x.iter().zip(y).zip(w).map(|((x, y), w)| mt::Vector4::new(*x, y, 0., w)).collect();
        let c = mt::Curve::NurbsCurve(mt::NurbsCurve::new(mt::BSplineCurve::new(mt::KnotVec::bezier_knot(degree), controls)));
        let a = mt::Vertex::new(c.subs(0.));
        let b = mt::Vertex::new(c.subs(1.));
        mt::Edge::new(&a, &b, c)
    }

    #[test]
    fn sample_aliasing_does_not_certify_spline_caps() {
        let e = edge(&[0., 10.], &trap(), &[1.], 17);
        for k in 0..=16 {
            assert!(e.curve().subs(k as f64 / 16.).y.abs() < 1e-4, "old sample locations must see a line");
        }
        assert!((e.curve().subs(1. / 32.).y - 1.).abs() < 1e-6);
        assert!(segment(&e, &Plane::XY, 1e-3, &mut 2_000_000).is_none());
        let front = e.front();
        let back = e.back();
        let low_right = mt::Vertex::new(mt::Point3::new(10., -3., 0.));
        let low_left = mt::Vertex::new(mt::Point3::new(0., -3., 0.));
        let wire: mt::Wire =
            vec![e.clone(), builder::line(back, &low_right), builder::line(&low_right, &low_left), builder::line(&low_left, front)].into();
        let cap = builder::try_attach_plane(&[wire]).unwrap();
        let body = crate::body::Body::new(builder::tsweep(&cap, mt::Vector3::new(0., 0., 1.))).unwrap();
        assert!(body.validity().is_empty());
        let tool = crate::box_solid(solidcraft_geom::Vec3::new(-1., -4., 0.5), solidcraft_geom::Vec3::new(12., 7., 1.)).unwrap();
        assert!(
            crate::prism::prism_boolean(&body, &tool, crate::BoolOp::Cut).is_none(),
            "an actual extruded aliased spline cap must reach the generic route"
        );
        let q = trap();
        let mut perturb = q.iter().map(|v| v * 0.1).collect::<Vec<_>>();
        perturb[0] += 1.;
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let x = multiply(&[10., 20. * (w - 1.), 10. * (1. - 2. * w)], &perturb);
        let y = multiply(&[0., 20. * w, 10. * (1. - 2. * w)], &perturb);
        let circle = edge(&x, &y, &[1., 2. * (w - 1.), 2. * (1. - w)], 19);
        for k in 0..=16 {
            let p = circle.curve().subs(k as f64 / 16.);
            assert!((p.x.hypot(p.y) - 10.).abs() < 1e-3, "old sample locations must see a circle");
        }
        let p = circle.curve().subs(1. / 32.);
        assert!((p.x.hypot(p.y) - 11.).abs() < 1e-5);
        assert!(segment(&circle, &Plane::XY, 1e-3, &mut 2_000_000).is_none());
    }

    #[test]
    fn genuine_curves_and_oriented_traversal_are_certified() {
        let a = mt::Vertex::new(mt::Point3::new(10., 0., 0.));
        let b = mt::Vertex::new(mt::Point3::new(0., 10., 0.));
        let arc = builder::circle_arc(&a, &b, mt::Point3::new(10. / 2f64.sqrt(), 10. / 2f64.sqrt(), 0.));
        for e in [&arc, &arc.inverse()] {
            let result = segment(e, &Plane::XY, 1e-5, &mut 2_000_000).unwrap();
            let Seg2::Arc { center, radius, sweep, .. } = result[0] else { panic!("expected arc") };
            assert!(center.len() < 1e-8 && (radius - 10.).abs() < 1e-8);
            assert!((sweep.abs() - std::f64::consts::FRAC_PI_2).abs() < 1e-8);
            assert_eq!(sweep > 0., e.orientation());
        }
        let line = edge(&[0., 10.], &[0.], &[1.], 17);
        assert!(matches!(segment(&line, &Plane::XY, 1e-5, &mut 2_000_000).unwrap()[0], Seg2::Line { .. }));
        // The longitudinal derivative is negative at t=1/3, independently
        // establishing genuine retracing rather than a stationary tangent.
        let backtrack = edge(&[0., 10., -35., 35.], &[0.], &[1.], 3);
        assert!(backtrack.curve().der(1. / 3.).x < -1.);
        assert!(segment(&backtrack, &Plane::XY, 1e-5, &mut 2_000_000).is_none());
        assert!(segment(&arc, &Plane::XY, 1e-5, &mut 0).is_none());
    }

    /// A curve read as an arc ends where the curve does, however large the tolerance (it grows
    /// with the part): a font's quadratic that is within a micron of a circle, not on one, is
    /// kept as the curve it is. Read as an arc whose end is a micron off, the outline of a letter
    /// cut into a 2 m plate did not close.
    #[test]
    fn a_curve_near_a_circle_is_not_an_arc_ending_off_it() {
        // A parabola within a micron of the circle through its ends and middle.
        let e = edge(&[0., 2., 0.], &[0., 0., 0.2], &[1.], 2);
        let (a, b) = (Vec2::new(0., 0.), Vec2::new(2., 0.2));
        assert!(from_p3(e.back().point()).dist(solidcraft_geom::Vec3::new(2., 0.2, 0.)) < 1e-12);
        for tol in [1e-5, 2e-3] {
            let segs = segment(&e, &Plane::XY, tol, &mut 2_000_000).unwrap();
            let (first, last) = (segs.first().unwrap(), segs.last().unwrap());
            assert!(first.start().dist(a) < 1e-9 && last.end().dist(b) < 1e-9, "{tol}: {segs:?}");
        }
        // A true arc is still one at either tolerance.
        let (p, q) = (mt::Vertex::new(mt::Point3::new(10., 0., 0.)), mt::Vertex::new(mt::Point3::new(0., 10., 0.)));
        let arc = builder::circle_arc(&p, &q, mt::Point3::new(10. / 2f64.sqrt(), 10. / 2f64.sqrt(), 0.));
        for tol in [1e-5, 2e-3] {
            assert!(matches!(segment(&arc, &Plane::XY, tol, &mut 2_000_000).unwrap()[..], [Seg2::Arc { .. }]));
        }
    }

    #[test]
    fn discontinuous_and_multiple_winding_curves_decline() {
        let edge_from = |knots: Vec<f64>, points: Vec<mt::Vector4>| {
            let c = mt::Curve::NurbsCurve(mt::NurbsCurve::new(mt::BSplineCurve::new(mt::KnotVec::from(knots), points)));
            let (lo, hi) = c.range_tuple();
            mt::Edge::new(&mt::Vertex::new(c.subs(lo)), &mt::Vertex::new(c.subs(hi)), c)
        };
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let p = |x, y, w| mt::Vector4::new(x, y, 0., w);
        let disconnected = edge_from(
            vec![0., 0., 0., 1., 1., 1., 2., 2., 2.],
            vec![p(10., 0., 1.), p(10. * w, 10. * w, w), p(0., 10., 1.), p(-10., 0., 1.), p(-10. * w, -10. * w, w), p(0., -10., 1.)],
        );
        assert!(segment(&disconnected, &Plane::XY, 1e-5, &mut 2_000_000).is_none());
        let skipped_line = edge_from(vec![0., 0., 1., 1., 2., 2.], vec![p(0., 0., 1.), p(2., 0., 1.), p(5., 0., 1.), p(10., 0., 1.)]);
        assert!(segment(&skipped_line, &Plane::XY, 1e-5, &mut 2_000_000).is_none());
        let mut points = vec![p(10., 0., 1.)];
        let mut knots = vec![0., 0., 0.];
        for i in 0..8 {
            let a = i as f64 * std::f64::consts::FRAC_PI_2;
            let b = a + std::f64::consts::FRAC_PI_2;
            points.push(p(10. * ((a + b) / 2.).cos(), 10. * ((a + b) / 2.).sin(), w));
            points.push(p(10. * b.cos(), 10. * b.sin(), 1.));
            knots.extend([i as f64 + 1.; 2]);
        }
        knots.push(8.);
        let twice = edge_from(knots, points);
        assert!(segment(&twice, &Plane::XY, 1e-5, &mut 2_000_000).is_none());
    }
}
