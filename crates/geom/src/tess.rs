//! Display tessellation plans: the parameter-space triangulation of every face of a body,
//! with the surfaces to evaluate it on, kept apart from the evaluation itself.
//!
//! The kernel decides *where* the vertices go (in each face's (u, v) parameters, with the
//! shared edge nodes fixed in 3D so neighbouring faces meet exactly) and *how* they are
//! connected. Evaluating the surfaces at those parameters is the bulk of the work and has no
//! branching on topology, so it can run anywhere: [`TessPlan::evaluate`] does it on the CPU,
//! and the GPU evaluator runs the same maths in a compute shader. Both evaluate the
//! [`EvalSurface`] forms defined here, so the two cannot drift apart.
//!
//! References: Piegl & Tiller, *The NURBS Book* (2nd ed.), A2.1 (span), A2.3 (basis
//! functions and derivatives), §4.5 (derivatives of a rational surface).

use crate::bspline::MAX_SPLINE_DEGREE;
use crate::{Mesh, Vec3};

/// Most knots in one evaluable knot vector (bounds the shader's span search).
pub const MAX_EVAL_KNOTS: usize = 1 << 12;
/// Most control points in one evaluable surface.
pub const MAX_EVAL_POINTS: usize = 1 << 18;
/// Basis slots for the highest evaluable degree.
pub(crate) const W: usize = MAX_SPLINE_DEGREE + 1;

/// A rational B-spline curve: homogeneous control points `[x·w, y·w, z·w, w]`. Outside its
/// domain it continues as the polynomial of the end span (a line extends for ever).
#[derive(Clone, Debug, PartialEq)]
pub struct EvalCurve {
    pub degree: usize,
    pub knots: Vec<f64>,
    pub ctrl: Vec<[f64; 4]>,
}

/// A surface in a form both evaluators understand.
#[derive(Clone, Debug, PartialEq)]
pub enum EvalSurface {
    /// `origin + u·du + v·dv`.
    Plane { origin: Vec3, du: Vec3, dv: Vec3 },
    /// A rational tensor B-spline; control point `(i, j)` is `ctrl[j·count[0] + i]`.
    /// `closed[d]`: the surface joins itself along direction `d` (a B-spline tube's way round).
    /// Such a direction is periodic: a parameter outside the knot range is moved by whole
    /// ranges back into it, so a face whose boundary runs across the join still evaluates.
    Nurbs { degree: [usize; 2], knots: [Vec<f64>; 2], count: [usize; 2], ctrl: Vec<[f64; 4]>, closed: [bool; 2] },
    /// `profile(u)` turned by the angle `v` (right-handed) about the axis through `origin`
    /// along the unit `axis`, then mapped by the affine `transform` (row-major 3×4); with
    /// `swap` the parameters are exchanged first (the surface is evaluated at `(v, u)`).
    Revolved { profile: EvalCurve, origin: Vec3, axis: Vec3, transform: [[f64; 4]; 3], swap: bool },
}

/// One face's share of a [`TessPlan`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FacePlan {
    /// The B-rep face (index in the body's face order).
    pub face: u32,
    /// Index into [`TessPlan::surfaces`]; `None` when every position and normal is given.
    pub surface: Option<u32>,
    /// Parameters of every vertex.
    pub uv: Vec<[f64; 2]>,
    /// Positions of the first `fixed.len()` vertices: the shared edge nodes, used exactly as
    /// they are so the neighbouring face gets bit-identical boundary points.
    pub fixed: Vec<Vec3>,
    /// Normals of every vertex, only for faces without an evaluable surface.
    pub normals: Vec<Vec3>,
    /// The face is reversed against its surface: normals point the other way.
    pub flip: bool,
    /// Counter-clockwise seen from outside (orientation already applied).
    pub triangles: Vec<[u32; 3]>,
}

/// A body's display tessellation, ready to evaluate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TessPlan {
    pub surfaces: Vec<EvalSurface>,
    pub faces: Vec<FacePlan>,
    /// B-rep edges as polylines (the shared edge nodes), in the body's edge order.
    pub edges: Vec<Vec<Vec3>>,
    /// Per edge: the faces it bounds.
    pub edge_faces: Vec<Vec<u32>>,
}

impl TessPlan {
    /// Evaluate on the CPU into an indexed mesh (vertices are shared within a face, not
    /// across faces, so creases keep their own normals).
    pub fn evaluate(&self) -> Mesh {
        let mut out = Mesh::default();
        for f in &self.faces {
            let base = out.positions.len();
            let surface = f.surface.and_then(|i| self.surfaces.get(i as usize));
            for (k, uv) in f.uv.iter().enumerate() {
                let (p, n) = match surface {
                    Some(s) => s.eval(uv[0], uv[1]),
                    None => (Vec3::ZERO, f.normals.get(k).copied().unwrap_or(Vec3::Z)),
                };
                let p = f.fixed.get(k).copied().unwrap_or(p);
                out.positions.push(p);
                out.normals.push(if f.flip { -n } else { n });
            }
            let n = f.uv.len();
            for t in &f.triangles {
                if t.iter().all(|&i| (i as usize) < n) {
                    let at = |i: u32| u32::try_from(base + i as usize).unwrap_or(u32::MAX);
                    out.triangles.push([at(t[0]), at(t[1]), at(t[2])]);
                    out.tri_face.push(f.face);
                }
            }
        }
        out.edges = self.edges.clone();
        out.edge_faces = self.edge_faces.clone();
        out
    }

    /// Vertices across all faces.
    pub fn vertex_count(&self) -> usize {
        self.faces.iter().map(|f| f.uv.len()).sum()
    }
}

impl EvalCurve {
    /// Point and first derivative at `t`; `None` if the curve is malformed or its weight
    /// vanishes there.
    pub fn eval(&self, t: f64) -> Option<(Vec3, Vec3)> {
        let p = self.degree;
        let n = self.ctrl.len();
        if p == 0 || p > MAX_SPLINE_DEGREE || n <= p || self.knots.len() != n + p + 1 || !t.is_finite() {
            return None;
        }
        let span = span_of(&self.knots, p, n, t);
        let ders = basis_ders(&self.knots, span, p, t)?;
        let (mut a, mut da) = ([0.0; 4], [0.0; 4]);
        for (k, (n0, n1)) in ders[0].iter().zip(&ders[1]).take(p + 1).enumerate() {
            let c = self.ctrl.get(span + k - p)?;
            for d in 0..4 {
                a[d] += n0 * c[d];
                da[d] += n1 * c[d];
            }
        }
        rational(a, da)
    }
}

impl EvalSurface {
    /// Point and unit normal (`S_u × S_v`, oriented as the surface) at `(u, v)`. A degenerate
    /// point (a cone's apex, a sphere's pole) takes its normal from a step inside; anything
    /// that still fails gives the origin and +Z rather than a NaN.
    pub fn eval(&self, u: f64, v: f64) -> (Vec3, Vec3) {
        let Some((p, du, dv)) = self.derivs(u, v) else { return (Vec3::ZERO, Vec3::Z) };
        let n = self.oriented(du.cross(dv));
        if let Some(n) = n.normalized().filter(|_| n.len2() > 1e-20 * (du.len2() * dv.len2()).max(1e-300)) {
            return (p, n);
        }
        // Step into the parameter domain around the degenerate point.
        let h = 1e-6 * (1.0 + u.abs().max(v.abs()));
        for (su, sv) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            if let Some((_, a, b)) = self.derivs(u + su * h, v + sv * h)
                && let Some(n) = self.oriented(a.cross(b)).normalized()
            {
                return (p, n);
            }
        }
        (p, Vec3::Z)
    }

    /// Mirror transforms reverse the cross product of the mapped derivatives.
    fn oriented(&self, n: Vec3) -> Vec3 {
        match self {
            EvalSurface::Revolved { transform: m, .. } if det3(m) < 0.0 => -n,
            _ => n,
        }
    }

    /// Point and first partial derivatives.
    pub fn derivs(&self, u: f64, v: f64) -> Option<(Vec3, Vec3, Vec3)> {
        if !(u.is_finite() && v.is_finite()) {
            return None;
        }
        match self {
            EvalSurface::Plane { origin, du, dv } => Some((*origin + *du * u + *dv * v, *du, *dv)),
            EvalSurface::Nurbs { degree, knots, count, ctrl, closed } => {
                // The knot range of direction `d`: the parameters the surface is defined over.
                let range = |d: usize| -> Option<(f64, f64)> {
                    let k = knots.get(d)?;
                    Some((*k.get(*degree.get(d)?)?, *k.get(*count.get(d)?)?))
                };
                // A direction the surface joins itself along repeats, so a parameter outside the
                // range is moved by whole ranges back into it. Any other direction is left alone.
                let wrap = |d: usize, t: f64| match range(d) {
                    Some((a, b)) if closed.get(d).copied().unwrap_or(false) && b > a && !(a..=b).contains(&t) => a + (t - a).rem_euclid(b - a),
                    _ => t,
                };
                nurbs_derivs(*degree, knots, *count, ctrl, wrap(0, u), wrap(1, v))
            }
            EvalSurface::Revolved { profile, origin, axis, transform, swap } => {
                let (s, a) = if *swap { (v, u) } else { (u, v) };
                let (c, dc) = profile.eval(s)?;
                let (sin, cos) = a.sin_cos();
                let rot = |x: Vec3| x * cos + axis.cross(x) * sin + *axis * (axis.dot(x) * (1.0 - cos));
                let r = c - *origin;
                let p = *origin + rot(r);
                let ds = rot(dc);
                // d/dθ of the rotated point: axis × (rotated radius).
                let da = axis.cross(rot(r));
                let (p, ds, da) = (affine(transform, p), linear(transform, ds), linear(transform, da));
                Some(if *swap { (p, da, ds) } else { (p, ds, da) })
            }
        }
    }
}

fn affine(m: &[[f64; 4]; 3], p: Vec3) -> Vec3 {
    linear(m, p) + Vec3::new(m[0][3], m[1][3], m[2][3])
}

fn linear(m: &[[f64; 4]; 3], v: Vec3) -> Vec3 {
    let row = |r: &[f64; 4]| r[0] * v.x + r[1] * v.y + r[2] * v.z;
    Vec3::new(row(&m[0]), row(&m[1]), row(&m[2]))
}

fn det3(m: &[[f64; 4]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// The point and derivative of a rational function from its homogeneous value and derivative.
fn rational(a: [f64; 4], da: [f64; 4]) -> Option<(Vec3, Vec3)> {
    let w = a[3];
    if !(w.abs() > 1e-300) {
        return None;
    }
    let p = Vec3::new(a[0] / w, a[1] / w, a[2] / w);
    let d = (Vec3::new(da[0], da[1], da[2]) - p * da[3]) * (1.0 / w);
    (p.is_finite() && d.is_finite()).then_some((p, d))
}

fn nurbs_derivs(degree: [usize; 2], knots: &[Vec<f64>; 2], count: [usize; 2], ctrl: &[[f64; 4]], u: f64, v: f64) -> Option<(Vec3, Vec3, Vec3)> {
    let [pu, pv] = degree;
    let [nu, nv] = count;
    let ok = |p: usize, n: usize, k: &[f64]| (1..=MAX_SPLINE_DEGREE).contains(&p) && n > p && k.len() == n + p + 1;
    if !ok(pu, nu, &knots[0]) || !ok(pv, nv, &knots[1]) || nu.checked_mul(nv)? != ctrl.len() {
        return None;
    }
    let (su, sv) = (span_of(&knots[0], pu, nu, u), span_of(&knots[1], pv, nv, v));
    let (bu, bv) = (basis_ders(&knots[0], su, pu, u)?, basis_ders(&knots[1], sv, pv, v)?);
    let (mut s, mut s_u, mut s_v) = ([0.0; 4], [0.0; 4], [0.0; 4]);
    for (b, (nv0, nv1)) in bv[0].iter().zip(&bv[1]).take(pv + 1).enumerate() {
        for (a, (nu0, nu1)) in bu[0].iter().zip(&bu[1]).take(pu + 1).enumerate() {
            let c = ctrl.get((sv + b - pv) * nu + su + a - pu)?;
            for d in 0..4 {
                s[d] += nu0 * nv0 * c[d];
                s_u[d] += nu1 * nv0 * c[d];
                s_v[d] += nu0 * nv1 * c[d];
            }
        }
    }
    let (p, du) = rational(s, s_u)?;
    let (_, dv) = rational(s, s_v)?;
    Some((p, du, dv))
}

/// The knot span used at `t`: the last non-empty span starting at or before `t`, clamped to
/// the valid spans `p ≤ k < n` so parameters just outside the domain extend the end spans.
pub(crate) fn span_of(knots: &[f64], p: usize, n: usize, t: f64) -> usize {
    let mut k = p;
    for i in p..n {
        if let (Some(&a), Some(&b)) = (knots.get(i), knots.get(i + 1))
            && a < b
        {
            if a <= t {
                k = i;
            } else {
                break;
            }
        }
    }
    k
}

/// Non-zero basis functions of degree `p` on span `span` and their first derivatives
/// (NURBS Book A2.3 with n = 1). `ders[0][j]` is `N_{span−p+j}`, `ders[1][j]` its derivative.
pub(crate) fn basis_ders(knots: &[f64], span: usize, p: usize, t: f64) -> Option<[[f64; W]; 2]> {
    if p == 0 || p >= W {
        return None;
    }
    let kn = |i: usize| knots.get(i).copied();
    // ndu[j][r]: basis values (upper triangle incl. diagonal), knot differences (lower).
    let mut ndu = [[0.0f64; W]; W];
    let (mut left, mut right) = ([0.0f64; W], [0.0f64; W]);
    ndu[0][0] = 1.0;
    for j in 1..=p {
        left[j] = t - kn((span + 1).checked_sub(j)?)?;
        right[j] = kn(span + j)? - t;
        let mut saved = 0.0;
        for r in 0..j {
            ndu[j][r] = right[r + 1] + left[j - r];
            let temp = if ndu[j][r] != 0.0 { ndu[r][j - 1] / ndu[j][r] } else { 0.0 };
            ndu[r][j] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        ndu[j][j] = saved;
    }
    let mut ders = [[0.0f64; W]; 2];
    for j in 0..=p {
        ders[0][j] = ndu[j][p];
    }
    // First derivative: p·(N_{r,p−1}/(u_{r+p}−u_r) − N_{r+1,p−1}/(u_{r+p+1}−u_{r+1})).
    for r in 0..=p {
        let mut d = 0.0;
        if r >= 1 {
            let den = ndu[p][r - 1];
            if den != 0.0 {
                d += ndu[r - 1][p - 1] / den;
            }
        }
        if r < p {
            let den = ndu[p][r];
            if den != 0.0 {
                d -= ndu[r][p - 1] / den;
            }
        }
        ders[1][r] = d * p as f64;
    }
    Some(ders)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3, tol: f64) -> bool {
        a.dist(b) <= tol
    }

    fn line(a: Vec3, b: Vec3) -> EvalCurve {
        EvalCurve { degree: 1, knots: vec![0.0, 0.0, 1.0, 1.0], ctrl: vec![[a.x, a.y, a.z, 1.0], [b.x, b.y, b.z, 1.0]] }
    }

    const IDENTITY: [[f64; 4]; 3] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

    #[test]
    fn plane_point_and_normal() {
        let s = EvalSurface::Plane { origin: Vec3::new(1.0, 2.0, 3.0), du: Vec3::X, dv: Vec3::Y };
        let (p, n) = s.eval(2.0, 3.0);
        assert!(close(p, Vec3::new(3.0, 5.0, 3.0), 1e-12));
        assert!(close(n, Vec3::Z, 1e-12));
    }

    #[test]
    fn revolved_line_is_a_cylinder() {
        // A vertical line at radius 2 turned about Z.
        let s = EvalSurface::Revolved {
            profile: line(Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 5.0)),
            origin: Vec3::ZERO,
            axis: Vec3::Z,
            transform: IDENTITY,
            swap: false,
        };
        for &(u, v) in &[(0.0, 0.0), (0.5, 1.0), (1.0, 4.0), (0.25, 6.0)] {
            let (p, n) = s.eval(u, v);
            assert!(close(p, Vec3::new(2.0 * v.cos(), 2.0 * v.sin(), 5.0 * u), 1e-12), "{p:?}");
            // S_u = +Z, S_v = tangential: the normal points inward (−radial) for this profile.
            assert!(close(n, Vec3::new(-v.cos(), -v.sin(), 0.0), 1e-12), "{n:?}");
        }
    }

    #[test]
    fn swapped_parameters_reverse_the_normal() {
        let mk = |swap| EvalSurface::Revolved {
            profile: line(Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 5.0)),
            origin: Vec3::ZERO,
            axis: Vec3::Z,
            transform: IDENTITY,
            swap,
        };
        let (p0, n0) = mk(false).eval(0.3, 1.1);
        let (p1, n1) = mk(true).eval(1.1, 0.3);
        assert!(close(p0, p1, 1e-12));
        assert!(close(n0, -n1, 1e-12));
    }

    #[test]
    fn mirror_transform_keeps_the_normal_outward() {
        let mirror = [[-1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let s = EvalSurface::Revolved {
            profile: line(Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 5.0)),
            origin: Vec3::ZERO,
            axis: Vec3::Z,
            transform: mirror,
            swap: false,
        };
        let (p, n) = s.eval(0.5, 0.0);
        assert!(close(p, Vec3::new(-2.0, 0.0, 2.5), 1e-12));
        // The unmirrored normal at θ = 0 is −X; mirrored it is +X, still pointing to the axis.
        assert!(close(n, Vec3::X, 1e-12), "{n:?}");
    }

    #[test]
    fn rational_quarter_circle_profile_is_exact() {
        // A quarter circle of radius 3 in the XZ plane, turned about Z: a sphere zone.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let profile = EvalCurve {
            degree: 2,
            knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            ctrl: vec![[3.0, 0.0, 0.0, 1.0], [3.0 * w, 0.0, 3.0 * w, w], [0.0, 0.0, 3.0, 1.0]],
        };
        let s = EvalSurface::Revolved { profile, origin: Vec3::ZERO, axis: Vec3::Z, transform: IDENTITY, swap: false };
        for i in 0..=10 {
            for j in 0..=6 {
                let (u, v) = (i as f64 / 10.0, j as f64);
                let (p, n) = s.eval(u, v);
                assert!((p.len() - 3.0).abs() < 1e-12, "{p:?}");
                // At the pole (u = 1) the normal comes from a step inside: within ~1e-6 rad.
                assert!(n.cross(p).len() < 3e-5, "normal {n:?} not radial at {p:?}");
            }
        }
    }

    #[test]
    fn bilinear_nurbs_patch_and_derivatives() {
        // Degree (1, 1) over [0, 2] × [0, 1] with corners lifted: a hyperbolic paraboloid.
        let ctrl = vec![[0.0, 0.0, 0.0, 1.0], [2.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [2.0, 1.0, 1.0, 1.0]];
        let s = EvalSurface::Nurbs {
            degree: [1, 1],
            knots: [vec![0.0, 0.0, 2.0, 2.0], vec![0.0, 0.0, 1.0, 1.0]],
            count: [2, 2],
            ctrl,
            closed: [false, false],
        };
        let Some((p, du, dv)) = s.derivs(1.0, 0.5) else { panic!("evaluates") };
        assert!(close(p, Vec3::new(1.0, 0.5, 0.25), 1e-12), "{p:?}");
        assert!(close(du, Vec3::new(1.0, 0.0, 0.25), 1e-12), "{du:?}");
        assert!(close(dv, Vec3::new(0.0, 1.0, 0.5), 1e-12), "{dv:?}");
    }

    #[test]
    fn cubic_nurbs_matches_the_geom_surface() {
        // Cross-check the evaluator against `bspline::NurbsSurface::eval` on a weighted patch.
        let knots = [vec![0.0, 0.0, 0.0, 0.0, 0.4, 1.0, 1.0, 1.0, 1.0], vec![0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0]];
        let (nu, nv) = (5, 4);
        let mut pts = Vec::new();
        let mut wts = Vec::new();
        for j in 0..nv {
            for i in 0..nu {
                pts.push(Vec3::new(i as f64, j as f64, ((i * 7 + j * 3) % 5) as f64 * 0.3));
                wts.push(1.0 + ((i + 2 * j) % 3) as f64 * 0.25);
            }
        }
        let Ok(reference) = crate::bspline::NurbsSurface::new([3, 2], knots.clone(), &pts, &wts) else { panic!("valid surface") };
        let ctrl = reference.homogeneous().iter().map(|h| [h.x, h.y, h.z, h.w]).collect();
        let s = EvalSurface::Nurbs { degree: [3, 2], knots, count: [nu, nv], ctrl, closed: [false, false] };
        for i in 0..=8 {
            for j in 0..=8 {
                let (u, v) = (i as f64 / 8.0, j as f64 / 4.0);
                let Ok(want) = reference.eval(u, v) else { panic!("in domain") };
                let Some((p, du, dv)) = s.derivs(u, v) else { panic!("evaluates") };
                assert!(close(p, want, 1e-12), "({u}, {v}): {p:?} vs {want:?}");
                // Derivatives against central differences (one-sided at the domain ends).
                let h = 1e-6;
                let fd = |a: (f64, f64), b: (f64, f64)| {
                    let (Some(pa), Some(pb)) = (s.derivs(a.0, a.1), s.derivs(b.0, b.1)) else { panic!("evaluates") };
                    (pb.0 - pa.0) * (1.0 / (b.0 - a.0 + b.1 - a.1))
                };
                let fu = fd((u.max(h) - h, v), ((u + h).min(1.0), v));
                let fv = fd((u, v.max(h) - h), (u, (v + h).min(2.0)));
                assert!(close(du, fu, 1e-4 * (1.0 + du.len())), "du at ({u}, {v}): {du:?} vs {fu:?}");
                assert!(close(dv, fv, 1e-4 * (1.0 + dv.len())), "dv at ({u}, {v}): {dv:?} vs {fv:?}");
            }
        }
    }

    #[test]
    fn hostile_surfaces_do_not_panic() {
        let bad = [
            EvalSurface::Nurbs { degree: [9, 1], knots: [vec![0.0; 3], vec![0.0; 3]], count: [1, 1], ctrl: vec![], closed: [false, false] },
            EvalSurface::Nurbs {
                degree: [1, 1],
                knots: [vec![0.0, 0.0, 1.0, 1.0], vec![0.0, 0.0, 1.0, 1.0]],
                count: [2, 2],
                ctrl: vec![[0.0; 4]; 4],
                closed: [false, false],
            },
            EvalSurface::Revolved {
                profile: EvalCurve { degree: 0, knots: vec![], ctrl: vec![] },
                origin: Vec3::ZERO,
                axis: Vec3::Z,
                transform: IDENTITY,
                swap: false,
            },
        ];
        for s in &bad {
            for &(u, v) in &[(0.5, 0.5), (f64::NAN, 0.0), (1e300, -1e300)] {
                let (p, n) = s.eval(u, v);
                assert!(p.is_finite() && n.is_finite());
            }
        }
    }

    #[test]
    fn evaluate_uses_fixed_points_and_drops_bad_indices() {
        let plan = TessPlan {
            surfaces: vec![EvalSurface::Plane { origin: Vec3::ZERO, du: Vec3::X, dv: Vec3::Y }],
            faces: vec![FacePlan {
                face: 3,
                surface: Some(0),
                uv: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                fixed: vec![Vec3::new(0.0, 0.0, 1e-9)],
                normals: vec![],
                flip: true,
                triangles: vec![[0, 1, 2], [0, 1, 7]],
            }],
            edges: vec![],
            edge_faces: vec![],
        };
        let m = plan.evaluate();
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.positions.first().copied(), Some(Vec3::new(0.0, 0.0, 1e-9)));
        assert_eq!(m.triangles, vec![[0, 1, 2]]);
        assert_eq!(m.tri_face, vec![3]);
        assert!(m.normals.iter().all(|n| close(*n, -Vec3::Z, 1e-12)));
    }
}
