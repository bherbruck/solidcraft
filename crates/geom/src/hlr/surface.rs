//! Surfaces of portable B-rep faces with a parameter chart, inversion, ray intersection and
//! analytic recognition. Planes are closed form; revolutions are reduced to their 2D profile;
//! spline surfaces use Newton iterations seeded from a sample grid.

use super::HlrError;
use super::curve::{C3, as_circle, frame_axes, v3};
use crate::Vec3;
use crate::brep::{Frame, Surface};
use crate::bspline::{HPoint, NurbsSurface};

const TAU: f64 = std::f64::consts::TAU;

#[derive(Clone, Debug)]
pub(crate) struct Rev {
    o: Vec3,
    pub a: Vec3,
    e1: Vec3,
    e2: Vec3,
    profile: C3,
    dom: [f64; 2],
    theta: [f64; 2],
    closed: bool,
    /// Profile samples `[s, r, h]`.
    samples: Vec<[f64; 3]>,
    pub rmax: f64,
    hrange: [f64; 2],
}

#[derive(Clone, Debug)]
pub(crate) struct Nurbs {
    s: NurbsSurface,
    dom: [[f64; 2]; 2],
    g: usize,
    /// `(g+1)²` samples, u fastest.
    grid: Vec<Vec3>,
}

#[derive(Clone, Debug)]
pub(crate) enum S3 {
    Plane { o: Vec3, u: Vec3, v: Vec3, n: Vec3 },
    Rev(Box<Rev>),
    Nurbs(Box<Nurbs>),
}

/// What a surface is, when it is a quadric or torus regardless of its record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Analytic {
    Plane,
    /// A point on the axis, unit axis, radius.
    Cylinder {
        p: Vec3,
        a: Vec3,
        r: f64,
    },
    /// Apex, unit axis (opening direction), half angle.
    Cone {
        apex: Vec3,
        a: Vec3,
        half: f64,
    },
    Sphere {
        c: Vec3,
        r: f64,
    },
    Torus {
        c: Vec3,
        a: Vec3,
        major: f64,
        minor: f64,
    },
    Other,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Hit {
    pub lam: f64,
    pub uv: [f64; 2],
    pub grazing: bool,
}

fn perp(a: Vec3) -> Vec3 {
    a.any_perp().normalized().unwrap_or(Vec3::new(1., 0., 0.))
}

impl S3 {
    pub fn new(s: &Surface) -> Result<S3, HlrError> {
        Ok(match s {
            Surface::Plane { origin, u_axis, v_axis } => {
                let (u, v) = (v3(*u_axis), v3(*v_axis));
                let n = u.cross(v).normalized().ok_or(HlrError::Unsupported("degenerate plane"))?;
                S3::Plane { o: v3(*origin), u, v, n }
            }
            Surface::Spline { u_knots, v_knots, points } => {
                let h = points.iter().map(|r| r.iter().map(|p| [p[0], p[1], p[2], 1.]).collect()).collect::<Vec<Vec<_>>>();
                S3::Nurbs(Box::new(Nurbs::new(u_knots, v_knots, &h)?))
            }
            Surface::RationalSpline { u_knots, v_knots, homogeneous } => S3::Nurbs(Box::new(Nurbs::new(u_knots, v_knots, homogeneous)?)),
            Surface::Revolution { profile, origin, axis, angle } => {
                let (c, _) = C3::new(profile)?;
                S3::Rev(Box::new(Rev::new(v3(*origin), v3(*axis), None, c, *angle)?))
            }
            Surface::Cylinder { frame, radius, angle, height } => {
                let (o, x, _, z) = axes(frame);
                let line = C3::Line { a: o + x * *radius + z * height[0], b: o + x * *radius + z * height[1] };
                S3::Rev(Box::new(Rev::new(o, z, Some(x), line, *angle)?))
            }
            Surface::Cone { frame, radius_at_origin, half_angle, angle, height } => {
                let (o, x, _, z) = axes(frame);
                let r = |h: f64| radius_at_origin + h * half_angle.tan();
                let line = C3::Line { a: o + x * r(height[0]) + z * height[0], b: o + x * r(height[1]) + z * height[1] };
                S3::Rev(Box::new(Rev::new(o, z, Some(x), line, *angle)?))
            }
            Surface::Sphere { frame, radius, longitude, latitude } => {
                let (o, x, _, z) = axes(frame);
                let arc = C3::Arc { o, x, y: z, r: [*radius, *radius], range: *latitude };
                S3::Rev(Box::new(Rev::new(o, z, Some(x), arc, *longitude)?))
            }
            Surface::Torus { frame, major_radius, minor_radius, longitude, meridian } => {
                let (o, x, _, z) = axes(frame);
                let arc = C3::Arc { o: o + x * *major_radius, x, y: z, r: [*minor_radius, *minor_radius], range: *meridian };
                S3::Rev(Box::new(Rev::new(o, z, Some(x), arc, *longitude)?))
            }
        })
    }

    /// The recognised quadric or torus as a revolution chart: closed-form inversion and exact
    /// points for trimming, whatever record (spline, rational spline) the face was stored as.
    /// `h` bounds the axial extent of cylinders and cones (the face's boundary along the axis);
    /// `toward` points from a sphere's centre to the face.
    pub fn from_analytic(an: &Analytic, h: [f64; 2], toward: Vec3) -> Option<S3> {
        let rev = match *an {
            Analytic::Cylinder { p, a, r } => {
                let e1 = perp(a);
                Rev::new(p, a, Some(e1), C3::Line { a: p + e1 * r + a * h[0], b: p + e1 * r + a * h[1] }, [0., TAU])
            }
            Analytic::Cone { apex, a, half } => {
                let e1 = perp(a);
                let at = |x: f64| apex + a * x + e1 * (x * half.tan());
                Rev::new(apex, a, Some(e1), C3::Line { a: at(h[0].max(0.)), b: at(h[1].max(0.)) }, [0., TAU])
            }
            Analytic::Sphere { c, r } => {
                // An axis square to the patch's direction keeps the poles off the patch.
                let a = perp(toward);
                let e1 = toward.normalized().unwrap_or_else(|| perp(a));
                let half = std::f64::consts::FRAC_PI_2;
                Rev::new(c, a, Some(e1), C3::Arc { o: c, x: e1, y: a, r: [r, r], range: [-half, half] }, [0., TAU])
            }
            Analytic::Torus { c, a, major, minor } => {
                let e1 = perp(a);
                Rev::new(c, a, Some(e1), C3::Arc { o: c + e1 * major, x: e1, y: a, r: [minor, minor], range: [0., TAU] }, [0., TAU])
            }
            Analytic::Plane | Analytic::Other => return None,
        };
        rev.ok().map(|r| S3::Rev(Box::new(r)))
    }

    /// A box that contains the whole surface for certain: the control-point hull of a spline
    /// (weights are positive), the turned profile's hull for a revolution; `None` for planes.
    pub fn bound(&self) -> Option<(Vec3, Vec3)> {
        let hull = |pts: &mut dyn Iterator<Item = Vec3>| {
            pts.fold(None, |acc: Option<(Vec3, Vec3)>, p| Some(acc.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p)))))
        };
        match self {
            S3::Plane { .. } => None,
            S3::Nurbs(n) => hull(&mut n.s.homogeneous().iter().filter_map(|h| h.project())),
            S3::Rev(r) => {
                // Radial distance is convex, so its maximum over the profile's hull is at a vertex.
                let pts: Vec<Vec3> = match &r.profile {
                    C3::Line { a, b } => vec![*a, *b],
                    C3::Polyline(p) => p.clone(),
                    C3::Spline { points, .. } => {
                        points.iter().filter(|h| h[3] > 0.).map(|h| Vec3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3])).collect()
                    }
                    C3::Arc { o, x, y, r: rad, .. } => {
                        // The whole circle's bounding square in its plane.
                        let m = rad[0].max(rad[1]);
                        vec![*o + *x * m + *y * m, *o + *x * m - *y * m, *o - *x * m + *y * m, *o - *x * m - *y * m]
                    }
                };
                let (mut rmax, mut h0, mut h1) = (0f64, f64::MAX, f64::MIN);
                for p in &pts {
                    let w = *p - r.o;
                    let h = w.dot(r.a);
                    rmax = rmax.max((w - r.a * h).len());
                    h0 = h0.min(h);
                    h1 = h1.max(h);
                }
                if !(h1 >= h0) {
                    return None;
                }
                let (c0, c1) = (r.o + r.a * h0, r.o + r.a * h1);
                let ext = |k: f64| rmax * (1. - k * k).max(0.).sqrt();
                let e = Vec3::new(ext(r.a.x), ext(r.a.y), ext(r.a.z));
                Some((c0.min(c1) - e, c0.max(c1) + e))
            }
        }
    }

    /// Silhouette points of a revolution for direction `d`, traced along the profile: at each
    /// profile parameter the normal is perpendicular to `d` at up to two angles. Returns
    /// polylines of exact points with their chord deviation, and exact circles when `d` is the axis.
    pub fn rev_silhouette(&self, d: Vec3) -> Vec<(C3, f64)> {
        let S3::Rev(r) = self else { return Vec::new() };
        r.silhouette(d)
    }

    pub fn domain(&self) -> [[f64; 2]; 2] {
        match self {
            S3::Plane { .. } => [[f64::NEG_INFINITY, f64::INFINITY]; 2],
            S3::Rev(r) => [r.theta, r.dom],
            S3::Nurbs(n) => n.dom,
        }
    }

    pub fn periods(&self) -> [Option<f64>; 2] {
        match self {
            S3::Rev(r) => [((r.theta[1] - r.theta[0]) >= TAU - 1e-9).then_some(TAU), r.closed.then_some(r.dom[1] - r.dom[0])],
            _ => [None, None],
        }
    }

    pub fn point(&self, uv: [f64; 2]) -> Vec3 {
        match self {
            S3::Plane { o, u, v, .. } => *o + *u * uv[0] + *v * uv[1],
            S3::Rev(r) => r.point(uv),
            S3::Nurbs(n) => n.point(uv),
        }
    }

    pub fn partials(&self, uv: [f64; 2]) -> (Vec3, Vec3) {
        match self {
            S3::Plane { u, v, .. } => (*u, *v),
            _ => {
                let d = self.domain();
                let fd = |k: usize| {
                    let span = d[k][1] - d[k][0];
                    let h = span * 1e-6;
                    let lo = if self.periods()[k].is_some() { uv[k] - h } else { (uv[k] - h).max(d[k][0]) };
                    let hi = if self.periods()[k].is_some() { uv[k] + h } else { (uv[k] + h).min(d[k][1]) };
                    let (mut a, mut b) = (uv, uv);
                    a[k] = lo;
                    b[k] = hi;
                    if hi - lo <= 0. { Vec3::ZERO } else { (self.point(b) - self.point(a)) / (hi - lo) }
                };
                (fd(0), fd(1))
            }
        }
    }

    /// The chart normal `Su x Sv`, unit, or `None` at a singular point.
    pub fn normal(&self, uv: [f64; 2]) -> Option<Vec3> {
        match self {
            S3::Plane { n, .. } => Some(*n),
            _ => {
                let (su, sv) = self.partials(uv);
                su.cross(sv).normalized()
            }
        }
    }

    /// Chart coordinates of a point on (or very near) the surface. `hint` guides spline
    /// Newton iterations and the choice of periodic branch.
    pub fn inverse(&self, p: Vec3, hint: Option<[f64; 2]>) -> Option<[f64; 2]> {
        match self {
            S3::Plane { o, u, v, .. } => {
                let w = p - *o;
                let (uu, uv, vv) = (u.dot(*u), u.dot(*v), v.dot(*v));
                let det = uu * vv - uv * uv;
                if det.abs() <= 1e-300 {
                    return None;
                }
                let (bu, bv) = (w.dot(*u), w.dot(*v));
                Some([(bu * vv - bv * uv) / det, (bv * uu - bu * uv) / det])
            }
            S3::Rev(r) => r.inverse(p, hint),
            S3::Nurbs(n) => n.inverse(p, hint),
        }
    }

    /// Ray hits along unit `d` from `p0` with `lam` in `range` (the face's box along the ray).
    pub fn ray(&self, p0: Vec3, d: Vec3, range: [f64; 2], out: &mut Vec<Hit>) {
        match self {
            S3::Plane { n, .. } => {
                let nd = n.dot(d);
                if nd.abs() < 1e-12 {
                    return;
                }
                let lam = n.dot(self.point([0., 0.]) - p0) / nd;
                if lam >= range[0]
                    && lam <= range[1]
                    && let Some(uv) = self.inverse(p0 + d * lam, None)
                {
                    out.push(Hit { lam, uv, grazing: nd.abs() < 1e-6 });
                }
            }
            S3::Rev(r) => r.ray(p0, d, range, out),
            S3::Nurbs(n) => n.ray(p0, d, range, out),
        }
    }

    /// Ray hits using the recognised analytic form when there is one (closed form for
    /// quadrics, a sampled quartic for tori), else the record's own intersection.
    pub fn ray_as(&self, an: &Analytic, p0: Vec3, d: Vec3, range: [f64; 2], out: &mut Vec<Hit>) {
        let lams = match *an {
            Analytic::Cylinder { p, a, r } => {
                let (w, v) = (p0 - p, d - a * d.dot(a));
                let w = w - a * w.dot(a);
                quadratic(v.dot(v), 2. * v.dot(w), w.dot(w) - r * r)
            }
            Analytic::Cone { apex, a, half } => {
                let w = p0 - apex;
                let c2 = half.cos().powi(2);
                let (da, wa) = (d.dot(a), w.dot(a));
                let roots = quadratic(da * da - c2, 2. * (da * wa - c2 * d.dot(w)), wa * wa - c2 * w.dot(w));
                roots.into_iter().filter(|(l, _)| (w + d * *l).dot(a) >= 0.).collect()
            }
            Analytic::Sphere { c, r } => {
                let w = p0 - c;
                quadratic(1., 2. * d.dot(w), w.dot(w) - r * r)
            }
            Analytic::Torus { c, a, major, minor } => torus_roots(p0 - c, d, a, major, minor, range),
            Analytic::Plane | Analytic::Other => return self.ray(p0, d, range, out),
        };
        for (lam, grazing) in lams {
            if lam >= range[0] && lam <= range[1] {
                // The analytic surface extends past this record's domain: keep hits on it only.
                let p = p0 + d * lam;
                if let Some(uv) = self.inverse(p, None).filter(|uv| self.point(*uv).dist(p) <= 1e-7 * (1. + p.len())) {
                    out.push(Hit { lam, uv, grazing });
                }
            }
        }
    }

    /// Which analytic surface this is, checked at sample points to `tol` relative to its size.
    pub fn analytic(&self, tol: f64) -> Analytic {
        match self {
            S3::Plane { .. } => Analytic::Plane,
            S3::Rev(r) => r.analytic(tol),
            S3::Nurbs(n) => n.analytic(tol),
        }
    }
}

/// Real roots of `a x^2 + b x + c`, a double root flagged as grazing.
pub(crate) fn quadratic(a: f64, b: f64, c: f64) -> Vec<(f64, bool)> {
    let scale = b * b + (a * c).abs();
    if a.abs() <= 1e-14 * (1. + b.abs()) {
        return if b.abs() > 1e-300 { vec![(-c / b, false)] } else { Vec::new() };
    }
    let disc = b * b - 4. * a * c;
    if disc < -1e-12 * scale {
        return Vec::new();
    }
    if disc <= 1e-12 * scale {
        return vec![(-b / (2. * a), true)];
    }
    // Numerically stable pair.
    let q = -0.5 * (b + b.signum() * disc.sqrt());
    vec![(q / a, false), (c / q, false)]
}

/// Roots of the torus implicit function along a ray, by sampling and bisection; dips that
/// touch zero without a sign change are grazing.
pub(crate) fn torus_roots(w: Vec3, d: Vec3, a: Vec3, big: f64, small: f64, range: [f64; 2]) -> Vec<(f64, bool)> {
    // Signed distance from the tube: distance to the centre circle less the tube radius,
    // computed directly (no quartic), so a thin tube on a large ring keeps its precision.
    let g = |l: f64| {
        let p = w + d * l;
        let z = p.dot(a);
        let rho = (p - a * z).len();
        (rho - big).hypot(z) - small
    };
    // Sample only where the ray is inside the shell |z| <= r, R - r <= rho <= R + r, in steps
    // of a quarter of the tube radius: the shell is short along any ray, so the steps are few.
    // The shell is padded: a crossing exactly on its surface (rho = R + r at z = 0, say) must
    // fall strictly inside a sampled piece.
    let m = 0.5 * small + 1e-9 * big;
    let (z0, dz) = (w.dot(a), d.dot(a));
    let mut span = range;
    if dz.abs() > 1e-300 {
        let (l0, l1) = ((-small - m - z0) / dz, (small + m - z0) / dz);
        span = [span[0].max(l0.min(l1)), span[1].min(l0.max(l1))];
    } else if z0.abs() > small + m {
        return Vec::new();
    }
    // rho^2 along the ray: |w_perp + l d_perp|^2.
    let (wp, dp) = (w - a * z0, d - a * dz);
    let (qa, qb, qc) = (dp.dot(dp), 2. * wp.dot(dp), wp.dot(wp));
    let within = |r2: f64| -> Option<[f64; 2]> {
        // Where rho^2 <= r2.
        if qa <= 1e-300 {
            return (qc <= r2).then_some([f64::NEG_INFINITY, f64::INFINITY]);
        }
        let disc = qb * qb - 4. * qa * (qc - r2);
        (disc >= 0.).then(|| [(-qb - disc.sqrt()) / (2. * qa), (-qb + disc.sqrt()) / (2. * qa)])
    };
    let Some(outer) = within((big + small + m).powi(2)) else { return Vec::new() };
    span = [span[0].max(outer[0]), span[1].min(outer[1])];
    if !(span[1] > span[0]) {
        return Vec::new();
    }
    // Remove the inside of the inner radius, leaving up to two pieces.
    let mut pieces = vec![span];
    if big > small + m
        && let Some(inner) = within((big - small - m).powi(2))
    {
        pieces = vec![[span[0], span[1].min(inner[0])], [span[0].max(inner[1]), span[1]]];
    }
    // |g| is 1-Lipschitz along the ray: two samples whose values share a sign and add up to
    // more than their distance cannot have a crossing between them. Other pairs are halved down
    // to a thousandth of the tube radius, so a shallow cut through the tube (a chord shorter than
    // the sample step) is still found.
    let fine = 1e-3 * small;
    let mut out = Vec::new();
    for [s0, s1] in pieces.into_iter().filter(|p| p[1] > p[0]) {
        let n = ((s1 - s0) / (0.25 * small)).ceil().clamp(8., 1_000_000.) as usize;
        let at = |i: usize| s0 + (s1 - s0) * i as f64 / n as f64;
        let mut stack: Vec<(f64, f64, f64, f64)> = Vec::new();
        let vals: Vec<f64> = (0..=n).map(|i| g(at(i))).collect();
        for i in 0..n {
            if let (Some(&ga), Some(&gb)) = (vals.get(i), vals.get(i + 1)) {
                stack.push((at(i), at(i + 1), ga, gb));
            }
        }
        stack.reverse();
        let mut budget = 1_000_000usize;
        while let Some((lo0, hi0, ga, gb)) = stack.pop() {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                break;
            }
            if ga != 0. && gb != 0. && ga.signum() != gb.signum() {
                let (mut lo, mut hi, mut glo) = (lo0, hi0, ga);
                for _ in 0..80 {
                    let mid = 0.5 * (lo + hi);
                    let gm = g(mid);
                    if gm.signum() == glo.signum() {
                        lo = mid;
                        glo = gm;
                    } else {
                        hi = mid;
                    }
                }
                out.push((0.5 * (lo + hi), false));
                continue;
            }
            if ga.abs() + gb.abs() > hi0 - lo0 {
                continue;
            }
            if hi0 - lo0 <= fine {
                // Close to the tube without crossing it: a touch when it is a dip inside the shell.
                let m = 0.5 * (lo0 + hi0);
                let gm = g(m);
                if gm > 0. && gm <= ga && gm <= gb && lo0 > s0 && hi0 < s1 && gm < 1e-4 * small.max(1e-9 * big) {
                    out.push((m, true));
                }
                continue;
            }
            let m = 0.5 * (lo0 + hi0);
            let gm = g(m);
            stack.push((m, hi0, gm, gb));
            stack.push((lo0, m, ga, gm));
        }
    }
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    out.dedup_by(|x, y| (x.0 - y.0).abs() <= fine && x.1 == y.1);
    out
}

fn axes(f: &Frame) -> (Vec3, Vec3, Vec3, Vec3) {
    let (o, x, y) = frame_axes(f);
    (o, x, y, x.cross(y))
}

impl Rev {
    fn new(o: Vec3, axis: Vec3, e1: Option<Vec3>, profile: C3, angle: [f64; 2]) -> Result<Rev, HlrError> {
        let a = axis.normalized().ok_or(HlrError::Unsupported("zero revolution axis"))?;
        let dom = profile.domain();
        let e1 = e1.unwrap_or_else(|| {
            let w = profile.at(dom[0]) - o;
            (w - a * w.dot(a)).normalized().unwrap_or_else(|| perp(a))
        });
        let e2 = a.cross(e1);
        let closed = profile.at(dom[0]).dist(profile.at(dom[1])) <= 1e-9 * (1. + profile.at(dom[0]).dist(o));
        let n = 512;
        let mut samples = Vec::with_capacity(n + 1);
        let (mut rmax, mut hlo, mut hhi) = (0f64, f64::MAX, f64::MIN);
        for i in 0..=n {
            let s = dom[0] + (dom[1] - dom[0]) * i as f64 / n as f64;
            let w = profile.at(s) - o;
            let h = w.dot(a);
            let r = (w - a * h).len();
            rmax = rmax.max(r);
            hlo = hlo.min(h);
            hhi = hhi.max(h);
            samples.push([s, r, h]);
        }
        if !(rmax.is_finite() && hlo.is_finite() && hhi.is_finite()) {
            return Err(HlrError::Unsupported("non-finite revolution profile"));
        }
        // Chart theta is measured from e1; a full turn is [0, 2pi).
        let theta = if angle[1] - angle[0] >= TAU - 1e-9 { [0., TAU] } else { angle };
        Ok(Rev { o, a, e1, e2, profile, dom, theta, closed, samples, rmax, hrange: [hlo, hhi] })
    }

    fn rh(&self, s: f64) -> [f64; 2] {
        let w = self.profile.at(s) - self.o;
        let h = w.dot(self.a);
        [(w - self.a * h).len(), h]
    }

    fn rh_d(&self, s: f64) -> [f64; 2] {
        let h = (self.dom[1] - self.dom[0]) * 1e-7;
        let (a, b) = ((s - h).max(self.dom[0]), (s + h).min(self.dom[1]));
        if b - a <= 0. {
            return [0., 0.];
        }
        let (p, q) = (self.rh(a), self.rh(b));
        [(q[0] - p[0]) / (b - a), (q[1] - p[1]) / (b - a)]
    }

    fn point(&self, uv: [f64; 2]) -> Vec3 {
        let [r, h] = self.rh(uv[1]);
        self.o + self.a * h + (self.e1 * uv[0].cos() + self.e2 * uv[0].sin()) * r
    }

    /// Profile parameters where component `k` of the profile's (r, h) tangent vanishes: sign
    /// changes between samples, refined by bisection, exact zeros at samples, and for a closed
    /// profile the change across the seam where its ends meet.
    fn roots(&self, ss: &[f64], k: usize) -> Vec<f64> {
        let f = |s: f64| self.rh_d(s).get(k).copied().unwrap_or(0.);
        let vals: Vec<f64> = ss.iter().map(|s| f(*s)).collect();
        let scale = vals.iter().fold(0f64, |m, v| m.max(v.abs())).max(1e-300);
        let zero = |v: f64| v.abs() <= 1e-12 * scale;
        let mut out: Vec<f64> = Vec::new();
        let mut pairs: Vec<(usize, usize)> = (0..ss.len().saturating_sub(1)).map(|i| (i, i + 1)).collect();
        if self.closed && ss.len() > 2 {
            // The last sample repeats the first; the pair across the seam is (n-1, 1).
            pairs.push((ss.len() - 2, 1));
        }
        for (i, sv) in vals.iter().enumerate() {
            if zero(*sv)
                && !(self.closed && i + 1 == vals.len())
                && let Some(s0) = ss.get(i)
            {
                out.push(*s0);
            }
        }
        for (i, j) in pairs {
            let (Some(&a), Some(&b), Some(&sa), Some(&sb)) = (vals.get(i), vals.get(j), ss.get(i), ss.get(j)) else { continue };
            if zero(a) || zero(b) || a.signum() == b.signum() {
                continue;
            }
            // Across the seam, walk sb forward a period so the interval is increasing.
            let sb = if sb < sa { sb + (self.dom[1] - self.dom[0]) } else { sb };
            let wrap = |x: f64| if x > self.dom[1] { x - (self.dom[1] - self.dom[0]) } else { x };
            let (mut lo, mut hi, mut flo) = (sa, sb, a);
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                let fm = f(wrap(mid));
                if fm.signum() == flo.signum() && !zero(fm) {
                    lo = mid;
                    flo = fm;
                } else {
                    hi = mid;
                }
            }
            out.push(wrap(0.5 * (lo + hi)));
        }
        out.sort_by(f64::total_cmp);
        out.dedup_by(|x, y| (*x - *y).abs() <= 1e-9 * (self.dom[1] - self.dom[0]));
        out
    }

    /// cos(theta - phi) = dr (a.d) / (dh rho) along the profile; see `S3::rev_silhouette`.
    fn silhouette(&self, d: Vec3) -> Vec<(C3, f64)> {
        let (d1, d2, da) = (d.dot(self.e1), d.dot(self.e2), d.dot(self.a));
        let rho = (d1 * d1 + d2 * d2).sqrt();
        let phi = d2.atan2(d1);
        let n = 720usize;
        let mut ss: Vec<f64> = (0..=n).map(|i| self.dom[0] + (self.dom[1] - self.dom[0]) * i as f64 / n as f64).collect();
        let mut out = Vec::new();
        if rho > 1e-12 {
            // The silhouette exists where |dr (a.d)| <= |dh| rho; near the axis direction those
            // intervals shrink around the zeros of dr, which are sampled so none is stepped over.
            let extra = self.roots(&ss, 0);
            ss.extend(extra);
            ss.sort_by(f64::total_cmp);
            ss.dedup_by(|x, y| (*x - *y).abs() <= 1e-15 * (self.dom[1] - self.dom[0]));
        }
        if rho <= 1e-12 {
            // Along the axis: circles where the profile runs parallel to the axis (dr = 0).
            for s0 in self.roots(&ss, 0) {
                let [r, h] = self.rh(s0);
                if r > 1e-12 {
                    out.push((C3::Arc { o: self.o + self.a * h, x: self.e1, y: self.e2, r: [r, r], range: [0., TAU] }, 0.));
                }
            }
            return out;
        }
        if da.abs() <= 1e-12 {
            // Square to the axis: where the profile runs radially (dh = 0) the normal is the
            // axis, square to d for every angle, so the whole parallel circle is a silhouette.
            for s0 in self.roots(&ss, 1) {
                let [r, h] = self.rh(s0);
                if r > 1e-12 {
                    out.push((C3::Arc { o: self.o + self.a * h, x: self.e1, y: self.e2, r: [r, r], range: [0., TAU] }, 0.));
                }
            }
        }
        let c = |s: f64| {
            let t = self.rh_d(s);
            if t[1].abs() <= 1e-300 { f64::INFINITY } else { t[0] * da / (t[1] * rho) }
        };
        let at = |sign: f64, s: f64| self.point([phi + sign * c(s).clamp(-1., 1.).acos(), s]);
        // The parameter between a (|c| <= 1) and b (|c| > 1) where |c| = 1.
        let edge = |a: f64, b: f64| {
            let (mut lo, mut hi) = (a, b);
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                if c(mid).abs() <= 1. {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        // One polyline per branch while |c| <= 1.
        for sign in [1., -1.] {
            let mut run: Vec<f64> = Vec::new();
            let flush = |run: &mut Vec<f64>, out: &mut Vec<(C3, f64)>| {
                if run.len() >= 2 {
                    let pts: Vec<Vec3> = run.iter().map(|s| at(sign, *s)).collect();
                    let mut dev = 0f64;
                    for (w, q) in run.windows(2).zip(pts.windows(2)) {
                        let (Some(a), Some(b), Some(pa), Some(pb)) = (w.first(), w.get(1), q.first(), q.get(1)) else { continue };
                        dev = dev.max(at(sign, 0.5 * (a + b)).dist_to_segment(*pa, *pb));
                    }
                    out.push((C3::Polyline(pts), dev));
                }
                run.clear();
            };
            for i in 0..ss.len() {
                let Some(&s) = ss.get(i) else { continue };
                let inside = c(s).abs() <= 1.;
                let prev = i.checked_sub(1).and_then(|j| ss.get(j).copied());
                match (inside, prev.map(|p| c(p).abs() <= 1.)) {
                    (true, Some(false)) => {
                        if let Some(p) = prev {
                            run.push(edge(s, p));
                        }
                        run.push(s);
                    }
                    (true, _) => run.push(s),
                    (false, Some(true)) => {
                        if let Some(p) = prev {
                            run.push(edge(p, s));
                        }
                        flush(&mut run, &mut out);
                    }
                    (false, _) => {}
                }
            }
            flush(&mut run, &mut out);
        }
        out
    }

    /// Closest profile parameter to `(r, h)`, with the squared distance.
    fn closest(&self, q: [f64; 2]) -> (f64, f64) {
        if let C3::Line { .. } = self.profile {
            let (p0, p1) = (self.rh(self.dom[0]), self.rh(self.dom[1]));
            let e = [p1[0] - p0[0], p1[1] - p0[1]];
            let l2 = e[0] * e[0] + e[1] * e[1];
            let t = if l2 > 0. { (((q[0] - p0[0]) * e[0] + (q[1] - p0[1]) * e[1]) / l2).clamp(0., 1.) } else { 0. };
            let s = self.dom[0] + (self.dom[1] - self.dom[0]) * t;
            let p = self.rh(s);
            return (s, (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2));
        }
        // Coarse scan, then the finer samples around the best coarse one.
        let mut best = (0usize, f64::MAX);
        for (i, smp) in self.samples.iter().enumerate().step_by(8) {
            let d2 = (smp[1] - q[0]).powi(2) + (smp[2] - q[1]).powi(2);
            if d2 < best.1 {
                best = (i, d2);
            }
        }
        let mut best = (self.samples.get(best.0).map_or(self.dom[0], |s| s[0]), best.1, best.0);
        for i in best.2.saturating_sub(8)..(best.2 + 9).min(self.samples.len()) {
            if let Some(smp) = self.samples.get(i) {
                let d2 = (smp[1] - q[0]).powi(2) + (smp[2] - q[1]).powi(2);
                if d2 < best.1 {
                    best = (smp[0], d2, i);
                }
            }
        }
        let best = (best.0, best.1);
        let mut s = best.0;
        for _ in 0..30 {
            let p = self.rh(s);
            let d = self.rh_d(s);
            let f = (p[0] - q[0]) * d[0] + (p[1] - q[1]) * d[1];
            let fp = d[0] * d[0] + d[1] * d[1];
            if fp <= 1e-300 {
                break;
            }
            let next = (s - f / fp).clamp(self.dom[0], self.dom[1]);
            if (next - s).abs() <= 1e-15 * (1. + s.abs()) {
                s = next;
                break;
            }
            s = next;
        }
        let p = self.rh(s);
        (s, (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2))
    }

    fn inverse(&self, p: Vec3, hint: Option<[f64; 2]>) -> Option<[f64; 2]> {
        let w = p - self.o;
        let h = w.dot(self.a);
        let radial = w - self.a * h;
        let r = radial.len();
        let theta = if r > 1e-12 * (1. + self.rmax) { radial.dot(self.e2).atan2(radial.dot(self.e1)) } else { hint.map_or(0., |u| u[0]) };
        let (s, _) = self.closest([r, h]);
        let mut t = theta;
        if self.theta[1] - self.theta[0] >= TAU - 1e-9 {
            t = t.rem_euclid(TAU);
        } else if t < self.theta[0] {
            t += TAU;
        }
        Some([t, s])
    }

    /// Signed distance of `(r, h)` from the profile, or `None` beyond an open profile's ends.
    fn signed(&self, q: [f64; 2]) -> Option<f64> {
        let (s, d2) = self.closest(q);
        let at_end = !self.closed && ((s - self.dom[0]).abs() <= 1e-12 * (1. + s.abs()) || (s - self.dom[1]).abs() <= 1e-12 * (1. + s.abs()));
        let p = self.rh(s);
        let t = self.rh_d(s);
        if at_end {
            // Beyond the end the normal distance is undefined unless q projects onto it.
            let along = (q[0] - p[0]) * t[0] + (q[1] - p[1]) * t[1];
            let outward = if (s - self.dom[0]).abs() < (s - self.dom[1]).abs() { -along } else { along };
            if outward > 1e-12 * (1. + self.rmax) {
                return None;
            }
        }
        let cross = t[0] * (q[1] - p[1]) - t[1] * (q[0] - p[0]);
        Some(cross.signum() * d2.sqrt())
    }

    fn ray(&self, p0: Vec3, d: Vec3, range: [f64; 2], out: &mut Vec<Hit>) {
        let rh = |lam: f64| {
            let w = p0 + d * lam - self.o;
            let h = w.dot(self.a);
            [(w - self.a * h).len(), h]
        };
        // Clip the ray to the bounding cylinder, coarsely, by sampling.
        let pad = 1e-6 * (1. + self.rmax + (self.hrange[1] - self.hrange[0]));
        let n = 400usize;
        let f = |lam: f64| {
            let q = rh(lam);
            if q[0] > self.rmax + pad || q[1] < self.hrange[0] - pad || q[1] > self.hrange[1] + pad { None } else { self.signed(q) }
        };
        // Values within the noise floor carry no sign (a ray tangent at its start, say).
        let floor = 1e-9 * (1. + self.rmax + (self.hrange[1] - self.hrange[0]));
        let mut prev: Option<(f64, f64)> = None;
        let mut minima: Vec<(f64, f64)> = Vec::new();
        let mut last: Option<f64> = None;
        let mut falling = false;
        let at = |i: usize| range[0] + (range[1] - range[0]) * i as f64 / n as f64;
        for i in 0..=n {
            let lam = at(i);
            let v = f(lam);
            match (prev, v) {
                (Some((la, fa)), Some(fb)) if fa.abs() > floor && fb.abs() > floor && fa.signum() != fb.signum() => {
                    let (mut lo, mut hi, mut flo) = (la, lam, fa);
                    for _ in 0..80 {
                        let mid = 0.5 * (lo + hi);
                        match f(mid) {
                            Some(fm) if fm.signum() == flo.signum() => {
                                lo = mid;
                                flo = fm;
                            }
                            Some(_) => hi = mid,
                            None => break,
                        }
                    }
                    let lam = 0.5 * (lo + hi);
                    if lam >= range[0]
                        && let Some(uv) = self.inverse(p0 + d * lam, None)
                    {
                        out.push(Hit { lam, uv, grazing: false });
                    }
                }
                _ => {}
            }
            // Interior dips only: the range's first sample is never a minimum.
            if let Some(fb) = v {
                let a = fb.abs();
                if let Some(l) = last {
                    if falling && a > l {
                        minima.push((at(i - 1), l));
                    }
                    falling = a < l;
                }
                last = Some(a);
            } else {
                falling = false;
                last = None;
            }
            match v {
                Some(fb) if fb.abs() > floor => prev = Some((lam, fb)),
                Some(_) => {}
                None => prev = None,
            }
        }
        // Near-tangent rays: a dip close to zero without a sign change.
        let tol = 1e-4 * (1. + self.rmax);
        for (lam, a) in minima {
            if a < tol
                && lam >= range[0]
                && !out.iter().any(|h| (h.lam - lam).abs() < 2. * (range[1] - range[0]) / n as f64)
                && let Some(uv) = self.inverse(p0 + d * lam, None)
            {
                out.push(Hit { lam, uv, grazing: true });
            }
        }
    }

    fn analytic(&self, tol: f64) -> Analytic {
        let size = self.rmax.max(self.hrange[1] - self.hrange[0]).max(1e-9);
        let pts: Vec<[f64; 2]> = self.samples.iter().map(|s| [s[1], s[2]]).collect();
        let (Some(first), Some(last)) = (pts.first(), pts.last()) else { return Analytic::Other };
        // A straight profile: cylinder (constant r), cone, or a plane square to the axis.
        let e = [last[0] - first[0], last[1] - first[1]];
        let el = (e[0] * e[0] + e[1] * e[1]).sqrt();
        let straight = el > 0. && pts.iter().all(|p| ((p[0] - first[0]) * e[1] - (p[1] - first[1]) * e[0]).abs() / el <= tol * size);
        if straight {
            if e[0].abs() <= tol * el {
                return Analytic::Cylinder { p: self.o, a: self.a, r: first[0] };
            }
            if e[1].abs() <= tol * el {
                return Analytic::Plane;
            }
            // Apex where r = 0 on the line.
            let t = -first[0] / e[0];
            let h_apex = first[1] + t * e[1];
            let dir = if e[1] * e[0] > 0. { self.a } else { -self.a };
            let half = (e[0].abs() / el).asin();
            return Analytic::Cone { apex: self.o + self.a * h_apex, a: dir, half };
        }
        // A circular profile: sphere when its centre is on the axis, torus otherwise.
        if let Some((c, _, r)) = as_circle(&self.profile, tol) {
            let w = c - self.o;
            let h = w.dot(self.a);
            let rc = (w - self.a * h).len();
            if rc <= tol * size {
                return Analytic::Sphere { c: self.o + self.a * h, r };
            }
            return Analytic::Torus { c: self.o + self.a * h, a: self.a, major: rc, minor: r };
        }
        Analytic::Other
    }
}

impl Nurbs {
    fn new(u: &[f64], v: &[f64], rows: &[Vec<[f64; 4]>]) -> Result<Nurbs, HlrError> {
        let nu = rows.len();
        let nv = rows.first().map_or(0, Vec::len);
        let pu = u.len().checked_sub(nu + 1).ok_or(HlrError::Unsupported("surface knots"))?;
        let pv = v.len().checked_sub(nv + 1).ok_or(HlrError::Unsupported("surface knots"))?;
        let mut ctrl = Vec::with_capacity(nu * nv);
        for j in 0..nv {
            for row in rows {
                let p = row.get(j).ok_or(HlrError::Unsupported("ragged surface"))?;
                ctrl.push(HPoint { x: p[0], y: p[1], z: p[2], w: p[3] });
            }
        }
        let s = NurbsSurface::from_homogeneous([pu, pv], [u.to_vec(), v.to_vec()], ctrl).map_err(|_| HlrError::Unsupported("spline surface"))?;
        let dom = s.domain();
        let g = (4 * nu.max(nv)).clamp(16, 64);
        let mut n = Nurbs { s, dom, g, grid: Vec::new() };
        let mut grid = Vec::with_capacity((g + 1) * (g + 1));
        for j in 0..=g {
            for i in 0..=g {
                grid.push(n.point(n.grid_uv(i, j)));
            }
        }
        if grid.iter().any(|p| !p.is_finite()) {
            return Err(HlrError::Unsupported("spline surface evaluation"));
        }
        n.grid = grid;
        Ok(n)
    }

    pub fn grid_uv(&self, i: usize, j: usize) -> [f64; 2] {
        let f = |k: usize, x: usize| self.dom[k][0] + (self.dom[k][1] - self.dom[k][0]) * x as f64 / self.g as f64;
        [f(0, i), f(1, j)]
    }

    fn point(&self, uv: [f64; 2]) -> Vec3 {
        let u = uv[0].clamp(self.dom[0][0], self.dom[0][1]);
        let v = uv[1].clamp(self.dom[1][0], self.dom[1][1]);
        self.s.eval(u, v).unwrap_or(Vec3::new(f64::NAN, f64::NAN, f64::NAN))
    }

    fn partials(&self, uv: [f64; 2]) -> (Vec3, Vec3) {
        let fd = |k: usize| {
            let h = (self.dom[k][1] - self.dom[k][0]) * 1e-6;
            let (mut a, mut b) = (uv, uv);
            a[k] = (uv[k] - h).max(self.dom[k][0]);
            b[k] = (uv[k] + h).min(self.dom[k][1]);
            if b[k] - a[k] <= 0. { Vec3::ZERO } else { (self.point(b) - self.point(a)) / (b[k] - a[k]) }
        };
        (fd(0), fd(1))
    }

    fn clamp(&self, uv: [f64; 2]) -> [f64; 2] {
        [uv[0].clamp(self.dom[0][0], self.dom[0][1]), uv[1].clamp(self.dom[1][0], self.dom[1][1])]
    }

    fn inverse(&self, p: Vec3, hint: Option<[f64; 2]>) -> Option<[f64; 2]> {
        let mut uv = match hint {
            Some(h) => self.clamp(h),
            None => {
                let mut best = (0usize, f64::MAX);
                for (k, q) in self.grid.iter().enumerate() {
                    let d = q.dist(p);
                    if d < best.1 {
                        best = (k, d);
                    }
                }
                self.grid_uv(best.0 % (self.g + 1), best.0 / (self.g + 1))
            }
        };
        for _ in 0..40 {
            let (su, sv) = self.partials(uv);
            let r = self.point(uv) - p;
            let (a, b, c) = (su.dot(su), su.dot(sv), sv.dot(sv));
            let det = a * c - b * b;
            if det.abs() <= 1e-300 {
                break;
            }
            let (gu, gv) = (su.dot(r), sv.dot(r));
            let du = (gu * c - gv * b) / det;
            let dv = (gv * a - gu * b) / det;
            let next = self.clamp([uv[0] - du, uv[1] - dv]);
            let moved = (next[0] - uv[0]).abs() + (next[1] - uv[1]).abs();
            uv = next;
            if moved <= 1e-15 * (1. + uv[0].abs() + uv[1].abs()) {
                break;
            }
        }
        uv[0].is_finite().then_some(uv)
    }

    fn ray(&self, p0: Vec3, d: Vec3, range: [f64; 2], out: &mut Vec<Hit>) {
        let ep = perp(d);
        let eq = d.cross(ep);
        let q2 = |p: Vec3| [(p - p0).dot(ep), (p - p0).dot(eq)];
        let g = self.g;
        let start = out.len();
        let proj: Vec<[f64; 2]> = self.grid.iter().map(|p| q2(*p)).collect();
        for j in 0..g {
            for i in 0..g {
                let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                let pts: Vec<[f64; 2]> = corners.iter().filter_map(|(a, b)| proj.get(b * (g + 1) + a).copied()).collect();
                let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
                for p in &pts {
                    lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                    hi = [hi[0].max(p[0]), hi[1].max(p[1])];
                }
                let pad = 0.25 * ((hi[0] - lo[0]).max(hi[1] - lo[1]));
                if lo[0] - pad > 0. || hi[0] + pad < 0. || lo[1] - pad > 0. || hi[1] + pad < 0. {
                    continue;
                }
                let (a, b) = (self.grid_uv(i, j), self.grid_uv(i + 1, j + 1));
                let mut uv = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
                let mut ok = false;
                for _ in 0..40 {
                    let p = self.point(uv);
                    let f = q2(p);
                    let (su, sv) = self.partials(uv);
                    let (j11, j12, j21, j22) = (su.dot(ep), sv.dot(ep), su.dot(eq), sv.dot(eq));
                    let det = j11 * j22 - j12 * j21;
                    if det.abs() <= 1e-300 {
                        break;
                    }
                    let du = (f[0] * j22 - f[1] * j12) / det;
                    let dv = (f[1] * j11 - f[0] * j21) / det;
                    uv = self.clamp([uv[0] - du, uv[1] - dv]);
                    if du.abs() + dv.abs() <= 1e-14 * (1. + uv[0].abs() + uv[1].abs()) {
                        let r = q2(self.point(uv));
                        ok = r[0].abs() + r[1].abs() <= 1e-9 * (1. + self.size_hint());
                        break;
                    }
                }
                // Keep roots inside this cell (with slack) so neighbouring cells don't repeat them.
                let slack = [(b[0] - a[0]) * 1e-6, (b[1] - a[1]) * 1e-6];
                if !ok || uv[0] < a[0] - slack[0] || uv[0] > b[0] + slack[0] || uv[1] < a[1] - slack[1] || uv[1] > b[1] + slack[1] {
                    continue;
                }
                let p = self.point(uv);
                let lam = (p - p0).dot(d);
                if lam >= range[0] && lam <= range[1] && !out.iter().skip(start).any(|h| (h.lam - lam).abs() <= 1e-9 * (1. + lam)) {
                    let (su, sv) = self.partials(uv);
                    let grazing = su.cross(sv).normalized().is_none_or(|n| n.dot(d).abs() < 1e-6);
                    out.push(Hit { lam, uv, grazing });
                }
            }
        }
    }

    fn size_hint(&self) -> f64 {
        self.grid.first().zip(self.grid.last()).map_or(1., |(a, b)| a.dist(*b))
    }

    /// Cylinder/cone/sphere recognition from iso-curves: one family straight, the other circles.
    fn analytic(&self, tol: f64) -> Analytic {
        let size = self.size_hint().max(1e-9);
        let iso = |k: usize, x: f64| -> C3 {
            let n = 24;
            let pts = (0..=n)
                .map(|i| {
                    let t = self.dom[k][0] + (self.dom[k][1] - self.dom[k][0]) * i as f64 / n as f64;
                    if k == 0 { self.point([t, x]) } else { self.point([x, t]) }
                })
                .collect();
            C3::Polyline(pts)
        };
        let straight = |c: &C3| {
            let C3::Polyline(p) = c else { return false };
            let (Some(a), Some(b)) = (p.first(), p.last()) else { return false };
            let Some(e) = (*b - *a).normalized() else { return false };
            p.iter().all(|q| ((*q - *a) - e * (*q - *a).dot(e)).len() <= tol * size)
        };
        let circle_of = |k: usize, x: f64| -> Option<(Vec3, Vec3, f64)> {
            // Fit through exact surface points, then verify densely.
            let at = |f: f64| {
                let t = self.dom[k][0] + (self.dom[k][1] - self.dom[k][0]) * f;
                if k == 0 { self.point([t, x]) } else { self.point([x, t]) }
            };
            let pts: Vec<Vec3> = (0..=32).map(|i| at(i as f64 / 32.)).collect();
            let sp = C3::Polyline(pts.clone());
            let (a, m, b) = (pts.first().copied()?, pts.get(16).copied()?, pts.get(8).copied()?);
            let (u, v) = (m - a, b - a);
            let n = u.cross(v);
            let n2 = n.len2();
            if n2 <= 1e-24 {
                return None;
            }
            let c = a + (v * u.len2() - u * v.len2()).cross(n) * (1. / (2. * n2));
            let nn = n.normalized()?;
            let r = c.dist(a);
            let ok = match &sp {
                C3::Polyline(p) => p.iter().all(|q| (q.dist(c) - r).abs() <= tol * r && (*q - c).dot(nn).abs() <= tol * r),
                _ => false,
            };
            ok.then_some((c, nn, r))
        };
        for (line_dir, circ_dir) in [(1usize, 0usize), (0, 1)] {
            let mids = [0., 0.5, 1.].map(|f| self.dom[circ_dir][0] + (self.dom[circ_dir][1] - self.dom[circ_dir][0]) * f);
            if !mids.iter().all(|x| straight(&iso(line_dir, *x))) {
                continue;
            }
            let ends = [0., 0.5, 1.].map(|f| self.dom[line_dir][0] + (self.dom[line_dir][1] - self.dom[line_dir][0]) * f);
            let circles: Option<Vec<_>> = ends.iter().map(|x| circle_of(circ_dir, *x)).collect();
            let Some(circles) = circles else { continue };
            let (Some(c0), Some(c2)) = (circles.first(), circles.last()) else { continue };
            let Some(axis) = (c2.0 - c0.0).normalized() else { continue };
            if c0.1.cross(axis).len() > tol {
                continue;
            }
            if (c0.2 - c2.2).abs() <= tol * c0.2 {
                return Analytic::Cylinder { p: c0.0, a: axis, r: c0.2 };
            }
            let dr = (c2.2 - c0.2) / c0.0.dist(c2.0);
            let half = dr.abs().atan();
            let dir = if dr > 0. { axis } else { -axis };
            let apex = c0.0 - dir * (c0.2 / dr.abs());
            return Analytic::Cone { apex, a: dir, half };
        }
        // Sphere: every grid point equidistant from the best centre of four spread points.
        let g = self.g;
        let pick = |i: usize, j: usize| self.grid.get(j * (g + 1) + i).copied();
        if let (Some(a), Some(b), Some(c), Some(d)) = (pick(0, 0), pick(g, 0), pick(g / 2, g / 2), pick(0, g))
            && let Some(centre) = sphere_centre(a, b, c, d)
        {
            let r = centre.dist(a);
            if r > 0. && self.grid.iter().all(|p| (p.dist(centre) - r).abs() <= tol * r) {
                return Analytic::Sphere { c: centre, r };
            }
        }
        Analytic::Other
    }
}

fn sphere_centre(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> Option<Vec3> {
    // Solve 2(p_i - a).x = |p_i|^2 - |a|^2 for i = b, c, d.
    let rows = [b - a, c - a, d - a];
    let rhs = [b.len2() - a.len2(), c.len2() - a.len2(), d.len2() - a.len2()].map(|v| v * 0.5);
    let [r0, r1, r2] = rows;
    let det = r0.dot(r1.cross(r2));
    if det.abs() <= 1e-18 * (1. + r0.len() * r1.len() * r2.len()) {
        return None;
    }
    let x = (r1.cross(r2) * rhs[0] + r2.cross(r0) * rhs[1] + r0.cross(r1) * rhs[2]) / det;
    x.is_finite().then_some(x)
}
