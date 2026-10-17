//! Periodic projections must retain the local trim loop even when Newton chooses a far turn.
use truck_meshalgo::prelude::*;
use truck_modeling::{self as mt, ParametricSurface};

#[derive(Clone)]
struct FarAngles(mt::Surface, f64);

impl ParametricSurface for FarAngles {
    type Point = mt::Point3;
    type Vector = mt::Vector3;
    fn subs(&self, u: f64, v: f64) -> Self::Point {
        self.0.subs(u, v)
    }
    fn uder(&self, u: f64, v: f64) -> Self::Vector {
        self.0.uder(u, v)
    }
    fn vder(&self, u: f64, v: f64) -> Self::Vector {
        self.0.vder(u, v)
    }
    fn uuder(&self, u: f64, v: f64) -> Self::Vector {
        self.0.uuder(u, v)
    }
    fn uvder(&self, u: f64, v: f64) -> Self::Vector {
        self.0.uvder(u, v)
    }
    fn vvder(&self, u: f64, v: f64) -> Self::Vector {
        self.0.vvder(u, v)
    }
    fn parameter_range(&self) -> (mt::ParameterRange, mt::ParameterRange) {
        self.0.parameter_range()
    }
    fn u_period(&self) -> Option<f64> {
        self.0.u_period()
    }
    fn v_period(&self) -> Option<f64> {
        self.0.v_period()
    }
}
impl mt::ParametricSurface3D for FarAngles {
    fn normal(&self, u: f64, v: f64) -> mt::Vector3 {
        self.0.normal(u, v)
    }
}
impl mt::ParameterDivision2D for FarAngles {
    fn parameter_division(&self, range: ((f64, f64), (f64, f64)), tol: f64) -> (Vec<f64>, Vec<f64>) {
        self.0.parameter_division(range, tol)
    }
}
impl mt::SearchParameter<mt::D2> for FarAngles {
    type Point = mt::Point3;
    fn search_parameter<H: Into<mt::SPHint2D>>(&self, point: mt::Point3, hint: H, trials: usize) -> Option<(f64, f64)> {
        let hint = hint.into();
        let hinted = matches!(hint, mt::SPHint2D::Parameter(..));
        if let (Some(period), mt::SPHint2D::Parameter(_, v)) = (self.v_period(), hint) {
            // Fail before a bad trim can generate a huge grid. This cylinder's loop has one turn.
            assert!(v.abs() <= 4.0 * period, "the next search inherited a distant periodic equivalent: {v}");
        }
        self.0.search_parameter(point, hint, trials).map(|(u, v)| {
            let shift = if hinted { self.v_period().unwrap_or(0.0) * self.1 } else { 0.0 };
            (u, v + shift)
        })
    }
}

#[test]
fn periodic_projection_keeps_far_equivalents_near_the_previous_point() {
    use solidcraft_geom::Vec3;
    let body = crate::cylinder(Vec3::ZERO, Vec3::Z, 5.0, 12.0).unwrap();
    // The primitive's spline support has no declared period. Use the exact periodic
    // cylindrical support, as STEP's CYLINDRICAL_SURFACE reader does.
    let support = crate::step_in::geom::revolved_oriented(
        mt::Curve::Line(mt::Line(mt::Point3::new(5.0, 0.0, 0.0), mt::Point3::new(5.0, 0.0, 12.0))),
        mt::Point3::new(0.0, 0.0, 0.0),
        mt::Vector3::new(0.0, 0.0, 1.0),
        0.5,
        mt::Vector3::new(1.0, 0.0, 0.0),
    );
    let expected = std::f64::consts::PI * 25.0 * 12.0;
    for turns in [-4096.0, 4096.0] {
        let shell =
            body.solid.boundaries().first().unwrap().mapped(Clone::clone, Clone::clone, |s| {
                FarAngles(if matches!(s, mt::Surface::Plane(_)) { s.clone() } else { support.clone() }, turns)
            });
        let mesh = shell.triangulation(0.01).to_polygon();
        assert!((mesh.volume() - expected).abs() / expected < 0.005, "{} vs {expected}", mesh.volume());
        assert!(mesh.positions().iter().all(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite()));
    }
}
