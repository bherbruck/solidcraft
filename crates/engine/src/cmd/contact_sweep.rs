//! Bounded, chronological checks of a joint trajectory. This samples tessellated geometry;
//! it is deliberately not advertised as exact continuous collision detection.
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use solidcraft_doc::joints;
use solidcraft_doc::{Document, IDENTITY, Mat, apply_point, mat_mul};
use solidcraft_geom::{Aabb3, Bvh, Mesh, Vec3};

use crate::params::bad;
use crate::{Result, Session};

const MAX_SAMPLES: usize = 4096;
const MAX_PLACEMENTS: usize = 4096;
const MAX_BODIES: usize = 256;
const MAX_TRIANGLES: usize = 50_000;
const MAX_WORK: usize = 50_000_000;

fn limit(message: &str) -> crate::EngineError {
    bad("contact.sweep", message)
}

struct CachedBody {
    path: Vec<u64>,
    offset: Mat,
    mesh: Arc<Mesh>,
    size: f64,
    /// How far its mesh may stray from its faces: 0 when every face is flat (the mesh is the
    /// shape), else the chord tolerance it was tessellated to.
    chord: f64,
}

/// A member of a contact set: (occurrence id, name).
pub(super) type Member = (u64, String);

pub(super) struct ContactGeometry {
    bodies: Vec<CachedBody>,
    /// Body indices, and the set members (occurrence id, name) they are under.
    pairs: Vec<(usize, usize, Member, Member)>,
    step: f64,
}

impl ContactGeometry {
    pub(super) fn new(s: &Session) -> Result<Self> {
        let assembly = &s.doc.assembly;
        let sets: Vec<Vec<u64>> = if assembly.contact_all {
            vec![s.doc.occurrences.iter().map(|o| o.id).collect()]
        } else {
            assembly.contact_sets.iter().filter(|set| !set.suppressed).map(|set| set.occurrences.clone()).collect()
        };
        Self::of_sets(s, &sets)
    }

    /// The geometry of the bodies under `sets` (occurrence ids), paired as contact sets pair
    /// them: bodies under different members of one set.
    pub(super) fn of_sets(s: &Session, sets: &[Vec<u64>]) -> Result<Self> {
        let mut out = Self { bodies: Vec::new(), pairs: Vec::new(), step: f64::INFINITY };
        if sets.is_empty() {
            return Ok(out);
        }
        let state = s.model.state();
        let mut meshes: HashMap<String, (Arc<Mesh>, f64)> = HashMap::new();
        let mut stack = vec![(0, Vec::<u64>::new())];
        let mut count = 0;
        while let Some((component, path)) = stack.pop() {
            count += 1;
            if count > MAX_PLACEMENTS || path.len() > 64 {
                return Err(limit("assembly exceeds contact sweep placement budget"));
            }
            for occurrence in s.doc.occurrences.iter().filter(|o| o.parent == component) {
                let mut child = path.clone();
                child.push(occurrence.id);
                stack.push((occurrence.component, child));
                if stack.len() > MAX_PLACEMENTS {
                    return Err(limit("assembly exceeds contact sweep placement budget"));
                }
            }
            if !sets.iter().any(|set| path.iter().any(|id| set.contains(id))) {
                continue;
            }
            for body in state.bodies.iter().filter(|b| s.doc.model_body_component(b) == component) {
                if body.body.is_mesh() {
                    return Err(limit("contact sweep does not support mesh bodies; disable their contact set to move them"));
                }
                if out.bodies.len() >= MAX_BODIES {
                    return Err(limit("too many bodies for a contact sweep"));
                }
                let (mesh, chord) = if let Some(cached) = meshes.get(&body.name) {
                    cached.clone()
                } else {
                    let tolerance = (body.body.size() * 1e-3).max(1e-3);
                    let mesh = body.body.tessellate(tolerance)?;
                    if mesh.triangles.len() > MAX_TRIANGLES || mesh.positions.len().saturating_add(mesh.triangles.len().saturating_mul(4)) > 200_000 {
                        return Err(limit("body exceeds contact sweep tessellation budget"));
                    }
                    let extent = mesh.bounds().size();
                    // Whole-body widths along bounded distinct facet normals catch tilted
                    // plates without interpreting skinny mesh triangles as thin material.
                    // Local thin walls of a wider body remain a sampling limitation.
                    let mut thickness = extent.x.min(extent.y).min(extent.z);
                    let mut directions: Vec<Vec3> = Vec::new();
                    for triangle in &mesh.triangles {
                        let Some([a, b, c]) = mesh.tri(triangle) else {
                            return Err(limit("invalid contact tessellation"));
                        };
                        let Some(direction) = (b - a).cross(c - a).normalized() else { continue };
                        if directions.len() < 32 && !directions.iter().any(|d| d.dot(direction).abs() > 1.0 - 1e-6) {
                            directions.push(direction);
                        }
                    }
                    for direction in directions {
                        let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
                        for point in &mesh.positions {
                            let projected = point.dot(direction);
                            low = low.min(projected);
                            high = high.max(projected);
                        }
                        thickness = thickness.min(high - low);
                    }
                    if !thickness.is_finite() || thickness <= 1e-9 {
                        return Err(limit("body is too thin for a bounded contact sweep"));
                    }
                    out.step = out.step.min(thickness * 0.25);
                    let mesh = Arc::new(mesh);
                    // Flat faces tessellate exactly; only planes known from how the face was
                    // made count (an unknown face is taken as curved).
                    let faces = body.body.face_count();
                    let flat = faces <= 1024 && (0..faces).all(|i| body.body.face_plane(i).is_some());
                    let chord = if flat { 0.0 } else { tolerance };
                    meshes.insert(body.name.clone(), (mesh.clone(), chord));
                    (mesh, chord)
                };
                out.bodies.push(CachedBody {
                    path: path.clone(),
                    offset: s.doc.model_body_offset(body).copied().unwrap_or(IDENTITY),
                    mesh,
                    size: body.body.size(),
                    chord,
                });
            }
        }
        let mut pairs = BTreeSet::new();
        for set in sets {
            let tagged: Vec<(usize, u64)> =
                out.bodies.iter().enumerate().filter_map(|(i, b)| b.path.iter().rev().find(|id| set.contains(id)).map(|id| (i, *id))).collect();
            for (i, (a, oa)) in tagged.iter().enumerate() {
                for (b, ob) in tagged.iter().skip(i + 1) {
                    if oa != ob {
                        pairs.insert((*a, *b, *oa, *ob));
                    }
                }
            }
        }
        let name = |id| s.doc.occurrences.iter().find(|o| o.id == id).map(|o| o.name.clone()).unwrap_or_default();
        out.pairs = pairs.into_iter().map(|(a, b, oa, ob)| (a, b, (oa, name(oa)), (ob, name(ob)))).collect();
        Ok(out)
    }

    pub(super) fn has_pairs(&self) -> bool {
        !self.pairs.is_empty()
    }

    /// Keeps only the pairs of set members (occurrence ids) `keep` takes.
    pub(super) fn keep_pairs(&mut self, keep: impl Fn(u64, u64) -> bool) {
        self.pairs.retain(|(_, _, a, b)| keep(a.0, b.0));
    }

    /// How many mesh points are placed for each check.
    pub(super) fn vertex_count(&self) -> usize {
        self.bodies.iter().map(|b| b.mesh.positions.len()).fold(0, usize::saturating_add)
    }

    /// How many pairs of set members are checked against each other.
    pub(super) fn pair_count(&self) -> usize {
        self.pairs.iter().map(|(_, _, a, b)| (a.0.min(b.0), a.0.max(b.0))).collect::<BTreeSet<_>>().len()
    }

    fn frame(body: &CachedBody, s: &Session) -> Result<Mat> {
        Self::frame_in(body, &s.doc)
    }

    fn frame_in(body: &CachedBody, doc: &Document) -> Result<Mat> {
        let mut frame = IDENTITY;
        for id in &body.path {
            let occurrence = doc.occurrences.iter().find(|o| o.id == *id).ok_or_else(|| limit("contact occurrence disappeared"))?;
            frame = mat_mul(&frame, &occurrence.transform);
        }
        Ok(mat_mul(&frame, &body.offset))
    }

    fn bounds(&self, s: &Session) -> Result<Aabb3> {
        let mut bounds = Aabb3::EMPTY;
        for body in &self.bodies {
            let frame = Self::frame(body, s)?;
            for point in &body.mesh.positions {
                bounds.add(apply_point(&frame, *point));
            }
        }
        Ok(bounds)
    }

    pub(super) fn samples(&self, start: &Session, end: &Session) -> Result<usize> {
        let a = joints::joint_values(&start.doc.assembly);
        let b = joints::joint_values(&end.doc.assembly);
        let bounds = self.bounds(start)?.union(&self.bounds(end)?);
        let state = start.model.state();
        let (mut linear, mut angular, mut radius) = (0.0, 0.0, 0.0_f64);
        for joint in start.doc.assembly.joints.iter().filter(|j| !j.suppressed) {
            for (index, _) in joint.kind.dofs().iter().enumerate() {
                let delta = (a.get(&joint.id).and_then(|q| q.get(index)).copied().unwrap_or(0.0)
                    - b.get(&joint.id).and_then(|q| q.get(index)).copied().unwrap_or(0.0))
                .abs();
                if joint.kind.is_angle(index) {
                    angular += delta;
                    if delta == 0.0 {
                        continue;
                    }
                    for session in [start, end] {
                        for origin in [&joint.a, &joint.b] {
                            let frame = session.doc.origin_world(&state, origin)?;
                            let pivot = apply_point(&frame, Vec3::ZERO);
                            let far = Vec3::new(
                                (bounds.min.x - pivot.x).abs().max((bounds.max.x - pivot.x).abs()),
                                (bounds.min.y - pivot.y).abs().max((bounds.max.y - pivot.y).abs()),
                                (bounds.min.z - pivot.z).abs().max((bounds.max.z - pivot.z).abs()),
                            );
                            radius = radius.max(far.len());
                        }
                    }
                } else {
                    linear += delta;
                }
            }
        }
        let required = ((linear + angular * (radius + linear)) / self.step).max(angular / 1_f64.to_radians()).ceil().max(1.0);
        if !required.is_finite() || required > MAX_SAMPLES as f64 {
            return Err(limit("motion exceeds contact sweep sampling budget; drive the joint in smaller increments"));
        }
        let vertices: usize = self.bodies.iter().map(|body| body.mesh.positions.len()).sum();
        if (required as usize).saturating_mul(vertices.saturating_add(self.pairs.len())) > MAX_WORK {
            return Err(limit("motion exceeds contact sweep transform budget; simplify the contact set or use smaller increments"));
        }
        Ok(required as usize)
    }

    /// Each body's mesh placed where `s` has it.
    fn placed(&self, s: &Session) -> Result<Vec<Mesh>> {
        self.bodies
            .iter()
            .map(|body| {
                let frame = Self::frame(body, s)?;
                let positions = body.mesh.positions.iter().map(|p| apply_point(&frame, *p)).collect();
                Ok(Mesh { positions, triangles: body.mesh.triangles.clone(), ..Mesh::default() })
            })
            .collect()
    }

    pub(super) fn first_hit(&self, s: &Session, work: &mut usize) -> Result<Option<(String, String, f64)>> {
        Ok(self.hits(s, work, MAX_WORK, false)?.into_iter().next().map(|(a, b, depth)| (a.1, b.1, depth)))
    }

    /// The pairs of set members whose bodies pass through each other in `s`, with how deep the
    /// first two bodies found go (every pair, or the first found when `all` is false). `work`
    /// adds up the cost, refused past `budget`.
    pub(super) fn hits(&self, s: &Session, work: &mut usize, budget: usize, all: bool) -> Result<Vec<(Member, Member, f64)>> {
        let mut out: Vec<(Member, Member, f64)> = Vec::new();
        // The member pairs found so far (lower id first).
        let mut found: BTreeSet<(u64, u64)> = BTreeSet::new();
        let meshes = self.placed(s)?;
        // Each mesh's box once, not once per pair it is in.
        let boxes: Vec<Aabb3> = meshes.iter().map(Mesh::bounds).collect();
        for (a, b, oa, ob) in &self.pairs {
            // Members already found colliding need no more of their bodies tested.
            if found.contains(&(oa.0.min(ob.0), oa.0.max(ob.0))) {
                continue;
            }
            let (Some(ma), Some(mb), Some(ba), Some(bb)) = (meshes.get(*a), meshes.get(*b), self.bodies.get(*a), self.bodies.get(*b)) else {
                return Err(limit("invalid contact geometry pair"));
            };
            let (Some(aabb), Some(bbb)) = (boxes.get(*a), boxes.get(*b)) else {
                return Err(limit("invalid contact geometry pair"));
            };
            if aabb.max.x < bbb.min.x
                || bbb.max.x < aabb.min.x
                || aabb.max.y < bbb.min.y
                || bbb.max.y < aabb.min.y
                || aabb.max.z < bbb.min.z
                || bbb.max.z < aabb.min.z
            {
                continue;
            }
            let cost = ma
                .positions
                .len()
                .saturating_add(ma.triangles.len().saturating_mul(4))
                .saturating_mul(mb.triangles.len())
                .saturating_add(mb.positions.len().saturating_add(mb.triangles.len().saturating_mul(4)).saturating_mul(ma.triangles.len()));
            *work = work.saturating_add(cost);
            if *work > budget {
                return Err(limit("motion exceeds contact sweep collision budget; simplify the contact set or use smaller increments"));
            }
            if let Some(depth) = super::mesh_overlap(ma, mb, 1e-4 * ba.size.max(bb.size).max(1.0)) {
                found.insert((oa.0.min(ob.0), oa.0.max(ob.0)));
                out.push((oa.clone(), ob.clone(), depth));
                if !all {
                    break;
                }
            }
        }
        Ok(out)
    }
}

/// How near two set members come: the bodies under them nearest each other.
pub(super) struct Gap {
    pub(super) a: Member,
    pub(super) b: Member,
    /// 0 where they touch or pass into each other.
    pub(super) mm: f64,
    /// The nearest points (world), on a's body and on b's.
    pub(super) at: (Vec3, Vec3),
}

impl ContactGeometry {
    /// The gap between each pair of set members whose bodies come within `cutoff` of each
    /// other in `s` (all pairs with an infinite cutoff), nearest first. Mesh distances: exact
    /// for flat faces, within [`ContactGeometry::chord`] for curved ones. `work` adds up the
    /// cost, refused past `budget`.
    pub(super) fn gaps(&self, s: &Session, cutoff: f64, work: &mut usize, budget: usize) -> Result<Vec<Gap>> {
        let meshes = self.placed(s)?;
        let tris: Vec<Vec<[Vec3; 3]>> = meshes.iter().map(|m| m.triangles.iter().filter_map(|t| m.tri(t)).collect()).collect();
        let boxes: Vec<Aabb3> = meshes.iter().map(Mesh::bounds).collect();
        let mut trees: Vec<Option<Bvh>> = vec![None; meshes.len()];
        let mut nearest: BTreeMap<(u64, u64), Gap> = BTreeMap::new();
        for (a, b, oa, ob) in &self.pairs {
            let key = (oa.0.min(ob.0), oa.0.max(ob.0));
            let limit = nearest.get(&key).map_or(cutoff, |g| g.mm.min(cutoff));
            let (Some(ta), Some(tb), Some(ba), Some(bb)) = (tris.get(*a), tris.get(*b), boxes.get(*a), boxes.get(*b)) else {
                return Err(limit_error("invalid contact geometry pair"));
            };
            if box_distance(ba, bb) > limit {
                continue;
            }
            if trees.get(*b).is_some_and(Option::is_none)
                && let Some(slot) = trees.get_mut(*b)
            {
                *work = work.saturating_add(tb.len());
                *slot = Some(Bvh::new(&tb.iter().map(tri_box).collect::<Vec<_>>()));
            }
            let Some(Some(tree)) = trees.get(*b) else { return Err(limit_error("invalid contact geometry pair")) };
            let Some((mm, pa, pb)) = mesh_gap(ta, tb, tree, ba, bb, limit, work, budget)? else { continue };
            nearest.insert(key, Gap { a: oa.clone(), b: ob.clone(), mm, at: (pa, pb) });
        }
        let mut out: Vec<Gap> = nearest.into_values().collect();
        out.sort_by(|x, y| x.mm.total_cmp(&y.mm));
        Ok(out)
    }

    /// The most the gaps can be off: the largest chord tolerance of a curved body (0 when every
    /// body is flat-faced, and the mesh distances are exact).
    pub(super) fn chord(&self) -> f64 {
        self.bodies.iter().map(|b| b.chord).fold(0.0, f64::max)
    }

    /// The size of the largest body (at least 1 mm): what tolerances scale with.
    pub(super) fn scale(&self) -> f64 {
        self.bodies.iter().map(|b| b.size).fold(1.0, f64::max)
    }

    /// The furthest any point of the bodies moves between `from` and `to` (bounded by the
    /// corners of each body's box, which a rigid move takes furthest).
    pub(super) fn travel(&self, from: &Document, to: &Document) -> Result<f64> {
        let mut most = 0.0_f64;
        for body in &self.bodies {
            let (fa, fb) = (Self::frame_in(body, from)?, Self::frame_in(body, to)?);
            let local = body.mesh.bounds();
            for c in 0..8 {
                let pick = |bit: usize, lo: f64, hi: f64| if c & bit == 0 { lo } else { hi };
                let corner = Vec3::new(pick(1, local.min.x, local.max.x), pick(2, local.min.y, local.max.y), pick(4, local.min.z, local.max.z));
                most = most.max(apply_point(&fa, corner).dist(apply_point(&fb, corner)));
            }
        }
        Ok(most)
    }
}

fn limit_error(message: &str) -> crate::EngineError {
    limit(message)
}

fn tri_box(t: &[Vec3; 3]) -> Aabb3 {
    let mut b = Aabb3::EMPTY;
    for p in t {
        b.add(*p);
    }
    b
}

/// The distance between two boxes (0 where they overlap).
fn box_distance(a: &Aabb3, b: &Aabb3) -> f64 {
    let axis = |a0: f64, a1: f64, b0: f64, b1: f64| (b0 - a1).max(a0 - b1).max(0.0);
    let (x, y, z) = (axis(a.min.x, a.max.x, b.min.x, b.max.x), axis(a.min.y, a.max.y, b.min.y, b.max.y), axis(a.min.z, a.max.z, b.min.z, b.max.z));
    (x * x + y * y + z * z).sqrt()
}

/// The gap between meshes `a` and `b` (their triangles; `tree` over b's) and their nearest
/// points, when within `limit` (None: farther). 0 where their surfaces cross, and where one
/// lies wholly inside the other.
#[allow(clippy::too_many_arguments)]
fn mesh_gap(
    a: &[[Vec3; 3]],
    b: &[[Vec3; 3]],
    tree: &Bvh,
    ba: &Aabb3,
    bb: &Aabb3,
    limit: f64,
    work: &mut usize,
    budget: usize,
) -> Result<Option<(f64, Vec3, Vec3)>> {
    let mut best: Option<(f64, Vec3, Vec3)> = None;
    // An upper bound to prune with when there is no limit: a vertex of each near the other.
    let mut bound = limit;
    if !bound.is_finite() {
        let centre = bb.center();
        let near = |list: &[[Vec3; 3]], to: Vec3| list.iter().map(|t| t[0]).min_by(|p, q| p.dist(to).total_cmp(&q.dist(to)));
        if let Some(pa) = near(a, centre)
            && let Some(pb) = near(b, pa)
        {
            *work = work.saturating_add(a.len() + b.len());
            bound = pa.dist(pb);
            best = Some((bound, pa, pb));
        }
    }
    for ta in a {
        let box_a = tri_box(ta);
        if box_distance(&box_a, bb) > bound {
            continue;
        }
        // Both callbacks read the bound the other tightens.
        let (visits, shared) = (Cell::new(0usize), Cell::new(bound));
        tree.visit(
            |node| {
                visits.set(visits.get() + 1);
                box_distance(node, &box_a) <= shared.get()
            },
            |j| {
                let Some(tb) = b.get(j as usize) else { return };
                visits.set(visits.get() + 8);
                let (d, pa, pb) = tri_distance(ta, tb);
                if d <= shared.get() && best.is_none_or(|(x, _, _)| d < x) {
                    shared.set(d);
                    best = Some((d, pa, pb));
                }
            },
        );
        bound = shared.get();
        let visits = visits.get();
        *work = work.saturating_add(visits);
        if *work > budget {
            return Err(limit_error("gaps exceed the work budget; check fewer parts or leave out report_gaps"));
        }
        if bound <= 0.0 {
            return Ok(best);
        }
    }
    // Wholly inside, no surface crossing: a vertex of one inside the other.
    let overlap =
        ba.min.x <= bb.max.x && bb.min.x <= ba.max.x && ba.min.y <= bb.max.y && bb.min.y <= ba.max.y && ba.min.z <= bb.max.z && bb.min.z <= ba.max.z;
    if overlap {
        *work = work.saturating_add(a.len() + b.len());
        for (p, other) in [(a.first().map(|t| t[0]), b), (b.first().map(|t| t[0]), a)] {
            if let Some(p) = p
                && super::inside(p, other)
            {
                return Ok(Some((0.0, p, p)));
            }
        }
    }
    Ok(best.filter(|(d, _, _)| *d <= limit))
}

/// The distance between triangles `a` and `b`, with their nearest points (0 where they cross).
fn tri_distance(a: &[Vec3; 3], b: &[Vec3; 3]) -> (f64, Vec3, Vec3) {
    for (x, y) in [(a, b), (b, a)] {
        for i in 0..3 {
            if let (Some(p), Some(q)) = (x.get(i), x.get((i + 1) % 3))
                && let Some(hit) = segment_hits_triangle(*p, *q, y)
            {
                return (0.0, hit, hit);
            }
        }
    }
    let mut best = (f64::INFINITY, a[0], b[0]);
    let mut keep = |pa: Vec3, pb: Vec3| {
        let d = pa.dist(pb);
        if d < best.0 {
            best = (d, pa, pb);
        }
    };
    for p in a {
        keep(*p, super::closest(*p, b[0], b[1], b[2]));
    }
    for q in b {
        keep(super::closest(*q, a[0], a[1], a[2]), *q);
    }
    for i in 0..3 {
        for j in 0..3 {
            let (pa, pb) = segment_closest(a[i], a[(i + 1) % 3], b[j], b[(j + 1) % 3]);
            keep(pa, pb);
        }
    }
    best
}

/// Where segment pq passes through triangle t (None: it misses, or lies in its plane).
fn segment_hits_triangle(p: Vec3, q: Vec3, t: &[Vec3; 3]) -> Option<Vec3> {
    let d = q - p;
    let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
    let h = d.cross(e2);
    let det = e1.dot(h);
    // Parallel (or degenerate): the vertex and edge distances answer it.
    if det.abs() < 1e-12 * e1.len().max(1e-300) * e2.len().max(1e-300) * d.len().max(1e-300) {
        return None;
    }
    let f = 1.0 / det;
    let sv = p - t[0];
    let u = f * sv.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = sv.cross(e1);
    let v = f * d.dot(qv);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let along = f * e2.dot(qv);
    (0.0..=1.0).contains(&along).then(|| p + d * along)
}

/// The nearest points of segments p1q1 and p2q2.
fn segment_closest(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    let (d1, d2, r) = (q1 - p1, q2 - p2, p1 - p2);
    let (a, e, f) = (d1.dot(d1), d2.dot(d2), d2.dot(r));
    let eps = 1e-24;
    let (s, t) = if a <= eps && e <= eps {
        (0.0, 0.0)
    } else if a <= eps {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e <= eps {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s = if denom > eps * a * e { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    (p1 + d1 * s, p2 + d2 * t)
}

#[cfg(test)]
#[path = "contact_sweep_tests.rs"]
mod tests;
