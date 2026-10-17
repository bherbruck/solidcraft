//! Boundary surfaces (Surface › Boundary): one B-spline face through curves in one or two
//! directions (`solidcraft_geom::network`: a loft between curves, a Coons patch inside four, a
//! Gordon surface through a network), made from sketch curves, 3D sketch curves or a body's
//! edges. A boundary curve that is a chain of open edges of a surface may carry a continuity
//! condition with the face beside it: G1 (tangent) or G2 (curvature).
//!
//! The reusable pieces: [`CurveSource`] turns each kind of curve into a curve the network can
//! measure, [`EdgeFaces`] gives the frame and second fundamental form of the faces along a chain
//! of edges (what a G1 or G2 condition needs), and [`boundary_surface`] builds the face.
//!
//! The two-curve bridge with G0–G3 against a sketch's plane is [`crate::boundary`]; this is the
//! general case: three or four sides, interior curves, 3D curves and edges.

use std::collections::HashSet;

use mt::{BoundedCurve, Cut, InnerSpace, ParametricCurve, ParametricSurface, SearchNearestParameter};
use solidcraft_geom::bspline::{CurveN, NurbsSurface, SplineError};
use solidcraft_geom::network::{self, CrossFace, FaceFrame, NetCurve, NetworkCurve, PointCurve, Tolerances};
use solidcraft_geom::{Plane, Seg2, Vec3};
use truck_modeling as mt;

use crate::body::{Body, from_p3, p3};
use crate::bridge::Continuity;
use crate::plane_fit::EdgeSampler;
use crate::record::Rebuild;
use crate::{KernelError, Result, guard};

/// How far apart curves of the two directions may pass and still cross (mm).
pub const CROSS_TOLERANCE: f64 = 0.01;
/// How far the surface may stray from its curves (mm).
pub const FIT_TOLERANCE: f64 = 1e-4;
/// Most points of a 3D sketch curve a boundary surface follows (longer ones are resampled).
const MAX_WIRE_POINTS: usize = 400;
/// Most points picking the edges of one curve.
pub const MAX_EDGE_PICKS: usize = 10_000;
/// Most sketch curves one curve of a boundary surface chains.
pub const MAX_CHAIN_CURVES: usize = 1000;

/// The network's continuity for a curve's: G3 is refused (a network holds up to G2).
fn net_order(c: &BoundaryCurve) -> Result<network::Continuity> {
    match c.continuity {
        Continuity::G0 => Ok(network::Continuity::G0),
        Continuity::G1 => Ok(network::Continuity::G1),
        Continuity::G2 => Ok(network::Continuity::G2),
        Continuity::G3 => Err(KernelError::Invalid(format!("not supported yet: G3 continuity on {} (a curve network holds up to G2)", c.label))),
    }
}

/// Where a curve of a boundary surface comes from.
pub enum CurveSource<'a> {
    /// Sketch segments on a plane, each starting where the one before it ends.
    Sketch { plane: Plane, segs: Vec<Seg2> },
    /// A 3D sketch curve's points: followed smoothly through them (a cubic spline).
    Points(Vec<Vec3>),
    /// A chain of a body's edges, each picked by a point on it.
    Edges { body: &'a Body, at: Vec<Vec3> },
    /// A side collapsed to a point: the apex of a three-sided patch, where the sides beside it
    /// meet.
    Point(Vec3),
}

/// A curve of a boundary surface: a name for messages, where it comes from, and (for edges of a
/// surface) its continuity with the face beside it.
pub struct BoundaryCurve<'a> {
    pub label: String,
    pub source: CurveSource<'a>,
    pub continuity: Continuity,
}

/// One B-spline face through curves `u` (two or more; or one with a continuity condition, crossed
/// by `v`) and `v` (none: a loft through `u`; two each way and no more: a Coons patch; more: a
/// Gordon surface through the network). A surface body; its history has no operands (the face
/// and its edges are new). Refused, naming the curves: curves that miss each other (by more than
/// [`CROSS_TOLERANCE`]) or cross twice, curves out of order, closed curves, a continuity
/// condition on anything but an open edge of a surface, or on curves of both directions.
pub fn boundary_surface(u: &[BoundaryCurve], v: &[BoundaryCurve]) -> Result<Body> {
    for c in u.iter().chain(v) {
        if let CurveSource::Edges { body, .. } = &c.source {
            body.require_brep("boundary surface")?;
        }
    }
    guard("boundary surface", || {
        let mut corners: Vec<Vec3> = Vec::new();
        let mut to_net = |c: &BoundaryCurve| -> Result<NetworkCurve<'static>> {
            let (curve, face): (Box<dyn NetCurve>, Option<EdgeFaces>) = match &c.source {
                CurveSource::Sketch { plane, segs } => (Box::new(SketchChain::new(&c.label, plane, segs)?), None),
                CurveSource::Points(pts) => (Box::new(Polyline::new(&c.label, pts)?), None),
                CurveSource::Point(p) => {
                    if !p.is_finite() {
                        return Err(KernelError::Invalid(format!("{} is not a finite point", c.label)));
                    }
                    (Box::new(PointCurve(*p)), None)
                }
                CurveSource::Edges { body, at } => {
                    if at.is_empty() || at.len() > MAX_EDGE_PICKS || at.iter().any(|p| !p.is_finite()) {
                        return Err(KernelError::Invalid(format!("{}: pick 1 to {MAX_EDGE_PICKS} points on its edges", c.label)));
                    }
                    let (chain, closed) = crate::surfaces::picked_chain(body, at)?;
                    if closed {
                        return Err(KernelError::Invalid(format!("not supported yet: {} is a closed chain of edges", c.label)));
                    }
                    // The chain's inner vertices: the face's sides are split there, so that a
                    // join shares the body's edges one for one.
                    corners.extend(chain.iter().skip(1).map(|e| from_p3(e.front().point())));
                    let face = match c.continuity {
                        Continuity::G0 => None,
                        _ => Some(EdgeFaces::new(&c.label, body, &chain)?),
                    };
                    (Box::new(EdgeChain::new(&c.label, &chain)?), face)
                }
            };
            if c.continuity != Continuity::G0 && face.is_none() {
                return Err(KernelError::Invalid(format!("{}: G1 and G2 apply only to a body's edges", c.label)));
            }
            let order = net_order(c)?;
            Ok(NetworkCurve { label: c.label.clone(), curve, condition: face.map(|f| (order, Box::new(f) as Box<dyn CrossFace>)) })
        };
        let nu: Vec<NetworkCurve> = u.iter().map(&mut to_net).collect::<Result<_>>()?;
        let nv: Vec<NetworkCurve> = v.iter().map(&mut to_net).collect::<Result<_>>()?;
        let surface = network::network_surface(nu, nv, Tolerances { cross: CROSS_TOLERANCE, fit: FIT_TOLERANCE }).map_err(|e| match e {
            SplineError::Unsupported(m) => KernelError::Invalid(format!("not supported yet: {m}")),
            SplineError::TooLarge(m) => KernelError::TooLarge(m),
            SplineError::Invalid(m) => KernelError::Invalid(m),
        })?;
        let face = face_of(&surface, &corners)?;
        let mut rec = Rebuild::default();
        rec.made(&face, Vec::new());
        crate::surfaces::body_of(vec![face], &[], &rec)
    })
}

/// `made` (a boundary surface) sewn into `target` along the edges they share: one body, a solid
/// when it closes the gap. Refused when they share no edge.
pub fn boundary_join(target: &Body, made: &Body) -> Result<Body> {
    target.require_brep("join")?;
    let edges = |b: &Body| b.solid.edge_iter().map(|e| e.id()).collect::<HashSet<_>>().len();
    let joined = crate::surfaces::stitch(&[target, made], CROSS_TOLERANCE)?;
    if edges(&joined) == edges(target) + edges(made) {
        return Err(KernelError::Invalid("the surface shares no edge with the body it joins".into()));
    }
    Ok(joined)
}

/// Sketch segments on a plane, run end to end by arc length.
struct SketchChain {
    plane: Plane,
    segs: Vec<Seg2>,
    /// Cumulative length at the start of each segment, and the total.
    starts: Vec<f64>,
    total: f64,
}

impl SketchChain {
    fn new(label: &str, plane: &Plane, segs: &[Seg2]) -> Result<SketchChain> {
        if segs.is_empty() || segs.len() > MAX_CHAIN_CURVES {
            return Err(KernelError::Invalid(format!("{label} has no curves, or too many")));
        }
        if segs.windows(2).any(|w| w[0].end().dist(w[1].start()) > 1e-6) {
            return Err(KernelError::Invalid(format!("{label}: its curves don't meet end to end")));
        }
        let mut starts = Vec::with_capacity(segs.len());
        let mut total = 0.0;
        for s in segs {
            starts.push(total);
            total += s.length();
        }
        if !(total > 0.0 && total.is_finite()) {
            return Err(KernelError::Invalid(format!("{label} has no length")));
        }
        Ok(SketchChain { plane: *plane, segs: segs.to_vec(), starts, total })
    }
}

impl NetCurve for SketchChain {
    fn point(&mut self, t: f64) -> Option<Vec3> {
        let at = t.clamp(0.0, 1.0) * self.total;
        let k = self.starts.partition_point(|s| *s <= at).saturating_sub(1);
        let (seg, start) = (self.segs.get(k)?, self.starts.get(k)?);
        let len = seg.length();
        let local = if len > 0.0 { ((at - start) / len).clamp(0.0, 1.0) } else { 0.0 };
        let p = self.plane.to_world(seg.point_at(local));
        p.is_finite().then_some(p)
    }
}

/// A 3D polyline followed smoothly: a cubic through its points by chord length (fewer points, a
/// lower degree; a long one resampled first).
struct Polyline {
    curve: CurveN<3>,
    end: f64,
}

impl Polyline {
    fn new(label: &str, pts: &[Vec3]) -> Result<Polyline> {
        let mut clean: Vec<Vec3> = Vec::with_capacity(pts.len());
        for p in pts {
            if !p.is_finite() {
                return Err(KernelError::Invalid(format!("{label} has a point that is not a finite number")));
            }
            if clean.last().is_none_or(|q: &Vec3| q.dist(*p) > 1e-9) {
                clean.push(*p);
            }
        }
        let pts = resample(&clean, MAX_WIRE_POINTS);
        if pts.len() < 2 {
            return Err(KernelError::Invalid(format!("{label} has no length")));
        }
        let mut params = vec![0.0];
        for w in pts.windows(2) {
            params.push(params.last().copied().unwrap_or(0.0) + w[0].dist(w[1]));
        }
        let end = params.last().copied().unwrap_or(0.0);
        let data: Vec<[f64; 3]> = pts.iter().map(|p| [p.x, p.y, p.z]).collect();
        let curve = CurveN::interpolate(&params, &data, (pts.len() - 1).min(3))
            .map_err(|e| KernelError::Invalid(format!("{label} can't be followed smoothly: {e}")))?;
        Ok(Polyline { curve, end })
    }
}

impl NetCurve for Polyline {
    fn point(&mut self, t: f64) -> Option<Vec3> {
        let ([x, y, z], _) = self.curve.eval(t.clamp(0.0, 1.0) * self.end)?;
        Some(Vec3::new(x, y, z))
    }
}

/// At most `max` points evenly along a polyline by arc length, its ends kept.
fn resample(pts: &[Vec3], max: usize) -> Vec<Vec3> {
    if pts.len() <= max || max < 2 {
        return pts.to_vec();
    }
    let mut cum = vec![0.0];
    for w in pts.windows(2) {
        cum.push(cum.last().copied().unwrap_or(0.0) + w[0].dist(w[1]));
    }
    let total = cum.last().copied().unwrap_or(0.0);
    (0..max)
        .filter_map(|k| {
            let at = total * k as f64 / (max - 1) as f64;
            let i = cum.partition_point(|c| *c <= at).saturating_sub(1).min(pts.len().saturating_sub(2));
            let (c0, c1) = (cum.get(i)?, cum.get(i + 1)?);
            let w = if c1 > c0 { ((at - c0) / (c1 - c0)).clamp(0.0, 1.0) } else { 0.0 };
            Some(pts.get(i)?.lerp(*pts.get(i + 1)?, w))
        })
        .collect()
}

/// A chain of edges (each turned to run along it), by their lengths.
struct EdgeChain {
    edges: Vec<EdgeSampler>,
    starts: Vec<f64>,
    total: f64,
}

impl EdgeChain {
    fn new(label: &str, chain: &[mt::Edge]) -> Result<EdgeChain> {
        let mut edges = Vec::with_capacity(chain.len());
        let mut starts = Vec::with_capacity(chain.len());
        let mut total = 0.0;
        for e in chain {
            let mut s = EdgeSampler::new(e).ok_or_else(|| KernelError::Invalid(format!("{label}: an edge has no parameter range")))?;
            let mut len = 0.0;
            let mut last: Option<Vec3> = None;
            for k in 0..=16 {
                let p = s.at(k as f64 / 16.0).ok_or_else(|| KernelError::Invalid(format!("{label}: an edge can't be evaluated")))?;
                if let Some(q) = last {
                    len += p.dist(q);
                }
                last = Some(p);
            }
            starts.push(total);
            total += len;
            edges.push(s);
        }
        if !(total > 0.0 && total.is_finite()) {
            return Err(KernelError::Invalid(format!("{label} has no length")));
        }
        Ok(EdgeChain { edges, starts, total })
    }
}

impl NetCurve for EdgeChain {
    fn point(&mut self, t: f64) -> Option<Vec3> {
        let at = t.clamp(0.0, 1.0) * self.total;
        let k = self.starts.partition_point(|s| *s <= at).saturating_sub(1);
        let start = *self.starts.get(k)?;
        let end = self.starts.get(k + 1).copied().unwrap_or(self.total);
        let local = if end > start { ((at - start) / (end - start)).clamp(0.0, 1.0) } else { 0.0 };
        self.edges.get_mut(k)?.at(local)
    }
}

/// The faces along a chain of edges, each edge an open edge of one face of its body: their
/// frames and second fundamental forms where a boundary surface meets them (G1 and G2
/// conditions).
pub(crate) struct EdgeFaces {
    sides: Vec<EdgeSide>,
}

/// One edge of the chain: points along it (to find the edge nearest a point), the surface of
/// its face, whether the face's boundary runs along the chain (`along`), and the surface
/// parameters found last (the next search starts there).
struct EdgeSide {
    pts: Vec<Vec3>,
    surface: mt::Surface,
    along: bool,
    hint: Option<(f64, f64)>,
}

impl EdgeFaces {
    pub(crate) fn new(label: &str, body: &Body, chain: &[mt::Edge]) -> Result<EdgeFaces> {
        let mut sides = Vec::with_capacity(chain.len());
        for e in chain {
            let mut faces = body.solid.face_iter().filter(|f| f.boundaries().iter().flatten().any(|x| x.id() == e.id()));
            let (Some(face), None) = (faces.next(), faces.next()) else {
                return Err(KernelError::Invalid(format!(
                    "{label}: G1 and G2 need an open edge of a surface (an edge with a face on one side only)"
                )));
            };
            // How the face's own (absolute) boundary runs this edge.
            let in_wire = face.absolute_boundaries().iter().flatten().find(|x| x.id() == e.id()).cloned();
            let in_wire = in_wire.ok_or_else(|| KernelError::Failed("boundary surface: an edge's face".into()))?;
            let along = in_wire.front().id() == e.front().id();
            let mut sampler = EdgeSampler::new(e).ok_or_else(|| KernelError::Invalid(format!("{label}: an edge has no parameter range")))?;
            let pts = (0..=16).map(|k| sampler.at(k as f64 / 16.0)).collect::<Option<Vec<_>>>();
            let pts = pts.ok_or_else(|| KernelError::Invalid(format!("{label}: an edge can't be evaluated")))?;
            sides.push(EdgeSide { pts, surface: face.surface(), along, hint: None });
        }
        Ok(EdgeFaces { sides })
    }
}

impl CrossFace for EdgeFaces {
    fn frame(&mut self, at: Vec3, tangent: Vec3) -> Option<FaceFrame> {
        // The edge nearest the point, and the chain's direction along it there.
        let near = |s: &EdgeSide| s.pts.windows(2).map(|w| at.dist_to_segment(w[0], w[1])).fold(f64::INFINITY, f64::min);
        let k = (0..self.sides.len()).min_by(|a, b| near(&self.sides[*a]).total_cmp(&near(&self.sides[*b])))?;
        let side = self.sides.get_mut(k)?;
        let seg = side.pts.windows(2).min_by(|a, b| at.dist_to_segment(a[0], a[1]).total_cmp(&at.dist_to_segment(b[0], b[1])))?;
        let chain_dir = seg[1] - seg[0];
        let s = &side.surface;
        let (u, v) = s.search_nearest_parameter(p3(at), side.hint, 100).or_else(|| s.search_nearest_parameter(p3(at), None, 100))?;
        side.hint = Some((u, v));
        let (su, sv) = (s.uder(u, v), s.vder(u, v));
        let n = su.cross(sv);
        if !(n.magnitude2() > 0.0) {
            return None;
        }
        let n = n.normalize();
        let normal = Vec3::new(n.x, n.y, n.z);
        // The face's boundary runs counter-clockwise about its surface's normal, so the face lies
        // to the left of it: outward is the boundary's direction × the normal.
        let wire = if side.along { chain_dir } else { -chain_dir };
        let outward = wire.cross(normal).normalized()?;
        // The second fundamental form, in the basis of the tangent asked about and outward.
        let t = (tangent - normal * tangent.dot(normal)).normalized()?;
        let b = (outward - t * outward.dot(t)).normalized()?;
        let (e, f, g) = (su.dot(su), su.dot(sv), sv.dot(sv));
        let det = e * g - f * f;
        if !(det.abs() > 1e-24) {
            return None;
        }
        // II(w, w) = L·α² + 2M·αβ + N·β² for w = α·S_u + β·S_v, with L, M, N the surface's
        // second derivatives along its unit normal (do Carmo, §3-3): exact, rational surfaces
        // included, and with nothing sampled beyond the edge, where the face's domain ends.
        let nv = mt::Vector3::new(normal.x, normal.y, normal.z);
        let (l, m, nn) = (s.uuder(u, v).dot(nv), s.uvder(u, v).dot(nv), s.vvder(u, v).dot(nv));
        let ii = |w: Vec3| -> Option<f64> {
            let wv = mt::Vector3::new(w.x, w.y, w.z);
            let (p, q) = (wv.dot(su), wv.dot(sv));
            let (alpha, beta) = ((p * g - q * f) / det, (q * e - p * f) / det);
            let out = l * alpha * alpha + 2.0 * m * alpha * beta + nn * beta * beta;
            out.is_finite().then_some(out)
        };
        let (tt, bb) = (ii(t)?, ii(b)?);
        let tb = (ii(t + b)? - tt - bb) / 2.0;
        let form = [tt, tb, bb];
        form.iter().all(|x| x.is_finite()).then_some(FaceFrame { normal, outward: b, form })
    }
}

/// The face of a network surface: its four sides from the surface's boundary rows, each split
/// at the `corners` that lie on it (inner vertices of edge chains). Refused when a side shrinks to
/// a point.
fn face_of(s: &NurbsSurface, corners: &[Vec3]) -> Result<mt::Face> {
    let fail = |m: &str| KernelError::Failed(format!("boundary surface: {m}"));
    let [nu, nv] = s.count();
    let [ku, kv] = s.knots();
    let rows: Vec<Vec<mt::Point3>> = (0..nu).map(|i| (0..nv).map(|j| p3(s.point(i, j).xyz())).collect()).collect();
    let (uk, vk) = (mt::KnotVec::from(ku.clone()), mt::KnotVec::from(kv.clone()));
    let surface = mt::BSplineSurface::try_new((uk.clone(), vk.clone()), rows.clone()).map_err(|e| fail(&e.to_string()))?;
    let row = |pts: Vec<mt::Point3>, k: &mt::KnotVec| mt::BSplineCurve::try_new(k.clone(), pts).map_err(|e| fail(&e.to_string()));
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else { return Err(fail("control net")) };
    let bottom = row(rows.iter().filter_map(|r| r.first().copied()).collect(), &uk)?;
    let top = row(rows.iter().filter_map(|r| r.last().copied()).collect(), &uk)?;
    let left = row(first.clone(), &vk)?;
    let right = row(last.clone(), &vk)?;
    let corner = |c: &mt::BSplineCurve<mt::Point3>, end: bool| {
        let (t0, t1) = c.range_tuple();
        c.subs(if end { t1 } else { t0 })
    };
    let (c00, c10, c11, c01) = (corner(&bottom, false), corner(&bottom, true), corner(&top, true), corner(&top, false));
    let size = [c00, c10, c11, c01].iter().map(|p| from_p3(*p).dist(from_p3(c00))).fold(0.0, f64::max).max(1.0);
    // A side collapsed to a point (a three-sided patch) is left out of the loop: the two sides
    // beside it meet at one vertex there.
    let sides = [(c00, c10), (c10, c11), (c01, c11), (c00, c01)];
    let collapsed: Vec<bool> = sides.iter().map(|(a, b)| from_p3(*a).dist(from_p3(*b)) <= size * 1e-9).collect();
    match collapsed.iter().filter(|c| **c).count() {
        0 | 1 => {}
        _ => return Err(KernelError::Invalid("not supported yet: a boundary surface with more than one side shrunk to a point".into())),
    }
    let mut verts: Vec<mt::Vertex> = Vec::with_capacity(4);
    for c in [c00, c10, c11, c01] {
        let same = verts.iter().find(|v| from_p3(v.point()).dist(from_p3(c)) <= size * 1e-9).cloned();
        verts.push(same.unwrap_or_else(|| mt::Vertex::new(c)));
    }
    let (Some(v00), Some(v10), Some(v11), Some(v01)) = (verts.first(), verts.get(1), verts.get(2), verts.get(3)) else {
        return Err(fail("corners"));
    };
    let is = |k: usize| collapsed.get(k).copied().unwrap_or(false);
    let mut wire: Vec<mt::Edge> = Vec::new();
    if !is(0) {
        wire.extend(side_edges(bottom, v00, v10, corners));
    }
    if !is(1) {
        wire.extend(side_edges(right, v10, v11, corners));
    }
    if !is(2) {
        wire.extend(side_edges(top, v01, v11, corners).into_iter().rev().map(|e| e.inverse()));
    }
    if !is(3) {
        wire.extend(side_edges(left, v00, v01, corners).into_iter().rev().map(|e| e.inverse()));
    }
    // u along the bottom, v up the left: counter-clockwise in (u, v).
    mt::Face::try_new(vec![wire.into()], mt::Surface::BSplineSurface(surface)).map_err(|e| fail(&e.to_string()))
}

/// The edges of one side from `a` to `b`, split at the points of `corners` that lie on it
/// (within [`CROSS_TOLERANCE`]) strictly between its ends.
fn side_edges(mut curve: mt::BSplineCurve<mt::Point3>, a: &mt::Vertex, b: &mt::Vertex, corners: &[Vec3]) -> Vec<mt::Edge> {
    let (t0, t1) = curve.range_tuple();
    let margin = (t1 - t0) * 1e-6;
    let mut cuts: Vec<f64> = corners
        .iter()
        .filter_map(|c| {
            let t = curve.search_nearest_parameter(p3(*c), None, 100)?;
            (t > t0 + margin && t < t1 - margin && from_p3(curve.subs(t)).dist(*c) <= CROSS_TOLERANCE).then_some(t)
        })
        .collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|x, y| (*x - *y).abs() <= margin);
    let mut edges = Vec::with_capacity(cuts.len() + 1);
    let mut from = a.clone();
    // Cut from the far end back, so the earlier parameters stay on the part kept.
    let mut tails = Vec::with_capacity(cuts.len());
    for &t in cuts.iter().rev() {
        tails.push(curve.cut(t));
    }
    let mut pieces = vec![curve];
    pieces.extend(tails.into_iter().rev());
    let count = pieces.len();
    for (k, piece) in pieces.into_iter().enumerate() {
        let to = if k + 1 == count { b.clone() } else { mt::Vertex::new(piece.subs(piece.range_tuple().1)) };
        edges.push(mt::Edge::new(&from, &to, mt::Curve::BSplineCurve(piece)));
        from = to;
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_region;
    use mt::ParametricSurface3D;
    use solidcraft_geom::{Loop2, Region2, Vec2};

    fn sketch(label: &str, plane: Plane, segs: Vec<Seg2>) -> BoundaryCurve<'static> {
        BoundaryCurve { label: label.into(), source: CurveSource::Sketch { plane, segs }, continuity: Continuity::G0 }
    }

    fn line3(label: &str, a: Vec3, b: Vec3) -> BoundaryCurve<'static> {
        BoundaryCurve { label: label.into(), source: CurveSource::Points(vec![a, b]), continuity: Continuity::G0 }
    }

    fn face_surface(b: &Body) -> mt::Surface {
        b.solid.face_iter().next().unwrap().surface()
    }

    /// The flat sheet y ∈ [−10, 0], x ∈ [0, 10] in z = 0, its edge along y = 0 open.
    fn sheet() -> Body {
        let sq = Loop2::polygon(&[Vec2::new(0.0, -10.0), Vec2::new(10.0, -10.0), Vec2::new(10.0, 0.0), Vec2::new(0.0, 0.0)]);
        patch_region(&Plane::XY, &Region2 { outer: sq, holes: vec![] }).unwrap()
    }

    /// A Coons patch of four lines bounding the hyperbolic paraboloid z = x·y/10: every point of
    /// the face lies on it, and its area is the paraboloid's over the square.
    #[test]
    fn coons_patch_of_lines_is_a_hyperbolic_paraboloid() {
        let p = |x: f64, y: f64| Vec3::new(x, y, x * y / 10.0);
        let body = boundary_surface(
            &[line3("u0", p(0.0, 0.0), p(10.0, 0.0)), line3("u1", p(0.0, 10.0), p(10.0, 10.0))],
            &[line3("v0", p(0.0, 0.0), p(0.0, 10.0)), line3("v1", p(10.0, 0.0), p(10.0, 10.0))],
        )
        .unwrap();
        assert!(body.is_surface() && body.face_count() == 1);
        let s = face_surface(&body);
        for i in 0..=10 {
            for j in 0..=10 {
                let q = from_p3(s.subs(i as f64 / 10.0, j as f64 / 10.0));
                assert!((q.z - q.x * q.y / 10.0).abs() < 1e-9, "{q:?}");
            }
        }
    }

    /// Sketch curves: a patch inside two arcs and two lines on one plane lies in that plane.
    #[test]
    fn coplanar_sketch_curves_give_a_plane() {
        use std::f64::consts::PI;
        // Two half-discs' arcs over [0, 10] (radius 5) facing out, joined by the lines x = 0, 10.
        let low = Seg2::Arc { center: Vec2::new(5.0, 0.0), radius: 5.0, start: PI, sweep: PI };
        let high = Seg2::Arc { center: Vec2::new(5.0, 8.0), radius: 5.0, start: PI, sweep: -PI };
        let left = Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(0.0, 8.0) };
        let right = Seg2::Line { a: Vec2::new(10.0, 0.0), b: Vec2::new(10.0, 8.0) };
        let pl = Plane::XY.offset(3.0);
        let body = boundary_surface(
            &[sketch("low", pl, vec![low]), sketch("high", pl, vec![high])],
            &[sketch("left", pl, vec![left]), sketch("right", pl, vec![right])],
        )
        .unwrap();
        let s = face_surface(&body);
        for i in 0..=8 {
            for j in 0..=8 {
                assert!((s.subs(i as f64 / 8.0, j as f64 / 8.0).z - 3.0).abs() < 1e-9);
            }
        }
    }

    /// A stadium: two half-circle arcs (radius 5) bulging out of the 10 × 8 rectangle between two
    /// lines. Each arc meets its lines at 180°, so the patch is singular at its corners, and the
    /// reference mesher's walk round the boundary used to run from such a corner onto the
    /// surface's extrapolation past its domain (a planar spline passes through every point of its
    /// plane there): the face measured 289 mm² for its 158.5. The vendored mesher's bounded
    /// projection (`nearest_projection`, vendor/README.md) keeps the walk on the domain. Its area
    /// is the stadium's, 80 + 25π, by the faces, the measuring mesh and the display mesh.
    #[test]
    fn a_patch_with_straight_corners_measures_its_own_area() {
        use std::f64::consts::PI;
        let low = Seg2::Arc { center: Vec2::new(5.0, 0.0), radius: 5.0, start: PI, sweep: PI };
        let high = Seg2::Arc { center: Vec2::new(5.0, 8.0), radius: 5.0, start: PI, sweep: -PI };
        let left = Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(0.0, 8.0) };
        let right = Seg2::Line { a: Vec2::new(10.0, 0.0), b: Vec2::new(10.0, 8.0) };
        let pl = Plane::XY.offset(3.0);
        let body = boundary_surface(
            &[sketch("low", pl, vec![low]), sketch("high", pl, vec![high])],
            &[sketch("left", pl, vec![left]), sketch("right", pl, vec![right])],
        )
        .unwrap();
        let want = 80.0 + 25.0 * PI;
        // Chords of the arcs at 0.01 mm sag cut off up to (2/3)·sag·length ≈ 0.21 mm².
        let near = |a: f64| (a - want).abs() < 0.25;
        let faces = body.faces(0.01).unwrap();
        assert!(faces.len() == 1 && near(faces[0].area), "faces: {faces:?}");
        let tess = body.tessellate(0.01).unwrap().measure().area;
        assert!(near(tess), "measuring mesh: {tess}");
        let display = body.display_mesh(0.01).unwrap().measure().area;
        assert!(near(display), "display mesh: {display}");
    }

    /// Three sides: a quarter disc of radius 10 (two radii and the arc) is one face with three
    /// edges, the collapsed side left out of its loop, and measures 25π by its faces and both
    /// meshes.
    #[test]
    fn a_three_sided_patch_is_one_face_with_three_edges() {
        use std::f64::consts::{FRAC_PI_2, PI};
        let r = 10.0;
        let arc = Seg2::Arc { center: Vec2::new(0.0, 0.0), radius: r, start: 0.0, sweep: FRAC_PI_2 };
        let x = Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(r, 0.0) };
        let y = Seg2::Line { a: Vec2::new(0.0, r), b: Vec2::new(0.0, 0.0) };
        let pl = Plane::XY;
        let apex = BoundaryCurve { label: "centre".into(), source: CurveSource::Point(Vec3::ZERO), continuity: Continuity::G0 };
        let body = boundary_surface(&[sketch("arc", pl, vec![arc]), apex], &[sketch("x", pl, vec![x]), sketch("y", pl, vec![y])]).unwrap();
        assert!(body.is_surface() && body.face_count() == 1);
        let edges: std::collections::HashSet<_> = body.solid.edge_iter().map(|e| e.id()).collect();
        assert_eq!(edges.len(), 3, "the collapsed side is no edge");
        let want = PI * r * r / 4.0;
        let near = |a: f64| (a - want).abs() < 0.2;
        let faces = body.faces(0.01).unwrap();
        assert!(near(faces[0].area), "faces: {faces:?}");
        let tess = body.tessellate(0.01).unwrap().measure().area;
        assert!(near(tess), "measuring mesh: {tess}");
        let display = body.display_mesh(0.01).unwrap().measure().area;
        assert!(near(display), "display mesh: {display}");
        // Two collapsed sides are no patch.
        let p = |l: &str, q: Vec3| BoundaryCurve { label: l.into(), source: CurveSource::Point(q), continuity: Continuity::G0 };
        assert!(
            boundary_surface(&[sketch("arc", pl, vec![arc]), p("o", Vec3::ZERO)], &[p("a", Vec3::new(r, 0.0, 0.0)), sketch("y", pl, vec![y])])
                .is_err()
        );
        assert!(boundary_surface(&[sketch("arc", pl, vec![arc]), p("nan", Vec3::new(f64::NAN, 0.0, 0.0))], &[]).is_err());
    }

    /// The angle between the two faces' normals across the edge at `p`.
    fn normal_angle(a: &mt::Surface, b: &mt::Surface, p: Vec3) -> f64 {
        let n = |s: &mt::Surface| {
            let (u, v) = s.search_nearest_parameter(p3(p), None, 100).unwrap();
            let n = s.normal(u, v);
            Vec3::new(n.x, n.y, n.z)
        };
        n(a).dot(n(b)).abs().clamp(0.0, 1.0).acos().to_degrees()
    }

    /// G1 against a flat sheet's open edge: a loft from the edge to a line above, and a patch
    /// whose sides are arcs leaving the sheet tangent to it. The normals across the shared edge
    /// agree within 0.1°; the patch joins the sheet into one surface body.
    #[test]
    fn g1_meets_the_sheet_tangent() {
        let base = sheet();
        let edge =
            |g| BoundaryCurve { label: "edge".into(), source: CurveSource::Edges { body: &base, at: vec![Vec3::new(5.0, 0.0, 0.0)] }, continuity: g };
        let far = || line3("far", Vec3::new(0.0, 10.0, 5.0), Vec3::new(10.0, 10.0, 5.0));
        let loft = boundary_surface(&[edge(Continuity::G1), far()], &[]).unwrap();
        let plain = boundary_surface(&[edge(Continuity::G0), far()], &[]).unwrap();
        let (sb, sl, sp) = (face_surface(&base), face_surface(&loft), face_surface(&plain));
        for k in 0..=10 {
            let p = Vec3::new(k as f64, 0.0, 0.0);
            let a = normal_angle(&sb, &sl, p);
            assert!(a < 0.1, "G1 at x = {k}: {a}°");
        }
        assert!(normal_angle(&sb, &sp, Vec3::new(5.0, 0.0, 0.0)) > 10.0, "G0 has a crease");
        // Arcs in x = 0 and x = 10 from the edge, tangent to the sheet, up to the far line.
        let arc_plane = |x: f64| Plane::new(Vec3::new(x, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap();
        let arc = Seg2::Arc { center: Vec2::new(0.0, 12.5), radius: 12.5, start: -std::f64::consts::FRAC_PI_2, sweep: 0.927_295_218_001_612_2 };
        let patch = boundary_surface(
            &[edge(Continuity::G1), far()],
            &[sketch("left", arc_plane(0.0), vec![arc]), sketch("right", arc_plane(10.0), vec![arc])],
        )
        .unwrap();
        let spa = face_surface(&patch);
        for k in 0..=10 {
            let a = normal_angle(&sb, &spa, Vec3::new(k as f64, 0.0, 0.0));
            assert!(a < 0.1, "G1 patch at x = {k}: {a}°");
        }
        let joined = boundary_join(&base, &patch).unwrap();
        assert_eq!(joined.face_count(), 2);
        assert!(
            boundary_join(&base, &boundary_surface(&[far(), line3("other", Vec3::new(0.0, 20.0, 5.0), Vec3::new(10.0, 20.0, 5.0))], &[]).unwrap())
                .is_err()
        );
    }

    /// Normal curvature of `s` at the point nearest `p` in the direction `w` (projected into the
    /// tangent plane), signed against the unit normal `n`.
    fn normal_curvature(s: &mt::Surface, p: Vec3, w: Vec3, n: Vec3) -> f64 {
        let (u, v) = s.search_nearest_parameter(p3(p), None, 100).unwrap();
        let v3 = |x: mt::Vector3| Vec3::new(x.x, x.y, x.z);
        let (su, sv) = (v3(s.uder(u, v)), v3(s.vder(u, v)));
        let (suu, suv, svv) = (v3(s.uuder(u, v)), v3(s.uvder(u, v)), v3(s.vvder(u, v)));
        let (e, f, g) = (su.dot(su), su.dot(sv), sv.dot(sv));
        let (l, m, nn) = (suu.dot(n), suv.dot(n), svv.dot(n));
        let (p1, q1) = (w.dot(su), w.dot(sv));
        let det = e * g - f * f;
        let (a, b) = ((p1 * g - q1 * f) / det, (q1 * e - p1 * f) / det);
        (l * a * a + 2.0 * m * a * b + nn * b * b) / (e * a * a + 2.0 * f * a * b + g * b * b)
    }

    /// G2 against a real cylinder's open edge (R = 10, the face below the edge): across the edge
    /// the surface's normal curvature is the cylinder's, −1/R about its outward normal, within
    /// 0.1 % — also when the far curve is sheared along the edge, so that the surface's own
    /// cross direction leans. With G1 alone it is not.
    #[test]
    fn g2_matches_a_cylinder_across_its_edge() {
        let r = 10.0;
        let pl = Plane::new(Vec3::ZERO, Vec3::Y, Vec3::Z).unwrap();
        let arc = Seg2::Arc { center: Vec2::new(0.0, 0.0), radius: r, start: -std::f64::consts::FRAC_PI_2, sweep: std::f64::consts::FRAC_PI_2 };
        let cyl = crate::extrude_sheet(&pl, &[vec![arc]], 0.0, 10.0).unwrap();
        let sc = face_surface(&cyl);
        let made = |g, shear: f64| {
            let edge =
                BoundaryCurve { label: "rim".into(), source: CurveSource::Edges { body: &cyl, at: vec![Vec3::new(5.0, r, 0.0)] }, continuity: g };
            let top = line3("top", Vec3::new(shear, 3.0, 12.0), Vec3::new(10.0 + shear, 3.0, 12.0));
            face_surface(&boundary_surface(&[edge, top], &[]).unwrap())
        };
        for shear in [0.0, 5.0] {
            let sm = made(Continuity::G2, shear);
            for k in 1..=9 {
                let p = Vec3::new(k as f64, r, 0.0);
                let kc = normal_curvature(&sc, p, Vec3::Z, Vec3::Y);
                assert!((kc + 1.0 / r).abs() < 1e-9, "the cylinder's own curvature: {kc}");
                let km = normal_curvature(&sm, p, Vec3::Z, Vec3::Y);
                assert!((km - kc).abs() < 1e-3 / r, "shear {shear}, x = {k}: {km} across the edge where the cylinder has {kc}");
                // Along the edge both are straight.
                assert!(normal_curvature(&sm, p, Vec3::X, Vec3::Y).abs() < 1e-6);
                assert!(normal_angle(&sc, &sm, p) < 0.1);
            }
        }
        let g1 = made(Continuity::G1, 0.0);
        let k1 = normal_curvature(&g1, Vec3::new(5.0, r, 0.0), Vec3::Z, Vec3::Y);
        assert!((k1 + 1.0 / r).abs() > 0.1 / r, "G1 alone does not carry the curvature: {k1}");
    }

    /// G3 is more than a network holds: refused, never a panic.
    #[test]
    fn g3_is_not_supported() {
        let base = sheet();
        let edge = BoundaryCurve {
            label: "edge".into(),
            source: CurveSource::Edges { body: &base, at: vec![Vec3::new(5.0, 0.0, 0.0)] },
            continuity: Continuity::G3,
        };
        let e = boundary_surface(&[edge, line3("far", Vec3::new(0.0, 10.0, 5.0), Vec3::new(10.0, 10.0, 5.0))], &[]).unwrap_err();
        assert!(e.to_string().contains("not supported yet"), "{e}");
    }

    /// Refusals: G1 on a sketch curve, on an edge between two faces, curves that miss.
    #[test]
    fn bad_boundaries_are_refused() {
        let cube = crate::box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let edge = BoundaryCurve {
            label: "edge".into(),
            source: CurveSource::Edges { body: &cube, at: vec![Vec3::new(5.0, 0.0, 0.0)] },
            continuity: Continuity::G1,
        };
        let far = line3("far", Vec3::new(0.0, 10.0, 15.0), Vec3::new(10.0, 10.0, 15.0));
        let e = boundary_surface(&[edge, far], &[]).unwrap_err();
        assert!(e.to_string().contains("open edge"), "{e}");
        let mut tangent = line3("t", Vec3::ZERO, Vec3::X);
        tangent.continuity = Continuity::G1;
        assert!(boundary_surface(&[tangent, line3("b", Vec3::Y, Vec3::new(1.0, 1.0, 0.0))], &[]).is_err());
        let miss = boundary_surface(
            &[line3("a", Vec3::ZERO, Vec3::X * 10.0), line3("b", Vec3::Y * 10.0, Vec3::new(10.0, 10.0, 0.0))],
            &[line3("c", Vec3::ZERO, Vec3::Y * 10.0), line3("d", Vec3::new(10.0, 0.0, 2.0), Vec3::new(10.0, 10.0, 2.0))],
        )
        .unwrap_err();
        assert!(miss.to_string().contains("don't meet"), "{miss}");
        let pts = BoundaryCurve { label: "p".into(), source: CurveSource::Points(vec![Vec3::ZERO; 3]), continuity: Continuity::G0 };
        assert!(boundary_surface(&[pts, line3("b", Vec3::Y, Vec3::new(1.0, 1.0, 0.0))], &[]).is_err());
        let nan = BoundaryCurve {
            label: "n".into(),
            source: CurveSource::Points(vec![Vec3::ZERO, Vec3::new(f64::NAN, 0.0, 0.0)]),
            continuity: Continuity::G0,
        };
        assert!(boundary_surface(&[nan, line3("b", Vec3::Y, Vec3::new(1.0, 1.0, 0.0))], &[]).is_err());
    }
}
