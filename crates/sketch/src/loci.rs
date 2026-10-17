//! What the constructive seed reads from a sketch and how it intersects it, shared by the
//! point-by-point placement and the cluster decomposition: the relations driving constraints
//! set between points, their loci, circle ∩ circle, circle ∩ line, line ∩ line, which of two
//! mirror-image intersections keeps the orientation of a reference layout (Owen 1991, §2.4.3
//! and §5), and where two or more loci put a point ([`place`]).
//!
//! [`read`] turns the driving constraints into
//!
//! - **relations between two points** ([`Rel`]): coincidence, distance, line length (also
//!   through equal lines), horizontal and vertical (also through parallel, perpendicular and
//!   collinear lines, and axis-aligned line angles), horizontal and vertical distances; the
//!   radius of an arc (a radius or diameter, also through equal arcs and circles) as distances
//!   from its centre to its ends, a point on a circle of known radius, concentric centres,
//!   tangent circles of known radii (external or internal as in the reference), the centre
//!   distance of a slot of known radii; and, on a horizontal or vertical line, a point on it,
//!   at a distance from it, its midpoint, a point mirrored across it and a tangent circle of
//!   known radius (its centre at the radius from the line, above or beside the touch point).
//!   These are invariant under translation (and rotation unless along an axis), so the
//!   decomposition's clusters are built on them;
//! - **ties** between more points ([`Tie`]), read where a placement has the points they are
//!   anchored on: a point on a line or at a distance from it (a tangent circle's centre, with a
//!   known or arc radius), the radius to a touch point normal to the line, a midpoint, a point
//!   mirrored across a line, equal lengths, an arc's ends at one distance from its centre (and
//!   the centre on their bisector), and a line along a direction derived from a line angle, an
//!   angle to another line, or parallel or perpendicular to it.
//!
//! Signs no residual keeps (horizontal or vertical distances, distances from a line, which
//! side of a line a tangent circle is on, internal or external tangency) come from the
//! reference layout. Ellipses, splines, conics, arc sweeps and lengths, and curvature
//! continuity are not read: the solver handles them.
//!
//! Tolerances are relative to the loci's own size plus the rounding of the coordinates they
//! are computed from, so a construction far from the origin behaves like the same one near it.

use solidcraft_geom::Vec2;

use crate::model::{ConstraintKind, CurveKind, MAX_COORD, Sketch, in_range, radius_in_range};
use crate::union_find::{find, union};

/// Two intersections closer than this (relative to the size of the loci: radii, centre
/// distance) are one touching intersection.
pub(crate) const TANGENT: f64 = 1e-6;
/// A point this close to the line that mirrors two candidates (relative to the size of the
/// loci) is on neither side of it: its branch is unknown.
const ON_MIRROR: f64 = 1e-9;

/// A distance shorter than this (mm) joins two points into one.
const ZERO_LENGTH: f64 = 1e-9;

/// Loci considered per point; more only add pairs that confirm the same position.
pub(crate) const MAX_LOCI: usize = 16;
/// Direction sources read per line.
const MAX_SOURCES: usize = 4;
/// How many lines deep a direction is derived through angles between lines.
const DEPTH: usize = 3;
/// Lines whose ties one placed point wakes through direction dependencies.
const MAX_WOKEN_LINES: usize = 16;

/// Largest absolute coordinate.
pub(crate) fn mag(v: Vec2) -> f64 {
    v.x.abs().max(v.y.abs())
}

/// Rounding error of a position computed from coordinates up to `m` (mm), with headroom for
/// the few operations of an intersection. Independent of the loci's own size.
fn rounding(m: f64) -> f64 {
    8.0 * f64::EPSILON * m
}

/// A length tolerance: `rel` of the loci's own `size` (at least 1 mm), plus the rounding of
/// coordinates up to `coords`.
pub(crate) fn tol(rel: f64, size: f64, coords: f64) -> f64 {
    rel * size.max(1.0) + rounding(coords)
}

/// A point `miss` away from a locus of `size` (coordinates up to `coords`) lies on it.
pub(crate) fn within(miss: f64, size: f64, coords: f64) -> bool {
    miss <= tol(TANGENT, size, coords)
}

/// `v` is clear of zero: farther from it than the `ON_MIRROR` share of `size` plus the
/// rounding of coordinates up to `coords`.
fn clear_of_zero(v: f64, size: f64, coords: f64) -> bool {
    v.abs() > tol(ON_MIRROR, size, coords)
}

/// The sign of an offset `delta` between two points (coordinates up to `coords`) that an
/// offset of `size` keeps; None when it is too close to zero to tell.
pub(crate) fn side(delta: f64, size: f64, coords: f64) -> Option<f64> {
    clear_of_zero(delta, size, coords).then(|| delta.signum())
}

/// How a driving constraint relates two points, as seen from either of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Rel {
    Same,
    SameX,
    SameY,
    Dist(f64),
    /// |q.x − p.x| = d.
    OffsetX(f64),
    /// |q.y − p.y| = d.
    OffsetY(f64),
}

impl Rel {
    /// Sort key, so that a result never depends on the order of the constraints.
    pub(crate) fn key(self) -> (u8, u64) {
        match self {
            Rel::Same => (0, 0),
            Rel::SameX => (1, 0),
            Rel::SameY => (2, 0),
            Rel::Dist(d) => (3, d.to_bits()),
            Rel::OffsetX(d) => (4, d.to_bits()),
            Rel::OffsetY(d) => (5, d.to_bits()),
        }
    }

    /// The two points are one: coincident, or at a distance shorter than [`ZERO_LENGTH`].
    pub(crate) fn joins(self) -> bool {
        match self {
            Rel::Same => true,
            Rel::Dist(d) => d < ZERO_LENGTH,
            _ => false,
        }
    }

    /// Along an axis of the sketch: holds only in a frame oriented as the sketch's.
    fn along_axis(self) -> bool {
        matches!(self, Rel::SameX | Rel::SameY | Rel::OffsetX(_) | Rel::OffsetY(_))
    }

    /// The dimension's value (zero for the relations without one).
    pub(crate) fn size(self) -> f64 {
        match self {
            Rel::Same | Rel::SameX | Rel::SameY => 0.0,
            Rel::Dist(d) | Rel::OffsetX(d) | Rel::OffsetY(d) => d,
        }
    }

    /// Does it hold between `p` and `q`, within the tolerance of a locus of size `scale`
    /// (at least the dimension's own)?
    pub(crate) fn holds(self, p: Vec2, q: Vec2, scale: f64) -> bool {
        let d = q - p;
        let miss = match self {
            Rel::Same => d.len(),
            Rel::SameX => d.x.abs(),
            Rel::SameY => d.y.abs(),
            Rel::Dist(v) => (d.len() - v).abs(),
            Rel::OffsetX(v) => (d.x.abs() - v).abs(),
            Rel::OffsetY(v) => (d.y.abs() - v).abs(),
        };
        within(miss, self.size().max(scale), mag(p).max(mag(q)))
    }

    /// The locus of a point so related to a point at `at`; `side` is the side of an offset
    /// (±1, 0 when unknown). None for a coincidence, or an offset whose side is unknown.
    fn locus(self, at: Vec2, side: f64) -> Option<Locus> {
        Some(match self {
            Rel::Same => return None,
            Rel::SameX => Locus::Line { o: at, d: Vec2::Y },
            Rel::SameY => Locus::Line { o: at, d: Vec2::X },
            Rel::Dist(r) => Locus::Circle { c: at, r },
            // A zero offset needs no side; any other offset needs the side the point was on.
            Rel::OffsetX(d) if d == 0.0 || side != 0.0 => Locus::Line { o: at + Vec2::X * (side * d), d: Vec2::Y },
            Rel::OffsetY(d) if d == 0.0 || side != 0.0 => Locus::Line { o: at + Vec2::Y * (side * d), d: Vec2::X },
            Rel::OffsetX(_) | Rel::OffsetY(_) => return None,
        })
    }

    /// Which side of the anchor `p` was on before, along x or y (0 when unknown or level).
    fn offset_side(self, p: Option<Vec2>, anchor: Option<Vec2>) -> f64 {
        let (Some(p), Some(a)) = (p, anchor) else { return 0.0 };
        let (delta, d) = match self {
            Rel::OffsetX(d) => (p.x - a.x, d),
            Rel::OffsetY(d) => (p.y - a.y, d),
            _ => return 0.0,
        };
        side(delta, d, mag(p).max(mag(a))).unwrap_or(0.0)
    }
}

/// A relation between two points, as stored at each of them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Edge {
    pub p: usize,
    pub q: usize,
    pub rel: Rel,
}

impl Edge {
    pub(crate) fn other(&self, i: usize) -> usize {
        if self.p == i { self.q } else { self.p }
    }
}

/// Where a point can be.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Locus {
    Circle {
        c: Vec2,
        r: f64,
    },
    /// The line through `o` along the unit direction `d`.
    Line {
        o: Vec2,
        d: Vec2,
    },
}

impl Locus {
    /// How far `x` is from the locus.
    pub(crate) fn miss(&self, x: Vec2) -> f64 {
        match *self {
            Locus::Circle { c, r } => (x.dist(c) - r).abs(),
            Locus::Line { o, d } => d.cross(x - o).abs(),
        }
    }
    /// The locus's own size (a line has none).
    pub(crate) fn extent(&self) -> f64 {
        match *self {
            Locus::Circle { r, .. } => r,
            Locus::Line { .. } => 0.0,
        }
    }
    /// A point of the locus's definition (how far from the origin it is sets the rounding).
    fn origin(&self) -> Vec2 {
        match *self {
            Locus::Circle { c, .. } => c,
            Locus::Line { o, .. } => o,
        }
    }
    /// A locus the arithmetic can use: finite, in range, a unit direction or a radius.
    fn checked(self) -> Option<Self> {
        let ok = match self {
            Locus::Circle { c, r } => in_range(c) && r.is_finite() && (0.0..=MAX_COORD).contains(&r),
            Locus::Line { o, d } => in_range(o) && d.is_finite() && (d.len() - 1.0).abs() < 1e-9,
        };
        ok.then_some(self)
    }
}

/// The intersection of two loci.
pub(crate) enum Meet {
    /// Parallel lines or concentric circles: no isolated intersection.
    Degenerate,
    /// The loci do not meet.
    Empty,
    One {
        at: Vec2,
        transversal: bool,
    },
    /// Mirror images across the line of [`mirror`]: `pos` on its left, `neg` on its right.
    Two {
        pos: Vec2,
        neg: Vec2,
    },
}

/// The line (point, unit direction) across which the two intersections of `l1` and `l2` mirror
/// each other.
fn mirror(l1: &Locus, l2: &Locus) -> Option<(Vec2, Vec2)> {
    match (*l1, *l2) {
        (Locus::Circle { c: c1, .. }, Locus::Circle { c: c2, .. }) => Some((c1, (c2 - c1).normalized()?)),
        (Locus::Circle { c, .. }, Locus::Line { d, .. }) | (Locus::Line { d, .. }, Locus::Circle { c, .. }) => Some((c, Vec2::new(d.y, -d.x))),
        (Locus::Line { .. }, Locus::Line { .. }) => None,
    }
}

pub(crate) fn meet(l1: &Locus, l2: &Locus) -> Meet {
    let noise = rounding(mag(l1.origin()).max(mag(l2.origin())));
    // The candidates are mid ± h·n (n to the left of the mirror line). h² is a difference of
    // squares of lengths up to `s`: touching when within the band of its size and of the
    // rounding of the positions.
    let chord = |mid: Vec2, h2: f64, n: Vec2, s: f64| {
        let band = (TANGENT * s).powi(2) + 2.0 * s * noise;
        if !(h2 >= -band) {
            Meet::Empty
        } else if h2 <= band {
            Meet::One { at: mid, transversal: false }
        } else {
            let h = h2.sqrt();
            Meet::Two { pos: mid + n * h, neg: mid - n * h }
        }
    };
    match (*l1, *l2) {
        (Locus::Circle { c: c1, r: r1 }, Locus::Circle { c: c2, r: r2 }) => {
            let d = c2 - c1;
            let dd = d.len();
            if !(dd > tol(TANGENT, r1.max(r2), mag(c1).max(mag(c2)))) {
                return Meet::Degenerate;
            }
            let u = d / dd;
            let along = (r1 * r1 - r2 * r2 + dd * dd) / (2.0 * dd);
            chord(c1 + u * along, r1 * r1 - along * along, u.perp(), r1.max(r2).max(dd).max(1.0))
        }
        (Locus::Circle { c, r }, Locus::Line { o, d }) | (Locus::Line { o, d }, Locus::Circle { c, r }) => {
            let foot = o + d * (c - o).dot(d);
            let off = (c - foot).len2();
            // The mirror line runs along -perp(d), so d points to its left.
            chord(foot, r * r - off, d, r.max(off.sqrt()).max(1.0))
        }
        (Locus::Line { o: o1, d: d1 }, Locus::Line { o: o2, d: d2 }) => {
            let s = d1.cross(d2);
            if s.abs() < 1e-12 {
                return Meet::Degenerate;
            }
            Meet::One { at: o1 + d1 * ((o2 - o1).cross(d2) / s), transversal: true }
        }
    }
}

/// Which side of the mirror line of `l1` and `l2` the point `p` is on: positive on its left.
/// None when unknown or on the line. Built from the loci as they were in the reference layout,
/// it tells which of two intersections keeps the orientation `p` had there.
pub(crate) fn branch(l1: Option<&Locus>, l2: Option<&Locus>, p: Option<Vec2>) -> Option<f64> {
    let (l1, l2, p) = (l1?, l2?, p?);
    let (o, u) = mirror(l1, l2)?;
    let side = u.cross(p - o);
    clear_of_zero(side, l1.extent().max(l2.extent()), mag(p).max(mag(o))).then_some(side)
}

/// The crossing of the loci `now` that keeps the orientation `p` has relative to `then` (the
/// same loci in the reference layout). None when the loci do not cross (a touching
/// intersection pins nothing down well) or the side is unknown.
pub(crate) fn crossing(now: (Locus, Locus), then: Option<(Locus, Locus)>, p: Option<Vec2>) -> Option<Vec2> {
    match meet(&now.0, &now.1) {
        Meet::Two { pos, neg } => {
            let (t1, t2) = then.unzip();
            branch(t1.as_ref(), t2.as_ref(), p).map(|side| if side > 0.0 { pos } else { neg })
        }
        Meet::One { at, transversal: true } => Some(at),
        Meet::One { transversal: false, .. } | Meet::Empty | Meet::Degenerate => None,
    }
}

/// What one relation says about a point.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Shape {
    /// The point is here.
    At(Vec2),
    /// The point is on `locus`; `then` is the same locus in the reference layout (it picks the
    /// branch of an intersection).
    On { locus: Locus, then: Option<Locus> },
}

/// One relation of a point to placed points; `anchor` is the placed point it is anchored on
/// (the lowest, for a relation anchored on several).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Item {
    pub anchor: usize,
    pub shape: Shape,
}

/// Where [`place`] puts a point: at the intersection of two loci (or at a point) anchored on
/// `a` ≤ `b`; `transversal` when the loci cross.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fix {
    pub at: Vec2,
    pub a: usize,
    pub b: usize,
    pub transversal: bool,
}

/// Where the relations `items` put a point that was at `p_before`: every pair of loci is
/// intersected, and a candidate must also lie on every other locus and point. A transversal
/// placement is preferred to a touching one, then the one from the lowest anchors, so the
/// result does not depend on the order of the relations. None when no pair pins the point
/// down, or the relations conflict.
pub(crate) fn place(items: &[Item], p_before: Option<Vec2>) -> Option<Fix> {
    let points: Vec<(usize, Vec2)> = items
        .iter()
        .filter_map(|i| match i.shape {
            Shape::At(x) => Some((i.anchor, x)),
            Shape::On { .. } => None,
        })
        .collect();
    let loci: Vec<(usize, Locus, Option<Locus>)> = items
        .iter()
        .filter_map(|i| match i.shape {
            Shape::On { locus, then } => Some((i.anchor, locus, then)),
            Shape::At(_) => None,
        })
        .collect();
    // Does `x` lie on every point and locus except the ones it was built from?
    let fits = |x: Vec2, from_point: Option<usize>, from_loci: Option<(usize, usize)>, scale: f64| {
        let ok = |miss: f64, size: f64| within(miss, size.max(scale), mag(x));
        points.iter().enumerate().all(|(k, (_, q))| Some(k) == from_point || ok(x.dist(*q), 0.0))
            && loci.iter().enumerate().all(|(k, (_, l, _))| from_loci.is_some_and(|(i, j)| k == i || k == j) || ok(l.miss(x), l.extent()))
    };
    // Best first: transversal, then the lowest anchors.
    let mut best: Option<((bool, (usize, usize)), Vec2)> = None;
    let mut offer = |transversal: bool, a: usize, b: usize, at: Vec2| {
        let key = (!transversal, (a.min(b), a.max(b)));
        if in_range(at) && best.is_none_or(|(k, _)| key < k) {
            best = Some((key, at));
        }
    };
    for (k, (anchor, at)) in points.iter().enumerate() {
        if fits(*at, Some(k), None, 0.0) {
            offer(true, *anchor, *anchor, *at);
        }
    }
    for (i, (a1, l1, t1)) in loci.iter().enumerate() {
        for (j, (a2, l2, t2)) in loci.iter().enumerate().skip(i + 1) {
            let scale = l1.extent().max(l2.extent());
            match meet(l1, l2) {
                Meet::Degenerate => {}
                // The constraints conflict here: the solver reports it.
                Meet::Empty => return None,
                Meet::One { at, transversal } => {
                    if fits(at, None, Some((i, j)), scale) {
                        offer(transversal, *a1, *a2, at);
                    }
                }
                Meet::Two { pos: left, neg: right } => {
                    let at = match (fits(left, None, Some((i, j)), scale), fits(right, None, Some((i, j)), scale)) {
                        (true, false) => Some(left),
                        (false, true) => Some(right),
                        (true, true) => branch(t1.as_ref(), t2.as_ref(), p_before).map(|side| if side > 0.0 { left } else { right }),
                        (false, false) => None,
                    };
                    if let Some(at) = at {
                        offer(true, *a1, *a2, at);
                    }
                }
            }
        }
    }
    best.map(|((touching, (a, b)), at)| Fix { at, a, b, transversal: !touching })
}

/// Turn `d` by `by` radians.
fn turned(d: Vec2, by: f64) -> Vec2 {
    let (s, c) = by.sin_cos();
    Vec2::new(d.x * c - d.y * s, d.x * s + d.y * c)
}

/// `x` mirrored across the line through `o` along the unit direction `d`.
fn reflected(x: Vec2, o: Vec2, d: Vec2) -> Vec2 {
    let w = x - o;
    o + d * (2.0 * w.dot(d)) - w
}

/// The other one of `x` and `y` when `t` is one of them.
fn partner(t: usize, x: usize, y: usize) -> Option<usize> {
    if t == x {
        Some(y)
    } else if t == y {
        Some(x)
    } else {
        None
    }
}

/// The readings of one value: the lowest and highest read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Span {
    pub lo: f64,
    pub hi: f64,
}

impl Span {
    pub(crate) fn add(span: &mut Option<Span>, v: f64) {
        *span = Some(span.map_or(Span { lo: v, hi: v }, |s| Span { lo: s.lo.min(v), hi: s.hi.max(v) }));
    }
    /// The value the readings agree on; None when they disagree.
    pub(crate) fn agreed(self) -> Option<f64> {
        (self.hi - self.lo <= tol(TANGENT, self.hi.abs(), 0.0)).then_some(self.lo)
    }
}

/// Where a line's direction comes from, besides its two placed ends.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Src {
    /// Fixed in the sketch's frame (horizontal, vertical, a line angle): only in a frame
    /// oriented as the sketch's.
    Fixed(Vec2),
    /// Line `from`'s direction turned by `by` (an angle between lines; for parallel and
    /// perpendicular lines the turn of the reference layout).
    Turn { from: usize, by: f64 },
}

impl Src {
    fn key(&self) -> (u8, usize, u64, u64) {
        match *self {
            Src::Fixed(d) => (0, 0, d.x.to_bits(), d.y.to_bits()),
            Src::Turn { from, by } => (1, from, by.to_bits(), 0),
        }
    }
}

/// A length: a value, or the distance between two points once both are placed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Len {
    Value(f64),
    Between(usize, usize),
}

impl Len {
    fn value(self, pos: &dyn Fn(usize) -> Option<Vec2>) -> Option<f64> {
        match self {
            Len::Value(v) => Some(v),
            Len::Between(a, b) => Some(pos(a)?.dist(pos(b)?)),
        }
        .filter(|v| v.is_finite() && *v <= MAX_COORD)
    }
}

/// A relation between more than two points (lines `l` by curve index).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tie {
    /// `p` lies on line `l` (or, for an end of `l`, `l` runs along its known direction).
    OnLine {
        p: usize,
        l: usize,
    },
    /// `p` is `len` from line `l`, on its side in the reference.
    Offset {
        p: usize,
        l: usize,
        len: Len,
    },
    /// `p − e` is normal to line `l` (the radius to a touch point; a point and its mirror).
    Normal {
        p: usize,
        e: usize,
        l: usize,
    },
    Midpoint {
        p: usize,
        a: usize,
        b: usize,
    },
    /// `q` mirrors `p` across line `l`.
    Mirror {
        p: usize,
        q: usize,
        l: usize,
    },
    /// |q1 − p1| = |q2 − p2|.
    SameLength {
        p1: usize,
        q1: usize,
        p2: usize,
        q2: usize,
    },
    /// |b − c| = |a − c| (an arc's ends; a point on an arc).
    Equidistant {
        c: usize,
        a: usize,
        b: usize,
    },
}

impl Tie {
    fn key(&self) -> [u64; 6] {
        let u = |i: usize| i as u64;
        let len = |l: Len| match l {
            Len::Value(v) => (0, v.to_bits(), 0),
            Len::Between(a, b) => (1, u(a), u(b)),
        };
        match *self {
            Tie::OnLine { p, l } => [0, u(p), u(l), 0, 0, 0],
            Tie::Offset { p, l, len: n } => {
                let (k, a, b) = len(n);
                [1, u(p), u(l), k, a, b]
            }
            Tie::Normal { p, e, l } => [2, u(p), u(e), u(l), 0, 0],
            Tie::Midpoint { p, a, b } => [3, u(p), u(a), u(b), 0, 0],
            Tie::Mirror { p, q, l } => [4, u(p), u(q), u(l), 0, 0],
            Tie::SameLength { p1, q1, p2, q2 } => [5, u(p1), u(q1), u(p2), u(q2), 0],
            Tie::Equidistant { c, a, b } => [6, u(c), u(a), u(b), 0, 0],
        }
    }

    /// The line whose direction it reads.
    fn line(&self) -> Option<usize> {
        match *self {
            Tie::OnLine { l, .. } | Tie::Offset { l, .. } | Tie::Normal { l, .. } | Tie::Mirror { l, .. } => Some(l),
            _ => None,
        }
    }

    /// The points it relates, given the lines' ends.
    fn points(&self, ends: &dyn Fn(usize) -> Option<(usize, usize)>) -> Vec<usize> {
        let line = |l: usize| ends(l).map(|(a, b)| [a, b]);
        let mut v: Vec<usize> = match *self {
            Tie::OnLine { p, l } => [p].into_iter().chain(line(l).into_iter().flatten()).collect(),
            Tie::Offset { p, l, len } => {
                let between = match len {
                    Len::Value(_) => None,
                    Len::Between(a, b) => Some([a, b]),
                };
                [p].into_iter().chain(line(l).into_iter().flatten()).chain(between.into_iter().flatten()).collect()
            }
            Tie::Normal { p, e: q, l } | Tie::Mirror { p, q, l } => [p, q].into_iter().chain(line(l).into_iter().flatten()).collect(),
            Tie::Midpoint { p, a, b } | Tie::Equidistant { c: p, a, b } => vec![p, a, b],
            Tie::SameLength { p1, q1, p2, q2 } => vec![p1, q1, p2, q2],
        };
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// A circle or arc: its centre, and an arc's ends.
#[derive(Clone, Copy, Debug)]
struct Round {
    c: usize,
    ends: Option<(usize, usize)>,
}

/// Everything the seed reads from a sketch, with the reference layout that picks branches
/// and signs.
pub(crate) struct Reading {
    /// Relations between two points.
    pub rels: Vec<(usize, usize, Rel)>,
    /// Per point: its relations, sorted by the other point (independent of constraint order).
    adj: Vec<Vec<Edge>>,
    ties: Vec<Tie>,
    /// Per tie: the points it relates (line ends included).
    tie_points: Vec<Vec<usize>>,
    /// Per point: the ties it takes part in.
    ties_of: Vec<Vec<usize>>,
    /// Per curve: a line's ends.
    lines: Vec<Option<(usize, usize)>>,
    /// Per curve: a line's direction sources.
    sources: Vec<Vec<Src>>,
    /// Per curve: the lines whose direction derives from it.
    dependents: Vec<Vec<usize>>,
    /// Per curve: the ties that read its direction.
    using: Vec<Vec<usize>>,
    /// Per point: the lines it ends.
    lines_of: Vec<Vec<usize>>,
    /// Per point: its position in the reference layout.
    before: Vec<Option<Vec2>>,
}

/// The value each group of `m` items agrees on: `direct` values per item, groups joined by
/// `equal`. A group whose values disagree has none.
fn agreed(m: usize, direct: &[(usize, f64)], equal: &[(usize, usize)]) -> Vec<Option<f64>> {
    let mut parent: Vec<usize> = (0..m).collect();
    for &(a, b) in equal {
        if a < m && b < m {
            union(&mut parent, a, b);
        }
    }
    let mut span: Vec<Option<Span>> = vec![None; m];
    for &(i, v) in direct {
        let root = find(&mut parent, i);
        if let Some(s) = span.get_mut(root) {
            Span::add(s, v);
        }
    }
    (0..m).map(|i| span.get(find(&mut parent, i)).copied().flatten().and_then(Span::agreed)).collect()
}

/// A line's level: horizontal (`Some(true)`) or vertical (`Some(false)`) when `angle` is a
/// multiple of a quarter turn.
fn level_at(angle: f64) -> Option<bool> {
    if angle.sin().abs() < 1e-12 {
        Some(true)
    } else if angle.cos().abs() < 1e-12 {
        Some(false)
    } else {
        None
    }
}

/// Per line: horizontal (`Some(true)`) or vertical (`Some(false)`), from horizontal, vertical
/// and axis-aligned line angle constraints carried through parallel, collinear (same),
/// perpendicular (swapped) lines and lines at a multiple of a quarter turn. In a group of
/// lines whose readings contradict each other, each line has only its own readings (when they
/// agree). A line with a line angle has none: a row or column has no direction, and the angle
/// gives the line its own (see `Src::Fixed`).
fn levels(lines: &[Option<(usize, usize)>], cons: &[&ConstraintKind]) -> Vec<Option<bool>> {
    use ConstraintKind::*;
    let m = lines.len();
    let is_line = |l: usize| lines.get(l).is_some_and(Option::is_some);
    let mut adj: Vec<Vec<(usize, bool)>> = vec![Vec::new(); m];
    let mut seeds: Vec<Vec<bool>> = vec![Vec::new(); m];
    let mut angled = vec![false; m];
    for k in cons {
        if let LineAngle { l, .. } = **k
            && let Some(a) = angled.get_mut(l)
        {
            *a = true;
        }
        let (pair, seed) = match **k {
            Parallel { a, b } | Collinear { a, b } => (Some((a, b, false)), None),
            Perpendicular { a, b } => (Some((a, b, true)), None),
            Angle { a, b, value, .. } if value.is_finite() => (level_at(value).map(|same| (a, b, !same)), None),
            Horizontal { l } => (None, Some((l, true))),
            Vertical { l } => (None, Some((l, false))),
            LineAngle { l, value } if value.is_finite() => (None, level_at(value).map(|row| (l, row))),
            _ => (None, None),
        };
        if let Some((a, b, swap)) = pair.filter(|(a, b, _)| a != b && is_line(*a) && is_line(*b)) {
            for (x, y) in [(a, b), (b, a)] {
                if let Some(list) = adj.get_mut(x) {
                    list.push((y, swap));
                }
            }
        }
        if let Some((l, row)) = seed.filter(|(l, _)| is_line(*l))
            && let Some(s) = seeds.get_mut(l)
        {
            s.push(row);
        }
    }
    // Per component (breadth first from its lowest line): each line's parity relative to the
    // root, and the root's level every seed agrees on.
    let mut parity: Vec<Option<bool>> = vec![None; m];
    let mut out = vec![None; m];
    for root in 0..m {
        if parity.get(root).is_some_and(Option::is_some) || !is_line(root) {
            continue;
        }
        let mut comp = vec![root];
        if let Some(p) = parity.get_mut(root) {
            *p = Some(false);
        }
        let (mut consistent, mut level): (bool, Option<Option<bool>>) = (true, None);
        let mut k = 0;
        while let Some(&x) = comp.get(k) {
            k += 1;
            let px = parity.get(x).copied().flatten().unwrap_or(false);
            for &s in seeds.get(x).into_iter().flatten() {
                let at_root = s != px;
                level = match level {
                    None => Some(Some(at_root)),
                    Some(Some(v)) if v == at_root => Some(Some(v)),
                    Some(_) => Some(None),
                };
            }
            for &(y, swap) in adj.get(x).into_iter().flatten() {
                match parity.get(y).copied().flatten() {
                    Some(py) => consistent &= py == (px != swap),
                    None => {
                        if let Some(p) = parity.get_mut(y) {
                            *p = Some(px != swap);
                        }
                        comp.push(y);
                    }
                }
            }
        }
        for x in comp {
            let px = parity.get(x).copied().flatten().unwrap_or(false);
            if let Some(o) = out.get_mut(x)
                && !angled.get(x).copied().unwrap_or(false)
            {
                *o = match level {
                    Some(Some(at_root)) if consistent => Some(at_root != px),
                    // A contradiction: the line's own readings, when they agree.
                    _ => seeds.get(x).and_then(|s| s.first().copied().filter(|f| s.iter().all(|v| v == f))),
                };
            }
        }
    }
    out
}

/// A circle or arc of `sk` (with valid point indices).
fn round_of(sk: &Sketch, k: usize) -> Option<Round> {
    let n = sk.points.len();
    match sk.curves.get(k)?.kind {
        CurveKind::Circle { c, .. } if c < n => Some(Round { c, ends: None }),
        CurveKind::Arc { c, a, b } if c < n && a < n && b < n && c != a && c != b => Some(Round { c, ends: Some((a, b)) }),
        _ => None,
    }
}

/// Per curve: a circle's or arc's radius as the driving dimensions set it (also through equal
/// circles and arcs), or a fixed or projected circle's own.
pub(crate) fn radii(sk: &Sketch) -> Vec<Option<f64>> {
    use ConstraintKind::*;
    let mut radii = Vec::new();
    let mut equal = Vec::new();
    for (k, c) in sk.curves.iter().enumerate() {
        if let CurveKind::Circle { r, .. } = c.kind
            && (c.fixed || c.link.is_some())
            && radius_in_range(r)
        {
            radii.push((k, r));
        }
    }
    let round = |k: usize| round_of(sk, k).is_some();
    for c in sk.constraints.iter().filter(|c| !c.driven) {
        match c.kind {
            Radius { c, value } if round(c) && radius_in_range(value) => radii.push((c, value)),
            Diameter { c, value } if round(c) && radius_in_range(value / 2.0) => radii.push((c, value / 2.0)),
            Equal { a, b } if round(a) && round(b) => equal.push((a, b)),
            _ => {}
        }
    }
    agreed(sk.curves.len(), &radii, &equal)
}

/// The unit direction of the line `ends` at the positions `pos`.
fn direction(ends: Option<(usize, usize)>, pos: &dyn Fn(usize) -> Option<Vec2>) -> Option<Vec2> {
    let (a, b) = ends?;
    (pos(b)? - pos(a)?).normalized()
}

/// Read the driving constraints of `sk`; `before(i)` is point `i`'s position in the reference
/// layout (`None`, or out of range: unknown, and whatever it would decide is not read);
/// `on_curve` the sketch's point-on-curve pairs ([`crate::solver::points_on_curves`]), `radius`
/// its radii ([`radii`]).
///
/// Every constraint on a line is read as a tie; a tie on a horizontal or vertical line is
/// then lowered to the relations between two points it amounts to (a row, a column, an offset
/// across it), which the decomposition builds clusters on.
pub(crate) fn read(sk: &Sketch, before: &dyn Fn(usize) -> Option<Vec2>, on_curve: &[(usize, usize)], radius: &[Option<f64>]) -> Reading {
    use ConstraintKind::*;
    let n = sk.points.len();
    let m = sk.curves.len();
    let before: Vec<Option<Vec2>> = (0..n).map(|i| before(i).filter(|p| in_range(*p))).collect();
    let rf = |i: usize| before.get(i).copied().flatten();
    let lines: Vec<Option<(usize, usize)>> = sk
        .curves
        .iter()
        .map(|c| match c.kind {
            CurveKind::Line { a, b } if a < n && b < n && a != b => Some((a, b)),
            _ => None,
        })
        .collect();
    let rounds: Vec<Option<Round>> = (0..m).map(|k| round_of(sk, k)).collect();
    let line = |l: usize| lines.get(l).copied().flatten();
    let round = |k: usize| rounds.get(k).copied().flatten();
    let cons: Vec<&ConstraintKind> = sk.constraints.iter().filter(|c| !c.driven).map(|c| &c.kind).collect();
    let dim = |v: f64| (v.is_finite() && (0.0..=MAX_COORD).contains(&v)).then_some(v);

    // Radii and lengths the dimensions set, also through equal curves.
    let mut lengths = Vec::new();
    let mut equal_lines = Vec::new();
    for k in &cons {
        match **k {
            Length { l, value } if line(l).is_some() => lengths.extend(dim(value).map(|v| (l, v))),
            Equal { a, b } if line(a).is_some() && line(b).is_some() => equal_lines.push((a, b)),
            _ => {}
        }
    }
    let length = agreed(m, &lengths, &equal_lines);
    let level = levels(&lines, &cons);
    let r_of = |k: usize| radius.get(k).copied().flatten();
    let level_of = |l: usize| level.get(l).copied().flatten();
    let ref_dir = |l: usize| direction(line(l), &rf);

    let mut rels = Vec::new();
    let mut ties = Vec::new();
    let mut rel = |p: usize, q: usize, r: Rel| {
        if p != q && p < n && q < n {
            rels.push((p, q, r));
        }
    };
    for k in &cons {
        match **k {
            Coincident { p, q } => rel(p, q, Rel::Same),
            HorizontalPoints { p, q } => rel(p, q, Rel::SameY),
            VerticalPoints { p, q } => rel(p, q, Rel::SameX),
            Distance { p, q, value } | DistanceX { p, q, value } | DistanceY { p, q, value } => {
                if let Some(d) = dim(value) {
                    let r = match **k {
                        DistanceX { .. } => Rel::OffsetX(d),
                        DistanceY { .. } => Rel::OffsetY(d),
                        _ => Rel::Dist(d),
                    };
                    rel(p, q, r);
                }
            }
            PointOnCurve { p, c } if p < n => {
                if line(c).is_some() {
                    ties.push(Tie::OnLine { p, l: c });
                } else if let Some(rd) = round(c) {
                    match (r_of(c), rd.ends) {
                        (Some(r), _) => rel(rd.c, p, Rel::Dist(r)),
                        (None, Some((a, b))) if p != a && p != b => ties.push(Tie::Equidistant { c: rd.c, a, b: p }),
                        _ => {}
                    }
                }
            }
            PointLineDistance { p, l, value } | LinearDiameter { p, l, value } if p < n && line(l).is_some() => {
                let d = if matches!(**k, LinearDiameter { .. }) { value / 2.0 } else { value };
                if let Some(d) = dim(d) {
                    ties.push(Tie::Offset { p, l, len: Len::Value(d) });
                }
            }
            // The centre lies radius ± the gap from the line, on its side in the reference.
            LineCircleDistance { l, c, value, inside } if line(l).is_some() => {
                if let (Some(rd), Some(r)) = (round(c), r_of(c))
                    && let Some(d) = dim(if inside { r - value } else { r + value })
                {
                    ties.push(Tie::Offset { p: rd.c, l, len: Len::Value(d) });
                }
            }
            Tangent { a, b } => {
                let (l, curve) = if line(a).is_some() { (a, b) } else { (b, a) };
                if line(l).is_some()
                    && let Some(rd) = round(curve)
                {
                    if let Some(len) = r_of(curve).map(Len::Value).or(rd.ends.map(|(s, _)| Len::Between(rd.c, s))) {
                        ties.push(Tie::Offset { p: rd.c, l, len });
                    }
                    // The radius to the touch point is normal to the line.
                    if let Some(e) = crate::solver::touch_point(sk, k, on_curve).filter(|e| *e < n && *e != rd.c) {
                        ties.push(Tie::Normal { p: rd.c, e, l });
                    }
                } else if let (Some(r1), Some(r2), Some(k1), Some(k2)) = (r_of(a), r_of(b), round(a), round(b)) {
                    // External or internal, as the residual picks it on the reference.
                    let ref_r = |k: usize, rd: Round| match (sk.curves.get(k).map(|c| &c.kind), rd.ends) {
                        (Some(CurveKind::Circle { r, .. }), _) => Some(*r),
                        (_, Some((s, _))) => Some(rf(rd.c)?.dist(rf(s)?)),
                        _ => None,
                    };
                    if let (Some(c1), Some(c2), Some(q1), Some(q2)) = (rf(k1.c), rf(k2.c), ref_r(a, k1), ref_r(b, k2)) {
                        let d = c1.dist(c2);
                        let external = (d - (q1 + q2)).abs() <= (d - (q1 - q2).abs()).abs();
                        rel(k1.c, k2.c, Rel::Dist(if external { r1 + r2 } else { (r1 - r2).abs() }));
                    }
                }
            }
            Concentric { a, b } => {
                if let (Some(k1), Some(k2)) = (round(a), round(b)) {
                    rel(k1.c, k2.c, Rel::Same);
                }
            }
            SlotOverallLength { a, b, value } => {
                if let (Some(k1), Some(k2), Some(r1), Some(r2)) = (round(a), round(b), r_of(a), r_of(b))
                    && let Some(d) = dim(value - r1 - r2)
                {
                    rel(k1.c, k2.c, Rel::Dist(d));
                }
            }
            Midpoint { p, l } if p < n => {
                if let Some((a, b)) = line(l).filter(|(a, b)| p != *a && p != *b) {
                    ties.push(Tie::OnLine { p, l });
                    ties.push(Tie::Midpoint { p, a, b });
                }
            }
            Symmetric { p, q, l } if p < n && q < n && p != q && line(l).is_some() => {
                ties.push(Tie::Normal { p, e: q, l });
                ties.push(Tie::Mirror { p, q, l });
            }
            Collinear { a, b } => {
                if let (Some((a0, a1)), Some((b0, b1))) = (line(a), line(b)) {
                    for (p, l) in [(b0, a), (b1, a), (a0, b), (a1, b)] {
                        ties.push(Tie::OnLine { p, l });
                    }
                }
            }
            Equal { a, b } => {
                if let (Some((a0, a1)), Some((b0, b1))) = (line(a), line(b)) {
                    if length.get(a).copied().flatten().is_none() {
                        ties.push(Tie::SameLength { p1: a0, q1: a1, p2: b0, q2: b1 });
                    }
                } else if let (Some(Round { c: c1, ends: Some((s1, _)) }), Some(Round { c: c2, ends: Some((s2, _)) })) = (round(a), round(b))
                    && r_of(a).is_none()
                {
                    ties.push(Tie::SameLength { p1: c1, q1: s1, p2: c2, q2: s2 });
                }
            }
            _ => {}
        }
    }
    for (l, ends) in lines.iter().enumerate() {
        if let (Some((a, b)), Some(d)) = (*ends, length.get(l).copied().flatten()) {
            rel(a, b, Rel::Dist(d));
        }
    }
    for (k, rd) in rounds.iter().enumerate() {
        let Some(Round { c, ends: Some((a, b)) }) = *rd else { continue };
        match r_of(k) {
            Some(r) => {
                rel(c, a, Rel::Dist(r));
                rel(c, b, Rel::Dist(r));
            }
            None if a != b => ties.push(Tie::Equidistant { c, a, b }),
            None => {}
        }
    }

    // Line directions: fixed ones, and turns from other lines.
    let mut sources: Vec<Vec<Src>> = vec![Vec::new(); m];
    {
        // The turn from `from` to `l` among `base` and `base + π` the reference has (`base`
        // when it has none).
        let turn = |from: usize, l: usize, base: f64| match (ref_dir(from), ref_dir(l)) {
            (Some(f), Some(d)) if turned(f, base).dot(d) < 0.0 => base + std::f64::consts::PI,
            _ => base,
        };
        let mut add = |l: usize, s: Src| {
            if let Some(list) = sources.get_mut(l) {
                list.push(s);
            }
        };
        for k in &cons {
            match **k {
                LineAngle { l, value } if line(l).is_some() && value.is_finite() => add(l, Src::Fixed(Vec2::from_angle(value))),
                Angle { a, b, value, flip } if a != b && line(a).is_some() && line(b).is_some() && value.is_finite() && value.abs() <= MAX_COORD => {
                    let by = value + if flip { std::f64::consts::PI } else { 0.0 };
                    add(b, Src::Turn { from: a, by });
                    add(a, Src::Turn { from: b, by: -by });
                }
                Parallel { a, b } | Collinear { a, b } | Perpendicular { a, b } if a != b && line(a).is_some() && line(b).is_some() => {
                    let base = if matches!(**k, Perpendicular { .. }) { std::f64::consts::FRAC_PI_2 } else { 0.0 };
                    add(b, Src::Turn { from: a, by: turn(a, b, base) });
                    add(a, Src::Turn { from: b, by: turn(b, a, base) });
                }
                _ => {}
            }
        }
        // A row or column gives no sign: the reference's (a line with a line angle has its
        // own, and no level).
        for l in 0..m {
            if let Some(row) = level_of(l) {
                let axis = if row { Vec2::X } else { Vec2::Y };
                let d = if ref_dir(l).is_some_and(|d| d.dot(axis) < 0.0) { -axis } else { axis };
                add(l, Src::Fixed(d));
            }
        }
    }
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); m];
    for (l, list) in sources.iter_mut().enumerate() {
        list.sort_by_key(Src::key);
        list.dedup();
        list.truncate(MAX_SOURCES);
        // A line with a direction runs along it from either end.
        if let (Some((_, b)), false) = (line(l), list.is_empty()) {
            ties.push(Tie::OnLine { p: b, l });
        }
        for s in list.iter() {
            if let Src::Turn { from, .. } = s
                && let Some(d) = dependents.get_mut(*from)
            {
                d.push(l);
            }
        }
    }
    for d in &mut dependents {
        d.sort_unstable();
        d.dedup();
    }

    // Ties on a horizontal or vertical line are relations between two points: a row or
    // column through the line's ends, an offset across it, a column or row normal to it.
    let lowered = |row: bool, d: Option<f64>| match (row, d) {
        (true, None) => Rel::SameY,
        (false, None) => Rel::SameX,
        (true, Some(d)) => Rel::OffsetY(d),
        (false, Some(d)) => Rel::OffsetX(d),
    };
    ties.retain(|t| {
        let Some(((a, b), row)) = t.line().and_then(|l| Some((line(l)?, level_of(l)?))) else { return true };
        match *t {
            Tie::OnLine { p, .. } => [a, b].into_iter().for_each(|q| rel(q, p, lowered(row, None))),
            Tie::Offset { p, len: Len::Value(d), .. } => [a, b].into_iter().for_each(|q| rel(q, p, lowered(row, Some(d)))),
            Tie::Normal { p, e, .. } => rel(e, p, lowered(!row, None)),
            Tie::Offset { .. } | Tie::Mirror { .. } | Tie::Midpoint { .. } | Tie::SameLength { .. } | Tie::Equidistant { .. } => return true,
        }
        false
    });

    ties.sort_by_key(Tie::key);
    ties.dedup();
    let tie_points: Vec<Vec<usize>> = ties.iter().map(|t| t.points(&line).into_iter().filter(|p| *p < n).collect()).collect();
    let mut ties_of = vec![Vec::new(); n];
    let mut using = vec![Vec::new(); m];
    for (i, (t, pts)) in ties.iter().zip(&tie_points).enumerate() {
        for p in pts {
            if let Some(list) = ties_of.get_mut(*p) {
                list.push(i);
            }
        }
        if let Some(list) = t.line().and_then(|l| using.get_mut(l)) {
            list.push(i);
        }
    }
    let mut lines_of = vec![Vec::new(); n];
    for (l, ends) in lines.iter().enumerate() {
        for p in ends.iter().flat_map(|(a, b)| [*a, *b]) {
            if let Some(list) = lines_of.get_mut(p) {
                list.push(l);
            }
        }
    }
    let mut adj = vec![Vec::new(); n];
    for &(p, q, rel) in &rels {
        for end in [p, q] {
            if let Some(list) = adj.get_mut(end) {
                list.push(Edge { p, q, rel });
            }
        }
    }
    for (i, list) in adj.iter_mut().enumerate() {
        list.sort_by_key(|e: &Edge| (e.other(i), e.rel.key()));
    }
    Reading { rels, adj, ties, tie_points, ties_of, lines, sources, dependents, using, lines_of, before }
}

/// The points as one frame has them: `at(i)` the position of point `i` (None while
/// unplaced), `oriented` when the frame is turned as the sketch is (directions fixed in the
/// sketch hold).
pub(crate) struct View<'a> {
    pub at: &'a dyn Fn(usize) -> Option<Vec2>,
    pub oriented: bool,
}

impl Reading {
    /// How many points the sketch has.
    pub(crate) fn points(&self) -> usize {
        self.adj.len()
    }

    /// How many ties it read.
    pub(crate) fn tie_count(&self) -> usize {
        self.ties.len()
    }

    /// The relations of point `i`.
    pub(crate) fn adj(&self, i: usize) -> &[Edge] {
        self.adj.get(i).map_or(&[][..], Vec::as_slice)
    }

    /// How many ties point `i` takes part in.
    pub(crate) fn tie_count_of(&self, i: usize) -> usize {
        self.ties_of.get(i).map_or(0, Vec::len)
    }

    pub(crate) fn before(&self, i: usize) -> Option<Vec2> {
        self.before.get(i).copied().flatten()
    }

    /// The point lists of every tie.
    pub(crate) fn tie_point_lists(&self) -> impl Iterator<Item = &[usize]> {
        self.tie_points.iter().map(Vec::as_slice)
    }

    /// The points related to `i` by a relation.
    pub(crate) fn related(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.adj(i).iter().map(move |e| e.other(i))
    }

    /// The points related to `i` (by a relation or a tie).
    pub(crate) fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let ties = self.ties_of.get(i).into_iter().flatten().flat_map(|t| self.tie_points.get(*t).into_iter().flatten().copied());
        self.related(i).chain(ties).filter(move |o| *o != i)
    }

    /// The points a tie may now place once `p` is placed, each once: the points of its ties,
    /// and of the ties reading the direction of a line it ends or of a line whose direction
    /// derives from one (at most [`MAX_WOKEN_LINES`] lines, [`DEPTH`] deep).
    pub(crate) fn woken(&self, p: usize) -> Vec<usize> {
        let mut lines: Vec<usize> = self.lines_of.get(p).cloned().unwrap_or_default();
        let mut start = 0;
        'deeper: for _ in 0..DEPTH {
            let end = lines.len();
            for k in start..end {
                let Some(l) = lines.get(k).copied() else { break };
                for d in self.dependents.get(l).into_iter().flatten() {
                    if lines.len() >= MAX_WOKEN_LINES {
                        break 'deeper;
                    }
                    if !lines.contains(d) {
                        lines.push(*d);
                    }
                }
            }
            start = end;
        }
        let ties = self.ties_of.get(p).into_iter().flatten().chain(lines.iter().flat_map(|l| self.using.get(*l).into_iter().flatten()));
        let mut out: Vec<usize> = ties.flat_map(|t| self.tie_points.get(*t).into_iter().flatten().copied()).filter(|o| *o != p).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The relations of `t` to points placed in `view`: up to [`MAX_LOCI`] items, relations
    /// between two points first.
    pub(crate) fn items(&self, t: usize, view: &View) -> Vec<Item> {
        let mut out = Vec::new();
        let t_before = self.before(t);
        for e in self.adj(t) {
            if out.len() >= MAX_LOCI {
                return out;
            }
            if !view.oriented && e.rel.along_axis() {
                continue;
            }
            let anchor = e.other(t);
            let Some(at) = (view.at)(anchor) else { continue };
            if let Rel::Same = e.rel {
                out.push(Item { anchor, shape: Shape::At(at) });
                continue;
            }
            let side = e.rel.offset_side(t_before, self.before(anchor));
            let Some(locus) = e.rel.locus(at, side) else { continue };
            let then = self.before(anchor).and_then(|b| e.rel.locus(b, side));
            out.push(Item { anchor, shape: Shape::On { locus, then } });
        }
        for ti in self.ties_of.get(t).into_iter().flatten() {
            if out.len() >= MAX_LOCI {
                break;
            }
            if let Some(item) = self.ties.get(*ti).and_then(|tie| self.tie_item(tie, t, view)) {
                out.push(item);
            }
        }
        out
    }

    /// Does `t` at `x` hold every relation and tie to the points placed in `view`?
    pub(crate) fn holds(&self, t: usize, x: Vec2, view: &View) -> bool {
        let pos = |i: usize| if i == t { Some(x) } else { (view.at)(i) };
        self.adj(t).iter().all(|e| (view.at)(e.other(t)).is_none_or(|q| e.rel.holds(x, q, 0.0)))
            && self.ties_of.get(t).into_iter().flatten().filter_map(|ti| self.ties.get(*ti)).all(|tie| self.tie_holds(tie, &pos))
    }

    fn ends(&self, l: usize) -> Option<(usize, usize)> {
        self.lines.get(l).copied().flatten()
    }

    /// Line `l`'s unit direction in the reference layout.
    fn ref_dir(&self, l: usize) -> Option<Vec2> {
        direction(self.ends(l), &|i| self.before(i))
    }

    /// Line `l`'s unit direction in `view`: from its placed ends, or derived from its sources
    /// (at most `depth` lines deep).
    fn dir(&self, l: usize, view: &View, depth: usize) -> Option<Vec2> {
        if let Some(d) = direction(self.ends(l), view.at) {
            return Some(d);
        }
        if depth == 0 {
            return None;
        }
        self.sources.get(l)?.iter().find_map(|s| match *s {
            Src::Fixed(d) => view.oriented.then_some(d),
            Src::Turn { from, by } => self.dir(from, view, depth - 1).map(|d| turned(d, by)),
        })
    }

    /// A placed point of line `l` other than `t` (its first end first).
    fn line_point(&self, l: usize, t: usize, view: &View) -> Option<(usize, Vec2)> {
        let (a, b) = self.ends(l)?;
        [a, b].into_iter().filter(|p| *p != t).find_map(|p| (view.at)(p).map(|at| (p, at)))
    }

    /// The item tie `tie` gives point `t` from the points placed in `view`.
    fn tie_item(&self, tie: &Tie, t: usize, view: &View) -> Option<Item> {
        let at = |i: usize| (view.at)(i);
        let rf = |i: usize| self.before(i);
        let line = |o: Vec2, d: Vec2| Locus::Line { o, d }.checked();
        let circle = |c: Vec2, r: f64| Locus::Circle { c, r }.checked();
        let on = |anchor: usize, locus: Option<Locus>, then: Option<Locus>| locus.map(|locus| Item { anchor, shape: Shape::On { locus, then } });
        let point = |anchor: usize, x: Vec2| in_range(x).then_some(Item { anchor, shape: Shape::At(x) });
        match *tie {
            Tie::OnLine { p, l } => {
                let (a, b) = self.ends(l)?;
                // `p` along the line from a placed end, or an end along the line from `p`.
                let (anchor, o) = if t == p {
                    self.line_point(l, t, view)?
                } else if t == a || t == b {
                    (p, at(p)?)
                } else {
                    return None;
                };
                let d = self.dir(l, view, DEPTH)?;
                on(anchor, line(o, d), rf(anchor).zip(self.ref_dir(l)).and_then(|(o, d)| line(o, d)))
            }
            Tie::Offset { p, l, len } => {
                let (a, b) = self.ends(l)?;
                // `p` is at `s·v` along the line's left normal from it.
                let (anchor, o, sign) = if t == p {
                    let (q, o) = self.line_point(l, t, view)?;
                    (q, o, 1.0)
                } else if t == a || t == b {
                    (p, at(p)?, -1.0)
                } else {
                    return None;
                };
                let v = len.value(&at)?;
                let rd = self.ref_dir(l)?;
                let (pb, ab) = (rf(p)?, rf(a)?);
                let s = if v == 0.0 { 1.0 } else { side(rd.cross(pb - ab), v, mag(pb).max(mag(ab)))? };
                let d = self.dir(l, view, DEPTH)?;
                let shift = sign * s * v;
                on(anchor, line(o + d.perp() * shift, d), line(rf(anchor)? + rd.perp() * shift, rd))
            }
            Tie::Normal { p, e, l } => {
                let anchor = partner(t, p, e)?;
                let o = at(anchor)?;
                let d = self.dir(l, view, DEPTH)?.perp();
                let then = rf(anchor).zip(self.ref_dir(l)).and_then(|(o, d)| line(o, d.perp()));
                on(anchor, line(o, d), then)
            }
            Tie::Midpoint { p, a, b } => {
                if t == p {
                    point(a.min(b), (at(a)? + at(b)?) * 0.5)
                } else {
                    let other = partner(t, a, b)?;
                    point(p.min(other), at(p)? * 2.0 - at(other)?)
                }
            }
            Tie::Mirror { p, q, l } => {
                let from = partner(t, p, q)?;
                let x = at(from)?;
                let (_, o) = self.line_point(l, t, view)?;
                point(from, reflected(x, o, self.dir(l, view, DEPTH)?))
            }
            Tie::SameLength { p1, q1, p2, q2 } => {
                let (centre, (x, y)) = match (partner(t, p2, q2), partner(t, p1, q1)) {
                    (Some(c), None) => (c, (p1, q1)),
                    (None, Some(c)) => (c, (p2, q2)),
                    _ => return None,
                };
                let r = at(x)?.dist(at(y)?);
                on(centre, circle(at(centre)?, r), rf(centre).and_then(|c| circle(c, r)))
            }
            Tie::Equidistant { c, a, b } => {
                if t == c {
                    let (pa, pb) = (at(a)?, at(b)?);
                    let n = (pb - pa).normalized()?.perp();
                    let then = rf(a).zip(rf(b)).and_then(|(ra, rb)| line((ra + rb) * 0.5, (rb - ra).normalized()?.perp()));
                    on(a.min(b), line((pa + pb) * 0.5, n), then)
                } else {
                    let other = partner(t, a, b)?;
                    let r = at(other)?.dist(at(c)?);
                    on(c, circle(at(c)?, r), rf(c).and_then(|x| circle(x, r)))
                }
            }
        }
    }

    /// Does `tie` hold at the positions `pos` (true when a point it needs is unknown)?
    fn tie_holds(&self, tie: &Tie, pos: &dyn Fn(usize) -> Option<Vec2>) -> bool {
        let check = |miss: f64, size: f64, pts: &[Vec2]| within(miss, size, pts.iter().fold(0.0, |m, p| m.max(mag(*p))));
        let base = |l: usize| self.ends(l).and_then(|(a, b)| Some((pos(a)?, direction(Some((a, b)), pos)?)));
        match *tie {
            Tie::OnLine { p, l } => match (pos(p), base(l)) {
                (Some(x), Some((a, d))) => check(d.cross(x - a).abs(), 0.0, &[x, a]),
                _ => true,
            },
            Tie::Offset { p, l, len } => match (pos(p), base(l), len.value(pos)) {
                (Some(x), Some((a, d)), Some(v)) => check((d.cross(x - a).abs() - v).abs(), v, &[x, a]),
                _ => true,
            },
            Tie::Normal { p, e, l } => match (pos(p), pos(e), base(l)) {
                (Some(x), Some(y), Some((_, d))) => check((x - y).dot(d).abs(), x.dist(y), &[x, y]),
                _ => true,
            },
            Tie::Midpoint { p, a, b } => match (pos(p), pos(a), pos(b)) {
                (Some(x), Some(a), Some(b)) => check(x.dist((a + b) * 0.5), a.dist(b), &[x, a, b]),
                _ => true,
            },
            Tie::Mirror { p, q, l } => match (pos(p), pos(q), base(l)) {
                (Some(x), Some(y), Some((o, d))) => check(y.dist(reflected(x, o, d)), x.dist(o), &[x, y, o]),
                _ => true,
            },
            Tie::SameLength { p1, q1, p2, q2 } => match (pos(p1), pos(q1), pos(p2), pos(q2)) {
                (Some(a), Some(b), Some(c), Some(d)) => check((a.dist(b) - c.dist(d)).abs(), a.dist(b), &[a, b, c, d]),
                _ => true,
            },
            Tie::Equidistant { c, a, b } => match (pos(c), pos(a), pos(b)) {
                (Some(c), Some(a), Some(b)) => check((a.dist(c) - b.dist(c)).abs(), a.dist(c), &[c, a, b]),
                _ => true,
            },
        }
    }
}
