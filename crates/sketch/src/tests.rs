use std::f64::consts::{FRAC_PI_2, PI};

use solidcraft_geom::{Seg2, Vec2};

use crate::*;

pub(crate) fn v(x: f64, y: f64) -> Vec2 {
    Vec2::new(x, y)
}

/// Deterministic pseudo-random numbers for the solver harnesses.
pub(crate) struct Lcg(pub u64);

impl Lcg {
    pub(crate) fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    pub(crate) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next() * (hi - lo)
    }
    /// ±1, evenly.
    pub(crate) fn sign(&mut self) -> f64 {
        if self.next() < 0.5 { 1.0 } else { -1.0 }
    }
}

/// Run `f` with the constructive seed on or off, without the seed-on/seed-off comparison (for
/// tests that measure the difference).
pub(crate) fn seeded<T>(on: bool, f: impl FnOnce() -> T) -> T {
    use crate::construct::switch;
    switch::without_differential(|| if on { f() } else { switch::without_seed(f) })
}

/// The points that are constants for a plain solve.
pub(crate) fn given(sk: &Sketch) -> Vec<bool> {
    crate::solver::held_points(sk, &Hold::default())
}

/// The constructive seed of a plain solve.
pub(crate) fn seed_of(sk: &Sketch) -> crate::construct::Seed {
    crate::construct::seed(sk, &given(sk), None, true, &crate::solver::points_on_curves(sk), &mut crate::decompose::Budget::full())
}

/// Rough rectangle from four lines sharing corner points.
fn rect(sk: &mut Sketch, p: [Vec2; 4]) -> [usize; 4] {
    let a = sk.add_point(p[0], None).unwrap();
    let b = sk.add_point(p[1], None).unwrap();
    let c = sk.add_point(p[2], None).unwrap();
    let d = sk.add_point(p[3], None).unwrap();
    [
        sk.add_line_pts(a, b, None).unwrap(),
        sk.add_line_pts(b, c, None).unwrap(),
        sk.add_line_pts(c, d, None).unwrap(),
        sk.add_line_pts(d, a, None).unwrap(),
    ]
}

fn line_pts(sk: &Sketch, l: usize) -> (Vec2, Vec2) {
    match sk.curves[l].kind {
        CurveKind::Line { a, b } => (sk.points[a].pos, sk.points[b].pos),
        _ => panic!(),
    }
}

#[test]
fn fully_constrained_rectangle() {
    let mut sk = Sketch::new();
    let l = rect(&mut sk, [v(0.3, -0.2), v(38.0, 1.0), v(41.0, 29.0), v(-1.0, 31.0)]);
    use ConstraintKind::*;
    sk.add_constraint(Horizontal { l: l[0] }, None).unwrap();
    sk.add_constraint(Horizontal { l: l[2] }, None).unwrap();
    sk.add_constraint(Vertical { l: l[1] }, None).unwrap();
    sk.add_constraint(Vertical { l: l[3] }, None).unwrap();
    let (a, _) = match sk.curves[l[0]].kind {
        CurveKind::Line { a, b } => (a, b),
        _ => panic!(),
    };
    sk.add_constraint(Coincident { p: a, q: 0 }, None).unwrap();
    sk.add_constraint(Length { l: l[0], value: 40.0 }, Some("d1".into())).unwrap();
    sk.add_constraint(Length { l: l[1], value: 30.0 }, Some("d2".into())).unwrap();
    let rep = solve(&mut sk);
    assert!(rep.ok(), "{rep:?}");
    assert_eq!(rep.dof, 0, "{rep:?}");
    assert!(rep.point_determined.iter().all(|d| *d));
    let (p0, p1) = line_pts(&sk, l[0]);
    assert!(p0.dist(v(0.0, 0.0)) < 1e-7, "{p0:?}");
    assert!(p1.dist(v(40.0, 0.0)) < 1e-7, "{p1:?}");
    let (_, p2) = line_pts(&sk, l[1]);
    assert!(p2.dist(v(40.0, 30.0)) < 1e-7, "{p2:?}");
    // Change a dimension and re-solve.
    sk.constraints[5].kind.set_value(55.0);
    assert!(solve(&mut sk).ok());
    let (_, p1) = line_pts(&sk, l[0]);
    assert!(p1.dist(v(55.0, 0.0)) < 1e-7);
}

#[test]
fn under_constrained_reports_dof() {
    let mut sk = Sketch::new();
    let l = sk.add_line(v(0.0, 0.0), v(10.0, 1.0), None, None, None).unwrap();
    sk.add_constraint(ConstraintKind::Horizontal { l }, None).unwrap();
    let rep = solve(&mut sk);
    assert!(rep.ok());
    assert_eq!(rep.dof, 3);
    let (a, b) = line_pts(&sk, l);
    assert!((a.y - b.y).abs() < 1e-9);
    // Minimal movement: the line moved by about half a unit each.
    assert!((a.y - 0.5).abs() < 0.05 && (b.y - 0.5).abs() < 0.05, "{a:?} {b:?}");
}

#[test]
fn conflicting_constraints_fail_and_restore() {
    let mut sk = Sketch::new();
    let l = sk.add_line(v(0.0, 0.0), v(10.0, 0.0), None, None, None).unwrap();
    sk.add_constraint(ConstraintKind::Horizontal { l }, None).unwrap();
    sk.add_constraint(ConstraintKind::Vertical { l }, None).unwrap();
    sk.add_constraint(ConstraintKind::Length { l, value: 10.0 }, None).unwrap();
    let before = sk.clone();
    let rep = solve(&mut sk);
    assert_eq!(rep.status, SolveStatus::Failed);
    assert!(!rep.failing.is_empty());
    assert_eq!(sk.points, before.points);
}

#[test]
fn circles_tangent_equal_concentric() {
    let mut sk = Sketch::new();
    let c1 = sk.add_circle(v(0.0, 0.0), 5.0, Some(0), None).unwrap();
    let c2 = sk.add_circle(v(12.0, 1.0), 3.0, None, None).unwrap();
    use ConstraintKind::*;
    sk.add_constraint(Radius { c: c1, value: 10.0 }, None).unwrap();
    sk.add_constraint(Equal { a: c1, b: c2 }, None).unwrap();
    sk.add_constraint(Tangent { a: c1, b: c2 }, None).unwrap();
    let rep = solve(&mut sk);
    assert!(rep.ok(), "{rep:?}");
    assert!((sk.radius(c2).unwrap() - 10.0).abs() < 1e-7);
    assert!((sk.center(c2).unwrap().len() - 20.0).abs() < 1e-7);
    let c3 = sk.add_circle(v(3.0, 3.0), 2.0, None, None).unwrap();
    sk.add_constraint(Concentric { a: c3, b: c2 }, None).unwrap();
    assert!(solve(&mut sk).ok());
    assert!(sk.center(c3).unwrap().dist(sk.center(c2).unwrap()) < 1e-7);
}

#[test]
fn line_tangent_to_circle_and_angles() {
    let mut sk = Sketch::new();
    let c = sk.add_circle(v(0.0, 0.0), 10.0, Some(0), None).unwrap();
    use ConstraintKind::*;
    sk.add_constraint(Radius { c, value: 10.0 }, None).unwrap();
    let l = sk.add_line(v(-20.0, 12.0), v(20.0, 11.0), None, None, None).unwrap();
    sk.add_constraint(Tangent { a: l, b: c }, None).unwrap();
    sk.add_constraint(Horizontal { l }, None).unwrap();
    assert!(solve(&mut sk).ok());
    let (a, _) = line_pts(&sk, l);
    assert!((a.y.abs() - 10.0).abs() < 1e-7, "{a:?}");
    let m = sk.add_line(v(0.0, 0.0), v(10.0, 3.0), Some(0), None, None).unwrap();
    sk.add_constraint(Angle { a: l, b: m, value: PI / 6.0, flip: false }, None).unwrap();
    sk.add_constraint(Length { l: m, value: 8.0 }, None).unwrap();
    assert!(solve(&mut sk).ok());
    let (p, q) = line_pts(&sk, m);
    let (la, lb) = line_pts(&sk, l);
    assert!(((q - p).angle() - (lb - la).angle() - PI / 6.0).abs() < 1e-7, "{la:?} {lb:?} {p:?} {q:?}");
    let n2 = sk.add_line(v(1.0, 1.0), v(2.0, 5.0), None, None, None).unwrap();
    sk.add_constraint(Perpendicular { a: m, b: n2 }, None).unwrap();
    sk.add_constraint(Parallel { a: n2, b: n2 }, None).unwrap();
    assert!(solve(&mut sk).ok());
    let (p2, q2) = line_pts(&sk, n2);
    assert!(((q2 - p2).angle() - (PI / 6.0 + FRAC_PI_2)).abs() < 1e-6 || ((q2 - p2).angle() - (PI / 6.0 - FRAC_PI_2)).abs() < 1e-6);
}

#[test]
fn midpoint_symmetry_point_on_curve_arc() {
    let mut sk = Sketch::new();
    use ConstraintKind::*;
    let l = sk.add_line(v(-5.0, 0.0), v(5.0, 0.0), None, None, None).unwrap();
    let p = sk.add_point(v(1.0, 1.0), None).unwrap();
    sk.add_constraint(Midpoint { p, l }, None).unwrap();
    let a = sk.add_arc(v(0.0, 0.0), v(4.0, 0.0), v(0.0, 4.0), [None, None, None], None).unwrap();
    sk.add_constraint(Radius { c: a, value: 6.0 }, None).unwrap();
    let q = sk.add_point(v(3.0, 3.0), None).unwrap();
    sk.add_constraint(PointOnCurve { p: q, c: a }, None).unwrap();
    let s1 = sk.add_point(v(-2.0, 1.0), None).unwrap();
    let s2 = sk.add_point(v(3.0, -1.5), None).unwrap();
    let axis = sk.add_line(v(0.0, -10.0), v(0.0, 10.0), None, None, None).unwrap();
    sk.add_constraint(Vertical { l: axis }, None).unwrap();
    sk.add_constraint(Symmetric { p: s1, q: s2, l: axis }, None).unwrap();
    let rep = solve(&mut sk);
    assert!(rep.ok(), "{rep:?}");
    let (la, lb) = line_pts(&sk, l);
    assert!(sk.points[p].pos.dist((la + lb) * 0.5) < 1e-7);
    assert!((sk.radius(a).unwrap() - 6.0).abs() < 1e-7);
    let c = sk.center(a).unwrap();
    assert!((sk.points[q].pos.dist(c) - 6.0).abs() < 1e-7);
    let (ax, _) = line_pts(&sk, axis);
    assert!((sk.points[s1].pos.x + sk.points[s2].pos.x - 2.0 * ax.x).abs() < 1e-7);
    assert!((sk.points[s1].pos.y - sk.points[s2].pos.y).abs() < 1e-7);
}

#[test]
fn profiles_rectangle_with_hole_and_split() {
    let mut sk = Sketch::new();
    rect(&mut sk, [v(0.0, 0.0), v(40.0, 0.0), v(40.0, 30.0), v(0.0, 30.0)]);
    sk.add_circle(v(20.0, 15.0), 5.0, None, None).unwrap();
    let ps = find_profiles(&sk);
    assert_eq!(ps.len(), 2, "{ps:?}");
    let big = ps.iter().find(|p| p.region.holes.len() == 1).unwrap();
    assert!((big.area - (1200.0 - PI * 25.0)).abs() < 1e-6);
    let disc = ps.iter().find(|p| p.region.holes.is_empty()).unwrap();
    assert!((disc.area - PI * 25.0).abs() < 1e-6);
    assert_eq!(big.hole_curves.len(), 1);

    // A line across the rectangle splits it into two regions (with the hole in one).
    let mut sk2 = Sketch::new();
    let l = rect(&mut sk2, [v(0.0, 0.0), v(40.0, 0.0), v(40.0, 30.0), v(0.0, 30.0)]);
    let (b0, _) = line_pts(&sk2, l[0]);
    let _ = b0;
    let pa = sk2.add_point(v(10.0, 0.0), None).unwrap();
    let pb = sk2.add_point(v(10.0, 30.0), None).unwrap();
    // Re-split bottom and top edges at x = 10 by rebuilding the sketch as 7 lines.
    let mut s3 = Sketch::new();
    let pts = [v(0.0, 0.0), v(10.0, 0.0), v(40.0, 0.0), v(40.0, 30.0), v(10.0, 30.0), v(0.0, 30.0)];
    let ids: Vec<usize> = pts.iter().map(|p| s3.add_point(*p, None).unwrap()).collect();
    for i in 0..6 {
        s3.add_line_pts(ids[i], ids[(i + 1) % 6], None).unwrap();
    }
    s3.add_line_pts(ids[1], ids[4], None).unwrap();
    let _ = (pa, pb);
    let ps = find_profiles(&s3);
    assert_eq!(ps.len(), 2);
    let mut areas: Vec<f64> = ps.iter().map(|p| p.area).collect();
    areas.sort_by(f64::total_cmp);
    assert!((areas[0] - 300.0).abs() < 1e-9 && (areas[1] - 900.0).abs() < 1e-9, "{areas:?}");
}

#[test]
fn profile_with_arcs_and_construction_ignored() {
    // Slot: two lines and two half arcs.
    let mut sk = Sketch::new();
    let p = [v(0.0, 0.0), v(20.0, 0.0), v(20.0, 10.0), v(0.0, 10.0)];
    let ids: Vec<usize> = p.iter().map(|q| sk.add_point(*q, None).unwrap()).collect();
    sk.add_line_pts(ids[0], ids[1], None).unwrap();
    sk.add_arc(v(20.0, 5.0), p[1], p[2], [None, Some(ids[1]), Some(ids[2])], None).unwrap();
    sk.add_line_pts(ids[2], ids[3], None).unwrap();
    sk.add_arc(v(0.0, 5.0), p[3], p[0], [None, Some(ids[3]), Some(ids[0])], None).unwrap();
    let c = sk.add_line(v(-5.0, -5.0), v(30.0, 20.0), None, None, None).unwrap();
    sk.curves[c].construction = true;
    let ps = find_profiles(&sk);
    assert_eq!(ps.len(), 1, "{ps:?}");
    assert!((ps[0].area - (200.0 + PI * 25.0)).abs() < 1e-6, "{}", ps[0].area);
    assert_eq!(ps[0].outer_curves.len(), 4);
}

#[test]
fn removal_and_references() {
    let mut sk = Sketch::new();
    let l1 = sk.add_line(v(0.0, 0.0), v(1.0, 0.0), None, None, Some("l1")).unwrap();
    let e = sk.resolve_point("l1.end").unwrap();
    let l2 = sk.add_line(v(1.0, 0.0), v(1.0, 1.0), Some(e), None, Some("l2")).unwrap();
    sk.add_constraint(ConstraintKind::Horizontal { l: l1 }, None).unwrap();
    sk.add_constraint(ConstraintKind::Vertical { l: l2 }, None).unwrap();
    assert!(sk.add_line(v(0.0, 0.0), v(1.0, 0.0), None, None, Some("l1")).is_err());
    sk.remove_curves(&[l1]);
    assert_eq!(sk.curves.len(), 1);
    assert_eq!(sk.constraints.len(), 1);
    assert!(sk.resolve_point("l2.start").is_some());
    assert!(sk.resolve_point("l1.start").is_none());
    assert_eq!(sk.points.len(), 3, "origin + l2's two points");
    assert!(solve(&mut sk).ok());
    let json = serde_json::to_string(&sk).unwrap();
    let back: Sketch = serde_json::from_str(&json).unwrap();
    assert_eq!(back, sk);
}

#[test]
fn hostile_values_rejected() {
    let mut sk = Sketch::new();
    assert!(sk.add_point(v(f64::NAN, 0.0), None).is_err());
    assert!(sk.add_circle(v(0.0, 0.0), -1.0, None, None).is_err());
    assert!(sk.add_circle(v(0.0, 0.0), f64::INFINITY, None, None).is_err());
    assert!(sk.add_constraint(ConstraintKind::Horizontal { l: 99 }, None).is_err());
    let c = sk.add_circle(v(0.0, 0.0), 1.0, None, None).unwrap();
    assert!(sk.add_constraint(ConstraintKind::Horizontal { l: c }, None).is_err());
    assert!(sk.add_constraint(ConstraintKind::Radius { c, value: f64::NAN }, None).is_err());
    assert!(sk.add_constraint(ConstraintKind::Radius { c, value: -2.0 }, None).is_err());
}

#[test]
fn profiles_split_at_crossings_and_touch_points() {
    // A line across a circle: two half discs.
    let mut sk = Sketch::new();
    sk.add_circle(v(0.0, 0.0), 10.0, None, None).unwrap();
    sk.add_line(v(-15.0, 0.0), v(15.0, 0.0), None, None, None).unwrap();
    let ps = find_profiles(&sk);
    assert_eq!(ps.len(), 2, "{ps:?}");
    for p in &ps {
        assert!((p.area - PI * 50.0).abs() < 1e-6, "{}", p.area);
    }
    // Belt: two circles joined by two tangent lines whose ends sit on the circles.
    let mut b = Sketch::new();
    let c1 = b.add_circle(v(0.0, 0.0), 20.0, Some(0), None).unwrap();
    let c2 = b.add_circle(v(50.0, 0.0), 10.0, None, None).unwrap();
    let l1 = b.add_line(v(3.0, 19.0), v(47.0, 13.0), None, None, None).unwrap();
    let l2 = b.add_line(v(1.0, -22.0), v(44.0, -8.0), None, None, None).unwrap();
    use ConstraintKind::*;
    for (l, end, c) in [(l1, "start", c1), (l1, "end", c2), (l2, "start", c1), (l2, "end", c2)] {
        let p = b.resolve_point(&format!("{}.{end}", b.curves[l].id)).unwrap();
        b.add_constraint(PointOnCurve { p, c }, None).unwrap();
    }
    for (l, c) in [(l1, c1), (l1, c2), (l2, c1), (l2, c2)] {
        b.add_constraint(Tangent { a: l, b: c }, None).unwrap();
    }
    b.add_constraint(Radius { c: c1, value: 20.0 }, None).unwrap();
    b.add_constraint(Radius { c: c2, value: 10.0 }, None).unwrap();
    let r = solve(&mut b);
    assert!(r.ok(), "{r:?}");
    let ps = find_profiles(&b);
    assert_eq!(ps.len(), 3, "two discs and the band between them: {:?}", ps.iter().map(|p| p.area).collect::<Vec<_>>());
    let total: f64 = ps.iter().map(|p| p.area).sum();
    assert!(total > PI * 500.0, "{total}");
}

/// Curves made tangent by the solver touch within rounding noise, either side of exact (#588):
/// the touch still splits them. Lines running past their tangent points close the band between
/// two circles, and a square's side tangent to a circle closes the square off from the disc.
#[test]
fn solver_tangencies_split_profiles() {
    use ConstraintKind::*;
    // Two fixed circles and two free lines, longer than the band, made tangent to both.
    for (r1, r2, d) in [(15.0, 8.0, 40.0), (20.0, 10.0, 50.0), (7.3, 7.3, 21.1)] {
        let mut b = Sketch::new();
        let c1 = b.add_circle(v(0.0, 0.0), r1, None, None).unwrap();
        let c2 = b.add_circle(v(d, 0.0), r2, None, None).unwrap();
        let l1 = b.add_line(v(-r1, r1 + 2.0), v(d + r2 + 5.0, r2 - 1.0), None, None, None).unwrap();
        let l2 = b.add_line(v(-r1, -r1 - 2.0), v(d + r2 + 5.0, 1.0 - r2), None, None, None).unwrap();
        for c in [c1, c2] {
            let p = b.resolve_point(&format!("{}.center", b.curves[c].id)).unwrap();
            b.add_constraint(Fix { p }, None).unwrap();
        }
        b.add_constraint(Radius { c: c1, value: r1 }, None).unwrap();
        b.add_constraint(Radius { c: c2, value: r2 }, None).unwrap();
        for (l, c) in [(l1, c1), (l1, c2), (l2, c1), (l2, c2)] {
            b.add_constraint(Tangent { a: l, b: c }, None).unwrap();
        }
        assert!(solve(&mut b).ok());
        let ps = find_profiles(&b);
        // The band: the trapezoid between the tangent points less the circular segments the
        // circles take from it (as exact sums: the hull less the two discs).
        let alpha = ((r1 - r2) / d).asin();
        let tangent_len = (d * d - (r1 - r2).powi(2)).sqrt();
        let hull = r1 * r1 * (PI / 2.0 + alpha) + r2 * r2 * (PI / 2.0 - alpha) + (r1 + r2) * tangent_len;
        let total: f64 = ps.iter().map(|p| p.area).sum();
        assert_eq!(ps.len(), 3, "r {r1}, {r2}: {:?}", ps.iter().map(|p| p.area).collect::<Vec<_>>());
        assert!((total - hull).abs() < 1e-6 * hull, "r {r1}, {r2}: {total} vs {hull}");
    }
    // A square tangent to a circle inside it and outside it, the circle placed by the solver.
    for (inside, centre) in [(true, v(5.0, 6.3)), (false, v(5.0, 16.7))] {
        let mut s = Sketch::new();
        let sides = [v(0.0, 0.0), v(10.0, 0.0), v(10.0, 10.0), v(0.0, 10.0)];
        let mut lines = Vec::new();
        for k in 0..4 {
            lines.push(s.add_line(sides[k], sides[(k + 1) % 4], None, None, None).unwrap());
        }
        for k in 0..4 {
            let (a, b) = (
                s.resolve_point(&format!("{}.end", s.curves[lines[k]].id)).unwrap(),
                s.resolve_point(&format!("{}.start", s.curves[lines[(k + 1) % 4]].id)).unwrap(),
            );
            s.add_constraint(Coincident { p: a, q: b }, None).unwrap();
            s.add_constraint(Fix { p: a }, None).unwrap();
        }
        let c = s.add_circle(centre, 3.7, None, None).unwrap();
        s.add_constraint(Radius { c, value: 3.7 }, None).unwrap();
        s.add_constraint(Tangent { a: lines[2], b: c }, None).unwrap();
        assert!(solve(&mut s).ok());
        let ps = find_profiles(&s);
        let disc = PI * 3.7 * 3.7;
        let mut areas: Vec<f64> = ps.iter().map(|p| p.area).collect();
        areas.sort_by(f64::total_cmp);
        let want = if inside { [disc, 100.0 - disc] } else { [disc, 100.0] };
        let mut want = want.to_vec();
        want.sort_by(f64::total_cmp);
        assert_eq!(areas.len(), 2, "inside {inside}: {areas:?}");
        for (a, w) in areas.iter().zip(&want) {
            assert!((a - w).abs() < 1e-6, "inside {inside}: {areas:?} vs {want:?}");
        }
    }
}

/// Sorted curve ids.
fn id_set(sk: &Sketch, curves: &[usize]) -> Vec<String> {
    let mut ids: Vec<String> = curves.iter().map(|c| sk.curves[*c].id.clone()).collect();
    ids.sort();
    ids
}

/// A projected line of `a`–`b` in `sk`.
fn reference_line(sk: &mut Sketch, a: Vec2, b: Vec2) -> usize {
    let link = sk.add_link(LinkKind::Project, LinkSource::SketchCurve { sketch: 1, curve: "edge".into() }, &[LinkGeom::Line(a, b)]).unwrap();
    sk.link_curves(&link)[0]
}

/// #635: a side constrained onto a longer projected line lies on it only up to the solver's
/// round-off. Where they overlap they are one edge of the drawing, so the rectangle is still a
/// closed profile, and the curve that came first names that edge, as when the side lies on the
/// line exactly.
#[test]
fn a_side_on_a_projected_line_still_closes_its_rectangle() {
    // Round-off left on the side's two ends (the first pair as solved in #635), and none.
    for (y0, y1) in
        [(1.6e-12, 2.2e-12), (2.2e-12, 1.6e-12), (-1e-12, 1e-12), (1e-12, -1e-12), (-3e-11, -1e-11), (-3e-10, 3e-10), (2e-10, 4e-10), (0.0, 0.0)]
    {
        for reference_first in [true, false] {
            for (ra, rb) in [(v(40.0, 0.0), v(-10.0, 0.0)), (v(-10.0, 0.0), v(40.0, 0.0))] {
                let mut sk = Sketch::new();
                let early = reference_first.then(|| reference_line(&mut sk, ra, rb));
                let sides = rect(&mut sk, [v(5.0, y0), v(25.0, y1), v(25.0, 15.0), v(5.0, 15.0)]);
                let reference = early.unwrap_or_else(|| reference_line(&mut sk, ra, rb));
                let case = format!("ends at y {y0}, {y1}; reference {} {ra:?} to {rb:?}", if reference_first { "first" } else { "last" });
                let ps = find_profiles(&sk);
                assert_eq!(ps.len(), 1, "{case}: {:?}", ps.iter().map(|p| (p.area, &p.outer_curves)).collect::<Vec<_>>());
                assert!((ps[0].area - 300.0).abs() < 1e-6, "{case}: {}", ps[0].area);
                let mut got = ps[0].outer_curves.clone();
                got.sort();
                let want = id_set(&sk, &[if reference_first { reference } else { sides[0] }, sides[1], sides[2], sides[3]]);
                assert_eq!(got, want, "{case}");
                // Whole curves make a lasting identity; a piece of the reference cut at the corners does not.
                let whole = (!reference_first).then(|| (want, Vec::new()));
                assert_eq!(ps[0].whole_curve_boundaries(), whole, "{case}");
            }
        }
    }
}

/// A third curve meeting a side and the projected line under it would meet each a little apart:
/// a circle touching the line from inside the rectangle (it misses or crosses the side by the
/// round-off), and a line crossing both at a shallow angle (the crossings 3e-5 mm apart). It
/// meets both where it meets the first of them, so they still make one edge, whichever way the
/// side runs.
#[test]
fn a_curve_meeting_a_side_on_a_projected_line_meets_both_at_once() {
    for y0 in [3e-10, -3e-10, 0.0] {
        for reference_first in [true, false] {
            for reversed in [false, true] {
                let case = format!("side at y {y0}, reference {}, side reversed {reversed}", if reference_first { "first" } else { "last" });
                let corners = |x1: f64| {
                    let (a, b) = if reversed { (x1, 5.0) } else { (5.0, x1) };
                    [v(a, y0), v(b, y0), v(b, 15.0), v(a, 15.0)]
                };
                let mut sk = Sketch::new();
                let early = reference_first.then(|| reference_line(&mut sk, v(-10.0, 0.0), v(40.0, 0.0)));
                rect(&mut sk, corners(25.0));
                sk.add_circle(v(15.0, 5.0), 5.0, None, None).unwrap();
                early.unwrap_or_else(|| reference_line(&mut sk, v(-10.0, 0.0), v(40.0, 0.0)));
                let mut areas: Vec<f64> = find_profiles(&sk).iter().map(|p| p.area).collect();
                areas.sort_by(f64::total_cmp);
                assert_eq!(areas.len(), 2, "touching circle, {case}: {areas:?}");
                assert!((areas[0] - 25.0 * PI).abs() < 1e-6 && (areas[1] - (300.0 - 25.0 * PI)).abs() < 1e-6, "touching circle, {case}: {areas:?}");

                let mut sk = Sketch::new();
                let early = reference_first.then(|| reference_line(&mut sk, v(-10.0, 0.0), v(440.0, 0.0)));
                rect(&mut sk, corners(405.0));
                // Through (200, 0), rising 1e-5 per mm: a sliver of 205 x 0.00205 / 2 under it.
                sk.add_line(v(0.0, -0.002), v(430.0, 0.0023), None, None, None).unwrap();
                early.unwrap_or_else(|| reference_line(&mut sk, v(-10.0, 0.0), v(440.0, 0.0)));
                let mut areas: Vec<f64> = find_profiles(&sk).iter().map(|p| p.area).collect();
                areas.sort_by(f64::total_cmp);
                assert_eq!(areas.len(), 2, "shallow line, {case}: {areas:?}");
                assert!((areas[0] - 0.210125).abs() < 1e-6 && (areas[1] - (6000.0 - 0.210125)).abs() < 1e-6, "shallow line, {case}: {areas:?}");
            }
        }
    }
}

/// The same for drawn curves on drawn curves: two rectangles drawn side by side, the second's
/// side solved onto the first's; and a quarter disc whose arc is solved onto a projected circle.
#[test]
fn curves_solved_onto_each_other_keep_the_regions_on_both_sides() {
    for (x0, x1) in [(1e-12, -1e-12), (-2e-12, -1e-12), (2.2e-12, 1.6e-12), (0.0, 0.0)] {
        let mut sk = Sketch::new();
        let a = rect(&mut sk, [v(0.0, 0.0), v(20.0, 0.0), v(20.0, 15.0), v(0.0, 15.0)]);
        let b = rect(&mut sk, [v(20.0 + x0, 0.0), v(40.0, 0.0), v(40.0, 15.0), v(20.0 + x1, 15.0)]);
        let ps = find_profiles(&sk);
        let areas: Vec<f64> = ps.iter().map(|p| p.area).collect();
        assert_eq!(ps.len(), 2, "{x0}, {x1}: {areas:?}");
        assert!(areas.iter().all(|a| (a - 300.0).abs() < 1e-6), "{x0}, {x1}: {areas:?}");
        // The shared side is the first rectangle's (it was there first).
        let mut first = ps[0].outer_curves.clone();
        first.sort();
        assert_eq!(first, id_set(&sk, &a), "{x0}, {x1}");
        let mut second = ps[1].outer_curves.clone();
        second.sort();
        assert_eq!(second, id_set(&sk, &[b[0], b[1], b[2], a[1]]), "{x0}, {x1}");
    }
    let hole = |sk: &mut Sketch| {
        let link = sk
            .add_link(LinkKind::Project, LinkSource::SketchCurve { sketch: 1, curve: "hole".into() }, &[LinkGeom::Circle(v(0.0, 0.0), 10.0)])
            .unwrap();
        sk.link_curves(&link)[0]
    };
    for (dx, dr) in [(1e-12, 2e-12), (-1e-12, -1e-12), (0.0, 0.0)] {
        for circle_first in [true, false] {
            let mut sk = Sketch::new();
            let early = circle_first.then(|| hole(&mut sk));
            let c = sk.add_point(v(dx, -dx), None).unwrap();
            let (p0, p1) = (sk.add_point(v(10.0 + dr, 0.0), None).unwrap(), sk.add_point(v(0.0, 10.0 + dr), None).unwrap());
            let arc = sk.add_arc(v(dx, -dx), v(10.0 + dr, 0.0), v(0.0, 10.0 + dr), [Some(c), Some(p0), Some(p1)], None).unwrap();
            let (l0, l1) = (sk.add_line_pts(c, p0, None).unwrap(), sk.add_line_pts(p1, c, None).unwrap());
            let circle = early.unwrap_or_else(|| hole(&mut sk));
            let case = format!("{dx}, {dr}, circle {}", if circle_first { "first" } else { "last" });
            let ps = find_profiles(&sk);
            let areas: Vec<f64> = ps.iter().map(|p| p.area).collect();
            assert_eq!(ps.len(), 2, "{case}: {areas:?}");
            let quarter = ps.iter().find(|p| p.area < 100.0).unwrap();
            assert!((quarter.area - 25.0 * PI).abs() < 1e-6, "{case}: {areas:?}");
            // One piece each: round-off alone does not cut the arc where it crosses the circle.
            let mut got = quarter.outer_curves.clone();
            got.sort();
            assert_eq!(got, id_set(&sk, &[if circle_first { circle } else { arc }, l0, l1]), "{case}");
            let rest = ps.iter().find(|p| p.area > 100.0).unwrap();
            assert!((rest.area - 75.0 * PI).abs() < 1e-6, "{case}: {areas:?}");
        }
    }
}

/// What does not run along one path stays apart: a chord and an arc bulging past it by about
/// the merge tolerance still bound their thin region, and two whole circles further apart than
/// it are still cut where they cross, into a lens and two crescents. Closer than it, their
/// halves are one path: one disc.
#[test]
fn near_overlaps_that_are_not_one_path_keep_their_regions() {
    let s = 1.5e-5;
    let r = (25.0 + s * s) / (2.0 * s);
    let mut sk = Sketch::new();
    let (a, b) = (sk.add_point(v(-5.0, 0.0), None).unwrap(), sk.add_point(v(5.0, 0.0), None).unwrap());
    let chord = sk.add_line_pts(a, b, None).unwrap();
    let arc = sk.add_arc(v(0.0, s - r), v(5.0, 0.0), v(-5.0, 0.0), [None, Some(b), Some(a)], None).unwrap();
    let ps = find_profiles(&sk);
    assert_eq!(ps.len(), 1, "{:?}", ps.iter().map(|p| p.area).collect::<Vec<_>>());
    // Two thirds of chord times height, to well within the rounding of the area.
    assert!((ps[0].area - 2.0 / 3.0 * 10.0 * s).abs() < 1e-8, "{}", ps[0].area);
    let mut got = ps[0].outer_curves.clone();
    got.sort();
    assert_eq!(got, id_set(&sk, &[chord, arc]));

    // The same with a free-form curve: a flat cubic (height s) over its chord. It takes part as
    // sampled pieces, one of them on the chord, but its exact curve bounds the region.
    let s = 8e-6;
    let mut sk = Sketch::new();
    let ids: Vec<usize> = [v(-5.0, 0.0), v(-5.0 / 3.0, 4.0 * s / 3.0), v(5.0 / 3.0, 4.0 * s / 3.0), v(5.0, 0.0)]
        .iter()
        .map(|p| sk.add_point(*p, None).unwrap())
        .collect();
    sk.add_curve(CurveKind::Spline { pts: ids.clone(), control: true, degree: 3, periodic: false }, None).unwrap();
    sk.add_line_pts(ids[3], ids[0], None).unwrap();
    let ps = find_profiles(&sk);
    assert_eq!(ps.len(), 1, "{:?}", ps.iter().map(|p| p.area).collect::<Vec<_>>());
    assert!((ps[0].area - 20.0 * s / 3.0).abs() < 1e-9, "{}", ps[0].area);

    // An arc a little off a circle, more than the merge tolerance apart at its ends (so they
    // don't cut the circle): the two still cut each other where they cross, and no profile is
    // the uncut circle.
    let mut sk = Sketch::new();
    let circle = sk.add_circle(v(0.0, 0.0), 10.0, None, None).unwrap();
    let (c, r) = (v(8e-6, 0.0), 10.0 + 6e-6);
    let (p0, p1) = (c + Vec2::from_angle(PI / 4.0) * r, c + Vec2::from_angle(-PI / 4.0) * r);
    let (a0, a1, tip) = (sk.add_point(p0, None).unwrap(), sk.add_point(p1, None).unwrap(), sk.add_point(v(30.0, 0.0), None).unwrap());
    sk.add_arc(c, p0, p1, [None, Some(a0), Some(a1)], None).unwrap();
    sk.add_line_pts(a1, tip, None).unwrap();
    sk.add_line_pts(tip, a0, None).unwrap();
    let ps = find_profiles(&sk);
    let circle_id = sk.curves[circle].id.clone();
    assert!(ps.iter().all(|p| p.outer_curves != [circle_id.clone()] && p.whole_curve_boundaries().is_none()), "{ps:?}");
    // Profiles don't overlap: together they cover the disc and the arc's region once, that is the
    // disc and the triangle to the tip less the disc's segment inside it (to within the slivers
    // between arc and circle).
    let beyond = 0.5 * (p0 - v(30.0, 0.0)).cross(p1 - v(30.0, 0.0)).abs() - 50.0 * (PI / 2.0 - 1.0);
    let covered = ps.iter().map(|p| p.area).sum::<f64>();
    assert!((covered - PI * 100.0 - beyond).abs() < 1e-2, "{:?}", ps.iter().map(|p| p.area).collect::<Vec<_>>());

    let r = 10.0_f64;
    for (d, regions) in [(1e-3, 3), (1e-4, 3), (8e-6, 1)] {
        let mut sk = Sketch::new();
        let first = sk.add_circle(v(0.0, 0.0), r, None, None).unwrap();
        sk.add_circle(v(d, 0.0), r, None, None).unwrap();
        let ps = find_profiles(&sk);
        let areas: Vec<f64> = ps.iter().map(|p| p.area).collect();
        assert_eq!(ps.len(), regions, "{d}: {areas:?}");
        let lens = 2.0 * r * r * (d / (2.0 * r)).acos() - d / 2.0 * (4.0 * r * r - d * d).sqrt();
        let union = if regions == 1 { PI * r * r } else { 2.0 * PI * r * r - lens };
        assert!((areas.iter().sum::<f64>() - union).abs() < 1e-6, "{d}: {areas:?}");
        // Cut where they cross: no whole-circle identity.
        assert!(ps.iter().all(|p| p.whole_curve_boundaries().is_none()), "{d}");
        if regions == 1 {
            assert_eq!(ps[0].outer_curves, vec![sk.curves[first].id.clone(); 2], "{d}: the first circle's halves");
        }
    }
}

/// Curves count as lying on one another only within the merge tolerance of the one carrying
/// them, not along a chain: an arc between two whole circles 1.6e-5 apart (each within 8e-6 of
/// it) does not stop the circles cutting each other where they cross.
#[test]
fn curves_lying_on_one_another_do_not_chain_past_the_tolerance() {
    let (d, r) = (16e-6, 10.0_f64);
    let mut sk = Sketch::new();
    sk.add_circle(v(0.0, 0.0), r, None, None).unwrap();
    sk.add_circle(v(d, 0.0), r, None, None).unwrap();
    let c = v(d / 2.0, 0.0);
    let (p0, p1) = (c + Vec2::from_angle(PI / 4.0) * r, c + Vec2::from_angle(3.0 * PI / 4.0) * r);
    sk.add_arc(c, p0, p1, [None, None, None], None).unwrap();
    let areas: Vec<f64> = find_profiles(&sk).iter().map(|p| p.area).collect();
    let lens = 2.0 * r * r * (d / (2.0 * r)).acos() - d / 2.0 * (4.0 * r * r - d * d).sqrt();
    // The discs' union once, not each disc whole.
    assert!((areas.iter().sum::<f64>() - (2.0 * PI * r * r - lens)).abs() < 1e-6, "{areas:?}");

    // Two dividers of a square, each carried by a short line 8e-6 off it that does not reach
    // their crossing: they still cross at one node, and the square has four quarters.
    let mut sk = Sketch::new();
    sk.add_line(v(-20.0, 0.0), v(-10.0, 0.0), None, None, None).unwrap();
    sk.add_line(v(0.0, -20.0), v(0.0, -10.0), None, None, None).unwrap();
    rect(&mut sk, [v(-15.0, -15.0), v(15.0, -15.0), v(15.0, 15.0), v(-15.0, 15.0)]);
    sk.add_line(v(-15.0, 8e-6), v(15.0, 8e-6), None, None, None).unwrap();
    sk.add_line(v(8e-6, -15.0), v(8e-6, 15.0), None, None, None).unwrap();
    let areas: Vec<f64> = find_profiles(&sk).iter().map(|p| p.area).collect();
    assert_eq!(areas.len(), 4, "{areas:?}");
    assert!((areas.iter().sum::<f64>() - 900.0).abs() < 1e-6, "{areas:?}");
}

#[test]
fn merging_adjacent_regions() {
    let a =
        solidcraft_geom::Region2 { outer: solidcraft_geom::Loop2::polygon(&[v(0.0, 0.0), v(10.0, 0.0), v(10.0, 10.0), v(0.0, 10.0)]), holes: vec![] };
    let b = solidcraft_geom::Region2 {
        outer: solidcraft_geom::Loop2::polygon(&[v(10.0, 0.0), v(20.0, 0.0), v(20.0, 10.0), v(10.0, 10.0)]),
        holes: vec![],
    };
    let c = solidcraft_geom::Region2 { outer: solidcraft_geom::Loop2::polygon(&[v(50.0, 0.0), v(60.0, 0.0), v(60.0, 10.0)]), holes: vec![] };
    let m = merge_regions(&[a, b, c]);
    assert_eq!(m.len(), 2);
    let mut areas: Vec<f64> = m.iter().map(|r| r.area()).collect();
    areas.sort_by(f64::total_cmp);
    assert!((areas[0] - 50.0).abs() < 1e-9 && (areas[1] - 200.0).abs() < 1e-9, "{areas:?}");
    // The two squares make one rectangle: the outline isn't left split where the shared edge was.
    let sides = m.iter().map(|r| r.outer.segs.len()).collect::<Vec<_>>();
    assert!(sides.contains(&4) && sides.contains(&3), "{sides:?}");
}

/// A sketch holding `text` the way `sketch.text` makes it: one text link of the built-in font's
/// outlines.
fn text_sketch(text: &str, at: Vec2, height: f64) -> Sketch {
    let mut sk = Sketch::new();
    let source = LinkSource::Text { text: text.into(), at, height, angle: 0.0, font: None, style: Default::default(), exprs: Default::default() };
    sk.add_link(LinkKind::Text, source, &text_geometry(text, at, height, 0.0).unwrap()).unwrap();
    sk
}

fn sorted_areas(ps: &[Profile]) -> Vec<f64> {
    let mut a: Vec<f64> = ps.iter().map(|p| p.area).collect();
    a.sort_by(f64::total_cmp);
    a
}

fn assert_areas(got: &[f64], mut want: Vec<f64>) {
    want.sort_by(f64::total_cmp);
    assert_eq!(got.len(), want.len(), "{got:?} against {want:?}");
    for (g, w) in got.iter().zip(&want) {
        assert!((g - w).abs() < 2e-3, "{got:?} against {want:?}");
    }
}

#[test]
fn text_counters_are_holes_of_their_letters_not_profiles_of_their_own() {
    // Areas in mm² at a capital height of 10 mm, summed from the Liberation Sans contours (shoelace
    // formula, quadratic pieces flattened finely): a glyph's outer contour, then its counters.
    //   O: 83.3133 - 47.4584   A: 39.2159 - 9.0647   B: 68.9283 - 12.9336 - 16.6277
    //   8: 56.8394 - 10.6451 - 13.7347
    let (o, o_counter) = (83.3133 - 47.4584, 47.4584);
    let (a, a_counter) = (39.2159 - 9.0647, 9.0647);
    let (b, b_counters) = (68.9283 - 12.9336 - 16.6277, [12.9336, 16.6277]);
    let (eight, eight_counters) = (56.8394 - 10.6451 - 13.7347, [10.6451, 13.7347]);
    let check = |text: &str, letters: Vec<f64>, counters: Vec<f64>| {
        let sk = text_sketch(text, Vec2::ZERO, 10.0);
        let all = find_drawn_profiles(&sk);
        // Every counter is a closed region of its own, next to its letter with the hole in it...
        assert_areas(&sorted_areas(&all), letters.iter().chain(&counters).copied().collect());
        // ...and "all profiles" of text means the letters.
        assert_areas(&sorted_areas(&without_text_counters(&sk, all)), letters);
    };
    check("O", vec![o], vec![o_counter]);
    check("8", vec![eight], eight_counters.to_vec());
    check("AB", vec![a, b], [vec![a_counter], b_counters.to_vec()].concat());
    // A letter without a counter is untouched.
    check("I", vec![13.5557], vec![]);
}

#[test]
fn text_beside_other_curves_is_drawn_geometry() {
    // A 30 x 20 rectangle with an "I" (a 10 mm tall bar, 13.5557 mm² in Liberation Sans) beside it.
    let mut sk = text_sketch("I", v(40.0, 0.0), 10.0);
    for (p, q) in [((0.0, 0.0), (30.0, 0.0)), ((30.0, 0.0), (30.0, 20.0)), ((30.0, 20.0), (0.0, 20.0)), ((0.0, 20.0), (0.0, 0.0))] {
        sk.add_line(v(p.0, p.1), v(q.0, q.1), None, None, None).unwrap();
    }
    assert_areas(&sorted_areas(&find_drawn_profiles(&sk)), vec![13.5557, 600.0]);
    // Projected reference geometry still doesn't count as drawn.
    let square = |s: f64| {
        vec![
            LinkGeom::Line(v(100.0, 0.0), v(100.0 + s, 0.0)),
            LinkGeom::Line(v(100.0 + s, 0.0), v(100.0 + s, s)),
            LinkGeom::Line(v(100.0 + s, s), v(100.0, s)),
            LinkGeom::Line(v(100.0, s), v(100.0, 0.0)),
        ]
    };
    sk.add_link(LinkKind::Project, LinkSource::Origin, &square(5.0)).unwrap();
    assert_eq!(find_profiles(&sk).len(), 3);
    assert_areas(&sorted_areas(&find_drawn_profiles(&sk)), vec![13.5557, 600.0]);
}

#[test]
fn a_letter_merged_with_its_counter_is_the_filled_letter() {
    // The ring of an O and its counter share the counter's two conics; merged, they cancel along it.
    let sk = text_sketch("O", Vec2::ZERO, 10.0);
    let regions: Vec<_> = find_profiles(&sk).into_iter().map(|p| p.region).collect();
    assert_eq!(regions.len(), 2);
    let merged = merge_regions(&regions);
    assert_eq!(merged.len(), 1, "{} regions", merged.len());
    assert!(merged[0].holes.is_empty(), "{} holes", merged[0].holes.len());
    assert!((merged[0].area() - 83.3133).abs() < 2e-3, "{}", merged[0].area());
}

#[test]
fn links_update_in_place_rebuild_and_break() {
    use crate::{LinkGeom, LinkKind, LinkSource};
    let mut sk = Sketch::new();
    let src = LinkSource::Origin;
    let sq = |s: f64| {
        vec![
            LinkGeom::Line(v(0.0, 0.0), v(s, 0.0)),
            LinkGeom::Line(v(s, 0.0), v(s, s)),
            LinkGeom::Line(v(s, s), v(0.0, s)),
            LinkGeom::Line(v(0.0, s), v(0.0, 0.0)),
        ]
    };
    let j = sk.add_link(LinkKind::Project, src, &sq(10.0)).unwrap();
    assert_eq!(sk.link_curves(&j).len(), 4);
    // Corners are shared: 4 points, all fixed.
    let pts = sk.link_points(&j);
    assert_eq!(pts.len(), 4);
    assert!(pts.iter().all(|p| sk.points[*p].fixed));
    // A free line dimensioned to a projected corner keeps its constraint across an update.
    let l = sk.add_line(v(3.0, 3.0), v(5.0, 7.0), None, None, None).unwrap();
    let (la, _) = match sk.curves[l].kind {
        CurveKind::Line { a, b } => (a, b),
        _ => panic!(),
    };
    sk.add_constraint(ConstraintKind::Coincident { p: la, q: pts[2] }, None).unwrap();
    let ids: Vec<String> = sk.link_curves(&j).iter().map(|c| sk.curves[*c].id.clone()).collect();
    assert!(!sk.update_link(&j, &sq(20.0)).unwrap(), "same shape updates in place");
    let ids2: Vec<String> = sk.link_curves(&j).iter().map(|c| sk.curves[*c].id.clone()).collect();
    assert_eq!(ids, ids2);
    assert!(solve(&mut sk).ok());
    assert!(sk.points[la].pos.dist(v(20.0, 20.0)) < 1e-9);
    assert_eq!(find_profiles(&sk).len(), 1);
    // A different shape rebuilds (the coincidence on the old corner goes).
    assert!(sk.update_link(&j, &[LinkGeom::Circle(v(1.0, 1.0), 4.0)]).unwrap());
    assert_eq!(sk.link_curves(&j).len(), 1);
    assert_eq!(sk.constraints.len(), 0);
    // Linked circles keep their radius in the solver.
    let c = sk.link_curves(&j)[0];
    let rep = solve(&mut sk);
    assert!(rep.curve_determined[c], "{rep:?}");
    // Breaking the link frees it.
    sk.break_link(&j).unwrap();
    assert!(sk.links.is_empty());
    assert!(sk.curves[c].link.is_none());
    let rep = solve(&mut sk);
    assert!(!rep.curve_determined[c]);
    // Deleting all of a link's curves removes the link.
    let j2 = sk.add_link(LinkKind::Project, LinkSource::Origin, &[LinkGeom::Line(v(0.0, 0.0), v(1.0, 0.0))]).unwrap();
    let cs = sk.link_curves(&j2);
    sk.remove_curves(&cs);
    assert!(sk.link(&j2).is_none());
    // Nothing to project is an error, not an empty link.
    assert!(sk.add_link(LinkKind::Project, LinkSource::Origin, &[LinkGeom::Line(v(0.0, 0.0), v(0.0, 0.0))]).is_err());
    // Round trip through JSON.
    let j3 = sk.add_link(LinkKind::Intersect, LinkSource::Axis { name: "X".into() }, &[LinkGeom::Point(v(2.0, 2.0))]).unwrap();
    let back: Sketch = serde_json::from_str(&serde_json::to_string(&sk).unwrap()).unwrap();
    assert_eq!(back, sk);
    assert_eq!(back.link(&j3).unwrap().kind, LinkKind::Intersect);
}

/// Many independent constrained rectangles solve quickly (per-component work).
#[test]
fn large_sketch_solves_fast() {
    let mut sk = Sketch::new();
    for i in 0..150 {
        let o = v(i as f64 * 50.0, 0.0);
        let l = rect(&mut sk, [o, o + v(40.0, 0.5), o + v(40.5, 30.0), o + v(-0.3, 30.2)]);
        use ConstraintKind::*;
        sk.add_constraint(Horizontal { l: l[0] }, None).unwrap();
        sk.add_constraint(Horizontal { l: l[2] }, None).unwrap();
        sk.add_constraint(Vertical { l: l[1] }, None).unwrap();
        sk.add_constraint(Vertical { l: l[3] }, None).unwrap();
        sk.add_constraint(Length { l: l[0], value: 40.0 }, None).unwrap();
        sk.add_constraint(Length { l: l[1], value: 30.0 }, None).unwrap();
    }
    let t = std::time::Instant::now();
    let rep = solve(&mut sk);
    let ms = t.elapsed().as_millis();
    assert!(rep.ok());
    assert_eq!(rep.dof, 300);
    assert!(ms < 10_000, "{ms} ms");
    eprintln!("large sketch: {ms} ms");
}

#[test]
fn long_connected_chain_solves() {
    let mut sk = Sketch::new();
    let n = 300;
    let mut prev = 0usize;
    for i in 0..n {
        let p = sk.add_point(v((i + 1) as f64 * 10.0 + 0.3, if i % 2 == 0 { 0.4 } else { -0.2 }), None).unwrap();
        let l = sk.add_line_pts(prev, p, None).unwrap();
        sk.add_constraint(ConstraintKind::Horizontal { l }, None).unwrap();
        sk.add_constraint(ConstraintKind::Length { l, value: 10.0 }, None).unwrap();
        prev = p;
    }
    let t = std::time::Instant::now();
    let rep = solve(&mut sk);
    let ms = t.elapsed().as_millis();
    eprintln!("chain: {ms} ms, iters {}", rep.iterations);
    assert!(rep.ok(), "{rep:?}");
    assert_eq!(rep.dof, 0);
    assert!(ms < 10_000, "{ms} ms");
}

/// Free-form curves stay exact in profiles: a degree-5 spline closed by a line, and a fit spline
/// cut in two by a line through it (each side gets an exact sub-curve ending on the line).
#[test]
fn exact_profiles_for_high_degree_and_cut_free_form_curves() {
    use solidcraft_geom::Seg2;
    let exact_loop = |p: &Profile| p.region.outer.segs.iter().all(|s| !matches!(s, Seg2::Line { .. }) || s.length() > 1.0);
    let closed = |p: &Profile| {
        let s = &p.region.outer.segs;
        s.iter().zip(s.iter().cycle().skip(1)).all(|(a, b)| a.end().dist(b.start()) < 1e-9)
    };
    // Degree 5 over six points, closed by a line.
    let mut sk = Sketch::default();
    let ids: Vec<usize> = [v(0.0, 0.0), v(4.0, 10.0), v(10.0, 12.0), v(16.0, 12.0), v(22.0, 10.0), v(26.0, 0.0)]
        .iter()
        .map(|p| sk.add_point(*p, None).unwrap())
        .collect();
    sk.add_curve(CurveKind::Spline { pts: ids.clone(), control: true, degree: 5, periodic: false }, None).unwrap();
    sk.add_line_pts(ids[5], ids[0], None).unwrap();
    let pr = find_profiles(&sk);
    assert_eq!(pr.len(), 1);
    assert!(pr[0].region.outer.segs.iter().any(|s| matches!(s, Seg2::Bezier { n: 5, .. })), "{:?}", pr[0].region.outer.segs);
    assert!(closed(&pr[0]));
    // A fit spline arch over a base line, cut by a vertical line at x = 10.
    let mut sk = Sketch::default();
    let ids: Vec<usize> = [v(0.0, 0.0), v(6.0, 9.0), v(14.0, 9.0), v(20.0, 0.0)].iter().map(|p| sk.add_point(*p, None).unwrap()).collect();
    sk.add_curve(CurveKind::Spline { pts: ids.clone(), control: false, degree: 3, periodic: false }, None).unwrap();
    sk.add_line_pts(ids[3], ids[0], None).unwrap();
    let (a, b) = (sk.add_point(v(10.0, -5.0), None).unwrap(), sk.add_point(v(10.0, 20.0), None).unwrap());
    sk.add_line_pts(a, b, None).unwrap();
    let pr = find_profiles(&sk);
    assert_eq!(pr.len(), 2, "{pr:?}");
    let whole: f64 = pr.iter().map(|p| p.area).sum();
    for p in &pr {
        assert!(closed(p), "{:?}", p.region.outer.segs);
        assert!(exact_loop(p), "polyline pieces left: {:?}", p.region.outer.segs);
        // The crossing lies on the line x = 10.
        assert!(
            p.region
                .outer
                .segs
                .iter()
                .any(|s| matches!(s, Seg2::Cubic { .. }) && ((s.start().x - 10.0).abs() < 1e-9 || (s.end().x - 10.0).abs() < 1e-9))
        );
    }
    // Same total area as the uncut arch.
    sk.curves.pop();
    let uncut: f64 = find_profiles(&sk).iter().map(|p| p.area).sum();
    assert!((whole - uncut).abs() < 1e-6 * uncut, "{whole} vs {uncut}");
}

/// A face boundary saved with stations along its straight edges (#1049: booleans used to leave
/// vertices nothing else used) keeps its saved pieces when the face comes back with each
/// straight edge whole: the same region, and features that name the pieces still find them.
#[test]
fn projected_boundary_keeps_its_pieces_when_only_stations_go() {
    use crate::{LinkGeom, LinkKind, LinkSource};
    let mut sk = Sketch::new();
    let v = Vec2::new;
    let split = vec![
        LinkGeom::Line(v(0.0, 0.0), v(5.0, 0.0)),
        LinkGeom::Line(v(5.0, 0.0), v(10.0, 0.0)),
        LinkGeom::Line(v(10.0, 0.0), v(10.0, 4.0)),
        LinkGeom::Line(v(10.0, 4.0), v(10.0, 10.0)),
        LinkGeom::Line(v(10.0, 10.0), v(0.0, 10.0)),
        LinkGeom::Line(v(0.0, 10.0), v(0.0, 0.0)),
    ];
    let j = sk.add_link(LinkKind::Project, LinkSource::Origin, &split).unwrap();
    let ids: Vec<String> = sk.link_curves(&j).iter().map(|c| sk.curves[*c].id.clone()).collect();
    let whole = vec![
        LinkGeom::Line(v(0.0, 0.0), v(10.0, 0.0)),
        LinkGeom::Line(v(10.0, 0.0), v(10.0, 10.0)),
        LinkGeom::Line(v(10.0, 10.0), v(0.0, 10.0)),
        LinkGeom::Line(v(0.0, 10.0), v(0.0, 0.0)),
    ];
    assert!(!sk.update_link(&j, &whole).unwrap(), "only the stations went: the pieces stay");
    let kept: Vec<String> = sk.link_curves(&j).iter().map(|c| sk.curves[*c].id.clone()).collect();
    assert_eq!(ids, kept);
    // A boundary that really changed is rebuilt.
    let smaller = vec![
        LinkGeom::Line(v(0.0, 0.0), v(8.0, 0.0)),
        LinkGeom::Line(v(8.0, 0.0), v(8.0, 10.0)),
        LinkGeom::Line(v(8.0, 10.0), v(0.0, 10.0)),
        LinkGeom::Line(v(0.0, 10.0), v(0.0, 0.0)),
    ];
    assert!(sk.update_link(&j, &smaller).unwrap());
    assert_eq!(sk.link_curves(&j).len(), 4);
}

#[test]
fn a_degree_nine_control_spline_stays_exact_and_closes_a_profile_exactly() {
    // A G4 blend curve is one degree-9 Bézier; a sketch keeps it as a control spline whose exact
    // piece is that Bézier, and profile measurement must keep using the curve, not its chords.
    let mut sk = Sketch::new();
    let bump =
        [v(0.0, 0.0), v(1.5, 4.0), v(3.0, 7.0), v(5.0, 9.5), v(7.5, 11.0), v(12.5, 11.0), v(15.0, 9.5), v(17.0, 7.0), v(18.5, 4.0), v(20.0, 0.0)];
    let pts: Vec<usize> = bump.iter().map(|q| sk.add_point(*q, None).unwrap()).collect();
    let s = sk.add_curve(CurveKind::Spline { pts: pts.clone(), control: true, degree: 9, periodic: false }, None).unwrap();
    let pieces = sk.exact_pieces(s);
    assert_eq!(pieces.len(), 1, "{pieces:?}");
    let Seg2::Bezier { n, p } = pieces[0] else { panic!("{:?}", pieces[0]) };
    assert_eq!(n, 9, "the highest degree a sketch piece can carry");
    for (i, q) in bump.iter().enumerate() {
        assert!(p[i].dist(*q) < 1e-12, "control {i}");
    }
    // Evaluation and the exact piece agree at the spline's real degree (a degree clamp would show
    // here), and a line's own exact piece still comes back as a line.
    let exact = pieces[0];
    for k in 0..=10 {
        let t = k as f64 / 10.0;
        assert!(crate::curves::spline_point(&bump, true, 9, t).dist(exact.point_at(t)) < 1e-9, "t={t}");
    }
    let l = sk.add_line(v(-5.0, 0.0), v(-2.0, 1.0), None, None, None).unwrap();
    assert!(matches!(sk.exact_pieces(l).as_slice(), [Seg2::Line { .. }]));
    // Close the loop: the region's area is the shoelace integral of the exact pieces, which a fine
    // sampling of the piece reproduces (a chord approximation would miss the bump's sagitta).
    sk.add_line(v(20.0, 0.0), v(0.0, 0.0), Some(pts[9]), Some(pts[0]), None).unwrap();
    let ps = find_profiles(&sk);
    assert_eq!(ps.len(), 1, "{ps:?}");
    let n = 20_000;
    let mut sampled = 0.0;
    for i in 0..n {
        let (t0, t1) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
        let (a, b) = (exact.point_at(t0), exact.point_at(t1));
        sampled += 0.5 * a.cross(b);
    }
    sampled -= 0.5 * v(20.0, 0.0).cross(v(0.0, 0.0));
    assert!((ps[0].area - sampled.abs()).abs() < 1e-5, "{} vs {sampled}", ps[0].area);
    assert!((ps[0].area - 100.0).abs() > 1e-3, "the region is not its chord: {}", ps[0].area);
    // A degree above the cap is still refused.
    let mut high: Vec<usize> = pts.clone();
    high.push(sk.add_point(v(21.0, -2.0), None).unwrap());
    assert!(sk.add_curve(CurveKind::Spline { pts: high, control: true, degree: 10, periodic: false }, None).is_err());
}

/// #755: a line drawn through a junction of a loop, but 2e-5 off it, still splits the loop. The
/// junction lies on the loop's two curves only, so it cuts neither of them where the line
/// crosses, and that crossing, next to the junction, was taken for one already cut there: the
/// line crossed the loop without meeting it, and the sketch had no profiles at all.
#[test]
fn profiles_line_passing_next_to_a_junction_splits_the_loop() {
    let o = v(0.0, 0.0);
    let at = |r: f64, a: f64| v(r * a.cos(), r * a.sin());
    // A slot: half circles of radius 1 round (0, 0) and (-5, 0), joined by the lines y = 1 and
    // y = -1, which meet the arcs tangentially at (0, 1) and (0, -1).
    let mut sk = Sketch::new();
    let right = sk.add_arc(o, v(0.0, -1.0), v(0.0, 1.0), [None; 3], None).unwrap();
    let left = sk.add_arc(v(-5.0, 0.0), v(-5.0, 1.0), v(-5.0, -1.0), [None; 3], None).unwrap();
    let ends = |i: usize| match sk.curves[i].kind {
        CurveKind::Arc { a, b, .. } => (a, b),
        _ => panic!("an arc"),
    };
    let ((r0, r1), (l0, l1)) = (ends(right), ends(left));
    sk.add_line_pts(r1, l0, None).unwrap();
    sk.add_line_pts(l1, r0, None).unwrap();
    // Lines from (0, 0) through the junctions to an arc of radius 2. The upper one is turned by
    // 2e-5 rad, so it passes 2e-5 from (0, 1) and crosses the line y = 1 next to it.
    let d = 2e-5;
    let (top, bottom) = (at(2.0, FRAC_PI_2 + d), v(0.0, -2.0));
    sk.add_line(o, top, None, None, None).unwrap();
    sk.add_line(o, bottom, None, None, None).unwrap();
    sk.add_arc(o, bottom, top, [None; 3], None).unwrap();
    let mut areas: Vec<f64> = find_profiles(&sk).iter().map(|p| p.area).collect();
    areas.sort_by(f64::total_cmp);
    // The right half disc, the band round it out to radius 2, and the rest of the slot.
    let want = [FRAC_PI_2, FRAC_PI_2 * (4.0 - 1.0), 10.0 + FRAC_PI_2];
    assert_eq!(areas.len(), 3, "{areas:?}");
    for (got, want) in areas.iter().zip(want) {
        assert!((got - want).abs() < 1e-3, "{areas:?} against {want}");
    }
}

/// A line ending 2e-5 from a corner, next to a crossing there: the corner, where the side of the
/// square meets the long line, stays a cut of the long line, and the square stays a profile.
#[test]
fn profiles_crossing_next_to_a_corner_keeps_the_corner() {
    let mut sk = Sketch::new();
    let (a, b) = (sk.add_point(v(1.0, 0.0), None).unwrap(), sk.add_point(v(2.0, 0.0), None).unwrap());
    let (c, d) = (sk.add_point(v(2.0, 1.0), None).unwrap(), sk.add_point(v(1.0, 1.0), None).unwrap());
    // The square's base is part of a longer line, which its sides end on.
    sk.add_line(v(0.0, 0.0), v(3.0, 0.0), None, None, None).unwrap();
    for (p, q) in [(b, c), (c, d), (d, a)] {
        sk.add_line_pts(p, q, None).unwrap();
    }
    // A line hanging across the long one, 2e-5 from the square's corner (1, 0).
    sk.add_line(v(1.0 - 2e-5, -0.5), v(1.0 - 2e-5, 0.5), None, None, None).unwrap();
    let areas: Vec<f64> = find_profiles(&sk).iter().map(|p| p.area).collect();
    assert_eq!(areas.len(), 1, "{areas:?}");
    assert!(areas.iter().all(|x| (x - 1.0).abs() < 1e-9), "{areas:?}");
}

/// A line across a rectangle, and a stray line ending on it 1e-3 from where it crosses the
/// rectangle's side: both the crossing and that end cut the long line, and the rectangle is
/// split into its two halves.
#[test]
fn profiles_crossing_next_to_a_stray_end_still_splits() {
    let mut sk = Sketch::new();
    rect(&mut sk, [v(0.0, -1.0), v(1.0, -1.0), v(1.0, 1.0), v(0.0, 1.0)]);
    sk.add_line(v(-1.0, 0.0), v(2.0, 0.0), None, None, None).unwrap();
    sk.add_line(v(0.00099998, 0.0000099), v(0.3, 0.6), None, None, None).unwrap();
    let areas: Vec<f64> = find_profiles(&sk).iter().map(|p| p.area).collect();
    assert_eq!(areas.len(), 2, "{areas:?}");
    assert!(areas.iter().all(|x| (x - 1.0).abs() < 1e-9), "{areas:?}");
}

/// A line whose own end overshoots a crossing line by 2e-5, with a rectangle standing on it 1e-3
/// further on: the crossing next to that end gives way to the rectangle's corner, which still
/// cuts the line, and the rectangle stays a profile.
#[test]
fn profiles_crossing_next_to_an_overshooting_end_keeps_the_corner_beside_it() {
    let mut sk = Sketch::new();
    sk.add_line(v(-2e-5, 0.0), v(3.0, 0.0), None, None, None).unwrap();
    sk.add_line(v(0.0, -0.5), v(0.0, 0.5), None, None, None).unwrap();
    let x = 0.00099999;
    let p = [v(x, 0.0000099), v(x, 1.0), v(2.0, 1.0), v(2.0, 0.0)].map(|q| sk.add_point(q, None).unwrap());
    for w in p.windows(2) {
        sk.add_line_pts(w[0], w[1], None).unwrap();
    }
    let areas: Vec<f64> = find_profiles(&sk).iter().map(|p| p.area).collect();
    assert_eq!(areas.len(), 1, "{areas:?}");
    assert!(areas.iter().all(|a| (a - (2.0 - x)).abs() < 1e-4), "{areas:?}");
}
