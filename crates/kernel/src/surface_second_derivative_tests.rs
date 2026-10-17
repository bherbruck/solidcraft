//! The vendored truck-geometry's second derivatives of B-spline and NURBS surfaces (`uuder`,
//! `vvder`) against finite differences of the surface itself, for degrees 2 to 5. Upstream's
//! were short of the factor p − 1, so every curvature read from a cubic surface came out half its
//! value (#763: a G2 boundary surface measured half the cylinder's curvature it does carry).
//! These pin the vendored fix, so a refresh of the crate can't quietly bring the error back.

use solidcraft_geom::Vec3;
use truck_modeling as mt;
use truck_modeling::ParametricSurface;

use crate::body::from_p3;

/// A degree-`p` × degree-`q` surface on a 5 × 6 net with an interior knot each way, bent in z.
fn surface(p: usize, q: usize) -> mt::BSplineSurface<mt::Point3> {
    let knots = |d: usize, n: usize| {
        let inner: Vec<f64> = (1..n - d).map(|k| k as f64 / (n - d) as f64).collect();
        let mut k = vec![0.0; d + 1];
        k.extend(inner);
        k.extend(vec![1.0; d + 1]);
        mt::KnotVec::from(k)
    };
    let (nu, nv) = (p + 3, q + 3);
    let rows: Vec<Vec<mt::Point3>> = (0..nu)
        .map(|i| {
            (0..nv)
                .map(|j| {
                    let (x, y) = (i as f64 * 2.0, j as f64 * 1.5);
                    mt::Point3::new(x + 0.3 * y, y, 0.4 * x * x - 0.2 * y * y + 0.1 * x * y)
                })
                .collect()
        })
        .collect();
    mt::BSplineSurface::new((knots(p, nu), knots(q, nv)), rows)
}

/// Second differences of `f` along the parameter line, central inside and one-sided at the ends.
fn second(f: impl Fn(f64) -> Vec3, t: f64) -> Vec3 {
    let h = 1e-4;
    if t < 2.0 * h {
        (f(t) * 2.0 - f(t + h) * 5.0 + f(t + 2.0 * h) * 4.0 - f(t + 3.0 * h)) / (h * h)
    } else if t > 1.0 - 2.0 * h {
        (f(t) * 2.0 - f(t - h) * 5.0 + f(t - 2.0 * h) * 4.0 - f(t - 3.0 * h)) / (h * h)
    } else {
        (f(t - h) - f(t) * 2.0 + f(t + h)) / (h * h)
    }
}

fn v3(x: mt::Vector3) -> Vec3 {
    Vec3::new(x.x, x.y, x.z)
}

#[test]
fn bspline_surface_second_derivatives_match_finite_differences() {
    for p in 2..=5 {
        for q in 2..=5 {
            let s = surface(p, q);
            for (u, v) in [(0.0, 0.0), (0.37, 0.0), (0.5, 0.61), (1.0, 0.2), (0.13, 1.0)] {
                let uu = second(|t| from_p3(s.subs(t, v)), u);
                let vv = second(|t| from_p3(s.subs(u, t)), v);
                let scale = uu.len().max(vv.len()).max(1.0);
                assert!(v3(s.uuder(u, v)).dist(uu) < 1e-4 * scale, "degree ({p}, {q}) uuder at ({u}, {v}): {:?} vs {uu:?}", s.uuder(u, v));
                assert!(v3(s.vvder(u, v)).dist(vv) < 1e-4 * scale, "degree ({p}, {q}) vvder at ({u}, {v}): {:?} vs {vv:?}", s.vvder(u, v));
            }
        }
    }
}

/// A rational surface's second derivatives come from its homogeneous B-spline's: they are right
/// too.
#[test]
fn nurbs_surface_second_derivatives_match_finite_differences() {
    let b = surface(3, 3);
    let (ku, kv) = b.knot_vecs().clone();
    let rows: Vec<Vec<mt::Vector4>> = b
        .control_points()
        .iter()
        .enumerate()
        .map(|(i, row)| {
            row.iter()
                .enumerate()
                .map(|(j, p)| {
                    let w = 1.0 + 0.3 * ((i + 2 * j) % 3) as f64;
                    mt::Vector4::new(p.x * w, p.y * w, p.z * w, w)
                })
                .collect()
        })
        .collect();
    let s = mt::NurbsSurface::new(mt::BSplineSurface::new((ku, kv), rows));
    for (u, v) in [(0.0, 0.0), (0.37, 0.0), (0.5, 0.61), (1.0, 0.2)] {
        let uu = second(|t| from_p3(s.subs(t, v)), u);
        let vv = second(|t| from_p3(s.subs(u, t)), v);
        let scale = uu.len().max(vv.len()).max(1.0);
        assert!(v3(s.uuder(u, v)).dist(uu) < 1e-4 * scale, "uuder at ({u}, {v}): {:?} vs {uu:?}", s.uuder(u, v));
        assert!(v3(s.vvder(u, v)).dist(vv) < 1e-4 * scale, "vvder at ({u}, {v}): {:?} vs {vv:?}", s.vvder(u, v));
    }
}
