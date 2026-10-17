//! Original analytic surfaces exercising trim projection (#1974), without an external CAD fixture.
use truck_meshalgo::prelude::*;
use truck_modeling::{self as mt, BoundedCurve, InnerSpace};

fn mesh(surface: mt::Surface, curves: Vec<mt::Curve>) -> PolygonMesh {
    let vertices: Vec<_> = curves.iter().map(|c| mt::Vertex::new(c.front())).collect();
    let count = vertices.len();
    let edges: Vec<_> = curves.into_iter().enumerate().map(|(i, c)| mt::Edge::new_unchecked(&vertices[i], &vertices[(i + 1) % count], c)).collect();
    let face = mt::Face::new_unchecked(vec![edges.into()], surface);
    let shell = mt::Shell::from(vec![face]).robust_triangulation(0.001);
    let polygon = shell.face_iter().next().unwrap().surface().unwrap();
    assert!(polygon.faces().triangle_iter().next().is_some(), "the trimmed face must remain visible");
    polygon
}

#[test]
fn approximate_boundary_does_not_escape_a_bounded_spline() {
    // S(u,v) = (50u + 10v, .001v, 10u² + 4v), 0 <= u,v <= 1.
    // Near u=1 its tangents are almost parallel. A slightly approximate top edge
    // sends an undamped search hinted from the preceding point to u>1.
    let p = mt::Point3::new;
    let surface = mt::BSplineSurface::new(
        (mt::KnotVec::bezier_knot(2), mt::KnotVec::bezier_knot(1)),
        vec![vec![p(0., 0., 0.), p(10., 0.001, 4.)], vec![p(25., 0., 0.), p(35., 0.001, 4.)], vec![p(50., 0., 10.), p(60., 0.001, 14.)]],
    );
    let curves = vec![
        mt::BSplineCurve::new(mt::KnotVec::bezier_knot(2), vec![p(60., 0.001, 14.), p(35., 0., 4.), p(10., 0.001, 4.)]).into(),
        mt::Line(p(10., 0.001, 4.), p(0., 0., 0.)).into(),
        mt::BSplineCurve::new(mt::KnotVec::bezier_knot(2), vec![p(0., 0., 0.), p(25., 0., 0.), p(50., 0., 10.)]).into(),
        mt::Line(p(50., 0., 10.), p(60., 0.001, 14.)).into(),
    ];
    let mesh = mesh(mt::Surface::BSplineSurface(surface), curves);
    for uv in mesh.uv_coords() {
        assert!((-1e-6..=1. + 1e-6).contains(&uv.x) && (-1e-6..=1. + 1e-6).contains(&uv.y), "parameter {uv:?}");
    }
    assert!(mesh.positions().iter().all(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite()));
}

#[test]
fn a_trim_crossing_a_closed_nurbs_seam_is_a_small_cylinder_patch() {
    let (radius, angle, bottom, top) = (10.0_f64, 0.1_f64, 2.0, 5.0);
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let circle = [(1., 0., 1.), (1., 1., w), (0., 1., 1.), (-1., 1., w), (-1., 0., 1.), (-1., -1., w), (0., -1., 1.), (1., -1., w), (1., 0., 1.)];
    let ctrl = circle
        .iter()
        .map(|&(x, y, w)| vec![mt::Vector4::new(radius * x * w, radius * y * w, 0., w), mt::Vector4::new(radius * x * w, radius * y * w, 10. * w, w)])
        .collect();
    let surface = mt::NurbsSurface::new(mt::BSplineSurface::new(
        (mt::KnotVec::from(vec![0., 0., 0., 1., 1., 2., 2., 3., 3., 4., 4., 4.]), mt::KnotVec::bezier_knot(1)),
        ctrl,
    ));
    let at = |theta: f64, z| mt::Point3::new(radius * theta.cos(), radius * theta.sin(), z);
    let arc = |z, reverse| {
        let w = angle.cos();
        let mut ctrl = vec![at(-angle, z).to_homogeneous(), mt::Vector4::new(radius, 0., z * w, w), at(angle, z).to_homogeneous()];
        if reverse {
            ctrl.reverse();
        }
        mt::NurbsCurve::new(mt::BSplineCurve::new(mt::KnotVec::bezier_knot(2), ctrl)).into()
    };
    let curves = vec![
        arc(bottom, false),
        mt::Line(at(angle, bottom), at(angle, top)).into(),
        arc(top, true),
        mt::Line(at(-angle, top), at(-angle, bottom)).into(),
    ];
    let mesh = mesh(mt::Surface::NurbsSurface(surface), curves);
    for p in mesh.positions() {
        assert!(p.x >= radius * angle.cos() - 1e-6 && p.x <= radius + 1e-6, "{p:?}");
        assert!(p.y.abs() <= radius * angle.sin() + 1e-6 && (bottom - 1e-6..=top + 1e-6).contains(&p.z), "{p:?}");
    }
    let area: f64 = mesh
        .faces()
        .triangle_iter()
        .map(|t| {
            let [a, b, c] = [0, 1, 2].map(|i| mesh.positions()[t[i].pos]);
            (b - a).cross(c - a).magnitude() * 0.5
        })
        .sum();
    assert!((area - radius * 2.0 * angle * (top - bottom)).abs() < 0.01, "area {area}");
}

#[test]
fn an_affine_planes_unit_chart_does_not_clip_a_trim() {
    let p = mt::Point3::new;
    let points = [p(10., 20., 0.), p(13., 20., 0.), p(13., 22., 0.), p(10., 22., 0.)];
    let curves = (0..4).map(|i| mt::Line(points[i], points[(i + 1) % 4]).into()).collect();
    let mesh = mesh(mt::Plane::new(p(0., 0., 0.), p(1., 0., 0.), p(0., 1., 0.)).into(), curves);
    for p in mesh.positions() {
        assert!((10. - 1e-6..=13. + 1e-6).contains(&p.x) && (20. - 1e-6..=22. + 1e-6).contains(&p.y), "{p:?}");
    }
}
