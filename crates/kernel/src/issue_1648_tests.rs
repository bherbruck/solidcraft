//! #1648: a face pinched to a point, where its boundary touches itself within the tolerance (a
//! sphere touching a box's edge leaves one on the box), meshes at every tolerance.

use solidcraft_geom::Vec3;
use truck_meshalgo::prelude::*;
use truck_modeling as mt;

use crate::body::p3;
use crate::{BoolOp, build::box_solid, build::sphere, ops::boolean};

/// Twice the signed area of a closed polyline in the XY plane.
fn area2(points: &[mt::Point3]) -> f64 {
    points.iter().zip(points.iter().cycle().skip(1)).map(|(p, q)| p.x * q.y - q.x * p.y).sum()
}

/// A face in the XY plane between y = 0 and an arch from (5, h) to (0, h) whose middle dips
/// 2.8e-7 below y = 0, as the intersection curve does in the union of #1648. Its loop starts
/// along the bottom, so the bottom goes into the triangulation before the arch.
#[test]
fn a_face_touching_its_own_edge_meshes() {
    let (h, dip) = (0.3175, 2.8e-7);
    let v = |x: f64, y: f64| mt::builder::vertex(p3(Vec3::new(x, y, 0.0)));
    let (lb, rb, rt, lt) = (v(0.0, 0.0), v(5.0, 0.0), v(5.0, h), v(0.0, h));
    let control = vec![p3(Vec3::new(5.0, h, 0.0)), p3(Vec3::new(2.5, -h - 2.0 * dip, 0.0)), p3(Vec3::new(0.0, h, 0.0))];
    let arch = mt::BSplineCurve::new(mt::KnotVec::bezier_knot(2), control);
    let arch = mt::Edge::new(&rt, &lt, mt::Curve::BSplineCurve(arch));
    let wire: mt::Wire = vec![mt::builder::line(&lb, &rb), mt::builder::line(&rb, &rt), arch, mt::builder::line(&lt, &lb)].into();
    let plane = mt::Plane::new(p3(Vec3::ZERO), p3(Vec3::X), p3(Vec3::Y));
    let face = mt::Face::new(vec![wire], mt::Surface::Plane(plane));
    let shell: mt::Shell = vec![face].into();
    for tol in [0.3, 0.08, 0.02, 0.005] {
        let meshed = shell.robust_triangulation_within(Deflection { rulings: true, ..Deflection::chord(tol) });
        let face = meshed.face_iter().next().unwrap();
        let boundary: Vec<mt::Point3> = face.boundaries()[0].edge_iter().flat_map(|e| e.oriented_curve().0.into_iter().skip(1)).collect();
        let mesh = face.surface().unwrap();
        let area: f64 = mesh
            .faces()
            .triangle_iter()
            .map(|t| {
                let [a, b, c] = [0, 1, 2].map(|i| mesh.positions()[t[i].pos]);
                (b - a).cross(c - a).magnitude() / 2.0
            })
            .sum();
        // The triangles cover the face's boundary polygon, the parts on both sides of the pinch.
        let polygon = area2(&boundary).abs() / 2.0;
        assert!((area - polygon).abs() < 1e-5, "at {tol}: triangles cover {area}, the boundary {polygon}");
    }
}

/// The union of #1648: the sphere touches the box's bottom edge at (2.5, 7.5, 17.5).
#[test]
fn sphere_touching_a_box_edge_unites_soundly() {
    let a = sphere(Vec3::new(2.5, 15.0, 27.5), 12.5).unwrap();
    let b = box_solid(Vec3::new(0.0, 0.0, 17.5), Vec3::new(5.0, 7.5, 22.5)).unwrap();
    let union = boolean(&a, &b, BoolOp::Union).unwrap().unwrap();
    assert!(union.validity().is_empty(), "{:?}", union.validity());
    let size = union.size();
    for tol in [size * 1e-2, size * 2.5e-3, size * 6e-4] {
        let faces: std::collections::HashSet<u32> = union.tessellate(tol).unwrap().tri_face.into_iter().collect();
        assert_eq!(faces.len(), union.face_count(), "at {tol}");
    }
}
