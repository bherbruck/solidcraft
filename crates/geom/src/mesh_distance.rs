//! Distances between two triangle meshes: the nearest pair of points, and every pair of
//! triangles within a distance. A [`TriangleSet`] keeps a mesh's triangles under a bounding
//! volume hierarchy, so a query costs about the pairs of triangles that come near each other
//! rather than the product of the triangle counts.

use crate::compare::closest_on_triangle;
use crate::{Aabb3, Bvh, Mesh, Vec3};

/// A mesh's triangles with their B-rep faces, indexed for distance queries.
#[derive(Clone, Debug, Default)]
pub struct TriangleSet {
    tris: Vec<[Vec3; 3]>,
    face: Vec<u32>,
    bvh: Bvh,
    bounds: Aabb3,
}

/// The nearest points of two triangle sets, and the triangles they lie on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshClosest {
    pub distance: f64,
    pub a: Vec3,
    pub b: Vec3,
    pub tri_a: u32,
    pub tri_b: u32,
}

impl TriangleSet {
    /// The set of `mesh`'s triangles (ones with a vertex that is not finite are left out).
    pub fn new(mesh: &Mesh) -> TriangleSet {
        let mut tris = Vec::with_capacity(mesh.triangles.len());
        let mut face = Vec::with_capacity(mesh.triangles.len());
        let mut boxes = Vec::with_capacity(mesh.triangles.len());
        let mut bounds = Aabb3::EMPTY;
        for (k, t) in mesh.triangles.iter().enumerate() {
            let Some(p) = mesh.tri(t) else { continue };
            if !p.iter().all(|v| v.is_finite()) {
                continue;
            }
            let mut b = Aabb3::EMPTY;
            for v in p {
                b.add(v);
            }
            bounds = bounds.union(&b);
            boxes.push(b);
            tris.push(p);
            face.push(mesh.tri_face.get(k).copied().unwrap_or(0));
        }
        TriangleSet { bvh: Bvh::new(&boxes), tris, face, bounds }
    }

    /// The box around every triangle.
    pub fn bounds(&self) -> Aabb3 {
        self.bounds
    }

    pub fn len(&self) -> usize {
        self.tris.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Triangle `t`'s corners.
    pub fn triangle(&self, t: u32) -> Option<[Vec3; 3]> {
        self.tris.get(t as usize).copied()
    }

    /// The B-rep face triangle `t` came from.
    pub fn face(&self, t: u32) -> Option<u32> {
        self.face.get(t as usize).copied()
    }

    /// The nearest points of this set and `other`, if they come within `limit` of each other
    /// (`f64::INFINITY`: always). Triangles that cross count as distance 0, at a point where
    /// they meet.
    pub fn closest(&self, other: &TriangleSet, limit: f64) -> Option<MeshClosest> {
        let mut best: Option<MeshClosest> = None;
        let bound = std::cell::Cell::new(if limit.is_nan() { f64::INFINITY } else { limit });
        let mut found = |i: u32, j: u32| {
            let (Some(x), Some(y)) = (self.tris.get(i as usize), other.tris.get(j as usize)) else { return };
            let (d, a, b) = triangle_distance(x, y);
            if d <= bound.get() && best.is_none_or(|m| d < m.distance) {
                best = Some(MeshClosest { distance: d, a, b, tri_a: i, tri_b: j });
                bound.set(d);
            }
        };
        self.bvh.visit_pairs(&other.bvh, |x, y| box_gap(x, y) <= bound.get(), &mut found);
        best
    }

    /// Calls `f` with every pair of triangles, one from this set and one from `other`, that come
    /// within `d` of each other, with their nearest points.
    pub fn within(&self, other: &TriangleSet, d: f64, mut f: impl FnMut(MeshClosest)) {
        if !(d >= 0.0) {
            return;
        }
        self.bvh.visit_pairs(
            &other.bvh,
            |x, y| box_gap(x, y) <= d,
            |i, j| {
                let (Some(x), Some(y)) = (self.tris.get(i as usize), other.tris.get(j as usize)) else { return };
                let (dist, a, b) = triangle_distance(x, y);
                if dist <= d {
                    f(MeshClosest { distance: dist, a, b, tri_a: i, tri_b: j });
                }
            },
        );
    }

    /// The distance from `p` to the nearest of the triangles `tris` (indices into this set).
    pub fn distance_to(&self, p: Vec3, tris: &[u32]) -> f64 {
        tris.iter()
            .filter_map(|&t| self.tris.get(t as usize))
            .map(|[a, b, c]| closest_on_triangle(p, *a, *b, *c).dist(p))
            .fold(f64::INFINITY, f64::min)
    }
}

/// The gap between two boxes (0 when they meet).
pub fn box_gap(a: &Aabb3, b: &Aabb3) -> f64 {
    if a.is_empty() || b.is_empty() {
        return f64::INFINITY;
    }
    let axis = |lo1: f64, hi1: f64, lo2: f64, hi2: f64| (lo2 - hi1).max(lo1 - hi2).max(0.0);
    let d = Vec3::new(axis(a.min.x, a.max.x, b.min.x, b.max.x), axis(a.min.y, a.max.y, b.min.y, b.max.y), axis(a.min.z, a.max.z, b.min.z, b.max.z));
    d.len()
}

/// The nearest points of segments `p0`–`p1` and `q0`–`q1` (Ericson, Real-Time Collision
/// Detection, 5.1.9).
pub fn closest_on_segments(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> (Vec3, Vec3) {
    let (d1, d2, r) = (p1 - p0, q1 - q0, p0 - q0);
    let (a, e, f) = (d1.len2(), d2.len2(), d2.dot(r));
    const TINY: f64 = 1e-300;
    let (s, t) = if a <= TINY && e <= TINY {
        (0.0, 0.0)
    } else if a <= TINY {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e <= TINY {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = d1.dot(d2);
            let den = a * e - b * b;
            let mut s = if den > TINY * (a * e).max(1.0) { ((b * f - c * e) / den).clamp(0.0, 1.0) } else { 0.0 };
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
    (p0 + d1 * s, q0 + d2 * t)
}

/// Where segment `p`–`q` passes through triangle `t` (not when it lies in its plane).
fn segment_through(p: Vec3, q: Vec3, t: &[Vec3; 3]) -> Option<Vec3> {
    let [a, b, c] = *t;
    let n = (b - a).cross(c - a);
    let (dp, dq) = (n.dot(p - a), n.dot(q - a));
    if (dp > 0.0 && dq > 0.0) || (dp < 0.0 && dq < 0.0) || dp == dq {
        return None;
    }
    let x = p + (q - p) * (dp / (dp - dq));
    let inside = [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).cross(x - *u).dot(n) >= 0.0);
    inside.then_some(x)
}

/// The distance between two triangles and their nearest points (0 where they cross).
pub fn triangle_distance(s: &[Vec3; 3], t: &[Vec3; 3]) -> (f64, Vec3, Vec3) {
    let edges = |x: &[Vec3; 3]| [(x[0], x[1]), (x[1], x[2]), (x[2], x[0])];
    for (p, q) in edges(s) {
        if let Some(x) = segment_through(p, q, t) {
            return (0.0, x, x);
        }
    }
    for (p, q) in edges(t) {
        if let Some(x) = segment_through(p, q, s) {
            return (0.0, x, x);
        }
    }
    let mut best = (f64::INFINITY, s[0], t[0]);
    let mut keep = |a: Vec3, b: Vec3| {
        let d = a.dist(b);
        if d < best.0 {
            best = (d, a, b);
        }
    };
    for p in s {
        keep(*p, closest_on_triangle(*p, t[0], t[1], t[2]));
    }
    for q in t {
        keep(closest_on_triangle(*q, s[0], s[1], s[2]), *q);
    }
    for (p0, p1) in edges(s) {
        for (q0, q1) in edges(t) {
            let (a, b) = closest_on_segments(p0, p1, q0, q1);
            keep(a, b);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [Vec3; 3] {
        [Vec3::from(a), Vec3::from(b), Vec3::from(c)]
    }

    /// Distances between triangles worked out by hand: face to vertex, edge to edge, crossing.
    #[test]
    fn triangle_distances_by_hand() {
        let floor = tri([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]);
        // A vertex 3 above the floor's inside.
        let (d, a, b) = triangle_distance(&floor, &tri([1.0, 1.0, 3.0], [1.0, 1.0, 9.0], [2.0, 1.0, 9.0]));
        assert!((d - 3.0).abs() < 1e-12 && a.dist(Vec3::new(1.0, 1.0, 0.0)) < 1e-12 && b.dist(Vec3::new(1.0, 1.0, 3.0)) < 1e-12);
        // Two skew edges: the floor's x axis edge and a line along y at x = 5, z = 2, off the floor at y < 0.
        let (d, ..) = triangle_distance(&floor, &tri([5.0, -1.0, 2.0], [5.0, -9.0, 2.0], [5.0, -5.0, 9.0]));
        assert!((d - 5f64.sqrt()).abs() < 1e-12, "{d}");
        // A triangle through the floor.
        let (d, ..) = triangle_distance(&floor, &tri([2.0, 2.0, -1.0], [2.0, 2.0, 1.0], [3.0, 2.0, 1.0]));
        assert_eq!(d, 0.0);
        // Parallel segments overlapping along their length.
        let (p, q) = closest_on_segments(Vec3::ZERO, Vec3::X * 4.0, Vec3::new(2.0, 1.0, 0.0), Vec3::new(6.0, 1.0, 0.0));
        assert!((p.dist(q) - 1.0).abs() < 1e-12);
    }

    fn cube(at: Vec3, size: f64) -> Mesh {
        let mut m = Mesh::default();
        let c = |x: f64, y: f64, z: f64| at + Vec3::new(x, y, z) * size;
        m.positions = vec![c(0., 0., 0.), c(1., 0., 0.), c(1., 1., 0.), c(0., 1., 0.), c(0., 0., 1.), c(1., 0., 1.), c(1., 1., 1.), c(0., 1., 1.)];
        m.triangles =
            vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7]];
        m.tri_face = vec![0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5];
        m
    }

    /// Two cubes apart along a diagonal: the nearest points are their facing corners.
    #[test]
    fn nearest_points_of_two_meshes() {
        let a = TriangleSet::new(&cube(Vec3::ZERO, 10.0));
        let b = TriangleSet::new(&cube(Vec3::new(13.0, 14.0, 0.0), 10.0));
        let c = a.closest(&b, f64::INFINITY);
        assert!(c.is_some_and(|c| (c.distance - 5.0).abs() < 1e-12), "{c:?}");
        assert!(a.closest(&b, 4.0).is_none(), "nothing within 4");
        let mut n = 0;
        a.within(&b, 5.5, |_| n += 1);
        assert!(n > 0);
        let far = TriangleSet::new(&cube(Vec3::new(5.0, 5.0, 5.0), 10.0));
        assert_eq!(a.closest(&far, f64::INFINITY).map(|c| c.distance), Some(0.0), "overlapping cubes cross");
    }
}
