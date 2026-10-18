//! The cut of a body by a plane as loops of curves in the plane's frame
//! (`inspect.section_curves`).
//!
//! The crossings come from the body's mesh (as for sketch Intersect), one chain per B-rep face.
//! A face's chain becomes one curve: a line when every crossing is on one line (planar faces),
//! an arc when the crossings that lie exactly on the surface (on a ruling of a cylinder or a
//! cone) are on one circle, else a fit spline through the crossings. The face curves are then
//! joined end to end into loops, lines that continue one another and arcs of one circle are
//! merged, and closed loops are oriented: outer loops counter-clockwise, holes clockwise.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use solidcraft_geom::{Mesh, Plane, Vec2};

/// Faces' curves in one cut at most, and points in one spline at most.
const MAX_PIECES: usize = 100_000;
const MAX_SPLINE_POINTS: usize = 500;

/// One curve of a section loop, in the plane's frame.
#[derive(Clone, Debug, PartialEq)]
pub enum SectionSeg {
    Line {
        a: Vec2,
        b: Vec2,
    },
    /// Around `c` with radius `r`, from the angle `start` through `sweep` (counter-clockwise
    /// when positive; a whole circle when it is ±2π).
    Arc {
        c: Vec2,
        r: f64,
        start: f64,
        sweep: f64,
    },
    /// A fit spline through `pts` (the sketch's interpolating cubic; when `periodic`, closed
    /// and C2 all round, the last point repeating the first).
    Spline {
        pts: Vec<Vec2>,
        periodic: bool,
    },
}

impl SectionSeg {
    pub fn start(&self) -> Vec2 {
        match self {
            SectionSeg::Line { a, .. } => *a,
            SectionSeg::Arc { c, r, start, .. } => *c + Vec2::from_angle(*start) * *r,
            SectionSeg::Spline { pts, .. } => pts.first().copied().unwrap_or_default(),
        }
    }

    pub fn end(&self) -> Vec2 {
        match self {
            SectionSeg::Line { b, .. } => *b,
            SectionSeg::Arc { c, r, start, sweep } => *c + Vec2::from_angle(*start + *sweep) * *r,
            SectionSeg::Spline { pts, .. } => pts.last().copied().unwrap_or_default(),
        }
    }

    fn reversed(self) -> SectionSeg {
        match self {
            SectionSeg::Line { a, b } => SectionSeg::Line { a: b, b: a },
            SectionSeg::Arc { c, r, start, sweep } => SectionSeg::Arc { c, r, start: start + sweep, sweep: -sweep },
            SectionSeg::Spline { mut pts, periodic } => {
                pts.reverse();
                SectionSeg::Spline { pts, periodic }
            }
        }
    }

    /// A spline made periodic: its last point (when it is within reach of the first) becomes
    /// the first exactly, or the first is repeated.
    fn closed(self, closed: bool) -> SectionSeg {
        match self {
            SectionSeg::Spline { mut pts, periodic: false } if closed => {
                let (Some(first), Some(last)) = (pts.first().copied(), pts.last().copied()) else {
                    return SectionSeg::Spline { pts, periodic: false };
                };
                let span = pts.windows(2).map(|w| w.first().zip(w.get(1)).map_or(0.0, |(a, b)| a.dist(*b))).fold(0.0, f64::max);
                if last.dist(first) <= span * 1e-6 {
                    pts.pop();
                }
                pts.push(first);
                SectionSeg::Spline { periodic: pts.len() >= 4, pts }
            }
            other => other,
        }
    }

    /// The cubic Bézier pieces of a spline (empty for lines and arcs).
    pub fn beziers(&self) -> Vec<[Vec2; 4]> {
        match self {
            SectionSeg::Spline { pts, periodic } => solidcraft_sketch::fit_beziers_mode(pts, *periodic).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Twice the signed area this curve sweeps about the origin, ∫ p × dp, exactly (Gauss
    /// quadrature is exact for the quintic integrand of a cubic).
    fn area2(&self) -> f64 {
        match self {
            SectionSeg::Line { a, b } => a.cross(*b),
            SectionSeg::Arc { c, r, start, sweep } => r * r * sweep + r * c.cross(Vec2::from_angle(start + sweep) - Vec2::from_angle(*start)),
            SectionSeg::Spline { .. } => {
                let (g, w) = ([-(0.6f64).sqrt(), 0.0, 0.6f64.sqrt()], [5.0 / 9.0, 8.0 / 9.0, 5.0 / 9.0]);
                self.beziers()
                    .iter()
                    .map(|[p0, p1, p2, p3]| {
                        g.iter()
                            .zip(w)
                            .map(|(x, wi)| {
                                let t = 0.5 * (x + 1.0);
                                let u = 1.0 - t;
                                let p = *p0 * (u * u * u) + *p1 * (3.0 * u * u * t) + *p2 * (3.0 * u * t * t) + *p3 * (t * t * t);
                                let d = (*p1 - *p0) * (3.0 * u * u) + (*p2 - *p1) * (6.0 * u * t) + (*p3 - *p2) * (3.0 * t * t);
                                0.5 * wi * p.cross(d)
                            })
                            .sum::<f64>()
                    })
                    .sum()
            }
        }
    }

    /// Points along the curve (ends included) for containment tests and drawing.
    pub fn sample(&self, out: &mut Vec<Vec2>) {
        match self {
            SectionSeg::Line { a, b } => out.extend([*a, *b]),
            SectionSeg::Arc { c, r, start, sweep } => {
                let n = ((sweep.abs() / TAU * 64.0).ceil() as usize).clamp(2, 64);
                out.extend((0..=n).map(|i| *c + Vec2::from_angle(start + sweep * i as f64 / n as f64) * *r));
            }
            SectionSeg::Spline { .. } => {
                for (i, [p0, p1, p2, p3]) in self.beziers().iter().enumerate() {
                    for k in usize::from(i > 0)..=8 {
                        let t = k as f64 / 8.0;
                        let u = 1.0 - t;
                        out.push(*p0 * (u * u * u) + *p1 * (3.0 * u * u * t) + *p2 * (3.0 * u * t * t) + *p3 * (t * t * t));
                    }
                }
            }
        }
    }
}

/// One loop of a section: its curves in order, end to end.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionLoop {
    pub closed: bool,
    pub segments: Vec<SectionSeg>,
    /// Signed area (counter-clockwise positive) of a closed loop, mm².
    pub area: Option<f64>,
}

type Key = (i64, i64);

/// One face's curve, with the keys of its chain's ends (equal keys join).
struct Piece {
    seg: SectionSeg,
    ends: Option<(Key, Key)>,
}

/// The loops where `plane` cuts the mesh, in the plane's frame.
pub fn section_loops(plane: &Plane, m: &Mesh) -> Vec<SectionLoop> {
    let size = m.bounds().diagonal().max(1e-9);
    let q = size * 1e-7;
    let key = |p: Vec2| -> Key { ((p.x / q).round() as i64, (p.y / q).round() as i64) };
    let (segs, exact) = super::section_segments(plane, m, None);
    let exact: BTreeSet<Key> = exact.iter().map(|p| key(*p)).collect();
    let mut by_face: BTreeMap<u32, Vec<(Vec2, Vec2)>> = BTreeMap::new();
    for (a, b, f) in segs {
        by_face.entry(f).or_default().push((a, b));
    }
    let mut pieces = Vec::new();
    for s in by_face.values() {
        for (chain, closed) in chains(s, &key) {
            if pieces.len() >= MAX_PIECES {
                break;
            }
            if let Some(seg) = classify(&chain, closed, &exact, &key, size) {
                let ends = (!closed).then(|| chain.first().zip(chain.last()).map(|(a, b)| (key(*a), key(*b)))).flatten();
                pieces.push(Piece { seg, ends });
            }
        }
    }
    let mut loops: Vec<SectionLoop> =
        join(pieces).into_iter().map(|(closed, segs)| SectionLoop { closed, segments: merge(segs, closed, size), area: None }).collect();
    orient(&mut loops);
    loops
}

/// Chain segments into simple paths (open ones from their ends) and closed loops.
fn chains(segs: &[(Vec2, Vec2)], key: &dyn Fn(Vec2) -> Key) -> Vec<(Vec<Vec2>, bool)> {
    let mut nodes: BTreeMap<Key, (Vec2, Vec<usize>)> = BTreeMap::new();
    let mut uniq: Vec<(Key, Key)> = Vec::new();
    let mut seen: BTreeSet<(Key, Key)> = BTreeSet::new();
    for (a, b) in segs {
        let (ka, kb) = (key(*a), key(*b));
        let k = if ka <= kb { (ka, kb) } else { (kb, ka) };
        if ka == kb || !seen.insert(k) {
            continue;
        }
        nodes.entry(ka).or_insert_with(|| (*a, Vec::new())).1.push(uniq.len());
        nodes.entry(kb).or_insert_with(|| (*b, Vec::new())).1.push(uniq.len());
        uniq.push(k);
    }
    let mut used = vec![false; uniq.len()];
    let mut out = Vec::new();
    let ends: Vec<Key> = nodes.iter().filter(|(_, n)| n.1.len() != 2).map(|(k, _)| *k).collect();
    let all: Vec<Key> = nodes.keys().copied().collect();
    for start in ends.into_iter().chain(all) {
        loop {
            let free = |k: &Key, used: &[bool]| nodes.get(k).and_then(|n| n.1.iter().copied().find(|s| !used.get(*s).copied().unwrap_or(true)));
            if free(&start, &used).is_none() {
                break;
            }
            let mut chain = vec![nodes.get(&start).map(|n| n.0).unwrap_or_default()];
            let mut cur = start;
            while let Some(s) = free(&cur, &used) {
                if let Some(u) = used.get_mut(s) {
                    *u = true;
                }
                let Some(&(ka, kb)) = uniq.get(s) else { break };
                cur = if ka == cur { kb } else { ka };
                chain.push(nodes.get(&cur).map(|n| n.0).unwrap_or_default());
                if cur == start || nodes.get(&cur).is_some_and(|n| n.1.len() != 2) {
                    break;
                }
            }
            out.push((chain, cur == start));
        }
    }
    out
}

/// The one curve of a face's chain.
fn classify(chain: &[Vec2], closed: bool, exact: &BTreeSet<Key>, key: &dyn Fn(Vec2) -> Key, size: f64) -> Option<SectionSeg> {
    // Exact crossings agree to rounding; the others (inside curved triangles) to the
    // surface's curvature over a mesh edge.
    let (tight, near) = (size * 1e-7, size * 1e-4);
    let (first, last) = (*chain.first()?, *chain.last()?);
    if chain.len() < 2 || (!closed && first.dist(last) <= tight) {
        return None;
    }
    if !closed && super::fits_line(chain, tight) {
        return Some(SectionSeg::Line { a: first, b: last });
    }
    let on: Vec<Vec2> = chain.iter().copied().filter(|p| exact.contains(&key(*p))).collect();
    let circle = match on.len() {
        0..=2 => None,
        3 => on.first().zip(on.get(1)).zip(on.get(2)).and_then(|((a, b), c)| super::circumcircle(*a, *b, *c)),
        _ => super::ls_circle(&on),
    };
    if let Some((c, r)) = circle
        && r < size * 1e3
        && on.iter().all(|p| (p.dist(c) - r).abs() <= tight * 10.0)
        && chain.iter().all(|p| (p.dist(c) - r).abs() <= near)
        && let Some(sweep) = turning(chain, c)
    {
        let sweep = if closed { TAU.copysign(sweep) } else { sweep };
        return Some(SectionSeg::Arc { c, r, start: (first - c).angle(), sweep });
    }
    // A spline through the exact crossings when there are enough of them, else all.
    let mut pts: Vec<Vec2> = if on.len() >= 4 {
        let mut p = Vec::with_capacity(on.len() + 2);
        if !closed {
            p.push(first);
        }
        p.extend(on.iter().copied().filter(|q| closed || (q.dist(first) > tight && q.dist(last) > tight)));
        if !closed {
            p.push(last);
        }
        p
    } else {
        chain.to_vec()
    };
    if closed && pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) <= tight * 10.0) {
        pts.pop();
    }
    pts.dedup_by(|a, b| a.dist(*b) <= tight * 10.0);
    if pts.len() > MAX_SPLINE_POINTS {
        let step = pts.len().div_ceil(MAX_SPLINE_POINTS);
        let end = pts.last().copied();
        pts = pts.into_iter().step_by(step).collect();
        if !closed && let Some(e) = end.filter(|e| pts.last() != Some(e)) {
            pts.push(e);
        }
    }
    match pts.len() {
        0 | 1 => None,
        2 if !closed => Some(SectionSeg::Line { a: first, b: last }),
        n if closed && n < 3 => None,
        _ => Some(SectionSeg::Spline { pts, periodic: false }.closed(closed)),
    }
}

/// The signed angle a chain turns through around `c`, when it turns one way in small steps.
fn turning(chain: &[Vec2], c: Vec2) -> Option<f64> {
    let mut sweep = 0.0f64;
    for w in chain.windows(2) {
        let (a, b) = (*w.first()?, *w.get(1)?);
        let d = (a - c).cross(b - c).atan2((a - c).dot(b - c));
        if d.abs() > 1.0 || (sweep != 0.0 && d * sweep < 0.0) {
            return None;
        }
        sweep += d;
    }
    (sweep.abs() > 1e-9).then_some(sweep)
}

/// Join the faces' curves end to end: (closed, curves in order).
fn join(pieces: Vec<Piece>) -> Vec<(bool, Vec<SectionSeg>)> {
    let mut out = Vec::new();
    let mut ends: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    let mut open: Vec<Option<(SectionSeg, Key, Key)>> = Vec::new();
    for p in pieces {
        match p.ends {
            None => out.push((true, vec![p.seg])),
            Some((a, b)) => {
                ends.entry(a).or_default().push(open.len());
                ends.entry(b).or_default().push(open.len());
                open.push(Some((p.seg, a, b)));
            }
        }
    }
    let take = |open: &mut Vec<Option<(SectionSeg, Key, Key)>>, at: Key, ends: &BTreeMap<Key, Vec<usize>>| -> Option<(SectionSeg, Key)> {
        let i = ends.get(&at)?.iter().copied().find(|i| open.get(*i).is_some_and(Option::is_some))?;
        let (seg, a, b) = open.get_mut(i)?.take()?;
        Some(if a == at { (seg, b) } else { (seg.reversed(), a) })
    };
    // Start open paths at a dangling end, then walk what is left (loops).
    let mut starts: Vec<Key> = ends.iter().filter(|(_, v)| v.len() == 1).map(|(k, _)| *k).collect();
    starts.extend(open.iter().flatten().map(|(_, a, _)| *a).collect::<Vec<_>>());
    for start in starts {
        while let Some((seg, mut cur)) = take(&mut open, start, &ends) {
            let mut segs = vec![seg];
            while cur != start {
                let Some((seg, next)) = take(&mut open, cur, &ends) else { break };
                segs.push(seg);
                cur = next;
            }
            out.push((cur == start, segs));
        }
    }
    out
}

/// Merge lines that continue one another, arcs of one circle and splines that join smoothly.
fn merge(segs: Vec<SectionSeg>, closed: bool, size: f64) -> Vec<SectionSeg> {
    let tight = size * 1e-6;
    let mut out: Vec<SectionSeg> = Vec::with_capacity(segs.len());
    for s in segs {
        match out.last_mut().and_then(|l| joined(l, &s, tight)) {
            Some(m) => {
                if let Some(l) = out.last_mut() {
                    *l = m;
                }
            }
            None => out.push(s),
        }
    }
    if closed && out.len() > 1 {
        let (Some(last), Some(first)) = (out.last(), out.first()) else { return out };
        if let Some(m) = joined(last, first, tight) {
            out.pop();
            if let Some(f) = out.first_mut() {
                *f = m;
            }
        }
    }
    // A loop that is one curve: a whole circle, or a closed spline.
    if closed && let [only] = &mut out[..] {
        match only {
            SectionSeg::Arc { sweep, .. } if (sweep.abs() - TAU).abs() < 1e-6 => *sweep = TAU.copysign(*sweep),
            SectionSeg::Spline { .. } => *only = std::mem::replace(only, SectionSeg::Line { a: Vec2::ZERO, b: Vec2::ZERO }).closed(true),
            _ => {}
        }
    }
    out
}

/// `a` followed by `b` as one curve, when it is one.
fn joined(a: &SectionSeg, b: &SectionSeg, tight: f64) -> Option<SectionSeg> {
    match (a, b) {
        (SectionSeg::Line { a: p, b: q }, SectionSeg::Line { b: r, .. }) => {
            let (u, v) = ((*q - *p).normalized()?, (*r - *q).normalized()?);
            (u.dot(v) > 0.0 && u.cross(*r - *p).abs() <= tight).then_some(SectionSeg::Line { a: *p, b: *r })
        }
        (SectionSeg::Arc { c, r, start, sweep }, SectionSeg::Arc { c: c2, r: r2, sweep: s2, .. }) => {
            let total = sweep + s2;
            (c.dist(*c2) <= tight && (r - r2).abs() <= tight && sweep * s2 > 0.0 && total.abs() <= TAU + 1e-6).then_some(SectionSeg::Arc {
                c: *c,
                r: *r,
                start: *start,
                sweep: total.clamp(-TAU, TAU),
            })
        }
        (SectionSeg::Spline { pts: p, periodic: false }, SectionSeg::Spline { pts: q, periodic: false }) => {
            // Smooth when the turn across the joint is no sharper than the turns beside it.
            let turn = |a: Vec2, b: Vec2, c: Vec2| -> Option<f64> {
                let (u, v) = ((b - a).normalized()?, (c - b).normalized()?);
                Some(u.cross(v).atan2(u.dot(v)).abs())
            };
            let n = p.len();
            let (pa, pb, pc) = (*p.get(n.checked_sub(3)?)?, *p.get(n - 2)?, *p.get(n - 1)?);
            let (qa, qb, qc) = (*q.first()?, *q.get(1)?, *q.get(2)?);
            let beside = turn(pa, pb, pc)?.max(turn(qa, qb, qc)?);
            let across = turn(pb, pc, qb)?;
            (pc.dist(qa) <= tight && across <= 1.5 * beside + 0.5f64.to_radians()).then(|| {
                let mut pts = p.clone();
                pts.extend(q.iter().skip(1).copied());
                SectionSeg::Spline { pts, periodic: false }
            })
        }
        _ => None,
    }
}

/// Closed loops' areas, and their direction: outer loops counter-clockwise, holes (inside an
/// odd number of other loops) clockwise.
fn orient(loops: &mut [SectionLoop]) {
    let polys: Vec<Option<Vec<Vec2>>> = loops
        .iter()
        .map(|l| {
            l.closed.then(|| {
                let mut p = Vec::new();
                for s in &l.segments {
                    s.sample(&mut p);
                }
                p
            })
        })
        .collect();
    for (i, l) in loops.iter_mut().enumerate() {
        if !l.closed {
            continue;
        }
        let area = 0.5 * l.segments.iter().map(SectionSeg::area2).sum::<f64>();
        let probe = l.segments.first().map(SectionSeg::start).unwrap_or_default();
        let depth = polys.iter().enumerate().filter(|(j, p)| *j != i && p.as_ref().is_some_and(|p| inside(p, probe))).count();
        let ccw = depth % 2 == 0;
        if (area > 0.0) != ccw {
            let segs = std::mem::take(&mut l.segments);
            l.segments = segs.into_iter().rev().map(SectionSeg::reversed).collect();
            l.area = Some(-area);
        } else {
            l.area = Some(area);
        }
    }
}

/// Even-odd point in polygon.
fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y) {
            odd = !odd;
        }
    }
    odd
}
