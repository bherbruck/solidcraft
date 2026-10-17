//! Original median-split BVH. World geometry is rebased around the camera target for f32.
#[path = "planar_patches.rs"]
mod planar_patches;
use std::collections::HashMap;

use solidcraft_geom::Vec3;
use solidcraft_render::{
    ProductScene,
    job::{CancelToken, ErrorCode, JobError, Result},
    surface::{OpaqueSurface, srgb8_to_linear},
    transmission::Transmission,
};

pub(super) struct Triangle {
    pub p: [Vec3; 3],
    pub indices: [u32; 3],
    pub surface: OpaqueSurface,
    pub transmission: Option<Transmission>,
    pub medium: u32,
}
type MediumEdge = (usize, usize, i32);
pub(super) fn medium_preparation_bytes(scene: &ProductScene) -> u64 {
    // Validation consumes one body's scratch at a time. Opaque/thin-sheet bodies
    // allocate none of the weld table: do not charge their triangles for it.
    let closed = |t: Option<Transmission>| t.is_some_and(|v| v.thin_wall_thickness_mm().is_none());
    let mut total = 0u64;
    let mut largest_medium = 0u64;
    for body in &scene.meshes {
        let triangles = body.mesh.triangles.len() as u64;
        total = total.saturating_add(triangles);
        if closed(body.transmission) || body.faces.iter().any(|face| closed(face.transmission)) {
            largest_medium = largest_medium.max(triangles);
        }
    }
    // Three representatives per triangle at most. The 320 includes positions,
    // full eight-slot hash entries, control bytes and worst-case table rounding.
    let scratch = 3 * (std::mem::size_of::<MediumEdge>() + 320) + std::mem::size_of::<usize>();
    total.saturating_mul(std::mem::size_of::<Triangle>() as u64).saturating_add(largest_medium.saturating_mul(scratch as u64))
}

pub(super) fn planar_preparation_bytes_per_triangle() -> usize {
    let common = std::mem::size_of::<Triangle>() + 2 * std::mem::size_of::<Node>();
    (common + planar_patches::temporary_bytes_per_triangle()).max(common + 84 + 64 + 4)
}

impl Triangle {
    fn centre(&self, axis: usize) -> f64 {
        self.p.iter().map(|p| [p.x, p.y, p.z].get(axis).copied().unwrap_or(0.0)).sum::<f64>() / 3.0
    }
}

#[derive(Default)]
struct Node {
    lo: [f32; 3],
    hi: [f32; 3],
    first: u32,
    count: u32,
}

pub(super) struct Upload {
    pub geometry: Vec<u8>,
    pub nodes: Vec<u8>,
    pub epsilon: f32,
}

fn invalid(message: &str) -> JobError {
    JobError::new(ErrorCode::InvalidScene, message)
}
pub(super) fn reserved<T>(count: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(count).map_err(|_| JobError::new(ErrorCode::AllocationFailed, "path tracer allocation failed"))?;
    Ok(v)
}

pub(super) fn flatten(scene: &ProductScene, origin: Vec3, cancel: &CancelToken) -> Result<Upload> {
    let count = scene.meshes.iter().map(|m| m.mesh.triangles.len()).sum::<usize>();
    let mut triangles = reserved(count)?;
    let vertex_count = scene.meshes.iter().map(|m| m.mesh.positions.len()).sum::<usize>();
    let mut vertices = reserved(vertex_count * 32)?;
    let mut vertex_offset = 0u32;
    let mut media_bounds = reserved::<(Vec3, Vec3, bool)>(8)?;
    for (body_index, body) in scene.meshes.iter().enumerate() {
        cancel.check()?;
        let body_start = triangles.len();
        let mut faces = reserved(body.faces.len())?;
        faces.extend(body.faces.iter());
        faces.sort_unstable_by_key(|f| f.face);
        if faces.windows(2).any(|f| f[0].face == f[1].face) {
            return Err(invalid("duplicate face appearance"));
        }
        let mesh = &body.mesh;
        if mesh.normals.len() != mesh.positions.len() || mesh.tri_face.len() != mesh.triangles.len() {
            return Err(invalid("path tracer requires normals and a face ID for every triangle"));
        }
        for (index, (position, normal)) in mesh.positions.iter().zip(&mesh.normals).enumerate() {
            if index % 1024 == 0 {
                cancel.check()?;
            }
            let position = *position - origin;
            if !position.is_finite() || [position.x, position.y, position.z].iter().any(|x| x.abs() > 1e7) {
                return Err(invalid("path geometry must be finite and within 1e7 mm of the camera target"));
            }
            let normal = solidcraft_render::surface::unit_direction(*normal).map_err(|_| invalid("invalid mesh normal"))?;
            vector(&mut vertices, position, 0.0);
            vector(&mut vertices, normal, 0.0);
        }
        for (index, indices) in mesh.triangles.iter().enumerate() {
            if index % 1024 == 0 {
                cancel.check()?;
            }
            let face = mesh.tri_face.get(index).copied().ok_or_else(|| invalid("missing triangle face"))?;
            let look = faces.binary_search_by_key(&face, |f| f.face).ok().and_then(|i| faces.get(i));
            let opacity = look.map_or(body.opacity, |f| f.opacity);
            let transmission = look.map_or(body.transmission, |f| f.transmission);
            if opacity != 1.0 && transmission.is_none() {
                return Err(invalid("gpu-path requires explicit transmission for nonopaque preview appearances"));
            }
            let color = look.map_or(body.color, |f| f.color);
            let surface = look
                .map_or(body.surface, |f| f.surface)
                .map(Ok)
                .unwrap_or_else(|| {
                    OpaqueSurface::new(Vec3::new(srgb8_to_linear(color[0]), srgb8_to_linear(color[1]), srgb8_to_linear(color[2])), 0.0, 0.5, 1.5)
                })
                .map_err(|e| invalid(&e.to_string()))?;
            let mut p = [Vec3::ZERO; 3];
            let mut global_indices = [0u32; 3];
            for (corner, &vertex) in indices.iter().enumerate() {
                let position = *mesh.positions.get(vertex as usize).ok_or_else(|| invalid("triangle index out of bounds"))? - origin;
                if !position.is_finite() || [position.x, position.y, position.z].iter().any(|x| x.abs() > 1e7) {
                    return Err(invalid("path geometry must be finite and within 1e7 mm of the camera target"));
                }
                if let Some(v) = p.get_mut(corner) {
                    *v = position;
                }
                if let Some(v) = global_indices.get_mut(corner) {
                    *v = vertex.checked_add(vertex_offset).ok_or_else(|| invalid("vertex offset overflow"))?;
                }
            }
            if (p[1] - p[0]).cross(p[2] - p[0]).len() <= 1e-12 {
                continue;
            }
            if transmission.is_some() && surface.clear_coat().is_some() {
                return Err(invalid("clear coat and transmission cannot share a surface"));
            }
            if transmission.is_some() && surface.metallic() != 0.0 {
                return Err(invalid("transmission requires a nonmetallic surface"));
            }
            triangles.push(Triangle { p, indices: global_indices, surface, transmission, medium: body_index as u32 + 1 });
        }
        validate_medium(&triangles[body_start..], cancel)?;
        let body_triangles = &triangles[body_start..];
        if body_triangles.iter().any(|t| t.transmission.is_some()) {
            if media_bounds.len() >= 8 {
                return Err(invalid("at most eight dielectric bodies are supported"));
            }
            let closed = body_triangles.iter().any(|t| t.transmission.is_some_and(|v| v.thin_wall_thickness_mm().is_none()));
            let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
            let mut hi = -lo;
            for t in body_triangles {
                for p in t.p {
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
            for (other_lo, other_hi, other_closed) in &media_bounds {
                if (closed || *other_closed)
                    && !(hi.x < other_lo.x || lo.x > other_hi.x || hi.y < other_lo.y || lo.y > other_hi.y || hi.z < other_lo.z || lo.z > other_hi.z)
                {
                    return Err(invalid("nested or overlapping dielectric bounds are unsupported; separate the bodies' bounds"));
                }
            }
            media_bounds.push((lo, hi, closed));
        }
        vertex_offset = vertex_offset.checked_add(mesh.positions.len() as u32).ok_or_else(|| invalid("vertex offset overflow"))?;
    }
    let mut nodes = reserved(triangles.len().saturating_mul(2).max(1))?;
    nodes.push(Node::default());
    build(&mut triangles, 0, &mut nodes, 0, cancel)?;
    let patches = planar_patches::ids(&triangles, cancel)?;
    let extent = nodes.first().map_or(1.0, |n| n.lo.iter().zip(n.hi).map(|(a, b)| b - a).fold(0.0f32, f32::max));
    let mut geometry = reserved(16 + triangles.len().max(1) * 84 + vertices.len())?;
    let mut materials = reserved(triangles.len().max(1) * 64)?;
    let vertex_word = 4 + triangles.len().max(1) * 4;
    let material_word = vertex_word + vertices.len() / 4;
    let patch_word = material_word + triangles.len().max(1) * 16;
    for word in [triangles.len() as u32, vertex_word as u32, material_word as u32, patch_word as u32] {
        geometry.extend_from_slice(&word.to_le_bytes());
    }
    for (material, t) in triangles.into_iter().enumerate() {
        if material % 1024 == 0 {
            cancel.check()?;
        }
        for index in t.indices {
            geometry.extend_from_slice(&index.to_le_bytes());
        }
        geometry.extend_from_slice(&(material as u32).to_le_bytes());
        vector(&mut materials, t.surface.base_color(), if t.transmission.is_some() { 1.0 } else { 0.0 });
        vector(
            &mut materials,
            Vec3::new(t.surface.metallic(), t.surface.roughness(), t.surface.dielectric_ior()),
            t.transmission.and_then(|t| t.thin_wall_thickness_mm()).unwrap_or(-1.0) as f32,
        );
        vector(&mut materials, t.transmission.map_or(Vec3::ZERO, |t| t.extinction()), t.medium as f32);
        let coat = t.surface.clear_coat().map_or(Vec3::new(0.0, 0.5, 1.5), |c| Vec3::new(c.weight(), c.roughness(), c.ior()));
        vector(&mut materials, coat, if t.transmission.is_some_and(|t| t.shadow_transmission()) { 1.0 } else { 0.0 });
    }
    if geometry.len() == 16 {
        geometry.resize(32, 0);
        materials.resize(64, 0);
    }
    geometry.extend_from_slice(&vertices);
    drop(vertices);
    geometry.extend_from_slice(&materials);
    drop(materials);
    if patches.is_empty() {
        geometry.extend_from_slice(&0u32.to_le_bytes());
    } else {
        for (index, patch) in patches.into_iter().enumerate() {
            if index % 1024 == 0 {
                cancel.check()?;
            }
            geometry.extend_from_slice(&patch.to_le_bytes());
        }
    }
    let mut node_bytes = reserved(nodes.len() * 32)?;
    for node in nodes {
        for x in node.lo {
            node_bytes.extend_from_slice(&x.to_le_bytes());
        }
        node_bytes.extend_from_slice(&node.first.to_le_bytes());
        for x in node.hi {
            node_bytes.extend_from_slice(&x.to_le_bytes());
        }
        node_bytes.extend_from_slice(&node.count.to_le_bytes());
    }
    Ok(Upload { geometry, nodes: node_bytes, epsilon: (extent * 1e-7).max(1e-5) })
}

const MEDIUM_WELD_MM: f64 = 1e-6;

#[derive(Default)]
struct MediumBucket {
    representatives: [usize; 8],
    len: usize,
}

/// Only topology uses these representatives; every uploaded vertex stays unchanged.
/// Checking neighboring cells avoids rounding-boundary cracks in curved CAD seams.
struct MediumWeld {
    cells: HashMap<[i64; 3], MediumBucket>,
    positions: Vec<Vec3>,
    limit: usize,
}

impl MediumWeld {
    fn new(limit: usize) -> Result<Self> {
        let mut cells = HashMap::new();
        cells.try_reserve(limit).map_err(|_| JobError::new(ErrorCode::AllocationFailed, "glass topology allocation failed"))?;
        Ok(Self { cells, positions: reserved(limit)?, limit })
    }

    fn key(&mut self, point: Vec3) -> Result<usize> {
        let xyz = [point.x, point.y, point.z];
        // Leave room for neighboring cells and refuse saturated integer casts.
        if !xyz.into_iter().all(|v| v.is_finite() && v.abs() <= 1e9) {
            return Err(invalid("glass topology position exceeds finite coordinate bounds"));
        }
        let cell = xyz.map(|v| (v / MEDIUM_WELD_MM).floor() as i64);
        let mut found: Option<usize> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let neighbor = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                    let Some(bucket) = self.cells.get(&neighbor) else { continue };
                    for &index in bucket.representatives.iter().take(bucket.len) {
                        let other = self.positions.get(index).ok_or_else(|| invalid("missing glass topology representative"))?;
                        let delta = point - *other;
                        if delta.dot(delta) <= MEDIUM_WELD_MM * MEDIUM_WELD_MM && found.is_none_or(|old| index < old) {
                            found = Some(index);
                        }
                    }
                }
            }
        }
        if let Some(index) = found {
            return Ok(index);
        }
        if self.positions.len() >= self.limit {
            return Err(invalid("glass topology representative budget exceeded"));
        }
        let index = self.positions.len();
        let bucket = self.cells.entry(cell).or_default();
        // Split a unit cell into eight half-size cubes: points sharing one are
        // at most sqrt(3)/2 tolerances apart, so a cell has at most eight reps.
        let slot = bucket.representatives.get_mut(bucket.len).ok_or_else(|| invalid("glass topology cell budget exceeded"))?;
        *slot = index;
        bucket.len += 1;
        self.positions.push(point);
        Ok(index)
    }
}

// Volume transmission needs a consistently oriented closed two-manifold. CAD
// tessellation duplicates face-seam vertices; compare their actual Euclidean
// separation at the existing one-nanometre-in-mm tolerance, without modifying
// uploaded geometry or merging chains beyond a representative's tolerance.
fn validate_medium(triangles: &[Triangle], cancel: &CancelToken) -> Result<()> {
    let Some(first) = triangles.iter().find(|t| t.transmission.is_some_and(|v| v.thin_wall_thickness_mm().is_none())) else {
        return Ok(());
    };
    if triangles.iter().any(|t| t.transmission != first.transmission || t.surface.dielectric_ior() != first.surface.dielectric_ior()) {
        return Err(invalid("a closed glass body must use one homogeneous transmission medium; use separate bodies or thin walls"));
    }
    let vertex_count = triangles.len().checked_mul(3).ok_or_else(|| invalid("glass topology vertex count overflow"))?;
    let mut edges = reserved::<MediumEdge>(vertex_count)?;
    let mut weld = MediumWeld::new(vertex_count)?;
    let mut parents = reserved::<usize>(triangles.len())?;
    parents.extend(0..triangles.len());
    let origin = first.p[0];
    let mut six_volume = 0.0;
    for (index, triangle) in triangles.iter().enumerate() {
        if index % 1024 == 0 {
            cancel.check()?;
        }
        let keys = [weld.key(triangle.p[0])?, weld.key(triangle.p[1])?, weld.key(triangle.p[2])?];
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            if keys[a] == keys[b] {
                return Err(invalid("glass triangle collapses at the 1e-6 mm topology tolerance"));
            }
            let ordinal = i32::try_from(index + 1).map_err(|_| invalid("too many glass triangles"))?;
            edges.push(if keys[a] < keys[b] { (keys[a], keys[b], ordinal) } else { (keys[b], keys[a], -ordinal) });
        }
        six_volume += (triangle.p[0] - origin).dot((triangle.p[1] - origin).cross(triangle.p[2] - origin));
    }
    edges.sort_unstable();
    for edge in edges.chunks(2) {
        let [a, b] = edge else {
            return Err(invalid("volume glass requires a closed mesh; use thin_wall_thickness_mm for sheets"));
        };
        if a.0 != b.0 || a.1 != b.1 || a.2.signum() + b.2.signum() != 0 {
            return Err(invalid("volume glass requires a closed, consistently outward-oriented mesh"));
        }
        let first = medium_root(&mut parents, a.2.unsigned_abs() as usize - 1)?;
        let second = medium_root(&mut parents, b.2.unsigned_abs() as usize - 1)?;
        *parents.get_mut(first).ok_or_else(|| invalid("invalid glass component"))? = second;
    }
    // Groups with four or more coincident directed edges cannot be a two-manifold.
    if edges.windows(3).any(|e| e[0].0 == e[2].0 && e[0].1 == e[2].1) {
        return Err(invalid("nonmanifold glass edge"));
    }
    let component = medium_root(&mut parents, 0)?;
    for index in 0..parents.len() {
        if index % 1024 == 0 {
            cancel.check()?;
        }
        if medium_root(&mut parents, index)? != component {
            return Err(invalid("closed dielectric body must contain exactly one connected shell"));
        }
    }
    if six_volume <= 0.0 || !six_volume.is_finite() {
        return Err(invalid("glass mesh must enclose positive outward-oriented volume"));
    }
    cancel.check()
}

fn medium_root(parents: &mut [usize], mut index: usize) -> Result<usize> {
    loop {
        let parent = *parents.get(index).ok_or_else(|| invalid("invalid glass component index"))?;
        if parent == index {
            return Ok(index);
        }
        let grandparent = *parents.get(parent).ok_or_else(|| invalid("invalid glass component parent"))?;
        *parents.get_mut(index).ok_or_else(|| invalid("invalid glass component index"))? = grandparent;
        index = grandparent;
    }
}

fn build(triangles: &mut [Triangle], offset: usize, nodes: &mut Vec<Node>, index: usize, cancel: &CancelToken) -> Result<()> {
    cancel.check()?;
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for t in triangles.iter() {
        for p in t.p {
            for ((low, high), value) in lo.iter_mut().zip(hi.iter_mut()).zip([p.x as f32, p.y as f32, p.z as f32]) {
                *low = low.min(value);
                *high = high.max(value);
            }
        }
    }
    let mut node = Node { lo, hi, first: offset as u32, count: triangles.len() as u32 };
    if triangles.is_empty() {
        node = Node { first: u32::MAX, ..Default::default() };
    }
    if triangles.len() > 4 {
        let axis = (0..3).max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b]))).unwrap_or(0);
        let middle = triangles.len() / 2;
        triangles.select_nth_unstable_by(middle, |a, b| a.centre(axis).total_cmp(&b.centre(axis)));
        let first = nodes.len();
        nodes.push(Node::default());
        nodes.push(Node::default());
        node.first = first as u32;
        node.count = 0;
        let (left, right) = triangles.split_at_mut(middle);
        build(left, offset, nodes, first, cancel)?;
        build(right, offset + middle, nodes, first + 1, cancel)?;
    }
    *nodes.get_mut(index).ok_or_else(|| invalid("missing BVH node"))? = node;
    Ok(())
}

pub(super) fn vector(bytes: &mut Vec<u8>, v: Vec3, w: f32) {
    for value in [v.x as f32, v.y as f32, v.z as f32, w] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

#[cfg(test)]
#[path = "scene_weld_tests.rs"]
mod weld_tests;
