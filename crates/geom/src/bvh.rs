//! A bounding volume hierarchy over boxes: the nearest hit along a ray, or every item whose box
//! passes a test, without looking at each item. Built once (median split on the widest axis of
//! the box centres, a few items per leaf) and only read afterwards. [`MeshIndex`] holds one over
//! a mesh's triangles and one over its edge segments, for picking.

use crate::{Aabb3, Mesh, Vec3};

/// Most items in a leaf.
const LEAF: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb3,
    /// A leaf: the first of its `count` items in [`Bvh::order`]. Otherwise its children are
    /// `first` and `first + 1`.
    first: u32,
    count: u32,
}

/// A hierarchy over boxes; items are the boxes' indices.
#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

impl Bvh {
    /// The hierarchy over `boxes` (empty boxes are left out).
    pub fn new(boxes: &[Aabb3]) -> Bvh {
        let mut order: Vec<u32> =
            (0..boxes.len().min(u32::MAX as usize)).filter(|&i| boxes.get(i).is_some_and(|b| !b.is_empty() && finite(b))).map(|i| i as u32).collect();
        if order.is_empty() {
            return Bvh::default();
        }
        let item = |i: u32| boxes.get(i as usize).copied().unwrap_or(Aabb3::EMPTY);
        let centre = |i: u32| item(i).center();
        let mut nodes = vec![Node { bounds: Aabb3::EMPTY, first: 0, count: 0 }];
        // (node, start, end) in `order` still to be filled in.
        let mut todo = vec![(0usize, 0usize, order.len())];
        while let Some((n, start, end)) = todo.pop() {
            let Some(span) = order.get_mut(start..end) else { continue };
            let mut bounds = Aabb3::EMPTY;
            let mut centres = Aabb3::EMPTY;
            for &i in span.iter() {
                bounds = bounds.union(&item(i));
                centres.add(centre(i));
            }
            let size = centres.size();
            let axis = if size.x >= size.y && size.x >= size.z {
                0
            } else if size.y >= size.z {
                1
            } else {
                2
            };
            let extent = [size.x, size.y, size.z][axis];
            if span.len() <= LEAF || !(extent > 0.0) {
                if let Some(node) = nodes.get_mut(n) {
                    *node = Node { bounds, first: start as u32, count: span.len() as u32 };
                }
                continue;
            }
            let key = |i: &u32| {
                let c = centre(*i);
                [c.x, c.y, c.z][axis]
            };
            let mid = span.len() / 2;
            span.select_nth_unstable_by(mid, |a, b| key(a).total_cmp(&key(b)));
            let left = nodes.len();
            nodes.push(Node { bounds: Aabb3::EMPTY, first: 0, count: 0 });
            nodes.push(Node { bounds: Aabb3::EMPTY, first: 0, count: 0 });
            if let Some(node) = nodes.get_mut(n) {
                *node = Node { bounds, first: left as u32, count: 0 };
            }
            todo.push((left, start, start + mid));
            todo.push((left + 1, start + mid, end));
        }
        Bvh { nodes, order }
    }

    /// The box around everything (empty for no items).
    pub fn bounds(&self) -> Aabb3 {
        self.nodes.first().map_or(Aabb3::EMPTY, |n| n.bounds)
    }

    /// Calls `item` with every item in a leaf whose box, and every box above it, `enter` accepts.
    pub fn visit(&self, mut enter: impl FnMut(&Aabb3) -> bool, mut item: impl FnMut(u32)) {
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            let Some(node) = self.nodes.get(n) else { continue };
            if !enter(&node.bounds) {
                continue;
            }
            if node.count > 0 {
                let (a, b) = (node.first as usize, node.first as usize + node.count as usize);
                for &i in self.order.get(a..b).unwrap_or(&[]) {
                    item(i);
                }
            } else {
                stack.push(node.first as usize + 1);
                stack.push(node.first as usize);
            }
        }
    }

    /// The nearest item along the ray from `o` along `d`: `hit` gives an item's distance along
    /// `d` (`None`: missed). Equal distances go to the lowest item, as a scan in item order
    /// keeping strictly nearer hits would answer. Boxes are entered nearest first, and one that
    /// starts past the best hit so far is skipped.
    pub fn nearest(&self, o: Vec3, d: Vec3, mut hit: impl FnMut(u32) -> Option<f64>) -> Option<(f64, u32)> {
        if ![o.x, o.y, o.z, d.x, d.y, d.z].iter().all(|v| v.is_finite()) {
            return None;
        }
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut best: Option<(f64, u32)> = None;
        let root = self.nodes.first().and_then(|n| ray_box(o, d, inv, &n.bounds, 0.0))?;
        let mut stack = vec![(0usize, root)];
        while let Some((n, t0)) = stack.pop() {
            if best.is_some_and(|(bt, _)| t0 > bt) {
                continue;
            }
            let Some(node) = self.nodes.get(n) else { continue };
            if node.count > 0 {
                let (a, b) = (node.first as usize, node.first as usize + node.count as usize);
                for &i in self.order.get(a..b).unwrap_or(&[]) {
                    if let Some(t) = hit(i)
                        && best.is_none_or(|(bt, bi)| t < bt || (t == bt && i < bi))
                    {
                        best = Some((t, i));
                    }
                }
                continue;
            }
            let (l, r) = (node.first as usize, node.first as usize + 1);
            let tl = self.nodes.get(l).and_then(|c| ray_box(o, d, inv, &c.bounds, 0.0));
            let tr = self.nodes.get(r).and_then(|c| ray_box(o, d, inv, &c.bounds, 0.0));
            // The nearer child is popped first.
            match (tl, tr) {
                (Some(a), Some(b)) if a <= b => stack.extend([(r, b), (l, a)]),
                (Some(a), Some(b)) => stack.extend([(l, a), (r, b)]),
                (Some(a), None) => stack.push((l, a)),
                (None, Some(b)) => stack.push((r, b)),
                (None, None) => {}
            }
        }
        best
    }

    /// Calls `item(i, j)` for pairs of items, `i` from this hierarchy and `j` from `other`, in
    /// leaves whose boxes, and every pair of boxes above them, `enter` accepts. `enter` is asked
    /// again for a pair of boxes when it is taken up, so a bound that tightens while items are
    /// visited (the nearest pair so far) prunes what is still waiting. Of two children, the pair
    /// whose box centres are nearer is taken up first.
    pub fn visit_pairs(&self, other: &Bvh, mut enter: impl FnMut(&Aabb3, &Aabb3) -> bool, mut item: impl FnMut(u32, u32)) {
        if self.nodes.is_empty() || other.nodes.is_empty() {
            return;
        }
        let mut stack = vec![(0usize, 0usize)];
        while let Some((i, j)) = stack.pop() {
            let (Some(a), Some(b)) = (self.nodes.get(i), other.nodes.get(j)) else { continue };
            if !enter(&a.bounds, &b.bounds) {
                continue;
            }
            match (a.count > 0, b.count > 0) {
                (true, true) => {
                    let (ia, ib) = (a.first as usize, b.first as usize);
                    let left = self.order.get(ia..ia + a.count as usize).unwrap_or(&[]);
                    let right = other.order.get(ib..ib + b.count as usize).unwrap_or(&[]);
                    for &x in left {
                        for &y in right {
                            item(x, y);
                        }
                    }
                }
                // Split the node that is not a leaf, or the larger one.
                (leaf_a, leaf_b) => {
                    let split_a = !leaf_a && (leaf_b || a.bounds.diagonal() >= b.bounds.diagonal());
                    let pairs =
                        if split_a { [(a.first as usize, j), (a.first as usize + 1, j)] } else { [(i, b.first as usize), (i, b.first as usize + 1)] };
                    let gap = |(x, y): (usize, usize)| match (self.nodes.get(x), other.nodes.get(y)) {
                        (Some(x), Some(y)) => x.bounds.center().dist(y.bounds.center()),
                        _ => f64::INFINITY,
                    };
                    let [p, q] = pairs;
                    // The nearer pair is popped first.
                    if gap(p) <= gap(q) {
                        stack.extend([q, p]);
                    } else {
                        stack.extend([p, q]);
                    }
                }
            }
        }
    }
}

fn finite(b: &Aabb3) -> bool {
    [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z].iter().all(|v| v.is_finite())
}

/// Where the ray from `o` along `d` (`inv` = 1 / `d` per axis) enters box `b` grown by `pad`,
/// from 0 on; `None` when it misses or the box lies behind.
fn ray_box(o: Vec3, d: Vec3, inv: Vec3, b: &Aabb3, pad: f64) -> Option<f64> {
    let (mut t0, mut t1) = (0.0f64, f64::INFINITY);
    for (o, d, inv, lo, hi) in [(o.x, d.x, inv.x, b.min.x, b.max.x), (o.y, d.y, inv.y, b.min.y, b.max.y), (o.z, d.z, inv.z, b.min.z, b.max.z)] {
        let (lo, hi) = (lo - pad, hi + pad);
        if d == 0.0 || !inv.is_finite() {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let (a, b) = ((lo - o) * inv, (hi - o) * inv);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    (t0 <= t1).then_some(t0)
}

/// The farthest any corner of `b` lies along `d` from `o` (0 when the box is behind).
fn farthest_along(o: Vec3, d: Vec3, b: &Aabb3) -> f64 {
    let along = |lo: f64, hi: f64, o: f64, d: f64| if d >= 0.0 { (hi - o) * d } else { (lo - o) * d };
    (along(b.min.x, b.max.x, o.x, d.x) + along(b.min.y, b.max.y, o.y, d.y) + along(b.min.z, b.max.z, o.z, d.z)).max(0.0)
}

/// A mesh's triangles and edge segments in hierarchies, for picking: the nearest triangle along
/// a ray, and the edge segments near one. Built from one mesh and answers only for it.
#[derive(Clone, Debug, Default)]
pub struct MeshIndex {
    triangles: Bvh,
    triangle_count: usize,
    segments: Bvh,
    /// Per segment item: its edge, and its first point in that edge's polyline.
    segment_of: Vec<(u32, u32)>,
}

impl MeshIndex {
    /// The index of `mesh`.
    pub fn new(mesh: &Mesh) -> MeshIndex {
        let pad = |mut b: Aabb3| {
            // Flat boxes (a triangle square to an axis) are grown a little, so a ray through
            // them is never lost to rounding in the box test.
            let scale = [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z].iter().fold(1.0f64, |m, v| m.max(v.abs()));
            let e = scale * 1e-9;
            b.min -= Vec3::new(e, e, e);
            b.max += Vec3::new(e, e, e);
            b
        };
        let boxes: Vec<Aabb3> = mesh
            .triangles
            .iter()
            .map(|t| {
                let mut b = Aabb3::EMPTY;
                for p in mesh.tri(t).into_iter().flatten() {
                    b.add(p);
                }
                if b.is_empty() { b } else { pad(b) }
            })
            .collect();
        let mut segment_of = Vec::new();
        let mut seg_boxes = Vec::new();
        for (ei, e) in mesh.edges.iter().enumerate() {
            // A one-point edge still has its vertex: it stands as a segment from it to itself.
            let n = if e.len() == 1 { 1 } else { e.len().saturating_sub(1) };
            for si in 0..n {
                let (Some(a), Some(b)) = (e.get(si), e.get(si + 1).or(e.get(si))) else { continue };
                let mut bx = Aabb3::EMPTY;
                bx.add(*a);
                bx.add(*b);
                seg_boxes.push(pad(bx));
                segment_of.push((ei as u32, si as u32));
            }
        }
        MeshIndex { triangles: Bvh::new(&boxes), triangle_count: mesh.triangles.len(), segments: Bvh::new(&seg_boxes), segment_of }
    }

    /// [`Mesh::raycast`] through the index: the same answer, including which triangle wins a tie.
    /// For a mesh other than the one indexed, the plain scan answers.
    pub fn raycast(&self, mesh: &Mesh, o: Vec3, dir: Vec3) -> Option<(f64, usize)> {
        if mesh.triangles.len() != self.triangle_count {
            return mesh.raycast(o, dir);
        }
        let frame = crate::mesh::RayFrame::new(o, dir)?;
        self.triangles
            .nearest(o, dir, |i| mesh.triangles.get(i as usize).and_then(|t| mesh.tri(t)).and_then(|p| frame.hit(p)))
            .map(|(t, i)| (t, i as usize))
    }

    /// Calls `f(edge, first point)` for every edge segment that may come within `a + b·t` of the
    /// ray from `o` along unit `d`, `t` being the distance along it (each at most once). A
    /// constant `a` suits an orthographic view; a perspective view widens with `b`.
    pub fn segments_near(&self, o: Vec3, d: Vec3, a: f64, b: f64, mut f: impl FnMut(usize, usize)) {
        if ![a, b, o.x, o.y, o.z, d.x, d.y, d.z].iter().all(|v| v.is_finite()) {
            return;
        }
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        self.segments.visit(
            |bx| {
                let reach = a.max(0.0) + b.max(0.0) * farthest_along(o, d, bx);
                ray_box(o, d, inv, bx, reach).is_some()
            },
            |i| {
                if let Some((e, s)) = self.segment_of.get(i as usize) {
                    f(*e as usize, *s as usize);
                }
            },
        );
    }
}

#[cfg(test)]
#[path = "bvh_tests.rs"]
mod tests;
