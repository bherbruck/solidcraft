//! Original median-split BVH. World geometry is rebased around the camera target for f32.
use solidcraft_geom::Vec3;
use solidcraft_render::{
    ProductScene,
    job::{CancelToken, ErrorCode, JobError, Result},
    surface::{OpaqueSurface, srgb8_to_linear},
};

pub(super) struct Triangle {
    pub p: [Vec3; 3],
    pub indices: [u32; 3],
    pub surface: OpaqueSurface,
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
    for body in &scene.meshes {
        cancel.check()?;
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
            if opacity != 1.0 {
                return Err(invalid("gpu-path currently requires opaque surfaces; refraction is not implemented"));
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
            triangles.push(Triangle { p, indices: global_indices, surface });
        }
        vertex_offset = vertex_offset.checked_add(mesh.positions.len() as u32).ok_or_else(|| invalid("vertex offset overflow"))?;
    }
    let mut nodes = reserved(triangles.len().saturating_mul(2).max(1))?;
    nodes.push(Node::default());
    build(&mut triangles, 0, &mut nodes, 0, cancel)?;
    let extent = nodes.first().map_or(1.0, |n| n.lo.iter().zip(n.hi).map(|(a, b)| b - a).fold(0.0f32, f32::max));
    let mut geometry = reserved(16 + triangles.len().max(1) * 48 + vertices.len())?;
    let mut materials = reserved(triangles.len().max(1) * 32)?;
    let vertex_word = 4 + triangles.len().max(1) * 4;
    let material_word = vertex_word + vertices.len() / 4;
    for word in [triangles.len() as u32, vertex_word as u32, material_word as u32, vertex_count as u32] {
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
        vector(&mut materials, t.surface.base_color(), 0.0);
        vector(&mut materials, Vec3::new(t.surface.metallic(), t.surface.roughness(), t.surface.dielectric_ior()), 0.0);
    }
    if geometry.len() == 16 {
        geometry.resize(32, 0);
        materials.resize(32, 0);
    }
    geometry.extend_from_slice(&vertices);
    drop(vertices);
    geometry.extend_from_slice(&materials);
    drop(materials);
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
