//! Sweeps along 3D point paths (3D sketch curves: projected, intersection and included curves).
//! The profile follows the path with a rotation-minimising frame. Smooth paths get one B-spline
//! wall per profile segment; paths with sharp corners (or profiles with holes) get ruled walls
//! between stations, mitred at the corners. Sweep options (orientation, twist, end scale, guide
//! rails; see [`SweepControl`]) reshape the stations; their default leaves them exactly as
//! described.

use mt::builder;
use solidcraft_geom::{Plane, Region2, Seg2, Vec2, Vec3};
use truck_modeling as mt;

use crate::body::{Body, p3};
use crate::build::{cap_and_close, close_sweep, interpolate_cubic, smooth_loft_faces, uniform_wire};
use crate::numbered::{NumberedLoop, NumberedRegion};
use crate::record::{self, Side};
use crate::{KernelError, Result, guard};

/// Most path points accepted.
const MAX_PATH_POINTS: usize = 100_000;
/// Stations of a smooth sweep (the B-spline walls interpolate them).
const MAX_SMOOTH_STATIONS: usize = 120;
/// Stations of a ruled sweep.
const MAX_RULED_STATIONS: usize = 2000;
/// A turn sharper than this between path segments is a corner (radians).
const CORNER: f64 = 0.35;
/// Samples per profile segment of the smooth walls.
const SEG_SAMPLES: usize = 12;
/// Stations of a smooth sweep that twists or scales, at the least.
const MIN_CONTROLLED_STATIONS: usize = 16;
/// Stations of a smooth sweep that twists or scales, at the most.
const MAX_CONTROLLED_STATIONS: usize = 160;
/// Stations of a ruled sweep that twists or scales, at the least.
const MIN_CONTROLLED_RULED: usize = 64;
/// The most twist one station of a smooth sweep may carry (radians).
const SMOOTH_TWIST_STEP: f64 = std::f64::consts::PI / 18.0;
/// The most twist one station of a ruled sweep may carry (radians).
const RULED_TWIST_STEP: f64 = std::f64::consts::PI / 36.0;

/// How the profile turns as it follows a sweep path.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SweepOrientation {
    /// Square to the path, turned along it by a rotation-minimising frame.
    #[default]
    Perpendicular,
    /// As drawn: the profile only moves along the path, never turning.
    Parallel,
    /// Square to the path, its turn about the path fixed by a direction: the direction's
    /// projection onto the profile's plane stays where it was at the start.
    Direction(Vec3),
}

/// How guide rails reshape the profile at each station.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RailFit {
    /// The profile grows or shrinks evenly with its distance across to the rail (between the
    /// two rails).
    #[default]
    Scale,
    /// The profile stretches only across to the rail (between the rails), keeping its size the
    /// other way.
    Stretch,
}

/// Sweep options beyond following the path. `twist` turns the profile about the path by that
/// many radians over the path's length (right hand about the direction of travel, in proportion
/// to the length run); `scale` is the profile's size at the end, reached linearly along the
/// path's length, about the path's start point. The default (perpendicular, no twist, scale 1,
/// no rails) is a plain sweep.
///
/// `rails` are guide rails, as polylines from the end nearest the profile. Each station of the
/// path is square to it; a rail meets the station's plane at one point. With one rail, the
/// profile turns about the path so that its point where the rail starts stays on the rail, and
/// is scaled (or stretched, see [`RailFit`]) by how far the rail is from the path compared with
/// the start. With two rails, the profile's points where the rails start stay on both: it moves
/// with the first rail, turns with the line between them and scales (stretches) with their
/// distance apart. Rails decide the profile's turn and size, so they take the default
/// orientation, no twist and scale 1, and a path without sharp corners.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepControl {
    pub orientation: SweepOrientation,
    pub twist: f64,
    pub scale: f64,
    pub rails: Vec<Vec<Vec3>>,
    pub rail_fit: RailFit,
}

impl Default for SweepControl {
    fn default() -> Self {
        SweepControl { orientation: SweepOrientation::Perpendicular, twist: 0.0, scale: 1.0, rails: Vec::new(), rail_fit: RailFit::Scale }
    }
}

impl SweepControl {
    /// The most twist accepted: four turns (radians).
    pub const MAX_TWIST: f64 = 4.0 * std::f64::consts::TAU;
    /// The smallest end scale accepted.
    pub const MIN_SCALE: f64 = 0.01;
    /// The largest end scale accepted.
    pub const MAX_SCALE: f64 = 100.0;

    /// A plain sweep: the options change nothing.
    pub fn is_default(&self) -> bool {
        *self == SweepControl::default()
    }

    /// Whether the profile changes along the path (it twists, scales or follows rails).
    fn varies(&self) -> bool {
        self.twist != 0.0 || self.scale != 1.0 || !self.rails.is_empty()
    }

    /// Refuses options out of range: a twist beyond [`Self::MAX_TWIST`], a scale outside
    /// [`Self::MIN_SCALE`]…[`Self::MAX_SCALE`], a direction that is not finite or has no length,
    /// more than two rails, a rail that is not 2…100000 finite points, and rails with any other
    /// orientation, a twist or an end scale.
    pub fn check(&self) -> Result<()> {
        if !(self.twist.is_finite() && self.twist.abs() <= Self::MAX_TWIST) {
            return Err(KernelError::Invalid("the sweep twist must be within four turns (1440 deg)".into()));
        }
        if !(self.scale.is_finite() && (Self::MIN_SCALE..=Self::MAX_SCALE).contains(&self.scale)) {
            return Err(KernelError::Invalid("the sweep end scale must be between 0.01 and 100".into()));
        }
        if let SweepOrientation::Direction(d) = self.orientation
            && !(d.is_finite() && d.len() > 1e-9 && d.len() < 1e9)
        {
            return Err(KernelError::Invalid("the sweep orientation direction must be a finite, non-zero vector".into()));
        }
        if self.rails.len() > 2 {
            return Err(KernelError::Invalid("a sweep takes one or two guide rails".into()));
        }
        for rail in &self.rails {
            clean(rail).map_err(|_| KernelError::Invalid("a guide rail needs 2…100000 finite points along its length".into()))?;
        }
        if !self.rails.is_empty() && (self.orientation != SweepOrientation::Perpendicular || self.twist != 0.0 || self.scale != 1.0) {
            return Err(KernelError::Invalid(
                "guide rails set the profile's turn and size along the path: leave the orientation, twist and end scale at their defaults".into(),
            ));
        }
        Ok(())
    }
}

/// A linear map of world vectors (columns) plus the station it is centred at.
#[derive(Clone, Copy)]
struct Station {
    at: Vec3,
    m: [Vec3; 3],
}

impl Station {
    fn apply(&self, v: Vec3) -> Vec3 {
        self.m[0] * v.x + self.m[1] * v.y + self.m[2] * v.z
    }
}

fn clean(path: &[Vec3]) -> Result<Vec<Vec3>> {
    if path.len() < 2 || path.len() > MAX_PATH_POINTS {
        return Err(KernelError::Invalid("a sweep path needs 2…100000 points".into()));
    }
    let mut out: Vec<Vec3> = Vec::with_capacity(path.len());
    for p in path {
        if !(p.is_finite() && p.x.abs() < 1e9 && p.y.abs() < 1e9 && p.z.abs() < 1e9) {
            return Err(KernelError::Invalid("the sweep path has a bad point".into()));
        }
        if out.last().is_none_or(|l| l.dist(*p) > 1e-9) {
            out.push(*p);
        }
    }
    if out.len() < 2 {
        return Err(KernelError::Invalid("the sweep path has no length".into()));
    }
    Ok(out)
}

pub(crate) fn closed_path(pts: &[Vec3]) -> bool {
    pts.len() > 3 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-7)
}

/// The cyclic correspondence of a loop's last section to its first. Matching whole curves
/// (rather than vertices alone) prevents sewing an incompatible twist or scale shut.
fn closing_offset(plane: &Plane, profile: &NumberedLoop, st: &[Station]) -> Result<usize> {
    let fail =
        || KernelError::Invalid("a sweep along a closed path must return to its starting profile; adjust the twist, end scale or rails".into());
    let (Some(first), Some(last)) = (st.first(), st.last()) else { return Err(fail()) };
    let n = profile.lp.segs.len();
    if n == 0 || n > 4096 {
        return Err(fail());
    }
    let at = |s: &Station, q: Vec2| s.at + s.apply(plane.to_world(q) - first.at);
    let tol = (profile.lp.signed_area().abs().sqrt() * 1e-7).clamp(1e-8, 1e-5);
    // Filter offsets by the first corner before comparing all the curve samples.
    let start = profile.lp.segs.first().map(Seg2::start).ok_or_else(fail)?;
    (0..n)
        .filter(|k| profile.lp.segs.get(*k).is_some_and(|s| at(last, start).dist(at(first, s.start())) <= tol))
        .find(|k| {
            profile.lp.segs.iter().enumerate().all(|(j, a)| {
                profile.lp.segs.get((j + k) % n).is_some_and(|b| {
                    (0..SEG_SAMPLES).all(|i| {
                        let t = i as f64 / (SEG_SAMPLES - 1) as f64;
                        at(last, a.point_at(t)).dist(at(first, b.point_at(t))) <= tol
                    })
                })
            })
        })
        .ok_or_else(fail)
}

fn rotated_wire(wire: &mt::Wire, offset: usize) -> mt::Wire {
    wire.edge_iter().skip(offset).chain(wire.edge_iter().take(offset)).cloned().collect()
}

fn turn(a: Vec3, b: Vec3) -> f64 {
    match (a.normalized(), b.normalized()) {
        (Some(a), Some(b)) => a.dot(b).clamp(-1.0, 1.0).acos(),
        _ => 0.0,
    }
}

/// Points evenly spaced by arc length along the polyline (ends kept).
fn resample(p: &[Vec3], n: usize) -> Vec<Vec3> {
    let mut cum = vec![0.0];
    for w in p.windows(2) {
        let l = cum.last().copied().unwrap_or(0.0) + w[0].dist(w[1]);
        cum.push(l);
    }
    let total = cum.last().copied().unwrap_or(0.0);
    let mut out = Vec::with_capacity(n + 1);
    let mut k = 0;
    for i in 0..=n {
        let s = total * i as f64 / n as f64;
        while k + 2 < cum.len() && cum.get(k + 1).is_some_and(|c| *c < s) {
            k += 1;
        }
        let (Some(a), Some(b), Some(ca), Some(cb)) = (p.get(k), p.get(k + 1), cum.get(k), cum.get(k + 1)) else { break };
        let t = if cb - ca > 1e-15 { ((s - ca) / (cb - ca)).clamp(0.0, 1.0) } else { 0.0 };
        out.push(*a + (*b - *a) * t);
    }
    out
}

/// Rotation-minimising frames (double reflection) at the points, for the given unit tangents;
/// each station maps the first frame onto its own.
fn frames(pts: &[Vec3], tangents: &[Vec3]) -> Option<Vec<[Vec3; 3]>> {
    let t0 = *tangents.first()?;
    let helper = if t0.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let mut r = t0.cross(helper).normalized()?;
    let mut out = vec![[t0, r, t0.cross(r)]];
    for i in 0..pts.len().saturating_sub(1) {
        let (x0, x1, ti, tn) = (*pts.get(i)?, *pts.get(i + 1)?, *tangents.get(i)?, *tangents.get(i + 1)?);
        let v1 = x1 - x0;
        let c1 = v1.dot(v1);
        let (rl, tl) = if c1 > 1e-24 { (r - v1 * (2.0 / c1 * v1.dot(r)), ti - v1 * (2.0 / c1 * v1.dot(ti))) } else { (r, ti) };
        let v2 = tn - tl;
        let c2 = v2.dot(v2);
        r = if c2 > 1e-24 { rl - v2 * (2.0 / c2 * v2.dot(rl)) } else { rl };
        // Keep it exactly perpendicular and unit.
        r = (r - tn * r.dot(tn)).normalized()?;
        out.push([tn, r, tn.cross(r)]);
    }
    Some(out)
}

/// Stations: each maps a world vector at the first station to the same vector at its own,
/// optionally stretched across a corner (mitre).
fn stations(pts: &[Vec3], tangents: &[Vec3], mitre: &[Option<(Vec3, f64)>]) -> Result<Vec<Station>> {
    let f = frames(pts, tangents).ok_or_else(|| KernelError::Invalid("the sweep path doubles back".into()))?;
    let f0 = *f.first().ok_or_else(|| KernelError::Invalid("sweep path".into()))?;
    let mut out = Vec::with_capacity(f.len());
    for (i, fi) in f.iter().enumerate() {
        // Rotation taking f0 to fi: columns are the images of the world axes.
        let col = |e: Vec3| fi[0] * f0[0].dot(e) + fi[1] * f0[1].dot(e) + fi[2] * f0[2].dot(e);
        let mut m = [col(Vec3::X), col(Vec3::Y), col(Vec3::Z)];
        if let Some(Some((b, s))) = mitre.get(i) {
            for c in &mut m {
                *c += *b * ((s - 1.0) * c.dot(*b));
            }
        }
        out.push(Station { at: *pts.get(i).ok_or_else(|| KernelError::Invalid("sweep path".into()))?, m });
    }
    Ok(out)
}

/// Frames square to the path whose turn about it keeps `d`'s projection: none where the path
/// runs along `d`.
fn frames_toward(tangents: &[Vec3], d: Vec3) -> Result<Vec<[Vec3; 3]>> {
    let d = d.normalized().ok_or_else(|| KernelError::Invalid("the sweep orientation direction has no length".into()))?;
    tangents
        .iter()
        .map(|t| {
            let across = d - *t * t.dot(d);
            // Within about 0.06 degrees of the direction the profile's turn is undefined.
            if across.len() < 1e-3 {
                return Err(KernelError::Invalid("the sweep path runs along the orientation direction".into()));
            }
            let r = across.normalized().ok_or_else(|| KernelError::Invalid("the sweep path runs along the orientation direction".into()))?;
            Ok([*t, r, t.cross(r)])
        })
        .collect()
}

/// Stations shaped by sweep options: each maps a world vector at the first station to its image
/// at its own (twisted and scaled about the profile's normal through the first station, then
/// turned by the orientation's frames), optionally stretched across a corner (mitre). Twist and
/// scale grow with the length run along `pts`.
fn controlled_stations(plane: &Plane, pts: &[Vec3], tangents: &[Vec3], mitre: &[Option<(Vec3, f64)>], ctl: &SweepControl) -> Result<Vec<Station>> {
    if !ctl.rails.is_empty() {
        return railed_stations(pts, tangents, mitre, ctl);
    }
    let bad = || KernelError::Invalid("sweep path".into());
    let t0 = *tangents.first().ok_or_else(bad)?;
    let f: Vec<[Vec3; 3]> = match ctl.orientation {
        SweepOrientation::Perpendicular => frames(pts, tangents).ok_or_else(|| KernelError::Invalid("the sweep path doubles back".into()))?,
        SweepOrientation::Parallel => vec![[Vec3::X, Vec3::Y, Vec3::Z]; pts.len()],
        SweepOrientation::Direction(d) => frames_toward(tangents, d)?,
    };
    let f0 = *f.first().ok_or_else(bad)?;
    // Twist and scale act about the profile's normal, pointing the way the path leaves it.
    let n = plane.normal().normalized().ok_or_else(|| KernelError::Invalid("sweep frame".into()))?;
    let axis = if n.dot(t0) < 0.0 { n * -1.0 } else { n };
    // Kept as drawn, the profile flattens to nothing where the path runs along its plane: the
    // path has to cross it at 5 degrees or more all the way.
    if ctl.orientation == SweepOrientation::Parallel && tangents.iter().any(|t| t.dot(axis) < 5f64.to_radians().sin()) {
        return Err(KernelError::Invalid(
            "kept parallel, the profile would lie along the sweep path: the path must keep crossing the profile's plane (by 5 deg or more)".into(),
        ));
    }
    let mut run = Vec::with_capacity(pts.len());
    let mut total = 0.0;
    for (i, p) in pts.iter().enumerate() {
        if let Some(prev) = i.checked_sub(1).and_then(|j| pts.get(j)) {
            total += prev.dist(*p);
        }
        run.push(total);
    }
    if !(total > 1e-12) {
        return Err(KernelError::Invalid("the sweep path has no length".into()));
    }
    let mut out = Vec::with_capacity(f.len());
    for (i, fi) in f.iter().enumerate() {
        let u = run.get(i).copied().unwrap_or(total) / total;
        let (sin, cos) = (ctl.twist * u).sin_cos();
        let k = 1.0 + (ctl.scale - 1.0) * u;
        let shape = |v: Vec3| {
            let along = axis * v.dot(axis);
            let r = (v - along) * k;
            along + r * cos + axis.cross(r) * sin
        };
        let col = |e: Vec3| {
            let v = shape(e);
            fi[0] * f0[0].dot(v) + fi[1] * f0[1].dot(v) + fi[2] * f0[2].dot(v)
        };
        let mut m = [col(Vec3::X), col(Vec3::Y), col(Vec3::Z)];
        if ctl.orientation != SweepOrientation::Parallel
            && let Some(Some((b, s))) = mitre.get(i)
        {
            for c in &mut m {
                *c += *b * ((s - 1.0) * c.dot(*b));
            }
        }
        out.push(Station { at: *pts.get(i).ok_or_else(bad)?, m });
    }
    Ok(out)
}

/// Where a rail polyline crosses the plane through `p` square to `t`: the first crossing at or
/// after `from` (segment index and fraction along it), with the position it was found at, so a
/// walk along the path finds each rail point after the last. Within `tol` of the plane a rail's
/// first and last points count as on it (a rail drawn to end where the path ends).
fn rail_crossing(rail: &[Vec3], p: Vec3, t: Vec3, from: (usize, f64), tol: f64) -> Option<(Vec3, (usize, f64))> {
    let f = |q: Vec3| (q - p).dot(t);
    let (k0, u0) = from;
    if k0 == 0 && u0 == 0.0 && rail.first().is_some_and(|q| f(*q).abs() <= tol) {
        return rail.first().map(|q| (*q, (0, 0.0)));
    }
    for k in k0..rail.len().saturating_sub(1) {
        let (a, b) = (*rail.get(k)?, *rail.get(k + 1)?);
        let a = if k == k0 { a.lerp(b, u0) } else { a };
        let (fa, fb) = (f(a), f(b));
        if fa == 0.0 && (k != k0 || u0 == 0.0) {
            return Some((a, (k, if k == k0 { u0 } else { 0.0 })));
        }
        if (fa < 0.0) != (fb < 0.0) || fb == 0.0 {
            let s = fa / (fa - fb);
            let q = a.lerp(b, s);
            // The fraction along the whole segment, for the next walk.
            let base = if k == k0 { u0 } else { 0.0 };
            return Some((q, (k, base + (1.0 - base) * s)));
        }
    }
    let last = rail.last()?;
    (f(*last).abs() <= tol).then(|| (*last, (rail.len().saturating_sub(2), 1.0)))
}

/// Stations shaped by guide rails (see [`SweepControl`]): each is square to the path, turned
/// about it to follow the rail (the line between the rails) and scaled or stretched by the
/// rail's distance across (the rails' distance apart) against the start's. A rail has to meet
/// the plane of every station, in order along its length.
fn railed_stations(pts: &[Vec3], tangents: &[Vec3], mitre: &[Option<(Vec3, f64)>], ctl: &SweepControl) -> Result<Vec<Station>> {
    // A polyline path's points bend it a little (its arcs' chords); a corner is a bend past
    // [`CORNER`], where the stations would have to be mitred.
    let corner = 1.0 / (CORNER / 2.0).cos();
    if mitre.iter().flatten().any(|(_, s)| *s > corner + 1e-12) {
        return Err(KernelError::Invalid("not supported yet: guide rails along a path with sharp corners".into()));
    }
    let rails: Vec<Vec<Vec3>> = ctl.rails.iter().map(|r| clean(r)).collect::<Result<_>>()?;
    let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    let tol = 1e-3f64.max(1e-6 * len);
    let mut cursors = vec![(0usize, 0.0f64); rails.len()];
    // At each station: the anchor the profile moves with, and the vector across that turns and
    // sizes it (to the rail, or from the first rail to the second).
    let mut across = Vec::with_capacity(pts.len());
    for (i, (p, t)) in pts.iter().zip(tangents).enumerate() {
        let mut hits = Vec::with_capacity(rails.len());
        for (rail, cursor) in rails.iter().zip(cursors.iter_mut()) {
            let Some((q, at)) = rail_crossing(rail, *p, *t, *cursor, tol) else {
                let done = 100.0 * i as f64 / pts.len().saturating_sub(1).max(1) as f64;
                return Err(KernelError::Invalid(format!(
                    "a guide rail does not reach the path's plane {done:.0}% of the way along it: the rail has to run the whole length of the path"
                )));
            };
            *cursor = at;
            hits.push(q);
        }
        let (anchor, v) = match hits[..] {
            [r] => (*p, r - *p),
            [a, b] => (a, b - a),
            _ => return Err(KernelError::Invalid("a sweep takes one or two guide rails".into())),
        };
        across.push((anchor, v - *t * v.dot(*t)));
    }
    let (Some(&(a0, d0)), Some(&t0)) = (across.first(), tangents.first()) else { return Err(KernelError::Invalid("sweep path".into())) };
    let s0 = *pts.first().ok_or_else(|| KernelError::Invalid("sweep path".into()))?;
    let near = |d: Vec3| d.len() <= 1e-6 * (1.0 + len);
    if near(d0) {
        return Err(KernelError::Invalid(if rails.len() == 1 {
            "the guide rail starts on the path: it has to start away from the path, at the point of the profile it guides".into()
        } else {
            "the two guide rails start at the same point".into()
        }));
    }
    let (u0, w0) = {
        let u = d0.normalized().ok_or_else(|| KernelError::Invalid("guide rail".into()))?;
        (u, t0.cross(u))
    };
    let mut out = Vec::with_capacity(pts.len());
    for ((anchor, d), t) in across.iter().zip(tangents) {
        if near(*d) {
            return Err(KernelError::Invalid(if rails.len() == 1 {
                format!("the guide rail meets the path near ({:.3}, {:.3}, {:.3}): the profile would shrink to nothing", anchor.x, anchor.y, anchor.z)
            } else {
                format!("the guide rails meet near ({:.3}, {:.3}, {:.3}): the profile would shrink to nothing", anchor.x, anchor.y, anchor.z)
            }));
        }
        let k = d.len() / d0.len();
        let k_other = match ctl.rail_fit {
            RailFit::Scale => k,
            RailFit::Stretch => 1.0,
        };
        let u = d.normalized().ok_or_else(|| KernelError::Invalid("guide rail".into()))?;
        let w = t.cross(u);
        // Along the path stays along it; across to the rail turns onto the rail and sizes with
        // it; the third way follows, scaled evenly or kept.
        let map = |v: Vec3| *t * v.dot(t0) + u * (k * v.dot(u0)) + w * (k_other * v.dot(w0));
        let m = [map(Vec3::X), map(Vec3::Y), map(Vec3::Z)];
        let st = Station { at: Vec3::default(), m };
        // The anchor's start position lands on the anchor.
        out.push(Station { at: *anchor - st.apply(a0 - s0), m });
    }
    Ok(out)
}

/// The volume a profile of `area` on `plane` sweeps through the stations: each station's
/// section area times how squarely the path crosses it, summed along the path (trapezoids).
fn swept_volume(plane: &Plane, area: f64, st: &[Station], tangents: &[Vec3]) -> f64 {
    let rate = |i: usize| match (st.get(i), tangents.get(i)) {
        (Some(s), Some(t)) => area * s.apply(plane.x).cross(s.apply(plane.y)).dot(*t).abs(),
        _ => 0.0,
    };
    st.windows(2).enumerate().map(|(i, w)| 0.5 * (rate(i) + rate(i + 1)) * w[0].at.dist(w[1].at)).sum()
}

/// The stations of a smooth sweep: as many as the path has points (4…[`MAX_SMOOTH_STATIONS`]),
/// more when the profile twists or scales.
fn smooth_count(points: usize, ctl: &SweepControl) -> usize {
    let n = points.saturating_sub(1).clamp(4, MAX_SMOOTH_STATIONS);
    if !ctl.varies() {
        return n;
    }
    let twist = (ctl.twist.abs() / SMOOTH_TWIST_STEP).ceil() as usize;
    n.max(MIN_CONTROLLED_STATIONS).max(twist).min(MAX_CONTROLLED_STATIONS)
}

/// The path with points added along its straight runs (its corners kept) when the profile
/// twists or scales, so ruled walls follow the change.
fn densify(pts: &[Vec3], ctl: &SweepControl) -> Vec<Vec3> {
    if !ctl.varies() {
        return pts.to_vec();
    }
    let total: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    let want = MIN_CONTROLLED_RULED.max((ctl.twist.abs() / RULED_TWIST_STEP).ceil() as usize).min(MAX_RULED_STATIONS / 2);
    let mut out = Vec::with_capacity(want + pts.len());
    out.extend(pts.first().copied());
    for w in pts.windows(2) {
        let pieces = if total > 0.0 { ((w[0].dist(w[1]) / total) * want as f64).ceil() as usize } else { 1 };
        let pieces = pieces.clamp(1, want.max(1));
        out.extend((1..=pieces).map(|j| w[0].lerp(w[1], j as f64 / pieces as f64)));
    }
    out
}

/// The profile plane carried to a station.
fn station_plane(plane: &Plane, s0: Vec3, st: &Station) -> Plane {
    Plane { origin: st.at + st.apply(plane.origin - s0), x: st.apply(plane.x), y: st.apply(plane.y) }
}

/// Sweep a planar region along a 3D polyline path. The profile keeps its relation to the path
/// (it need not touch it); the path should start near the profile.
pub fn sweep_path(plane: &Plane, region: &Region2, path: &[Vec3]) -> Result<Body> {
    sweep_path_with(plane, region, path, &SweepControl::default())
}

/// [`sweep_path`] with sweep options: orientation, twist and end scale.
pub fn sweep_path_with(plane: &Plane, region: &Region2, path: &[Vec3], ctl: &SweepControl) -> Result<Body> {
    ctl.check()?;
    if region.area().abs() < 1e-9 {
        return Err(KernelError::Invalid("profile has no area".into()));
    }
    let pts = clean(path)?;
    let corner = pts.windows(3).any(|w| turn(w[1] - w[0], w[2] - w[1]) > CORNER);
    let profile = crate::numbered::number_regions(std::slice::from_ref(region))
        .pop()
        .map(|r| r.oriented())
        .ok_or_else(|| KernelError::Invalid("profile".into()))?;
    if !corner && region.holes.is_empty() {
        // Smooth: evenly spaced stations, tangents from neighbours.
        let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
        let n = smooth_count(pts.len(), ctl);
        let sp = resample(&pts, n);
        let tangents = central_tangents(&sp)?;
        let (st, expected) = if ctl.is_default() {
            (stations(&sp, &tangents, &[])?, region.area().abs() * len * start_cos(plane, &tangents))
        } else {
            let st = controlled_stations(plane, &sp, &tangents, &[], ctl)?;
            let v = swept_volume(plane, region.area().abs(), &st, &tangents);
            (st, v)
        };
        if closed_path(&pts) {
            let offset = closing_offset(plane, &profile.outer, &st)?;
            if let Ok(b) = smooth_closed_walls(plane, &profile.outer, &st, offset, expected) {
                return Ok(b);
            }
        } else if let Ok(b) = smooth_walls(plane, &profile.outer, &st, expected) {
            return Ok(b);
        }
    }
    ruled(plane, &profile, &pts, ctl)
}

fn central_tangents(p: &[Vec3]) -> Result<Vec<Vec3>> {
    let n = p.len();
    let closed = closed_path(p);
    (0..n)
        .map(|i| {
            let a = p.get(if closed && (i == 0 || i + 1 == n) { n - 2 } else { i.saturating_sub(1) }).copied().unwrap_or_default();
            let b = p.get(if closed && (i == 0 || i + 1 == n) { 1 } else { (i + 1).min(n - 1) }).copied().unwrap_or_default();
            (b - a).normalized().ok_or_else(|| KernelError::Invalid("the sweep path has no direction".into()))
        })
        .collect()
}

/// How squarely the path leaves the profile (volume = area × length × this).
fn start_cos(plane: &Plane, tangents: &[Vec3]) -> f64 {
    tangents.first().map(|t| t.dot(plane.normal()).abs()).unwrap_or(1.0).max(1e-3)
}

/// Stations at the path's points (at most [`MAX_RULED_STATIONS`]), mitred at corners, with the
/// points they stand at.
fn ruled_stations(plane: &Plane, pts: &[Vec3], ctl: &SweepControl) -> Result<(Vec<Vec3>, Vec<Station>)> {
    let pts = densify(pts, ctl);
    let pts: Vec<Vec3> = if pts.len() > MAX_RULED_STATIONS { resample(&pts, MAX_RULED_STATIONS) } else { pts };
    let n = pts.len();
    let closed = closed_path(&pts);
    let seg_dir = |i: usize| -> Option<Vec3> { (*pts.get(i + 1)? - *pts.get(i)?).normalized() };
    let mut tangents = Vec::with_capacity(n);
    let mut mitre = Vec::with_capacity(n);
    for i in 0..n {
        let d_in = if i > 0 {
            seg_dir(i - 1)
        } else if closed {
            seg_dir(n - 2)
        } else {
            None
        };
        let d_out = if i + 1 < n {
            seg_dir(i)
        } else if closed {
            seg_dir(0)
        } else {
            None
        };
        let (t, m) = match (d_in, d_out) {
            (Some(a), Some(b)) => {
                let t = (a + b).normalized().ok_or_else(|| KernelError::Invalid("the sweep path turns back on itself".into()))?;
                let half = turn(a, b) / 2.0;
                // Across the bend the section stretches by 1 / cos(half the turn).
                let m = (b - a).normalized().map(|bend| (bend, 1.0 / half.cos().max(0.2)));
                (t, m)
            }
            (Some(a), None) => (a, None),
            (None, Some(b)) => (b, None),
            (None, None) => return Err(KernelError::Invalid("sweep path".into())),
        };
        tangents.push(t);
        mitre.push(m);
    }
    let st = if ctl.is_default() { stations(&pts, &tangents, &mitre)? } else { controlled_stations(plane, &pts, &tangents, &mitre, ctl)? };
    Ok((pts, st))
}

/// Walls of chains of sketch segments on `plane` (open, or closed) carried along a 3D polyline
/// path that starts near them: ruled between stations at the path's points, mitred at corners,
/// with no caps. Neighbouring walls share their edges.
pub(crate) fn ruled_sheet(plane: &Plane, chains: &[Vec<Seg2>], path: &[Vec3], ctl: &SweepControl) -> Result<Vec<Side>> {
    ctl.check()?;
    let (pts, st) = ruled_stations(plane, &clean(path)?, ctl)?;
    let edges: usize = chains.iter().map(Vec::len).sum();
    if st.len().saturating_mul(edges) > 400_000 {
        return Err(KernelError::Invalid("the sweep is too detailed (curves × path points)".into()));
    }
    let s0 = pts.first().copied().unwrap_or_default();
    // Each chain's segments are numbered on from the last chain's; every station's wire of a
    // chain has the same edges, each over the same segment.
    let offsets: Vec<usize> = chains.iter().scan(0, |o, c| Some(std::mem::replace(o, *o + c.len()))).collect();
    let mut wires: Vec<Vec<mt::Wire>> = Vec::with_capacity(st.len());
    let mut numbers: Vec<Vec<Vec<usize>>> = Vec::new();
    for s in &st {
        let pl = station_plane(plane, s0, s);
        let mut at = Vec::with_capacity(chains.len());
        for (c, offset) in chains.iter().zip(&offsets) {
            let (wire, seeds) = crate::build::chain_wire_seeds(&pl, c)?;
            if numbers.len() < chains.len() {
                numbers.push(seeds.iter().map(|(_, i)| vec![offset + i]).collect());
            }
            at.push(wire);
        }
        wires.push(at);
    }
    guard("path sweep", || {
        let mut faces: Vec<Side> = Vec::new();
        for w in wires.windows(2) {
            let (Some(w0), Some(w1)) = (w.first(), w.get(1)) else { continue };
            for ((a, b), n) in w0.iter().zip(w1).zip(&numbers) {
                let sides: mt::Shell = builder::try_wire_homotopy(a, b).map_err(|e| KernelError::Failed(format!("path sweep: {e}")))?;
                faces.extend(record::ruled(&sides, a, b, n, n));
            }
        }
        Ok(faces)
    })
}

/// Walls of chains of sketch segments on `plane` carried along a smooth 3D polyline path (no
/// turn sharper than [`CORNER`]): one B-spline wall per segment through evenly spaced,
/// rotation-minimising stations, neighbouring walls sharing their seam edges; no caps. `None`
/// for a path with corners (see [`ruled_sheet`]).
pub(crate) fn smooth_sheet(plane: &Plane, chains: &[Vec<Seg2>], path: &[Vec3], ctl: &SweepControl) -> Result<Option<Vec<Side>>> {
    ctl.check()?;
    let pts = clean(path)?;
    if pts.windows(3).any(|w| turn(w[1] - w[0], w[2] - w[1]) > CORNER) {
        return Ok(None);
    }
    let fail = |w: &str| KernelError::Failed(format!("path sweep: {w}"));
    if chains.iter().map(Vec::len).sum::<usize>() > 4096 {
        return Err(fail("too many curves for a smooth sweep"));
    }
    let n = smooth_count(pts.len(), ctl);
    let sp = resample(&pts, n);
    let tangents = central_tangents(&sp)?;
    let st = if ctl.is_default() { stations(&sp, &tangents, &[])? } else { controlled_stations(plane, &sp, &tangents, &[], ctl)? };
    let s0 = st.first().map(|s| s.at).unwrap_or_default();
    let at = |i: usize, q: Vec2| st.get(i).map(|s| s.at + s.apply(plane.to_world(q) - s0)).unwrap_or_default();
    let n = st.len() - 1;
    let mut faces: Vec<Side> = Vec::new();
    let mut offset = 0;
    for chain in chains {
        let closed = chain.first().zip(chain.last()).is_some_and(|(f, l)| l.end().dist(f.start()) <= 1e-6);
        // Each segment: its curve at the first and last station, the columns through its start
        // and end, and the wall surface.
        let mut data = Vec::with_capacity(chain.len());
        for s in chain {
            let mut station_curves = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let q: Vec<Vec3> = (0..SEG_SAMPLES).map(|j| at(i, s.point_at(j as f64 / (SEG_SAMPLES - 1) as f64))).collect();
                station_curves.push(interpolate_cubic(&q).ok_or_else(|| fail("profile curve"))?);
            }
            let kv = station_curves.first().map(|c| c.knot_vec().clone()).ok_or_else(|| fail("no stations"))?;
            let mut columns = Vec::with_capacity(SEG_SAMPLES);
            for j in 0..SEG_SAMPLES {
                let q: Vec<Vec3> = station_curves
                    .iter()
                    .map(|c| {
                        let p = c.control_point(j);
                        Vec3::new(p.x, p.y, p.z)
                    })
                    .collect();
                columns.push(interpolate_cubic(&q).ok_or_else(|| fail("path curve"))?);
            }
            let ku = columns.first().map(|c| c.knot_vec().clone()).ok_or_else(|| fail("no columns"))?;
            let net: Vec<Vec<mt::Point3>> = (0..=n).map(|m| columns.iter().map(|c| *c.control_point(m)).collect()).collect();
            let surf = mt::BSplineSurface::try_new((ku, kv), net).map_err(|e| fail(&e.to_string()))?;
            let (Some(c0), Some(c1), Some(first), Some(last)) =
                (station_curves.first().cloned(), station_curves.last().cloned(), columns.first().cloned(), columns.last().cloned())
            else {
                return Err(fail("stations"));
            };
            data.push((c0, c1, first, last, surf));
        }
        let walls: Vec<mt::Face> = guard("path sweep", || {
            let m = chain.len();
            // Vertices along the chain (an open chain has one more than it has segments).
            let corners: Vec<Vec2> = chain.iter().map(Seg2::start).chain(chain.last().map(Seg2::end).filter(|_| !closed)).collect();
            let start: Vec<mt::Vertex> = corners.iter().map(|q| builder::vertex(p3(at(0, *q)))).collect();
            let end: Vec<mt::Vertex> = corners.iter().map(|q| builder::vertex(p3(at(n, *q)))).collect();
            let next = |j: usize| if closed { (j + 1) % m } else { j + 1 };
            let mut seams = Vec::with_capacity(corners.len());
            for (k, (a, b)) in start.iter().zip(&end).enumerate() {
                // The column through each corner: the start column of the segment leaving it, or
                // the end column of the last segment.
                let col = match data.get(k) {
                    Some((_, _, first, _, _)) => first,
                    None => data.last().map(|d| &d.3).ok_or_else(|| fail("seam"))?,
                };
                seams.push(mt::Edge::try_new(a, b, mt::Curve::BSplineCurve(col.clone())).map_err(|e| fail(&e.to_string()))?);
            }
            let mut walls = Vec::with_capacity(m);
            for (j, (c0, c1, _, _, surf)) in data.iter().enumerate() {
                let (Some(a0), Some(b0), Some(a1), Some(b1), Some(sa), Some(sb)) =
                    (start.get(j), start.get(next(j)), end.get(j), end.get(next(j)), seams.get(j), seams.get(next(j)))
                else {
                    return Err(fail("edges"));
                };
                let e0 = mt::Edge::try_new(a0, b0, mt::Curve::BSplineCurve(c0.clone())).map_err(|e| fail(&e.to_string()))?;
                let e1 = mt::Edge::try_new(a1, b1, mt::Curve::BSplineCurve(c1.clone())).map_err(|e| fail(&e.to_string()))?;
                let w: mt::Wire = vec![sa.clone(), e1, sb.inverse(), e0.inverse()].into();
                walls.push(mt::Face::try_new(vec![w], mt::Surface::BSplineSurface(surf.clone())).map_err(|e| fail(&e.to_string()))?);
            }
            Ok(walls)
        })?;
        // Wall j comes from segment j of the chain.
        faces.extend(walls.into_iter().enumerate().map(|(j, w)| (w, Some(record::profile(&[offset + j])))));
        offset += chain.len();
    }
    Ok(Some(faces))
}

/// Ruled walls between stations at the path's points (mitred at corners), every loop of the
/// (oriented) profile.
fn ruled(plane: &Plane, profile: &NumberedRegion, pts: &[Vec3], ctl: &SweepControl) -> Result<Body> {
    let (pts, st) = ruled_stations(plane, pts, ctl)?;
    let s0 = pts.first().copied().unwrap_or_default();
    let loops: Vec<&NumberedLoop> = profile.loops().collect();
    let mut wires: Vec<Vec<mt::Wire>> = Vec::with_capacity(st.len());
    for s in &st {
        let pl = station_plane(plane, s0, s);
        wires.push(loops.iter().map(|l| uniform_wire(&pl, &l.lp)).collect::<Result<Vec<_>>>()?);
    }
    let closed = closed_path(&pts);
    if closed {
        let offsets = loops.iter().map(|l| closing_offset(plane, l, &st)).collect::<Result<Vec<_>>>()?;
        let first = wires.first().ok_or_else(|| KernelError::Invalid("sweep path".into()))?;
        let end: Vec<mt::Wire> = first.iter().zip(offsets).map(|(w, k)| rotated_wire(w, k)).collect();
        if let Some(last) = wires.last_mut() {
            *last = end;
        }
    }
    guard("path sweep", || {
        // Every station's wires have the loops' segments, edge for edge.
        let mut faces: Vec<Side> = Vec::new();
        for w in wires.windows(2) {
            let (Some(w0), Some(w1)) = (w.first(), w.get(1)) else { continue };
            for ((a, b), l) in w0.iter().zip(w1).zip(&loops) {
                let sides = builder::try_wire_homotopy(a, b).map_err(|e| KernelError::Failed(format!("path sweep: {e}")))?;
                faces.extend(record::ruled(&sides, a, b, &l.from, &l.from));
            }
        }
        if closed {
            return close_sweep(faces);
        }
        let (Some(first), Some(last)) = (wires.first(), wires.last()) else { return Err(KernelError::Failed("path sweep".into())) };
        cap_and_close(faces, first, last, false)
    })
}

/// Two smooth runs share their middle section and the closing section. Splitting the run
/// avoids edges from a vertex back to itself when the profile returns without a permutation.
fn smooth_closed_walls(plane: &Plane, profile: &NumberedLoop, st: &[Station], offset: usize, expected: f64) -> Result<Body> {
    let fail = || KernelError::Failed("closed path sweep: the walls do not close a solid".into());
    if profile.lp.segs.len() > 64 || st.len() < 7 {
        return Err(fail());
    }
    guard("closed path sweep", || {
        let first = st.first().ok_or_else(fail)?;
        let mut wires = st.iter().map(|s| uniform_wire(&station_plane(plane, first.at, s), &profile.lp)).collect::<Result<Vec<_>>>()?;
        let end = rotated_wire(wires.first().ok_or_else(fail)?, offset);
        if let Some(last) = wires.last_mut() {
            *last = end;
        }
        let middle = (wires.len() - 1) / 2;
        let numbers = vec![profile.from.clone(); wires.len()];
        let mut sides = smooth_loft_faces(wires.get(..=middle).ok_or_else(fail)?, numbers.get(..=middle).ok_or_else(fail)?, true)?;
        sides.extend(smooth_loft_faces(wires.get(middle..).ok_or_else(fail)?, numbers.get(middle..).ok_or_else(fail)?, true)?);
        let b = close_sweep(sides)?;
        let tol = (profile.lp.signed_area().abs().sqrt() * 0.05).max(1e-3);
        if b.tessellate(tol).is_ok_and(|m| (m.measure().volume - expected).abs() <= 0.15 * expected.abs()) { Ok(b) } else { Err(fail()) }
    })
}

/// One B-spline wall per profile segment through the stations, capped; checked by volume.
fn smooth_walls(plane: &Plane, profile: &NumberedLoop, st: &[Station], expected: f64) -> Result<Body> {
    let lp = &profile.lp;
    let nseg = lp.segs.len();
    let fail = |w: &str| KernelError::Failed(format!("path sweep: {w}"));
    if nseg == 0 || nseg > 64 || st.len() < 4 {
        return Err(fail("profile"));
    }
    let s0 = st.first().map(|s| s.at).unwrap_or_default();
    let at = |i: usize, q: Vec2| st.get(i).map(|s| s.at + s.apply(plane.to_world(q) - s0)).unwrap_or_default();
    let n = st.len() - 1;
    let mut data = Vec::with_capacity(nseg);
    for s in &lp.segs {
        let mut station_curves = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let pts: Vec<Vec3> = (0..SEG_SAMPLES).map(|j| at(i, s.point_at(j as f64 / (SEG_SAMPLES - 1) as f64))).collect();
            station_curves.push(interpolate_cubic(&pts).ok_or_else(|| fail("profile curve"))?);
        }
        let kv = station_curves.first().map(|c| c.knot_vec().clone()).ok_or_else(|| fail("no stations"))?;
        let mut columns = Vec::with_capacity(SEG_SAMPLES);
        for j in 0..SEG_SAMPLES {
            let pts: Vec<Vec3> = station_curves
                .iter()
                .map(|c| {
                    let p = c.control_point(j);
                    Vec3::new(p.x, p.y, p.z)
                })
                .collect();
            columns.push(interpolate_cubic(&pts).ok_or_else(|| fail("path curve"))?);
        }
        let ku = columns.first().map(|c| c.knot_vec().clone()).ok_or_else(|| fail("no columns"))?;
        let net: Vec<Vec<mt::Point3>> = (0..=n).map(|m| columns.iter().map(|c| *c.control_point(m)).collect()).collect();
        let surf = mt::BSplineSurface::try_new((ku, kv), net).map_err(|e| fail(&e.to_string()))?;
        let (Some(first), Some(last)) = (station_curves.first().cloned(), station_curves.last().cloned()) else { return Err(fail("stations")) };
        let seam = columns.first().cloned().ok_or_else(|| fail("seam"))?;
        data.push((first, last, seam, surf));
    }
    guard("path sweep", || {
        let start: Vec<mt::Vertex> = lp.segs.iter().map(|s| builder::vertex(p3(at(0, s.start())))).collect();
        let end: Vec<mt::Vertex> = lp.segs.iter().map(|s| builder::vertex(p3(at(n, s.start())))).collect();
        let (mut e0, mut e1, mut seams) = (Vec::with_capacity(nseg), Vec::with_capacity(nseg), Vec::with_capacity(nseg));
        for (j, (c0, c1, seam, _)) in data.iter().enumerate() {
            let (Some(a0), Some(b0), Some(a1), Some(b1)) = (start.get(j), start.get((j + 1) % nseg), end.get(j), end.get((j + 1) % nseg)) else {
                return Err(fail("vertices"));
            };
            e0.push(mt::Edge::try_new(a0, b0, mt::Curve::BSplineCurve(c0.clone())).map_err(|e| fail(&e.to_string()))?);
            e1.push(mt::Edge::try_new(a1, b1, mt::Curve::BSplineCurve(c1.clone())).map_err(|e| fail(&e.to_string()))?);
            seams.push(mt::Edge::try_new(a0, a1, mt::Curve::BSplineCurve(seam.clone())).map_err(|e| fail(&e.to_string()))?);
        }
        let mut walls = Vec::with_capacity(nseg);
        for (j, (_, _, _, surf)) in data.iter().enumerate() {
            let (Some(a), Some(b), Some(sa), Some(sb)) = (e0.get(j), e1.get(j), seams.get(j), seams.get((j + 1) % nseg)) else {
                return Err(fail("edges"));
            };
            let w: mt::Wire = vec![sa.clone(), b.clone(), sb.inverse(), a.inverse()].into();
            let wall = mt::Face::try_new(vec![w], mt::Surface::BSplineSurface(surf.clone())).map_err(|e| fail(&e.to_string()))?;
            walls.push((wall, Some(record::profile(profile.numbers(j)))));
        }
        let w0: mt::Wire = e0.iter().cloned().collect();
        let w1: mt::Wire = e1.iter().cloned().collect();
        let tol = (lp.signed_area().abs().sqrt() * 0.05).max(1e-3);
        for ws in [walls.clone(), walls.iter().map(|(f, prov)| (f.inverse(), prov.clone())).collect()] {
            if let Ok(b) = cap_and_close(ws, std::slice::from_ref(&w0), std::slice::from_ref(&w1), false)
                && b.tessellate(tol).is_ok_and(|m| (m.measure().volume - expected).abs() <= 0.15 * expected.abs())
            {
                return Ok(b);
            }
        }
        Err(fail("the walls do not close a solid"))
    })
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    use solidcraft_geom::{Loop2, Plane, Region2, Vec2, Vec3};

    use super::{SweepControl, SweepOrientation, sweep_path, sweep_path_with};

    fn circle(r: f64) -> Region2 {
        Region2 { outer: Loop2::circle(Vec2::ZERO, r), holes: Vec::new() }
    }

    #[test]
    fn sweeps_along_3d_paths() {
        // A quarter of a helix-like 3D curve: a smooth wall, volume ≈ area × length.
        let path: Vec<Vec3> = (0..=40)
            .map(|i| {
                let t = i as f64 / 40.0 * std::f64::consts::FRAC_PI_2;
                Vec3::new(30.0 * t.sin(), 30.0 * (1.0 - t.cos()), 10.0 * t)
            })
            .collect();
        let t0 = (path[1] - path[0]).normalized().unwrap();
        let plane = Plane::from_normal(path[0], t0).unwrap();
        let b = sweep_path(&plane, &circle(2.0), &path).unwrap();
        let len: f64 = path.windows(2).map(|w| w[0].dist(w[1])).sum();
        let v = b.tessellate(0.01).unwrap().measure().volume;
        let want = std::f64::consts::PI * 4.0 * len;
        assert!((v - want).abs() < 0.02 * want, "{v} vs {want}");
        // One wall per profile segment and two caps (ruled walls would give dozens).
        assert!(b.faces(0.01).unwrap().len() <= 8, "smooth walls: {}", b.faces(0.01).unwrap().len());
        // An L-shaped path (a sharp corner) with a holed square: mitred ruled walls.
        let path = [Vec3::ZERO, Vec3::new(0.0, 0.0, 20.0), Vec3::new(20.0, 0.0, 20.0)];
        let plane = Plane::from_normal(Vec3::ZERO, Vec3::Z).unwrap();
        let sq = |h: f64| Loop2::polygon(&[Vec2::new(-h, -h), Vec2::new(h, -h), Vec2::new(h, h), Vec2::new(-h, h)]);
        let region = Region2 { outer: sq(3.0), holes: vec![sq(1.0).reversed()] };
        let b = sweep_path(&plane, &region, &path).unwrap();
        let v = b.tessellate(0.01).unwrap().measure().volume;
        let want = 32.0 * 40.0;
        assert!((v - want).abs() < 0.03 * want, "{v} vs {want}");
        assert!(sweep_path(&plane, &region, &[Vec3::ZERO]).is_err());
        assert!(sweep_path(&plane, &region, &[Vec3::ZERO, Vec3::new(f64::NAN, 0.0, 0.0)]).is_err());
    }

    fn square(h: f64) -> Region2 {
        Region2 { outer: Loop2::polygon(&[Vec2::new(-h, -h), Vec2::new(h, -h), Vec2::new(h, h), Vec2::new(-h, h)]), holes: Vec::new() }
    }

    fn up(h: f64) -> [crate::PathSeg; 1] {
        [crate::PathSeg::Line { a: Vec3::ZERO, b: Vec3::new(0.0, 0.0, h) }]
    }

    /// A circle swept along a line to an end scale is a cone frustum: its volume and lateral
    /// area are analytic, for the solid and for the surface sweep.
    #[test]
    fn scaled_sweep_is_a_frustum() {
        let (big, h, k) = (10.0, 40.0, 0.5);
        let small = big * k;
        let ctl = SweepControl { scale: k, ..SweepControl::default() };
        let b = crate::sweep_with(&Plane::XY, &circle(big), &up(h), &ctl).unwrap();
        let m = b.tessellate(0.005).unwrap().measure();
        let volume = PI * h * (big * big + big * small + small * small) / 3.0;
        let lateral = PI * (big + small) * (h * h + (big - small).powi(2)).sqrt();
        let caps = PI * (big * big + small * small);
        assert!((m.volume - volume).abs() < 2e-3 * volume, "volume {} vs {volume}", m.volume);
        assert!((m.area - caps - lateral).abs() < 2e-3 * lateral, "lateral {} vs {lateral}", m.area - caps);
        // The end cap is the scaled circle.
        let top = b.tessellate(0.005).unwrap().positions.iter().filter(|p| (p.z - h).abs() < 1e-6).map(|p| p.x.hypot(p.y)).fold(0.0, f64::max);
        assert!((top - small).abs() < 1e-3, "end radius {top}");
        // The surface sweep of the circle's curve: the frustum's lateral face alone.
        let sheet = crate::sweep_sheet_with(&Plane::XY, std::slice::from_ref(&circle(big).outer.segs), &up(h), &ctl).unwrap();
        let a = crate::measure(&sheet).unwrap().area;
        assert!((a - lateral).abs() < 2e-3 * lateral, "sheet {a} vs {lateral}");
    }

    /// The corners of a twisted square: each seam edge's angle at `z`.
    fn corner_angles(b: &crate::Body, z: f64) -> Vec<f64> {
        let mesh = b.tessellate(0.002).unwrap();
        let mut out = Vec::new();
        for e in &mesh.edges {
            for w in e.windows(2) {
                let (a, c) = (w[0], w[1]);
                if (a.z - z) * (c.z - z) <= 0.0 && (a.z - c.z).abs() > 1e-9 {
                    let p = a.lerp(c, (z - a.z) / (c.z - a.z));
                    out.push(p.y.atan2(p.x));
                }
            }
        }
        out
    }

    /// A square swept along a line with a twist: the section halfway along is turned by half the
    /// twist, the end by all of it; the volume is the prism's.
    #[test]
    fn twisted_sweep_turns_in_proportion() {
        let (h, twist) = (40.0, 90f64.to_radians());
        let ctl = SweepControl { twist, ..SweepControl::default() };
        let b = crate::sweep_with(&Plane::XY, &square(5.0), &up(h), &ctl).unwrap();
        // Twisted walls are saddles: a fine mesh to measure them.
        let v = b.tessellate(0.0005).unwrap().measure().volume;
        assert!((v - 100.0 * h).abs() < 2e-3 * 100.0 * h, "volume {v}");
        // Corners start at 45 deg (and 135, -45, -135): each seam turns by the twist so far.
        let off = |a: f64, want: f64| {
            let d = (a - want).rem_euclid(FRAC_PI_2);
            d.min(FRAC_PI_2 - d)
        };
        for (z, turned) in [(h / 2.0, twist / 2.0), (h, twist), (h / 4.0, twist / 4.0)] {
            let angles = corner_angles(&b, z);
            assert!(angles.len() >= 4, "seams at {z}: {angles:?}");
            for a in &angles {
                assert!(off(*a, FRAC_PI_4 + turned) < 0.2f64.to_radians(), "at {z}: {} vs {}", a.to_degrees(), (FRAC_PI_4 + turned).to_degrees());
            }
        }
        // Right hand about the direction of travel: a positive twist going up turns
        // counter-clockwise seen from above (the corner at 45 deg reaches 90 deg + 45 deg).
        let end = corner_angles(&b, h);
        assert!(end.iter().any(|a| (a - (FRAC_PI_4 + twist)).abs() < 0.2f64.to_radians()), "{end:?}");
        // One wall per side and two caps, as an untwisted sweep.
        assert_eq!(b.faces(0.01).unwrap().len(), 6);
    }

    /// Parallel keeps the profile as drawn along a curved path: the volume is its area times
    /// the path's rise across the profile's plane.
    #[test]
    fn parallel_sweep_keeps_the_profile_square_to_its_normal() {
        // An eighth of a circle: the path ends leaning 45 deg from the profile's normal.
        let r = 30.0;
        let path: Vec<Vec3> = (0..=60)
            .map(|i| {
                let t = i as f64 / 60.0 * FRAC_PI_4;
                Vec3::new(r * (1.0 - t.cos()), 0.0, r * t.sin())
            })
            .collect();
        let rise = r * FRAC_PI_4.sin();
        let ctl = SweepControl { orientation: SweepOrientation::Parallel, ..SweepControl::default() };
        let b = sweep_path_with(&Plane::XY, &square(3.0), &path, &ctl).unwrap();
        let mesh = b.tessellate(0.005).unwrap();
        let v = mesh.measure().volume;
        let want = 36.0 * rise;
        assert!((v - want).abs() < 5e-3 * want, "volume {v} vs {want}");
        // The end cap is still horizontal: the body tops out flat at the path's end.
        let top: Vec<&Vec3> = mesh.positions.iter().filter(|p| (p.z - rise).abs() < 1e-6).collect();
        assert!(top.len() >= 4, "flat top");
        assert!(mesh.positions.iter().all(|p| p.z <= rise + 1e-6));
        // A path that turns to run along the profile's plane (a quarter circle) is refused.
        let quarter: Vec<Vec3> = (0..=60)
            .map(|i| {
                let t = i as f64 / 60.0 * FRAC_PI_2;
                Vec3::new(r * (1.0 - t.cos()), 0.0, r * t.sin())
            })
            .collect();
        assert!(sweep_path_with(&Plane::XY, &square(3.0), &quarter, &ctl).is_err());
    }

    /// A direction keeps the square's sides lined up with it along a curved path; in a plane
    /// square to the direction, that is the rotation-minimising frame too.
    #[test]
    fn direction_orientation_follows_the_vector() {
        let r = 30.0;
        let path: Vec<Vec3> = (0..=60)
            .map(|i| {
                let t = i as f64 / 60.0 * FRAC_PI_2;
                Vec3::new(r * (1.0 - t.cos()), 0.0, r * t.sin())
            })
            .collect();
        let ctl = SweepControl { orientation: SweepOrientation::Direction(Vec3::Y), ..SweepControl::default() };
        let b = sweep_path_with(&Plane::XY, &square(3.0), &path, &ctl).unwrap();
        let mesh = b.tessellate(0.005).unwrap();
        let len = r * FRAC_PI_2;
        assert!((mesh.measure().volume - 36.0 * len).abs() < 5e-3 * 36.0 * len);
        // The sides facing ±Y stay at y = ±3.
        assert!(mesh.positions.iter().all(|p| p.y.abs() <= 3.0 + 1e-6));
        // The path running along the direction is refused.
        let along = SweepControl { orientation: SweepOrientation::Direction(Vec3::Z), ..SweepControl::default() };
        assert!(crate::sweep_with(&Plane::XY, &square(3.0), &up(10.0), &along).is_err());
    }

    /// Out-of-range options are refused, never a panic; the default options are a plain sweep.
    #[test]
    fn sweep_options_refuse_hostile_values() {
        let bad = [
            SweepControl { twist: f64::NAN, ..SweepControl::default() },
            SweepControl { twist: 1e9, ..SweepControl::default() },
            SweepControl { twist: SweepControl::MAX_TWIST * 1.01, ..SweepControl::default() },
            SweepControl { scale: 0.0, ..SweepControl::default() },
            SweepControl { scale: -1.0, ..SweepControl::default() },
            SweepControl { scale: f64::INFINITY, ..SweepControl::default() },
            SweepControl { scale: 1e6, ..SweepControl::default() },
            SweepControl { orientation: SweepOrientation::Direction(Vec3::ZERO), ..SweepControl::default() },
            SweepControl { orientation: SweepOrientation::Direction(Vec3::new(f64::NAN, 0.0, 1.0)), ..SweepControl::default() },
        ];
        for ctl in bad {
            assert!(crate::sweep_with(&Plane::XY, &circle(2.0), &up(10.0), &ctl).is_err(), "{ctl:?}");
            assert!(crate::sweep_sheet_with(&Plane::XY, std::slice::from_ref(&circle(2.0).outer.segs), &up(10.0), &ctl).is_err(), "{ctl:?}");
            assert!(sweep_path_with(&Plane::XY, &circle(2.0), &[Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0)], &ctl).is_err(), "{ctl:?}");
        }
        // The largest twist still builds (a holed profile: ruled walls through dense stations).
        let holed = Region2 { outer: square(3.0).outer, holes: vec![Loop2::circle(Vec2::ZERO, 1.0).reversed()] };
        let max = SweepControl { twist: SweepControl::MAX_TWIST, scale: 0.5, ..SweepControl::default() };
        assert!(crate::sweep_with(&Plane::XY, &holed, &up(10.0), &max).is_ok());
        assert!(SweepControl::default().is_default());
    }
}
