//! Issue #2005: a closed sweep must join its end back to its start, without coincident caps.
//! The capped tool used to stall or fail when cutting the torus it lay inside.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use solidcraft_geom::{Loop2, Plane, Region2, Vec2, Vec3};

use crate::{Body, BoolOp, PathSeg, SweepControl, boolean, measure, sweep_with};

fn square(half: f64) -> Region2 {
    Region2 {
        outer: Loop2::polygon(&[Vec2::new(-half, -half), Vec2::new(half, -half), Vec2::new(half, half), Vec2::new(-half, half)]),
        holes: vec![],
    }
}

fn profile_plane() -> Plane {
    Plane { origin: Vec3::new(30.0, 0.0, 0.0), x: Vec3::X, y: Vec3::Z }
}

fn circle_path() -> [PathSeg; 2] {
    // A sketch circle reaches the kernel as two half arcs.
    [
        PathSeg::Arc { a: Vec3::new(30.0, 0.0, 0.0), center: Vec3::ZERO, axis: Vec3::Z, angle: PI },
        PathSeg::Arc { a: Vec3::new(-30.0, 0.0, 0.0), center: Vec3::ZERO, axis: Vec3::Z, angle: PI },
    ]
}

fn tube(body: &Body, volume: f64, genus: usize) {
    let m = measure(body).unwrap();
    assert!((m.volume - volume).abs() < volume * 0.01, "{m:?}, expected {volume}");
    // A closed orientable tube has Euler characteristic 2 - 2g. Coincident caps gave 2.
    assert_eq!(m.vertices + m.faces + 2 * genus, m.edges + 2, "{m:?}");
    assert!(body.solid.boundaries().iter().all(|s| s.extract_boundaries().is_empty()));
    let h = body.history().expect("sweep provenance");
    assert!(h.faces.iter().all(|f| f.origins.iter().all(|o| matches!(o, crate::history::FaceOrigin::Profile { .. }))));
}

#[test]
fn closed_circle_sweeps_have_no_caps() {
    for twist in [0.0, FRAC_PI_2, PI, TAU, -PI] {
        let ctl = SweepControl { twist, ..SweepControl::default() };
        let b = sweep_with(&profile_plane(), &square(4.0), &circle_path(), &ctl).unwrap();
        tube(&b, 64.0 * TAU * 30.0, 1);
    }
}

#[test]
fn one_full_arc_closes_like_a_sketch_circle() {
    for angle in [TAU, -TAU] {
        let path = [PathSeg::Arc { a: Vec3::new(30.0, 0.0, 0.0), center: Vec3::ZERO, axis: Vec3::Z, angle }];
        let b = sweep_with(&profile_plane(), &square(4.0), &path, &SweepControl::default()).unwrap();
        tube(&b, 64.0 * TAU * 30.0, 1);
    }
}

#[test]
fn closed_polyline_sweep_shares_its_starting_section() {
    let path = crate::path_polyline(&circle_path()).unwrap();
    let b = crate::sweep_path(&profile_plane(), &square(4.0), &path).unwrap();
    tube(&b, 64.0 * TAU * 30.0, 1);
}

#[test]
fn closed_sweep_refuses_an_end_section_that_does_not_match() {
    for ctl in [SweepControl { twist: PI / 4.0, ..SweepControl::default() }, SweepControl { scale: 2.0, ..SweepControl::default() }] {
        let err = sweep_with(&profile_plane(), &square(4.0), &circle_path(), &ctl).unwrap_err();
        assert!(err.to_string().contains("closed path"), "{err}");
    }
}

#[test]
fn closed_holed_sweep_joins_every_loop() {
    let region = Region2 { holes: vec![square(2.0).outer], ..square(4.0) };
    let ctl = SweepControl { twist: PI, ..SweepControl::default() };
    let b = sweep_with(&profile_plane(), &region, &circle_path(), &ctl).unwrap();
    // Two disjoint toroidal shells, one bounding the cavity.
    let m = measure(&b).unwrap();
    assert!((m.volume - 48.0 * TAU * 30.0).abs() < m.volume * 0.02, "{m:?}");
    assert_eq!(m.vertices + m.faces, m.edges, "{m:?}");
}

#[test]
fn half_turn_sweep_cuts_a_torus() {
    let base = crate::torus(Vec3::ZERO, 30.0, 8.0).unwrap();
    let tool = sweep_with(&profile_plane(), &square(4.0), &circle_path(), &SweepControl { twist: PI, ..SweepControl::default() }).unwrap();
    let result = boolean(&base, &tool, BoolOp::Cut).unwrap().expect("cut leaves the torus wall");
    let expected = (PI * 64.0 - 64.0) * TAU * 30.0;
    let m = measure(&result).unwrap();
    assert!((m.volume - expected).abs() < expected * 0.015, "{m:?}, expected {expected}");
}
