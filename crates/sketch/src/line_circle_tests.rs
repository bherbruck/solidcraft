//! The distance from a line to the edge of a circle or arc (#1803): solved numerically and by
//! construction, measured, kept on its side of the line through edits, and refused on other
//! kinds of curve.

use solidcraft_geom::Vec2;

use crate::tests::v;
use crate::*;

/// A fixed line along x from (0, 0) to (40, 0) and a circle of radius `r` at `centre` (its
/// centre free, its radius set by a dimension). The line, the circle and the centre point.
fn line_and_circle(centre: Vec2, r: f64) -> (Sketch, usize, usize, usize) {
    let mut sk = Sketch::new();
    let l = sk.add_line(v(0.0, 0.0), v(40.0, 0.0), None, None, None).unwrap();
    let (a, b) = sk.curves[l].kind.ends().unwrap();
    for p in [a, b] {
        sk.add_constraint(ConstraintKind::Fix { p }, None).unwrap();
    }
    let c = sk.add_circle(centre, r, None, None).unwrap();
    let CurveKind::Circle { c: ci, .. } = sk.curves[c].kind else { panic!("not a circle") };
    sk.add_constraint(ConstraintKind::Radius { c, value: r }, None).unwrap();
    (sk, l, c, ci)
}

fn kind_at(sk: &Sketch, k: usize) -> ConstraintKind {
    sk.constraints[k].kind.clone()
}

#[test]
fn a_line_to_circle_distance_places_the_circle_its_gap_from_the_line() {
    let (mut sk, l, c, ci) = line_and_circle(v(20.0, 15.0), 5.0);
    let k = sk.constraints.len();
    sk.add_constraint(ConstraintKind::LineCircleDistance { l, c, value: 10.0, inside: false }, None).unwrap();
    // As drawn the gap is 15 − 5 = 10: the sketch already holds it.
    assert!((solver::measure_dimension(&sk, &kind_at(&sk, k)).unwrap() - 10.0).abs() < 1e-9);
    assert!(solve(&mut sk).ok());
    // A gap of 7 puts the centre 7 + 5 = 12 from the line, on the side it was drawn on.
    sk.constraints[k].kind.set_value(7.0);
    let rep = solve(&mut sk);
    assert!(rep.ok(), "{rep:?}");
    let centre = sk.points[ci].pos;
    assert!((centre.y - 12.0).abs() < 1e-7, "{centre:?}");
    assert!((solver::measure_dimension(&sk, &kind_at(&sk, k)).unwrap() - 7.0).abs() < 1e-7);
    // Tangent: a gap of zero.
    sk.constraints[k].kind.set_value(0.0);
    assert!(solve(&mut sk).ok());
    assert!((sk.points[ci].pos.y - 5.0).abs() < 1e-7, "{:?}", sk.points[ci].pos);
}

#[test]
fn a_circle_below_the_line_stays_below_it() {
    let (mut sk, l, c, ci) = line_and_circle(v(20.0, -15.0), 5.0);
    let k = sk.constraints.len();
    sk.add_constraint(ConstraintKind::LineCircleDistance { l, c, value: 10.0, inside: false }, None).unwrap();
    assert!(solve(&mut sk).ok());
    sk.constraints[k].kind.set_value(3.0);
    assert!(solve(&mut sk).ok());
    assert!((sk.points[ci].pos.y + 8.0).abs() < 1e-7, "{:?}", sk.points[ci].pos);
}

#[test]
fn a_line_that_cuts_the_circle_measures_how_far_its_edge_reaches_past_it() {
    // Centre 3 above the line, radius 5: the edge reaches 5 − 3 = 2 below the line.
    let (mut sk, l, c, ci) = line_and_circle(v(20.0, 3.0), 5.0);
    let k = sk.constraints.len();
    sk.add_constraint(ConstraintKind::LineCircleDistance { l, c, value: 2.0, inside: true }, None).unwrap();
    assert!((solver::measure_dimension(&sk, &kind_at(&sk, k)).unwrap() - 2.0).abs() < 1e-9);
    assert!(solve(&mut sk).ok());
    // Reaching 4 past the line leaves the centre 5 − 4 = 1 above it.
    sk.constraints[k].kind.set_value(4.0);
    let rep = solve(&mut sk);
    assert!(rep.ok(), "{rep:?}");
    assert!((sk.points[ci].pos.y - 1.0).abs() < 1e-7, "{:?}", sk.points[ci].pos);
}

#[test]
fn an_arc_is_measured_to_its_circle() {
    let mut sk = Sketch::new();
    let l = sk.add_line(v(0.0, 0.0), v(40.0, 0.0), None, None, None).unwrap();
    let a = sk.add_arc(v(20.0, 20.0), v(26.0, 20.0), v(14.0, 20.0), [None; 3], None).unwrap();
    let k = sk.constraints.len();
    sk.add_constraint(ConstraintKind::LineCircleDistance { l, c: a, value: 14.0, inside: false }, None).unwrap();
    // Centre 20 above the line, radius 6.
    assert!((solver::measure_dimension(&sk, &kind_at(&sk, k)).unwrap() - 14.0).abs() < 1e-9);
}

#[test]
fn the_dimension_needs_a_line_and_a_circle_or_arc() {
    let mut sk = Sketch::new();
    let l = sk.add_line(v(0.0, 0.0), v(40.0, 0.0), None, None, None).unwrap();
    let m = sk.add_line(v(0.0, 10.0), v(40.0, 10.0), None, None, None).unwrap();
    let c = sk.add_circle(v(20.0, 20.0), 5.0, None, None).unwrap();
    for (l, c) in [(m, l), (c, c), (c, l)] {
        assert!(sk.add_constraint(ConstraintKind::LineCircleDistance { l, c, value: 1.0, inside: false }, None).is_err(), "{l} {c}");
    }
    for value in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(sk.add_constraint(ConstraintKind::LineCircleDistance { l, c, value, inside: false }, None).is_err(), "{value}");
    }
}

#[test]
fn the_dimension_survives_a_round_trip_and_a_renumbering() {
    let (mut sk, l, c, _) = line_and_circle(v(20.0, 3.0), 5.0);
    sk.add_constraint(ConstraintKind::LineCircleDistance { l, c, value: 2.0, inside: true }, None).unwrap();
    let json = serde_json::to_value(&sk).unwrap();
    let saved = json["constraints"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(saved["type"], "line_circle_distance", "{saved}");
    assert_eq!(saved["inside"], true, "{saved}");
    let back: Sketch = serde_json::from_value(json).unwrap();
    assert_eq!(back.constraints, sk.constraints);
    // `inside` is left out when false.
    let outside = ConstraintKind::LineCircleDistance { l, c, value: 2.0, inside: false };
    assert!(serde_json::to_value(&outside).unwrap().get("inside").is_none());
    let mut k = outside;
    assert!(k.remap(&|p| Some(p), &|i| Some(i + 10)));
    assert_eq!(k.indices(), (vec![], vec![l + 10, c + 10]));
    assert!(!k.remap(&|p| Some(p), &|_| None));
}

/// With the centre's x set too, the seed places the centre exactly (its offset line from the
/// dimension crossing the line of its x), before the numerical solver runs.
#[test]
fn the_seed_places_the_centre_by_construction() {
    for (drawn, inside, value, want) in [(v(20.0, 15.0), false, 7.0, v(20.0, 12.0)), (v(20.0, 3.0), true, 4.0, v(20.0, 1.0))] {
        let (mut sk, l, c, ci) = line_and_circle(drawn, 5.0);
        let (a, _) = sk.curves[l].kind.ends().unwrap();
        sk.add_constraint(ConstraintKind::DistanceX { p: a, q: ci, value: 20.0 }, None).unwrap();
        sk.add_constraint(ConstraintKind::LineCircleDistance { l, c, value, inside }, None).unwrap();
        let out = crate::tests::seed_of(&sk);
        assert!(out.placed_by[ci].is_some_and(crate::construct::Placement::anchors), "{out:?}");
        assert!(out.pos[ci].unwrap().dist(want) < 1e-9, "{out:?}");
        let rep = solve(&mut sk);
        assert!(rep.ok() && rep.dof == 0, "{rep:?}");
        assert!(sk.points[ci].pos.dist(want) < 1e-9, "{:?}", sk.points[ci].pos);
    }
}
