use std::sync::Arc;

use super::{MediumWeld, Triangle, flatten, medium_preparation_bytes};
use solidcraft_geom::{Mesh, Vec3};
use solidcraft_render::{ProductMesh, ProductScene, job::CancelToken, surface::OpaqueSurface, transmission::Transmission};

fn seam_tetra(offset: Vec3) -> ProductScene {
    // Original outward tetrahedron. Only one face has a duplicated CAD seam
    // endpoint; its measured displacement comes from our revolved tumbler.
    let origin = Vec3::new(-81.00000000023796, -51.6269330409162, 592.0);
    let mesh = Mesh {
        positions: vec![origin, origin + Vec3::X, origin + Vec3::Y, origin + Vec3::Z, origin + offset],
        normals: vec![Vec3::Z; 5],
        triangles: vec![[4, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
        tri_face: vec![0; 4],
        ..Default::default()
    };
    let bounds = mesh.bounds();
    ProductScene {
        meshes: vec![ProductMesh {
            name: "original seam tetrahedron".into(),
            mesh: Arc::new(mesh),
            color: [255; 3],
            opacity: 1.0,
            surface: Some(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap()),
            transmission: Some(Transmission::new(Vec3::ZERO, None).unwrap()),
            faces: Vec::new(),
        }],
        bounds,
    }
}

#[test]
fn closed_cad_seam_across_rounding_cells_keeps_one_nanometre_tolerance() {
    let delta = Vec3::new(-0.00000054155099, 0.00000031266462, 0.0);
    assert!(delta.len() < 1e-6);
    let scene = seam_tetra(delta);
    assert!(flatten(&scene, Vec3::ZERO, &CancelToken::default()).is_ok());
    let open = seam_tetra(Vec3::new(1.01e-6, 0.0, 0.0));
    assert!(flatten(&open, Vec3::ZERO, &CancelToken::default()).is_err());
}

#[test]
fn spatial_weld_is_deterministic_without_transitive_or_diagonal_expansion() {
    for _ in 0..8 {
        let mut weld = MediumWeld::new(8).unwrap();
        let a = weld.key(Vec3::ZERO).unwrap();
        assert_eq!(a, weld.key(Vec3::new(0.75e-6, 0.0, 0.0)).unwrap());
        assert_ne!(a, weld.key(Vec3::new(1.5e-6, 0.0, 0.0)).unwrap());
        assert_ne!(a, weld.key(Vec3::new(0.9e-6, 0.9e-6, 0.9e-6)).unwrap());
        assert!(weld.key(Vec3::new(f64::NAN, 0.0, 0.0)).is_err());
    }
}

#[test]
fn spatial_weld_preserves_winding_and_connected_component_checks() {
    let mut inward = seam_tetra(Vec3::ZERO);
    for tri in &mut Arc::make_mut(&mut inward.meshes[0].mesh).triangles {
        tri.swap(1, 2);
    }
    assert!(flatten(&inward, Vec3::ZERO, &CancelToken::default()).is_err());

    let mut disconnected = seam_tetra(Vec3::ZERO);
    let mesh = Arc::make_mut(&mut disconnected.meshes[0].mesh);
    let second = mesh.clone();
    mesh.positions.extend(second.positions.iter().map(|p| *p + Vec3::new(4.0, 0.0, 0.0)));
    mesh.normals.extend(&second.normals);
    mesh.triangles.extend(second.triangles.iter().map(|t| t.map(|i| i + 5)));
    mesh.tri_face.extend(&second.tri_face);
    let error = flatten(&disconnected, Vec3::ZERO, &CancelToken::default()).err().unwrap();
    assert!(error.to_string().contains("one connected shell"), "{error}");
}

#[test]
fn weld_scratch_only_counts_the_largest_closed_medium() {
    let glass = seam_tetra(Vec3::ZERO);
    let triangle_bytes = 4 * std::mem::size_of::<Triangle>() as u64;
    let mut opaque = glass.clone();
    opaque.meshes[0].transmission = None;
    assert_eq!(medium_preparation_bytes(&opaque), triangle_bytes);
    assert!(medium_preparation_bytes(&opaque) <= 4 * 400);
    opaque.meshes[0].transmission = Some(Transmission::new(Vec3::ZERO, Some(1.0)).unwrap());
    assert_eq!(medium_preparation_bytes(&opaque), triangle_bytes);
    let mut two = glass.clone();
    two.meshes.push(glass.meshes[0].clone());
    assert_eq!(medium_preparation_bytes(&two), medium_preparation_bytes(&glass) + triangle_bytes);
}
