//! The vendored edge division evaluates each parameter once (the ends of a piece are carried
//! along, its midpoint is the probe and an end of both halves). Regressions against the division
//! as it was, which evaluated both ends of every piece again: the polylines must be the same, bit
//! for bit, including the fallbacks for samples that do not evaluate.
#![allow(clippy::unwrap_used, clippy::panic)]
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use solidcraft_geom::Vec3;
use truck_meshalgo::prelude::PolylineCurve;
use truck_meshalgo::tessellation::{Deflection, TrySubs, bounded_polyline};
use truck_modeling::{BoundedCurve, Curve, EuclideanSpace, MetricSpace, ParameterDivision1D, ParameterRange, ParametricCurve, Point3, Vector3};

use crate::*;

/// A curve that counts its evaluations.
#[derive(Clone)]
struct Counted {
    curve: Curve,
    calls: Arc<AtomicUsize>,
}

impl Counted {
    fn new(curve: &Curve) -> Self {
        Counted { curve: curve.clone(), calls: Arc::default() }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl ParametricCurve for Counted {
    type Point = Point3;
    type Vector = Vector3;
    fn subs(&self, t: f64) -> Point3 {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.curve.subs(t)
    }
    fn der(&self, t: f64) -> Vector3 {
        self.curve.der(t)
    }
    fn der2(&self, t: f64) -> Vector3 {
        self.curve.der2(t)
    }
    fn parameter_range(&self) -> ParameterRange {
        self.curve.parameter_range()
    }
    fn period(&self) -> Option<f64> {
        self.curve.period()
    }
}

impl BoundedCurve for Counted {}

/// The division samples through `TrySubs`: count those evaluations.
impl TrySubs for Counted {
    fn try_subs(&self, t: f64) -> Option<Point3> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        TrySubs::try_subs(&self.curve, t)
    }
    fn approx_subs(&self, t: f64) -> Point3 {
        self.calls.fetch_add(1, Ordering::Relaxed);
        TrySubs::approx_subs(&self.curve, t)
    }
}

impl ParameterDivision1D for Counted {
    type Point = Point3;
    fn parameter_division(&self, range: (f64, f64), tol: f64) -> (Vec<f64>, Vec<Point3>) {
        self.curve.parameter_division(range, tol)
    }
}

/// `bounded_polyline` before it carried the end points: 4 evaluations per piece (both ends, the
/// probe and the midpoint), with the same fallbacks for samples that do not evaluate.
fn reference(curve: &Counted, range: (f64, f64), d: Deflection) -> PolylineCurve<Point3> {
    const MAX_EDGE_POINTS: usize = 20_000;
    let (tol, turn2) = (d.chord, d.turn2());
    let end = |t: f64| curve.try_subs(t).unwrap_or_else(|| curve.approx_subs(t));
    let probe = |t: f64| curve.try_subs(t).unwrap_or(Point3::new(f64::NAN, f64::NAN, f64::NAN));
    let mut out = vec![end(range.0)];
    let mut stack: Vec<(f64, f64, usize)> = vec![(range.0, range.1, 0)];
    while let Some((a, b, depth)) = stack.pop() {
        let (pa, pb) = (end(a), end(b));
        let t = a + (b - a) * 0.4729;
        let chord = pa + (pb - pa) * 0.4729;
        let d2 = probe(t).distance2(chord);
        let tm = 0.5 * (a + b);
        let dm = probe(tm).distance2(pa.midpoint(pb));
        let flat = !(d2 >= tol * tol || dm >= tol * tol);
        let flat = flat && !d.turns(turn2, d2.max(dm), pa.distance2(pb));
        if flat || depth >= 24 || out.len() + stack.len() >= MAX_EDGE_POINTS {
            out.push(pb);
        } else {
            stack.push((tm, b, depth + 1));
            stack.push((a, tm, depth + 1));
        }
    }
    PolylineCurve(out)
}

/// Divides `curve` both ways and checks the points, their order and the evaluations; returns
/// the polyline.
fn same_division(curve: &Curve, d: Deflection) -> Vec<Point3> {
    let range = curve.range_tuple();
    let (before, after) = (Counted::new(curve), Counted::new(curve));
    let old = reference(&before, range, d).0;
    let new = bounded_polyline(&after, range, d).0;
    let bits = |p: &Point3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
    assert_eq!(old.len(), new.len(), "{d:?}");
    for (i, (p, q)) in old.iter().zip(&new).enumerate() {
        assert_eq!(bits(p), bits(q), "point {i} of {}: {p:?} before, {q:?} now ({d:?})", new.len());
    }
    // Every piece is kept (one point) or halved: n points come from 2n - 3 pieces.
    let pieces = 2 * new.len() - 3;
    assert_eq!(before.calls(), 1 + 4 * pieces);
    assert_eq!(after.calls(), 2 + 2 * pieces);
    new
}

/// The chord tolerance alone (booleans), then with the display's facet angle.
fn deflections(tol: f64) -> [Deflection; 2] {
    [Deflection::chord(tol), Deflection { chord: tol, angle: DISPLAY_ANGLE, floor: tol * 0.05, display: true }]
}

#[test]
fn intersection_curves_of_a_drilled_cylinder_divide_as_before() {
    // The radial hole of a clamping hub: both cylinders are NURBS, so each point of the edges
    // is two grid searches.
    let hub = cylinder(Vec3::new(0.0, 0.0, -10.0), Vec3::Z, 8.0, 20.0).unwrap();
    let drill = cylinder(Vec3::new(-12.0, 0.0, 1.5), Vec3::X, 1.25, 24.0).unwrap();
    let drilled = boolean(&hub, &drill, BoolOp::Cut).unwrap().unwrap();
    let curves: Vec<Curve> = drilled.solid.edge_iter().map(|e| e.curve()).filter(|c| matches!(c, Curve::IntersectionCurve(_))).collect();
    assert!(curves.len() >= 2, "{} intersection curves", curves.len());
    // One curve with each deflection (each division takes about half a second here).
    for (curve, d) in curves.iter().zip(deflections(0.01)) {
        let points = same_division(curve, d);
        assert!(points.len() > 8, "{} points ({d:?})", points.len());
        // On the drill's wall.
        for p in &points {
            let r = (p.y * p.y + (p.z - 1.5) * (p.z - 1.5)).sqrt();
            assert!((r - 1.25).abs() < 1e-3, "{p:?} at {r} from the drill's axis");
        }
    }
}

#[test]
fn circle_divides_as_before() {
    let body = cylinder(Vec3::ZERO, Vec3::Z, 5.0, 10.0).unwrap();
    let circle = body.solid.edge_iter().map(|e| e.curve()).find(|c| !matches!(c, Curve::Line(_))).unwrap();
    for d in deflections(0.001) {
        let points = same_division(&circle, d);
        assert!(points.len() > 18, "{} points ({d:?})", points.len());
        for p in &points {
            assert!(((p.x * p.x + p.y * p.y).sqrt() - 5.0).abs() < 1e-9, "{p:?}");
        }
    }
}
