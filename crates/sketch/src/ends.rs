//! Loose ends of a sketch's profile curves, for the sketch to show where a profile can fail.
//!
//! Profiles are read from the geometry: ends that lie together close a loop whether or not a
//! constraint ties them (see `profiles`). Two kinds of end are worth showing:
//! - an **open end** touches no other profile curve, so no profile runs through it;
//! - an **untied end** touches another profile curve only by position: the loop closes now, but
//!   nothing keeps it closed when a dimension changes or a point is dragged.
//!
//! An end is tied when it is shared with another curve's end, joined to one by coincident
//! constraints, or held on a curve (point on curve, midpoint).

use solidcraft_geom::{Seg2, Vec2};

use crate::model::{ConstraintKind, CurveKind, Sketch};
use crate::union_find::{find, union};

/// How close an end has to be to another curve to touch it: the distance at which profile
/// finding joins and splits curves.
pub const END_TOL: f64 = 1e-5;

/// Ends looked at, at most: a larger sketch reports nothing rather than taking long.
const MAX_ENDS: usize = 100_000;

/// What an untied end touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndTouch {
    /// Another curve's end (a point index).
    Point(usize),
    /// Another curve, between its ends (a curve index).
    Curve(usize),
}

/// An end that touches another curve without being tied to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UntiedEnd {
    /// The end's point index.
    pub point: usize,
    /// The curve it is an end of.
    pub curve: usize,
    pub touches: EndTouch,
}

/// The open and untied ends of a sketch's drawn profile curves (point indices).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EndReport {
    pub open: Vec<usize>,
    pub untied: Vec<UntiedEnd>,
}

impl EndReport {
    pub fn is_empty(&self) -> bool {
        self.open.is_empty() && self.untied.is_empty()
    }
}

/// Curves profiles are made of (the ones `find_profiles` reads).
fn profile_curve(sk: &Sketch, ci: usize) -> bool {
    sk.curves.get(ci).is_some_and(|c| !(c.construction || c.centerline || c.infinite))
}

/// A box around a segment (conservative for curved ones).
fn seg_box(s: &Seg2) -> (Vec2, Vec2) {
    let hull = |pts: &[Vec2]| {
        pts.iter().fold((Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), p| {
            (Vec2::new(lo.x.min(p.x), lo.y.min(p.y)), Vec2::new(hi.x.max(p.x), hi.y.max(p.y)))
        })
    };
    match *s {
        Seg2::Line { a, b } => hull(&[a, b]),
        Seg2::Arc { center, radius, .. } => {
            let r = Vec2::new(radius.abs(), radius.abs());
            (center - r, center + r)
        }
        Seg2::Cubic { p0, p1, p2, p3 } => hull(&[p0, p1, p2, p3]),
        Seg2::Conic { a, apex, b, .. } => hull(&[a, apex, b]),
        Seg2::Bezier { n, p } => hull(p.get(..=usize::from(n)).unwrap_or(p.as_slice())),
    }
}

/// The open and untied ends of the drawn profile curves (lines, arcs and open free-form curves
/// that are not construction, projected or text). Positions are the sketch's current ones.
pub fn loose_ends(sk: &Sketch) -> EndReport {
    let n = sk.points.len();
    // Coincident constraints join points into one place.
    let mut group: Vec<usize> = (0..n).collect();
    for c in &sk.constraints {
        if let ConstraintKind::Coincident { p, q } = c.kind {
            union(&mut group, p, q);
        }
    }
    // Per place: how many profile curve ends meet there, and whether a point there is held
    // on a profile curve.
    let mut ends_at = vec![0usize; n];
    let mut held = vec![false; n];
    let mut ends: Vec<(usize, usize)> = Vec::new();
    for (ci, c) in sk.curves.iter().enumerate() {
        if !profile_curve(sk, ci) {
            continue;
        }
        let Some((a, b)) = c.kind.ends() else { continue };
        for p in [a, b] {
            let g = find(&mut group, p);
            if let Some(k) = ends_at.get_mut(g) {
                *k += 1;
            }
        }
        // Lines shorter than profile finding keeps are left out of profiles too.
        let degenerate = matches!(c.kind, CurveKind::Line { .. }) && sk.point(a).zip(sk.point(b)).is_some_and(|(pa, pb)| pa.dist(pb) <= END_TOL);
        if c.link.is_none() && !degenerate {
            ends.push((a, ci));
            if b != a {
                ends.push((b, ci));
            }
        }
    }
    for c in &sk.constraints {
        let on = match c.kind {
            ConstraintKind::PointOnCurve { p, c } | ConstraintKind::Midpoint { p, l: c } if profile_curve(sk, c) => Some(p),
            _ => None,
        };
        if let Some(p) = on {
            let g = find(&mut group, p);
            if let Some(h) = held.get_mut(g) {
                *h = true;
            }
        }
    }
    if ends.len() > MAX_ENDS {
        return EndReport::default();
    }
    // Ends tied to nothing: the ones to look at by position.
    let mut free: Vec<(usize, usize, Vec2)> = Vec::new();
    for &(p, ci) in &ends {
        let g = find(&mut group, p);
        if ends_at.get(g).copied().unwrap_or(0) >= 2 || held.get(g).copied().unwrap_or(false) {
            continue;
        }
        if let Some(at) = sk.point(p).filter(|v| v.x.is_finite() && v.y.is_finite()) {
            free.push((p, ci, at));
        }
    }
    if free.is_empty() {
        return EndReport::default();
    }
    free.sort_by(|a, b| a.2.x.total_cmp(&b.2.x));
    // What each free end touches: another curve's end first, else another curve's span.
    let mut touch: Vec<Option<EndTouch>> = vec![None; free.len()];
    for (ci, c) in sk.curves.iter().enumerate() {
        if !profile_curve(sk, ci) {
            continue;
        }
        let curve_ends = c.kind.ends();
        for seg in sk.segs(ci) {
            let (lo, hi) = seg_box(&seg);
            let first = free.partition_point(|f| f.2.x < lo.x - END_TOL);
            for (k, &(p, own, at)) in free.iter().enumerate().skip(first) {
                if at.x > hi.x + END_TOL {
                    break;
                }
                if at.y < lo.y - END_TOL || at.y > hi.y + END_TOL || matches!(touch.get(k), Some(Some(EndTouch::Point(_)))) {
                    continue;
                }
                // Another end of a curve (its own other end too: a loop drawn as one curve).
                let at_end = curve_ends.and_then(|(a, b)| [a, b].into_iter().find(|&q| q != p && sk.point(q).is_some_and(|v| v.dist(at) <= END_TOL)));
                let found = match at_end {
                    Some(q) => Some(EndTouch::Point(q)),
                    None if ci != own && seg.point_at(seg.closest_param(at)).dist(at) <= END_TOL => Some(EndTouch::Curve(ci)),
                    None => None,
                };
                if let (Some(found), Some(slot)) = (found, touch.get_mut(k))
                    && (slot.is_none() || matches!(found, EndTouch::Point(_)))
                {
                    *slot = Some(found);
                }
            }
        }
    }
    // Free ends that touch each other are one place, however many meet there: each is tied to
    // the place's first end, which is tied to what it touches outside the place (if anything).
    let index: std::collections::HashMap<usize, usize> = free.iter().enumerate().map(|(k, f)| (f.0, k)).collect();
    let mut place: Vec<usize> = (0..free.len()).collect();
    for (k, t) in touch.iter().enumerate() {
        if let Some(EndTouch::Point(q)) = t
            && let Some(&j) = index.get(q)
        {
            union(&mut place, k, j);
        }
    }
    let mut first: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (k, f) in free.iter().enumerate() {
        let root = find(&mut place, k);
        let e = first.entry(root).or_insert(f.0);
        *e = (*e).min(f.0);
    }
    let mut report = EndReport::default();
    for (k, &(p, curve, _)) in free.iter().enumerate() {
        let Some(t) = touch.get(k).copied().flatten() else {
            report.open.push(p);
            continue;
        };
        let root = find(&mut place, k);
        let anchor = first.get(&root).copied().unwrap_or(p);
        let touches = match t {
            _ if anchor != p => EndTouch::Point(anchor),
            // The place's first end: tied to what it touches outside the place.
            EndTouch::Point(q) if index.contains_key(&q) => continue,
            t => t,
        };
        report.untied.push(UntiedEnd { point: p, curve, touches });
    }
    report.open.sort_unstable();
    report.open.dedup();
    report.untied.sort_by_key(|u| u.point);
    report.untied.dedup_by_key(|u| u.point);
    report
}
