use serde::{Deserialize, Serialize};

use crate::Vec3;

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Aabb3 {
    fn default() -> Self {
        Aabb3::EMPTY
    }
}

impl Aabb3 {
    pub const EMPTY: Aabb3 = Aabb3 {
        min: Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
        max: Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
    };
    pub fn is_empty(&self) -> bool {
        !(self.min.x <= self.max.x && self.min.y <= self.max.y && self.min.z <= self.max.z)
    }
    pub fn add(&mut self, p: Vec3) {
        if p.is_finite() {
            self.min = self.min.min(p);
            self.max = self.max.max(p);
        }
    }
    pub fn union(&self, o: &Aabb3) -> Aabb3 {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Aabb3 { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    pub fn center(&self) -> Vec3 {
        if self.is_empty() { Vec3::ZERO } else { (self.min + self.max) * 0.5 }
    }
    pub fn size(&self) -> Vec3 {
        if self.is_empty() { Vec3::ZERO } else { self.max - self.min }
    }
    pub fn diagonal(&self) -> f64 {
        self.size().len()
    }
}

/// An indexed triangle mesh with per-vertex normals, a face id per triangle (the B-rep face it
/// came from) and edge polylines (the B-rep edges, for display and picking).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// Triangles as vertex index triples (counter-clockwise seen from outside).
    pub triangles: Vec<[u32; 3]>,
    /// B-rep face index per triangle.
    pub tri_face: Vec<u32>,
    /// B-rep edges as polylines.
    pub edges: Vec<Vec<Vec3>>,
    /// Per edge: a seam between two pieces of the same smooth surface (not drawn).
    #[serde(default)]
    pub seams: Vec<bool>,
    /// Display-only tangent joins. Edge identities, picking and face groups stay intact.
    #[serde(skip)]
    pub smooth_edges: Vec<bool>,
    /// Display-protected boundaries: certified current creases or authored splits.
    /// Overrides coarse seam fits; not a geometric/topological authority.
    #[serde(skip)]
    pub sharp_edges: Vec<bool>,
    /// Per edge: the B-rep faces it bounds (one or two).
    #[serde(default)]
    pub edge_faces: Vec<Vec<u32>>,
}

fn boxes_meet(a: &Aabb3, b: &Aabb3) -> bool {
    a.min.x <= b.max.x && b.min.x <= a.max.x && a.min.y <= b.max.y && b.min.y <= a.max.y && a.min.z <= b.max.z && b.min.z <= a.max.z
}

/// A triangle with its bounding box, as [`find_close_pair`] takes them.
pub type BoxedTriangle = ([Vec3; 3], Aabb3);

/// Most grid cells one triangle is filed under; a larger one is tested against every triangle.
const MAX_CELLS_PER_TRIANGLE: usize = 64;

/// Calls `test` on pairs of triangles, one from `a` and one from `b`, whose boxes come within
/// `gap` of each other (each such pair once), until it returns true, and says whether it did.
/// The cost follows the number of close pairs, not `a.len() * b.len()`: `b` is filed in a sparse
/// grid of cells about its typical triangle's size, and each triangle of `a` meets only those in
/// its cells. The work counts against `budget`, one unit each: a grid cell a triangle of `b` is
/// filed under, a cell a triangle of `a` visits, and a call of `test`. `None` when the budget runs out.
pub fn find_close_pair(
    a: &[BoxedTriangle],
    b: &[BoxedTriangle],
    gap: f64,
    budget: &mut usize,
    test: impl FnMut(&BoxedTriangle, &BoxedTriangle) -> bool,
) -> Option<bool> {
    let work = std::cell::Cell::new(*budget);
    let found = find_close_pair_within(a, b, gap, &work, test);
    *budget = work.get();
    found
}

fn find_close_pair_within(
    a: &[BoxedTriangle],
    b: &[BoxedTriangle],
    gap: f64,
    work: &std::cell::Cell<usize>,
    mut test: impl FnMut(&BoxedTriangle, &BoxedTriangle) -> bool,
) -> Option<bool> {
    let spend = |n: usize| -> Option<()> {
        work.set(work.get().checked_sub(n)?);
        Some(())
    };
    let gap = if gap.is_finite() { gap.max(0.0) } else { 0.0 };
    let close = |x: &Aabb3, y: &Aabb3| {
        x.min.x <= y.max.x + gap
            && y.min.x <= x.max.x + gap
            && x.min.y <= y.max.y + gap
            && y.min.y <= x.max.y + gap
            && x.min.z <= y.max.z + gap
            && y.min.z <= x.max.z + gap
    };
    let mut call = |t: &BoxedTriangle, u: &BoxedTriangle| -> Option<bool> {
        spend(1)?;
        Some(test(t, u))
    };
    let (Some(first), true) = (b.first(), !a.is_empty()) else { return Some(false) };
    // Cell size: the median extent of `b`'s boxes (so most of them span a few cells), at least `gap`.
    let mut extents: Vec<f64> = b
        .iter()
        .map(|(_, bx)| {
            let d = bx.size();
            d.x.max(d.y).max(d.z)
        })
        .filter(|e| e.is_finite())
        .collect();
    let mid = extents.len() / 2;
    let median = if extents.is_empty() { 0.0 } else { *extents.select_nth_unstable_by(mid, f64::total_cmp).1 };
    let origin = first.1.min;
    let cell = median.max(gap).max(1e-9);
    if !(origin.is_finite() && cell.is_finite()) {
        return Some(false);
    }
    let index = |v: f64, o: f64| ((v - o) / cell).floor() as i64;
    let span = |bx: &Aabb3, pad: f64| -> Option<[(i64, i64); 3]> {
        let r = [
            (index(bx.min.x - pad, origin.x), index(bx.max.x + pad, origin.x)),
            (index(bx.min.y - pad, origin.y), index(bx.max.y + pad, origin.y)),
            (index(bx.min.z - pad, origin.z), index(bx.max.z + pad, origin.z)),
        ];
        let count = r.iter().try_fold(1usize, |n, (lo, hi)| n.checked_mul(usize::try_from(hi.checked_sub(*lo)?.checked_add(1)?).ok()?))?;
        (count <= MAX_CELLS_PER_TRIANGLE).then_some(r)
    };
    let mut cells: std::collections::HashMap<(i64, i64, i64), Vec<u32>> = std::collections::HashMap::new();
    let mut big = Vec::new();
    for (j, (_, bx)) in b.iter().enumerate() {
        match span(bx, gap) {
            Some([x, y, z]) => {
                for ix in x.0..=x.1 {
                    for iy in y.0..=y.1 {
                        for iz in z.0..=z.1 {
                            spend(1)?;
                            cells.entry((ix, iy, iz)).or_default().push(j as u32);
                        }
                    }
                }
            }
            None => big.push(j),
        }
    }
    // Per triangle of `b`: the last triangle of `a` tested against it, plus one.
    let mut seen = vec![0usize; b.len()];
    for (i, t) in a.iter().enumerate() {
        for &j in &big {
            if let Some(u) = b.get(j).filter(|u| close(&t.1, &u.1)) {
                seen[j] = i + 1;
                if call(t, u)? {
                    return Some(true);
                }
            }
        }
        let Some([x, y, z]) = span(&t.1, 0.0) else {
            // A large triangle of `a`: against all of `b`.
            for (j, u) in b.iter().enumerate() {
                if seen[j] != i + 1 && close(&t.1, &u.1) && call(t, u)? {
                    return Some(true);
                }
            }
            continue;
        };
        for ix in x.0..=x.1 {
            for iy in y.0..=y.1 {
                for iz in z.0..=z.1 {
                    spend(1)?;
                    for &j in cells.get(&(ix, iy, iz)).into_iter().flatten() {
                        let (Some(mark), Some(u)) = (seen.get_mut(j as usize), b.get(j as usize)) else { continue };
                        if *mark == i + 1 {
                            continue;
                        }
                        *mark = i + 1;
                        if close(&t.1, &u.1) && call(t, u)? {
                            return Some(true);
                        }
                    }
                }
            }
        }
    }
    Some(false)
}

/// Two triangles that pass through each other (see [`Mesh::crosses`]).
/// Where two triangles pass through each other by more than `tol` (see [`Mesh::crosses`]): the
/// middle of the stretch of the line their planes share that both span, or `None`.
fn triangles_cross(t: &[Vec3; 3], u: &[Vec3; 3], tol: f64) -> Option<Vec3> {
    // Signed distances of one triangle's corners from the other's plane, when they lie more
    // than `tol` to both sides of it.
    let sides = |tri: &[Vec3; 3], plane: &[Vec3; 3]| -> Option<(Vec3, [f64; 3])> {
        let n = (plane[1] - plane[0]).cross(plane[2] - plane[0]).normalized()?;
        let d = [n.dot(tri[0] - plane[0]), n.dot(tri[1] - plane[0]), n.dot(tri[2] - plane[0])];
        let (lo, hi) = d.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        (lo < -tol && hi > tol).then_some((n, d))
    };
    let (Some((nu, dt)), Some((nt, du))) = (sides(t, u), sides(u, t)) else { return None };
    let line = nt.cross(nu).normalized()?;
    // Where a triangle's edges pass through the other's plane, measured along the line the
    // two planes share.
    // With the points there, for the middle of the stretch both span.
    let reach = |tri: &[Vec3; 3], d: &[f64; 3]| -> ((f64, Vec3), (f64, Vec3)) {
        let (mut lo, mut hi) = ((f64::INFINITY, Vec3::ZERO), (f64::NEG_INFINITY, Vec3::ZERO));
        for (i, j) in [(0, 1), (1, 2), (2, 0)] {
            let (p, q, dp, dq) = (tri[i], tri[j], d[i], d[j]);
            if (dp < 0.0) != (dq < 0.0) && dp != dq {
                let x = p + (q - p) * (dp / (dp - dq));
                let at = line.dot(x);
                if at < lo.0 {
                    lo = (at, x);
                }
                if at > hi.0 {
                    hi = (at, x);
                }
            }
        }
        (lo, hi)
    };
    let ((t0, t1), (u0, u1)) = (reach(t, &dt), reach(u, &du));
    let (from, to) = (if t0.0 > u0.0 { t0 } else { u0 }, if t1.0 < u1.0 { t1 } else { u1 });
    (to.0 - from.0 > tol).then(|| (from.1 + to.1) * 0.5)
}

/// Mass properties of a closed mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshMeasure {
    pub volume: f64,
    pub area: f64,
    pub centroid: Vec3,
}

/// Volume, centroid and inertia tensor of a closed mesh at unit density: the inertia is about
/// the centroid, in the mesh's axes (times a density, mass and inertia in that density's units).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MassProperties {
    pub volume: f64,
    pub centroid: Vec3,
    pub inertia: [[f64; 3]; 3],
}

impl Mesh {
    /// [`MassProperties`] of a closed mesh, by the divergence theorem over the tetrahedra its
    /// triangles make with the origin. `None` unless every edge (by vertex index: weld first) is
    /// shared by exactly two triangles that cross it in opposite directions (closed, consistently
    /// oriented), or for a mesh without volume or with non-finite values.
    pub fn mass_properties(&self) -> Option<MassProperties> {
        // Closed and consistently oriented: each edge is crossed once in each direction.
        let mut edges: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
        for t in &self.triangles {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a, b)).or_insert(0) += 1;
            }
        }
        if edges.is_empty() || edges.iter().any(|(&(a, b), n)| *n != 1 || edges.get(&(b, a)) != Some(&1)) {
            return None;
        }
        let xyz = |v: Vec3| [v.x, v.y, v.z];
        let (mut v6, mut first, mut second) = (0.0, Vec3::ZERO, [[0.0f64; 3]; 3]);
        for t in &self.triangles {
            let [a, b, c] = self.tri(t)?;
            let det = a.dot(b.cross(c));
            v6 += det;
            first += (a + b + c) * det;
            // ∫ x xᵀ over the tetrahedron (0, a, b, c) = det/120 · (Σ p pᵀ + s sᵀ), s = a + b + c.
            let (p, s) = ([xyz(a), xyz(b), xyz(c)], xyz(a + b + c));
            for (i, row) in second.iter_mut().enumerate() {
                for (j, cell) in row.iter_mut().enumerate() {
                    *cell += det / 120.0 * (p.iter().map(|q| q[i] * q[j]).sum::<f64>() + s[i] * s[j]);
                }
            }
        }
        if !(v6.abs() / 6.0 > 1e-12) {
            return None;
        }
        // Inward-facing: every sum changes sign together.
        let sign = v6.signum();
        let volume = v6.abs() / 6.0;
        let centroid = first / (4.0 * v6);
        let c = xyz(centroid);
        let mut about = [[0.0f64; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                about[i][j] = sign * second[i][j] - volume * c[i] * c[j];
            }
        }
        let trace = about[0][0] + about[1][1] + about[2][2];
        let mut inertia = [[0.0f64; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                inertia[i][j] = if i == j { trace } else { 0.0 } - about[i][j];
            }
        }
        let finite = volume.is_finite() && c.iter().all(|x| x.is_finite()) && inertia.iter().flatten().all(|x| x.is_finite());
        finite.then_some(MassProperties { volume, centroid, inertia })
    }
}

impl Mesh {
    /// Visibility in shaded edge presentation, independent of selection/topology.
    pub fn shaded_edge_visible(&self, index: usize) -> bool {
        self.edge_pickable(index) && !self.smooth_edges.get(index).copied().unwrap_or(false)
    }

    pub fn edge_pickable(&self, index: usize) -> bool {
        !self.seams.get(index).copied().unwrap_or(false) || self.sharp_edges.get(index).copied().unwrap_or(false)
    }

    /// Edges bounding face `f`.
    pub fn face_edges(&self, f: u32) -> Vec<usize> {
        self.edge_faces.iter().enumerate().filter(|(_, fs)| fs.contains(&f)).map(|(i, _)| i).collect()
    }
    /// Pieces of one surface joined across hidden seams, including `face` itself.
    pub fn surface_patch(&self, face: usize) -> Vec<usize> {
        let mut patch = vec![face];
        let mut todo = vec![face];
        while let Some(f) = todo.pop() {
            for (ei, fs) in self.edge_faces.iter().enumerate() {
                if !self.seams.get(ei).copied().unwrap_or(false)
                    || self.sharp_edges.get(ei).copied().unwrap_or(false)
                    || !fs.iter().any(|x| *x as usize == f)
                {
                    continue;
                }
                for &next in fs {
                    let next = next as usize;
                    if !patch.contains(&next) {
                        patch.push(next);
                        todo.push(next);
                    }
                }
            }
        }
        patch.sort_unstable();
        patch
    }

    /// A complete circular rim may be split into several kernel edges. Keep its pieces
    /// together for picking, without joining other tangent curves or changing edge indices.
    pub fn circular_rim(&self, edge: usize) -> Vec<usize> {
        let single = vec![edge];
        let Some(p) = self.edges.get(edge).filter(|p| p.len() >= 3) else { return single };
        let (Some(&a), Some(&b), Some(&c)) = (p.first(), p.get(p.len() / 2), p.last()) else { return single };
        let (u, v) = (b - a, c - a);
        let n = u.cross(v);
        if !(n.len2() > u.len2() * v.len2() * 1e-12) {
            return single;
        }
        let center = a + (v.cross(n) * u.len2() + n.cross(u) * v.len2()) / (2.0 * n.len2());
        let radius = center.dist(a);
        let normal = n.normalized().unwrap_or(Vec3::ZERO);
        let tol = (radius * 1e-6).max(1e-9);
        let mut rim = self.tangent_chain(edge, 2f64.to_radians());
        if rim.len() < 2 || !radius.is_finite() {
            return single;
        }
        // All samples must lie on this circle, and every end must join exactly one other
        // piece. Open arcs, ellipses, and branching tangent chains stay separate.
        for &i in &rim {
            let Some(points) = self.edges.get(i) else { return single };
            if points.iter().any(|q| !q.is_finite() || (q.dist(center) - radius).abs() > tol || (*q - center).dot(normal).abs() > tol) {
                return single;
            }
            for end in [points.first(), points.last()].into_iter().flatten() {
                let joins = rim
                    .iter()
                    .filter(|&&j| j != i)
                    .filter_map(|&j| self.edges.get(j))
                    .flat_map(|q| [q.first(), q.last()])
                    .flatten()
                    .filter(|q| q.dist(*end) < tol)
                    .count();
                if joins != 1 {
                    return single;
                }
            }
        }
        rim.sort_unstable();
        rim
    }
    /// The edges reached from `e` through vertices where the next edge continues smoothly
    /// (angle between the end tangents below `max_angle`), including `e` itself.
    pub fn tangent_chain(&self, e: usize, max_angle: f64) -> Vec<usize> {
        // (end point, direction leaving the edge there). A polyline's end chord leans inward
        // on a curve by half the turn to the next chord; extrapolating the two end chords
        // recovers the curve's end tangent.
        let end_dir = |p0: Vec3, p1: Vec3, p2: Option<Vec3>| -> Option<Vec3> {
            let u1 = (p0 - p1).normalized()?;
            match p2.and_then(|p2| (p1 - p2).normalized()) {
                Some(u2) => (u1 * 1.5 - u2 * 0.5).normalized(),
                None => Some(u1),
            }
        };
        let ends = |i: usize| -> Option<[(Vec3, Vec3); 2]> {
            let p = self.edges.get(i)?;
            let n = p.len();
            let (a, a1, a2) = (*p.first()?, *p.get(1)?, p.get(2).copied().filter(|_| n > 2));
            let (b, b1) = (*p.last()?, *p.get(n.checked_sub(2)?)?);
            let b2 = n.checked_sub(3).and_then(|k| p.get(k)).copied();
            Some([(a, end_dir(a, a1, a2)?), (b, end_dir(b, b1, b2)?)])
        };
        let size = self.bounds().diagonal().max(1e-9);
        let tol = size * 1e-7;
        let cos = max_angle.cos();
        let mut chain = vec![e];
        let mut todo = vec![e];
        while let Some(cur) = todo.pop() {
            let Some(ce) = ends(cur) else { continue };
            for (i, _) in self.edges.iter().enumerate() {
                if chain.contains(&i) || !self.edge_pickable(i) {
                    continue;
                }
                let Some(ne) = ends(i) else { continue };
                // Smooth: leaving `cur` at a shared end continues into `i` (opposite directions).
                let smooth = ce.iter().any(|(p, d)| ne.iter().any(|(q, dn)| p.dist(*q) < tol && d.dot(-*dn) > cos));
                if smooth && chain.len() < 10_000 {
                    chain.push(i);
                    todo.push(i);
                }
            }
        }
        chain
    }
    pub fn tri(&self, t: &[u32; 3]) -> Option<[Vec3; 3]> {
        Some([*self.positions.get(t[0] as usize)?, *self.positions.get(t[1] as usize)?, *self.positions.get(t[2] as usize)?])
    }

    pub fn bounds(&self) -> Aabb3 {
        let mut b = Aabb3::EMPTY;
        for p in &self.positions {
            b.add(*p);
        }
        b
    }

    /// Signed volume (divergence theorem), surface area and volume centroid.
    pub fn measure(&self) -> MeshMeasure {
        let (mut v6, mut area, mut c) = (0.0, 0.0, Vec3::ZERO);
        for t in &self.triangles {
            let Some([a, b, cc]) = self.tri(t) else { continue };
            let d = a.dot(b.cross(cc));
            v6 += d;
            c += (a + b + cc) * d;
            area += (b - a).cross(cc - a).len() * 0.5;
        }
        let volume = v6 / 6.0;
        let centroid = if v6.abs() > 1e-12 { c / (4.0 * v6) } else { self.bounds().center() };
        MeshMeasure { volume, area, centroid }
    }

    /// Append another mesh (face ids offset by `face_offset`).
    pub fn append(&mut self, o: &Mesh, face_offset: u32) {
        let base = u32::try_from(self.positions.len()).unwrap_or(u32::MAX);
        self.positions.extend_from_slice(&o.positions);
        self.normals.extend_from_slice(&o.normals);
        self.triangles.extend(o.triangles.iter().map(|t| [t[0].saturating_add(base), t[1].saturating_add(base), t[2].saturating_add(base)]));
        self.tri_face.extend(o.tri_face.iter().map(|f| f.saturating_add(face_offset)));
        self.edges.extend(o.edges.iter().cloned());
    }

    /// Number of triangles a ray crosses (for inside/outside tests on closed meshes).
    pub fn ray_crossings(&self, o: Vec3, dir: Vec3) -> usize {
        let mut n = 0;
        for t in &self.triangles {
            let Some([a, b, c]) = self.tri(t) else { continue };
            let (e1, e2) = (b - a, c - a);
            let p = dir.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-14 {
                continue;
            }
            let inv = 1.0 / det;
            let s = o - a;
            let u = s.dot(p) * inv;
            if !(0.0..1.0).contains(&u) {
                continue;
            }
            let q = s.cross(e1);
            let v = dir.dot(q) * inv;
            if v < 0.0 || u + v >= 1.0 {
                continue;
            }
            if e2.dot(q) * inv > 0.0 {
                n += 1;
            }
        }
        n
    }

    /// Point inside a closed mesh (majority vote of three skewed rays).
    pub fn contains(&self, p: Vec3) -> bool {
        let dirs = [Vec3::new(0.5773, 0.5774, 0.5776), Vec3::new(-0.6123, 0.3141, 0.7254), Vec3::new(0.2718, -0.8414, 0.4673)];
        dirs.iter().filter(|d| self.ray_crossings(p, **d) % 2 == 1).count() >= 2
    }

    /// Whether the surfaces of two meshes pass through each other by more than `tol`: some
    /// triangle of one has corners more than `tol` to both sides of a triangle of the other,
    /// and the reverse, and they share a stretch of the line where their planes meet longer
    /// than `tol`. Faces that only touch, lie on each other or stop on the other surface don't
    /// count, nor do two facetings of one curved surface that are within `tol` of it. Bodies
    /// can still overlap without crossing: one inside the other, or sharing a slab between
    /// faces that are parallel or in one plane.
    pub fn crosses(&self, o: &Mesh, tol: f64) -> bool {
        self.crosses_except(o, tol, |_| false)
    }

    /// [`Mesh::crosses`], leaving out the places where `ignored` holds for the middle of the
    /// stretch two triangles share: where the meshes stand for one surface of both bodies, and
    /// their facets cross by the facetings' error alone.
    pub fn crosses_except(&self, o: &Mesh, tol: f64, ignored: impl Fn(Vec3) -> bool) -> bool {
        if !tol.is_finite() {
            return false;
        }
        let tol = tol.max(0.0);
        let (ba, bb) = (self.bounds(), o.bounds());
        let region = Aabb3 { min: ba.min.max(bb.min), max: ba.max.min(bb.max) };
        if region.is_empty() {
            return false;
        }
        let near = |m: &Mesh| -> Vec<([Vec3; 3], Aabb3)> {
            m.triangles
                .iter()
                .filter_map(|t| {
                    let t = m.tri(t)?;
                    let mut bx = Aabb3::EMPTY;
                    t.iter().for_each(|p| bx.add(*p));
                    (!bx.is_empty() && boxes_meet(&bx, &region)).then_some((t, bx))
                })
                .collect()
        };
        let (ta, tb) = (near(self), near(o));
        if ta.is_empty() || tb.is_empty() {
            return false;
        }
        let mut unlimited = usize::MAX;
        find_close_pair(&ta, &tb, 0.0, &mut unlimited, |(t, _), (u, _)| triangles_cross(t, u, tol).is_some_and(|at| !ignored(at))).unwrap_or(false)
    }

    /// Closest intersection of a ray with the mesh: (distance along `dir`, triangle index).
    ///
    /// Watertight (Woop, Benthin & Wald, "Watertight Ray/Triangle Intersection", JCGT 2(1),
    /// 2013): the vertices are moved into a frame where the ray runs along +z through the
    /// origin, and each edge's side test is computed from those per-vertex values alone, so the
    /// two triangles that share an edge get exactly opposite values there. A ray through a
    /// shared edge or vertex hits at least one of them; it never slips between them.
    pub fn raycast(&self, o: Vec3, dir: Vec3) -> Option<(f64, usize)> {
        let frame = RayFrame::new(o, dir)?;
        let mut best: Option<(f64, usize)> = None;
        for (i, t) in self.triangles.iter().enumerate() {
            let Some(p) = self.tri(t) else { continue };
            if let Some(t) = frame.hit(p)
                && best.is_none_or(|(bt, _)| t < bt)
            {
                best = Some((t, i));
            }
        }
        best
    }
}

/// A ray in the frame of [`Mesh::raycast`]'s watertight test: its dominant axis becomes z; x and
/// y follow it cyclically, swapped to keep the frame right-handed when the ray runs towards −z.
pub(crate) struct RayFrame {
    o: Vec3,
    k: [usize; 3],
    s: [f64; 3],
}

fn comp(v: Vec3, k: usize) -> f64 {
    match k {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

impl RayFrame {
    /// `None` for a direction that is zero or not finite.
    pub(crate) fn new(o: Vec3, dir: Vec3) -> Option<RayFrame> {
        let a = [dir.x.abs(), dir.y.abs(), dir.z.abs()];
        let kz = if a[0] >= a[1] && a[0] >= a[2] {
            0
        } else if a[1] >= a[2] {
            1
        } else {
            2
        };
        let dz = comp(dir, kz);
        if !(dz.is_finite() && dz != 0.0) {
            return None;
        }
        let (mut kx, mut ky) = ((kz + 1) % 3, (kz + 2) % 3);
        if dz < 0.0 {
            std::mem::swap(&mut kx, &mut ky);
        }
        Some(RayFrame { o, k: [kx, ky, kz], s: [comp(dir, kx) / dz, comp(dir, ky) / dz, 1.0 / dz] })
    }

    /// A vertex in the ray's frame: (x, y) across the ray, z scaled along it.
    fn frame(&self, p: Vec3) -> (f64, f64, f64) {
        let q = p - self.o;
        let z = comp(q, self.k[2]);
        (comp(q, self.k[0]) - self.s[0] * z, comp(q, self.k[1]) - self.s[1] * z, self.s[2] * z)
    }

    /// The distance along the ray to triangle `[pa, pb, pc]` (`None`: missed, or not in front).
    pub(crate) fn hit(&self, [pa, pb, pc]: [Vec3; 3]) -> Option<f64> {
        let ((ax, ay, az), (bx, by, bz), (cx, cy, cz)) = (self.frame(pa), self.frame(pb), self.frame(pc));
        let u = cx * by - cy * bx;
        let v = ax * cy - ay * cx;
        let w = bx * ay - by * ax;
        if (u < 0.0 || v < 0.0 || w < 0.0) && (u > 0.0 || v > 0.0 || w > 0.0) {
            return None;
        }
        let det = u + v + w;
        if det == 0.0 {
            return None;
        }
        let t = (u * az + v * bz + w * cz) / det;
        (t.is_finite() && t > 1e-9).then_some(t)
    }
}

/// Point-in-mesh queries in bulk: for each of `Mesh::contains`'s three ray directions, the
/// triangles binned on a grid in the plane square to it, so a ray tests only the triangles
/// whose shadow covers its cell. Answers exactly as `Mesh::contains` does.
pub struct InsideIndex<'a> {
    mesh: &'a Mesh,
    rays: Vec<RayGrid>,
}

struct RayGrid {
    dir: Vec3,
    u: Vec3,
    v: Vec3,
    min: (f64, f64),
    cell: f64,
    n: (usize, usize),
    cells: Vec<Vec<u32>>,
}

const RAY_DIRS: [Vec3; 3] = [Vec3::new(0.5773, 0.5774, 0.5776), Vec3::new(-0.6123, 0.3141, 0.7254), Vec3::new(0.2718, -0.8414, 0.4673)];

impl Mesh {
    /// An index for many `contains` queries on this mesh.
    pub fn inside_index(&self) -> InsideIndex<'_> {
        let rays = RAY_DIRS
            .iter()
            .map(|d| {
                let u = d.any_perp();
                let v = d.cross(u);
                let tris: Vec<[(f64, f64); 3]> = self
                    .triangles
                    .iter()
                    .map(|t| match self.tri(t) {
                        Some([a, b, c]) => [(a.dot(u), a.dot(v)), (b.dot(u), b.dot(v)), (c.dot(u), c.dot(v))],
                        None => [(0.0, 0.0); 3],
                    })
                    .collect();
                let (mut lo, mut hi) = ((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY));
                for t in &tris {
                    for p in t {
                        lo = (lo.0.min(p.0), lo.1.min(p.1));
                        hi = (hi.0.max(p.0), hi.1.max(p.1));
                    }
                }
                let span = (hi.0 - lo.0).max(hi.1 - lo.1).max(1e-9);
                let side = ((tris.len() as f64).sqrt().ceil() as usize).clamp(1, 256);
                let cell = span / side as f64 * (1.0 + 1e-9);
                let n = (side + 1, side + 1);
                let mut cells = vec![Vec::new(); n.0 * n.1];
                if lo.0.is_finite() {
                    for (i, t) in tris.iter().enumerate() {
                        let (x0, x1) = (t.iter().map(|p| p.0).fold(f64::INFINITY, f64::min), t.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max));
                        let (y0, y1) = (t.iter().map(|p| p.1).fold(f64::INFINITY, f64::min), t.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max));
                        let c = |x: f64, o: f64, m: usize| (((x - o) / cell).floor().max(0.0) as usize).min(m - 1);
                        for gx in c(x0, lo.0, n.0)..=c(x1, lo.0, n.0) {
                            for gy in c(y0, lo.1, n.1)..=c(y1, lo.1, n.1) {
                                if let Some(slot) = cells.get_mut(gx * n.1 + gy) {
                                    slot.push(i as u32);
                                }
                            }
                        }
                    }
                }
                RayGrid { dir: *d, u, v, min: lo, cell, n, cells }
            })
            .collect();
        InsideIndex { mesh: self, rays }
    }
}

impl InsideIndex<'_> {
    fn crossings(&self, g: &RayGrid, o: Vec3) -> usize {
        let (x, y) = (o.dot(g.u), o.dot(g.v));
        if !(g.min.0.is_finite()) {
            return 0;
        }
        let (fx, fy) = ((x - g.min.0) / g.cell, (y - g.min.1) / g.cell);
        if fx < 0.0 || fy < 0.0 {
            return 0;
        }
        let (gx, gy) = (fx.floor() as usize, fy.floor() as usize);
        if gx >= g.n.0 || gy >= g.n.1 {
            return 0;
        }
        let Some(list) = g.cells.get(gx * g.n.1 + gy) else { return 0 };
        let dir = g.dir;
        let mut n = 0;
        for &i in list {
            let Some(t) = self.mesh.triangles.get(i as usize) else { continue };
            let Some([a, b, c]) = self.mesh.tri(t) else { continue };
            let (e1, e2) = (b - a, c - a);
            let p = dir.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-14 {
                continue;
            }
            let inv = 1.0 / det;
            let s = o - a;
            let u = s.dot(p) * inv;
            if !(0.0..1.0).contains(&u) {
                continue;
            }
            let q = s.cross(e1);
            let v = dir.dot(q) * inv;
            if v < 0.0 || u + v >= 1.0 {
                continue;
            }
            if e2.dot(q) * inv > 0.0 {
                n += 1;
            }
        }
        n
    }

    /// Same answer as `Mesh::contains`.
    pub fn contains(&self, p: Vec3) -> bool {
        self.rays.iter().filter(|g| self.crossings(g, p) % 2 == 1).count() >= 2
    }
}
