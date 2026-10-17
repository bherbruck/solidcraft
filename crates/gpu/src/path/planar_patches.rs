//! Exact connected planar patches of the positions actually uploaded to the GPU.
//! This is a source-plane identity, not an angular or distance tolerance. A ray
//! leaving a plane cannot meet that same plane again at a positive distance.
use super::{Triangle, invalid, medium_root, reserved};
use robust::{Coord, Coord3D, orient2d, orient3d};
use solidcraft_render::job::{CancelToken, Result};

#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct Edge {
    body: u32,
    a: [u32; 3],
    b: [u32; 3],
    triangle: usize,
}

pub(super) fn temporary_bytes_per_triangle() -> usize {
    3 * std::mem::size_of::<Edge>() + std::mem::size_of::<usize>() + std::mem::size_of::<u32>()
}

fn uploaded(t: &Triangle) -> [Coord3D<f64>; 3] {
    t.p.map(|p| Coord3D { x: f64::from(p.x as f32), y: f64::from(p.y as f32), z: f64::from(p.z as f32) })
}

fn key(p: Coord3D<f64>) -> [u32; 3] {
    [p.x, p.y, p.z].map(|x| if x == 0.0 { 0 } else { (x as f32).to_bits() })
}

fn nondegenerate(p: [Coord3D<f64>; 3]) -> bool {
    for axes in [(0, 1), (0, 2), (1, 2)] {
        let q = p.map(|v| {
            let values = [v.x, v.y, v.z];
            Coord { x: values[axes.0], y: values[axes.1] }
        });
        if orient2d(q[0], q[1], q[2]) != 0.0 {
            return true;
        }
    }
    false
}

pub(super) fn ids(triangles: &[Triangle], cancel: &CancelToken) -> Result<Vec<u32>> {
    cancel.check()?;
    if triangles.len() >= u32::MAX as usize {
        return Err(invalid("too many triangles for planar patch identities"));
    }
    let edge_count = triangles.len().checked_mul(3).ok_or_else(|| invalid("planar edge count overflow"))?;
    let mut edges = reserved::<Edge>(edge_count)?;
    let mut parents = reserved::<usize>(triangles.len())?;
    parents.extend(0..triangles.len());
    for (index, triangle) in triangles.iter().enumerate() {
        if index % 1024 == 0 {
            cancel.check()?;
        }
        let p = uploaded(triangle);
        // An f64 triangle can collapse at upload precision. It has no valid plane
        // and must never connect otherwise unrelated planes into one patch.
        if !nondegenerate(p) {
            continue;
        }
        let keys = p.map(key);
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (a, b) = if keys[a] <= keys[b] { (keys[a], keys[b]) } else { (keys[b], keys[a]) };
            edges.push(Edge { body: triangle.medium, a, b, triangle: index });
        }
    }
    cancel.check()?;
    edges.sort_unstable();
    cancel.check()?;
    let mut begin = 0;
    while begin < edges.len() {
        if begin % 1024 < 3 {
            cancel.check()?;
        }
        let first = edges.get(begin).ok_or_else(|| invalid("missing planar edge"))?;
        let mut end = begin + 1;
        while let Some(edge) = edges.get(end) {
            if (edge.body, edge.a, edge.b) != (first.body, first.a, first.b) {
                break;
            }
            if end % 1024 == 0 {
                cancel.check()?;
            }
            end += 1;
        }
        // Only unambiguous two-face adjacency is joined. At nonmanifold edges
        // leave identities separate rather than inferring a connected surface.
        if end - begin == 2 {
            let second = edges.get(begin + 1).ok_or_else(|| invalid("missing planar neighbor"))?;
            let a = uploaded(triangles.get(first.triangle).ok_or_else(|| invalid("missing planar triangle"))?);
            let b = uploaded(triangles.get(second.triangle).ok_or_else(|| invalid("missing planar triangle"))?);
            // Adaptive exact predicates on f32-as-f64 coordinates. Mirrored
            // winding does not change plane identity; curved facets do.
            if b.iter().all(|&p| orient3d(a[0], a[1], a[2], p) == 0.0) {
                let root_a = medium_root(&mut parents, first.triangle)?;
                let root_b = medium_root(&mut parents, second.triangle)?;
                let (keep, merge) = (root_a.min(root_b), root_a.max(root_b));
                *parents.get_mut(merge).ok_or_else(|| invalid("missing planar component"))? = keep;
            }
        }
        begin = end;
    }
    let mut result = reserved(triangles.len())?;
    for index in 0..triangles.len() {
        if index % 1024 == 0 {
            cancel.check()?;
        }
        result.push(medium_root(&mut parents, index)? as u32 + 1);
    }
    cancel.check()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcraft_geom::Vec3;
    use solidcraft_render::surface::OpaqueSurface;

    fn triangle(p: [[f64; 3]; 3], body: u32) -> Triangle {
        Triangle {
            p: p.map(|v| Vec3::new(v[0], v[1], v[2])),
            indices: [0, 1, 2],
            surface: OpaqueSurface::new(Vec3::ZERO, 0.0, 0.5, 1.5).unwrap(),
            transmission: None,
            medium: body,
        }
    }

    #[test]
    fn exact_connected_planes_join_with_either_winding() {
        let a = [191.3_f32 as f64, 79.2_f32 as f64, 84.7_f32 as f64];
        let b = [a[0] + 8.0, a[1], a[2] + 2.0];
        let c = [a[0] + 8.0, a[1] + 8.0, a[2] + 6.0];
        let d = [a[0], a[1] + 8.0, a[2] + 4.0];
        for neighbor in [[a, c, d], [d, c, a]] {
            let values = ids(&[triangle([a, b, c], 1), triangle(neighbor, 1)], &CancelToken::default()).unwrap();
            assert_eq!(values[0], values[1]);
        }
    }

    #[test]
    fn disconnected_parallel_bent_and_different_bodies_stay_separate() {
        let a = [0.0, 0.0, 0.0];
        let b = [1.0, 0.0, 0.0];
        let c = [1.0, 1.0, 0.0];
        let d = [0.0, 1.0, 0.0];
        let cases = [
            (triangle([a, c, d], 2), "different bodies"),
            (triangle([[2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 1.0, 0.0]], 1), "disconnected"),
            (triangle([a, c, [0.0, 1.0, 1e-20]], 1), "arbitrarily shallow bend"),
            (triangle([[0.0, 0.0, 1e-20], [1.0, 1.0, 1e-20], [0.0, 1.0, 1e-20]], 1), "near parallel"),
        ];
        for (neighbor, name) in cases {
            let values = ids(&[triangle([a, b, c], 1), neighbor], &CancelToken::default()).unwrap();
            assert_ne!(values[0], values[1], "{name}");
        }
    }

    #[test]
    fn grouping_uses_upload_precision_and_normalizes_signed_zero() {
        let values = ids(
            &[
                triangle([[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0]], 1),
                triangle([[-0.0, -0.0, 1.0 + 1e-10], [1.0, 1.0, 1.0 + 1e-10], [0.0, 1.0, 1.0]], 1),
            ],
            &CancelToken::default(),
        )
        .unwrap();
        assert_eq!(values[0], values[1]);
    }

    #[test]
    fn cancelled_and_empty_preparation_are_bounded() {
        let cancel = CancelToken::default();
        assert!(ids(&[], &cancel).unwrap().is_empty());
        cancel.cancel();
        assert!(ids(&[], &cancel).is_err());
        assert!(temporary_bytes_per_triangle() >= 3 * std::mem::size_of::<Edge>() + 12);
    }

    #[test]
    fn upload_collapse_never_bridges_planes() {
        let a = [1.0, 0.0, 0.0];
        let b = [1.0 + 1e-10, 0.0, 0.0];
        let c = [1.0, 1.0, 0.0];
        let values =
            ids(&[triangle([a, c, [2.0, 0.0, 0.0]], 1), triangle([a, b, c], 1), triangle([a, c, [1.0, 0.0, 1.0]], 1)], &CancelToken::default())
                .unwrap();
        assert_ne!(values[0], values[1]);
        assert_ne!(values[1], values[2]);
        assert_ne!(values[0], values[2]);
    }

    #[test]
    fn packing_offset_and_ids_follow_bvh_reordering() {
        use solidcraft_geom::Mesh;
        use solidcraft_render::{ProductMesh, ProductScene};
        use std::sync::Arc;
        let mut mesh = Mesh::default();
        for z in [5.0, 0.0, 10.0] {
            let first = mesh.positions.len() as u32;
            mesh.positions.extend([Vec3::new(0.0, 0.0, z), Vec3::new(1.0, 0.0, z), Vec3::new(1.0, 1.0, z), Vec3::new(0.0, 1.0, z)]);
            mesh.normals.extend([Vec3::Z; 4]);
            mesh.triangles.extend([[first, first + 1, first + 2], [first, first + 2, first + 3]]);
            mesh.tri_face.extend([0, 0]);
        }
        let bounds = mesh.bounds();
        let scene = ProductScene {
            bounds,
            meshes: vec![ProductMesh {
                name: "three separate planes".into(),
                mesh: Arc::new(mesh),
                color: [255; 3],
                opacity: 1.0,
                surface: None,
                transmission: None,
                faces: vec![],
            }],
        };
        let upload = super::super::flatten(&scene, Vec3::ZERO, &CancelToken::default()).unwrap();
        let word = |index: usize| u32::from_le_bytes(upload.geometry[index * 4..index * 4 + 4].try_into().unwrap()) as usize;
        assert_eq!(word(0), 6);
        assert_eq!(word(3), word(2) + 6 * 16);
        assert_eq!(upload.geometry.len(), 16 + 6 * 84 + 12 * 32);
        let mut pairs = Vec::new();
        for index in 0..6 {
            let vertex = word(4 + index * 4);
            let z = f32::from_bits(word(word(1) + vertex * 8 + 2) as u32);
            pairs.push((z, word(word(3) + index)));
        }
        for (a, pa) in &pairs {
            for (b, pb) in &pairs {
                assert_eq!(a == b, pa == pb);
            }
        }
        assert!(!upload.nodes.is_empty());
        assert!(upload.epsilon > 0.0);
    }
}
