//! Flat, deterministic binary hierarchy with resumable constant-size build work.
use super::{Bounds, Hit, MAX_BUILD_STEP, MAX_WORK, Ray, Result, TraceError, Triangle, WorkBudget, check_cancel, component};
use crate::job::CancelToken;
use std::mem::size_of;

pub const MAX_TRIANGLES: usize = 1_000_000;
pub const MAX_DEPTH: u8 = 48;
pub const MAX_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
const STACK_SLOTS: usize = MAX_DEPTH as usize + 1;

/// Logical owned-buffer admission. Does not reserve the host pool or promise OS RSS.
#[derive(Clone, Copy, Debug)]
pub struct BuildLimits {
    pub max_triangles: usize,
    pub max_memory_bytes: u64,
    pub max_work: u64,
    pub max_depth: u8,
    pub leaf_size: usize,
}
impl Default for BuildLimits {
    fn default() -> Self {
        Self { max_triangles: 100_000, max_memory_bytes: 128 * 1024 * 1024, max_work: 10_000_000, max_depth: 32, leaf_size: 8 }
    }
}
impl BuildLimits {
    fn validate(self) -> Result<()> {
        if self.max_triangles == 0
            || self.max_triangles > MAX_TRIANGLES
            || self.max_memory_bytes == 0
            || self.max_memory_bytes > MAX_MEMORY_BYTES
            || self.max_work == 0
            || self.max_work > MAX_WORK
            || self.max_depth == 0
            || self.max_depth > MAX_DEPTH
            || self.leaf_size == 0
            || self.leaf_size > 64
        {
            return Err(TraceError::InvalidInput("BVH build limits exceed supported bounds"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Leaf { start: usize, end: usize },
    Branch { left: usize, right: usize },
}
#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Bounds,
    kind: Kind,
}
impl Node {
    fn pending() -> Self {
        Self { bounds: Bounds::empty(), kind: Kind::Leaf { start: 0, end: 0 } }
    }
}
#[derive(Clone, Copy, Debug)]
struct Task {
    node: usize,
    start: usize,
    end: usize,
    depth: u8,
}
#[derive(Clone, Copy, Debug)]
enum State {
    Loading(usize),
    Next,
    Bounds { task: Task, cursor: usize, bounds: Bounds, centers: Bounds },
    Partition { task: Task, cursor: usize, split: usize, axis: usize, plane: f64, bounds: Bounds },
    Done,
}

/// No partial acceleration structure escapes this builder. Each `step` operation
/// handles at most one input ordinal, bound or partition item, or one node transition.
pub struct BvhBuilder {
    triangles: Vec<Triangle>,
    order: Vec<usize>,
    nodes: Vec<Node>,
    pending: Vec<Task>,
    state: State,
    limits: BuildLimits,
    work: WorkBudget,
    admitted_bytes: u64,
}

/// Progress is work consumed, not a speculative completion percentage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildProgress {
    pub complete: bool,
    pub work_used: u64,
}

fn bytes_for(count: usize, item: usize) -> Result<u64> {
    let bytes = count.checked_mul(item).ok_or(TraceError::MemoryLimit)?;
    u64::try_from(bytes).map_err(|_| TraceError::MemoryLimit)
}

fn memory(triangles: usize, order: usize, nodes: usize, pending: usize) -> Result<u64> {
    let mut total = (size_of::<BvhBuilder>() + size_of::<Bvh>() + size_of::<[usize; STACK_SLOTS]>()) as u64;
    for amount in [
        bytes_for(triangles, size_of::<Triangle>())?,
        bytes_for(order, size_of::<usize>())?,
        bytes_for(nodes, size_of::<Node>())?,
        bytes_for(pending, size_of::<Task>())?,
    ] {
        total = total.checked_add(amount).ok_or(TraceError::MemoryLimit)?;
    }
    Ok(total)
}

impl BvhBuilder {
    /// Includes the supplied input Vec's capacity, maximum nodes, ordinal array,
    /// pending tasks and one traversal stack. No hidden full-scene sorting copy.
    pub fn required_bytes(triangle_count: usize, input_capacity: usize, limits: BuildLimits) -> Result<u64> {
        limits.validate()?;
        if triangle_count > limits.max_triangles || input_capacity < triangle_count {
            return Err(TraceError::InvalidInput("triangle count or input capacity"));
        }
        let nodes =
            if triangle_count == 0 { 0 } else { triangle_count.checked_mul(2).and_then(|n| n.checked_sub(1)).ok_or(TraceError::MemoryLimit)? };
        let amount = memory(input_capacity, triangle_count, nodes, usize::from(limits.max_depth) + 1)?;
        if amount > limits.max_memory_bytes {
            return Err(TraceError::MemoryLimit);
        }
        Ok(amount)
    }

    /// Takes ownership of already admitted primitives. Checks/reserves before
    /// building; copying/initializing one million ordinals is deferred to `step`.
    pub fn new(triangles: Vec<Triangle>, limits: BuildLimits, cancel: &CancelToken) -> Result<Self> {
        check_cancel(cancel)?;
        let n = triangles.len();
        Self::required_bytes(n, triangles.capacity(), limits)?;
        let node_capacity = if n == 0 { 0 } else { n.checked_mul(2).and_then(|v| v.checked_sub(1)).ok_or(TraceError::MemoryLimit)? };
        let mut order = Vec::new();
        order.try_reserve_exact(n).map_err(|_| TraceError::AllocationFailed)?;
        check_cancel(cancel)?;
        let mut nodes = Vec::new();
        nodes.try_reserve_exact(node_capacity).map_err(|_| TraceError::AllocationFailed)?;
        check_cancel(cancel)?;
        let mut pending = Vec::new();
        pending.try_reserve_exact(usize::from(limits.max_depth) + 1).map_err(|_| TraceError::AllocationFailed)?;
        let admitted_bytes = memory(triangles.capacity(), order.capacity(), nodes.capacity(), pending.capacity())?;
        if admitted_bytes > limits.max_memory_bytes {
            return Err(TraceError::MemoryLimit);
        }
        let state = if n == 0 {
            State::Done
        } else {
            nodes.push(Node::pending());
            pending.push(Task { node: 0, start: 0, end: n, depth: 0 });
            State::Loading(0)
        };
        check_cancel(cancel)?;
        Ok(Self { triangles, order, nodes, pending, state, limits, work: WorkBudget::new(limits.max_work)?, admitted_bytes })
    }

    pub fn admitted_bytes(&self) -> u64 {
        self.admitted_bytes
    }

    fn triangle_at(&self, ordinal: usize) -> Result<Triangle> {
        let index = *self.order.get(ordinal).ok_or(TraceError::Internal("build ordinal"))?;
        self.triangles.get(index).copied().ok_or(TraceError::Internal("build primitive"))
    }

    pub fn step(&mut self, operations: usize, cancel: &CancelToken) -> Result<BuildProgress> {
        if operations == 0 || operations > MAX_BUILD_STEP {
            return Err(TraceError::InvalidInput("build step outside 1..=MAX_BUILD_STEP"));
        }
        check_cancel(cancel)?;
        for _ in 0..operations {
            if matches!(self.state, State::Done) {
                break;
            }
            self.work.tick(cancel)?;
            self.state = match self.state {
                State::Loading(cursor) => {
                    if cursor < self.triangles.len() {
                        self.order.push(cursor);
                        State::Loading(cursor + 1)
                    } else {
                        State::Next
                    }
                }
                State::Next => match self.pending.pop() {
                    Some(task) => State::Bounds { task, cursor: task.start, bounds: Bounds::empty(), centers: Bounds::empty() },
                    None => State::Done,
                },
                State::Bounds { task, cursor, mut bounds, mut centers } => {
                    if cursor < task.end {
                        let triangle = self.triangle_at(cursor)?;
                        bounds = bounds.include(triangle.bounds());
                        centers = centers.include(Bounds::point(triangle.centroid()));
                        State::Bounds { task, cursor: cursor + 1, bounds, centers }
                    } else if task.end - task.start <= self.limits.leaf_size || task.depth >= self.limits.max_depth {
                        let node = self.nodes.get_mut(task.node).ok_or(TraceError::Internal("build leaf"))?;
                        *node = Node { bounds, kind: Kind::Leaf { start: task.start, end: task.end } };
                        State::Next
                    } else {
                        let extent = centers.max - centers.min;
                        let axis = if extent.x >= extent.y && extent.x >= extent.z {
                            0
                        } else if extent.y >= extent.z {
                            1
                        } else {
                            2
                        };
                        let plane = component(centers.min, axis) * 0.5 + component(centers.max, axis) * 0.5;
                        State::Partition { task, cursor: task.start, split: task.start, axis, plane, bounds }
                    }
                }
                State::Partition { task, cursor, mut split, axis, plane, bounds } => {
                    if cursor < task.end {
                        if component(self.triangle_at(cursor)?.centroid(), axis) < plane {
                            // `swap` indices are bounded by this private task range,
                            // but still check rather than panic if an invariant breaks.
                            let value = *self.order.get(cursor).ok_or(TraceError::Internal("partition cursor"))?;
                            let other = *self.order.get(split).ok_or(TraceError::Internal("partition split"))?;
                            *self.order.get_mut(cursor).ok_or(TraceError::Internal("partition cursor"))? = other;
                            *self.order.get_mut(split).ok_or(TraceError::Internal("partition split"))? = value;
                            split += 1;
                        }
                        State::Partition { task, cursor: cursor + 1, split, axis, plane, bounds }
                    } else {
                        // Coincident centroids still make progress, in existing input
                        // order. Never recurse or retry an empty partition forever.
                        if split == task.start || split == task.end {
                            split = task.start + (task.end - task.start) / 2;
                        }
                        let left = self.nodes.len();
                        let right = left.checked_add(1).ok_or(TraceError::Internal("node count"))?;
                        if self.nodes.capacity() - self.nodes.len() < 2 || self.pending.capacity() - self.pending.len() < 2 {
                            return Err(TraceError::Internal("reserved BVH capacity"));
                        }
                        self.nodes.push(Node::pending());
                        self.nodes.push(Node::pending());
                        let parent = self.nodes.get_mut(task.node).ok_or(TraceError::Internal("build branch"))?;
                        *parent = Node { bounds, kind: Kind::Branch { left, right } };
                        self.pending.push(Task { node: right, start: split, end: task.end, depth: task.depth + 1 });
                        self.pending.push(Task { node: left, start: task.start, end: split, depth: task.depth + 1 });
                        State::Next
                    }
                }
                State::Done => State::Done,
            };
        }
        check_cancel(cancel)?;
        Ok(BuildProgress { complete: matches!(self.state, State::Done), work_used: self.work.used() })
    }

    pub fn finish(self) -> Result<Bvh> {
        if !matches!(self.state, State::Done) {
            return Err(TraceError::BuildIncomplete);
        }
        Ok(Bvh { triangles: self.triangles, order: self.order, nodes: self.nodes })
    }
}

/// Completed owned geometry; no shared cache, host worker or device state.
pub struct Bvh {
    triangles: Vec<Triangle>,
    order: Vec<usize>,
    nodes: Vec<Node>,
}
impl Bvh {
    pub fn primitive_count(&self) -> usize {
        self.triangles.len()
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn retained_bytes(&self) -> Result<u64> {
        // Conservatively retains the builder's fixed overhead allowance as well.
        memory(self.triangles.capacity(), self.order.capacity(), self.nodes.capacity(), 0)
    }
    pub const fn traversal_scratch_bytes() -> usize {
        size_of::<[usize; STACK_SLOTS]>()
    }

    pub fn closest_hit(&self, ray: &Ray, work: &mut WorkBudget, cancel: &CancelToken) -> Result<Option<Hit>> {
        self.trace(ray, false, work, cancel)
    }
    pub fn any_hit(&self, ray: &Ray, work: &mut WorkBudget, cancel: &CancelToken) -> Result<bool> {
        self.trace(ray, true, work, cancel).map(|hit| hit.is_some())
    }

    fn trace(&self, ray: &Ray, any: bool, work: &mut WorkBudget, cancel: &CancelToken) -> Result<Option<Hit>> {
        check_cancel(cancel)?;
        if self.nodes.is_empty() {
            return Ok(None);
        }
        let mut stack = [0_usize; STACK_SLOTS];
        let mut length = 1;
        let mut best: Option<Hit> = None;
        while length != 0 {
            work.tick(cancel)?;
            length -= 1;
            let node_index = *stack.get(length).ok_or(TraceError::Internal("traversal pop"))?;
            let node = self.nodes.get(node_index).ok_or(TraceError::Internal("traversal node"))?;
            let maximum = best.map_or(ray.max, |hit| hit.geometry.distance);
            if node.bounds.clip(ray, maximum).is_none() {
                continue;
            }
            match node.kind {
                Kind::Branch { left, right } => {
                    for child in [right, left] {
                        *stack.get_mut(length).ok_or(TraceError::Internal("bounded traversal stack"))? = child;
                        length += 1;
                    }
                }
                Kind::Leaf { start, end } => {
                    for ordinal in start..end {
                        work.tick(cancel)?;
                        let index = *self.order.get(ordinal).ok_or(TraceError::Internal("traversal ordinal"))?;
                        let triangle = self.triangles.get(index).ok_or(TraceError::Internal("traversal primitive"))?;
                        if let Some(geometry) = triangle.intersect(ray) {
                            let hit = Hit { primitive_index: index, geometry };
                            if any {
                                check_cancel(cancel)?;
                                return Ok(Some(hit));
                            }
                            if best.is_none_or(|old| {
                                geometry.distance < old.geometry.distance
                                    || (geometry.distance == old.geometry.distance && index < old.primitive_index)
                            }) {
                                best = Some(hit);
                            }
                        }
                    }
                }
            }
        }
        check_cancel(cancel)?;
        Ok(best)
    }
}
/// Resumable traversal used by image jobs. Each step visits one node or primitive.
/// The fixed stack is included in image admission; no per-ray heap allocation.
pub(crate) struct TraceCursor {
    ray: Ray,
    any: bool,
    stack: [usize; STACK_SLOTS],
    length: usize,
    leaf: Option<(usize, usize)>,
    best: Option<Hit>,
    done: bool,
}
impl TraceCursor {
    pub(crate) fn new(ray: Ray, any: bool) -> Self {
        Self { ray, any, stack: [0; STACK_SLOTS], length: 1, leaf: None, best: None, done: false }
    }
    pub(crate) fn step(&mut self, bvh: &Bvh, work: &mut WorkBudget, cancel: &CancelToken) -> Result<Option<Option<Hit>>> {
        work.tick(cancel)?;
        if self.done {
            return Err(TraceError::Internal("completed traversal cursor"));
        }
        if bvh.nodes.is_empty() {
            self.done = true;
            return Ok(Some(None));
        }
        if let Some((ordinal, end)) = self.leaf {
            if ordinal < end {
                self.leaf = Some((ordinal + 1, end));
                let index = *bvh.order.get(ordinal).ok_or(TraceError::Internal("cursor ordinal"))?;
                let triangle = bvh.triangles.get(index).ok_or(TraceError::Internal("cursor primitive"))?;
                if let Some(geometry) = triangle.intersect(&self.ray) {
                    let hit = Hit { primitive_index: index, geometry };
                    if self.any {
                        self.done = true;
                        return Ok(Some(Some(hit)));
                    }
                    if self.best.is_none_or(|old| {
                        geometry.distance < old.geometry.distance || (geometry.distance == old.geometry.distance && index < old.primitive_index)
                    }) {
                        self.best = Some(hit);
                    }
                }
                return Ok(None);
            }
            self.leaf = None;
        }
        if self.length == 0 {
            self.done = true;
            return Ok(Some(self.best));
        }
        self.length -= 1;
        let node_index = *self.stack.get(self.length).ok_or(TraceError::Internal("cursor pop"))?;
        let node = bvh.nodes.get(node_index).ok_or(TraceError::Internal("cursor node"))?;
        if node.bounds.clip(&self.ray, self.best.map_or(self.ray.max, |hit| hit.geometry.distance)).is_some() {
            match node.kind {
                Kind::Leaf { start, end } => self.leaf = Some((start, end)),
                Kind::Branch { left, right } => {
                    for child in [right, left] {
                        *self.stack.get_mut(self.length).ok_or(TraceError::Internal("cursor stack"))? = child;
                        self.length += 1;
                    }
                }
            }
        }
        Ok(None)
    }
}
