//! Spline function spaces and the linear operators surface construction is built from: a clamped
//! B-spline space ([`Space`]), interpolation in it with derivative (Hermite) conditions
//! ([`Interpolant`]), carrying a spline into a larger space that contains it ([`embed`]: Bézier
//! degree elevation and Boehm knot insertion, exact), and a monotone piecewise cubic through
//! increasing data ([`Monotone`], for reparameterising curves).
//!
//! These are the pieces that make several splines *compatible* (one degree, one knot vector) so
//! that their control points can be combined linearly: lofting, Coons and Gordon surfaces, and
//! cross-boundary derivative conditions all reduce to them.
//!
//! References: Piegl & Tiller, *The NURBS Book* (2nd ed.) — §2.2–2.3 (basis functions and
//! their derivatives), §5.2 (knot insertion), §5.5 (degree elevation; the Bézier case), §9.2.1
//! (global interpolation, knots by averaging the parameters) and §9.2.2 (interpolation with
//! end derivatives); Fritsch & Carlson 1980, "Monotone piecewise cubic interpolation" (SIAM J.
//! Numer. Anal. 17), with the weighted harmonic-mean slopes of Fritsch & Butland 1984.
//!
//! Input is hostile: constructors check sizes and finiteness and return [`SplineError`]; no
//! function here panics.

use crate::Vec3;
use crate::bspline::{HPoint, MAX_SPLINE_DEGREE, Side, SplineError, SplineResult, check_knots, find_span, insert_knot, invalid, local_basis};

/// Most basis functions a space used for interpolation may have (its systems are solved densely).
pub const MAX_SPACE_DIMENSION: usize = 2048;

/// Values a spline can carry: numbers and points (vectors).
pub trait Linear: Copy {
    fn zero() -> Self;
    fn plus(self, o: Self) -> Self;
    fn times(self, s: f64) -> Self;
    /// The value as a homogeneous point of weight one (knot insertion works on those).
    fn to_h(self) -> HPoint;
    fn from_h(h: HPoint) -> Self;
    fn is_finite(self) -> bool;
}

impl Linear for f64 {
    fn zero() -> Self {
        0.0
    }
    fn plus(self, o: Self) -> Self {
        self + o
    }
    fn times(self, s: f64) -> Self {
        self * s
    }
    fn to_h(self) -> HPoint {
        HPoint { x: self, y: 0.0, z: 0.0, w: 1.0 }
    }
    fn from_h(h: HPoint) -> Self {
        h.x
    }
    fn is_finite(self) -> bool {
        f64::is_finite(self)
    }
}

impl Linear for Vec3 {
    fn zero() -> Self {
        Vec3::ZERO
    }
    fn plus(self, o: Self) -> Self {
        self + o
    }
    fn times(self, s: f64) -> Self {
        self * s
    }
    fn to_h(self) -> HPoint {
        HPoint::weighted(self, 1.0)
    }
    fn from_h(h: HPoint) -> Self {
        h.xyz()
    }
    fn is_finite(self) -> bool {
        Vec3::is_finite(self)
    }
}

/// A clamped B-spline space: a degree and a knot vector whose first and last `degree + 1` knots
/// are the domain's ends, interior knots repeated at most `degree` times (the space's splines
/// are continuous).
#[derive(Clone, Debug, PartialEq)]
pub struct Space {
    degree: usize,
    knots: Vec<f64>,
}

impl Space {
    pub fn new(degree: usize, knots: Vec<f64>) -> SplineResult<Space> {
        if degree == 0 || degree > MAX_SPLINE_DEGREE {
            return Err(SplineError::Unsupported(format!("degree {degree} (1 to {MAX_SPLINE_DEGREE})")));
        }
        check_knots(&knots)?;
        let n = knots.len().checked_sub(degree + 1).filter(|n| *n > degree).ok_or_else(|| SplineError::Invalid("too few knots".into()))?;
        if n > MAX_SPACE_DIMENSION {
            return Err(SplineError::TooLarge(format!("{n} basis functions (at most {MAX_SPACE_DIMENSION})")));
        }
        let (a, b) = (knots[0], knots[knots.len() - 1]);
        if !(a < b) || knots[..=degree].iter().any(|k| *k != a) || knots[n..].iter().any(|k| *k != b) {
            return invalid("the knot vector must be clamped on a non-empty domain");
        }
        let inner = &knots[degree + 1..n];
        let mut run = 0;
        for w in inner.windows(2) {
            run = if w[0] == w[1] { run + 1 } else { 0 };
            if run + 1 > degree {
                return invalid("an interior knot is repeated more often than the degree");
            }
        }
        if inner.first().is_some_and(|k| *k == a) || inner.last().is_some_and(|k| *k == b) {
            return invalid("an interior knot lies on a domain end");
        }
        Ok(Space { degree, knots })
    }

    /// Polynomials of `degree` on `[a, b]` (one Bézier piece).
    pub fn polynomial(degree: usize, a: f64, b: f64) -> SplineResult<Space> {
        let mut knots = vec![a; degree + 1];
        knots.extend(std::iter::repeat_n(b, degree + 1));
        Space::new(degree, knots)
    }

    /// Splines of `degree` with a simple knot at every interior break: `breaks` increase
    /// strictly, the first and last are the domain's ends.
    pub fn with_breaks(degree: usize, breaks: &[f64]) -> SplineResult<Space> {
        let (Some(&a), Some(&b)) = (breaks.first(), breaks.last()) else { return invalid("no breaks") };
        if breaks.len() > MAX_SPACE_DIMENSION || breaks.windows(2).any(|w| !(w[0] < w[1])) {
            return invalid("the breaks must increase");
        }
        let mut knots = vec![a; degree + 1];
        knots.extend(breaks.iter().skip(1).take(breaks.len().saturating_sub(2)).copied());
        knots.extend(std::iter::repeat_n(b, degree + 1));
        Space::new(degree, knots)
    }

    pub fn degree(&self) -> usize {
        self.degree
    }

    pub fn knots(&self) -> &[f64] {
        &self.knots
    }

    /// The number of basis functions (control points of a spline in the space).
    pub fn count(&self) -> usize {
        self.knots.len() - self.degree - 1
    }

    /// The domain `[a, b]`.
    pub fn domain(&self) -> [f64; 2] {
        [self.knots[0], self.knots[self.knots.len() - 1]]
    }

    /// The `der`-th derivatives at `t` (clamped to the domain) of the basis functions that do not
    /// vanish there: the index of the first one and their values.
    pub fn basis(&self, t: f64, der: usize) -> SplineResult<(usize, Vec<f64>)> {
        let [a, b] = self.domain();
        if !t.is_finite() {
            return invalid("a parameter is not a finite number");
        }
        let t = t.clamp(a, b);
        let (p, n) = (self.degree, self.count());
        let span = find_span(&self.knots, p, n, t).ok_or_else(|| SplineError::Invalid(format!("{t} is outside the domain")))?;
        let side = if t >= b { Side::Left } else { Side::Right };
        let first = span - p;
        let values =
            (first..=span).map(|i| self.knots.get(i..i + p + 2).map_or(0.0, |k| local_basis(k, t, side, der).get(der).copied().unwrap_or(0.0)));
        Ok((first, values.collect()))
    }

    /// The `der`-th derivative at `t` of the spline with control values `ctrl`.
    pub fn eval<T: Linear>(&self, ctrl: &[T], t: f64, der: usize) -> SplineResult<T> {
        if ctrl.len() != self.count() {
            return invalid(format!("{} control values for {} basis functions", ctrl.len(), self.count()));
        }
        let (first, values) = self.basis(t, der)?;
        Ok(values.iter().zip(ctrl.iter().skip(first)).fold(T::zero(), |acc, (n, c)| acc.plus(c.times(*n))))
    }

    /// The splines of this space with `breaks` added as simple knots (those already knots are
    /// left as they are). The result contains this space.
    pub fn refined(&self, breaks: &[f64]) -> SplineResult<Space> {
        let [a, b] = self.domain();
        let mut knots = self.knots.clone();
        for &x in breaks {
            if x > a && x < b && !knots.contains(&x) {
                knots.push(x);
            }
        }
        knots.sort_by(f64::total_cmp);
        Space::new(self.degree, knots)
    }
}

/// A condition on a spline: the value (`der` = 0) or a derivative at `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cond {
    pub t: f64,
    pub der: usize,
}

/// Interpolation in a space: the spline whose values and derivatives at the conditions are given
/// (as many conditions as the space has basis functions). The system is factored once; each
/// [`Interpolant::fit`] solves it for new data, so one interpolant serves many curves and the
/// rows of a surface alike (the operator is linear).
#[derive(Clone, Debug)]
pub struct Interpolant {
    space: Space,
    conds: Vec<Cond>,
    lu: Lu,
}

impl Interpolant {
    pub fn new(space: Space, conds: Vec<Cond>) -> SplineResult<Interpolant> {
        let n = space.count();
        if conds.len() != n {
            return invalid(format!("{} conditions for {n} basis functions", conds.len()));
        }
        let mut a = vec![0.0; n * n];
        for (r, c) in conds.iter().enumerate() {
            let (first, values) = space.basis(c.t, c.der)?;
            for (k, v) in values.iter().enumerate() {
                if let Some(x) = a.get_mut(r * n + first + k) {
                    *x = *v;
                }
            }
        }
        let lu = Lu::new(n, a).map_err(|_| SplineError::Invalid("the interpolation conditions do not determine a spline".into()))?;
        Ok(Interpolant { space, conds, lu })
    }

    /// Interpolation by a single polynomial piece of the lowest degree that meets `conds` (degree
    /// one less than their number, at least one: a lone value is held constant), on `[a, b]`;
    /// with more than four conditions, a cubic spline whose interior knots average the
    /// conditions' parameters (taken as a sequence with one entry per condition, NURBS Book eq.
    /// 9.8 applied to that confluent sequence, which keeps every basis function's support over
    /// the condition it is matched with).
    pub fn lowest(conds: Vec<Cond>, a: f64, b: f64) -> SplineResult<Interpolant> {
        let mut conds = conds;
        if conds.iter().any(|c| !c.t.is_finite() || c.t < a || c.t > b) {
            return invalid("an interpolation parameter is outside the domain");
        }
        if conds.len() == 1
            && let Some(c) = conds.first().copied()
        {
            // A constant: the value and a zero slope (the caller supplies zero data for it).
            conds.push(Cond { t: c.t, der: 1 });
        }
        let k = conds.len();
        if k <= 4 {
            return Interpolant::new(Space::polynomial(k.saturating_sub(1).max(1), a, b)?, conds);
        }
        let mut tau: Vec<f64> = conds.iter().map(|c| c.t).collect();
        tau.sort_by(f64::total_cmp);
        let mut breaks = vec![a];
        for j in 1..k - 3 {
            breaks.push((tau[j] + tau[j + 1] + tau[j + 2]) / 3.0);
        }
        breaks.push(b);
        if breaks.windows(2).any(|w| !(w[0] < w[1])) {
            return invalid("the interpolation parameters repeat too often");
        }
        Interpolant::new(Space::with_breaks(3, &breaks)?, conds)
    }

    pub fn space(&self) -> &Space {
        &self.space
    }

    pub fn conds(&self) -> &[Cond] {
        &self.conds
    }

    /// The control values of the spline meeting the conditions with `data` (one per condition).
    pub fn fit<T: Linear>(&self, data: &[T]) -> SplineResult<Vec<T>> {
        if data.len() != self.conds.len() {
            return invalid(format!("{} values for {} conditions", data.len(), self.conds.len()));
        }
        let out = self.lu.solve(data);
        if out.iter().any(|x| !x.is_finite()) {
            return invalid("interpolation did not give finite control values");
        }
        Ok(out)
    }
}

/// An LU factorisation with partial pivoting of a dense square matrix (row-major).
#[derive(Clone, Debug)]
struct Lu {
    n: usize,
    a: Vec<f64>,
    piv: Vec<usize>,
}

impl Lu {
    fn new(n: usize, mut a: Vec<f64>) -> SplineResult<Lu> {
        if a.len() != n * n {
            return invalid("matrix size");
        }
        let scale = a.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        if !(scale > 0.0 && scale.is_finite()) {
            return invalid("singular system");
        }
        let mut piv = vec![0; n];
        for k in 0..n {
            let (mut best, mut big) = (k, 0.0);
            for r in k..n {
                let x = a[r * n + k].abs();
                if x > big {
                    (best, big) = (r, x);
                }
            }
            if !(big > scale * 1e-13) {
                return invalid("singular system");
            }
            piv[k] = best;
            if best != k {
                for c in 0..n {
                    a.swap(k * n + c, best * n + c);
                }
            }
            let d = a[k * n + k];
            for r in k + 1..n {
                let f = a[r * n + k] / d;
                a[r * n + k] = f;
                if f != 0.0 {
                    for c in k + 1..n {
                        a[r * n + c] -= f * a[k * n + c];
                    }
                }
            }
        }
        Ok(Lu { n, a, piv })
    }

    fn solve<T: Linear>(&self, b: &[T]) -> Vec<T> {
        let n = self.n;
        let mut x = b.to_vec();
        if x.len() != n {
            return x;
        }
        for (k, &p) in self.piv.iter().enumerate() {
            x.swap(k, p);
        }
        for r in 0..n {
            let row = &self.a[r * n..r * n + r];
            let acc = row.iter().zip(&x[..r]).fold(x[r], |acc, (l, xc)| acc.plus(xc.times(-l)));
            x[r] = acc;
        }
        for r in (0..n).rev() {
            let row = &self.a[r * n + r + 1..(r + 1) * n];
            let acc = row.iter().zip(&x[r + 1..]).fold(x[r], |acc, (u, xc)| acc.plus(xc.times(-u)));
            x[r] = acc.times(1.0 / self.a[r * n + r]);
        }
        x
    }
}

/// The control values in `to` of the spline with control values `ctrl` in `from`, which `to`
/// contains: the same domain, a degree at least as high, and every knot of `from` among those of
/// `to` (with a polynomial `from` when the degree rises). Exact: Bézier degree elevation (NURBS
/// Book §5.5) to the target degree, then Boehm knot insertion (§5.2) of the knots `to` adds.
pub fn embed<T: Linear>(from: &Space, ctrl: &[T], to: &Space) -> SplineResult<Vec<T>> {
    if ctrl.len() != from.count() {
        return invalid("control values do not match the space");
    }
    let ([a0, b0], [a1, b1]) = (from.domain(), to.domain());
    if a0 != a1 || b0 != b1 {
        return invalid("the spaces' domains differ");
    }
    let mut p = from.degree();
    let mut knots = from.knots().to_vec();
    let mut pts: Vec<T> = ctrl.to_vec();
    if p > to.degree() {
        return invalid("the target space's degree is lower");
    }
    if p < to.degree() {
        if from.count() != p + 1 {
            return Err(SplineError::Unsupported("raising the degree of a spline with interior knots".into()));
        }
        while p < to.degree() {
            pts = elevate_bezier(&pts);
            p += 1;
        }
        knots = vec![a0; p + 1];
        knots.extend(std::iter::repeat_n(b0, p + 1));
    }
    // The knots `to` adds: a merge of the two sorted vectors.
    let target = to.knots();
    let mut missing = Vec::new();
    let mut j = 0;
    for &x in target {
        match knots.get(j) {
            Some(&k) if k == x => j += 1,
            Some(&k) if k < x => return invalid("a knot of the spline is not in the target space"),
            _ => missing.push(x),
        }
    }
    if j != knots.len() {
        return invalid("a knot of the spline is not in the target space");
    }
    let mut h: Vec<HPoint> = pts.iter().map(|x| x.to_h()).collect();
    for x in missing {
        (knots, h) = insert_knot(p, &knots, &h, x)?;
    }
    if knots.as_slice() != target {
        return invalid("knot insertion did not reach the target space");
    }
    Ok(h.into_iter().map(T::from_h).collect())
}

/// A Bézier piece's control values one degree higher, the same polynomial (NURBS Book eq. 5.36):
/// `Qᵢ = (i/(p+1))·Pᵢ₋₁ + (1 − i/(p+1))·Pᵢ`.
fn elevate_bezier<T: Linear>(pts: &[T]) -> Vec<T> {
    let p = pts.len().saturating_sub(1);
    let at = |i: usize| pts.get(i).copied().unwrap_or_else(T::zero);
    (0..=p + 1)
        .map(|i| {
            let a = i as f64 / (p + 1) as f64;
            let prev = if i == 0 { T::zero() } else { at(i - 1).times(a) };
            prev.plus(at(i).times(1.0 - a))
        })
        .collect()
}

/// A monotone piecewise cubic Hermite function through `(xₖ, yₖ)`, both strictly increasing
/// (Fritsch & Carlson): slopes are the weighted harmonic means of the neighbouring secants
/// (Fritsch & Butland), the secant at the ends. Its slope is positive everywhere, and through two
/// points it is the straight line.
#[derive(Clone, Debug, PartialEq)]
pub struct Monotone {
    x: Vec<f64>,
    y: Vec<f64>,
    m: Vec<f64>,
}

impl Monotone {
    pub fn new(x: Vec<f64>, y: Vec<f64>) -> SplineResult<Monotone> {
        let n = x.len();
        if n < 2 || y.len() != n {
            return invalid("a monotone cubic needs two points or more");
        }
        let increasing = |v: &[f64]| v.iter().all(|t| t.is_finite()) && v.windows(2).all(|w| w[0] < w[1]);
        if !increasing(&x) || !increasing(&y) {
            return invalid("the points of a monotone cubic must increase");
        }
        let h: Vec<f64> = x.windows(2).map(|w| w[1] - w[0]).collect();
        let d: Vec<f64> = y.windows(2).zip(&h).map(|(w, h)| (w[1] - w[0]) / h).collect();
        let mut m = Vec::with_capacity(n);
        m.push(d[0]);
        for k in 1..n - 1 {
            let (w1, w2) = (2.0 * h[k] + h[k - 1], h[k] + 2.0 * h[k - 1]);
            m.push((w1 + w2) / (w1 / d[k - 1] + w2 / d[k]));
        }
        m.push(d[n - 2]);
        Ok(Monotone { x, y, m })
    }

    /// The value and the first two derivatives at `t` (clamped to the points' range).
    pub fn eval(&self, t: f64) -> [f64; 3] {
        let n = self.x.len();
        let t = t.clamp(self.x[0], self.x[n - 1]);
        let k = self.x.partition_point(|x| *x <= t).saturating_sub(1).min(n - 2);
        let (x0, x1, y0, y1, m0, m1) = (self.x[k], self.x[k + 1], self.y[k], self.y[k + 1], self.m[k], self.m[k + 1]);
        let h = x1 - x0;
        let s = (t - x0) / h;
        let (s2, s3) = (s * s, s * s * s);
        let value = (2.0 * s3 - 3.0 * s2 + 1.0) * y0 + (s3 - 2.0 * s2 + s) * h * m0 + (-2.0 * s3 + 3.0 * s2) * y1 + (s3 - s2) * h * m1;
        let d1 = ((6.0 * s2 - 6.0 * s) * y0 + (3.0 * s2 - 4.0 * s + 1.0) * h * m0 + (-6.0 * s2 + 6.0 * s) * y1 + (3.0 * s2 - 2.0 * s) * h * m1) / h;
        let d2 = ((12.0 * s - 6.0) * y0 + (6.0 * s - 4.0) * h * m0 + (-12.0 * s + 6.0) * y1 + (6.0 * s - 2.0) * h * m1) / (h * h);
        [value, d1, d2]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-10
    }

    /// Hermite interpolation reproduces a cubic from its values and end derivatives, and the
    /// lowest-degree interpolant through two values is the straight line.
    #[test]
    fn interpolation_meets_values_and_derivatives() {
        let f = |t: f64| 1.0 + 2.0 * t - 3.0 * t * t + 0.5 * t * t * t;
        let df = |t: f64| 2.0 - 6.0 * t + 1.5 * t * t;
        let d2f = |t: f64| -6.0 + 3.0 * t;
        let conds = vec![Cond { t: 0.0, der: 0 }, Cond { t: 0.0, der: 1 }, Cond { t: 0.0, der: 2 }, Cond { t: 2.0, der: 0 }];
        let it = Interpolant::lowest(conds, 0.0, 2.0).unwrap();
        let c = it.fit(&[f(0.0), df(0.0), d2f(0.0), f(2.0)]).unwrap();
        for k in 0..=10 {
            let t = k as f64 * 0.2;
            assert!(close(it.space().eval(&c, t, 0).unwrap(), f(t)), "{t}");
        }
        let line = Interpolant::lowest(vec![Cond { t: 0.0, der: 0 }, Cond { t: 1.0, der: 0 }], 0.0, 1.0).unwrap();
        let c = line.fit(&[2.0, 5.0]).unwrap();
        assert!(close(line.space().eval(&c, 0.25, 0).unwrap(), 2.75));
        let constant = Interpolant::lowest(vec![Cond { t: 0.3, der: 0 }], 0.0, 1.0).unwrap();
        let c = constant.fit(&[4.0, 0.0]).unwrap();
        assert!(close(constant.space().eval(&c, 0.9, 0).unwrap(), 4.0));
    }

    /// Many conditions: a cubic spline with averaged knots through samples of a smooth function
    /// with derivative conditions at both ends (values exact at the samples).
    #[test]
    fn spline_interpolation_with_end_derivatives() {
        let f = |t: f64| (2.0 * t).sin();
        let df = |t: f64| 2.0 * (2.0 * t).cos();
        let ts: Vec<f64> = (0..=8).map(|k| k as f64 / 8.0).collect();
        let mut conds: Vec<Cond> = ts.iter().map(|&t| Cond { t, der: 0 }).collect();
        conds.push(Cond { t: 0.0, der: 1 });
        conds.push(Cond { t: 1.0, der: 1 });
        let it = Interpolant::lowest(conds, 0.0, 1.0).unwrap();
        let mut data: Vec<f64> = ts.iter().map(|&t| f(t)).collect();
        data.push(df(0.0));
        data.push(df(1.0));
        let c = it.fit(&data).unwrap();
        for &t in &ts {
            assert!(close(it.space().eval(&c, t, 0).unwrap(), f(t)));
        }
        assert!(close(it.space().eval(&c, 0.0, 1).unwrap(), df(0.0)));
        assert!(close(it.space().eval(&c, 1.0, 1).unwrap(), df(1.0)));
        assert!((it.space().eval(&c, 0.53, 0).unwrap() - f(0.53)).abs() < 1e-4);
    }

    /// Embedding is exact: a quadratic polynomial raised to a cubic with knots inserted, and a
    /// cubic spline refined, evaluate the same everywhere; a space that does not contain the
    /// spline is refused.
    #[test]
    fn embedding_keeps_the_spline() {
        let from = Space::polynomial(2, 0.0, 1.0).unwrap();
        let ctrl = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 2.0, 0.0), Vec3::new(3.0, -1.0, 1.0)];
        let to = Space::with_breaks(3, &[0.0, 0.2, 0.5, 0.7, 1.0]).unwrap();
        let out = embed(&from, &ctrl, &to).unwrap();
        let spline = Space::with_breaks(3, &[0.0, 0.5, 1.0]).unwrap();
        let sctrl: Vec<f64> = (0..spline.count()).map(|i| (i as f64 * 1.7).sin()).collect();
        let finer = spline.refined(&[0.25, 0.5, 0.8]).unwrap();
        let sout = embed(&spline, &sctrl, &finer).unwrap();
        for k in 0..=20 {
            let t = k as f64 / 20.0;
            assert!(from.eval(&ctrl, t, 0).unwrap().dist(to.eval(&out, t, 0).unwrap()) < 1e-12);
            assert!(close(spline.eval(&sctrl, t, 0).unwrap(), finer.eval(&sout, t, 0).unwrap()));
        }
        assert!(embed(&finer, &sout, &spline).is_err(), "a coarser space");
        assert!(embed(&spline, &sctrl, &Space::with_breaks(3, &[0.0, 0.4, 1.0]).unwrap()).is_err(), "a knot missing");
        assert!(embed(&spline, &sctrl, &Space::with_breaks(3, &[0.0, 0.5, 2.0]).unwrap()).is_err(), "another domain");
    }

    #[test]
    fn monotone_cubic_is_monotone_and_straight_through_two_points() {
        let m = Monotone::new(vec![0.0, 0.3, 0.5, 1.0], vec![0.0, 0.05, 0.9, 1.0]).unwrap();
        let mut last = -1.0;
        for k in 0..=200 {
            let [y, d, _] = m.eval(k as f64 / 200.0);
            assert!(y >= last && d >= 0.0, "{k}");
            last = y;
        }
        assert!(close(m.eval(0.3)[0], 0.05) && close(m.eval(0.5)[0], 0.9));
        let line = Monotone::new(vec![0.0, 1.0], vec![2.0, 4.0]).unwrap();
        assert!(close(line.eval(0.25)[0], 2.5) && close(line.eval(0.7)[1], 2.0) && close(line.eval(0.7)[2], 0.0));
        assert!(Monotone::new(vec![0.0, 0.0], vec![0.0, 1.0]).is_err());
        assert!(Monotone::new(vec![0.0, 1.0], vec![1.0, 0.0]).is_err());
        assert!(Monotone::new(vec![0.0, f64::NAN], vec![0.0, 1.0]).is_err());
    }

    #[test]
    fn hostile_spaces_are_refused() {
        assert!(Space::new(3, vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]).is_ok());
        assert!(Space::new(3, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]).is_err(), "too few knots");
        assert!(Space::new(3, vec![0.0, 0.0, 0.0, 0.1, 1.0, 1.0, 1.0, 1.0]).is_err(), "not clamped");
        assert!(Space::new(1, vec![0.0, 0.0, 0.5, 0.5, 1.0, 1.0]).is_err(), "a discontinuous space");
        assert!(Space::new(0, vec![0.0, 1.0]).is_err());
        assert!(Space::with_breaks(3, &[0.0, 0.5, 0.5, 1.0]).is_err());
        assert!(Space::with_breaks(3, &[0.0, f64::NAN, 1.0]).is_err());
        let s = Space::with_breaks(3, &[0.0, 1.0]).unwrap();
        assert!(s.eval(&[1.0, 2.0], 0.5, 0).is_err(), "wrong control count");
        assert!(s.basis(f64::NAN, 0).is_err());
        // Repeated conditions do not determine a spline.
        assert!(Interpolant::new(s.clone(), vec![Cond { t: 0.5, der: 0 }; 4]).is_err());
        assert!(Interpolant::lowest(vec![Cond { t: 2.0, der: 0 }], 0.0, 1.0).is_err());
    }
}
