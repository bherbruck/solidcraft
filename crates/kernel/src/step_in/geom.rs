//! STEP geometry (ISO 10303-42 entities) to kernel curves and surfaces.
//!
//! Points, placements, lines, conics (ellipses, hyperbolas and parabolas), B-splines (plain and rational), elementary surfaces and
//! swept surfaces, with exact offsets of planes, cylinders, cones, spheres and regular tori.
//! Free-form offsets and offsets through collapsed radii remain explicit import warnings.
//! Edge curves are trimmed to their vertices here, so every kernel edge runs
//! exactly from its start vertex to its end vertex. Lengths are scaled to millimetres while
//! reading.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use mt::{BoundedCurve, Cut, InnerSpace, Invertible, MetricSpace, ParametricCurve, ParametricSurface3D, SearchNearestParameter};
use truck_modeling as mt;

use super::Ctx;
use super::p21::{Entity, Param};

/// Geometry nesting we follow (trimmed curve of a surface curve of …).
const MAX_GEOM_DEPTH: usize = 8;
/// Most control points in one B-spline (per direction product for surfaces).
const MAX_CTRL: usize = 1_000_000;
const MAX_COORD: f64 = 1e9;

pub(crate) type R<T> = std::result::Result<T, String>;

/// An orthonormal frame (AXIS2_PLACEMENT_3D).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub o: mt::Point3,
    pub x: mt::Vector3,
    pub y: mt::Vector3,
    pub z: mt::Vector3,
}

impl Frame {
    pub fn matrix(&self) -> mt::Matrix4 {
        mt::Matrix4::from_cols(self.x.extend(0.0), self.y.extend(0.0), self.z.extend(0.0), self.o.to_homogeneous())
    }
    fn at(&self, cx: f64, cy: f64, cz: f64) -> mt::Point3 {
        self.o + self.x * cx + self.y * cy + self.z * cz
    }
}

/// A curve before trimming.
#[derive(Clone, Debug)]
pub(crate) enum CurveGeo {
    Line {
        p: mt::Point3,
        d: mt::Vector3,
    },
    /// Ellipse (`a == b` for circles) in the frame's XY plane.
    Conic {
        f: Frame,
        a: f64,
        b: f64,
    },
    /// Hyperbola branch `o + a·cosh t·x + b·sinh t·y` in the frame's XY plane.
    Hyperbola {
        f: Frame,
        a: f64,
        b: f64,
    },
    /// Parabola `o + focal·(t²·x + 2t·y)` in the frame's XY plane.
    Parabola {
        f: Frame,
        focal: f64,
    },
    BSpline(mt::BSplineCurve<mt::Point3>),
    Nurbs(mt::NurbsCurve<mt::Vector4>),
}

pub(crate) fn entity<'a>(cx: &Ctx<'a>, id: u64) -> R<&'a Entity> {
    cx.ex.get(id).ok_or_else(|| format!("missing entity #{id}"))
}

fn ent_of<'a>(cx: &Ctx<'a>, p: Option<&Param>, what: &str) -> R<(u64, &'a Entity)> {
    let id = p.and_then(Param::as_ref_id).ok_or_else(|| format!("{what}: expected a reference"))?;
    Ok((id, entity(cx, id)?))
}

fn num(p: Option<&Param>, what: &str) -> R<f64> {
    let x = p.and_then(Param::as_f64).ok_or_else(|| format!("{what}: expected a number"))?;
    if x.is_finite() && x.abs() < MAX_COORD { Ok(x) } else { Err(format!("{what}: {x} out of range")) }
}

fn length(cx: &Ctx, p: Option<&Param>, what: &str) -> R<f64> {
    Ok(num(p, what)? * cx.len)
}

pub(crate) fn point(cx: &Ctx, id: u64) -> R<mt::Point3> {
    let e = entity(cx, id)?;
    if e.name() != "CARTESIAN_POINT" {
        return Err(format!("#{id}: expected CARTESIAN_POINT, found {}", e.name()));
    }
    let c = e.params().get(1).and_then(Param::as_list).ok_or("CARTESIAN_POINT without coordinates")?;
    let g = |i: usize| -> R<f64> { if i < c.len() { num(c.get(i), "coordinate") } else { Ok(0.0) } };
    if c.is_empty() || c.len() > 3 {
        return Err(format!("#{id}: point with {} coordinates", c.len()));
    }
    Ok(mt::Point3::new(g(0)?, g(1)?, g(2)?) * cx.len)
}

fn direction(cx: &Ctx, id: u64) -> R<mt::Vector3> {
    let e = entity(cx, id)?;
    if e.name() != "DIRECTION" {
        return Err(format!("#{id}: expected DIRECTION, found {}", e.name()));
    }
    let c = e.params().get(1).and_then(Param::as_list).ok_or("DIRECTION without ratios")?;
    let g = |i: usize| -> R<f64> { if i < c.len() { num(c.get(i), "direction") } else { Ok(0.0) } };
    let v = mt::Vector3::new(g(0)?, g(1)?, g(2)?);
    let m = v.magnitude();
    if m > 1e-12 && m.is_finite() { Ok(v / m) } else { Err(format!("#{id}: zero direction")) }
}

fn any_perpendicular(z: mt::Vector3) -> mt::Vector3 {
    let a = if z.x.abs() < 0.9 { mt::Vector3::unit_x() } else { mt::Vector3::unit_y() };
    let x = a - z * a.dot(z);
    x / x.magnitude()
}

/// AXIS2_PLACEMENT_3D (or 2D, as a frame in the XY plane).
pub(crate) fn frame(cx: &Ctx, id: u64) -> R<Frame> {
    let e = entity(cx, id)?;
    let p = e.params();
    let o = point(cx, p.get(1).and_then(Param::as_ref_id).ok_or("placement without location")?)?;
    let (z, rf) = match e.name() {
        "AXIS2_PLACEMENT_3D" => {
            let z = match p.get(2).and_then(Param::as_ref_id) {
                Some(d) => direction(cx, d)?,
                None => mt::Vector3::unit_z(),
            };
            (z, p.get(3).and_then(Param::as_ref_id))
        }
        "AXIS2_PLACEMENT_2D" => (mt::Vector3::unit_z(), p.get(2).and_then(Param::as_ref_id)),
        n => return Err(format!("#{id}: expected AXIS2_PLACEMENT_3D, found {n}")),
    };
    let x = match rf {
        Some(d) => {
            let r = direction(cx, d)?;
            let x = r - z * r.dot(z);
            if x.magnitude() > 1e-9 { x / x.magnitude() } else { any_perpendicular(z) }
        }
        None => any_perpendicular(z),
    };
    Ok(Frame { o, x, y: z.cross(x), z })
}

/// AXIS1_PLACEMENT: origin and unit axis.
pub(crate) fn axis1(cx: &Ctx, id: u64) -> R<(mt::Point3, mt::Vector3)> {
    let e = entity(cx, id)?;
    let p = e.params();
    let o = point(cx, p.get(1).and_then(Param::as_ref_id).ok_or("axis without location")?)?;
    let d = match p.get(2).and_then(Param::as_ref_id) {
        Some(d) => direction(cx, d)?,
        None => mt::Vector3::unit_z(),
    };
    Ok((o, d))
}

/// `expected` is the knot count the control points and degree imply. Multiplicities are
/// expanded into one value each, so the sum has to be checked before the expansion: 20 million
/// knots of multiplicity 64 would build a 10 GB vector that the length check then rejects, and
/// an allocation failure aborts the process where an error does not.
fn knot_vec(mults: Option<&Param>, knots: Option<&Param>, expected: usize) -> R<mt::KnotVec> {
    let m: Vec<usize> = mults
        .and_then(Param::as_list)
        .ok_or("B-spline without knot multiplicities")?
        .iter()
        .map(|x| x.as_i64().filter(|v| (1..=64).contains(v)).map(|v| v as usize).ok_or_else(|| "bad knot multiplicity".to_string()))
        .collect::<R<_>>()?;
    let k: Vec<f64> = knots.and_then(Param::as_list).ok_or("B-spline without knots")?.iter().map(|x| num(Some(x), "knot")).collect::<R<_>>()?;
    if m.len() != k.len() || k.is_empty() || k.windows(2).any(|w| w.first() > w.get(1)) {
        return Err("inconsistent knot vector".into());
    }
    if m.len() > expected || m.iter().try_fold(0usize, |a, v| a.checked_add(*v)) != Some(expected) {
        return Err("knot multiplicities do not match the control points".into());
    }
    mt::KnotVec::from_single_multi(k, m).map_err(|e| format!("knot vector: {e}"))
}

fn generated_knots(kind: &str, n_ctrl: usize, degree: usize) -> R<mt::KnotVec> {
    if n_ctrl <= degree {
        return Err("too few control points".into());
    }
    let n = n_ctrl + degree + 1;
    let v: Vec<f64> = match kind {
        "BEZIER" => {
            if n_ctrl != degree + 1 {
                // Piecewise Bézier: segments of `degree` with full interior multiplicity.
                let segs = (n_ctrl - 1) / degree.max(1);
                if segs * degree + 1 != n_ctrl {
                    return Err("Bezier control point count".into());
                }
                let mut v = vec![0.0; degree + 1];
                for s in 1..segs {
                    v.extend(std::iter::repeat_n(s as f64, degree));
                }
                v.extend(std::iter::repeat_n(segs as f64, degree + 1));
                v
            } else {
                let mut v = vec![0.0; degree + 1];
                v.extend(std::iter::repeat_n(1.0, degree + 1));
                v
            }
        }
        "UNIFORM" => (0..n).map(|i| i as f64 - degree as f64).collect(),
        _ => {
            // Quasi-uniform: clamped ends, uniform interior.
            let inner = n_ctrl - degree;
            let mut v = vec![0.0; degree + 1];
            v.extend((1..inner).map(|i| i as f64));
            v.extend(std::iter::repeat_n(inner as f64, degree + 1));
            v
        }
    };
    Ok(mt::KnotVec::from(v))
}

/// The ends of the part of a knot vector where a B-spline of `degree` is defined, when its end
/// knots repeat fewer than `degree + 1` times (an unclamped or periodic knot vector, as
/// OpenCascade writes closed B-splines and `UNIFORM_*` curves and surfaces have): only between
/// knot `degree` and knot `len - degree - 1` do the basis functions sum to one. `None` for a
/// clamped vector, whose whole range is the domain, and for one whose domain is a point.
fn unclamped_ends(kv: &mt::KnotVec, degree: usize) -> Option<(Option<f64>, Option<f64>)> {
    let n = kv.len();
    if n < 2 * (degree + 1) || !(kv[n - 1 - degree] - kv[degree] > mt::TOLERANCE) {
        return None;
    }
    let lo = (kv.multiplicity(0) <= degree).then(|| kv[degree]);
    let hi = (kv.multiplicity(n - 1) <= degree).then(|| kv[n - 1 - degree]);
    (lo.is_some() || hi.is_some()).then_some((lo, hi))
}

/// `c` restricted to its domain, with clamped ends (exact: knots are inserted at the domain's
/// ends and the control points beyond them dropped). The kernel takes a B-spline's whole knot
/// range as its parameter range, and an unclamped curve evaluated outside its domain leaves
/// its control polygon (a rational one divides by zero at the outermost knots).
fn clamped_curve<P: mt::control_point::ControlPoint<f64> + mt::Tolerance>(mut c: mt::BSplineCurve<P>) -> mt::BSplineCurve<P> {
    let Some((lo, hi)) = unclamped_ends(c.knot_vec(), c.degree()) else { return c };
    if let Some(lo) = lo {
        c = c.cut(lo);
    }
    if let Some(hi) = hi {
        let _ = c.cut(hi);
    }
    c
}

/// [`clamped_curve`] for a surface, in each direction whose knot vector is unclamped.
fn clamped_surface<P: mt::control_point::ControlPoint<f64> + mt::Tolerance>(mut s: mt::BSplineSurface<P>) -> mt::BSplineSurface<P> {
    if let Some((lo, hi)) = unclamped_ends(s.uknot_vec(), s.udegree()) {
        if let Some(lo) = lo {
            s = s.ucut(lo);
        }
        if let Some(hi) = hi {
            let _ = s.ucut(hi);
        }
    }
    if let Some((lo, hi)) = unclamped_ends(s.vknot_vec(), s.vdegree()) {
        if let Some(lo) = lo {
            s = s.vcut(lo);
        }
        if let Some(hi) = hi {
            let _ = s.vcut(hi);
        }
    }
    s
}

fn weights_list(p: Option<&Param>) -> R<Vec<f64>> {
    p.and_then(Param::as_list)
        .ok_or("rational B-spline without weights")?
        .iter()
        .map(|w| num(Some(w), "weight").and_then(|w| if w > 0.0 { Ok(w) } else { Err("non-positive weight".into()) }))
        .collect()
}

fn bspline_curve(cx: &Ctx, e: &Entity) -> R<CurveGeo> {
    // Common part (B_SPLINE_CURVE): degree, control points, form, closed, self-intersect.
    let (base, knots, kind): (Vec<Param>, Option<(Option<&Param>, Option<&Param>)>, &str) = if e.is_complex() {
        let b = e.record("B_SPLINE_CURVE").ok_or("complex curve without B_SPLINE_CURVE")?;
        let mut base = vec![Param::Str(String::new())];
        base.extend(b.iter().cloned());
        let k = e.record("B_SPLINE_CURVE_WITH_KNOTS").map(|k| (k.first(), k.get(1)));
        let kind = if e.has("BEZIER_CURVE") {
            "BEZIER"
        } else if e.has("UNIFORM_CURVE") {
            "UNIFORM"
        } else {
            "QUASI"
        };
        (base, k, kind)
    } else {
        let p = e.params();
        let k = (e.name() == "B_SPLINE_CURVE_WITH_KNOTS").then(|| (p.get(6), p.get(7)));
        let kind = match e.name() {
            "BEZIER_CURVE" => "BEZIER",
            "UNIFORM_CURVE" => "UNIFORM",
            _ => "QUASI",
        };
        (p.to_vec(), k, kind)
    };
    let degree = base.get(1).and_then(Param::as_i64).filter(|d| (1..=32).contains(d)).ok_or("bad B-spline degree")? as usize;
    let ctrl_ids = base.get(2).and_then(Param::as_list).ok_or("B-spline without control points")?;
    if ctrl_ids.len() > MAX_CTRL {
        return Err("B-spline with too many control points".into());
    }
    let ctrl: Vec<mt::Point3> = ctrl_ids.iter().map(|c| point(cx, c.as_ref_id().ok_or("bad control point")?)).collect::<R<_>>()?;
    let kv = match knots {
        Some((m, k)) => knot_vec(m, k, ctrl.len() + degree + 1)?,
        None => generated_knots(kind, ctrl.len(), degree)?,
    };
    if kv.len() != ctrl.len() + degree + 1 {
        return Err(format!("B-spline knot count {} does not match {} control points of degree {degree}", kv.len(), ctrl.len()));
    }
    let bsp = mt::BSplineCurve::try_new(kv, ctrl).map_err(|e| format!("B-spline: {e}"))?;
    match e.record("RATIONAL_B_SPLINE_CURVE") {
        Some(r) => {
            let w = weights_list(r.first())?;
            let n = mt::NurbsCurve::<mt::Vector4>::try_from_bspline_and_weights(bsp, w).map_err(|e| format!("rational B-spline: {e}"))?;
            Ok(CurveGeo::Nurbs(mt::NurbsCurve::new(clamped_curve(n.into_non_rationalized()))))
        }
        None => Ok(CurveGeo::BSpline(clamped_curve(bsp))),
    }
}

/// A STEP curve entity (unbounded or bounded).
pub(crate) fn curve(cx: &Ctx, id: u64, depth: usize) -> R<CurveGeo> {
    if depth > MAX_GEOM_DEPTH {
        return Err("curve definitions nested too deeply".into());
    }
    let e = entity(cx, id)?;
    let p = e.params();
    if e.is_complex() {
        return bspline_curve(cx, e);
    }
    match e.name() {
        "LINE" => {
            let pt = point(cx, p.get(1).and_then(Param::as_ref_id).ok_or("LINE without point")?)?;
            let (_, v) = ent_of(cx, p.get(2), "LINE vector")?;
            let d = direction(cx, v.params().get(1).and_then(Param::as_ref_id).ok_or("VECTOR without direction")?)?;
            Ok(CurveGeo::Line { p: pt, d })
        }
        "CIRCLE" => {
            let f = frame(cx, p.get(1).and_then(Param::as_ref_id).ok_or("CIRCLE without placement")?)?;
            let r = length(cx, p.get(2), "radius")?;
            if r <= 0.0 {
                return Err("circle radius must be positive".into());
            }
            Ok(CurveGeo::Conic { f, a: r, b: r })
        }
        "ELLIPSE" => {
            let f = frame(cx, p.get(1).and_then(Param::as_ref_id).ok_or("ELLIPSE without placement")?)?;
            let (a, b) = (length(cx, p.get(2), "semi axis")?, length(cx, p.get(3), "semi axis")?);
            if a <= 0.0 || b <= 0.0 {
                return Err("ellipse axes must be positive".into());
            }
            Ok(CurveGeo::Conic { f, a, b })
        }
        "HYPERBOLA" => {
            let f = frame(cx, p.get(1).and_then(Param::as_ref_id).ok_or("HYPERBOLA without placement")?)?;
            let (a, b) = (length(cx, p.get(2), "semi axis")?, length(cx, p.get(3), "semi imaginary axis")?);
            if a <= 0.0 || b <= 0.0 {
                return Err("hyperbola axes must be positive".into());
            }
            Ok(CurveGeo::Hyperbola { f, a, b })
        }
        "PARABOLA" => {
            let f = frame(cx, p.get(1).and_then(Param::as_ref_id).ok_or("PARABOLA without placement")?)?;
            let focal = length(cx, p.get(2), "focal distance")?;
            if focal == 0.0 {
                return Err("parabola focal distance must not be zero".into());
            }
            Ok(CurveGeo::Parabola { f, focal })
        }
        "B_SPLINE_CURVE_WITH_KNOTS" | "BEZIER_CURVE" | "UNIFORM_CURVE" | "QUASI_UNIFORM_CURVE" => bspline_curve(cx, e),
        "POLYLINE" => {
            let pts: Vec<mt::Point3> = p
                .get(1)
                .and_then(Param::as_list)
                .ok_or("POLYLINE without points")?
                .iter()
                .map(|c| point(cx, c.as_ref_id().ok_or("bad polyline point")?))
                .collect::<R<_>>()?;
            if pts.len() < 2 || pts.len() > MAX_CTRL {
                return Err("polyline point count".into());
            }
            let mut k = vec![0.0];
            k.extend((0..pts.len()).map(|i| i as f64));
            k.push((pts.len() - 1) as f64);
            Ok(CurveGeo::BSpline(mt::BSplineCurve::try_new(mt::KnotVec::from(k), pts).map_err(|e| format!("polyline: {e}"))?))
        }
        "TRIMMED_CURVE" => curve(cx, p.get(1).and_then(Param::as_ref_id).ok_or("TRIMMED_CURVE without basis")?, depth + 1),
        "SURFACE_CURVE" | "SEAM_CURVE" | "INTERSECTION_CURVE" | "BOUNDED_SURFACE_CURVE" => {
            curve(cx, p.get(1).and_then(Param::as_ref_id).ok_or("surface curve without 3D curve")?, depth + 1)
        }
        n => Err(format!("unsupported curve {n}")),
    }
}

/// Rational quadratic NURBS arc of an ellipse from angle `t0` over `sweep > 0`.
pub(crate) fn conic_arc(f: &Frame, a: f64, b: f64, t0: f64, sweep: f64) -> mt::NurbsCurve<mt::Vector4> {
    let n = ((sweep / FRAC_PI_2).ceil() as usize).clamp(1, 8);
    let dt = sweep / n as f64;
    let w = (dt * 0.5).cos();
    let at = |c: f64, s: f64| f.at(a * c, b * s, 0.0);
    let mut ctrl = Vec::with_capacity(2 * n + 1);
    let p0 = at(t0.cos(), t0.sin());
    ctrl.push(p0.to_homogeneous());
    let mut knots = vec![0.0, 0.0, 0.0];
    for i in 0..n {
        let ta = t0 + dt * i as f64;
        let tm = ta + dt * 0.5;
        let tb = ta + dt;
        let m = at(tm.cos() / w, tm.sin() / w);
        ctrl.push(mt::Vector4::new(m.x * w, m.y * w, m.z * w, w));
        ctrl.push(at(tb.cos(), tb.sin()).to_homogeneous());
        let k = (i + 1) as f64 / n as f64;
        knots.push(k);
        knots.push(k);
    }
    // Last knot pair becomes the clamped end (multiplicity 3).
    knots.push(1.0);
    mt::NurbsCurve::new(mt::BSplineCurve::new(mt::KnotVec::from(knots), ctrl))
}

/// Largest hyperbola parameter step one rational quadratic piece spans (keeps its weight
/// `cosh(step / 2)` near 1).
const HYPERBOLA_STEP: f64 = 1.0;

/// Exact rational quadratic NURBS of the hyperbola branch from parameter `t0` to `t1` (either
/// order): pieces whose middle control point is the meeting point of the end tangents, weighted
/// `cosh` of the half step (the hyperbolic counterpart of [`conic_arc`]).
pub(crate) fn hyperbola_arc(f: &Frame, a: f64, b: f64, t0: f64, t1: f64) -> mt::NurbsCurve<mt::Vector4> {
    let n = (((t1 - t0).abs() / HYPERBOLA_STEP).ceil() as usize).clamp(1, 64);
    let dt = (t1 - t0) / n as f64;
    let w = (dt * 0.5).cosh();
    let at = |c: f64, s: f64| f.at(a * c, b * s, 0.0);
    let mut ctrl = Vec::with_capacity(2 * n + 1);
    ctrl.push(at(t0.cosh(), t0.sinh()).to_homogeneous());
    let mut knots = vec![0.0, 0.0, 0.0];
    for i in 0..n {
        let ta = t0 + dt * i as f64;
        let tm = ta + dt * 0.5;
        let tb = ta + dt;
        let m = at(tm.cosh() / w, tm.sinh() / w);
        ctrl.push(mt::Vector4::new(m.x * w, m.y * w, m.z * w, w));
        ctrl.push(at(tb.cosh(), tb.sinh()).to_homogeneous());
        let k = (i + 1) as f64 / n as f64;
        knots.push(k);
        knots.push(k);
    }
    knots.push(1.0);
    mt::NurbsCurve::new(mt::BSplineCurve::new(mt::KnotVec::from(knots), ctrl))
}

/// The parabola from parameter `t0` to `t1` (either order): one quadratic Bézier, exact.
pub(crate) fn parabola_arc(f: &Frame, focal: f64, t0: f64, t1: f64) -> mt::BSplineCurve<mt::Point3> {
    let at = |t: f64| f.at(focal * t * t, 2.0 * focal * t, 0.0);
    // The end tangents meet above the parameter midpoint: o + focal·(t0·t1, t0 + t1).
    let mid = f.at(focal * t0 * t1, focal * (t0 + t1), 0.0);
    mt::BSplineCurve::new(mt::KnotVec::bezier_knot(2), vec![at(t0), mid, at(t1)])
}

/// Parameter of the open conic at `p`, a point on (or within `tol` of) the curve.
fn open_conic_param(g: &CurveGeo, p: mt::Point3, tol: f64) -> R<f64> {
    let (t, q) = match g {
        CurveGeo::Hyperbola { f, a, b } => {
            let t = ((p - f.o).dot(f.y) / b).asinh();
            (t, f.at(a * t.cosh(), b * t.sinh(), 0.0))
        }
        CurveGeo::Parabola { f, focal } => {
            let t = (p - f.o).dot(f.y) / (2.0 * focal);
            (t, f.at(focal * t * t, 2.0 * focal * t, 0.0))
        }
        _ => return Err("not an open conic".into()),
    };
    let d = q.distance(p);
    if !t.is_finite() || !(d <= tol) {
        return Err(format!(
            "edge vertex is {d:.3e} mm off its {} curve",
            if matches!(g, CurveGeo::Parabola { .. }) { "parabola" } else { "hyperbola" }
        ));
    }
    Ok(t)
}

/// The open conic between parameters `t0` and `t1` as a kernel curve.
fn open_conic_curve(g: &CurveGeo, t0: f64, t1: f64) -> R<mt::Curve> {
    match g {
        CurveGeo::Hyperbola { f, a, b } => Ok(mt::Curve::NurbsCurve(hyperbola_arc(f, *a, *b, t0, t1))),
        CurveGeo::Parabola { f, focal } => Ok(mt::Curve::BSplineCurve(parabola_arc(f, *focal, t0, t1))),
        _ => Err("not an open conic".into()),
    }
}

fn conic_angle(f: &Frame, a: f64, b: f64, p: mt::Point3) -> f64 {
    let d = p - f.o;
    (d.dot(f.y) / b).atan2(d.dot(f.x) / a)
}

/// Parameter of the curve point nearest to `p`, and its distance.
fn nearest_param<C>(c: &C, p: mt::Point3) -> Option<(f64, f64)>
where
    C: ParametricCurve<Point = mt::Point3> + BoundedCurve + SearchNearestParameter<mt::D1, Point = mt::Point3>,
{
    let (t0, t1) = c.range_tuple();
    let n = 256;
    let mut best = (t0, f64::INFINITY);
    for i in 0..=n {
        let t = t0 + (t1 - t0) * i as f64 / n as f64;
        let d = c.subs(t).distance(p);
        if d < best.1 {
            best = (t, d);
        }
    }
    if let Some(t) = c.search_nearest_parameter(p, best.0, 64) {
        let t = t.clamp(t0, t1);
        let d = c.subs(t).distance(p);
        if d < best.1 {
            best = (t, d);
        }
    }
    best.1.is_finite().then_some(best)
}

/// Sub-curve on `[a, b]` (inside the curve's range).
fn sub_curve<C: Cut + BoundedCurve + Clone>(c: &C, a: f64, b: f64) -> C {
    let (t0, t1) = c.range_tuple();
    let eps = (t1 - t0).abs() * 1e-9;
    let mut out = c.clone();
    if a > t0 + eps {
        out = out.cut(a);
    }
    if b < t1 - eps {
        let _ = out.cut(b);
    }
    out
}

/// Pieces of a spline edge from `ps` to `pe` (two pieces for a closed edge).
fn spline_pieces<C>(c: &C, ps: mt::Point3, pe: mt::Point3, sense: bool, closed: bool, tol: f64) -> R<Vec<C>>
where
    C: ParametricCurve<Point = mt::Point3> + BoundedCurve + SearchNearestParameter<mt::D1, Point = mt::Point3> + Cut + Clone + Invertible,
{
    let (t0, t1) = c.range_tuple();
    let (ts, ds) = nearest_param(c, ps).ok_or("spline evaluation failed")?;
    let (te, de) = nearest_param(c, pe).ok_or("spline evaluation failed")?;
    if ds > tol || de > tol {
        return Err(format!("edge vertices are {:.3e} mm off their curve", ds.max(de)));
    }
    let span = t1 - t0;
    let near = |a: f64, b: f64| (a - b).abs() <= span * 1e-7;
    let mut pieces = if closed {
        // Whole closed curve starting at the vertex.
        if near(ts, t0) || near(ts, t1) {
            let m = t0 + span * 0.5;
            vec![sub_curve(c, t0, m), sub_curve(c, m, t1)]
        } else {
            vec![sub_curve(c, ts, t1), sub_curve(c, t0, ts)]
        }
    } else {
        let (mut a, mut b) = if sense { (ts, te) } else { (te, ts) };
        // A closed curve traversed across its seam.
        if a >= b && c.subs(t0).distance(c.subs(t1)) <= tol {
            if near(b, t0) {
                b = t1;
            } else if near(a, t1) {
                a = t0;
            } else {
                let mut v = vec![sub_curve(c, a, t1), sub_curve(c, t0, b)];
                if !sense {
                    v.reverse();
                    v.iter_mut().for_each(|x| x.invert());
                }
                return Ok(v);
            }
        }
        if b - a <= span * 1e-9 {
            return Err("degenerate spline edge".into());
        }
        vec![sub_curve(c, a, b)]
    };
    if !sense {
        pieces.reverse();
        pieces.iter_mut().for_each(|x| x.invert());
    }
    Ok(pieces)
}

/// Kernel curves for an edge from `ps` to `pe`: one piece, or two for a closed edge (so no
/// kernel edge starts and ends at the same vertex).
pub(crate) fn edge_pieces(g: &CurveGeo, ps: mt::Point3, pe: mt::Point3, sense: bool, closed: bool, tol: f64) -> R<Vec<mt::Curve>> {
    match g {
        CurveGeo::Line { .. } => {
            if closed {
                return Err("closed straight edge".into());
            }
            Ok(vec![mt::Curve::Line(mt::Line(ps, pe))])
        }
        CurveGeo::Conic { f, a, b } => {
            let (a, b) = (*a, *b);
            let ts = conic_angle(f, a, b, ps);
            let te = conic_angle(f, a, b, pe);
            let mut sweep = if sense { te - ts } else { ts - te };
            sweep = sweep.rem_euclid(TAU);
            if closed || sweep < 1e-12 {
                sweep = TAU;
            }
            let pieces: Vec<(f64, f64)> = if sweep > PI * 1.5 { vec![(0.0, sweep * 0.5), (sweep * 0.5, sweep)] } else { vec![(0.0, sweep)] };
            Ok(pieces
                .into_iter()
                .map(|(s0, s1)| {
                    if sense {
                        mt::Curve::NurbsCurve(conic_arc(f, a, b, ts + s0, s1 - s0))
                    } else {
                        let mut c = conic_arc(f, a, b, ts - s1, s1 - s0);
                        c.invert();
                        mt::Curve::NurbsCurve(c)
                    }
                })
                .collect())
        }
        CurveGeo::Hyperbola { .. } | CurveGeo::Parabola { .. } => {
            if closed {
                return Err("closed edge on an open conic".into());
            }
            // The curve is open, so the edge is the parameters between its vertices: rising
            // for the same sense, falling for the opposite one.
            let (ts, te) = (open_conic_param(g, ps, tol)?, open_conic_param(g, pe, tol)?);
            let scale = 1.0 + ts.abs().max(te.abs());
            if (te - ts).abs() <= 1e-9 * scale {
                return Err("degenerate conic edge".into());
            }
            Ok(vec![open_conic_curve(g, ts, te)?])
        }
        CurveGeo::BSpline(c) => Ok(spline_pieces(c, ps, pe, sense, closed, tol)?.into_iter().map(mt::Curve::BSplineCurve).collect()),
        CurveGeo::Nurbs(c) => Ok(spline_pieces(c, ps, pe, sense, closed, tol)?.into_iter().map(mt::Curve::NurbsCurve).collect()),
    }
}

/// A bounded kernel curve for a swept surface's profile (lines are bounded by `extent`).
fn bounded_curve(g: &CurveGeo, extent: &[mt::Point3]) -> R<mt::Curve> {
    Ok(match g {
        CurveGeo::Line { p, d } => {
            let (lo, hi) = span_along(extent, *p, *d);
            mt::Curve::Line(mt::Line(*p + *d * lo, *p + *d * hi))
        }
        CurveGeo::Conic { f, a, b } => mt::Curve::NurbsCurve(conic_arc(f, *a, *b, 0.0, TAU)),
        CurveGeo::Hyperbola { f, .. } | CurveGeo::Parabola { f, .. } => {
            // Bounded by the face's points (their parameters along the curve), with a margin.
            let along = |q: &mt::Point3| match g {
                CurveGeo::Hyperbola { b, .. } => ((q - f.o).dot(f.y) / b).asinh(),
                CurveGeo::Parabola { focal, .. } => (q - f.o).dot(f.y) / (2.0 * focal),
                _ => 0.0,
            };
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for t in extent.iter().map(along).filter(|t| t.is_finite()) {
                lo = lo.min(t);
                hi = hi.max(t);
            }
            let (lo, hi) = if lo < hi { (lo - (hi - lo) * 0.25 - 1e-3, hi + (hi - lo) * 0.25 + 1e-3) } else { (-1.0, 1.0) };
            open_conic_curve(g, lo, hi)?
        }
        CurveGeo::BSpline(c) => mt::Curve::BSplineCurve(c.clone()),
        CurveGeo::Nurbs(c) => mt::Curve::NurbsCurve(c.clone()),
    })
}

/// Range of `(q - o)·d` over the points, widened by a margin.
fn span_along(pts: &[mt::Point3], o: mt::Point3, d: mt::Vector3) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for q in pts {
        let t = (q - o).dot(d);
        lo = lo.min(t);
        hi = hi.max(t);
    }
    if !(lo.is_finite() && hi.is_finite()) {
        return (-1.0, 1.0);
    }
    let m = (hi - lo) * 0.25 + 1e-3;
    (lo - m, hi + m)
}

/// The angle shared by all the given angles (within 1e-4 rad), if there are any.
fn common_angle(angles: impl Iterator<Item = f64>) -> Option<f64> {
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0usize);
    let mut all = Vec::new();
    for a in angles {
        sx += a.cos();
        sy += a.sin();
        n += 1;
        all.push(a);
    }
    if n == 0 {
        return None;
    }
    let m = sy.atan2(sx);
    let close = all.iter().all(|a| {
        let d = (a - m).rem_euclid(TAU);
        d.min(TAU - d) < 1e-4
    });
    close.then_some(m)
}

/// Middle and width of the largest gap between angles (radians, any range).
fn largest_gap(angles: &[f64]) -> Option<(f64, f64)> {
    let mut a: Vec<f64> = angles.iter().map(|x| x.rem_euclid(TAU)).filter(|x| x.is_finite()).collect();
    if a.is_empty() {
        return None;
    }
    a.sort_by(f64::total_cmp);
    let (mut best, mut mid) = (0.0, 0.0);
    for i in 0..a.len() {
        let (Some(&x), Some(&y)) = (a.get(i), a.get((i + 1) % a.len())) else { continue };
        let gap = if i + 1 == a.len() { y + TAU - x } else { y - x };
        if gap > best {
            best = gap;
            mid = x + gap * 0.5;
        }
    }
    Some((mid, best))
}

/// The frame turned about its Z axis so that its X axis (the parameter seam of a revolved
/// surface) lies in the widest angular gap of the face boundary.
fn seam_frame(f: &Frame, extent: &[mt::Point3], seam: &[mt::Point3]) -> Frame {
    // A seam edge of the face along the rotation: the parameter seam must be there.
    if let Some(m) = common_angle(seam.iter().filter_map(|p| {
        let d = p - f.o;
        let (x, y) = (d.dot(f.x), d.dot(f.y));
        (x.hypot(y) > 1e-6 * (1.0 + d.magnitude())).then(|| y.atan2(x))
    })) {
        let x = f.x * m.cos() + f.y * m.sin();
        return Frame { o: f.o, x, y: f.z.cross(x), z: f.z };
    }
    let az: Vec<f64> = extent
        .iter()
        .filter_map(|p| {
            let d = p - f.o;
            let (x, y) = (d.dot(f.x), d.dot(f.y));
            (x.hypot(y) > 1e-9).then(|| y.atan2(x))
        })
        .collect();
    match largest_gap(&az) {
        Some((m, gap)) if gap > 1e-3 => {
            let x = f.x * m.cos() + f.y * m.sin();
            Frame { o: f.o, x, y: f.z.cross(x), z: f.z }
        }
        _ => *f,
    }
}

/// Length of the mean unit direction from the frame origin to the points (1: all in one
/// direction, 0: spread all round).
fn mean_direction(f: &Frame, pts: &[mt::Point3]) -> f64 {
    let dirs: Vec<mt::Vector3> = pts
        .iter()
        .filter_map(|p| {
            let d = p - f.o;
            let m = d.magnitude();
            (m > 1e-12).then(|| d / m)
        })
        .collect();
    if dirs.is_empty() {
        return 0.0;
    }
    (dirs.iter().fold(mt::Vector3::new(0.0, 0.0, 0.0), |a, d| a + d) / dirs.len() as f64).magnitude()
}

/// Polar axis for a sphere: far from every boundary point, and (for a face concentrated on
/// one side) with both poles outside the face, so every loop closes in parameter space.
fn sphere_axis(f: &Frame, extent: &[mt::Point3]) -> mt::Vector3 {
    let dirs: Vec<mt::Vector3> = extent
        .iter()
        .filter_map(|p| {
            let d = p - f.o;
            let m = d.magnitude();
            (m > 1e-12).then(|| d / m)
        })
        .collect();
    if dirs.is_empty() {
        return f.z;
    }
    let mean = dirs.iter().fold(mt::Vector3::new(0.0, 0.0, 0.0), |a, d| a + d) / dirs.len() as f64;
    let concentrated = mean.magnitude() > 0.3;
    let mhat = if concentrated { mean / mean.magnitude() } else { mean };
    let n = 400;
    let golden = PI * (3.0 - 5f64.sqrt());
    let mut best = (f64::NEG_INFINITY, f.z);
    // Half of a Fibonacci sphere (an axis and its opposite are the same poles).
    for i in 0..n {
        let z = 1.0 - (i as f64 + 0.5) / n as f64;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let t = golden * i as f64;
        let a = mt::Vector3::new(r * t.cos(), r * t.sin(), z);
        if concentrated && a.dot(mhat).abs() > 0.35 {
            continue;
        }
        let score = dirs.iter().map(|d| 1.0 - d.dot(a).abs()).fold(f64::INFINITY, f64::min);
        if score > best.0 {
            best = (score, a);
        }
    }
    best.1
}

/// Tube angle of a point on a torus.
fn tube_angle(f: &Frame, big: f64, p: mt::Point3) -> f64 {
    let d = p - f.o;
    let z = d.dot(f.z);
    let rho = (d - f.z * z).magnitude();
    z.atan2(rho - big)
}

/// A straight profile of a surface of revolution, bounded by the face: boundary points are
/// turned into the profile's meridian plane and projected onto the line, and the line stops at
/// the axis (beyond it the surface would fold over itself).
fn revolved_line(p: mt::Point3, d: mt::Vector3, o: mt::Point3, axis: mt::Vector3, extent: &[mt::Point3]) -> R<mt::Curve> {
    let radial = |q: mt::Point3| {
        let v = q - o;
        v - axis * v.dot(axis)
    };
    // The meridian half-plane of the profile.
    let e = {
        let r = radial(p);
        let r2 = radial(p + d);
        let r = if r.magnitude() >= r2.magnitude() { r } else { r2 };
        if r.magnitude() < 1e-12 {
            return Err("profile line on the axis".into());
        }
        r / r.magnitude()
    };
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for q in extent {
        let v = q - o;
        let h = v.dot(axis);
        let r = radial(*q).magnitude();
        let t = (o + axis * h + e * r - p).dot(d);
        lo = lo.min(t);
        hi = hi.max(t);
    }
    if !(lo.is_finite() && hi.is_finite()) {
        return Err("surface of revolution without extent".into());
    }
    let m = (hi - lo) * 0.02 + 1e-6;
    let (mut lo, mut hi) = (lo - m, hi + m);
    // Where the line meets the axis (radial component along e vanishes).
    let de = d.dot(e);
    if de.abs() > 1e-12 {
        let t_axis = -(p - o).dot(e) / de;
        // The meridian half-plane has a non-negative radius along `e`.
        if de > 0.0 {
            lo = lo.max(t_axis);
        } else {
            hi = hi.min(t_axis);
        }
    }
    if hi - lo <= 1e-12 {
        return Err("degenerate surface of revolution".into());
    }
    Ok(mt::Curve::Line(mt::Line(p + d * lo, p + d * hi)))
}

fn revolved(curve: mt::Curve, o: mt::Point3, axis: mt::Vector3) -> mt::Surface {
    mt::Surface::RevolutedCurve(mt::Processor::new(mt::RevolutedCurve::by_revolution(curve, o, axis)))
}

/// A surface of revolution whose normal at the profile point `(u, 0)` points along `expected`.
/// The rotation is reversed rather than the surface inverted: truck's inverted `Processor`
/// swaps the parameters but not the search hints, which breaks meshing.
pub(crate) fn revolved_oriented(curve: mt::Curve, o: mt::Point3, axis: mt::Vector3, u: f64, expected: mt::Vector3) -> mt::Surface {
    let s = revolved(curve.clone(), o, axis);
    if s.normal(u, 0.0).dot(expected) >= 0.0 {
        return s;
    }
    revolved(curve, o, -axis)
}

pub(crate) fn bspline_surface(cx: &Ctx, e: &Entity) -> R<mt::Surface> {
    let (base, knots, kind): (Vec<Param>, Option<[Option<&Param>; 4]>, &str) = if e.is_complex() {
        let b = e.record("B_SPLINE_SURFACE").ok_or("complex surface without B_SPLINE_SURFACE")?;
        let mut base = vec![Param::Str(String::new())];
        base.extend(b.iter().cloned());
        let k = e.record("B_SPLINE_SURFACE_WITH_KNOTS").map(|k| [k.first(), k.get(1), k.get(2), k.get(3)]);
        let kind = if e.has("BEZIER_SURFACE") {
            "BEZIER"
        } else if e.has("UNIFORM_SURFACE") {
            "UNIFORM"
        } else {
            "QUASI"
        };
        (base, k, kind)
    } else {
        let p = e.params();
        let k = (e.name() == "B_SPLINE_SURFACE_WITH_KNOTS").then(|| [p.get(8), p.get(9), p.get(10), p.get(11)]);
        let kind = match e.name() {
            "BEZIER_SURFACE" => "BEZIER",
            "UNIFORM_SURFACE" => "UNIFORM",
            _ => "QUASI",
        };
        (p.to_vec(), k, kind)
    };
    let deg = |i: usize| base.get(i).and_then(Param::as_i64).filter(|d| (1..=32).contains(d)).map(|d| d as usize).ok_or("bad B-spline degree");
    let (du, dv) = (deg(1)?, deg(2)?);
    let rows = base.get(3).and_then(Param::as_list).ok_or("B-spline surface without control points")?;
    let mut ctrl: Vec<Vec<mt::Point3>> = Vec::with_capacity(rows.len());
    let mut total = 0usize;
    for r in rows {
        let r = r.as_list().ok_or("bad control point row")?;
        total += r.len();
        if total > MAX_CTRL {
            return Err("B-spline surface with too many control points".into());
        }
        ctrl.push(r.iter().map(|c| point(cx, c.as_ref_id().ok_or("bad control point")?)).collect::<R<_>>()?);
    }
    let nu = ctrl.len();
    let nv = ctrl.first().map(Vec::len).unwrap_or(0);
    if nu == 0 || ctrl.iter().any(|r| r.len() != nv) {
        return Err("ragged B-spline control net".into());
    }
    let (ku, kv) = match knots {
        Some([mu, mv, ku, kv]) => (knot_vec(mu, ku, nu + du + 1)?, knot_vec(mv, kv, nv + dv + 1)?),
        None => (generated_knots(kind, nu, du)?, generated_knots(kind, nv, dv)?),
    };
    if ku.len() != nu + du + 1 || kv.len() != nv + dv + 1 {
        return Err("B-spline surface knot counts do not match the control net".into());
    }
    let bsp = mt::BSplineSurface::try_new((ku, kv), ctrl).map_err(|e| format!("B-spline surface: {e}"))?;
    match e.record("RATIONAL_B_SPLINE_SURFACE") {
        Some(r) => {
            let w: Vec<Vec<f64>> = r
                .first()
                .and_then(Param::as_list)
                .ok_or("rational surface without weights")?
                .iter()
                .map(|row| weights_list(Some(row)))
                .collect::<R<_>>()?;
            if w.len() != nu || w.iter().any(|r| r.len() != nv) {
                return Err("weights do not match the control net".into());
            }
            let n = mt::NurbsSurface::<mt::Vector4>::try_from_bspline_and_weights(bsp, w).map_err(|e| format!("rational surface: {e}"))?;
            Ok(mt::Surface::NurbsSurface(mt::NurbsSurface::new(clamped_surface(n.into_non_rationalized()))))
        }
        None => Ok(mt::Surface::BSplineSurface(clamped_surface(bsp))),
    }
}

/// Resolve exact analytic offsets. Each intermediate offset is checked: an invalid inner
/// offset must not become valid merely because an outer offset cancels its distance.
/// Entity attributes follow ISO 10303-42 (`basis_surface`, `distance`, `self_intersect`).
fn offset_basis(cx: &Ctx, id: u64, depth: usize) -> R<(u64, f64)> {
    if depth > MAX_GEOM_DEPTH {
        return Err("surface definitions nested too deeply".into());
    }
    let e = entity(cx, id)?;
    if e.name() != "OFFSET_SURFACE" {
        return Ok((id, 0.0));
    }
    let p = e.params();
    let basis = p.get(1).and_then(Param::as_ref_id).ok_or("offset surface without basis")?;
    let distance = length(cx, p.get(2), "offset distance")?;
    if !matches!(p.get(3).and_then(Param::as_enum), Some("T" | "F" | "U")) {
        return Err("offset surface without a logical self_intersect flag".into());
    }
    let (basis, inner) = offset_basis(cx, basis, depth + 1)?;
    let offset = inner + distance;
    if !offset.is_finite() || offset.abs() >= MAX_COORD {
        return Err("offset distance out of range after unit conversion".into());
    }
    let e = entity(cx, basis)?;
    let p = e.params();
    match e.name() {
        "PLANE" => {}
        "CYLINDRICAL_SURFACE" | "SPHERICAL_SURFACE" => {
            let radius = length(cx, p.get(2), "radius")?;
            if radius <= 0.0 || !(1e-9..MAX_COORD).contains(&(radius + offset)) {
                return Err("offset surface collapses or reverses its radius".into());
            }
        }
        "CONICAL_SURFACE" => {
            let radius = length(cx, p.get(2), "radius")?;
            let angle = num(p.get(3), "semi angle")? * cx.ang;
            if radius < 0.0 || !(angle.abs() > 1e-9 && angle.abs() < FRAC_PI_2 - 1e-9) || !(0.0..MAX_COORD).contains(&(radius + offset / angle.cos()))
            {
                return Err("offset cone has an invalid reference radius or angle".into());
            }
        }
        "TOROIDAL_SURFACE" => {
            let major = length(cx, p.get(2), "major radius")?;
            let minor = length(cx, p.get(3), "minor radius")?;
            if minor <= 0.0 || major <= minor || major >= MAX_COORD || !(1e-9..major).contains(&(minor + offset)) {
                return Err("offset torus collapses or becomes self-intersecting".into());
            }
        }
        _ if offset == 0.0 => {}
        n => return Err(format!("unsupported OFFSET_SURFACE basis surface {n}")),
    }
    Ok((basis, offset))
}

/// A STEP surface entity as a kernel surface whose normal is the STEP surface normal.
/// `extent` holds points of the face boundary (bounds the analytic surfaces' parameter ranges).
/// `sense` false gives the opposite normal (a face with `same_sense = .F.`).
/// `seam` holds points of the face's seam edges (edges its loops use twice).
pub(crate) fn surface(cx: &Ctx, id: u64, extent: &[mt::Point3], seam: &[mt::Point3], sense: bool, depth: usize) -> R<mt::Surface> {
    let sign = if sense { 1.0 } else { -1.0 };
    let flip = |mut s: mt::Surface| {
        if !sense {
            s.invert();
        }
        s
    };
    if depth > MAX_GEOM_DEPTH {
        return Err("surface definitions nested too deeply".into());
    }
    let (id, offset) = offset_basis(cx, id, depth)?;
    let e = entity(cx, id)?;
    if e.is_complex() {
        return bspline_surface(cx, e).map(flip);
    }
    let p = e.params();
    let placement = || -> R<Frame> { frame(cx, p.get(1).and_then(Param::as_ref_id).ok_or("surface without placement")?) };
    match e.name() {
        "PLANE" => {
            let f = placement()?;
            let o = f.o + f.z * offset;
            Ok(flip(mt::Surface::Plane(mt::Plane::new(o, o + f.x, o + f.y))))
        }
        "CYLINDRICAL_SURFACE" => {
            let f = seam_frame(&placement()?, extent, seam);
            let r = length(cx, p.get(2), "radius")? + offset;
            if r <= 0.0 {
                return Err("cylinder radius must be positive".into());
            }
            let (lo, hi) = span_along(extent, f.o, f.z);
            let line = mt::Curve::Line(mt::Line(f.at(r, 0.0, lo), f.at(r, 0.0, hi)));
            Ok(revolved_oriented(line, f.o, f.z, 0.5, f.x * sign))
        }
        "CONICAL_SURFACE" => {
            let f = seam_frame(&placement()?, extent, seam);
            let basis_r = length(cx, p.get(2), "radius")?;
            let a = num(p.get(3), "semi angle")? * cx.ang;
            let r = basis_r + offset / a.cos();
            if r < 0.0 || !(a.abs() > 1e-9 && a.abs() < FRAC_PI_2 - 1e-9) {
                return Err("bad cone".into());
            }
            let t = a.tan();
            let (mut lo, mut hi) = span_along(extent, f.o, f.z);
            // Stop at the apex.
            let apex = -r / t;
            if t > 0.0 {
                lo = lo.max(apex);
            } else {
                hi = hi.min(apex);
            }
            if hi - lo <= 1e-9 {
                return Err("cone face has no extent".into());
            }
            let line = mt::Curve::Line(mt::Line(f.at(r + lo * t, 0.0, lo), f.at(r + hi * t, 0.0, hi)));
            Ok(revolved_oriented(line, f.o, f.z, 0.5, (f.x * a.cos() - f.z * a.sin()) * sign))
        }
        "SPHERICAL_SURFACE" => {
            let f = placement()?;
            let r = length(cx, p.get(2), "radius")? + offset;
            if r <= 0.0 {
                return Err("sphere radius must be positive".into());
            }
            // With seam edges the file's own parameterisation fits the loops; else pick poles
            // away from the boundary.
            // A small patch (a corner blend) gets poles away from it even when the file runs a
            // seam through it: a seam ending at a pole on the patch boundary leaves the mesher
            // no way to tell the patch from the rest of the sphere.
            let f = if seam.is_empty() || mean_direction(&f, extent) > 0.3 {
                let z = sphere_axis(&f, extent);
                let x = any_perpendicular(z);
                seam_frame(&Frame { o: f.o, x, y: z.cross(x), z }, extent, seam)
            } else {
                seam_frame(&f, extent, seam)
            };
            let half = Frame { o: f.o, x: f.x, y: f.z, z: -f.y };
            let arc = conic_arc(&half, r, r, -FRAC_PI_2, PI);
            let um = (arc.range_tuple().0 + arc.range_tuple().1) * 0.5;
            Ok(revolved_oriented(mt::Curve::NurbsCurve(arc), f.o, f.z, um, f.x * sign))
        }
        "TOROIDAL_SURFACE" => {
            let f = placement()?;
            let (big, small) = (length(cx, p.get(2), "major radius")?, length(cx, p.get(3), "minor radius")? + offset);
            if small <= 0.0 || big < 0.0 {
                return Err("bad torus radii".into());
            }
            // Seam edges either run around the axis (constant tube angle) or around the tube
            // (constant azimuth); each fixes one parameter seam.
            let tube_seam: Vec<mt::Point3> =
                seam.iter().copied().filter(|p| common_angle(std::iter::once(tube_angle(&f, big, *p))).is_some()).collect();
            let phis_seam = common_angle(tube_seam.iter().map(|p| tube_angle(&f, big, *p)));
            let az_seam: Vec<mt::Point3> = if phis_seam.is_some() { Vec::new() } else { seam.to_vec() };
            let f = seam_frame(&f, extent, &az_seam);
            let phis: Vec<f64> = extent.iter().map(|p| tube_angle(&f, big, *p)).collect();
            let phi0 = match (phis_seam, largest_gap(&phis)) {
                (Some(m), _) => m,
                (None, Some((m, _))) => m,
                (None, None) => 0.0,
            };
            let tube = Frame { o: f.at(big, 0.0, 0.0), x: f.x, y: f.z, z: -f.y };
            let circle = conic_arc(&tube, small, small, phi0, TAU);
            let u0 = circle.range_tuple().0;
            let outward = f.x * phi0.cos() + f.z * phi0.sin();
            Ok(revolved_oriented(mt::Curve::NurbsCurve(circle), f.o, f.z, u0, outward * sign))
        }
        "B_SPLINE_SURFACE_WITH_KNOTS" | "BEZIER_SURFACE" | "UNIFORM_SURFACE" | "QUASI_UNIFORM_SURFACE" => bspline_surface(cx, e).map(flip),
        "SURFACE_OF_REVOLUTION" => {
            let g = curve(cx, p.get(1).and_then(Param::as_ref_id).ok_or("revolution without profile")?, depth + 1)?;
            let (o, axis) = axis1(cx, p.get(2).and_then(Param::as_ref_id).ok_or("revolution without axis")?)?;
            let profile = match &g {
                CurveGeo::Line { p, d } => revolved_line(*p, *d, o, axis, extent)?,
                _ => bounded_curve(&g, extent)?,
            };
            // ISO 10303-42 parameterises a surface of revolution by the angle u and the profile
            // v, so its normal is ∂S/∂angle × ∂S/∂profile: the opposite of truck's revolution
            // (profile first). Fusion reads it this way. Reversed sense turns it back.
            Ok(revolved(profile, o, -axis * sign))
        }
        "SURFACE_OF_LINEAR_EXTRUSION" => {
            let g = curve(cx, p.get(1).and_then(Param::as_ref_id).ok_or("extrusion without profile")?, depth + 1)?;
            let (_, v) = ent_of(cx, p.get(2), "extrusion vector")?;
            let d = direction(cx, v.params().get(1).and_then(Param::as_ref_id).ok_or("VECTOR without direction")?)?;
            let nurbs: mt::NurbsCurve<mt::Vector4> = match bounded_curve(&g, extent)? {
                mt::Curve::Line(l) => mt::BSplineCurve::new(mt::KnotVec::bezier_knot(1), vec![l.0, l.1]).into(),
                mt::Curve::BSplineCurve(c) => c.into(),
                mt::Curve::NurbsCurve(c) => c,
                mt::Curve::IntersectionCurve(_) => return Err("unsupported extrusion profile".into()),
            };
            let (t0, t1) = nurbs.range_tuple();
            let profile: Vec<mt::Point3> = (0..=32).map(|i| nurbs.subs(t0 + (t1 - t0) * i as f64 / 32.0)).collect();
            let (plo, phi) = span_along(&profile, mt::Point3::new(0.0, 0.0, 0.0), d);
            let (elo, ehi) = span_along(extent, mt::Point3::new(0.0, 0.0, 0.0), d);
            let (lo, hi) = (elo - phi, ehi - plo);
            let bsp = nurbs.into_non_rationalized();
            let shift = |w: &mt::Vector4, s: f64| mt::Vector4::new(w.x + d.x * s * w.w, w.y + d.y * s * w.w, w.z + d.z * s * w.w, w.w);
            let ctrl: Vec<Vec<mt::Vector4>> = bsp.control_points().iter().map(|w| vec![shift(w, lo), shift(w, hi)]).collect();
            let s =
                mt::BSplineSurface::try_new((bsp.knot_vec().clone(), mt::KnotVec::bezier_knot(1)), ctrl).map_err(|e| format!("extrusion: {e}"))?;
            Ok(flip(mt::Surface::NurbsSurface(mt::NurbsSurface::new(s))))
        }
        "RECTANGULAR_TRIMMED_SURFACE" | "CURVE_BOUNDED_SURFACE" => {
            surface(cx, p.get(1).and_then(Param::as_ref_id).ok_or("trimmed surface without basis")?, extent, seam, sense, depth + 1)
        }
        n => Err(format!("unsupported surface {n}")),
    }
}
