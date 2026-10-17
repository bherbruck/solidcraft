//! Open and untied ends of profile curves.

use solidcraft_geom::Vec2;

use crate::{ConstraintKind, EndTouch, Sketch, find_profiles, loose_ends};

fn v(x: f64, y: f64) -> Vec2 {
    Vec2::new(x, y)
}

/// A closed polyline whose corners are each one shared point.
fn shared_square(sk: &mut Sketch) -> Vec<usize> {
    let pts: Vec<usize> = [v(0.0, 0.0), v(10.0, 0.0), v(10.0, 10.0), v(0.0, 10.0)].iter().map(|p| sk.add_point(*p, None).unwrap()).collect();
    (0..4).map(|i| sk.add_line_pts(pts[i], pts[(i + 1) % 4], None).unwrap()).collect()
}

/// A square drawn as four separate lines: every corner is two points at one place.
fn loose_square(sk: &mut Sketch) -> Vec<usize> {
    let c = [v(0.0, 0.0), v(10.0, 0.0), v(10.0, 10.0), v(0.0, 10.0)];
    (0..4).map(|i| sk.add_line(c[i], c[(i + 1) % 4], None, None, None).unwrap()).collect()
}

#[test]
fn shared_corners_are_tied() {
    let mut sk = Sketch::new();
    shared_square(&mut sk);
    assert!(loose_ends(&sk).is_empty(), "{:?}", loose_ends(&sk));
}

#[test]
fn corners_together_by_position_only_are_untied() {
    let mut sk = Sketch::new();
    loose_square(&mut sk);
    // The square closes (profiles are read from positions)...
    assert_eq!(find_profiles(&sk).len(), 1);
    // ...but nothing keeps it closed: four corners, each reported once.
    let r = loose_ends(&sk);
    assert!(r.open.is_empty(), "{r:?}");
    assert_eq!(r.untied.len(), 4, "{r:?}");
    assert!(r.untied.iter().all(|u| matches!(u.touches, EndTouch::Point(_))), "{r:?}");
}

#[test]
fn coincident_constraints_tie_corners() {
    let mut sk = Sketch::new();
    let lines = loose_square(&mut sk);
    for i in 0..4 {
        let end = sk.curves[lines[i]].kind.ends().unwrap().1;
        let start = sk.curves[lines[(i + 1) % 4]].kind.ends().unwrap().0;
        sk.add_constraint(ConstraintKind::Coincident { p: end, q: start }, None).unwrap();
    }
    assert!(loose_ends(&sk).is_empty(), "{:?}", loose_ends(&sk));
}

#[test]
fn a_gap_is_two_open_ends() {
    let mut sk = Sketch::new();
    let a = sk.add_point(v(0.0, 0.0), None).unwrap();
    let b = sk.add_point(v(10.0, 0.0), None).unwrap();
    let c = sk.add_point(v(10.0, 10.0), None).unwrap();
    sk.add_line_pts(a, b, None).unwrap();
    sk.add_line_pts(b, c, None).unwrap();
    // The closing line stops 0.01 mm short of the start.
    let d = sk.add_point(v(0.0, 0.01), None).unwrap();
    sk.add_line_pts(c, d, None).unwrap();
    assert!(find_profiles(&sk).is_empty());
    let r = loose_ends(&sk);
    assert_eq!(r.open, vec![a, d], "{r:?}");
    assert!(r.untied.is_empty(), "{r:?}");
}

#[test]
fn an_end_on_another_curve_is_untied_until_held() {
    let mut sk = Sketch::new();
    shared_square(&mut sk);
    // A divider from the bottom side's middle to the top side's middle, by position only.
    let l = sk.add_line(v(5.0, 0.0), v(5.0, 10.0), None, None, None).unwrap();
    assert_eq!(find_profiles(&sk).len(), 2);
    let r = loose_ends(&sk);
    assert_eq!(r.untied.len(), 2, "{r:?}");
    assert!(r.untied.iter().all(|u| u.curve == l && matches!(u.touches, EndTouch::Curve(_))), "{r:?}");
    // Held on the sides, they are tied.
    for u in r.untied {
        if let EndTouch::Curve(c) = u.touches {
            sk.add_constraint(ConstraintKind::PointOnCurve { p: u.point, c }, None).unwrap();
        }
    }
    assert!(loose_ends(&sk).is_empty(), "{:?}", loose_ends(&sk));
}

#[test]
fn construction_lines_and_circles_have_no_loose_ends() {
    let mut sk = Sketch::new();
    let l = sk.add_line(v(0.0, 0.0), v(10.0, 0.0), None, None, None).unwrap();
    sk.curves[l].construction = true;
    sk.add_circle(v(20.0, 0.0), 3.0, None, None).unwrap();
    assert!(loose_ends(&sk).is_empty(), "{:?}", loose_ends(&sk));
}

#[test]
fn a_lone_line_has_two_open_ends() {
    let mut sk = Sketch::new();
    let l = sk.add_line(v(0.0, 0.0), v(10.0, 0.0), None, None, None).unwrap();
    let (a, b) = sk.curves[l].kind.ends().unwrap();
    assert_eq!(loose_ends(&sk).open, vec![a.min(b), a.max(b)]);
}

#[test]
fn three_ends_meeting_by_position_are_one_place_tied_at_once() {
    let mut sk = Sketch::new();
    for to in [v(0.0, 0.0), v(20.0, 0.0), v(10.0, 10.0)] {
        sk.add_line(to, v(10.0, 0.0), None, None, None).unwrap();
    }
    let r = loose_ends(&sk);
    // Two of the three need tying, both to the third.
    assert_eq!(r.untied.len(), 2, "{r:?}");
    let anchor = match r.untied[0].touches {
        EndTouch::Point(q) => q,
        EndTouch::Curve(_) => usize::MAX,
    };
    assert!(r.untied.iter().all(|u| u.touches == EndTouch::Point(anchor) && u.point != anchor), "{r:?}");
    for u in &r.untied {
        sk.add_constraint(ConstraintKind::Coincident { p: u.point, q: anchor }, None).unwrap();
    }
    assert!(loose_ends(&sk).untied.is_empty(), "{:?}", loose_ends(&sk));
}
