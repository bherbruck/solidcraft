use truck_modeling as mt;
use truck_stepio::out;

use crate::body::Body;
use crate::{KernelError, Result, guard};

/// Intersection-curve edges (from booleans) as cubic B-splines through the exact curve: STEP
/// readers get plain 3D geometry that lies on both faces, and the file does not repeat the two
/// surfaces for every such edge.
fn plain_curves(b: &Body) -> Result<mt::Solid> {
    let s = crate::ops::placed_topology(&b.solid, mt::Matrix4::from_scale(1.0))?;
    let size = b.size();
    let mut seen = std::collections::HashSet::new();
    for e in s.edge_iter() {
        if !seen.insert(e.id()) {
            continue;
        }
        if let mt::Curve::IntersectionCurve(ic) = e.curve()
            && let Some(c) = hermite_through(&ic, size * 1e-4)
        {
            e.set_curve(mt::Curve::BSplineCurve(c));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for f in s.face_iter() {
        if !seen.insert(f.id()) {
            continue;
        }
        if let mt::Surface::RevolutedCurve(p) = f.surface()
            && !keeps_revolution(p.entity().axis(), p.transform())
            && let Some(n) = revolution_nurbs(&p)
        {
            f.set_surface(mt::Surface::NurbsSurface(n));
        }
    }
    Ok(s)
}

/// Whether a transformed surface of revolution is still one (the transform commutes with
/// turning about the axis), as the STEP writer requires; a scale that differs across the axis,
/// such as scaling a part's length and width but not its height, makes it elliptical.
fn keeps_revolution(axis: mt::Vector3, t: &mt::Matrix4) -> bool {
    use mt::{InnerSpace, SquareMatrix, Transform};
    // Tighter than truck's 1e-6, which the writer tests with: a surface near its limit is
    // written as NURBS rather than refused.
    const TOL: f64 = 1e-7;
    let small = |x: f64| x.abs() < TOL;
    if small(t.determinant()) {
        return false;
    }
    let a = axis.normalize();
    let la = t.transform_vector(a);
    if small(la.magnitude()) {
        return false;
    }
    let image = la.normalize();
    let seed = if a.x.abs() < 0.9 { mt::Vector3::unit_x() } else { mt::Vector3::unit_y() };
    let u = (seed - a * a.dot(seed)).normalize();
    let v = a.cross(u);
    let (lu, lv) = (t.transform_vector(u), t.transform_vector(v));
    let s = lu.magnitude();
    !small(s) && small(lu.dot(image) / s) && small(lv.dot(image) / s) && small(lu.dot(lv) / (s * s)) && small(lv.magnitude() / s - 1.0)
}

/// The exact NURBS form of a transformed surface of revolution whose profile is a line,
/// B-spline or NURBS: the profile turned by the rational quadratic full circle (u along the
/// profile, v the turn, as the revolution is parameterised), then the affine transform applied
/// to the homogeneous control points.
fn revolution_nurbs(p: &mt::Processor<mt::RevolutedCurve<mt::Curve>, mt::Matrix4>) -> Option<mt::NurbsSurface<mt::Vector4>> {
    use mt::{EuclideanSpace, InnerSpace};
    let rc = p.entity();
    let (knots, profile): (mt::KnotVec, Vec<mt::Vector4>) = match rc.entity_curve() {
        mt::Curve::Line(l) => (mt::KnotVec::bezier_knot(1), vec![l.0.to_homogeneous(), l.1.to_homogeneous()]),
        mt::Curve::BSplineCurve(c) => (c.knot_vec().clone(), c.control_points().iter().map(|q| q.to_homogeneous()).collect()),
        mt::Curve::NurbsCurve(c) => (c.knot_vec().clone(), c.control_points().clone()),
        mt::Curve::IntersectionCurve(_) => return None,
    };
    let (o, a) = (rc.origin(), rc.axis().normalize());
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let turn = [
        (1.0, 0.0, 1.0),
        (1.0, 1.0, h),
        (0.0, 1.0, 1.0),
        (-1.0, 1.0, h),
        (-1.0, 0.0, 1.0),
        (-1.0, -1.0, h),
        (0.0, -1.0, 1.0),
        (1.0, -1.0, h),
        (1.0, 0.0, 1.0),
    ];
    let t = *p.transform();
    let mut grid: Vec<Vec<mt::Vector4>> = Vec::with_capacity(profile.len());
    for q in &profile {
        if q.w.abs() < 1e-12 {
            return None;
        }
        let pt = mt::Point3::from_vec(q.truncate() / q.w);
        let c = o + a * a.dot(pt - o);
        let r = pt - c;
        let s = a.cross(r);
        grid.push(turn.iter().map(|(x, y, w)| t * ((c + r * *x + s * *y).to_homogeneous() * (q.w * w))).collect());
    }
    let vknots = mt::KnotVec::from(vec![0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0]);
    let (knots, grid) = if p.orientation() {
        ((knots, vknots), grid)
    } else {
        let n = grid.first().map_or(0, Vec::len);
        let swapped = (0..n).map(|j| grid.iter().filter_map(|row| row.get(j).copied()).collect()).collect();
        ((vknots, knots), swapped)
    };
    if grid.iter().flatten().any(|q| !(q.x.is_finite() && q.y.is_finite() && q.z.is_finite() && q.w.is_finite())) {
        return None;
    }
    mt::BSplineSurface::try_new(knots, grid).ok().map(mt::NurbsSurface::new)
}

/// A C¹ cubic B-spline through exact points of `c` spaced so that chords deviate at most `tol`
/// (Catmull–Rom tangents over chord length; the cubic is far closer than the chords).
fn hermite_through<C>(c: &C, tol: f64) -> Option<mt::BSplineCurve<mt::Point3>>
where
    C: mt::ParametricCurve<Point = mt::Point3, Vector = mt::Vector3> + mt::BoundedCurve + mt::ParameterDivision1D<Point = mt::Point3>,
{
    use mt::{InnerSpace, MetricSpace};
    let (_, pts) = c.parameter_division(c.range_tuple(), tol);
    let pts: Vec<mt::Point3> = pts.into_iter().fold(Vec::new(), |mut v: Vec<mt::Point3>, p| {
        if v.last().is_none_or(|q| q.distance(p) > tol * 1e-3) {
            v.push(p);
        }
        v
    });
    let n = pts.len().checked_sub(1).filter(|n| *n >= 1 && *n < 1_000_000)?;
    let mut u = vec![0.0; n + 1];
    for i in 1..=n {
        u[i] = u[i - 1] + pts[i].distance(pts[i - 1]);
    }
    let tangent = |i: usize| {
        let (a, b) = (i.saturating_sub(1), (i + 1).min(n));
        (pts[b] - pts[a]) / (u[b] - u[a])
    };
    let mut ctrl = vec![pts[0]];
    let mut knots = vec![u[0]; 4];
    for i in 0..n {
        let h = u[i + 1] - u[i];
        ctrl.push(pts[i] + tangent(i) * (h / 3.0));
        ctrl.push(pts[i + 1] - tangent(i + 1) * (h / 3.0));
        ctrl.push(pts[i + 1]);
        knots.extend(std::iter::repeat_n(u[i + 1], if i + 1 == n { 4 } else { 3 }));
    }
    if ctrl.iter().any(|p| !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite())) || tangent(0).magnitude() == 0.0 {
        return None;
    }
    mt::BSplineCurve::try_new(mt::KnotVec::from(knots), ctrl).ok()
}

/// truck's STEP text for the given bodies (geometry and topology; product data is ours).
pub(crate) fn truck_step(bodies: &[&Body], system: &str) -> Result<String> {
    if bodies.is_empty() {
        return Err(KernelError::Invalid("nothing to export".into()));
    }
    for b in bodies {
        b.require_brep("STEP export")?;
    }
    guard("step export", || {
        let cs: Vec<_> = bodies.iter().map(|b| plain_curves(b).map(|solid| solid.compress())).collect::<Result<_>>()?;
        let models: out::StepModels<_, _, _> = cs.iter().collect();
        let header = out::StepHeaderDescriptor { organization_system: system.to_owned(), ..Default::default() };
        written(&out::CompleteStepDisplay::new(models, header))
    })
}

/// The text of truck's STEP writer, or an error. The writer refuses some geometry by returning
/// `fmt::Error` after a line saying why; `to_string` would turn that into a panic (caught by
/// `guard` on the desktop, but the browser build aborts on any panic).
fn written(display: &impl std::fmt::Display) -> Result<String> {
    use std::fmt::Write;
    let mut text = String::new();
    match write!(text, "{display}") {
        Ok(()) => Ok(text),
        Err(_) => {
            let why = text.lines().last().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("unsupported geometry");
            Err(KernelError::Failed(format!("not supported yet: STEP export of this body ({why})")))
        }
    }
}

/// STEP (ISO 10303-21, AP242) text for the given bodies as one part (`Body1`…, with the
/// bodies' colours).
pub fn step_export(bodies: &[&Body], system: &str) -> Result<String> {
    let list: Vec<crate::ExportBody> =
        bodies.iter().enumerate().map(|(i, b)| crate::ExportBody { name: format!("Body{}", i + 1), body: b, color: b.color() }).collect();
    let header = crate::StepHeader { organization: system.to_string(), ..Default::default() };
    crate::step_out::step_export_bodies(&list, &header)
}

#[cfg(test)]
mod tests {
    use solidcraft_geom::Vec3;

    use crate::{cylinder, step_export, step_import, transform_matrix};

    #[test]
    fn scaled_across_the_axis_revolution_exports_as_nurbs() {
        // A cylinder along X scaled differently in Y and Z is elliptical: no longer a surface
        // of revolution, so it is written as its exact NURBS form.
        // Through STEP, as the part this came from was: imported cylinders are revolved lines.
        let made = cylinder(Vec3::ZERO, Vec3::X, 5.0, 20.0).unwrap();
        let text = step_export(&[&made], "test").unwrap();
        let c = step_import(&text).unwrap().bodies.into_iter().next().unwrap().body;
        assert!(c.solid.face_iter().any(|f| matches!(f.surface(), truck_modeling::Surface::RevolutedCurve(_))));
        let m = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.9, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let squashed = transform_matrix(&c, m).unwrap();
        let text = step_export(&[&squashed], "test").unwrap();
        assert!(text.contains("B_SPLINE_SURFACE"), "{text}");
        let back = step_import(&text).unwrap();
        let body = &back.bodies.first().unwrap().body;
        let expected = std::f64::consts::PI * 25.0 * 20.0 * 0.9;
        let v = body.volume_at(1e-3).unwrap();
        assert!((v - expected).abs() < 1e-3 * expected, "{v} vs {expected}");
        // Scaled the same across the axis, it stays a surface of revolution.
        let m = [[0.5, 0.0, 0.0, 0.0], [0.0, 0.9, 0.0, 0.0], [0.0, 0.0, 0.9, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let text = step_export(&[&transform_matrix(&c, m).unwrap()], "test").unwrap();
        assert!(!text.contains("B_SPLINE_SURFACE"), "{text}");
    }
}
