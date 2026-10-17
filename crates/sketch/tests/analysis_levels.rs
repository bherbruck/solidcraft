//! Reduced diagnostics must preserve placement, rollback, feasibility and exact freedom results.

use solidcraft_geom::Vec2;
use solidcraft_sketch::{Analysis, ConstraintKind, Hold, Sketch, solve_with};

fn compare(sketch: &Sketch, hold: &Hold) {
    let mut full_geometry = sketch.clone();
    let mut freedom_geometry = sketch.clone();
    let mut status_geometry = sketch.clone();
    let mut full = solve_with(&mut full_geometry, hold, None, Analysis::Full, None);
    let freedom = solve_with(&mut freedom_geometry, hold, None, Analysis::Freedom, None);
    let status = solve_with(&mut status_geometry, hold, None, Analysis::StatusOnly, None);
    assert_eq!(freedom_geometry, full_geometry);
    assert_eq!(status_geometry, full_geometry);
    assert!(freedom.redundant.is_empty(), "freedom queries must not compute redundant IDs");
    full.redundant.clear();
    assert_eq!(serde_json::to_value(&freedom).unwrap(), serde_json::to_value(&full).unwrap());
    full.dof = 0;
    full.point_determined.clear();
    full.curve_determined.clear();
    assert_eq!(serde_json::to_value(&status).unwrap(), serde_json::to_value(&full).unwrap());
}

#[test]
fn analysis_levels_preserve_results_on_anchored_floating_failed_and_held_geometry() {
    let mut anchored = Sketch::new();
    let q = anchored.add_point(Vec2::new(12.0, 3.0), None).unwrap();
    let k = ConstraintKind::Distance { p: 0, q, value: 10.0 };
    anchored.add_constraint(k.clone(), None).unwrap();
    let redundant = anchored.add_constraint(k, None).unwrap();
    let full = solve_with(&mut anchored.clone(), &Hold::default(), None, Analysis::Full, None);
    assert!(full.ok());
    assert_eq!(full.dof, 1);
    assert!(full.redundant.contains(&redundant));
    compare(&anchored, &Hold::default());
    compare(&anchored, &Hold { points: vec![q], radii: vec![] });

    let mut conflict = anchored.clone();
    conflict.add_constraint(ConstraintKind::Distance { p: 0, q, value: 11.0 }, None).unwrap();
    assert!(!solve_with(&mut conflict.clone(), &Hold::default(), None, Analysis::Full, None).ok());
    compare(&conflict, &Hold::default());

    let mut floating = Sketch::new();
    let l = floating.add_line(Vec2::new(5.0, 5.0), Vec2::new(14.0, 6.0), None, None, None).unwrap();
    floating.add_constraint(ConstraintKind::Horizontal { l }, None).unwrap();
    floating.add_constraint(ConstraintKind::Horizontal { l }, None).unwrap();
    floating.add_constraint(ConstraintKind::Length { l, value: 10.0 }, None).unwrap();
    compare(&floating, &Hold::default());

    let mut blocks = Sketch::new();
    let p = blocks.add_point(Vec2::new(4.0, 1.0), None).unwrap();
    let q = blocks.add_point(Vec2::new(5.0, 2.0), None).unwrap();
    blocks.add_constraint(ConstraintKind::Coincident { p, q }, None).unwrap();
    blocks.add_constraint(ConstraintKind::Coincident { p, q }, None).unwrap();
    let c = blocks.add_circle(Vec2::new(20.0, 30.0), 5.0, None, None).unwrap();
    blocks.add_constraint(ConstraintKind::Radius { c, value: 5.0 }, None).unwrap();
    blocks.add_constraint(ConstraintKind::Radius { c, value: 5.0 }, None).unwrap();
    blocks.add_constraint(ConstraintKind::Distance { p: 0, q: p, value: 999.0 }, None).unwrap();
    blocks.constraints.last_mut().unwrap().driven = true;
    compare(&blocks, &Hold::default());
    compare(&blocks, &Hold { points: vec![p], radii: vec![c] });

    let mut constants = Sketch::new();
    let q = constants.add_point(Vec2::new(5.0, 0.0), None).unwrap();
    constants.points[q].fixed = true;
    constants.add_constraint(ConstraintKind::Distance { p: 0, q, value: 10.0 }, None).unwrap();
    compare(&constants, &Hold::default());
}
