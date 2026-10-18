//! The constraint solver.
//!
//! Unknowns are the coordinates of every non-fixed point and the radius of every circle. Each
//! constraint contributes residuals (zero when satisfied, in millimetres so that all rows have a
//! comparable scale). The system is split into connected components and each is solved with a
//! damped Gauss–Newton (Levenberg–Marquardt) iteration on a dense Jacobian built from local
//! central differences. Small steps from the current positions are preferred, so
//! under-constrained sketches move as little as possible. Afterwards the rank of the Jacobian
//! gives the remaining degrees of freedom and which points are fully determined.
//!
//! Unknowns the constraints pin down one after another from the constants (a point at the
//! crossing of two loci on known points, see [`pinned`]) are eliminated exactly, without
//! the dense matrices: values the constructive seed placed stay constants for LM, and the rank
//! analysis runs only on what remains.

use serde::{Deserialize, Serialize};
use solidcraft_geom::{Seg2, Vec2};

use crate::decompose::Budget;
use crate::linalg::{Mat, determined_vars, solve as lin_solve};
use crate::model::{ConstraintKind, CurveKind, Sketch};
use crate::union_find::{find, union};

#[path = "redundancy.rs"]
mod redundancy;
pub use redundancy::{CONSISTENT, constraint_residual, determined_by};

const MAX_ITERS: usize = 500;
/// Variables in one connected component. Each component is solved as a dense system: an n by n
/// matrix for the Levenberg-Marquardt step, rows by n for the Jacobian. 4,000 variables is a
/// 128 MB matrix; past that the request grows to gigabytes and an allocation failure aborts the
/// process, which no panic guard can intercept. A component this size is reported as failed and
/// its geometry left alone.
const MAX_COMPONENT_VARS: usize = 4_000;
/// Converged when every residual is below this (mm).
const TOL: f64 = 1e-9;
/// A solution is accepted when every residual is below this (mm).
const ACCEPT: f64 = 1e-6;
/// How much less it costs a solve to move a point held on a curve than any other point: such a
/// point is placed relative to the curve, so it follows the curve rather than the curve it.
const SLIDE_WEIGHT: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolveStatus {
    /// All constraints satisfied.
    Solved,
    /// The constraints conflict (or the solver could not satisfy them); geometry was left as it
    /// was before solving.
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolveReport {
    pub status: SolveStatus,
    /// Remaining degrees of freedom (0 = fully constrained).
    pub dof: usize,
    pub iterations: usize,
    pub max_residual: f64,
    /// Constraints still violated after a failed solve, and the ones in `folded`.
    pub failing: Vec<String>,
    /// Constraints whose orientation (a triangle's turn, an offset's side) the solve for edited
    /// driving values flipped relative to the stored solved state, where continuation could
    /// not keep it. The solve keeps that best result (an evaluation shows it with a warning);
    /// a command that changes values refuses it.
    #[serde(default)]
    pub folded: Vec<String>,
    /// Components this solve stepped from the stored solved state to the edited values.
    #[serde(default)]
    pub continuations: usize,
    /// Per point: fully determined by constraints (fixed points count as determined).
    pub point_determined: Vec<bool>,
    /// Per curve: all of its defining values determined.
    pub curve_determined: Vec<bool>,
    /// Constraints that remove no degree of freedom: everything they ask for is already implied
    /// by the constraints before them (a second "vertical" on a side two rectangles share).
    /// They hold, so they are not a conflict; they are shown as redundant. Only from a full
    /// analysis, and only for components of at most a few hundred variables.
    #[serde(default)]
    pub redundant: Vec<String>,
}

impl SolveReport {
    pub fn ok(&self) -> bool {
        self.status == SolveStatus::Solved
    }
    pub fn fully_constrained(&self) -> bool {
        self.dof == 0
    }
    /// Solved, and no orientation of the stored state flipped: what a value edit accepts.
    pub fn keeps_orientation(&self) -> bool {
        self.ok() && self.folded.is_empty()
    }
    /// Why a solve failed or turned over: the constraints whose orientation the edit flips,
    /// or else `conflict` of the constraints still violated (listed, comma-separated).
    pub fn failure(&self, conflict: impl FnOnce(&str) -> String) -> String {
        if self.folded.is_empty() {
            conflict(&self.failing.join(", "))
        } else {
            format!("the new values would flip the sketch over ({} would change orientation)", self.folded.join(", "))
        }
    }
}

/// Variable layout: point → (x var, y var), circle → radius var. The point coordinates come
/// first, in pairs (variables `0..points`), then the radii.
struct Layout {
    pvar: Vec<Option<usize>>,
    rvar: Vec<Option<usize>>,
    n: usize,
    /// The point coordinates' variables: `0..points`.
    points: usize,
    /// Per variable: a coordinate of a point held on a curve (point on curve, midpoint), which
    /// a solve slides along it before moving the curve (see [`SLIDE_WEIGHT`]).
    slide: Vec<bool>,
}

impl Layout {
    /// The variable is a point coordinate.
    fn is_point(&self, v: usize) -> bool {
        v < self.points
    }
    /// The first variable of the unknown `v` belongs to: its point's x, or the radius itself.
    fn unit(&self, v: usize) -> usize {
        if self.is_point(v) { v & !1 } else { v }
    }
    /// The variables of the unknown starting at `first`.
    fn width(&self, first: usize) -> usize {
        if self.is_point(first) { 2 } else { 1 }
    }
}

/// Geometry held in place for one solve: point indices and curves whose radius is held.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hold {
    pub points: Vec<usize>,
    pub radii: Vec<usize>,
}

/// Per point: a constant for this solve (locked — fixed, fixed by a constraint, a point of a
/// fixed curve, linked — or held).
pub(crate) fn held_points(sk: &Sketch, hold: &Hold) -> Vec<bool> {
    let mut fixed = sk.locked_points();
    for p in &hold.points {
        if let Some(f) = fixed.get_mut(*p) {
            *f = true;
        }
    }
    fixed
}

fn layout(sk: &Sketch, hold: &Hold) -> Layout {
    let mut n = 0;
    let pvar: Vec<Option<usize>> = held_points(sk, hold)
        .iter()
        .map(|f| {
            if *f {
                None
            } else {
                n += 2;
                Some(n - 2)
            }
        })
        .collect();
    let points = n;
    let rvar = sk
        .curves
        .iter()
        .enumerate()
        .map(|(i, c)| match c.kind {
            CurveKind::Circle { .. } | CurveKind::Ellipse { .. } if c.link.is_none() && !c.fixed && !hold.radii.contains(&i) => {
                n += 1;
                Some(n - 1)
            }
            _ => None,
        })
        .collect();
    let mut slide = vec![false; n];
    for c in &sk.constraints {
        if let ConstraintKind::PointOnCurve { p, .. } | ConstraintKind::Midpoint { p, .. } = c.kind
            && let Some(Some(v)) = pvar.get(p)
        {
            for k in [*v, *v + 1] {
                if let Some(s) = slide.get_mut(k) {
                    *s = true;
                }
            }
        }
    }
    Layout { pvar, rvar, n, points, slide }
}

/// Read-only view of the sketch with variable values substituted.
struct State<'a> {
    sk: &'a Sketch,
    lay: &'a Layout,
    x: &'a [f64],
}

impl State<'_> {
    fn p(&self, i: usize) -> Vec2 {
        match self.lay.pvar.get(i).copied().flatten() {
            Some(v) => Vec2::new(self.x.get(v).copied().unwrap_or(0.0), self.x.get(v + 1).copied().unwrap_or(0.0)),
            None => self.sk.points.get(i).map(|p| p.pos).unwrap_or_default(),
        }
    }
    fn line(&self, l: usize) -> (Vec2, Vec2) {
        match self.sk.curves.get(l).map(|c| &c.kind) {
            Some(CurveKind::Line { a, b }) => (self.p(*a), self.p(*b)),
            _ => (Vec2::ZERO, Vec2::X),
        }
    }
    /// Centre and radius of a circle or arc.
    fn round(&self, c: usize) -> (Vec2, f64) {
        match self.sk.curves.get(c).map(|c| &c.kind) {
            Some(CurveKind::Circle { c: ci, r }) => {
                let r = match self.lay.rvar.get(c).copied().flatten() {
                    Some(v) => self.x.get(v).copied().unwrap_or(*r),
                    None => *r,
                };
                (self.p(*ci), r)
            }
            Some(CurveKind::Arc { c: ci, a, .. }) => {
                let cc = self.p(*ci);
                (cc, cc.dist(self.p(*a)))
            }
            _ => (Vec2::ZERO, 0.0),
        }
    }
    fn is_line(&self, c: usize) -> bool {
        matches!(self.sk.curves.get(c).map(|c| &c.kind), Some(CurveKind::Line { .. }))
    }
    fn is_freeform(&self, c: usize) -> bool {
        self.sk.curves.get(c).is_some_and(|c| c.kind.is_freeform())
    }
    /// Polyline of a free-form curve with trial values.
    fn poly(&self, c: usize) -> Vec<Vec2> {
        match self.sk.curves.get(c).map(|c| &c.kind) {
            Some(CurveKind::Ellipse { c: ci, m, r }) => {
                let r = match self.lay.rvar.get(c).copied().flatten() {
                    Some(v) => self.x.get(v).copied().unwrap_or(*r),
                    None => *r,
                };
                crate::curves::ellipse_polyline(self.p(*ci), self.p(*m), r)
            }
            Some(CurveKind::Spline { pts, control, degree, periodic }) => {
                let p = self.spline_points(pts, *periodic);
                crate::curves::spline_polyline_mode(&p, *control, *degree, *periodic)
            }
            Some(CurveKind::Conic { a, b, apex, rho }) => crate::curves::conic_polyline(self.p(*a), self.p(*apex), self.p(*b), *rho),
            _ => Vec::new(),
        }
    }
    /// Point-on-curve constraints use the exact shape. A polyline residual would move an
    /// exact crossing onto its chord, changing geometry merely by splitting it.
    fn spline_points(&self, pts: &[usize], periodic: bool) -> Vec<Vec2> {
        let mut points: Vec<Vec2> = pts.iter().map(|i| self.p(*i)).collect();
        if periodic
            && let Some(first) = points.first().copied()
            && let Some(last) = points.last_mut()
        {
            *last = first;
        }
        points
    }
    fn freeform_dist(&self, c: usize, q: Vec2) -> f64 {
        let segs: Vec<Seg2> = match self.sk.curves.get(c).map(|c| &c.kind) {
            Some(CurveKind::Ellipse { c: ci, m, .. }) => {
                let center = self.p(*ci);
                let u = self.p(*m) - center;
                let a = u.len();
                let r = self.radius_of(c);
                if a <= 1e-12 || r <= 1e-12 {
                    return q.dist(center);
                }
                let delta = q - center;
                let x = delta.dot(u) / (a * a);
                let y = delta.cross(u) / (a * r);
                return (x.hypot(y) - 1.0) * a.min(r);
            }
            Some(CurveKind::Spline { pts, control, degree, periodic }) => {
                let p = self.spline_points(pts, *periodic);
                if p.len() < 2 {
                    return q.dist(p.first().copied().unwrap_or_default());
                }
                if *control {
                    let k = (*degree as usize).min(solidcraft_geom::MAX_BEZIER_DEGREE).min(p.len().saturating_sub(1)).max(1);
                    crate::curves::bspline_beziers(&p, k).iter().filter_map(|span| solidcraft_geom::bezier(span)).collect()
                } else {
                    crate::curves::fit_beziers_mode(&p, *periodic)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|[p0, p1, p2, p3]| Seg2::Cubic { p0, p1, p2, p3 })
                        .collect()
                }
            }
            Some(CurveKind::Conic { a, b, apex, rho }) => {
                vec![Seg2::Conic { a: self.p(*a), b: self.p(*b), apex: self.p(*apex), w: rho / (1.0 - rho).max(1e-9) }]
            }
            _ => Vec::new(),
        };
        segs.iter()
            .map(|seg| {
                let t = seg.closest_param(q);
                let delta = q - seg.point_at(t);
                let d = delta.len();
                (d, if seg.derivative(t).cross(delta) >= 0.0 { d } else { -d })
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, signed)| signed)
            .unwrap_or_else(|| crate::curves::polyline_signed_dist(&self.poly(c), q))
    }
    /// Radius of a circle or arc; an ellipse's minor radius.
    fn radius_of(&self, c: usize) -> f64 {
        match self.sk.curves.get(c).map(|x| &x.kind) {
            Some(CurveKind::Ellipse { r, .. }) => match self.lay.rvar.get(c).copied().flatten() {
                Some(v) => self.x.get(v).copied().unwrap_or(*r),
                None => *r,
            },
            _ => self.round(c).1,
        }
    }
    fn ends(&self, c: usize) -> Option<(usize, usize)> {
        self.sk.curves.get(c)?.kind.ends()
    }
    /// Length along an arc (counter-clockwise from its start).
    fn arc_length(&self, c: usize) -> f64 {
        match self.sk.curves.get(c).map(|c| &c.kind) {
            Some(CurveKind::Arc { c: ci, a, b }) => {
                let (cc, pa, pb) = (self.p(*ci), self.p(*a), self.p(*b));
                let mut sw = (pb - cc).angle() - (pa - cc).angle();
                while sw <= 1e-12 {
                    sw += std::f64::consts::TAU;
                }
                cc.dist(pa) * sw
            }
            _ => 0.0,
        }
    }
    fn arc_sweep(&self, c: usize) -> f64 {
        let Some(curve) = self.sk.curves.get(c) else { return 0.0 };
        let CurveKind::Arc { c: cp, a, b } = curve.kind else { return 0.0 };
        let sweep = ((self.p(b) - self.p(cp)).angle() - (self.p(a) - self.p(cp)).angle()).rem_euclid(std::f64::consts::TAU);
        if curve.reversed { -sweep } else { sweep }
    }
    fn slot_length(&self, a: usize, b: usize) -> f64 {
        let (ca, ra) = self.round(a);
        let (cb, rb) = self.round(b);
        ca.dist(cb) + ra + rb
    }
    /// Curvature vector of a curve at its point `p` (towards the centre of curvature).
    fn curvature_at(&self, c: usize, p: usize) -> Vec2 {
        let Some(kind) = self.sk.curves.get(c).map(|c| &c.kind) else { return Vec2::ZERO };
        match kind {
            CurveKind::Line { .. } | CurveKind::Ellipse { .. } => Vec2::ZERO,
            CurveKind::Circle { .. } | CurveKind::Arc { .. } => {
                let (cc, r) = self.round(c);
                (cc - self.p(p)) / (r * r).max(1e-24)
            }
            CurveKind::Spline { pts, control, degree, periodic } => {
                let q = self.spline_points(pts, *periodic);
                let at_end = pts.last() == Some(&p) && pts.first() != Some(&p);
                crate::curves::end_curvature(&|t| crate::curves::spline_point_mode(&q, *control, *degree, *periodic, t), at_end)
            }
            CurveKind::Conic { a, b, apex, rho } => {
                let (pa, pb, px) = (self.p(*a), self.p(*b), self.p(*apex));
                crate::curves::end_curvature(&|t| crate::curves::conic_point(pa, px, pb, *rho, t), p == *b)
            }
        }
    }
    /// Unit tangent of any curve at its point `p` (an end point for open free-form curves).
    fn tangent_at(&self, c: usize, p: usize) -> Option<Vec2> {
        match self.sk.curves.get(c).map(|c| &c.kind)? {
            CurveKind::Line { a, b } => (self.p(*b) - self.p(*a)).normalized(),
            CurveKind::Circle { .. } | CurveKind::Arc { .. } => (self.p(p) - self.round(c).0).perp().normalized(),
            CurveKind::Spline { pts, control, periodic, .. } => {
                let q = self.spline_points(pts, *periodic);
                crate::curves::spline_end_tangent_mode(&q, *control, *periodic, pts.last() == Some(&p) && pts.first() != Some(&p))
            }
            CurveKind::Conic { a, b, apex, .. } => {
                if p == *a {
                    (self.p(*apex) - self.p(*a)).normalized()
                } else if p == *b {
                    (self.p(*b) - self.p(*apex)).normalized()
                } else {
                    None
                }
            }
            CurveKind::Ellipse { .. } => {
                let poly = self.poly(c);
                let q = self.p(p);
                let k = (0..poly.len().saturating_sub(1)).min_by(|i, j| poly[*i].dist(q).total_cmp(&poly[*j].dist(q)))?;
                (*poly.get(k + 1)? - *poly.get(k)?).normalized()
            }
        }
    }
}

fn wrap_angle(a: f64) -> f64 {
    let t = std::f64::consts::TAU;
    let mut a = a % t;
    if a > std::f64::consts::PI {
        a -= t;
    } else if a <= -std::f64::consts::PI {
        a += t;
    }
    a
}

/// Distance from `p` to the infinite line through `a`, `b` (signed by side).
fn line_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l = d.len().max(1e-12);
    d.cross(p - a) / l
}

/// Residual rows of one constraint.
fn residuals(s: &State, k: &ConstraintKind, out: &mut Vec<f64>) {
    use ConstraintKind::*;
    match *k {
        PatternPoint { p, q, center, angle, mut offset, directions } => {
            for (l, distance) in directions.into_iter().flatten() {
                let (a, b) = s.line(l);
                let u = (b - a).normalized().unwrap_or(Vec2::ZERO);
                offset += u * distance.x + u.perp() * distance.y;
            }
            let c = center.map(|c| s.p(c)).unwrap_or(Vec2::ZERO);
            let d = s.p(p) - c;
            let (sin, cos) = angle.sin_cos();
            let delta = s.p(q) - c - Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos) - offset;
            out.extend([delta.x, delta.y]);
        }
        PatternRadius { a, b } => out.push(s.radius_of(a) - s.radius_of(b)),
        // Offset distances drive derived geometry in refresh_offsets, never source DOFs.
        OffsetDistance { .. } => {}
        Coincident { p, q } => {
            let d = s.p(p) - s.p(q);
            out.extend([d.x, d.y]);
        }
        PointOnCurve { p, c } => {
            let pp = s.p(p);
            if s.is_freeform(c) {
                out.push(s.freeform_dist(c, pp));
            } else if s.is_line(c) {
                let (a, b) = s.line(c);
                out.push(line_dist(pp, a, b));
            } else {
                let (cc, r) = s.round(c);
                out.push(pp.dist(cc) - r);
            }
        }
        Horizontal { l } => {
            let (a, b) = s.line(l);
            out.push(b.y - a.y);
        }
        Vertical { l } => {
            let (a, b) = s.line(l);
            out.push(b.x - a.x);
        }
        HorizontalPoints { p, q } => out.push(s.p(q).y - s.p(p).y),
        VerticalPoints { p, q } => out.push(s.p(q).x - s.p(p).x),
        Parallel { a, b } | Perpendicular { a, b } => {
            let (a0, a1) = s.line(a);
            let (b0, b1) = s.line(b);
            let (da, db) = (a1 - a0, b1 - b0);
            // The sine (parallel) or cosine (perpendicular) of the angle between the lines:
            // independent of their lengths, so the solver can't satisfy it by shrinking a line
            // to nothing (scaling by the summed lengths did, collapsing dragged rectangles).
            let scale = (da.len() * db.len()).max(1e-12);
            let v = if matches!(k, Parallel { .. }) { da.cross(db) } else { da.dot(db) };
            out.push(v / scale);
        }
        Collinear { a, b } => {
            let (a0, a1) = s.line(a);
            let (b0, b1) = s.line(b);
            out.push(line_dist(b0, a0, a1));
            out.push(line_dist(b1, a0, a1));
        }
        Tangent { a, b } => {
            let (la, lb) = (s.is_line(a), s.is_line(b));
            if s.is_freeform(a) || s.is_freeform(b) {
                // Without a shared end point: the curves touch (closest approach zero).
                let (f, o) = if s.is_freeform(a) { (a, b) } else { (b, a) };
                let poly = s.poly(f);
                // Signed gap at the closest approach (smooth through zero, unlike its size).
                let closest = |v: &mut dyn Iterator<Item = f64>| v.fold(f64::INFINITY, |m: f64, x| if x.abs() < m.abs() { x } else { m });
                let d = if s.is_line(o) {
                    let (p0, p1) = s.line(o);
                    closest(&mut poly.iter().map(|q| line_dist(*q, p0, p1)))
                } else if s.is_freeform(o) {
                    let other = s.poly(o);
                    closest(&mut poly.iter().map(|q| crate::curves::polyline_signed_dist(&other, *q)))
                } else {
                    let (cc, r) = s.round(o);
                    closest(&mut poly.iter().map(|q| q.dist(cc) - r))
                };
                out.push(if d.is_finite() { d } else { 0.0 });
            } else if la || lb {
                let (l, c) = if la { (a, b) } else { (b, a) };
                let (p0, p1) = s.line(l);
                let (cc, r) = s.round(c);
                out.push(line_dist(cc, p0, p1).abs() - r);
            } else {
                let (c1, r1) = s.round(a);
                let (c2, r2) = s.round(b);
                let d = c1.dist(c2);
                let ext = d - (r1 + r2);
                let int = d - (r1 - r2).abs();
                out.push(if ext.abs() <= int.abs() { ext } else { int });
            }
        }
        Equal { a, b } => {
            if s.is_line(a) {
                let (a0, a1) = s.line(a);
                let (b0, b1) = s.line(b);
                out.push(a0.dist(a1) - b0.dist(b1));
            } else {
                out.push(s.round(a).1 - s.round(b).1);
            }
        }
        Concentric { a, b } => {
            let d = s.round(a).0 - s.round(b).0;
            out.extend([d.x, d.y]);
        }
        Midpoint { p, l } => {
            let (a, b) = s.line(l);
            let d = s.p(p) - (a + b) * 0.5;
            out.extend([d.x, d.y]);
        }
        Symmetric { p, q, l } => {
            let (a, b) = s.line(l);
            let (pp, qq) = (s.p(p), s.p(q));
            let dir = b - a;
            let len = dir.len().max(1e-12);
            out.push(line_dist((pp + qq) * 0.5, a, b));
            out.push((qq - pp).dot(dir) / len);
        }
        Fix { .. } => {}
        Distance { p, q, value } => out.push(s.p(p).dist(s.p(q)) - value),
        DistanceX { p, q, value } => out.push((s.p(q).x - s.p(p).x).abs() - value),
        DistanceY { p, q, value } => out.push((s.p(q).y - s.p(p).y).abs() - value),
        PointLineDistance { p, l, value } => {
            let (a, b) = s.line(l);
            out.push(line_dist(s.p(p), a, b).abs() - value);
        }
        Length { l, value } => {
            let (a, b) = s.line(l);
            out.push(a.dist(b) - value);
        }
        LineAngle { l, value } => {
            let (a, b) = s.line(l);
            // A dimension with no typed length must not satisfy its angle by collapsing
            // the line. Scale with the original geometry, independently of the unknowns.
            let scale = match s.sk.curves.get(l).map(|c| &c.kind) {
                Some(CurveKind::Line { a, b }) => s.sk.point(*a).zip(s.sk.point(*b)).map(|(a, b)| a.dist(b)).unwrap_or(1.0),
                _ => 1.0,
            };
            out.push(wrap_angle((b - a).angle() - value) * scale.max(1.0));
        }
        ArcSweep { c, value } => out.push((s.arc_sweep(c) - value) * s.sk.radius(c).unwrap_or(1.0).max(1.0)),
        SlotOverallLength { a, b, value } => out.push(s.slot_length(a, b) - value),
        Radius { c, value } => out.push(s.radius_of(c) - value),
        Diameter { c, value } => out.push(2.0 * s.round(c).1 - value),
        Angle { a, b, value, flip } => {
            let (a0, a1) = s.line(a);
            let (b0, b1) = s.line(b);
            let (da, db) = (a1 - a0, b1 - b0);
            let ang = da.cross(db).atan2(da.dot(db));
            let scale = 0.5 * (da.len() + db.len());
            let target = if flip { value + std::f64::consts::PI } else { value };
            out.push(wrap_angle(ang - target) * scale.max(1e-6));
        }
        Smooth { a, b } => {
            let shared = match (s.ends(a), s.ends(b)) {
                (Some((a0, a1)), Some((b0, b1))) => [a0, a1].into_iter().find(|x| *x == b0 || *x == b1),
                _ => None,
            };
            let Some(p) = shared else {
                out.extend([0.0, 0.0, 0.0]);
                return;
            };
            match (s.tangent_at(a, p), s.tangent_at(b, p)) {
                (Some(u), Some(v)) => out.push(u.cross(v) * 10.0),
                _ => out.push(0.0),
            }
            let k = s.curvature_at(a, p) - s.curvature_at(b, p);
            out.extend([k.x * 100.0, k.y * 100.0]);
        }
        ArcLength { c, value } => out.push(s.arc_length(c) - value),
        LinearDiameter { p, l, value } => {
            let (a, b) = s.line(l);
            out.push(2.0 * line_dist(s.p(p), a, b).abs() - value);
        }
    }
}

/// Measured value of a dimension (for driven dimensions).
fn measure(s: &State, k: &ConstraintKind) -> Option<f64> {
    use ConstraintKind::*;
    Some(match *k {
        Distance { p, q, .. } => s.p(p).dist(s.p(q)),
        DistanceX { p, q, .. } => (s.p(q).x - s.p(p).x).abs(),
        DistanceY { p, q, .. } => (s.p(q).y - s.p(p).y).abs(),
        PointLineDistance { p, l, .. } => {
            let (a, b) = s.line(l);
            line_dist(s.p(p), a, b).abs()
        }
        Length { l, .. } => {
            let (a, b) = s.line(l);
            a.dist(b)
        }
        LineAngle { l, value } => {
            let (a, b) = s.line(l);
            value + wrap_angle((b - a).angle() - value)
        }
        ArcSweep { c, .. } => s.arc_sweep(c),
        SlotOverallLength { a, b, .. } => s.slot_length(a, b),
        Radius { c, .. } => s.radius_of(c),
        Diameter { c, .. } => 2.0 * s.round(c).1,
        Angle { a, b, flip, .. } => {
            let (a0, a1) = s.line(a);
            let (b0, b1) = s.line(b);
            let (da, db) = (a1 - a0, b1 - b0);
            let ang = da.cross(db).atan2(da.dot(db)) - if flip { std::f64::consts::PI } else { 0.0 };
            ang.rem_euclid(std::f64::consts::TAU)
        }
        ArcLength { c, .. } => s.arc_length(c),
        LinearDiameter { p, l, .. } => {
            let (a, b) = s.line(l);
            2.0 * line_dist(s.p(p), a, b).abs()
        }
        _ => return None,
    })
}

/// What a dimension measures on the sketch as it stands (mm, radians for angles); `None` for a
/// constraint that is not a dimension.
pub fn measure_dimension(sk: &Sketch, k: &ConstraintKind) -> Option<f64> {
    let lay = Layout { pvar: vec![None; sk.points.len()], rvar: vec![None; sk.curves.len()], n: 0, points: 0, slide: Vec::new() };
    measure(&State { sk, lay: &lay, x: &[] }, k).filter(|v| v.is_finite())
}

/// One residual block: implicit arc radius equality, a user constraint, or a tangency at a known
/// touch point (better conditioned than the distance form when an end point is on the curve).
enum Block<'a> {
    Closure { a: usize, b: usize },
    Arc { c: usize, a: usize, b: usize },
    User(usize, &'a ConstraintKind),
    TangentAt(usize, &'a ConstraintKind, usize),
}

fn copy_block<'a>(b: &Block<'a>) -> Block<'a> {
    match b {
        Block::Closure { a, b } => Block::Closure { a: *a, b: *b },
        Block::Arc { c, a, b } => Block::Arc { c: *c, a: *a, b: *b },
        Block::User(i, k) => Block::User(*i, k),
        Block::TangentAt(i, k, p) => Block::TangentAt(*i, k, *p),
    }
}

fn block_residuals(s: &State, b: &Block, out: &mut Vec<f64>) {
    match b {
        Block::Closure { a, b } => {
            let d = s.p(*a) - s.p(*b);
            out.extend([d.x, d.y]);
        }
        Block::Arc { c, a, b } => {
            let cc = s.p(*c);
            out.push(cc.dist(s.p(*b)) - cc.dist(s.p(*a)));
        }
        Block::User(_, k) => residuals(s, k, out),
        Block::TangentAt(_, k, p) => {
            let ConstraintKind::Tangent { a, b } = **k else { return };
            let pt = s.p(*p);
            if s.is_freeform(a) || s.is_freeform(b) {
                // Directions at the shared end parallel.
                match (s.tangent_at(a, *p), s.tangent_at(b, *p)) {
                    (Some(u), Some(v)) => out.push(u.cross(v) * 10.0),
                    _ => out.push(0.0),
                }
            } else if s.is_line(a) || s.is_line(b) {
                let (l, c) = if s.is_line(a) { (a, b) } else { (b, a) };
                let (p0, p1) = s.line(l);
                let (cc, _) = s.round(c);
                let d = p1 - p0;
                // The radius to the touch point is perpendicular to the line.
                out.push((pt - cc).dot(d) / d.len().max(1e-12));
            } else {
                let (c1, _) = s.round(a);
                let (c2, _) = s.round(b);
                let (u, v) = (pt - c1, pt - c2);
                // Both centres on one line through the touch point.
                out.push(u.cross(v) / (u.len() + v.len()).max(1e-12));
            }
        }
    }
}

/// Every (point, curve) pair of a point-on-curve constraint, sorted (for [`touch_point`]).
pub(crate) fn points_on_curves(sk: &Sketch) -> Vec<(usize, usize)> {
    let mut on: Vec<(usize, usize)> = sk
        .constraints
        .iter()
        .filter_map(|c| match c.kind {
            ConstraintKind::PointOnCurve { p, c } => Some((p, c)),
            _ => None,
        })
        .collect();
    on.sort_unstable();
    on
}

/// A point where a tangency must happen: an end point shared by the two curves, or a line end
/// point constrained onto the round curve (`on_curve` from [`points_on_curves`]).
pub(crate) fn touch_point(sk: &Sketch, k: &ConstraintKind, on_curve: &[(usize, usize)]) -> Option<usize> {
    let ConstraintKind::Tangent { a, b } = *k else { return None };
    let ends = |c: usize| -> Vec<usize> {
        match sk.curves.get(c).map(|c| &c.kind) {
            Some(k) => k.ends().map(|(a, b)| vec![a, b]).unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let (ea, eb) = (ends(a), ends(b));
    if let Some(p) = ea.iter().find(|p| eb.contains(p)) {
        return Some(*p);
    }
    let on = |p: usize, c: usize| on_curve.binary_search(&(p, c)).is_ok();
    ea.iter().find(|p| on(**p, b)).or_else(|| eb.iter().find(|p| on(**p, a))).copied()
}

/// Variables a block depends on, and whether it reads a point that is a constant of the solve.
fn block_vars(sk: &Sketch, lay: &Layout, b: &Block) -> (Vec<usize>, bool) {
    let mut v = Vec::new();
    let mut constant = false;
    // Adds a point's variables; true when it is a constant.
    let pt = |v: &mut Vec<usize>, p: usize| match lay.pvar.get(p).copied().flatten() {
        Some(x) => {
            v.extend([x, x + 1]);
            false
        }
        None => true,
    };
    for r in block_refs(sk, b) {
        let (c, fits): (usize, fn(&CurveKind) -> bool) = match r {
            Ref::Point(p) => {
                constant |= pt(&mut v, p);
                continue;
            }
            Ref::Curve(c) => (c, |_| true),
            Ref::Line(c) => (c, |k| matches!(k, CurveKind::Line { .. })),
            Ref::Round(c) => (c, |k| matches!(k, CurveKind::Circle { .. } | CurveKind::Arc { .. })),
        };
        // A curve of another kind reads as a constant (see `State::line`, `State::round`).
        let Some(k) = sk.curves.get(c).map(|c| &c.kind).filter(|k| fits(k)) else {
            constant = true;
            continue;
        };
        for q in k.point_ids() {
            constant |= pt(&mut v, q);
        }
        v.extend(lay.rvar.get(c).copied().flatten());
    }
    v.sort_unstable();
    v.dedup();
    (v, constant)
}

/// What a block reads: a point, or a curve (its points and its radius), of any kind or read
/// as a line or as a circle or arc (the residual reads another kind as a constant).
#[derive(Clone, Copy)]
enum Ref {
    Point(usize),
    Curve(usize),
    Line(usize),
    Round(usize),
}

/// What a block reads.
fn block_refs(sk: &Sketch, b: &Block) -> Vec<Ref> {
    use ConstraintKind::*;
    use Ref::{Curve as C, Line as L, Point as P, Round as R};
    let is = |c: usize, f: fn(&CurveKind) -> bool| sk.curves.get(c).is_some_and(|x| f(&x.kind));
    let line = |c: usize| is(c, |k| matches!(k, CurveKind::Line { .. }));
    let freeform = |c: usize| is(c, CurveKind::is_freeform);
    // A tangency reads a free-form curve as such, else a line and a round, or two rounds.
    let tangent = |a: usize, b: usize| {
        if freeform(a) || freeform(b) {
            [C(a), C(b)]
        } else if line(a) {
            [L(a), R(b)]
        } else if line(b) {
            [R(a), L(b)]
        } else {
            [R(a), R(b)]
        }
    };
    match b {
        Block::Closure { a, b } => vec![P(*a), P(*b)],
        Block::Arc { c, a, b } => vec![P(*c), P(*a), P(*b)],
        Block::User(_, k) => match **k {
            PatternPoint { p, q, center, directions, .. } => {
                vec![P(p), P(q)].into_iter().chain(center.map(P)).chain(directions.into_iter().flatten().map(|(l, _)| L(l))).collect()
            }
            PatternRadius { a, b } => vec![C(a), C(b)],
            Coincident { p, q }
            | HorizontalPoints { p, q }
            | VerticalPoints { p, q }
            | Distance { p, q, .. }
            | DistanceX { p, q, .. }
            | DistanceY { p, q, .. } => vec![P(p), P(q)],
            PointOnCurve { p, c } => vec![P(p), C(c)],
            Midpoint { p, l } | PointLineDistance { p, l, .. } | LinearDiameter { p, l, .. } => vec![P(p), L(l)],
            Horizontal { l } | Vertical { l } | Length { l, .. } | LineAngle { l, .. } => vec![L(l)],
            // Another kind reads a constant value, whatever its points: as good as any.
            ArcSweep { c, .. } | Radius { c, .. } | Diameter { c, .. } | ArcLength { c, .. } => vec![C(c)],
            Parallel { a, b } | Perpendicular { a, b } | Collinear { a, b } | Angle { a, b, .. } => vec![L(a), L(b)],
            Equal { a, b } if line(a) => vec![L(a), L(b)],
            Equal { a, b } | Concentric { a, b } | SlotOverallLength { a, b, .. } => vec![R(a), R(b)],
            Tangent { a, b } => tangent(a, b).to_vec(),
            Smooth { a, b } => vec![C(a), C(b)],
            Symmetric { p, q, l } => vec![P(p), P(q), L(l)],
            Fix { .. } | OffsetDistance { .. } => vec![],
        },
        Block::TangentAt(_, k, p) => match **k {
            ConstraintKind::Tangent { a, b } if freeform(a) || freeform(b) => vec![P(*p), C(a), C(b)],
            ConstraintKind::Tangent { a, b } => [P(*p)].into_iter().chain(tangent(a, b)).collect(),
            _ => vec![P(*p)],
        },
    }
}

/// Residual vector and Jacobian (rows × all n vars) for the given blocks.
fn system(sk: &Sketch, lay: &Layout, blocks: &[(Block, Vec<usize>)], x: &mut [f64], with_j: bool) -> (Vec<f64>, Option<Mat>, Vec<usize>) {
    let mut r = Vec::new();
    let mut owner = Vec::new();
    for (bi, (b, _)) in blocks.iter().enumerate() {
        let before = r.len();
        block_residuals(&State { sk, lay, x }, b, &mut r);
        owner.extend(std::iter::repeat_n(bi, r.len() - before));
    }
    if !with_j {
        return (r, None, owner);
    }
    let mut j = Mat::zeros(r.len(), lay.n);
    let mut row = 0;
    let mut plus = Vec::new();
    let mut minus = Vec::new();
    for (b, vars) in blocks {
        plus.clear();
        block_residuals(&State { sk, lay, x }, b, &mut plus);
        let rows = plus.len();
        for &v in vars {
            let Some(x0) = x.get(v).copied() else { continue };
            let h = 1e-7 * x0.abs().max(1.0);
            if let Some(xv) = x.get_mut(v) {
                *xv = x0 + h;
            }
            plus.clear();
            block_residuals(&State { sk, lay, x }, b, &mut plus);
            if let Some(xv) = x.get_mut(v) {
                *xv = x0 - h;
            }
            minus.clear();
            block_residuals(&State { sk, lay, x }, b, &mut minus);
            if let Some(xv) = x.get_mut(v) {
                *xv = x0;
            }
            for k in 0..rows {
                let d = (plus.get(k).copied().unwrap_or(0.0) - minus.get(k).copied().unwrap_or(0.0)) / (2.0 * h);
                j.set(row + k, v, d);
            }
        }
        row += rows;
    }
    (r, Some(j), owner)
}

/// Residuals and the Jacobian restricted to `vars` (columns in that order; `col` maps a global
/// variable to its column, usize::MAX elsewhere). Blocks only touch their component's variables,
/// so a component's Jacobian is small even in a huge sketch.
fn local_system(
    sk: &Sketch,
    lay: &Layout,
    blocks: &[(Block, Vec<usize>)],
    x: &mut [f64],
    col: &[usize],
    ncols: usize,
) -> (Vec<f64>, Mat, Vec<usize>) {
    let (r, _, owner) = system(sk, lay, blocks, x, false);
    let mut j = Mat::zeros(r.len(), ncols);
    let mut row = 0;
    let mut plus = Vec::new();
    let mut minus = Vec::new();
    for (b, vars) in blocks {
        plus.clear();
        block_residuals(&State { sk, lay, x }, b, &mut plus);
        let rows = plus.len();
        for &v in vars {
            let Some(c) = col.get(v).copied().filter(|c| *c < ncols) else { continue };
            let Some(x0) = x.get(v).copied() else { continue };
            let h = 1e-7 * x0.abs().max(1.0);
            if let Some(xv) = x.get_mut(v) {
                *xv = x0 + h;
            }
            plus.clear();
            block_residuals(&State { sk, lay, x }, b, &mut plus);
            if let Some(xv) = x.get_mut(v) {
                *xv = x0 - h;
            }
            minus.clear();
            block_residuals(&State { sk, lay, x }, b, &mut minus);
            if let Some(xv) = x.get_mut(v) {
                *xv = x0;
            }
            for k in 0..rows {
                let d = (plus.get(k).copied().unwrap_or(0.0) - minus.get(k).copied().unwrap_or(0.0)) / (2.0 * h);
                j.set(row + k, c, d);
            }
        }
        row += rows;
    }
    (r, j, owner)
}

fn max_abs(v: &[f64]) -> f64 {
    v.iter().fold(0.0_f64, |m, x| if x.is_finite() { m.max(x.abs()) } else { f64::INFINITY })
}
fn norm2(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum()
}

/// Levenberg–Marquardt on one component (variables `vars`, blocks `blocks`). Returns the
/// iterations and the largest residual left.
fn lm(sk: &Sketch, lay: &Layout, blocks: &[(Block, Vec<usize>)], vars: &[usize], x: &mut Vec<f64>, col: &[usize]) -> (usize, f64) {
    lm_capped(sk, lay, blocks, vars, x, col, MAX_ITERS)
}

/// [`lm`] with at most `max_iters` iterations.
fn lm_capped(
    sk: &Sketch,
    lay: &Layout,
    blocks: &[(Block, Vec<usize>)],
    vars: &[usize],
    x: &mut Vec<f64>,
    col: &[usize],
    max_iters: usize,
) -> (usize, f64) {
    let n = vars.len();
    #[cfg(test)]
    lm_vars::add(n);
    let mut lambda = 1e-3;
    let (mut r, _, _) = system(sk, lay, blocks, x, false);
    let mut f = norm2(&r);
    for it in 0..max_iters {
        if max_abs(&r) < TOL {
            return (it, max_abs(&r));
        }
        let (_, j, _) = local_system(sk, lay, blocks, x, col, n);
        // Normal equations of this component.
        let mut a = Mat::zeros(n, n);
        let mut g = vec![0.0; n];
        for row in 0..j.rows {
            let ri = r.get(row).copied().unwrap_or(0.0);
            let nz: Vec<(usize, f64)> = (0..n).map(|li| (li, j.get(row, li))).filter(|(_, d)| *d != 0.0).collect();
            for &(li, di) in &nz {
                if let Some(gi) = g.get_mut(li) {
                    *gi += di * ri;
                }
                for &(lk, dk) in &nz {
                    a.add(li, lk, di * dk);
                }
            }
        }
        let mut improved = false;
        // Damping by distance moved, not by JᵀJ's diagonal (Marquardt): of the steps that fit the
        // residuals equally well it takes the shortest, so what the constraints leave free moves
        // as little as it can. (Scaled by the diagonal, a variable the residuals see weakly is
        // cheap to move: a point held on a line made the line's far end swing round instead of
        // the point sliding along it.) A point held on a curve is cheaper still, so it slides
        // rather than the curve's ends moving.
        let weight: Vec<f64> = vars.iter().map(|&v| if lay.slide.get(v).copied().unwrap_or(false) { SLIDE_WEIGHT } else { 1.0 }).collect();
        for _ in 0..30 {
            let mut m = a.clone();
            for (i, w) in weight.iter().enumerate() {
                m.add(i, i, lambda * w + 1e-12);
            }
            let rhs: Vec<f64> = g.iter().map(|v| -v).collect();
            let Some(step) = lin_solve(m, rhs) else {
                lambda *= 10.0;
                continue;
            };
            let mut trial = x.clone();
            for (li, &v) in vars.iter().enumerate() {
                if let (Some(t), Some(s)) = (trial.get_mut(v), step.get(li)) {
                    *t += s;
                }
            }
            let (rt, _, _) = system(sk, lay, blocks, &mut trial, false);
            let ft = norm2(&rt);
            if ft.is_finite() && ft < f {
                *x = trial;
                r = rt;
                f = ft;
                lambda = (lambda / 3.0).max(1e-12);
                improved = true;
                break;
            }
            lambda *= 4.0;
            if lambda > 1e12 {
                break;
            }
        }
        if !improved {
            return (it, max_abs(&r));
        }
    }
    (max_iters, max_abs(&r))
}

/// The variables LM was given and its runs on this thread, for tests that count what the
/// construction leaves to it.
#[cfg(test)]
pub(crate) mod lm_vars {
    use std::cell::Cell;

    thread_local! {
        static VARS: Cell<usize> = const { Cell::new(0) };
        static RUNS: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn add(n: usize) {
        VARS.with(|c| c.set(c.get() + n));
        RUNS.with(|c| c.set(c.get() + 1));
    }

    /// The variables since the last call.
    pub(crate) fn take() -> usize {
        VARS.with(|c| c.replace(0))
    }

    /// The LM runs since the last call.
    pub(crate) fn runs() -> usize {
        RUNS.with(|c| c.replace(0))
    }
}

/// The pinned analysis is exact (see [`pinned`]); in this crate's tests every analysis is
/// checked against the dense analysis of the whole component wherever that one is reliable:
/// its Jacobian finite, and its answer the same from a thousandth to a thousand times its
/// tolerance. (Where it is not, the Jacobian is nearly singular and the two analyses may
/// decide its rank differently: the dense one on the whole matrix, the pinned one on the
/// remainder after eliminating what the constraints pin down, and which pivots fall under the
/// tolerance depends on that order.)
#[cfg(test)]
fn assert_dense_agrees(j: &Mat, rank: usize, determined: &[bool]) {
    let at = |tol: f64| if j.rows > 0 { determined_vars(j, tol) } else { (vec![false; j.cols], 0) };
    let dense = at(1e-7);
    if j.data.iter().any(|v| !v.is_finite()) || at(1e-10) != dense || at(1e-4) != dense {
        return;
    }
    assert_eq!((rank, determined), (dense.1, dense.0.as_slice()), "the pinned rank analysis disagrees with the dense one");
}

/// Least squared singular value the rows pinning an unknown must have on its own columns
/// (a singular value of 1e-4, a thousand times the rank analysis' pivot tolerance)…
const PIN_RANK: f64 = 1e-8;
/// …and the least ratio of its squared singular values (a condition number of at most 1e4), so
/// that the 2 × 2 test is decided well clear of rounding.
const PIN_CONDITION: f64 = 1e-8;

/// What [`pinned`] works for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pin {
    /// The solve: the candidates are where the seed constructed them, and every row that pins
    /// one must hold there (below `TOL`), as LM will not move it.
    Verified,
    /// The rank analysis of a component some block ties to a constant.
    Anchored,
    /// The rank analysis of a component no block ties to a constant: the point whose first
    /// variable this is counts as a constant (it takes out the translation, see `pinned`).
    Floating(usize),
}

impl Pin {
    /// The first variable of the point taken as a constant.
    fn anchor(self) -> Option<usize> {
        match self {
            Pin::Floating(a) => Some(a),
            Pin::Verified | Pin::Anchored => None,
        }
    }
}

/// Map `keep` to columns `0..` and the other variables of `all` to none.
fn columns(col: &mut [usize], keep: &[usize], all: &[usize]) {
    for v in all {
        if let Some(c) = col.get_mut(*v) {
            *c = usize::MAX;
        }
    }
    for (k, v) in keep.iter().enumerate() {
        if let Some(c) = col.get_mut(*v) {
            *c = k;
        }
    }
}

/// The variables of `vars` neither pinned nor of the point at `anchor` (sorted, as `vars`).
fn unpinned(vars: &[usize], pin: &[bool], anchor: Option<usize>) -> Vec<usize> {
    let anchored = |v: usize| anchor.is_some_and(|a| v == a || v == a + 1);
    vars.iter().zip(pin).filter(|(v, p)| !**p && !anchored(**v)).map(|(v, _)| *v).collect()
}

/// The blocks that read one of `vars` (sorted).
fn blocks_reading<'a>(blocks: &[(Block<'a>, Vec<usize>)], vars: &[usize]) -> Vec<(Block<'a>, Vec<usize>)> {
    blocks.iter().filter(|(_, bv)| bv.iter().any(|v| vars.binary_search(v).is_ok())).map(|(b, v)| (copy_block(b), v.clone())).collect()
}

/// Which variables of one component (`vars`, sorted; `blocks` its blocks) the constraints pin
/// down one unknown after another: an unknown is pinned when the rows of the blocks that read
/// nothing else but constants and unknowns pinned before it have full column rank on its own
/// columns (well conditioned: `PIN_RANK`, `PIN_CONDITION`). Only `candidate` unknowns are
/// pinned; for [`Pin::Verified`] also only where those rows hold.
///
/// Why this is exact. Order the pinned unknowns as they were pinned, P, and call the rest R.
/// Each pinned unknown has its rows S_u, which read nothing after it in P and nothing in R, so
/// the rows S = ∪ S_u of the Jacobian restricted to the columns of P form a block lower
/// triangular matrix whose diagonal blocks have full column rank, and they are zero on R. For
/// any vector (p, r) in the null space of J, S gives p = 0, so the null space is
/// {(0, r) : J_R r = 0}, where J_R is J's columns of R (every row, those of S included, are zero
/// there). Hence:
/// - rank J = |P| + rank J_R: the analysis eliminates P by construction and runs the dense rank
///   analysis on J_R only;
/// - every pinned variable is determined (zero in every null vector), and a variable of R is
///   determined in J exactly when it is in J_R.
///
/// For the solve, the same structure means the pinned values are the only ones near them that
/// satisfy S, given the constants: freezing them at verified values and solving R with LM finds
/// a solution of the whole system whenever one exists with P where the seed put it. (When LM
/// cannot solve R that way, the component is solved whole, as without pinning.)
///
/// A component no block ties to a constant ([`Pin::Floating`]) is invariant under
/// translation: every residual depends only on differences of its points (and radii, which a
/// translation leaves alone), so the translations t_x, t_y are null vectors of J. Removing one
/// point a's two columns then keeps the rank, as any null vector of J is a null vector of J
/// without a (all of whose entries on a are zero) plus a translation: rank J = rank J_{-a}, and
/// J_{-a} is analysed as above with a as a constant. The translations move every point, so no
/// point coordinate of such a component is determined; a radius is determined when it is in
/// J_{-a} (the translations leave it alone).
fn pinned(
    sk: &Sketch,
    lay: &Layout,
    blocks: &[(Block, Vec<usize>)],
    vars: &[usize],
    x: &mut [f64],
    candidate: &dyn Fn(usize) -> bool,
    pin: Pin,
) -> Vec<bool> {
    let nv = vars.len();
    let local = |v: usize| vars.binary_search(&v).ok();
    // Per local unknown (by its first variable's index): pinned, or the anchor.
    let mut done = vec![false; nv];
    if let Some(a) = pin.anchor().and_then(local).and_then(|a| done.get_mut(a)) {
        *a = true;
    }
    let block_units: Vec<Vec<usize>> = blocks
        .iter()
        .map(|(_, bv)| {
            let mut u: Vec<usize> = bv.iter().filter_map(|v| local(lay.unit(*v))).collect();
            u.dedup();
            u
        })
        .collect();
    let mut of_unit: Vec<Vec<usize>> = vec![Vec::new(); nv];
    for (bi, us) in block_units.iter().enumerate() {
        for u in us {
            if let Some(list) = of_unit.get_mut(*u) {
                list.push(bi);
            }
        }
    }
    let open = |done: &[bool], u: &usize| !done.get(*u).copied().unwrap_or(true);
    // Per block: its unknowns not pinned yet. A block is ready for its last one.
    let mut pending: Vec<usize> = block_units.iter().map(|us| us.iter().filter(|u| open(&done, u)).count()).collect();
    let mut ready: Vec<usize> = (0..blocks.len()).filter(|bi| pending.get(*bi) == Some(&1)).rev().collect();
    let mut gram = vec![[0.0_f64; 3]; nv];
    let mut bad = vec![false; nv];
    let mut queue = Vec::new();
    let mut out = vec![false; nv];
    loop {
        while let Some(bi) = ready.pop() {
            let (Some(us), Some((block, _))) = (block_units.get(bi), blocks.get(bi)) else { continue };
            let Some(&u) = us.iter().find(|u| open(&done, u)) else { continue };
            let Some(&first) = vars.get(u) else { continue };
            if bad.get(u).copied().unwrap_or(true) || !candidate(first) {
                continue;
            }
            let Some(g) = gram.get_mut(u) else { continue };
            let residual = accumulate(sk, lay, block, &[first, first + 1][..lay.width(first)], x, g);
            if pin == Pin::Verified
                && !(residual < TOL)
                && let Some(b) = bad.get_mut(u)
            {
                *b = true;
                continue;
            }
            queue.push(u);
        }
        let Some(u) = queue.pop() else { break };
        let (Some(&first), Some(&g)) = (vars.get(u), gram.get(u)) else { continue };
        let w = lay.width(first);
        if !open(&done, &u) || bad.get(u).copied().unwrap_or(true) || !full_rank(g, w) {
            continue;
        }
        for k in u..u + w {
            if let (Some(d), Some(o)) = (done.get_mut(k), out.get_mut(k)) {
                (*d, *o) = (true, true);
            }
        }
        for &bi in of_unit.get(u).into_iter().flatten() {
            if let Some(p) = pending.get_mut(bi) {
                *p = p.saturating_sub(1);
                if *p == 1 {
                    ready.push(bi);
                }
            }
        }
    }
    out
}

/// Add the rows of block `b`, restricted to the columns of `unit` (one or two variables), to
/// `gram` (their AᵀA: [a00, a01, a11]), with the same differences as [`local_system`]; returns
/// the block's largest residual.
fn accumulate(sk: &Sketch, lay: &Layout, b: &Block, unit: &[usize], x: &mut [f64], gram: &mut [f64; 3]) -> f64 {
    let mut r = Vec::new();
    block_residuals(&State { sk, lay, x }, b, &mut r);
    let mut d = [Vec::new(), Vec::new()];
    let (mut plus, mut minus) = (Vec::new(), Vec::new());
    for (&v, dk) in unit.iter().zip(&mut d) {
        let Some(x0) = x.get(v).copied() else { continue };
        let h = 1e-7 * x0.abs().max(1.0);
        for (at, out) in [(x0 + h, &mut plus), (x0 - h, &mut minus)] {
            if let Some(xv) = x.get_mut(v) {
                *xv = at;
            }
            out.clear();
            block_residuals(&State { sk, lay, x }, b, out);
        }
        if let Some(xv) = x.get_mut(v) {
            *xv = x0;
        }
        *dk = plus.iter().zip(&minus).map(|(p, m)| (p - m) / (2.0 * h)).collect();
    }
    let [d0, d1] = &d;
    for k in 0..r.len() {
        let (a, b) = (d0.get(k).copied().unwrap_or(0.0), d1.get(k).copied().unwrap_or(0.0));
        gram[0] += a * a;
        gram[1] += a * b;
        gram[2] += b * b;
    }
    max_abs(&r)
}

/// The rows behind `gram` (see [`accumulate`]) have full, well-conditioned column rank on a
/// unit of `width` variables.
fn full_rank(gram: [f64; 3], width: usize) -> bool {
    let [a, b, c] = gram;
    let (min, max) = if width == 1 {
        (a, a)
    } else {
        let max = 0.5 * (a + c + ((a - c).powi(2) + 4.0 * b * b).sqrt());
        (if max > 0.0 { (a * c - b * b) / max } else { 0.0 }, max)
    };
    min.is_finite() && max.is_finite() && min >= PIN_RANK && min >= PIN_CONDITION * max
}

/// Solve the sketch in place. On failure the geometry is restored and the report lists the
/// violated constraints.
pub fn solve(sk: &mut Sketch) -> SolveReport {
    solve_with(sk, &Hold::default(), None, Analysis::Full, None)
}

/// Solve with some geometry held as if it were fixed (for this solve only): applying a
/// constraint between two entities holds the first so only the second moves. Everything else
/// moves as little as it can. On failure the geometry is restored.
pub fn solve_holding(sk: &mut Sketch, hold: &Hold) -> SolveReport {
    solve_with(sk, hold, None, Analysis::Full, None)
}

/// What a solve works out besides placing the geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Analysis {
    /// Degrees of freedom and which geometry the constraints determine: a rank analysis of the
    /// Jacobian, the costly part of solving an already solved sketch.
    Full,
    /// Only whether it solved: the report's `dof` is 0 and its determined flags are empty. For
    /// solves whose report is only checked with [`SolveReport::ok`] (the frames of a drag).
    StatusOnly,
}

/// [`solve`] with some geometry held as if it were fixed (for this solve only): applying a
/// constraint between two entities holds the first so only the second moves; a drag holds what
/// it drags. Everything else moves as little as it can. On failure the geometry is restored.
///
/// `reference`, after an interactive move: the points' positions from before it. Points the
/// constraints pin down keep the orientation they had there rather than the one the move left
/// them in. Points appended after the reference was taken use their current positions; a
/// reference longer than the point list is ignored.
///
/// `budget`: the work the seed's cluster decomposition may do, shared with other solves (the
/// steps of a live drag frame, [`Budget::frame`]); [`Budget::full`] for this solve alone when
/// `None`.
pub fn solve_with(sk: &mut Sketch, hold: &Hold, reference: Option<&[Vec2]>, analysis: Analysis, budget: Option<&mut Budget>) -> SolveReport {
    let mut own = Budget::full();
    solve_from(sk, hold, reference, analysis, None, budget.unwrap_or(&mut own))
}

/// Driving values by constraint id (see [`Sketch::driving_values`]).
pub type DrivingValues = std::collections::BTreeMap<String, f64>;

/// [`solve`] after driving values changed: the sketch's geometry is its stored solved state,
/// solved for `old` (by constraint id; constraints added since have none). A component the
/// construction leaves to the iteration (a triconnected one) can fail to converge, or fold a
/// constrained triangle over, when solved for the new values in one jump; it is then solved
/// again by stepping its changed values from old to new (see `continue_values`). Where no
/// value changed a solve behaves as [`solve`].
pub fn solve_after_value_edit(sk: &mut Sketch, old: &DrivingValues) -> SolveReport {
    solve_after_edit(sk, old, None)
}

/// [`solve_after_value_edit`] of a sketch whose linked geometry may have moved as well: once
/// its links are refreshed ([`Sketch::update_link`]), the linked points are where their sources
/// are now and everything else is still the stored solved state. `stored` has every point's
/// position in that state ([`Sketch::positions_in`]; `None` when no link moved), and the sides
/// and turns the solve keeps are read there, as [`solve_with`] reads them from the positions
/// before an interactive move. Read where the links are now, a point dimensioned from a
/// projected line is on the far side of it as soon as the source moves the line past the point.
pub fn solve_after_edit(sk: &mut Sketch, old: &DrivingValues, stored: Option<&[Vec2]>) -> SolveReport {
    let old_values: Vec<Option<f64>> = sk.constraints.iter().map(|c| old.get(&c.id).copied()).collect();
    solve_from(sk, &Hold::default(), stored, Analysis::Full, Some(&old_values), &mut Budget::full())
}

impl Sketch {
    /// Each dimension's driving value by constraint id, as [`solve_after_value_edit`] takes
    /// them.
    pub fn driving_values(&self) -> DrivingValues {
        self.constraints.iter().filter_map(|c| Some((c.id.clone(), c.kind.value()?))).collect()
    }
}

/// The variables' values where the sketch is now.
fn initial_x(sk: &Sketch, lay: &Layout) -> Vec<f64> {
    let mut x = vec![0.0; lay.n];
    for (i, p) in sk.points.iter().enumerate() {
        if let Some(v) = lay.pvar.get(i).copied().flatten() {
            set_point(&mut x, v, p.pos);
        }
    }
    for (i, c) in sk.curves.iter().enumerate() {
        if let (CurveKind::Circle { r, .. } | CurveKind::Ellipse { r, .. }, Some(v)) = (&c.kind, lay.rvar.get(i).copied().flatten())
            && let Some(s) = x.get_mut(v)
        {
            *s = *r;
        }
    }
    x
}

/// The solve's blocks in order (the curves' own relations, then the driving constraints in
/// constraint order) with the variables each reads, and per block whether it reads a constant
/// point.
fn build_blocks<'a>(sk: &'a Sketch, lay: &Layout, on_curve: &[(usize, usize)]) -> (Vec<(Block<'a>, Vec<usize>)>, Vec<bool>) {
    let mut raw = Vec::new();
    for c in &sk.curves {
        if let CurveKind::Spline { pts, periodic: true, .. } = &c.kind
            && let (Some(a), Some(b)) = (pts.first(), pts.last())
            && a != b
        {
            raw.push(Block::Closure { a: *a, b: *b });
        }
        if let CurveKind::Arc { c, a, b } = c.kind {
            raw.push(Block::Arc { c, a, b });
        }
    }
    for (i, c) in sk.constraints.iter().enumerate() {
        if c.driven || matches!(c.kind, ConstraintKind::OffsetDistance { .. }) {
            continue;
        }
        raw.push(match touch_point(sk, &c.kind, on_curve) {
            Some(p) => Block::TangentAt(i, &c.kind, p),
            None => Block::User(i, &c.kind),
        });
    }
    let mut blocks: Vec<(Block, Vec<usize>)> = Vec::with_capacity(raw.len());
    let mut reads_constant = Vec::with_capacity(raw.len());
    for bl in raw {
        let (vars, constant) = block_vars(sk, lay, &bl);
        blocks.push((bl, vars));
        reads_constant.push(constant);
    }

    (blocks, reads_constant)
}

#[cfg(not(test))]
fn solve_from(
    sk: &mut Sketch,
    hold: &Hold,
    reference: Option<&[Vec2]>,
    analysis: Analysis,
    old_values: Option<&[Option<f64>]>,
    budget: &mut Budget,
) -> SolveReport {
    solve_inner(sk, hold, reference, analysis, old_values, budget)
}

/// In this crate's tests every solve is repeated without the constructive seed: the seed only
/// chooses where the iteration starts, so status, degrees of freedom and determined flags must
/// not depend on it, nor the (empty) failing list of a solved sketch. The one difference
/// allowed is the seed's purpose: the iteration from the plain start fails where the seeded
/// one solves. Which constraints a failed solve leaves violated depends on where the iteration
/// stopped; the seed may only narrow it.
#[cfg(test)]
fn solve_from(
    sk: &mut Sketch,
    hold: &Hold,
    reference: Option<&[Vec2]>,
    analysis: Analysis,
    old_values: Option<&[Option<f64>]>,
    budget: &mut Budget,
) -> SolveReport {
    use crate::construct::switch;
    if !(switch::seed_on() && switch::differential_on()) {
        return solve_inner(sk, hold, reference, analysis, old_values, budget);
    }
    let mut plain = sk.clone();
    let without = switch::without_seed(|| solve_inner(&mut plain, hold, reference, analysis, old_values, &mut Budget::full()));
    let with = solve_inner(sk, hold, reference, analysis, old_values, budget);
    assert_same_outcome(&with, &without);
    with
}

/// The outcome the constructive seed must not change (see `solve_from`).
#[cfg(test)]
fn assert_same_outcome(with: &SolveReport, without: &SolveReport) {
    if with.ok() && !without.ok() {
        return;
    }
    assert_eq!(
        (with.status, with.dof, &with.point_determined, &with.curve_determined),
        (without.status, without.dof, &without.point_determined, &without.curve_determined),
        "the constructive seed changed the solve's outcome"
    );
    // Turned-over orientations depend on where the iteration went; only conflicts count.
    let conflicts = |r: &SolveReport| r.failing.iter().filter(|c| !r.folded.contains(c)).cloned().collect::<Vec<_>>();
    if with.ok() {
        assert_eq!(conflicts(with), conflicts(without));
    } else {
        assert!(conflicts(with).iter().all(|c| without.failing.contains(c)), "seeded {with:?}, unseeded {without:?}");
    }
}

/// Write a point's position into its two variables starting at `v`.
/// An arc's ends follow its centre: when the seed moved the centre and did not construct an end,
/// that end starts carried by the centre's displacement (the arc translates, keeping its radius).
/// Left behind, the ends of an arc whose centre is put on the origin are turned by LM instead of
/// following it (reference test 20-slot-constrained).
fn carry_with_seeded(x: &mut [f64], x0: &[f64], constructed: &[bool], blocks: &[(Block, Vec<usize>)], lay: &Layout) {
    let var = |p: usize| lay.pvar.get(p).copied().flatten();
    let built = |v: usize| constructed.get(v).copied().unwrap_or(false);
    let mut carried: Vec<(usize, Vec2)> = Vec::new();
    for (block, _) in blocks {
        let Block::Arc { c, a, b } = *block else { continue };
        let Some(vc) = var(c).filter(|v| built(*v)) else { continue };
        let (Some(now), Some(was)) = (x.get(vc..vc + 2), x0.get(vc..vc + 2)) else { continue };
        let d = Vec2::new(now[0] - was[0], now[1] - was[1]);
        if d.x == 0.0 && d.y == 0.0 {
            continue;
        }
        carried.extend([a, b].into_iter().filter_map(var).filter(|v| !built(*v)).map(|v| (v, d)));
    }
    for (v, d) in carried {
        if let Some([px, py]) = x.get_mut(v..v + 2) {
            *px += d.x;
            *py += d.y;
        }
    }
}

fn set_point(x: &mut [f64], v: usize, at: Vec2) {
    if let Some([sx, sy]) = x.get_mut(v..v.saturating_add(2)) {
        *sx = at.x;
        *sy = at.y;
    }
}

/// Exactly opposite directions sit on the wrapped angle's derivative discontinuity: nudge one
/// free endpoint of each such line into a continuous branch before Newton iterations.
fn nudge_line_angles(sk: &Sketch, lay: &Layout, x: &mut [f64]) {
    for constraint in &sk.constraints {
        if constraint.driven {
            continue;
        }
        let ConstraintKind::LineAngle { l, value } = constraint.kind else { continue };
        let Some(CurveKind::Line { a, b }) = sk.curves.get(l).map(|c| &c.kind) else { continue };
        let state = State { sk, lay, x };
        let (pa, pb) = (state.p(*a), state.p(*b));
        let delta = wrap_angle((pb - pa).angle() - value);
        if delta.abs() < std::f64::consts::PI - 1e-5 || !delta.is_finite() {
            continue;
        }
        let d = Vec2::from_angle((pb - pa).angle() - delta.signum() * 0.01) * pa.dist(pb);
        let (point, at) = if lay.pvar.get(*b).copied().flatten().is_some() { (*b, pa + d) } else { (*a, pb - d) };
        if let Some(index) = lay.pvar.get(point).copied().flatten() {
            set_point(x, index, at);
        }
    }
}

/// Continuation steps an edit in sixty-fourths of it: the smallest sub-step is one of them.
const SUBSTEPS: usize = 64;
/// LM iterations one continuation may spend over all its sub-steps (taken and halved).
const CONTINUATION_ITERS: usize = 4 * MAX_ITERS;
/// Triangles of distance dimensions whose turn one component's fold check reads.
const MAX_TRIPLES: usize = 1024;
/// Neighbour visits while finding those triangles.
const MAX_TRIPLE_WORK: usize = 100_000;

/// An orientation the constraints leave open, which an edit of their values must keep: the
/// mirror image is "the other solution" (Owen 1991 §5: whether three dimensioned points occur
/// clockwise or counter-clockwise; an unsigned offset's side).
#[derive(Clone, Copy, Debug)]
enum Orient {
    /// The turn of three points pairwise joined by distances (coincident corners merged).
    Turn([usize; 3]),
    /// Which side of `p` the point `q` is along x (`DistanceX` of the given size).
    SideX(usize, usize, f64),
    /// The same along y (`DistanceY`).
    SideY(usize, usize, f64),
    /// Which side of line `l` the point `p` is on (`PointLineDistance`, `LinearDiameter`).
    OfLine(usize, usize, f64),
}

/// An orientation's signed offset in a state (a corner's distance from the opposite side, an
/// offset along its axis or from its line), the size it is measured against and how far the
/// coordinates are from the origin.
fn offset(s: &State, o: Orient) -> Option<(f64, f64, f64)> {
    let coords = |ps: &[Vec2]| ps.iter().fold(0.0_f64, |m, p| m.max(crate::loci::mag(*p)));
    Some(match o {
        Orient::Turn([a, b, c]) => {
            let (pa, pb, pc) = (s.p(a), s.p(b), s.p(c));
            let size = pa.dist(pb).max(pa.dist(pc)).max(pb.dist(pc));
            ((pb - pa).normalized()?.cross(pc - pa), size, coords(&[pa, pb, pc]))
        }
        Orient::SideX(p, q, size) => {
            let (pp, pq) = (s.p(p), s.p(q));
            (pq.x - pp.x, size, coords(&[pp, pq]))
        }
        Orient::SideY(p, q, size) => {
            let (pp, pq) = (s.p(p), s.p(q));
            (pq.y - pp.y, size, coords(&[pp, pq]))
        }
        Orient::OfLine(p, l, size) => {
            let (a, b) = s.line(l);
            let pp = s.p(p);
            ((b - a).normalized()?.cross(pp - a), size, coords(&[pp, a, b]))
        }
    })
}

/// The sign of an orientation in a state; `None` when too close to zero to tell. It is the
/// constructive seed's side test (`loci::side`, the rounding of the coordinates and a tiny
/// share of the size), so the seed and the fold check agree on which way a triangle turns.
/// Belongs in `loci` beside `side` (moved there once the lines-and-rounds work is merged).
fn orientation(s: &State, o: Orient) -> Option<f64> {
    offset(s, o).and_then(|(delta, size, coords)| crate::loci::side(delta, size, coords))
}

/// A reference orientation must be clear: an offset of at least this share of its size (for a
/// triangle, a height of 1/1000 of its longest side, about 0.06°). Nearer degenerate, solver
/// noise decides the sign, and no edit is refused for turning it over.
const ORIENTED: f64 = 1e-3;

/// The orientation of the stored state that an edit must keep: its sign where it is clear
/// (see [`ORIENTED`]), else `None`.
fn reference_orientation(s: &State, o: Orient) -> Option<f64> {
    offset(s, o).filter(|(delta, size, _)| delta.abs() > ORIENTED * size).and_then(|(delta, size, coords)| crate::loci::side(delta, size, coords))
}

/// A driving value `t` of the way from `old` to `new`: angles of a line or between two lines
/// turn the short way round.
fn value_at(kind: &ConstraintKind, old: f64, new: f64, t: f64) -> f64 {
    if t >= 1.0 {
        return new;
    }
    match kind {
        ConstraintKind::LineAngle { .. } | ConstraintKind::Angle { .. } => old + wrap_angle(new - old) * t,
        _ => old + (new - old) * t,
    }
}

/// Whether a value can be stepped from `old` to `new`: not a signed arc sweep that changes
/// sign (the arc's end points swap; there is no path of arcs between the two).
fn steppable(kind: &ConstraintKind, old: f64, new: f64) -> bool {
    !(matches!(kind, ConstraintKind::ArcSweep { .. }) && old * new <= 0.0)
}

/// One component's driving values changed by an edit, and the orientations of the stored
/// state.
struct ValueEdit {
    /// Block index, constraint index, the constraint (with its new value) and its old value.
    changed: Vec<(usize, usize, ConstraintKind, f64)>,
    /// Orientations with the constraints that set them and their sign in the stored state.
    orients: Vec<(Orient, Vec<usize>, Option<f64>)>,
}

impl ValueEdit {
    /// The values of `sub` changed against `old` (per constraint), `before` being the stored
    /// state; `None` when nothing changed.
    fn of(before: &State, sub: &[(Block, Vec<usize>)], old: &[Option<f64>]) -> Option<Self> {
        let changed: Vec<_> = sub
            .iter()
            .enumerate()
            .filter_map(|(k, (b, _))| {
                let Block::User(ci, kind) = b else { return None };
                let (o, n) = (old.get(*ci).copied().flatten()?, kind.value()?);
                (o.is_finite() && n.is_finite() && o != n).then(|| (k, *ci, (*kind).clone(), o))
            })
            .collect();
        if changed.is_empty() {
            return None;
        }
        let orients = orients(before, sub).into_iter().map(|(o, cs)| (o, cs, reference_orientation(before, o))).collect();
        Some(ValueEdit { changed, orients })
    }

    /// The stored state is solved for the old values: there is a solved state to continue
    /// from and orientations to keep (a sketch stored unsolved has neither).
    fn starts_solved(&self, before: &State, sub: &[(Block, Vec<usize>)]) -> bool {
        let olds = self.kinds_at(0);
        let mut blocks: Vec<(Block, Vec<usize>)> = sub.iter().map(|(b, v)| (copy_block(b), v.clone())).collect();
        self.point_at(&mut blocks, &olds);
        max_abs(&system(before.sk, before.lay, &blocks, &mut before.x.to_vec(), false).0) < ACCEPT
    }

    /// The values can be stepped (see [`steppable`]).
    fn continuable(&self) -> bool {
        self.changed.iter().all(|(_, _, k, o)| k.value().is_some_and(|n| steppable(k, *o, n)))
    }

    /// The changed constraints `j` sixty-fourths of the way from their old values to the new.
    fn kinds_at(&self, j: usize) -> Vec<ConstraintKind> {
        let t = j as f64 / SUBSTEPS as f64;
        self.changed
            .iter()
            .map(|(_, _, kind, o)| {
                let mut k = kind.clone();
                if let Some(n) = kind.value() {
                    k.set_value(value_at(kind, *o, n, t));
                }
                k
            })
            .collect()
    }

    /// Point the changed constraints' blocks at `kinds` (as [`ValueEdit::kinds_at`] gives them).
    fn point_at<'a>(&self, blocks: &mut [(Block<'a>, Vec<usize>)], kinds: &'a [ConstraintKind]) {
        for ((k, ci, ..), kind) in self.changed.iter().zip(kinds) {
            if let Some(slot) = blocks.get_mut(*k) {
                slot.0 = Block::User(*ci, kind);
            }
        }
    }

    /// The constraints setting an orientation of the stored state that `now` turns the other way.
    fn flipped<'a>(&'a self, now: &'a State) -> impl Iterator<Item = &'a usize> + 'a {
        self.orients.iter().filter(|(o, _, before)| before.zip(orientation(now, *o)).is_some_and(|(b, n)| b != n)).flat_map(|(_, cs, _)| cs)
    }

    /// Solved, keeping every orientation of the stored state.
    fn holds(&self, residual: f64, now: &State) -> bool {
        residual < ACCEPT && self.flipped(now).next().is_none()
    }
}

/// The orientations the driving constraints of `sub` leave open: triangles of distances and
/// line lengths (coincident points count as one, the corners of a triangle of lines), and the
/// sides of unsigned offsets. Each with the constraints that set it; capped.
fn orients(s: &State, sub: &[(Block, Vec<usize>)]) -> Vec<(Orient, Vec<usize>)> {
    use std::collections::{BTreeMap, BTreeSet};
    let free = |p: usize| s.lay.pvar.get(p).copied().flatten().is_some();
    let line = |l: usize| match s.sk.curves.get(l).map(|c| &c.kind) {
        Some(CurveKind::Line { a, b }) => Some((*a, *b)),
        _ => None,
    };
    let mut out = Vec::new();
    let mut joins = Vec::new();
    let mut edges = Vec::new();
    for (b, _) in sub {
        let Block::User(ci, kind) = b else { continue };
        match **kind {
            ConstraintKind::Coincident { p, q } => joins.push((p, q)),
            ConstraintKind::Distance { p, q, .. } => edges.push((p, q, *ci)),
            ConstraintKind::Length { l, .. } => edges.extend(line(l).map(|(p, q)| (p, q, *ci))),
            ConstraintKind::DistanceX { p, q, value } if free(p) || free(q) => out.push((Orient::SideX(p, q, value), vec![*ci])),
            ConstraintKind::DistanceY { p, q, value } if free(p) || free(q) => out.push((Orient::SideY(p, q, value), vec![*ci])),
            ConstraintKind::PointLineDistance { p, l, value } => out.push((Orient::OfLine(p, l, value), vec![*ci])),
            ConstraintKind::LinearDiameter { p, l, value } => out.push((Orient::OfLine(p, l, value / 2.0), vec![*ci])),
            _ => {}
        }
    }
    // Union-find over the points these constraints name (not the whole sketch).
    let mut local: BTreeMap<usize, usize> = BTreeMap::new();
    for (p, q) in joins.iter().copied().chain(edges.iter().map(|&(p, q, _)| (p, q))) {
        for i in [p, q] {
            let n = local.len();
            local.entry(i).or_insert(n);
        }
    }
    let mut parent: Vec<usize> = (0..local.len()).collect();
    let at = |i: usize| local.get(&i).copied().unwrap_or(0);
    for &(p, q) in &joins {
        union(&mut parent, at(p), at(q));
    }
    // Each group stands for the first of its points met; it is free when any of them is.
    let mut rep: BTreeMap<usize, usize> = BTreeMap::new();
    let mut free_groups: BTreeSet<usize> = BTreeSet::new();
    let mut adj: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
    for &(p, q, ci) in &edges {
        let [a, b] = [p, q].map(|i| {
            let r = *rep.entry(find(&mut parent, at(i))).or_insert(i);
            if free(i) {
                free_groups.insert(r);
            }
            r
        });
        if a != b {
            adj.entry(a).or_default().entry(b).or_insert(ci);
            adj.entry(b).or_default().entry(a).or_insert(ci);
        }
    }
    let mut work = 0usize;
    let mut triangles = 0usize;
    for (&a, na) in &adj {
        for (&b, &ab) in na.range(a + 1..) {
            let Some(nb) = adj.get(&b) else { continue };
            for (&c, &ac) in na.range(b + 1..) {
                work += 1;
                if work > MAX_TRIPLE_WORK || triangles >= MAX_TRIPLES {
                    return out;
                }
                if let Some(&bc) = nb.get(&c)
                    && [a, b, c].iter().any(|p| free_groups.contains(p))
                {
                    triangles += 1;
                    out.push((Orient::Turn([a, b, c]), vec![ab, bc, ac]));
                }
            }
        }
    }
    out
}

/// Dimension continuation: solve the component `sub` from `x` (its previous solved state) for
/// its changed values stepped from old to new, each sub-step from the previous solution. A
/// sub-step that fails or flips an orientation of the previous state is halved; the step
/// doubles again after one that holds. Gives up below one [`SUBSTEPS`]th of the edit or after
/// [`CONTINUATION_ITERS`] iterations. Returns the iterations and whether the new values were
/// reached; `x` holds the last sub-step's solution either way.
fn continue_values(
    sk: &Sketch,
    lay: &Layout,
    sub: &[(Block, Vec<usize>)],
    vars: &[usize],
    col: &[usize],
    x: &mut [f64],
    edit: &ValueEdit,
) -> (usize, bool) {
    // Every sub-step lands on a sixty-fourth: the changed constraints at each, built once.
    let arena: Vec<Vec<ConstraintKind>> = (0..=SUBSTEPS).map(|j| edit.kinds_at(j)).collect();
    let mut blocks: Vec<(Block, Vec<usize>)> = sub.iter().map(|(b, v)| (copy_block(b), v.clone())).collect();
    let (mut t, mut h, mut spent) = (0, SUBSTEPS / 2, 0);
    let mut trial = x.to_vec();
    while t < SUBSTEPS && spent < CONTINUATION_ITERS {
        let next = (t + h).min(SUBSTEPS);
        if let Some(kinds) = arena.get(next) {
            edit.point_at(&mut blocks, kinds);
        }
        trial.copy_from_slice(x);
        let (its, residual) = lm_capped(sk, lay, &blocks, vars, &mut trial, col, MAX_ITERS.min(CONTINUATION_ITERS - spent));
        spent += its.max(1);
        if edit.holds(residual, &State { sk, lay, x: &trial }) {
            x.copy_from_slice(&trial);
            t = next;
            h = (2 * h).min(SUBSTEPS);
        } else if h > 1 {
            h /= 2;
        } else {
            break;
        }
    }
    (spent, t >= SUBSTEPS)
}

fn solve_inner(
    sk: &mut Sketch,
    hold: &Hold,
    reference: Option<&[Vec2]>,
    analysis: Analysis,
    old_values: Option<&[Option<f64>]>,
    budget: &mut Budget,
) -> SolveReport {
    sk.prune_patterns();
    sk.refresh_pattern_shapes();
    let mut orientation_before = Vec::new();
    for constraint in &sk.constraints {
        if !constraint.driven
            && let ConstraintKind::ArcSweep { c, value } = constraint.kind
            && value.is_finite()
            && let Some(curve) = sk.curves.get_mut(c)
            && let CurveKind::Arc { a, b, .. } = &mut curve.kind
            && curve.reversed != (value < 0.0)
        {
            orientation_before.push((c, *a, *b, curve.reversed));
            std::mem::swap(a, b);
            curve.reversed = value < 0.0;
        }
    }
    let lay = layout(sk, hold);
    let mut x = initial_x(sk, &lay);
    let x0 = x.clone();
    let on_curve = points_on_curves(sk);
    // Start from the constructed positions of the points the constraints pin down (see
    // `construct`); LM still verifies every constraint from there. A touching placement does
    // not pin a point down well and is left where it is (`Placement::anchors`). `unseeded`
    // keeps the start the solve would have without the seed, for the variables the seed moved:
    // a component the seeded start cannot solve is solved again from there, and only if the
    // seed moved its start.
    let mut unseeded = None;
    // Per variable: the seed constructed it (a candidate to stay where it was put, see `pinned`).
    let mut constructed = vec![false; lay.n];
    if crate::construct::switch::seed_on() {
        let given: Vec<bool> = lay.pvar.iter().map(Option::is_none).collect();
        // A drag step solves against its reference and moves only held points. Any other solve
        // applies edits: also the one after linked geometry moved, whose reference is the
        // stored state.
        let edits = reference.is_none() || old_values.is_some();
        let seeded = crate::construct::seed(sk, &given, reference, edits, &on_curve, budget);
        for (i, (at, how)) in seeded.pos.iter().zip(&seeded.placed_by).enumerate() {
            if let (Some(at), Some(how), Some(v)) = (at, how, lay.pvar.get(i).copied().flatten())
                && how.anchors()
            {
                set_point(&mut x, v, *at);
                // A floated cluster's pose is a guess at freedom the constraints leave: LM's
                // start, never pinned.
                if how.builds()
                    && let Some(c) = constructed.get_mut(v..v + 2)
                {
                    c.fill(true);
                }
            }
        }
        for (r, v) in seeded.radius.iter().zip(&lay.rvar) {
            if let (Some(r), Some(v)) = (r, v)
                && let Some(slot) = x.get_mut(*v)
            {
                *slot = *r;
                if let Some(c) = constructed.get_mut(*v) {
                    *c = true;
                }
            }
        }
        if x != x0 {
            let mut plain = x0.clone();
            nudge_line_angles(sk, &lay, &mut plain);
            unseeded = Some(plain);
        }
    }
    nudge_line_angles(sk, &lay, &mut x);

    // The blocks, and per block whether it reads a constant point.
    let (blocks, reads_constant) = build_blocks(sk, &lay, &on_curve);

    carry_with_seeded(&mut x, &x0, &constructed, &blocks, &lay);
    // The plain start, for the variables whose start the seed (or carrying along it) moved.
    let unseeded = unseeded.map(|plain: Vec<f64>| plain.into_iter().zip(&x).map(|(p, s)| (p != *s).then_some(p)).collect::<Vec<_>>());

    // Connected components over variables.
    let mut parent: Vec<usize> = (0..lay.n).collect();
    for (_, vars) in &blocks {
        if let Some(&first) = vars.first() {
            for &v in vars.iter().skip(1) {
                union(&mut parent, first, v);
            }
        }
    }
    let mut comps: Vec<(usize, Vec<usize>, Vec<usize>)> = Vec::new(); // (root, vars, block idx)
    let mut comp_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for v in 0..lay.n {
        let r = find(&mut parent, v);
        match comp_of.get(&r) {
            Some(&k) => {
                if let Some(c) = comps.get_mut(k) {
                    c.1.push(v);
                }
            }
            None => {
                comp_of.insert(r, comps.len());
                comps.push((r, vec![v], Vec::new()));
            }
        }
    }
    let mut constant_blocks = Vec::new();
    for (bi, (_, vars)) in blocks.iter().enumerate() {
        match vars.first() {
            Some(&v) => {
                let r = find(&mut parent, v);
                if let Some(c) = comp_of.get(&r).and_then(|k| comps.get_mut(*k)) {
                    c.2.push(bi);
                }
            }
            None => constant_blocks.push(bi),
        }
    }
    // Column of each variable in the system being solved or analysed (usize::MAX outside it).
    let mut col = vec![usize::MAX; lay.n];

    let mut iterations = 0;
    let mut continuations = 0;
    let mut folded: Vec<String> = Vec::new();
    let all_blocks = blocks;
    let oversized = |vars: &[usize]| vars.len() > MAX_COMPONENT_VARS;
    let blocks_of = |bis: &mut dyn Iterator<Item = usize>| -> Vec<(Block, Vec<usize>)> {
        bis.filter_map(|bi| all_blocks.get(bi).map(|(b, v)| (copy_block(b), v.clone()))).collect()
    };
    for (_, vars, bis) in &comps {
        if bis.is_empty() || oversized(vars) {
            continue;
        }
        let sub = blocks_of(&mut bis.iter().copied());
        let start: Vec<f64> = vars.iter().filter_map(|v| x.get(*v).copied()).collect();
        let mut residual = max_abs(&system(sk, &lay, &sub, &mut x, false).0);
        // Already solved (by the seed, or before): nothing to iterate.
        if !(residual < TOL) {
            columns(&mut col, vars, vars);
            // What the seed constructed and the constraints pin down stays where the seed put it
            // (see `pinned`); LM solves the rest with those values as constants.
            let (mut pinned_failed, mut solved) = (false, false);
            if vars.iter().any(|v| constructed.get(*v).copied().unwrap_or(false)) {
                let candidate = |v: usize| constructed.get(v).copied().unwrap_or(false);
                let pin = pinned(sk, &lay, &sub, vars, &mut x, &candidate, Pin::Verified);
                if pin.contains(&true) {
                    let rest = unpinned(vars, &pin, None);
                    let rest_blocks = blocks_reading(&sub, &rest);
                    columns(&mut col, &rest, vars);
                    let (its, r) = if rest.is_empty() { (0, 0.0) } else { lm(sk, &lay, &rest_blocks, &rest, &mut x, &col) };
                    iterations += its;
                    residual = r;
                    solved = residual < ACCEPT;
                    if !solved {
                        // The rest cannot be solved with the pinned values: the whole component is.
                        for (&v, s0) in vars.iter().zip(&start) {
                            if let Some(s) = x.get_mut(v) {
                                *s = *s0;
                            }
                        }
                        columns(&mut col, vars, vars);
                        pinned_failed = true;
                    }
                }
            }
            // A seeded start can leave LM stuck where the plain start solves (a partly
            // constructed component): the seeded solution is kept whenever it solves, otherwise a
            // component the seed touched is solved again from where it would have started without
            // the seed. After a failed pinned solve, the whole component goes straight to that
            // start (the seeded one is where the pinned solve failed), or to its own start when
            // the seed did not move it.
            let reseeded = unseeded.as_ref().filter(|plain| vars.iter().any(|v| plain.get(*v).is_some_and(Option::is_some)));
            if !solved && !(pinned_failed && reseeded.is_some()) {
                let (its, r) = lm(sk, &lay, &sub, vars, &mut x, &col);
                iterations += its;
                residual = r;
            }
            if !(residual < ACCEPT)
                && let Some(plain) = reseeded
            {
                for (&v, s0) in vars.iter().zip(&start) {
                    if let Some(s) = x.get_mut(v) {
                        *s = plain.get(v).copied().flatten().unwrap_or(*s0);
                    }
                }
                let (its, r) = lm(sk, &lay, &sub, vars, &mut x, &col);
                iterations += its;
                residual = r;
            }
        }
        // Continuation (below) solves the whole component: the pinned values belong to the new
        // values, not to the ones it steps through.
        columns(&mut col, vars, vars);
        // After an edit of driving values from a solved state, a component the one-jump solve
        // left unsolved, or turned over (an orientation of that state flipped), is solved by
        // continuation from there. Its result replaces the one-jump result only when it solves
        // and keeps every orientation; a solved result that still flips one is refused.
        if let Some(old) = old_values
            && let Some(edit) = ValueEdit::of(&State { sk, lay: &lay, x: &x0 }, &sub, old)
            && !edit.holds(residual, &State { sk, lay: &lay, x: &x })
            && edit.starts_solved(&State { sk, lay: &lay, x: &x0 }, &sub)
        {
            if edit.continuable() {
                continuations += 1;
                // This component's variables restart from the previous state; the components
                // solved before it keep their solutions (LM here touches only `vars`).
                let mut from = x.clone();
                for &v in vars {
                    if let (Some(s), Some(s0)) = (from.get_mut(v), x0.get(v)) {
                        *s = *s0;
                    }
                }
                let (its, mut done) = continue_values(sk, &lay, &sub, vars, &col, &mut from, &edit);
                iterations += its;
                if !done {
                    // The last sub-step's solution, solved for the new values in one jump.
                    let (its, r) = lm(sk, &lay, &sub, vars, &mut from, &col);
                    iterations += its;
                    done = edit.holds(r, &State { sk, lay: &lay, x: &from });
                }
                if done {
                    x = from;
                    residual = 0.0;
                }
            }
            if residual < ACCEPT {
                let now = State { sk, lay: &lay, x: &x };
                for ci in edit.flipped(&now) {
                    if let Some(c) = sk.constraints.get(*ci)
                        && !folded.contains(&c.id)
                    {
                        folded.push(c.id.clone());
                    }
                }
            }
        }
    }

    // Final residuals and analysis, component by component (the Jacobian is block diagonal:
    // its rank is the sum of the components' ranks).
    let mut max_residual = 0.0_f64;
    let mut failing: Vec<String> = Vec::new();
    let mut det = vec![false; lay.n];
    let mut rank = 0;
    let mut redundant: Vec<String> = Vec::new();
    // A component too large to solve is a failure, not an allocation.
    if comps.iter().any(|(_, vars, bis)| !bis.is_empty() && oversized(vars)) {
        max_residual = f64::MAX;
    }
    let mut check = |r: &[f64], owner: &[usize], blocks: &[(Block, Vec<usize>)], failing: &mut Vec<String>| {
        max_residual = max_residual.max(max_abs(r));
        for (row, v) in r.iter().enumerate() {
            if !(v.abs() < ACCEPT)
                && let Some((Block::User(ci, _) | Block::TangentAt(ci, _, _), _)) = owner.get(row).and_then(|bi| blocks.get(*bi))
                && let Some(c) = sk.constraints.get(*ci)
                && !failing.contains(&c.id)
            {
                failing.push(c.id.clone());
            }
        }
    };
    for (_, vars, bis) in &comps {
        if oversized(vars) {
            continue;
        }
        let sub = blocks_of(&mut bis.iter().copied());
        if sub.is_empty() {
            continue;
        }
        let (r, _, owner) = system(sk, &lay, &sub, &mut x, false);
        check(&r, &owner, &sub, &mut failing);
        if analysis == Analysis::StatusOnly {
            continue;
        }
        // The rank analysis runs on what `pinned` leaves: see there for why that is exact.
        let floating = bis.iter().all(|bi| !reads_constant.get(*bi).copied().unwrap_or(true));
        let pin = match vars.iter().copied().find(|v| lay.is_point(*v)) {
            Some(anchor) if floating => Pin::Floating(anchor),
            _ => Pin::Anchored,
        };
        let pin_flags = pinned(sk, &lay, &sub, vars, &mut x, &|_| true, pin);
        let rest = unpinned(vars, &pin_flags, pin.anchor());
        columns(&mut col, &rest, vars);
        let (_, j, _) = local_system(sk, &lay, &blocks_reading(&sub, &rest), &mut x, &col, rest.len());
        let (rest_det, rest_rank) = if j.rows > 0 { determined_vars(&j, 1e-7) } else { (vec![false; rest.len()], 0) };
        let comp_rank = pin_flags.iter().filter(|p| **p).count() + rest_rank;
        rank += comp_rank;
        // More rows than rank: some constraint may be implied by the others.
        if r.len() > comp_rank && vars.len() <= redundancy::MAX_REDUNDANCY_VARS {
            let (dependent, _) = redundancy::dependent_blocks(sk, &lay, &sub, vars, &mut x, None);
            for ((b, _), d) in sub.iter().zip(dependent) {
                if d && let Some(c) = redundancy::block_constraint(b).and_then(|ci| sk.constraints.get(ci))
                    && !redundant.contains(&c.id)
                {
                    redundant.push(c.id.clone());
                }
            }
        }
        for (&v, pinned) in vars.iter().zip(&pin_flags) {
            let flag = match rest.binary_search(&v) {
                Ok(k) => rest_det.get(k).copied().unwrap_or(false),
                Err(_) => *pinned,
            };
            // A floating component's translation moves every point.
            let moves = matches!(pin, Pin::Floating(_)) && lay.is_point(v);
            if let Some(slot) = det.get_mut(v) {
                *slot = flag && !moves;
            }
        }
        #[cfg(test)]
        {
            columns(&mut col, vars, vars);
            let (_, j, _) = local_system(sk, &lay, &sub, &mut x, &col, vars.len());
            let reduced: Vec<bool> = vars.iter().map(|v| det.get(*v).copied().unwrap_or(false)).collect();
            assert_dense_agrees(&j, comp_rank, &reduced);
        }
    }
    let consts = blocks_of(&mut constant_blocks.iter().copied());
    if !consts.is_empty() {
        let (r, _, owner) = system(sk, &lay, &consts, &mut x, false);
        check(&r, &owner, &consts, &mut failing);
    }
    // Weld the whole connected closure graph, preserving every immutable node.
    // Validate the actual prospective geometry before accepting a numerical solve.
    let has_periodic = sk.curves.iter().any(|c| matches!(c.kind, CurveKind::Spline { periodic: true, .. }));
    let periodic_positions = if has_periodic {
        let state = State { sk, lay: &lay, x: &x };
        let mut positions = (0..sk.points.len()).map(|i| state.p(i)).collect::<Vec<_>>();
        let mut parent = (0..positions.len()).collect::<Vec<_>>();
        for curve in &sk.curves {
            if let CurveKind::Spline { pts, periodic: true, .. } = &curve.kind
                && let (Some(a), Some(b)) = (pts.first(), pts.last())
                && *a < parent.len()
                && *b < parent.len()
            {
                union(&mut parent, *b, *a);
            }
        }
        let mut representatives = std::collections::BTreeMap::new();
        let mut valid = true;
        for i in 0..positions.len() {
            let root = find(&mut parent, i);
            if let Some(previous) = representatives.get(&root).copied() {
                let fixed = |p| lay.pvar.get(p).is_some_and(Option::is_none);
                if fixed(i) && fixed(previous) && positions[i] != positions[previous] {
                    valid = false;
                }
                if fixed(i) && !fixed(previous) {
                    representatives.insert(root, i);
                }
            } else {
                representatives.insert(root, i);
            }
        }
        for (i, position) in positions.iter_mut().enumerate() {
            if let Some(index) = representatives.get(&find(&mut parent, i)) {
                *position = state.p(*index);
            }
        }
        for curve in &sk.curves {
            if let CurveKind::Spline { pts, control, degree, periodic: true } = &curve.kind {
                let points = pts.iter().map(|i| positions.get(*i).copied()).collect::<Option<Vec<_>>>();
                if *control || *degree != 3 || points.as_ref().is_none_or(|p| crate::periodic::validate_periodic_points(p).is_err()) {
                    valid = false;
                }
            }
        }
        if !valid {
            max_residual = max_residual.max(1.0);
            None
        } else {
            Some(positions)
        }
    } else {
        None
    };
    let status = if max_residual < ACCEPT { SolveStatus::Solved } else { SolveStatus::Failed };
    for id in &folded {
        if !failing.contains(id) {
            failing.push(id.clone());
        }
    }
    let use_x = if status == SolveStatus::Solved { &x } else { &x0 };
    // Write back.
    for (i, p) in sk.points.iter_mut().enumerate() {
        if let Some(v) = lay.pvar.get(i).copied().flatten() {
            p.pos = Vec2::new(use_x.get(v).copied().unwrap_or(p.pos.x), use_x.get(v + 1).copied().unwrap_or(p.pos.y));
        }
    }
    for (i, c) in sk.curves.iter_mut().enumerate() {
        if let (CurveKind::Circle { r, .. } | CurveKind::Ellipse { r, .. }, Some(v)) = (&mut c.kind, lay.rvar.get(i).copied().flatten()) {
            *r = use_x.get(v).copied().unwrap_or(*r).abs().max(1e-9);
        }
    }
    if status == SolveStatus::Solved
        && let Some(positions) = periodic_positions
    {
        for (point, position) in sk.points.iter_mut().zip(positions) {
            point.pos = position;
        }
    }
    if status == SolveStatus::Failed {
        for (ci, a_before, b_before, reversed) in orientation_before.into_iter().rev() {
            if let Some(curve) = sk.curves.get_mut(ci)
                && let CurveKind::Arc { a, b, .. } = &mut curve.kind
            {
                *a = a_before;
                *b = b_before;
                curve.reversed = reversed;
            }
        }
    }
    let vdet = |v: Option<usize>| v.is_none_or(|v| det.get(v).copied().unwrap_or(false));
    let pdet = |p: usize| match lay.pvar.get(p).copied().flatten() {
        Some(v) => vdet(Some(v)) && vdet(Some(v + 1)),
        None => true,
    };
    // Driven dimensions measure the solved geometry.
    let fixed_lay = Layout { pvar: vec![None; sk.points.len()], rvar: vec![None; sk.curves.len()], n: 0, points: 0, slide: Vec::new() };
    let measured: Vec<Option<f64>> =
        sk.constraints.iter().map(|c| if c.driven { measure(&State { sk, lay: &fixed_lay, x: &[] }, &c.kind) } else { None }).collect();
    for (c, m) in sk.constraints.iter_mut().zip(measured) {
        if let Some(v) = m
            && v.is_finite()
        {
            c.kind.set_value(v);
        }
    }
    let (dof, point_determined, curve_determined) = match analysis {
        Analysis::Full => (
            lay.n.saturating_sub(rank),
            (0..sk.points.len()).map(pdet).collect(),
            sk.curves.iter().enumerate().map(|(i, c)| c.kind.point_ids().into_iter().all(pdet) && vdet(lay.rvar.get(i).copied().flatten())).collect(),
        ),
        Analysis::StatusOnly => (0, Vec::new(), Vec::new()),
    };
    SolveReport {
        status,
        dof,
        iterations,
        max_residual: if max_residual.is_finite() { max_residual } else { f64::MAX },
        failing,
        folded,
        continuations,
        point_determined,
        curve_determined,
        redundant,
    }
}

#[cfg(test)]
mod continuation_unit_tests {
    use super::*;

    #[test]
    fn angles_step_the_short_way_round() {
        let line = ConstraintKind::LineAngle { l: 0, value: 0.0 };
        let (old, new) = (179f64.to_radians(), (-179f64).to_radians());
        let half = value_at(&line, old, new, 0.5);
        assert!((half - 180f64.to_radians()).abs() < 1e-12, "{}", half.to_degrees());
        let first = value_at(&line, old, new, 1.0 / SUBSTEPS as f64);
        assert!((first - old).abs() < 2f64.to_radians() / 32.0, "a sixty-fourth of 2°, not of 358°");
        assert_eq!(value_at(&line, old, new, 1.0), new);
        let distance = ConstraintKind::Distance { p: 0, q: 1, value: 0.0 };
        assert_eq!(value_at(&distance, 10.0, 20.0, 0.5), 15.0);
    }

    #[test]
    fn a_sweep_changing_sign_is_not_stepped() {
        let sweep = ConstraintKind::ArcSweep { c: 0, value: 0.0 };
        assert!(!steppable(&sweep, 1.0, -1.0));
        assert!(steppable(&sweep, 1.0, 2.0));
        assert!(steppable(&ConstraintKind::LineAngle { l: 0, value: 0.0 }, 3.0, -3.0));
    }

    #[test]
    fn only_clear_orientations_are_kept() {
        // A triangle flat to solver noise has a side by the seed's test but none to keep.
        let mut sk = Sketch::new();
        let a = sk.add_point(Vec2::new(0.0, 0.0), None).unwrap();
        let b = sk.add_point(Vec2::new(40.0, 0.0), None).unwrap();
        let c = sk.add_point(Vec2::new(20.0, 1e-6), None).unwrap();
        let lay = Layout { pvar: vec![None; sk.points.len()], rvar: vec![None; sk.curves.len()], n: 0, points: 0, slide: Vec::new() };
        let turn = Orient::Turn([a, b, c]);
        let state =
            |sk: &Sketch| (orientation(&State { sk, lay: &lay, x: &[] }, turn), reference_orientation(&State { sk, lay: &lay, x: &[] }, turn));
        assert_eq!(state(&sk), (Some(1.0), None));
        sk.points[c].pos = Vec2::new(20.0, 0.1);
        assert_eq!(state(&sk), (Some(1.0), Some(1.0)));
        // A side offset well below its dimension's size is no reference either.
        let side = Orient::SideX(a, c, 20.0);
        sk.points[c].pos = Vec2::new(1e-6, 5.0);
        assert_eq!(reference_orientation(&State { sk: &sk, lay: &lay, x: &[] }, side), None);
    }

    #[test]
    fn offsets_have_a_side() {
        let mut sk = Sketch::new();
        let p = sk.add_point(Vec2::new(0.0, 0.0), None).unwrap();
        let q = sk.add_point(Vec2::new(10.0, 5.0), None).unwrap();
        let r = sk.add_point(Vec2::new(0.0, 10.0), None).unwrap();
        let l = sk.add_line(Vec2::new(-5.0, -5.0), Vec2::new(5.0, -5.0), None, None, None).unwrap();
        let lay = Layout { pvar: vec![None; sk.points.len()], rvar: vec![None; sk.curves.len()], n: 0, points: 0, slide: Vec::new() };
        let sign = |sk: &Sketch, o: Orient| orientation(&State { sk, lay: &lay, x: &[] }, o);
        let checks = [Orient::SideX(p, q, 10.0), Orient::SideY(p, q, 5.0), Orient::OfLine(q, l, 10.0), Orient::Turn([p, q, r])];
        let before: Vec<_> = checks.iter().map(|o| sign(&sk, *o)).collect();
        assert!(before.iter().all(Option::is_some), "{before:?}");
        // q mirrored through p and below the line: every side and the turn flip.
        sk.points[q].pos = Vec2::new(-10.0, -15.0);
        let after: Vec<_> = checks.iter().map(|o| sign(&sk, *o)).collect();
        for (b, a) in before.iter().zip(&after) {
            assert_eq!(b.zip(*a).map(|(b, a)| b == a), Some(false), "{before:?} {after:?}");
        }
        // On the line: no side to tell.
        sk.points[q].pos = Vec2::new(0.0, -5.0);
        assert_eq!(sign(&sk, Orient::OfLine(q, l, 10.0)), None);
    }
}

#[cfg(test)]
mod typed_dimension_tests {
    use super::*;

    #[test]
    fn failed_signed_sweep_restores_arc_orientation_and_coordinates() {
        let mut sk = Sketch::new();
        let c = sk.add_arc(Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(0.0, 10.0), [Some(0), None, None], None).unwrap();
        let id = sk.add_constraint(ConstraintKind::ArcSweep { c, value: std::f64::consts::FRAC_PI_2 }, None).unwrap();
        sk.curves[c].fixed = true;
        assert!(solve(&mut sk).ok());
        let before = sk.curves.clone();
        let points = sk.points.clone();
        sk.constraints.iter_mut().find(|constraint| constraint.id == id).unwrap().kind.set_value(-std::f64::consts::FRAC_PI_2);
        assert!(!solve(&mut sk).ok());
        assert_eq!(sk.curves, before);
        assert_eq!(sk.points, points);
    }

    /// A constraint read on a curve of the wrong kind (a midpoint of an arc, pushed past
    /// validation) reads constants for it: its component is tied to them, not floating, and the
    /// pinned analysis agrees with the dense one (checked in every test solve).
    #[test]
    fn a_curve_read_as_another_kind_ties_its_component() {
        let mut sk = Sketch::new();
        let arc = sk.add_arc(Vec2::new(5.0, 5.0), Vec2::new(15.0, 5.0), Vec2::new(5.0, 15.0), [None, None, None], None).unwrap();
        let p = sk.add_point(Vec2::new(3.0, 1.0), None).unwrap();
        sk.constraints.push(crate::model::Constraint {
            id: "m".into(),
            kind: ConstraintKind::Midpoint { p, l: arc },
            param: None,
            driven: false,
            text: None,
        });
        let rep = solve(&mut sk);
        assert!(rep.ok(), "{rep:?}");
        assert!(rep.point_determined[p], "the midpoint of the constant line: {rep:?}");
        assert_eq!(rep.dof, 5, "the arc's centre and ends, less its equal radii: {rep:?}");
    }

    #[test]
    fn a_status_only_solve_places_the_geometry_as_a_full_one() {
        let mut sk = Sketch::new();
        let l = sk.add_line(Vec2::ZERO, Vec2::new(10.0, 3.0), Some(0), None, None).unwrap();
        sk.add_constraint(ConstraintKind::LineAngle { l, value: 0.5 }, None).unwrap();
        let mut quick = sk.clone();
        let full = solve(&mut sk);
        let status = solve_with(&mut quick, &Hold::default(), None, Analysis::StatusOnly, None);
        assert!(full.ok() && status.ok());
        assert_eq!(quick, sk);
        assert_eq!(full.dof, 1, "the end slides along the line");
        assert_eq!((status.dof, status.point_determined.len(), status.curve_determined.len()), (0, 0, 0));
    }

    #[test]
    fn signed_dimensions_validate_kinds_values_and_survive_curve_removal() {
        let mut sk = Sketch::new();
        let l = sk.add_line(Vec2::ZERO, Vec2::new(10.0, 10.0), Some(0), None, None).unwrap();
        assert!(sk.add_constraint(ConstraintKind::ArcSweep { c: l, value: 1.0 }, None).is_err());
        assert!(sk.add_constraint(ConstraintKind::LineAngle { l, value: f64::NAN }, None).is_err());
        sk.add_constraint(ConstraintKind::LineAngle { l, value: -std::f64::consts::FRAC_PI_4 }, None).unwrap();
        assert!(solve(&mut sk).ok());
        let CurveKind::Line { a, b } = sk.curves[l].kind else { panic!("line") };
        let direction = sk.points[b].pos - sk.points[a].pos;
        assert!((direction.angle() + std::f64::consts::FRAC_PI_4).abs() < 1e-7);
        sk.remove_curves(&[l]);
        assert!(sk.constraints.is_empty());
    }

    /// One connected component is solved as a dense system, so its variable count sets the
    /// allocation: 10,000 chained points are 20,000 variables and a 3.2 GB matrix. The solve
    /// has to report failure instead of asking for the memory, because an allocation failure
    /// aborts the process where a failed solve does not.
    #[test]
    fn components_too_large_to_solve_fail_instead_of_allocating() {
        let mut sk = Sketch::new();
        let mut previous = 0;
        for i in 1..10_000 {
            let p = sk.add_point(Vec2::new(0.001 * i as f64, 0.0), None).unwrap();
            sk.add_constraint(ConstraintKind::Coincident { p: previous, q: p }, None).unwrap();
            previous = p;
        }
        let before = sk.points.clone();
        let t = std::time::Instant::now();
        let report = solve(&mut sk);
        assert_eq!(report.status, SolveStatus::Failed);
        // Geometry is left as it was.
        assert_eq!(sk.points, before);
        assert!(t.elapsed() < std::time::Duration::from_secs(10), "took {:?}", t.elapsed());
    }

    /// A sketch whose components each stay under the cap still solves.
    #[test]
    fn many_small_components_still_solve() {
        let mut sk = Sketch::new();
        for i in 0..200 {
            let a = sk.add_point(Vec2::new(i as f64, 0.0), None).unwrap();
            let b = sk.add_point(Vec2::new(i as f64, 5.0), None).unwrap();
            let l = sk.add_line_pts(a, b, None).unwrap();
            sk.add_constraint(ConstraintKind::Vertical { l }, None).unwrap();
        }
        assert!(solve(&mut sk).ok());
    }
}
