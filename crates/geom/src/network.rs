//! Surfaces through curve networks (Surface › Boundary): one non-rational B-spline surface
//! through curves in one direction (a loft that skins them), or through curves in two
//! directions that cross each other (a Gordon surface; with two curves each way, the bilinearly
//! blended Coons patch). A boundary curve may carry a continuity condition with a neighbouring
//! face ([`CrossFace`]): G1, the surface leaves that face tangent to it, or G2, with its normal
//! curvature as well.
//!
//! Method:
//! - Each curve is measured by (normalised) arc length. Where curves of the two directions cross
//!   ([`closest`]: refused when they miss by more than the crossing tolerance, or cross twice),
//!   the crossing positions give the network's parameters: `ū_j` (where v-curve `j` crosses the
//!   u-curves) and `v̄_i` the other way, averages of the relative positions over the curves.
//!   With curves in one direction only, `v̄_i` is the mean distance between neighbours (chord
//!   length, NURBS Book §9.2.1 applied across).
//! - Each curve is reparameterised onto the network parameter by a monotone cubic through its
//!   crossings, so that every u-curve passes crossing `j` at `ū_j`; the small gap at a crossing
//!   (within tolerance) is closed by moving each curve onto the crossing's mid-point, the move
//!   spread linearly between crossings.
//! - All curves of one direction, and the cross-boundary derivative functions, are fitted in one
//!   common cubic spline space whose breaks include every crossing parameter (refined until each
//!   is within tolerance): so they are compatible from the start and pass exactly through the
//!   crossing points.
//! - The surface is Gordon's `S = P_u + P_v − P_uv` (W. J. Gordon, "Spline-blended surface
//!   interpolation through curve networks", 1969; the NURBS Book, ch. 10, interpolation of a
//!   bidirectional curve network): `P_u` interpolates the u-curves across v (skinning: the same
//!   interpolation applied to every column of control points), `P_v` the v-curves across u, and
//!   `P_uv` the crossing points both ways, by the same two interpolation operators. `S` then
//!   reproduces every curve. Two curves each way and linear operators give the bilinearly blended
//!   Coons patch (S. A. Coons, 1967); curves in one direction only give `S = P_u`.
//! - A continuity condition enters the across-v operator as Hermite end conditions (NURBS Book
//!   §9.2.2): the surface's first (G1) and second (G2) v-derivatives along the boundary curve.
//!   The first lies in the face's tangent plane, pointing away from the face (or into it at the
//!   far end); the second has the normal component `II(T, T)` that matches the face's normal
//!   curvature in that direction (the face's second fundamental form). A curve that crosses
//!   such a boundary is part of the surface, so it must already leave the face tangent to it
//!   (G1) and with its curvature (G2), within tolerance; otherwise the network is refused,
//!   naming the curve.
//! - The three surfaces are brought into one space ([`crate::spline_space::embed`]: Bézier degree
//!   elevation and knot insertion) and summed.
//!
//! - A side may collapse to a point ([`PointCurve`]: the apex of a three-sided patch, where the
//!   two sides beside it meet). The curves through it cross it at the point itself, so the side
//!   stays one point; the surface is singular there, as a three-sided tensor patch must be.
//!
//! Continuity conditions may sit on curves of one direction only (on two boundaries that meet
//! at a corner the cross derivatives would also have to agree on the twist there: not supported
//! yet), and only on the first and last curves of that direction. Closed curves are not
//! supported yet.
//!
//! Input is hostile: sizes are capped, every evaluation is checked, and failures are
//! [`SplineError`]s naming the curves; nothing here panics.

use crate::Vec3;
use crate::bspline::{NurbsSurface, SplineError, SplineResult, invalid};
use crate::spline_space::{Cond, Interpolant, Linear, Monotone, Space, embed};

/// Most curves in one direction of a network.
pub const MAX_NETWORK_CURVES: usize = 32;
/// Most spans of the common fitting space in one direction.
const MAX_SPANS: usize = 512;
/// Fewest spans the fitting starts with in one direction.
const MIN_SPANS: usize = 8;
/// Samples of a curve's arc-length table.
const TABLE: usize = 256;
/// How far a curve that crosses a G1 or G2 boundary may lean out of the face's tangent plane
/// there (radians, about 1°).
const TANGENT_TOL: f64 = 0.0175;
/// How much a curve that crosses a G2 boundary may differ from the face's normal curvature in
/// its direction there, relative to that curvature (or to one over the network's size, when
/// larger).
const CURVATURE_TOL: f64 = 0.02;

/// A curve of a network: its points along a parameter running over `[0, 1]` (any regular
/// parameterisation; the network measures its arc length).
pub trait NetCurve {
    fn point(&mut self, t: f64) -> Option<Vec3>;

    /// A side collapsed to a point (a three-sided patch: the apex where two sides meet). Any
    /// other curve of no length is refused.
    fn is_point(&self) -> bool {
        false
    }
}

/// A side of a network collapsed to one point: the apex of a three-sided patch, where the two
/// sides beside it meet.
pub struct PointCurve(pub Vec3);

impl NetCurve for PointCurve {
    fn point(&mut self, _: f64) -> Option<Vec3> {
        Some(self.0)
    }

    fn is_point(&self) -> bool {
        true
    }
}

/// A face along a boundary curve, seen from a point of the curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceFrame {
    /// The face's unit normal there.
    pub normal: Vec3,
    /// A unit vector in the face's tangent plane, square to the curve, pointing away from the
    /// face (out across its edge).
    pub outward: Vec3,
    /// The face's second fundamental form against `normal` (`II(w, w)`: the normal component of
    /// the second derivative of a curve on the face with first derivative `w`), in the
    /// orthonormal basis of the tangent asked about `t` (made unit in the tangent plane, running
    /// the way it was given) and `outward` `b`: `[II(t, t), II(t, b), II(b, b)]`.
    pub form: [f64; 3],
}

/// The face a boundary curve runs along: its frame at a point of the curve (`tangent`: the
/// curve's direction there, either way).
pub trait CrossFace {
    fn frame(&mut self, at: Vec3, tangent: Vec3) -> Option<FaceFrame>;
}

/// How smoothly a surface meets a neighbouring face across a boundary curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Continuity {
    /// Position only.
    #[default]
    G0,
    /// Tangent: the same tangent plane along the curve.
    G1,
    /// Curvature: the same tangent plane and normal curvature along the curve.
    G2,
}

impl Continuity {
    /// The number of cross-boundary derivatives it fixes.
    pub fn order(self) -> usize {
        match self {
            Continuity::G0 => 0,
            Continuity::G1 => 1,
            Continuity::G2 => 2,
        }
    }
}

/// One curve of a network: a name for messages, the curve, and its continuity with a face.
pub struct NetworkCurve<'a> {
    pub label: String,
    pub curve: Box<dyn NetCurve + 'a>,
    pub condition: Option<(Continuity, Box<dyn CrossFace + 'a>)>,
}

/// The tolerances of a network (mm): how close curves must come to cross, and how far the fitted
/// curves may stray from the given ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerances {
    pub cross: f64,
    pub fit: f64,
}

/// A curve measured by arc length: `point(s)` with `s` in `[0, 1]` (reversed when `flipped`).
struct Track<'a> {
    label: String,
    curve: Box<dyn NetCurve + 'a>,
    /// Normalised arc length and curve parameter at each table sample.
    s: Vec<f64>,
    t: Vec<f64>,
    pts: Vec<Vec3>,
    length: f64,
    flipped: bool,
    /// A side collapsed to a point ([`PointCurve`]).
    point: bool,
    /// The finite-difference step of [`Track::ders`]: a quarter of the smallest table cell.
    step: f64,
    condition: Option<(Continuity, Box<dyn CrossFace + 'a>)>,
}

impl<'a> Track<'a> {
    fn new(c: NetworkCurve<'a>) -> SplineResult<Track<'a>> {
        let NetworkCurve { label, mut curve, condition } = c;
        let mut pts = Vec::with_capacity(TABLE + 1);
        let mut t = Vec::with_capacity(TABLE + 1);
        for k in 0..=TABLE {
            let tk = k as f64 / TABLE as f64;
            let p = curve.point(tk).filter(|p| p.is_finite()).ok_or_else(|| SplineError::Invalid(format!("{label} can't be evaluated")))?;
            pts.push(p);
            t.push(tk);
        }
        let mut s = vec![0.0];
        let mut acc = 0.0;
        for w in pts.windows(2) {
            acc += w[0].dist(w[1]);
            s.push(acc);
        }
        let point = curve.is_point();
        if point {
            // Its parameter runs evenly: every value is the same point.
            if acc > 1e-9 || !acc.is_finite() {
                return invalid(format!("{label} is meant to be a point but moves"));
            }
            if condition.as_ref().is_some_and(|(g, _)| *g != Continuity::G0) {
                return invalid(format!("{label} is a point: a continuity condition needs a curve"));
            }
            let step = step(&t);
            return Ok(Track { label, curve, s: t.clone(), t, pts, length: 0.0, flipped: false, point, step, condition: None });
        }
        if !(acc > 1e-9 && acc.is_finite()) {
            return invalid(format!("{label} has no length"));
        }
        for x in &mut s {
            *x /= acc;
        }
        let condition = condition.filter(|(g, _)| *g != Continuity::G0);
        let step = step(&s);
        Ok(Track { label, curve, s, t, pts, length: acc, flipped: false, point, step, condition })
    }

    fn continuity(&self) -> Continuity {
        self.condition.as_ref().map_or(Continuity::G0, |(g, _)| *g)
    }

    /// The point at normalised arc length `s` (clamped to `[0, 1]`).
    fn point(&mut self, s: f64) -> SplineResult<Vec3> {
        let s = if self.flipped { 1.0 - s } else { s }.clamp(0.0, 1.0);
        let k = self.s.partition_point(|x| *x <= s).saturating_sub(1).min(TABLE - 1);
        let (s0, s1, t0, t1) = (self.s[k], self.s[k + 1], self.t[k], self.t[k + 1]);
        let t = if s1 > s0 { t0 + (t1 - t0) * (s - s0) / (s1 - s0) } else { t0 };
        self.curve.point(t).filter(|p| p.is_finite()).ok_or_else(|| SplineError::Invalid(format!("{} can't be evaluated", self.label)))
    }

    /// The table's points in the curve's current direction.
    fn polyline(&self) -> Vec<Vec3> {
        if self.flipped { self.pts.iter().rev().copied().collect() } else { self.pts.clone() }
    }

    /// The first two derivatives by `s` at `s` (one-sided at the ends), by finite differences
    /// within one table cell.
    fn ders(&mut self, s: f64) -> SplineResult<(Vec3, Vec3)> {
        let h = self.step;
        if s - 2.0 * h < 0.0 {
            let f: Vec<Vec3> = (0..4).map(|k| self.point(s + k as f64 * h)).collect::<SplineResult<_>>()?;
            Ok(((f[0] * -3.0 + f[1] * 4.0 - f[2]) / (2.0 * h), (f[0] * 2.0 - f[1] * 5.0 + f[2] * 4.0 - f[3]) / (h * h)))
        } else if s + 2.0 * h > 1.0 {
            let f: Vec<Vec3> = (0..4).map(|k| self.point(s - k as f64 * h)).collect::<SplineResult<_>>()?;
            Ok(((f[0] * 3.0 - f[1] * 4.0 + f[2]) / (2.0 * h), (f[0] * 2.0 - f[1] * 5.0 + f[2] * 4.0 - f[3]) / (h * h)))
        } else {
            let (a, b, c) = (self.point(s - h)?, self.point(s)?, self.point(s + h)?);
            Ok(((c - a) / (2.0 * h), (a - b * 2.0 + c) / (h * h)))
        }
    }
}

/// The finite-difference step along a table of parameters: a quarter of its smallest cell, at
/// most 1e-3.
fn step(s: &[f64]) -> f64 {
    let cell = s.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0).fold(f64::INFINITY, f64::min);
    (cell / 4.0).min(1e-3)
}

/// Where two curves cross: the arc-length positions on each and the mid-point between them.
#[derive(Clone, Copy, Debug)]
struct Crossing {
    s: f64,
    r: f64,
    at: Vec3,
}

/// The one place where `a` and `b` come within `tol` of each other. Refused when they don't, or
/// do at two places.
fn closest(a: &mut Track, b: &mut Track, tol: f64) -> SplineResult<Crossing> {
    let (pa, pb) = (a.polyline(), b.polyline());
    let longest = |p: &[Vec3]| p.windows(2).map(|w| w[0].dist(w[1])).fold(0.0, f64::max);
    let near = tol + longest(&pa) + longest(&pb);
    // Distance from each sample of `a` to the polyline of `b`, and the nearest segment.
    let mut d = Vec::with_capacity(pa.len());
    for p in &pa {
        let mut best = (f64::INFINITY, 0usize);
        for (k, w) in pb.windows(2).enumerate() {
            let x = p.dist_to_segment(w[0], w[1]);
            if x < best.0 {
                best = (x, k);
            }
        }
        d.push(best);
    }
    // Runs of samples near `b`: one candidate crossing each, refined from its nearest sample.
    let n = d.len();
    let mut starts = Vec::new();
    let mut k = 0;
    while k < n {
        if d[k].0 > near {
            k += 1;
            continue;
        }
        let mut m = k;
        while k < n && d[k].0 <= near {
            if d[k].0 < d[m].0 {
                m = k;
            }
            k += 1;
        }
        starts.push(m);
    }
    if starts.is_empty() {
        starts.push((0..n).min_by(|x, y| d[*x].0.total_cmp(&d[*y].0)).unwrap_or(0));
    }
    let mut found: Vec<Crossing> = Vec::new();
    let mut miss: Option<(f64, Vec3)> = None;
    for m in starts {
        let c = refine(a, b, m as f64 / TABLE as f64, (d[m].1 as f64 + 0.5) / TABLE as f64)?;
        let gap = a.point(c.s)?.dist(b.point(c.r)?);
        if gap <= tol {
            if !found.iter().any(|f| (f.s - c.s).abs() < 1e-6 && (f.r - c.r).abs() < 1e-6) {
                found.push(c);
            }
        } else if miss.is_none_or(|(g, _)| gap < g) {
            miss = Some((gap, c.at));
        }
    }
    match found.as_slice() {
        [c] => Ok(*c),
        [] => {
            let (gap, at) = miss.unwrap_or((f64::INFINITY, Vec3::ZERO));
            invalid(format!(
                "{} and {} don't meet: they pass {gap:.4} mm apart near ({:.3}, {:.3}, {:.3}) (the crossing tolerance is {tol} mm)",
                a.label, b.label, at.x, at.y, at.z
            ))
        }
        _ => invalid(format!("{} and {} cross more than once", a.label, b.label)),
    }
}

/// The closest points of `a` and `b` near `(s, r)`: Gauss–Newton on `|a(s) − b(r)|²`, the
/// parameters kept in `[0, 1]`.
fn refine(a: &mut Track, b: &mut Track, mut s: f64, mut r: f64) -> SplineResult<Crossing> {
    let h = 1e-7;
    let slope = |t: &mut Track, x: f64| -> SplineResult<Vec3> {
        let (x0, x1) = ((x - h).max(0.0), (x + h).min(1.0));
        Ok((t.point(x1)? - t.point(x0)?) / (x1 - x0))
    };
    for _ in 0..40 {
        let (pa, pb) = (a.point(s)?, b.point(r)?);
        let (da, db) = (slope(a, s)?, slope(b, r)?);
        let e = pa - pb;
        // Normal equations of the 3×2 system [da, −db]·(Δs, Δr) = −e.
        let (m11, m12, m22) = (da.dot(da), -da.dot(db), db.dot(db));
        let (g1, g2) = (-da.dot(e), db.dot(e));
        let det = m11 * m22 - m12 * m12;
        let (ds, dr) = if det.abs() > 1e-14 * (m11 * m22).max(1e-300) {
            ((g1 * m22 - m12 * g2) / det, (m11 * g2 - m12 * g1) / det)
        } else {
            // Parallel there: a plain step along the gradient.
            (g1 / m11.max(1e-300), g2 / m22.max(1e-300))
        };
        let (ns, nr) = ((s + ds).clamp(0.0, 1.0), (r + dr).clamp(0.0, 1.0));
        let done = (ns - s).abs() < 1e-13 && (nr - r).abs() < 1e-13;
        (s, r) = (ns, nr);
        if done {
            break;
        }
    }
    let (pa, pb) = (a.point(s)?, b.point(r)?);
    Ok(Crossing { s, r, at: (pa + pb) * 0.5 })
}

/// What an interpolation condition across the network is matched with: curve `i`'s value, the
/// `order`-th cross derivative at the first (`end` 0) or last (`end` 1) curve, or zero (the
/// slope that holds a single curve's value constant).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Datum {
    Curve(usize),
    Der { end: usize, order: usize },
    Zero,
}

/// An interpolation operator across a direction: values at `params`, cross derivatives of the
/// orders in `ders` at the first and last, on `[0, 1]`.
struct Across {
    interp: Interpolant,
    data: Vec<Datum>,
}

impl Across {
    fn new(params: &[f64], ders: [usize; 2]) -> SplineResult<Across> {
        let mut conds = Vec::new();
        let mut data = Vec::new();
        for (i, &t) in params.iter().enumerate() {
            conds.push(Cond { t, der: 0 });
            data.push(Datum::Curve(i));
        }
        for (end, &count) in ders.iter().enumerate() {
            let t = if end == 0 { params.first() } else { params.last() };
            let Some(&t) = t else { return invalid("no curves") };
            for order in 1..=count {
                conds.push(Cond { t, der: order });
                data.push(Datum::Der { end, order });
            }
        }
        let interp = Interpolant::lowest(conds, 0.0, 1.0)?;
        // A lone value gets a zero slope to hold it constant.
        data.resize(interp.conds().len(), Datum::Zero);
        Ok(Across { interp, data })
    }

    /// Interpolation of values given by `datum` (one per condition).
    fn fit<T: Linear>(&self, mut datum: impl FnMut(Datum) -> SplineResult<T>) -> SplineResult<Vec<T>> {
        let values: Vec<T> = self.data.iter().map(|d| if *d == Datum::Zero { Ok(T::zero()) } else { datum(*d) }).collect::<SplineResult<_>>()?;
        self.interp.fit(&values)
    }

    fn space(&self) -> &Space {
        self.interp.space()
    }

    /// The interpolant's interior knots (the fitting space must contain them).
    fn knots(&self) -> Vec<f64> {
        let [a, b] = self.space().domain();
        self.space().knots().iter().copied().filter(|k| *k > a && *k < b).collect()
    }
}

/// A function along one direction that is fitted: its values (and, at the ends, derivatives) and
/// how close the fit must come.
struct Target<'f> {
    label: String,
    value: Box<dyn FnMut(f64) -> SplineResult<Vec3> + 'f>,
    /// Derivatives (orders 1, 2, …) at the first and last parameter.
    ders: [Vec<Vec3>; 2],
    tol: f64,
}

/// The common cubic space of a direction and each target's control points in it: breaks at
/// `base` (increasing, from 0 to 1) each split into equal parts, refined until every target is
/// within its tolerance at the quarter points of every span. A G2 end gets one more knot in its
/// end span, room for its second derivative.
fn fit_direction(base: &[f64], ders: [usize; 2], targets: &mut [Target]) -> SplineResult<(Space, Vec<Vec<Vec3>>)> {
    let intervals = base.len().saturating_sub(1).max(1);
    let mut m = MIN_SPANS.div_ceil(intervals);
    loop {
        let mut breaks = Vec::with_capacity(intervals * m + 1);
        for w in base.windows(2) {
            for k in 0..m {
                breaks.push(if k == 0 { w[0] } else { w[0] + (w[1] - w[0]) * k as f64 / m as f64 });
            }
        }
        if let Some(&last) = base.last() {
            breaks.push(last);
        }
        let spans = breaks.len() - 1;
        let mut space = Space::with_breaks(3, &breaks)?;
        let mid = |k: usize| (breaks[k] + breaks[k + 1]) / 2.0;
        let mut extra = Vec::new();
        if ders[0] >= 2 {
            extra.push(mid(0));
        }
        if ders[1] >= 2 {
            extra.push(mid(spans - 1));
        }
        space = space.refined(&extra)?;
        // Values at the breaks, the end derivatives, and a value mid-span at an end without one.
        let mut conds: Vec<Cond> = breaks.iter().map(|&t| Cond { t, der: 0 }).collect();
        let mut kinds: Vec<(usize, usize)> = breaks.iter().map(|_| (0, 0)).collect();
        for (end, &count) in ders.iter().enumerate() {
            let t = if end == 0 { 0.0 } else { 1.0 };
            for order in 1..=count {
                conds.push(Cond { t, der: order });
                kinds.push((end, order));
            }
            if count == 0 {
                conds.push(Cond { t: mid(if end == 0 { 0 } else { spans - 1 }), der: 0 });
                kinds.push((0, 0));
            }
        }
        let interp = Interpolant::new(space.clone(), conds.clone())?;
        let mut ctrls = Vec::with_capacity(targets.len());
        let mut worst: Option<(f64, String)> = None;
        for tg in targets.iter_mut() {
            let data: Vec<Vec3> = conds
                .iter()
                .zip(&kinds)
                .map(|(c, (end, order))| match order {
                    0 => (tg.value)(c.t),
                    o => tg.ders.get(*end).and_then(|d| d.get(o - 1)).copied().ok_or_else(|| SplineError::Invalid("a derivative is missing".into())),
                })
                .collect::<SplineResult<_>>()?;
            let ctrl = interp.fit(&data)?;
            for k in 0..spans {
                for q in [0.25, 0.75] {
                    let t = breaks[k] + (breaks[k + 1] - breaks[k]) * q;
                    let off = space.eval(&ctrl, t, 0)?.dist((tg.value)(t)?);
                    if off > tg.tol && worst.as_ref().is_none_or(|(w, _)| off / tg.tol > *w) {
                        worst = Some((off / tg.tol, format!("{} (off by {off:.2e} at the best fit)", tg.label)));
                    }
                }
            }
            ctrls.push(ctrl);
        }
        match worst {
            None => return Ok((space, ctrls)),
            Some((_, what)) if spans * 2 > MAX_SPANS => {
                return Err(SplineError::Unsupported(format!(
                    "{what} can't be fitted closely enough: a curve with a sharp corner, or one that turns very sharply, can't be part of a boundary surface"
                )));
            }
            Some(_) => m *= 2,
        }
    }
}

/// Linear interpolation at `x` of `values` given at the increasing parameters `xs` (held
/// constant beyond the first and last; a lone value everywhere).
fn lerp_between<T: Linear>(xs: &[f64], values: &[T], x: f64) -> T {
    match (xs.len(), values.len()) {
        (0, _) | (_, 0) => T::zero(),
        (1, _) | (_, 1) => values[0],
        (n, _) => {
            let k = xs.partition_point(|t| *t <= x).saturating_sub(1).min(n - 2);
            let (x0, x1) = (xs[k], xs[k + 1]);
            let w = if x1 > x0 { ((x - x0) / (x1 - x0)).clamp(0.0, 1.0) } else { 0.0 };
            values[k].times(1.0 - w).plus(values.get(k + 1).copied().unwrap_or(values[k]).times(w))
        }
    }
}

/// How a curve runs over the network parameter: its arc-length position at network parameter
/// `x`, and the shift that puts it through the crossing points exactly.
struct Placement {
    map: Monotone,
    /// The network parameters of the crossings and each crossing's shift.
    at: Vec<f64>,
    shift: Vec<Vec3>,
}

impl Placement {
    fn point(&self, track: &mut Track, x: f64) -> SplineResult<Vec3> {
        Ok(track.point(self.map.eval(x)[0])? + lerp_between(&self.at, &self.shift, x))
    }

    /// The first two derivatives by the network parameter at `x`.
    fn ders(&self, track: &mut Track, x: f64) -> SplineResult<(Vec3, Vec3)> {
        let [s, ds, dds] = self.map.eval(x);
        let (p1, p2) = track.ders(s)?;
        // The shift is linear between crossings: its slope on the span at x.
        let slope = match self.at.len() {
            0 | 1 => Vec3::ZERO,
            n => {
                let k = self.at.partition_point(|t| *t <= x).saturating_sub(1).min(n - 2);
                let (x0, x1) = (self.at[k], self.at[k + 1]);
                if x1 > x0 { (self.shift[k + 1] - self.shift[k]) / (x1 - x0) } else { Vec3::ZERO }
            }
        };
        Ok((p1 * ds + slope, p2 * (ds * ds) + p1 * dds))
    }
}

/// A placement through `nodes` (network parameter, arc-length position), with the shifts that
/// take the curve onto `targets` at the crossings `(x, s)` it was measured at.
fn placement(track: &mut Track, nodes: &[(f64, f64)], crossings: &[(f64, f64, Vec3)]) -> SplineResult<Placement> {
    let map = Monotone::new(nodes.iter().map(|n| n.0).collect(), nodes.iter().map(|n| n.1).collect())
        .map_err(|_| SplineError::Invalid(format!("{} is crossed out of order", track.label)))?;
    let mut at = Vec::with_capacity(crossings.len());
    let mut shift = Vec::with_capacity(crossings.len());
    for &(x, s, q) in crossings {
        at.push(x);
        shift.push(q - track.point(s)?);
    }
    Ok(Placement { map, at, shift })
}

/// The frame of the face along conditioned curve `e`, with the tangent and outward directions
/// made orthonormal in its tangent plane.
struct Frame {
    n: Vec3,
    t: Vec3,
    b: Vec3,
    form: [f64; 3],
}

impl Frame {
    /// `II(w, w)` for `w = a·b + c·t`.
    fn second(&self, a: f64, c: f64) -> f64 {
        let [tt, tb, bb] = self.form;
        c * c * tt + 2.0 * a * c * tb + a * a * bb
    }
}

/// A surface through a curve network (see the module documentation): curves `u` in one
/// direction, `v` (possibly none) in the other.
pub fn network_surface(u: Vec<NetworkCurve>, v: Vec<NetworkCurve>, tol: Tolerances) -> SplineResult<NurbsSurface> {
    if !(tol.cross > 0.0 && tol.cross.is_finite() && tol.fit > 0.0 && tol.fit.is_finite()) {
        return invalid("the tolerances must be positive");
    }
    if u.len() > MAX_NETWORK_CURVES || v.len() > MAX_NETWORK_CURVES {
        return Err(SplineError::TooLarge(format!("more than {MAX_NETWORK_CURVES} curves in one direction")));
    }
    let mut us: Vec<Track> = u.into_iter().map(Track::new).collect::<SplineResult<_>>()?;
    let mut vs: Vec<Track> = v.into_iter().map(Track::new).collect::<SplineResult<_>>()?;
    for t in us.iter().chain(&vs).filter(|t| !t.point) {
        if t.pts.first().zip(t.pts.last()).is_some_and(|(a, b)| a.dist(*b) <= tol.cross) {
            return Err(SplineError::Unsupported(format!("{} is closed: closed curves in a boundary surface", t.label)));
        }
    }
    let conditioned = |ts: &[Track]| ts.iter().any(|t| t.condition.is_some());
    if conditioned(&us) && conditioned(&vs) {
        return Err(SplineError::Unsupported("continuity conditions on curves of both directions (on two boundaries that meet at a corner)".into()));
    }
    // Conditions go on the u-curves, and there are two u-curves where possible.
    if conditioned(&vs) || (us.len() < 2 && vs.len() >= 2 && !conditioned(&us)) {
        std::mem::swap(&mut us, &mut vs);
    }
    let size = us
        .iter()
        .chain(&vs)
        .flat_map(|t| t.pts.iter())
        .fold(None, |acc: Option<(Vec3, Vec3)>, p| Some(acc.map_or((*p, *p), |(lo, hi)| (lo.min(*p), hi.max(*p)))));
    let size = size.map_or(0.0, |(lo, hi)| lo.dist(hi));
    match (us.len(), vs.len()) {
        (0, _) | (1, 0) => return invalid("a boundary surface needs two curves in one direction, or crossing curves"),
        (1, _) if !conditioned(&us) => return invalid("a boundary surface needs two curves in one direction"),
        _ => {}
    }
    let (nu, nv) = (us.len(), vs.len());
    // A side collapsed to a point: one at most, with another curve beside it in its own direction,
    // and the curves the other way (if any) running from it to the far side.
    let points = |ts: &[Track]| ts.iter().filter(|t| t.point).count();
    match (points(&us), points(&vs)) {
        (0, 0) => {}
        (1, 0) if nu >= 2 && nv != 1 => {}
        (0, 1) if nv >= 2 && nu >= 2 => {}
        _ => return invalid("a point can stand for one side of the patch only, with curves running from it to the side across"),
    }

    // Crossings: x[i][j] where u-curve i meets v-curve j (s along u-curve i, r along v-curve j).
    // Where one of them is a point, the crossing is that point itself, so that every curve
    // through it is moved onto it and the side stays collapsed.
    let mut x: Vec<Vec<Crossing>> = Vec::with_capacity(nu);
    for a in us.iter_mut() {
        let mut row = Vec::with_capacity(nv);
        for b in vs.iter_mut() {
            let mut c = closest(a, b, tol.cross)?;
            if a.point {
                c.at = a.point(0.0)?;
            } else if b.point {
                c.at = b.point(0.0)?;
            }
            row.push(c);
        }
        x.push(row);
    }
    // The first curve of a direction that is not a point (orders are read along it).
    let first_curve = |ts: &[Track]| ts.iter().position(|t| !t.point).unwrap_or(0);

    // Order and orient: the u-curves in their order along the first v-curve, each v-curve running
    // from the first u-curve to the last; likewise the v-curves along the first u-curve.
    if nv >= 1 && nu >= 2 {
        let mut order: Vec<usize> = (0..nu).collect();
        let j0 = first_curve(&vs);
        order.sort_by(|a, b| x[*a][j0].r.total_cmp(&x[*b][j0].r));
        us = reorder(us, &order);
        x = reorder(x, &order);
        for j in 0..nv {
            // A point runs no way: nothing to orient.
            if vs[j].point {
                continue;
            }
            let rs: Vec<f64> = x.iter().map(|row| row[j].r).collect();
            match direction(&rs) {
                Some(true) => {}
                Some(false) => {
                    vs[j].flipped = !vs[j].flipped;
                    for row in x.iter_mut() {
                        row[j].r = 1.0 - row[j].r;
                    }
                }
                None => return invalid(format!("{} crosses the other curves out of order (or two of them at one point)", vs[j].label)),
            }
        }
    } else if nv >= 1 {
        // One conditioned u-curve: each v-curve starts on it.
        for j in 0..nv {
            if x[0][j].r > 0.5 {
                vs[j].flipped = !vs[j].flipped;
                x[0][j].r = 1.0 - x[0][j].r;
            }
        }
    }
    if nv >= 2 {
        let mut order: Vec<usize> = (0..nv).collect();
        let i0 = first_curve(&us);
        order.sort_by(|a, b| x[i0][*a].s.total_cmp(&x[i0][*b].s));
        vs = reorder(vs, &order);
        for row in x.iter_mut() {
            *row = reorder(std::mem::take(row), &order);
        }
        for (i, row) in x.iter_mut().enumerate() {
            if us[i].point {
                continue;
            }
            let ss: Vec<f64> = row.iter().map(|c| c.s).collect();
            match direction(&ss) {
                Some(true) => {}
                Some(false) => {
                    us[i].flipped = !us[i].flipped;
                    for c in row.iter_mut() {
                        c.s = 1.0 - c.s;
                    }
                }
                None => return invalid(format!("{} crosses the other curves out of order (or two of them at one point)", us[i].label)),
            }
        }
    } else {
        // No order along the u-curves: each runs the way the one before it does.
        for i in 1..nu {
            if us[i].point || us[i - 1].point {
                continue;
            }
            let (ps, pe) = (us[i - 1].point(0.0)?, us[i - 1].point(1.0)?);
            let (s, e) = (us[i].point(0.0)?, us[i].point(1.0)?);
            if s.dist(ps) + e.dist(pe) > s.dist(pe) + e.dist(ps) {
                us[i].flipped = !us[i].flipped;
                for c in x[i].iter_mut() {
                    c.s = 1.0 - c.s;
                }
            }
        }
    }

    // A point is a side: the first or last curve of its direction.
    for ts in [&us, &vs] {
        for (i, t) in ts.iter().enumerate() {
            if t.point && i != 0 && i + 1 != ts.len() {
                return invalid(format!("{} lies inside the network: a point can only stand for a side", t.label));
            }
        }
    }
    // Conditions only on the first and last u-curve.
    for (i, t) in us.iter().enumerate() {
        if t.condition.is_some() && i != 0 && i + 1 != nu {
            return invalid(format!("{} lies inside the network: continuity applies only to a boundary curve", t.label));
        }
    }
    let end_order = [us.first().map_or(0, |t| t.continuity().order()), if nu >= 2 { us.last().map_or(0, |t| t.continuity().order()) } else { 0 }];

    // Network parameters (a point has no positions along it to average).
    let rows: Vec<Vec<f64>> = x.iter().zip(&us).filter(|(_, t)| !t.point).map(|(row, _)| row.iter().map(|c| c.s).collect()).collect();
    let ubar = network_params(&rows, nv)?;
    let vbar: Vec<f64> = if nv == 0 {
        chord_params(&mut us)?
    } else if nu >= 2 {
        let cols: Vec<Vec<f64>> = (0..nv).filter(|j| !vs[*j].point).map(|j| x.iter().map(|row| row[j].r).collect()).collect();
        network_params(&cols, nu)?
    } else {
        // One conditioned curve, crossed at the start of every v-curve.
        for j in 0..nv {
            if x[0][j].r * vs[j].length > tol.cross {
                return invalid(format!("{} must start on {}: it crosses it partway along", vs[j].label, us[0].label));
            }
        }
        vec![0.0]
    };

    // A point is placed evenly: its crossings sit at the network parameters themselves.
    for (i, row) in x.iter_mut().enumerate() {
        for (j, c) in row.iter_mut().enumerate() {
            if us.get(i).is_some_and(|t| t.point) {
                c.s = ubar.get(j).copied().unwrap_or(c.s);
            }
            if vs.get(j).is_some_and(|t| t.point) {
                c.r = vbar.get(i).copied().unwrap_or(c.r);
            }
        }
    }

    // Placements: each curve through its crossings at the network parameters.
    let mut up = Vec::with_capacity(nu);
    for (i, t) in us.iter_mut().enumerate() {
        let cross: Vec<(f64, f64, Vec3)> = (0..nv).map(|j| (ubar[j], x[i][j].s, x[i][j].at)).collect();
        let nodes = place_nodes(&cross, nv);
        up.push(placement(t, &nodes, &cross)?);
    }
    let mut vp = Vec::with_capacity(nv);
    for (j, t) in vs.iter_mut().enumerate() {
        let cross: Vec<(f64, f64, Vec3)> = (0..nu).map(|i| (vbar[i], x[i][j].r, x[i][j].at)).collect();
        let nodes = place_nodes(&cross, nu);
        vp.push(placement(t, &nodes, &cross)?);
    }

    // The operators across: Π_v over the u-curves (with the end conditions), Π_u over the v-curves.
    let across_v = Across::new(&vbar, end_order)?;
    let across_u = if nv >= 1 { Some(Across::new(&ubar, [0, 0])?) } else { None };

    // Cross-boundary conditions: for each conditioned end, the frame along it, the natural
    // derivatives, and the corrections that make them agree with the v-curves at the crossings.
    let mut ends: Vec<Option<EndCondition>> = vec![None, None];
    for end in 0..2 {
        if end_order[end] == 0 {
            continue;
        }
        let i = if end == 0 { 0 } else { nu - 1 };
        let mut cond = EndCondition { end, curve: i, order: end_order[end], neighbour: None, at: Vec::new(), corr: [None, None, None, None] };
        if nu >= 2 {
            let k = if end == 0 { 1 } else { nu - 2 };
            cond.neighbour = Some((k, (vbar[k] - vbar[i]).abs()));
        }
        // The v-curves where they meet this boundary: their derivatives, checked against the face.
        let mut values: Vec<[f64; 4]> = Vec::with_capacity(nv);
        for j in 0..nv {
            let xj = ubar[j];
            let frame = end_frame(&mut us, &up, i, xj)?;
            let (d1, d2) = vp[j].ders(&mut vs[j], vbar[i])?;
            let (vl, ul) = (vs[j].label.clone(), us[i].label.clone());
            let len = d1.len();
            if !(len > 0.0) {
                return invalid(format!("{vl} has no direction where it meets {ul}"));
            }
            if d1.dot(frame.n).abs() > TANGENT_TOL.sin() * len {
                let deg = (d1.dot(frame.n).abs() / len).clamp(0.0, 1.0).asin().to_degrees();
                return invalid(format!(
                    "{vl} meets the {} boundary {ul} at {deg:.2}° to its face: it must leave the face tangent to it there",
                    if end_order[end] == 1 { "G1" } else { "G2" }
                ));
            }
            let a = d1.dot(frame.b);
            let outward = if end == 0 { a } else { -a };
            if outward <= TANGENT_TOL.sin() * len {
                return invalid(format!("{vl} turns back over the face of {ul} where it meets it"));
            }
            let c = d1.dot(frame.t);
            let (mut kb, mut kt) = (0.0, 0.0);
            if end_order[end] >= 2 {
                let tproj = frame.b * a + frame.t * c;
                let want = frame.second(a, c) / tproj.len2();
                let got = d2.dot(frame.n) / d1.len2();
                let scale = want.abs().max(1.0 / size.max(1e-9));
                if (got - want).abs() > CURVATURE_TOL * scale {
                    return invalid(format!(
                        "{vl} meets the G2 boundary {ul} with a normal curvature of {got:.5} /mm where the face has {want:.5} /mm: it must follow the face's curvature there"
                    ));
                }
                kb = d2.dot(frame.b);
                kt = d2.dot(frame.t);
            }
            values.push([a, c, kb, kt]);
        }
        cond.at = values;
        ends[end] = Some(cond);
    }
    // Corrections: Π_u through (crossing value − natural value), so each function hits the
    // v-curves' own derivatives at the crossings.
    if let Some(au) = &across_u {
        for slot in ends.iter_mut() {
            let Some(mut cond) = slot.take() else { continue };
            let mut diffs: [Vec<f64>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
            for (&xj, at) in ubar.iter().zip(&cond.at) {
                let frame = end_frame(&mut us, &up, cond.curve, xj)?;
                let natural = cond.natural(&mut us, &up, &frame, xj)?;
                for (k, d) in diffs.iter_mut().enumerate() {
                    d.push(at[k] - natural[k]);
                }
            }
            for (k, d) in diffs.iter().enumerate() {
                let ctrl = au.fit(|datum| match datum {
                    Datum::Curve(j) => d.get(j).copied().ok_or_else(|| SplineError::Invalid("crossing".into())),
                    _ => Ok(0.0),
                })?;
                cond.corr[k] = Some(ctrl);
            }
            *slot = Some(cond);
        }
    }

    // Fit the u-direction: the u-curves, then each condition's first and second derivatives.
    let mut base_u: Vec<f64> = vec![0.0, 1.0];
    base_u.extend(ubar.iter().copied());
    if let Some(au) = &across_u {
        base_u.extend(au.knots());
    }
    let base_u = sorted_breaks(base_u);
    let (u_space, u_ctrl) = {
        let us_cell = std::cell::RefCell::new(&mut us);
        let up_ref = &up;
        let mut targets: Vec<Target> = Vec::new();
        for i in 0..nu {
            let label = us_cell.borrow()[i].label.clone();
            let cell = &us_cell;
            targets.push(Target {
                label,
                value: Box::new(move |t| {
                    let mut tracks = cell.borrow_mut();
                    up_ref[i].point(&mut tracks[i], t)
                }),
                ders: [Vec::new(), Vec::new()],
                tol: tol.fit,
            });
        }
        for cond in ends.iter().flatten() {
            let label = format!("the cross-boundary tangent along {}", us_cell.borrow()[cond.curve].label);
            let scale = cond.scale(&mut us_cell.borrow_mut(), up_ref, across_u.as_ref())?;
            for order in 1..=cond.order {
                let cell = &us_cell;
                let au = across_u.as_ref();
                targets.push(Target {
                    label: label.clone(),
                    value: Box::new(move |t| {
                        let mut tracks = cell.borrow_mut();
                        let (d1, d2) = cond.derivatives(&mut tracks, up_ref, au, t)?;
                        Ok(if order == 1 { d1 } else { d2 })
                    }),
                    ders: [Vec::new(), Vec::new()],
                    tol: scale[order - 1] * 1e-4,
                });
            }
        }
        fit_direction(&base_u, [0, 0], &mut targets)?
    };
    // Which fitted function is which: the u-curves, then per conditioned end its derivatives.
    let mut der_index: [[Option<usize>; 2]; 2] = [[None; 2]; 2];
    let mut next = nu;
    for cond in ends.iter().flatten() {
        for order in 1..=cond.order {
            der_index[cond.end][order - 1] = Some(next);
            next += 1;
        }
    }
    let u_fn = |d: Datum, a: usize| -> SplineResult<Vec3> {
        let k = match d {
            Datum::Curve(i) => Some(i),
            Datum::Der { end, order } => der_index.get(end).and_then(|e| e.get(order.wrapping_sub(1))).copied().flatten(),
            Datum::Zero => None,
        };
        Ok(k.and_then(|k| u_ctrl.get(k)).and_then(|c| c.get(a)).copied().unwrap_or(Vec3::ZERO))
    };
    // P_u: across v, column by column of the u-direction control points.
    let nu_ctrl = u_space.count();
    let mut pu: Vec<Vec<Vec3>> = Vec::with_capacity(nu_ctrl);
    for a in 0..nu_ctrl {
        pu.push(across_v.fit(|d| u_fn(d, a))?);
    }
    let v_op_space = across_v.space().clone();
    let Some(across_u) = across_u else {
        // Curves in one direction only: the skinned surface.
        return surface(&u_space, &v_op_space, &pu);
    };

    // The derivative data at each crossing of a conditioned boundary, exactly as the fitted
    // functions take them there.
    let mut cross_ders: Vec<[[Vec3; 2]; 2]> = vec![[[Vec3::ZERO; 2]; 2]; nv];
    for cond in ends.iter().flatten() {
        for (j, cd) in cross_ders.iter_mut().enumerate() {
            let (d1, d2) = cond.derivatives(&mut us, &up, Some(&across_u), ubar[j])?;
            cd[cond.end] = [d1, d2];
        }
    }

    // Fit the v-direction: the v-curves, with the conditioned ends' derivatives.
    let mut base_v: Vec<f64> = vec![0.0, 1.0];
    base_v.extend(vbar.iter().copied());
    base_v.extend(across_v.knots());
    let base_v = sorted_breaks(base_v);
    let (v_space, v_ctrl) = {
        let vs_cell = std::cell::RefCell::new(&mut vs);
        let vp_ref = &vp;
        let mut targets: Vec<Target> = Vec::new();
        for (j, cd) in cross_ders.iter().enumerate() {
            let label = vs_cell.borrow()[j].label.clone();
            let cell = &vs_cell;
            let ders = [cd[0][..end_order[0]].to_vec(), cd[1][..end_order[1]].to_vec()];
            targets.push(Target {
                label,
                value: Box::new(move |t| {
                    let mut tracks = cell.borrow_mut();
                    vp_ref[j].point(&mut tracks[j], t)
                }),
                ders,
                tol: tol.fit,
            });
        }
        fit_direction(&base_v, end_order, &mut targets)?
    };
    let nv_ctrl = v_space.count();
    // P_v: across u, row by row of the v-direction control points.
    let mut pv_rows: Vec<Vec<Vec3>> = Vec::with_capacity(nv_ctrl);
    for b in 0..nv_ctrl {
        pv_rows.push(across_u.fit(|d| match d {
            Datum::Curve(j) => v_ctrl.get(j).and_then(|c| c.get(b)).copied().ok_or_else(|| SplineError::Invalid("v-curve".into())),
            _ => Ok(Vec3::ZERO),
        })?);
    }
    // P_uv: the crossing points (and cross derivatives) across v, then across u.
    let mut cols: Vec<Vec<Vec3>> = Vec::with_capacity(nv);
    for (j, cd) in cross_ders.iter().enumerate() {
        cols.push(across_v.fit(|d| match d {
            Datum::Curve(i) => x.get(i).and_then(|row| row.get(j)).map(|c| c.at).ok_or_else(|| SplineError::Invalid("crossing".into())),
            Datum::Der { end, order } => Ok(cd.get(end).and_then(|e| e.get(order.wrapping_sub(1))).copied().unwrap_or(Vec3::ZERO)),
            Datum::Zero => Ok(Vec3::ZERO),
        })?);
    }
    let nvop = v_op_space.count();
    let mut puv_rows: Vec<Vec<Vec3>> = Vec::with_capacity(nvop);
    for b in 0..nvop {
        puv_rows.push(across_u.fit(|d| match d {
            Datum::Curve(j) => cols.get(j).and_then(|c| c.get(b)).copied().ok_or_else(|| SplineError::Invalid("crossing".into())),
            _ => Ok(Vec3::ZERO),
        })?);
    }
    // Into the common space KU × KV and summed.
    let u_op_space = across_u.space().clone();
    let pu_full: Vec<Vec<Vec3>> = pu.iter().map(|col| embed(&v_op_space, col, &v_space)).collect::<SplineResult<_>>()?;
    let pv_cols = transpose(&pv_rows.iter().map(|row| embed(&u_op_space, row, &u_space)).collect::<SplineResult<Vec<_>>>()?);
    let puv_rows_u: Vec<Vec<Vec3>> = puv_rows.iter().map(|row| embed(&u_op_space, row, &u_space)).collect::<SplineResult<_>>()?;
    let puv_cols: Vec<Vec<Vec3>> = transpose(&puv_rows_u).iter().map(|col| embed(&v_op_space, col, &v_space)).collect::<SplineResult<_>>()?;
    let mut net = Vec::with_capacity(nu_ctrl);
    for a in 0..nu_ctrl {
        let col: Vec<Vec3> = (0..nv_ctrl)
            .map(|b| {
                let get = |m: &Vec<Vec<Vec3>>| m.get(a).and_then(|c| c.get(b)).copied().unwrap_or(Vec3::ZERO);
                get(&pu_full) + get(&pv_cols) - get(&puv_cols)
            })
            .collect();
        net.push(col);
    }
    surface(&u_space, &v_space, &net)
}

/// A conditioned end of the u-direction: which curve, the derivative orders, its neighbouring
/// curve (and the parameter gap to it) for the natural tangent, the scalar values the v-curves
/// give at each crossing (`[a, c, kb, kt]`: the tangent's outward and along-curve components, the
/// second derivative's), and the corrections to the natural values (control values across u).
struct EndCondition {
    end: usize,
    curve: usize,
    order: usize,
    neighbour: Option<(usize, f64)>,
    at: Vec<[f64; 4]>,
    corr: [Option<Vec<f64>>; 4],
}

impl EndCondition {
    /// The natural `[a, c, kb, kt]` at `x`: the tangent as long as the step to the neighbouring
    /// curve, outward from the face (into it at the far end), sheared along the curve as that
    /// step is; no tangential second derivative.
    fn natural(&self, us: &mut [Track], up: &[Placement], frame: &Frame, x: f64) -> SplineResult<[f64; 4]> {
        let Some((k, gap)) = self.neighbour else { return Ok([0.0; 4]) };
        let p = up.get(self.curve).ok_or_else(|| SplineError::Invalid("curve".into()))?.point(track(us, self.curve)?, x)?;
        let q = up.get(k).ok_or_else(|| SplineError::Invalid("curve".into()))?.point(track(us, k)?, x)?;
        let step = if self.end == 0 { q - p } else { p - q };
        let gap = gap.max(1e-12);
        let a = step.len() / gap * if self.end == 0 { 1.0 } else { -1.0 };
        Ok([a, step.dot(frame.t) / gap, 0.0, 0.0])
    }

    /// The first and second v-derivatives of the surface along this boundary at `x`.
    fn derivatives(&self, us: &mut [Track], up: &[Placement], across_u: Option<&Across>, x: f64) -> SplineResult<(Vec3, Vec3)> {
        self.derivatives_framed(us, up, across_u, x).map(|(d1, d2, _)| (d1, d2))
    }

    /// [`EndCondition::derivatives`] with the face's frame there (projecting onto the face is
    /// the costly part: it is done once).
    fn derivatives_framed(&self, us: &mut [Track], up: &[Placement], across_u: Option<&Across>, x: f64) -> SplineResult<(Vec3, Vec3, Frame)> {
        let frame = end_frame(us, up, self.curve, x)?;
        let mut v = self.natural(us, up, &frame, x)?;
        if let Some(au) = across_u {
            for (k, c) in self.corr.iter().enumerate() {
                if let Some(c) = c {
                    v[k] += au.space().eval(c, x, 0)?;
                }
            }
        }
        let [a, c, kb, kt] = v;
        let d1 = frame.b * a + frame.t * c;
        let d2 = frame.n * frame.second(a, c) + frame.b * kb + frame.t * kt;
        Ok((d1, d2, frame))
    }

    /// Typical sizes of the first and second derivatives along the boundary (for fitting
    /// tolerances), checking that the tangent never turns back along it.
    fn scale(&self, us: &mut [Track], up: &[Placement], across_u: Option<&Across>) -> SplineResult<[f64; 2]> {
        let mut big = [0.0f64; 2];
        for k in 0..=32 {
            let x = k as f64 / 32.0;
            let (d1, d2, frame) = self.derivatives_framed(us, up, across_u, x)?;
            let out = d1.dot(frame.b) * if self.end == 0 { 1.0 } else { -1.0 };
            if !(out > 0.0) {
                let label = us.get(self.curve).map(|t| t.label.clone()).unwrap_or_default();
                return invalid(format!("the surface's tangent across {label} would turn back over its face"));
            }
            big = [big[0].max(d1.len()), big[1].max(d2.len())];
        }
        Ok([big[0].max(1e-9), big[1].max(big[0] * 1e-3).max(1e-9)])
    }
}

fn track<'t, 'a>(us: &'t mut [Track<'a>], i: usize) -> SplineResult<&'t mut Track<'a>> {
    us.get_mut(i).ok_or_else(|| SplineError::Invalid("curve".into()))
}

/// The face's frame along conditioned u-curve `i` at network parameter `x`.
fn end_frame(us: &mut [Track], up: &[Placement], i: usize, x: f64) -> SplineResult<Frame> {
    let place = up.get(i).ok_or_else(|| SplineError::Invalid("curve".into()))?;
    let t = track(us, i)?;
    let p = place.point(t, x)?;
    let (d1, _) = place.ders(t, x)?;
    let label = t.label.clone();
    let face = t.condition.as_mut().map(|(_, f)| f).ok_or_else(|| SplineError::Invalid(format!("{label} has no face")))?;
    let f = face
        .frame(p, d1)
        .ok_or_else(|| SplineError::Invalid(format!("the face along {label} can't be evaluated at ({:.3}, {:.3}, {:.3})", p.x, p.y, p.z)))?;
    let n = f.normal.normalized().ok_or_else(|| SplineError::Invalid(format!("the face along {label} has no normal")))?;
    let t = (d1 - n * d1.dot(n)).normalized().ok_or_else(|| SplineError::Invalid(format!("{label} runs along its face's normal")))?;
    let mut b = f.outward - n * f.outward.dot(n);
    b = (b - t * b.dot(t)).normalized().ok_or_else(|| SplineError::Invalid(format!("the face along {label} has no outward direction")))?;
    Ok(Frame { n, t, b, form: f.form })
}

/// `true` when `xs` increase strictly, `false` when they decrease strictly, `None` otherwise.
fn direction(xs: &[f64]) -> Option<bool> {
    let gap = 1e-9;
    if xs.windows(2).all(|w| w[1] - w[0] > gap) {
        Some(true)
    } else if xs.windows(2).all(|w| w[0] - w[1] > gap) {
        Some(false)
    } else {
        None
    }
}

fn reorder<T>(items: Vec<T>, order: &[usize]) -> Vec<T> {
    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    order.iter().filter_map(|&k| slots.get_mut(k).and_then(Option::take)).collect()
}

fn transpose(m: &[Vec<Vec3>]) -> Vec<Vec<Vec3>> {
    let cols = m.first().map_or(0, Vec::len);
    (0..cols).map(|c| m.iter().map(|row| row.get(c).copied().unwrap_or(Vec3::ZERO)).collect()).collect()
}

fn sorted_breaks(mut b: Vec<f64>) -> Vec<f64> {
    b.retain(|x| x.is_finite() && (0.0..=1.0).contains(x));
    b.sort_by(f64::total_cmp);
    b.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    b
}

/// The network parameters of the crossings along one direction: `rows[i][j]` is where curve `j`
/// of the other direction crosses curve `i` (arc length). With two crossings or more each curve
/// runs from its first to its last crossing; the parameters are the relative positions,
/// averaged over the curves. With one, it is its mean position along the whole curves.
fn network_params(rows: &[Vec<f64>], count: usize) -> SplineResult<Vec<f64>> {
    let n = rows.len().max(1) as f64;
    match count {
        0 => Ok(Vec::new()),
        1 => {
            let firsts: Vec<f64> = rows.iter().filter_map(|r| r.first().copied()).collect();
            let at_end = |x: f64| x <= 1e-9 || x >= 1.0 - 1e-9;
            if firsts.iter().any(|x| at_end(*x)) && !firsts.iter().all(|x| at_end(*x)) {
                return invalid("a curve crosses the others at the end of some and partway along others");
            }
            Ok(vec![firsts.iter().sum::<f64>() / n])
        }
        _ => {
            let mut out = vec![0.0; count];
            for r in rows {
                let (Some(&a), Some(&b)) = (r.first(), r.last()) else { return invalid("crossings") };
                if !(b - a > 1e-12) {
                    return invalid("the curves cross out of order");
                }
                for (o, x) in out.iter_mut().zip(r) {
                    *o += (x - a) / (b - a) / n;
                }
            }
            out[0] = 0.0;
            out[count - 1] = 1.0;
            if out.windows(2).any(|w| !(w[1] - w[0] > 1e-9)) {
                return invalid("two curves cross the others at the same place");
            }
            Ok(out)
        }
    }
}

/// Lofting parameters of curves in one direction only: cumulative mean distances between
/// neighbouring curves, from 0 to 1.
fn chord_params(us: &mut [Track]) -> SplineResult<Vec<f64>> {
    let mut out = vec![0.0];
    let mut acc = 0.0;
    for i in 1..us.len() {
        let mut d = 0.0;
        for k in 0..=32 {
            let s = k as f64 / 32.0;
            let p = track(us, i - 1)?.point(s)?;
            let q = track(us, i)?.point(s)?;
            d += p.dist(q) / 33.0;
        }
        if !(d > 1e-9) {
            let (a, b) = (us[i - 1].label.clone(), us[i].label.clone());
            return invalid(format!("{a} and {b} lie on each other"));
        }
        acc += d;
        out.push(acc);
    }
    Ok(out.into_iter().map(|x| x / acc).collect())
}

/// The nodes `(network parameter, arc length)` a curve is placed through: its crossings, and
/// with fewer than two the curve's ends at 0 and 1 (a crossing at an end standing for that end).
fn place_nodes(cross: &[(f64, f64, Vec3)], count: usize) -> Vec<(f64, f64)> {
    if count >= 2 {
        return cross.iter().map(|c| (c.0, c.1)).collect();
    }
    let mut nodes = vec![(0.0, 0.0)];
    if let Some(&(x, s, _)) = cross.first()
        && x > 1e-9
        && x < 1.0 - 1e-9
        && s > 1e-9
        && s < 1.0 - 1e-9
    {
        nodes.push((x, s));
    }
    nodes.push((1.0, 1.0));
    nodes
}

/// The tensor surface of a net of control points (`net[a][b]`: a along u).
fn surface(su: &Space, sv: &Space, net: &[Vec<Vec3>]) -> SplineResult<NurbsSurface> {
    let (nu, nv) = (su.count(), sv.count());
    if net.len() != nu || net.iter().any(|c| c.len() != nv) {
        return invalid("control net size");
    }
    let mut pts = Vec::with_capacity(nu * nv);
    for b in 0..nv {
        for col in net {
            pts.push(col[b]);
        }
    }
    let weights = vec![1.0; pts.len()];
    NurbsSurface::new([su.degree(), sv.degree()], [su.knots().to_vec(), sv.knots().to_vec()], &pts, &weights)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A curve given by a function of its parameter.
    struct F<G: FnMut(f64) -> Vec3>(G);
    impl<G: FnMut(f64) -> Vec3> NetCurve for F<G> {
        fn point(&mut self, t: f64) -> Option<Vec3> {
            Some((self.0)(t))
        }
    }

    fn curve<'a>(label: &str, f: impl FnMut(f64) -> Vec3 + 'a) -> NetworkCurve<'a> {
        NetworkCurve { label: label.into(), curve: Box::new(F(f)), condition: None }
    }

    fn line<'a>(label: &str, a: Vec3, b: Vec3) -> NetworkCurve<'a> {
        curve(label, move |t| a.lerp(b, t))
    }

    const TOL: Tolerances = Tolerances { cross: 1e-3, fit: 1e-5 };

    fn grid(s: &NurbsSurface, n: usize) -> Vec<Vec3> {
        let [du, dv] = s.domain();
        let mut out = Vec::new();
        for i in 0..=n {
            for j in 0..=n {
                let (u, v) = (du[0] + (du[1] - du[0]) * i as f64 / n as f64, dv[0] + (dv[1] - dv[0]) * j as f64 / n as f64);
                out.push(s.eval(u, v).unwrap());
            }
        }
        out
    }

    /// Distance from `p` to the polyline of `f` sampled finely.
    fn dist_to(p: Vec3, f: &dyn Fn(f64) -> Vec3) -> f64 {
        (0..2000).map(|k| p.dist_to_segment(f(k as f64 / 2000.0), f((k + 1) as f64 / 2000.0))).fold(f64::INFINITY, f64::min)
    }

    /// The bilinearly blended Coons patch of four straight lines reproduces the hyperbolic
    /// paraboloid z = x·y they bound, exactly; four coplanar curves (two arcs, two lines) give a
    /// patch in their plane.
    #[test]
    fn coons_patch_reproduces_a_hyperbolic_paraboloid_and_a_plane() {
        let p = |x: f64, y: f64| Vec3::new(x, y, x * y);
        let s = network_surface(
            vec![line("u0", p(0.0, 0.0), p(4.0, 0.0)), line("u1", p(0.0, 3.0), p(4.0, 3.0))],
            vec![line("v0", p(0.0, 0.0), p(0.0, 3.0)), line("v1", p(4.0, 3.0), p(4.0, 0.0))],
            TOL,
        )
        .unwrap();
        for q in grid(&s, 20) {
            assert!((q.z - q.x * q.y).abs() < 1e-9, "{q:?} off z = xy by {}", q.z - q.x * q.y);
        }
        assert!(s.eval(0.5, 0.5).unwrap().dist(p(2.0, 1.5)) < 1e-9, "the middle of the patch");
        // A plane z = 0.3x + 0.2y + 1 bounded by two arcs bulging out and two lines.
        let on = |x: f64, y: f64| Vec3::new(x, y, 0.3 * x + 0.2 * y + 1.0);
        let arc = |y: f64, sign: f64| move |t: f64| on(10.0 * t, y + sign * 2.0 * (std::f64::consts::PI * t).sin());
        let s = network_surface(
            vec![curve("bottom", arc(0.0, -1.0)), curve("top", arc(8.0, 1.0))],
            vec![line("left", on(0.0, 0.0), on(0.0, 8.0)), line("right", on(10.0, 0.0), on(10.0, 8.0))],
            TOL,
        )
        .unwrap();
        for q in grid(&s, 20) {
            assert!((q.z - (0.3 * q.x + 0.2 * q.y + 1.0)).abs() < 1e-9, "{q:?} off the plane");
        }
    }

    /// Area of a surface over its whole domain: |S_u × S_v| by central differences on an n × n grid
    /// of cell centres.
    fn area(s: &NurbsSurface, n: usize) -> f64 {
        let [du, dv] = s.domain();
        let (hu, hv) = ((du[1] - du[0]) / n as f64, (dv[1] - dv[0]) / n as f64);
        let mut a = 0.0;
        for i in 0..n {
            for j in 0..n {
                let (u, v) = (du[0] + hu * (i as f64 + 0.5), dv[0] + hv * (j as f64 + 0.5));
                let su = (s.eval(u + hu * 0.25, v).unwrap() - s.eval(u - hu * 0.25, v).unwrap()) / (hu * 0.5);
                let sv = (s.eval(u, v + hv * 0.25).unwrap() - s.eval(u, v - hv * 0.25).unwrap()) / (hv * 0.5);
                a += su.cross(sv).len() * hu * hv;
            }
        }
        a
    }

    fn apex<'a>(label: &str, p: Vec3) -> NetworkCurve<'a> {
        NetworkCurve { label: label.into(), curve: Box::new(PointCurve(p)), condition: None }
    }

    /// Three sides: one of them a point. A triangle of lines and a quarter disc (two radii and the
    /// arc, the apex at the centre) are filled flat, the whole patch on their plane, every point of
    /// the collapsed side at the apex, with the area of the triangle and of the quarter disc.
    #[test]
    fn three_sides_collapse_one_to_a_point() {
        let (a, b, c) = (Vec3::new(0.0, 0.0, 2.0), Vec3::new(10.0, 0.0, 2.0), Vec3::new(4.0, 8.0, 2.0));
        let s = network_surface(vec![line("ab", a, b), apex("c", c)], vec![line("ac", a, c), line("cb", c, b)], TOL).unwrap();
        for q in grid(&s, 20) {
            assert!((q.z - 2.0).abs() < 1e-9, "{q:?} off the plane");
        }
        let [du, dv] = s.domain();
        let collapsed = [s.eval(du[0], dv[0]).unwrap(), s.eval(du[1], dv[0]).unwrap(), s.eval(du[0], dv[1]).unwrap(), s.eval(du[1], dv[1]).unwrap()];
        assert!(collapsed.iter().filter(|p| p.dist(c) < 1e-12).count() == 2, "two corners at the apex: {collapsed:?}");
        let tri = area(&s, 200);
        assert!((tri - 40.0).abs() < 1e-3, "triangle area {tri}");
        // The quarter disc of radius 10 about the origin.
        let r = 10.0;
        let arc = move |t: f64| Vec3::new(r * (std::f64::consts::FRAC_PI_2 * t).cos(), r * (std::f64::consts::FRAC_PI_2 * t).sin(), 0.0);
        let s = network_surface(
            vec![curve("arc", arc), apex("centre", Vec3::ZERO)],
            vec![line("x", Vec3::ZERO, Vec3::X * r), line("y", Vec3::Y * r, Vec3::ZERO)],
            TOL,
        )
        .unwrap();
        for q in grid(&s, 20) {
            assert!(q.z.abs() < 1e-9 && q.len() <= r + 1e-4, "{q:?} off the quarter disc");
        }
        let quarter = area(&s, 200);
        let want = std::f64::consts::PI * r * r / 4.0;
        assert!((quarter - want).abs() < 2e-3 * want, "quarter disc area {quarter} vs {want}");
        // A point given as a v-curve is the same patch.
        let s2 = network_surface(
            vec![line("x", Vec3::ZERO, Vec3::X * r), line("y", Vec3::Y * r, Vec3::ZERO)],
            vec![curve("arc", arc), apex("centre", Vec3::ZERO)],
            TOL,
        )
        .unwrap();
        assert!((area(&s2, 200) - quarter).abs() < 1e-6);
    }

    /// Points where a patch has no side for them, a point that moves, and a point with a
    /// continuity condition are refused, never a panic.
    #[test]
    fn misplaced_points_are_refused() {
        let (a, b, c) = (Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(4.0, 8.0, 0.0));
        let two = network_surface(vec![apex("p", a), apex("q", c)], vec![], TOL).unwrap_err();
        assert!(two.to_string().contains("point"), "{two}");
        let both = network_surface(vec![line("ab", a, b), apex("c", c)], vec![apex("a", a), line("cb", c, b)], TOL).unwrap_err();
        assert!(both.to_string().contains("point"), "{both}");
        let lone = network_surface(vec![apex("c", c)], vec![line("ac", a, c), line("cb", c, b)], TOL);
        assert!(lone.is_err());
        let one_across = network_surface(vec![line("ab", a, b), apex("c", c)], vec![line("ac", a, c)], TOL).unwrap_err();
        assert!(one_across.to_string().contains("point"), "{one_across}");
        // A point between two curves of its direction.
        let mid = network_surface(
            vec![line("low", a, b), apex("mid", Vec3::new(5.0, 4.0, 0.0)), line("high", Vec3::new(0.0, 8.0, 0.0), Vec3::new(10.0, 8.0, 0.0))],
            vec![],
            TOL,
        )
        .unwrap_err();
        assert!(mid.to_string().contains("inside"), "{mid}");
        struct Moving;
        impl NetCurve for Moving {
            fn point(&mut self, t: f64) -> Option<Vec3> {
                Some(Vec3::new(t, 0.0, 0.0))
            }
            fn is_point(&self) -> bool {
                true
            }
        }
        let moving = NetworkCurve { label: "m".into(), curve: Box::new(Moving), condition: None };
        let e = network_surface(vec![line("ab", a, b), moving], vec![], TOL).unwrap_err();
        assert!(e.to_string().contains("moves"), "{e}");
        let with_g1 = NetworkCurve { label: "g".into(), curve: Box::new(PointCurve(c)), condition: Some((Continuity::G1, Box::new(Plane))) };
        let e = network_surface(vec![line("ab", a, b), with_g1], vec![], TOL).unwrap_err();
        assert!(e.to_string().contains("point"), "{e}");
    }

    /// Two curves loft straight between them; three or more pass smoothly through all; curves
    /// given running opposite ways are turned to agree.
    #[test]
    fn curves_one_way_are_lofted() {
        let ruled =
            network_surface(vec![line("a", Vec3::ZERO, Vec3::X * 10.0), line("b", Vec3::new(10.0, 5.0, 2.0), Vec3::new(0.0, 5.0, 2.0))], vec![], TOL)
                .unwrap();
        assert_eq!(ruled.degree()[1], 1, "ruled between two curves");
        for q in grid(&ruled, 10) {
            assert!((q.z - 0.4 * q.y).abs() < 1e-9, "{q:?}: a flat strip, not twisted");
        }
        let arc = |z: f64, r: f64| move |t: f64| Vec3::new(r * (std::f64::consts::PI * t).cos(), r * (std::f64::consts::PI * t).sin(), z);
        let fs: Vec<(f64, f64)> = vec![(0.0, 5.0), (4.0, 6.0), (8.0, 5.5), (12.0, 7.0)];
        let s = network_surface(fs.iter().enumerate().map(|(k, &(z, r))| curve(&format!("c{k}"), arc(z, r))).collect(), vec![], TOL).unwrap();
        // Every section lies on the surface: its iso-curve at the section's parameter is close.
        let pts = grid(&s, 60);
        for &(z, r) in &fs {
            let f = arc(z, r);
            for k in 0..=10 {
                let p = f(k as f64 / 10.0);
                let d = pts.iter().map(|q| q.dist(p)).fold(f64::INFINITY, f64::min);
                assert!(d < 0.3, "section at z = {z} is {d} from the surface's samples");
            }
        }
    }

    /// A 3 × 3 network: the Gordon surface passes through all six curves (within the fit
    /// tolerance), the interior ones included.
    #[test]
    fn gordon_surface_passes_through_every_curve() {
        let z = |x: f64, y: f64| 0.05 * x * x - 0.03 * y * y + 0.02 * x * y;
        let ux: Vec<f64> = vec![0.0, 4.0, 10.0];
        let vy: Vec<f64> = vec![0.0, 3.0, 8.0];
        let ucurve = |y: f64| move |t: f64| Vec3::new(10.0 * t, y, z(10.0 * t, y));
        let vcurve = |x: f64| move |t: f64| Vec3::new(x, 8.0 * t, z(x, 8.0 * t));
        let s = network_surface(
            vy.iter().map(|&y| curve(&format!("y={y}"), ucurve(y))).collect(),
            ux.iter().rev().map(|&x| curve(&format!("x={x}"), vcurve(x))).collect(),
            TOL,
        )
        .unwrap();
        // The surface's iso-curves at the crossing parameters are the given curves.
        let [du, dv] = s.domain();
        for &y in &vy {
            let f = ucurve(y);
            // Find the v of this curve by its start point.
            let start = f(0.0);
            let off = |v: f64| s.eval(du[0], v).unwrap().dist(start);
            let mut v = (0..=1000).map(|k| dv[0] + (dv[1] - dv[0]) * k as f64 / 1000.0).min_by(|a, b| off(*a).total_cmp(&off(*b))).unwrap();
            let (mut lo, mut hi) = ((v - 0.002).max(dv[0]), (v + 0.002).min(dv[1]));
            for _ in 0..100 {
                let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
                if off(a) < off(b) { hi = b } else { lo = a }
            }
            v = (lo + hi) / 2.0;
            for k in 0..=20 {
                let q = s.eval(du[0] + (du[1] - du[0]) * k as f64 / 20.0, v).unwrap();
                assert!(dist_to(q, &f) < 1e-4, "y = {y}: {q:?} is {} off", dist_to(q, &f));
            }
        }
        for q in grid(&s, 10) {
            assert!((q.z - z(q.x, q.y)).abs() < 0.05, "{q:?} strays from the network's surface");
        }
    }

    /// A plane face along the x axis (normal +z, the face on the −y side): a frame for G1 and G2.
    struct Plane;
    impl CrossFace for Plane {
        fn frame(&mut self, _: Vec3, _: Vec3) -> Option<FaceFrame> {
            Some(FaceFrame { normal: Vec3::Z, outward: Vec3::Y, form: [0.0; 3] })
        }
    }

    /// A cylinder of radius R about the x axis, the face below its edge at (y = R, z = 0): it
    /// leaves the edge straight up (+z), bending toward the axis.
    struct Cylinder(f64);
    impl CrossFace for Cylinder {
        fn frame(&mut self, at: Vec3, tangent: Vec3) -> Option<FaceFrame> {
            let n = Vec3::new(0.0, at.y, at.z).normalized()?;
            let _ = tangent;
            // Outward normal n; curvature −1/R in the direction around the axis, none along it.
            Some(FaceFrame { normal: n, outward: Vec3::Z, form: [0.0, 0.0, -1.0 / self.0] })
        }
    }

    fn deriv_v(s: &NurbsSurface, u: f64, h: f64) -> (Vec3, Vec3) {
        let f: Vec<Vec3> = (0..4).map(|k| s.eval(u, k as f64 * h).unwrap()).collect();
        ((f[0] * -3.0 + f[1] * 4.0 - f[2]) / (2.0 * h), (f[0] * 2.0 - f[1] * 5.0 + f[2] * 4.0 - f[3]) / (h * h))
    }

    /// G1: the surface leaves a plane edge tangent to the plane. G2 against a cylinder: the
    /// surface's normal curvature across the edge is the cylinder's, 1/R; with G1 only it is
    /// not.
    #[test]
    fn g1_and_g2_conditions_meet_the_face() {
        let edge = |c: Box<dyn CrossFace>, g| NetworkCurve {
            label: "edge".into(),
            curve: Box::new(F(|t: f64| Vec3::new(10.0 * t, 0.0, 0.0))),
            condition: Some((g, c)),
        };
        let s = network_surface(
            vec![edge(Box::new(Plane), Continuity::G1), line("far", Vec3::new(0.0, 10.0, 5.0), Vec3::new(10.0, 10.0, 5.0))],
            vec![],
            TOL,
        )
        .unwrap();
        for k in 0..=10 {
            let u = k as f64 / 10.0;
            let (d1, _) = deriv_v(&s, u, 1e-5);
            let angle = (d1.z / d1.len()).asin().to_degrees().abs();
            assert!(angle < 1e-3 && d1.y > 0.0, "u = {u}: leaves the plane at {angle}°");
        }
        let r = 10.0;
        let cyl = |g| {
            let edge = NetworkCurve {
                label: "rim".into(),
                curve: Box::new(F(move |t: f64| Vec3::new(10.0 * t, r, 0.0))),
                condition: Some((g, Box::new(Cylinder(r)) as Box<dyn CrossFace>)),
            };
            network_surface(vec![edge, line("top", Vec3::new(0.0, 3.0, 12.0), Vec3::new(10.0, 3.0, 12.0))], vec![], TOL).unwrap()
        };
        let curvature = |s: &NurbsSurface, u: f64| {
            let (d1, d2) = deriv_v(s, u, 1e-4);
            // Curvature of the v-curve: |d1 × d2| / |d1|³, toward the axis (−y).
            (d1.cross(d2).len() / d1.len().powi(3), d2.y)
        };
        let g2 = cyl(Continuity::G2);
        for k in 0..=4 {
            let (kappa, _) = curvature(&g2, k as f64 / 4.0);
            assert!((kappa - 1.0 / r).abs() < 1e-3 / r, "G2: curvature {kappa} vs {}", 1.0 / r);
        }
        let g1 = cyl(Continuity::G1);
        let (kappa, _) = curvature(&g1, 0.5);
        assert!((kappa - 1.0 / r).abs() > 0.1 / r, "G1 alone does not match the curvature: {kappa}");
    }

    /// A Coons patch with a G1 edge: the side curves leave the plane tangent to it (arcs), so the
    /// patch does too; a side curve that leaves it at an angle is refused, named.
    #[test]
    fn crossing_curves_must_agree_with_a_g1_face() {
        let edge = || NetworkCurve {
            label: "edge".into(),
            curve: Box::new(F(|t: f64| Vec3::new(10.0 * t, 0.0, 0.0))),
            condition: Some((Continuity::G1, Box::new(Plane) as Box<dyn CrossFace>)),
        };
        // Arcs in x = const from (y, z) = (0, 0), tangent to +y, to (10, 5): centre (0, 12.5).
        let side = |x: f64| {
            move |t: f64| {
                let a = -std::f64::consts::FRAC_PI_2 + t * 0.927_295_218_001_612_2;
                Vec3::new(x, 12.5 * a.cos(), 12.5 + 12.5 * a.sin())
            }
        };
        let far = line("far", Vec3::new(0.0, 10.0, 5.0), Vec3::new(10.0, 10.0, 5.0));
        let s = network_surface(vec![edge(), far], vec![curve("left", side(0.0)), curve("right", side(10.0))], TOL).unwrap();
        for k in 0..=10 {
            let (d1, _) = deriv_v(&s, k as f64 / 10.0, 1e-5);
            let angle = (d1.z / d1.len()).asin().to_degrees().abs();
            assert!(angle < 0.01, "leaves the plane at {angle}°");
        }
        let far = line("far", Vec3::new(0.0, 10.0, 5.0), Vec3::new(10.0, 10.0, 5.0));
        let steep = line("steep", Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 10.0, 5.0));
        let err = network_surface(vec![edge(), far], vec![steep, curve("right", side(10.0))], TOL).unwrap_err();
        assert!(err.to_string().contains("steep") && err.to_string().contains("tangent"), "{err}");
    }

    /// Networks that can't be built are refused, naming the curves; none panics.
    #[test]
    fn bad_networks_are_refused() {
        let l = |n: &str, a: [f64; 3], b: [f64; 3]| line(n, Vec3::from(a), Vec3::from(b));
        let miss = network_surface(
            vec![l("a", [0.0, 0.0, 0.0], [10.0, 0.0, 0.0]), l("b", [0.0, 5.0, 0.0], [10.0, 5.0, 0.0])],
            vec![l("c", [0.0, 0.0, 0.0], [0.0, 5.0, 0.0]), l("d", [10.0, 0.0, 1.0], [10.0, 5.0, 1.0])],
            TOL,
        )
        .unwrap_err();
        assert!(miss.to_string().contains("a and d") || miss.to_string().contains("b and d"), "{miss}");
        assert!(miss.to_string().contains("don't meet"), "{miss}");
        let one = network_surface(vec![l("a", [0.0, 0.0, 0.0], [1.0, 0.0, 0.0])], vec![], TOL);
        assert!(one.is_err());
        assert!(network_surface(vec![], vec![], TOL).is_err());
        let circle = curve("ring", |t: f64| Vec3::new((6.3 * t).cos(), (6.3 * t).sin(), 0.0).lerp(Vec3::X, if t >= 1.0 { 1.0 } else { 0.0 }));
        let closed = network_surface(vec![circle, l("b", [0.0, 0.0, 1.0], [1.0, 0.0, 1.0])], vec![], TOL).unwrap_err();
        assert!(closed.to_string().contains("closed"), "{closed}");
        let zero = network_surface(vec![l("a", [0.0; 3], [0.0; 3]), l("b", [0.0, 0.0, 1.0], [1.0, 0.0, 1.0])], vec![], TOL).unwrap_err();
        assert!(zero.to_string().contains("no length"), "{zero}");
        let nan = network_surface(vec![l("a", [f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0]), l("b", [0.0, 0.0, 1.0], [1.0, 0.0, 1.0])], vec![], TOL);
        assert!(nan.is_err());
        // A parabola crossing a line twice.
        let wave = curve("wave", |t: f64| Vec3::new(10.0 * t, 4.0 * (2.0 * t - 1.0).powi(2) - 1.0, 0.0));
        let twice = network_surface(
            vec![l("a", [0.0, 0.0, 0.0], [10.0, 0.0, 0.0]), l("b", [0.0, 5.0, 0.0], [10.0, 5.0, 0.0])],
            vec![l("c", [0.0, 0.0, 0.0], [0.0, 5.0, 0.0]), wave],
            TOL,
        )
        .unwrap_err();
        assert!(twice.to_string().contains("more than once"), "{twice}");
        // Conditions on both directions.
        let cond = |n: &str, a: [f64; 3], b: [f64; 3]| {
            let (a, b) = (Vec3::from(a), Vec3::from(b));
            NetworkCurve {
                label: n.into(),
                curve: Box::new(F(move |t: f64| a.lerp(b, t))),
                condition: Some((Continuity::G1, Box::new(Plane) as Box<dyn CrossFace>)),
            }
        };
        let both = network_surface(
            vec![cond("a", [0.0, 0.0, 0.0], [10.0, 0.0, 0.0]), l("b", [0.0, 5.0, 0.0], [10.0, 5.0, 0.0])],
            vec![cond("c", [0.0, 0.0, 0.0], [0.0, 5.0, 0.0]), l("d", [10.0, 0.0, 0.0], [10.0, 5.0, 0.0])],
            TOL,
        )
        .unwrap_err();
        assert!(matches!(both, SplineError::Unsupported(_)), "{both}");
        let bad_tol = network_surface(
            vec![l("a", [0.0; 3], [1.0, 0.0, 0.0]), l("b", [0.0, 1.0, 0.0], [1.0, 1.0, 0.0])],
            vec![],
            Tolerances { cross: f64::NAN, fit: 1.0 },
        );
        assert!(bad_tol.is_err());
    }
}
