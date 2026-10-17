//! Cursor distance is minimized within the design's freedom; lengths stay mandatory.
#[path = "../examples/support/four_bar.rs"]
mod four_bar;

use serde_json::json;
use solidcraft_geom::Vec2;

fn drag(s: &mut solidcraft_engine::Session, target: Vec2) {
    let from = four_bar::point(s, "crank.end").unwrap();
    s.execute("sketch.drag", &json!({"entity":"crank.end","from":[from.x,from.y],"to":[target.x,target.y],"live":true})).unwrap();
}

#[test]
fn four_bar_large_off_locus_drag_reaches_the_closest_feasible_point() {
    for target in [Vec2::new(-10.0, 8.0), Vec2::new(-10.0, -8.0), Vec2::new(20.0, 20.0), Vec2::new(20.0, 0.0), Vec2::new(1000.0, -1000.0)] {
        let mut s = four_bar::session().unwrap();
        let side = four_bar::branch(&s).unwrap();
        drag(&mut s, target);
        let actual = four_bar::point(&s, "crank.end").unwrap();
        let nearest = target * (10.0 / target.len());
        assert!(actual.dist(nearest) < 1e-4, "target {target:?}, actual {actual:?}, expected {nearest:?}");
        assert!(four_bar::length_error(&s).unwrap() < 1e-6);
        assert_eq!(four_bar::branch(&s).unwrap(), side);
        let state = s.model.state();
        let report = &state.sketch(s.active_sketch.unwrap()).unwrap().report;
        assert!(report.ok());
        assert_eq!(report.dof, 1);
        s.execute("sketch.drag_end", &json!({})).unwrap();
        assert!(four_bar::point(&s, "crank.end").unwrap().dist(nearest) < 1e-4);
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(four_bar::point(&s, "crank.end").unwrap().dist(Vec2::new(-10.0, 0.0)) < 1e-7);
    }
}

#[test]
fn four_bar_reverse_motion_and_stationary_cursor_do_not_drift() {
    let mut s = four_bar::session().unwrap();
    let side = four_bar::branch(&s).unwrap();
    for target in [Vec2::new(-10.0, 16.0), Vec2::new(-10.0, -16.0), Vec2::new(0.0, 25.0), Vec2::new(0.0, -25.0)] {
        drag(&mut s, target);
        let at = four_bar::point(&s, "crank.end").unwrap();
        assert!(
            at.dist(target * (10.0 / target.len())) < 1e-4,
            "target {target:?}, actual {at:?}, length error {}",
            four_bar::length_error(&s).unwrap()
        );
        for _ in 0..8 {
            drag(&mut s, target);
            assert!(four_bar::point(&s, "crank.end").unwrap().dist(at) < 1e-4);
            assert!(four_bar::length_error(&s).unwrap() < 1e-6);
            assert_eq!(four_bar::branch(&s).unwrap(), side);
        }
    }
}

#[test]
fn four_bar_one_event_and_split_moves_agree_and_keep_the_branch() {
    let target = Vec2::new(-10.0, 16.0);
    let mut single = four_bar::session().unwrap();
    let mut split = four_bar::session().unwrap();
    let start = four_bar::point(&single, "crank.end").unwrap();
    let side = four_bar::branch(&single).unwrap();
    drag(&mut single, target);
    for i in 1..=16 {
        drag(&mut split, start + (target - start) * (i as f64 / 16.0));
        assert!(four_bar::length_error(&split).unwrap() < 1e-6);
        assert_eq!(four_bar::branch(&split).unwrap(), side);
    }
    assert!(four_bar::point(&single, "crank.end").unwrap().dist(four_bar::point(&split, "crank.end").unwrap()) < 1e-4);
}

#[test]
fn four_bar_live_cursor_motion_updates_driven_dimensions_before_release() {
    let mut s = four_bar::session().unwrap();
    s.execute("sketch.dimension", &json!({"entities": ["crank"], "type": "horizontal", "driven": true})).unwrap();
    let id = s.active_sketch.unwrap();
    let dimension = s.doc.sketch(id).unwrap().constraints.last().unwrap().id.clone();
    drag(&mut s, Vec2::new(-10.0, 8.0));
    let at = four_bar::point(&s, "crank.end").unwrap();
    assert!(at.dist(Vec2::new(-10.0, 8.0) * (10.0 / 164.0_f64.sqrt())) < 1e-4);
    let state = s.model.state();
    for sketch in [s.doc.sketch(id).unwrap(), &state.sketch(id).unwrap().sketch] {
        let c = sketch.constraints.iter().find(|c| c.id == dimension).unwrap();
        assert!(c.driven);
        assert!((c.kind.value().unwrap() - at.x.abs()).abs() < 1e-6, "reference dimension must measure the final live pose: {:?}", c.kind);
    }
}

#[test]
fn four_bar_other_joint_follows_the_rocker_arc_and_its_limits() {
    let base = Vec2::new(42.0, 0.0);
    let lo = -(-1264.0_f64 / 1680.0).acos();
    let hi = -(0.2_f64).acos();
    for angle in [-2.6_f64, -2.1, -1.5, -1.1] {
        let mut s = four_bar::session().unwrap();
        let side = four_bar::branch(&s).unwrap();
        let from = four_bar::point(&s, "coupler.end").unwrap();
        let target = base + Vec2::new(angle.cos(), angle.sin()) * 60.0;
        let nearest = base + Vec2::new(angle.clamp(lo, hi).cos(), angle.clamp(lo, hi).sin()) * 20.0;
        s.execute("sketch.drag", &json!({"entity":"coupler.end","from":[from.x,from.y],"to":[target.x,target.y],"live":true})).unwrap();
        assert!(four_bar::point(&s, "coupler.end").unwrap().dist(nearest) < 1e-4, "angle {angle}");
        assert!(four_bar::length_error(&s).unwrap() < 1e-6);
        assert_eq!(four_bar::branch(&s).unwrap(), side);
        s.execute("sketch.drag_end", &json!({})).unwrap();
        assert!(four_bar::point(&s, "coupler.end").unwrap().dist(nearest) < 1e-4);
    }
}
