//! Guide rails on path sweeps (issue #771), checked against analytic solids: a rail that closes
//! in on the path scales a circle into a cone frustum, a stretching rail widens a square one way
//! only, a helical rail twists a square, two rails carry a line between them; and rails the
//! sweep cannot follow are refused.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::PI;

use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2, Vec3};

use crate::{RailFit, SweepControl, measure, sweep_path_with};

fn circle(r: f64) -> Region2 {
    Region2 { outer: Loop2::circle(Vec2::ZERO, r), holes: Vec::new() }
}

fn square(h: f64) -> Region2 {
    Region2 { outer: Loop2::polygon(&[Vec2::new(-h, -h), Vec2::new(h, -h), Vec2::new(h, h), Vec2::new(-h, h)]), holes: Vec::new() }
}

/// The Z axis from 0 to 40.
fn up() -> Vec<Vec3> {
    vec![Vec3::ZERO, Vec3::new(0.0, 0.0, 40.0)]
}

fn railed(rails: Vec<Vec<Vec3>>, fit: RailFit) -> SweepControl {
    SweepControl { rails, rail_fit: fit, ..SweepControl::default() }
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs()
}

/// A rail from the circle's edge at radius 10 in to radius 5 at the top scales the circle into
/// a frustum: volume πh(R² + Rr + r²)/3, lateral area π(R + r) times the slant height.
#[test]
fn a_rail_closing_in_scales_a_circle_into_a_frustum() {
    let rail = vec![Vec3::new(10.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 40.0)];
    let b = sweep_path_with(&Plane::XY, &circle(10.0), &up(), &railed(vec![rail], RailFit::Scale)).unwrap();
    let m = measure(&b).unwrap();
    let volume = PI * 40.0 / 3.0 * (100.0 + 50.0 + 25.0);
    let area = PI * 15.0 * (40.0f64 * 40.0 + 25.0).sqrt() + PI * 100.0 + PI * 25.0;
    assert!(close(m.volume, volume, 2e-3), "volume {} vs {volume}", m.volume);
    assert!(close(m.area, area, 2e-3), "area {} vs {area}", m.area);
    assert!(close(m.bbox.max.z, 40.0, 1e-9), "{:?}", m.bbox);
}

/// A stretching rail from x = 5 out to x = 10 doubles the square's width along X by the top and
/// leaves its depth along Y: sections 10 × 10k with k = 1 + z/40, volume 100 · 40 · 1.5.
#[test]
fn a_stretching_rail_widens_a_square_one_way() {
    let rail = vec![Vec3::new(5.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 40.0)];
    let b = sweep_path_with(&Plane::XY, &square(5.0), &up(), &railed(vec![rail], RailFit::Stretch)).unwrap();
    let m = measure(&b).unwrap();
    assert!(close(m.volume, 6000.0, 1e-3), "volume {}", m.volume);
    assert!((m.bbox.max.x - 10.0).abs() < 1e-3 && (m.bbox.min.x + 10.0).abs() < 1e-3, "{:?}", m.bbox);
    assert!((m.bbox.max.y - 5.0).abs() < 1e-3 && (m.bbox.min.y + 5.0).abs() < 1e-3, "{:?}", m.bbox);
}

/// The same rail scaling evenly: the square grows both ways, sections 100k², volume
/// 100 ∫ (1 + z/40)² dz = 100 · 40 · 7/3.
#[test]
fn a_scaling_rail_grows_a_square_both_ways() {
    let rail = vec![Vec3::new(5.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 40.0)];
    let b = sweep_path_with(&Plane::XY, &square(5.0), &up(), &railed(vec![rail], RailFit::Scale)).unwrap();
    let m = measure(&b).unwrap();
    let volume = 100.0 * 40.0 * 7.0 / 3.0;
    assert!(close(m.volume, volume, 2e-3), "volume {} vs {volume}", m.volume);
    assert!((m.bbox.max.y - 10.0).abs() < 1e-3, "{:?}", m.bbox);
}

/// A helical rail from the square's corner, a quarter turn about the path at the corner's
/// radius, twists the square a quarter turn: the volume stays 4000, halfway up the corners stand
/// on the axes (the box reaches 5√2), and at the top the corner is at (−5, 5).
#[test]
fn a_helical_rail_twists_a_square() {
    let r = 50f64.sqrt();
    let rail: Vec<Vec3> = (0..=200)
        .map(|i| {
            let u = i as f64 / 200.0;
            let a = PI / 4.0 + u * PI / 2.0;
            Vec3::new(r * a.cos(), r * a.sin(), 40.0 * u)
        })
        .collect();
    let b = sweep_path_with(&Plane::XY, &square(5.0), &up(), &railed(vec![rail], RailFit::Scale)).unwrap();
    let m = measure(&b).unwrap();
    assert!(close(m.volume, 4000.0, 2e-3), "volume {}", m.volume);
    assert!((m.bbox.max.x - r).abs() < 0.05 && (m.bbox.max.y - r).abs() < 0.05, "{:?}", m.bbox);
    // The edge running up from the corner (5, 5) ends a quarter turn on, at (−5, 5).
    let edges = b.edges(0.01).unwrap();
    let ends = |e: &crate::EdgeInfo| (e.points.first().copied().unwrap(), e.points.last().copied().unwrap());
    let joins = |p: Vec3, q: Vec3| {
        edges.iter().any(|e| {
            let (a, b) = ends(e);
            (a.dist(p) < 1e-3 && b.dist(q) < 1e-3) || (a.dist(q) < 1e-3 && b.dist(p) < 1e-3)
        })
    };
    assert!(joins(Vec3::new(5.0, 5.0, 0.0), Vec3::new(-5.0, 5.0, 40.0)), "{:?}", edges.iter().map(ends).collect::<Vec<_>>());
    assert!(!joins(Vec3::new(5.0, 5.0, 0.0), Vec3::new(5.0, 5.0, 40.0)));
}

/// Along a quarter circle of radius 30, a rail keeping 5 mm out from the path (the circle's
/// outer point, on a quarter circle of radius 35) leaves the circle as it is: a quarter torus,
/// volume πr² · R · π/2 (Pappus).
#[test]
fn a_parallel_rail_along_a_curved_path_keeps_the_section() {
    let arc = |radius: f64| -> Vec<Vec3> {
        (0..=180)
            .map(|i| {
                let a = PI / 2.0 * i as f64 / 180.0;
                Vec3::new(30.0 - radius * a.cos(), 0.0, radius * a.sin())
            })
            .collect()
    };
    let b = sweep_path_with(&Plane::XY, &circle(5.0), &arc(30.0), &railed(vec![arc(35.0)], RailFit::Scale)).unwrap();
    let m = measure(&b).unwrap();
    let volume = PI * 25.0 * 30.0 * PI / 2.0;
    assert!(close(m.volume, volume, 2e-3), "volume {} vs {volume}", m.volume);
}

/// Two rails bulging apart along arcs (radius 25 about (∓5, 0, 20), through x = ±10 at z = 0
/// and z = 40) carry a line between them: the sheet lies in the XZ plane between the arcs, area
/// 2(a√(R² − a²) + R² asin(a/R)) − 400 with a = 20.
#[test]
fn two_rails_carry_a_line_between_arcs() {
    let arc = |side: f64| -> Vec<Vec3> {
        (0..=400)
            .map(|i| {
                let z = 40.0 * i as f64 / 400.0;
                let x = -5.0 + (625.0 - (z - 20.0) * (z - 20.0)).sqrt();
                Vec3::new(side * x, 0.0, z)
            })
            .collect()
    };
    let line = vec![vec![Seg2::Line { a: Vec2::new(-10.0, 0.0), b: Vec2::new(10.0, 0.0) }]];
    for fit in [RailFit::Scale, RailFit::Stretch] {
        let ctl = railed(vec![arc(-1.0), arc(1.0)], fit);
        let b = crate::sweep_sheet_points_with(&Plane::XY, &line, &up(), &ctl).unwrap();
        let m = measure(&b).unwrap();
        let area = 2.0 * (20.0 * 15.0 + 625.0 * (0.8f64).asin()) - 400.0;
        assert!(close(m.area, area, 1e-3), "{fit:?}: area {} vs {area}", m.area);
        assert!(m.bbox.min.y.abs() < 1e-6 && m.bbox.max.y.abs() < 1e-6, "{fit:?}: {:?}", m.bbox);
    }
}

/// Rails the sweep cannot follow are refused with a reason, never a panic: three rails, a rail
/// with a bad point, a rail with a twist or scale besides, a rail starting on the path, a rail
/// that stops short, rails along a path with a corner.
#[test]
fn rails_the_sweep_cannot_follow_are_refused() {
    let rail = vec![Vec3::new(10.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 40.0)];
    let refuse = |ctl: SweepControl, path: &[Vec3], why: &str| {
        let e = sweep_path_with(&Plane::XY, &circle(10.0), path, &ctl).map(|_| ()).unwrap_err().to_string();
        assert!(e.contains(why), "expected `{why}` in `{e}`");
    };
    refuse(railed(vec![rail.clone(); 3], RailFit::Scale), &up(), "one or two guide rails");
    refuse(railed(vec![vec![Vec3::ZERO, Vec3::new(f64::NAN, 0.0, 1.0)]], RailFit::Scale), &up(), "finite points");
    refuse(SweepControl { twist: 1.0, ..railed(vec![rail.clone()], RailFit::Scale) }, &up(), "leave the orientation");
    refuse(SweepControl { scale: 2.0, ..railed(vec![rail.clone()], RailFit::Scale) }, &up(), "leave the orientation");
    refuse(railed(vec![vec![Vec3::ZERO, Vec3::new(5.0, 0.0, 40.0)]], RailFit::Scale), &up(), "starts on the path");
    refuse(railed(vec![vec![Vec3::new(10.0, 0.0, 0.0), Vec3::new(8.0, 0.0, 20.0)]], RailFit::Scale), &up(), "whole length");
    refuse(railed(vec![rail.clone(), rail.clone()], RailFit::Scale), &up(), "start at the same point");
    let bent = vec![Vec3::ZERO, Vec3::new(0.0, 0.0, 20.0), Vec3::new(20.0, 0.0, 20.0)];
    refuse(railed(vec![vec![Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 30.0)]], RailFit::Scale), &bent, "sharp corners");
}
