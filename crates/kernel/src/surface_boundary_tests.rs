//! Exercise generated trim boundaries through the vendored mesher's public API.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use truck_meshalgo::prelude::{MeshableShape, MeshedShape, RobustMeshableShape};
use truck_modeling::{self as mt, ParametricSurface};

// A rational planar support x = u / (u - 1/2), y = v. The authored loop stays
// clear of the pole, but completing its parameter box crosses it. Count actual
// evaluations so the old exponential recursion fails promptly on every machine.
#[derive(Clone)]
struct SingularBoundary(Arc<AtomicUsize>);

impl ParametricSurface for SingularBoundary {
    type Point = mt::Point3;
    type Vector = mt::Vector3;

    fn subs(&self, u: f64, v: f64) -> Self::Point {
        assert!(self.0.fetch_add(1, Ordering::Relaxed) < 4_000_000, "surface boundary subdivision did not terminate");
        mt::Point3::new(u / (u - 0.5), v, 0.0)
    }
    fn uder(&self, u: f64, _: f64) -> Self::Vector {
        mt::Vector3::new(-0.5 / (u - 0.5).powi(2), 0.0, 0.0)
    }
    fn vder(&self, _: f64, _: f64) -> Self::Vector {
        mt::Vector3::new(0.0, 1.0, 0.0)
    }
    fn uuder(&self, u: f64, _: f64) -> Self::Vector {
        mt::Vector3::new(1.0 / (u - 0.5).powi(3), 0.0, 0.0)
    }
    fn uvder(&self, _: f64, _: f64) -> Self::Vector {
        mt::Vector3::new(0.0, 0.0, 0.0)
    }
    fn vvder(&self, _: f64, _: f64) -> Self::Vector {
        mt::Vector3::new(0.0, 0.0, 0.0)
    }
    fn parameter_range(&self) -> (mt::ParameterRange, mt::ParameterRange) {
        use std::ops::Bound::Included;
        ((Included(0.0), Included(1.0)), (Included(0.0), Included(1.0)))
    }
}

impl mt::ParametricSurface3D for SingularBoundary {
    fn normal(&self, _: f64, _: f64) -> mt::Vector3 {
        mt::Vector3::new(0.0, 0.0, -1.0)
    }
}

impl mt::ParameterDivision2D for SingularBoundary {
    fn parameter_division(&self, _: ((f64, f64), (f64, f64)), _: f64) -> (Vec<f64>, Vec<f64>) {
        panic!("meshing must use bounded surface division");
    }
}

impl mt::SearchParameter<mt::D2> for SingularBoundary {
    type Point = mt::Point3;

    fn search_parameter<H: Into<mt::SPHint2D>>(&self, p: mt::Point3, _: H, _: usize) -> Option<(f64, f64)> {
        Some((0.5 * p.x / (p.x - 1.0), p.y))
    }
}

#[test]
fn generated_surface_boundaries_have_a_finite_work_budget() {
    let surface = SingularBoundary(Arc::new(AtomicUsize::new(0)));
    let vertices = [(0.75, 0.2), (0.75, 0.8), (0.9, 0.8), (0.9, 0.2)].map(|(u, v)| mt::Vertex::new(surface.subs(u, v)));
    let wire = (0..4).map(|i| mt::builder::line(&vertices[i], &vertices[(i + 1) % 4])).collect();
    let face = truck_topology::Face::new_unchecked(vec![wire], surface);
    let shell = truck_topology::Shell::from(vec![face]);
    let mesh = shell.triangulation(0.01);
    assert!(mesh.face_iter().next().and_then(|f| f.surface()).is_some(), "bounded meshing discarded the entire face");
}

#[derive(Clone)]
struct CountedProjection {
    surface: mt::BSplineSurface<mt::Point3>,
    global_searches: Arc<AtomicUsize>,
    stalled_hint: Option<(f64, f64)>,
}

impl ParametricSurface for CountedProjection {
    type Point = mt::Point3;
    type Vector = mt::Vector3;
    fn subs(&self, u: f64, v: f64) -> Self::Point {
        self.surface.subs(u, v)
    }
    fn uder(&self, u: f64, v: f64) -> Self::Vector {
        self.surface.uder(u, v)
    }
    fn vder(&self, u: f64, v: f64) -> Self::Vector {
        self.surface.vder(u, v)
    }
    fn uuder(&self, u: f64, v: f64) -> Self::Vector {
        self.surface.uuder(u, v)
    }
    fn uvder(&self, u: f64, v: f64) -> Self::Vector {
        self.surface.uvder(u, v)
    }
    fn vvder(&self, u: f64, v: f64) -> Self::Vector {
        self.surface.vvder(u, v)
    }
    fn parameter_range(&self) -> (mt::ParameterRange, mt::ParameterRange) {
        self.surface.parameter_range()
    }
}

impl mt::ParametricSurface3D for CountedProjection {
    fn normal(&self, u: f64, v: f64) -> mt::Vector3 {
        self.surface.normal(u, v)
    }
}

impl mt::ParameterDivision2D for CountedProjection {
    fn parameter_division(&self, range: ((f64, f64), (f64, f64)), tol: f64) -> (Vec<f64>, Vec<f64>) {
        self.surface.parameter_division(range, tol)
    }
}

impl mt::SearchParameter<mt::D2> for CountedProjection {
    type Point = mt::Point3;
    fn search_parameter<H: Into<mt::SPHint2D>>(&self, point: mt::Point3, hint: H, trials: usize) -> Option<(f64, f64)> {
        let hint = hint.into();
        if matches!(hint, mt::SPHint2D::None) {
            assert!(self.global_searches.fetch_add(1, Ordering::Relaxed) < 16, "repeated global surface searches");
        }
        self.surface.search_parameter(point, hint, trials)
    }
}

impl mt::SearchNearestParameter<mt::D2> for CountedProjection {
    type Point = mt::Point3;
    fn search_nearest_parameter<H: Into<mt::SPHint2D>>(&self, point: mt::Point3, hint: H, trials: usize) -> Option<(f64, f64)> {
        let hint = hint.into();
        if matches!(hint, mt::SPHint2D::None) {
            assert!(self.global_searches.fetch_add(1, Ordering::Relaxed) < 16, "repeated global surface searches");
        }
        if let mt::SPHint2D::Parameter(u, v) = hint
            && u < 0.1
            && v < 0.1
            && self.stalled_hint.is_some()
        {
            // Simulate a nearest solver stopping at the previous point or a singularity.
            return self.stalled_hint;
        }
        self.surface.search_nearest_parameter(point, hint, trials)
    }
}

#[test]
fn approximate_trim_projection_reuses_global_samples_and_rejects_bad_local_answers() {
    use mt::InnerSpace;
    const N: usize = 64;
    let hole_area = 0.5 * N as f64 * 0.2_f64.powi(2) * (std::f64::consts::TAU / N as f64).sin();
    for stalled_hint in [None, Some((0.05, 0.05)), Some((f64::NAN, f64::NAN))] {
        let surface = CountedProjection {
            surface: mt::BSplineSurface::new(
                (mt::KnotVec::bezier_knot(1), mt::KnotVec::bezier_knot(1)),
                vec![
                    vec![mt::Point3::new(0.0, 0.0, 0.0), mt::Point3::new(0.0, 1.0, 0.0)],
                    vec![mt::Point3::new(1.0, 0.0, 0.0), mt::Point3::new(1.0, 1.0, 0.0)],
                ],
            ),
            global_searches: Arc::new(AtomicUsize::new(0)),
            stalled_hint,
        };
        // Approximate export curves lie slightly above their support; exact searches fail.
        let wire = |points: Vec<mt::Point3>| {
            let vertices: Vec<_> = points.into_iter().map(mt::Vertex::new).collect();
            (0..vertices.len()).map(|i| mt::builder::line(&vertices[i], &vertices[(i + 1) % vertices.len()])).collect()
        };
        let outer = wire(vec![
            mt::Point3::new(0.05, 0.05, 1e-4),
            mt::Point3::new(0.95, 0.05, 1e-4),
            mt::Point3::new(0.95, 0.95, 1e-4),
            mt::Point3::new(0.05, 0.95, 1e-4),
        ]);
        let hole = wire(
            (0..N)
                .map(|i| {
                    let a = -(i as f64) * std::f64::consts::TAU / N as f64;
                    mt::Point3::new(0.5 + 0.2 * a.cos(), 0.5 + 0.2 * a.sin(), 1e-4)
                })
                .collect(),
        );
        let face = truck_topology::Face::new_unchecked(vec![outer, hole], surface);
        let shell = truck_topology::Shell::from(vec![face]);
        let mesh = shell.robust_triangulation(0.01).to_polygon();
        let area: f64 = mesh
            .tri_faces()
            .iter()
            .map(|tri| {
                let [a, b, c] = tri.map(|v| mesh.positions()[v.pos]);
                (b - a).cross(c - a).magnitude() * 0.5
            })
            .sum();
        assert!((area - (0.81 - hole_area)).abs() < 1e-6, "trimmed area {area}");
        assert!(mesh.positions().iter().all(|p| p.x.is_finite() && p.y.is_finite() && p.z.abs() < 1e-9));
    }
}
