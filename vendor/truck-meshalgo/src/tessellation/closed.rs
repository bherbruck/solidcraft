//! SolidCraft: a local periodic chart for geometrically closed spline surfaces.
//! The stored spline still has a finite knot domain; only meshing wraps its parameters.
use super::*;

#[derive(Clone, Debug)]
pub(super) struct Closed<'a, S> {
    pub base: &'a S,
    ranges: [Option<(f64, f64)>; 2],
    periods: [Option<f64>; 2],
    inferred: [bool; 2],
}

impl<'a, S: PreMeshableSurface> Closed<'a, S> {
    pub fn new(base: &'a S) -> Self {
        let (u, v) = base.try_range_tuple();
        let mut out = Self { base, ranges: [u, v], periods: [base.u_period(), base.v_period()], inferred: [false; 2] };
        let (Some((u0, u1)), Some((v0, v1))) = (u, v) else { return out };
        if ![u0, u1, v0, v1].iter().all(|v| v.is_finite()) || u1 <= u0 || v1 <= v0 {
            return out;
        }
        let origin = base.subs(u0, v0);
        let scale = (0..=4)
            .flat_map(|i| (0..=4).map(move |j| (i, j)))
            .map(|(i, j)| base.subs(u0 + (u1 - u0) * i as f64 / 4., v0 + (v1 - v0) * j as f64 / 4.).distance(origin))
            .fold(0.0_f64, f64::max);
        let tol2 = (scale * 1e-8).max(1e-9).powi(2);
        for d in 0..2 {
            if out.periods[d].is_some() {
                continue;
            }
            let (lo, hi, a, b) = if d == 0 { (u0, u1, v0, v1) } else { (v0, v1, u0, u1) };
            let mut noncollapsed = false;
            let closed = (0..=8).all(|k| {
                let t = a + (b - a) * k as f64 / 8.;
                let at = |x| if d == 0 { (x, t) } else { (t, x) };
                let (p, q, m) = (at(lo), at(hi), at((lo + hi) * 0.5));
                let first = base.subs(p.0, p.1);
                let (second, middle) = (base.subs(q.0, q.1), base.subs(m.0, m.1));
                if ![first.x, first.y, first.z, second.x, second.y, second.z, middle.x, middle.y, middle.z].iter().all(|x| x.is_finite()) {
                    return false;
                }
                noncollapsed |= first.distance2(middle) > tol2;
                if first.distance2(second) > tol2 {
                    return false;
                }
                let (n0, n1) = (base.normal(p.0, p.1), base.normal(q.0, q.1));
                !n0.magnitude2().is_finite() || !n1.magnitude2().is_finite() || n0.dot(n1) > 1.0 - 1e-6
            });
            if closed && noncollapsed {
                out.periods[d] = Some(hi - lo);
                out.inferred[d] = true;
            }
        }
        out
    }

    pub fn periods(&self) -> [Option<f64>; 2] {
        self.periods
    }

    pub fn wrap(&self, (u, v): (f64, f64)) -> (f64, f64) {
        let mut p = [u, v];
        for (d, x) in p.iter_mut().enumerate() {
            if let (true, Some((lo, hi)), Some(period)) = (self.inferred[d], self.ranges[d], self.periods[d]) {
                if *x < lo || *x > hi {
                    *x = lo + (*x - lo).rem_euclid(period);
                }
            }
        }
        (p[0], p[1])
    }
}

impl<S: PreMeshableSurface> ParametricSurface for Closed<'_, S> {
    type Point = Point3;
    type Vector = Vector3;
    fn subs(&self, u: f64, v: f64) -> Point3 {
        let (u, v) = self.wrap((u, v));
        self.base.subs(u, v)
    }
    fn uder(&self, u: f64, v: f64) -> Vector3 {
        let (u, v) = self.wrap((u, v));
        self.base.uder(u, v)
    }
    fn vder(&self, u: f64, v: f64) -> Vector3 {
        let (u, v) = self.wrap((u, v));
        self.base.vder(u, v)
    }
    fn uuder(&self, u: f64, v: f64) -> Vector3 {
        let (u, v) = self.wrap((u, v));
        self.base.uuder(u, v)
    }
    fn uvder(&self, u: f64, v: f64) -> Vector3 {
        let (u, v) = self.wrap((u, v));
        self.base.uvder(u, v)
    }
    fn vvder(&self, u: f64, v: f64) -> Vector3 {
        let (u, v) = self.wrap((u, v));
        self.base.vvder(u, v)
    }
    fn parameter_range(&self) -> (ParameterRange, ParameterRange) {
        self.base.parameter_range()
    }
    fn u_period(&self) -> Option<f64> {
        self.periods[0]
    }
    fn v_period(&self) -> Option<f64> {
        self.periods[1]
    }
}
impl<S: PreMeshableSurface> ParametricSurface3D for Closed<'_, S> {
    fn normal(&self, u: f64, v: f64) -> Vector3 {
        let (u, v) = self.wrap((u, v));
        self.base.normal(u, v)
    }
}
impl<S: PreMeshableSurface> ParameterDivision2D for Closed<'_, S> {
    fn parameter_division(&self, range: ((f64, f64), (f64, f64)), tol: f64) -> (Vec<f64>, Vec<f64>) {
        triangulation::bounded_surface_division(self, range, Deflection::chord(tol), triangulation::MAX_GRID)
    }
}
