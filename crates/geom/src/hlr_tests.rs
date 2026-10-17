use super::*;
use crate::brep::{Brep, Curve, Edge, EdgeUse, Face, Shell, Surface};

/// An axis-aligned box with line edges and plane faces.
fn cuboid(lo: [f64; 3], hi: [f64; 3]) -> Brep {
    let corner = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
    let vertices: Vec<[f64; 3]> = (0..8).map(corner).collect();
    let pairs = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    let edges: Vec<Edge> =
        pairs.iter().map(|&(a, b)| Edge { vertices: [a as u32, b as u32], curve: Curve::Line { start: corner(a), end: corner(b) } }).collect();
    let cycle = |vs: [usize; 4]| -> Vec<EdgeUse> {
        (0..4)
            .map(|k| {
                let (a, b) = (vs[k], vs[(k + 1) % 4]);
                let (i, fwd) = pairs
                    .iter()
                    .enumerate()
                    .find_map(|(i, &(x, y))| (x == a && y == b).then_some((i, true)).or((x == b && y == a).then_some((i, false))))
                    .unwrap();
                EdgeUse { edge: i as u32, forward: fwd }
            })
            .collect()
    };
    let plane = |o: usize, u: usize, v: usize| {
        let (o, u, v) = (corner(o), corner(u), corner(v));
        Surface::Plane { origin: o, u_axis: [u[0] - o[0], u[1] - o[1], u[2] - o[2]], v_axis: [v[0] - o[0], v[1] - o[1], v[2] - o[2]] }
    };
    let faces = vec![
        Face { loops: vec![cycle([0, 2, 3, 1])], surface: plane(0, 2, 1), forward: true },
        Face { loops: vec![cycle([4, 5, 7, 6])], surface: plane(4, 5, 6), forward: true },
        Face { loops: vec![cycle([0, 1, 5, 4])], surface: plane(0, 1, 4), forward: true },
        Face { loops: vec![cycle([2, 6, 7, 3])], surface: plane(2, 6, 3), forward: true },
        Face { loops: vec![cycle([0, 4, 6, 2])], surface: plane(0, 4, 2), forward: true },
        Face { loops: vec![cycle([1, 3, 7, 5])], surface: plane(1, 3, 5), forward: true },
    ];
    Brep { version: crate::brep::VERSION, shells: vec![Shell { vertices, edges, faces }] }
}

fn identity(body: usize, t: [f64; 3]) -> Placement {
    Placement { body, rotation: [Vec3::new(1., 0., 0.), Vec3::new(0., 1., 0.), Vec3::new(0., 0., 1.)], translation: Vec3::new(t[0], t[1], t[2]) }
}

fn never() -> bool {
    false
}

#[test]
fn iso_cube_hides_the_three_back_edges() {
    let def = prepare(&cuboid([0.; 3], [1.; 3]), &Options::default()).unwrap();
    let view = View::new(Vec3::new(-1., -1., -1.), Vec3::new(0., 0., 1.)).unwrap();
    let d = hidden_lines(&[def], &[identity(0, [0.; 3])], &view, &Options::default(), &never).unwrap();
    let mut hidden = Vec::new();
    for l in &d.lines {
        let Source::Edge { edge, .. } = l.source else { panic!("a box has no silhouettes") };
        let states: Vec<_> = l.pieces.iter().map(|p| p.visibility).collect();
        assert!(states.windows(2).all(|w| w[0] == w[1]), "edge {edge} is split: {states:?}");
        if states[0] == Visibility::Hidden {
            hidden.push(edge);
        }
    }
    hidden.sort_unstable();
    // The edges at the corner nearest the origin: (0,1), (0,2), (0,4).
    assert_eq!(hidden, vec![0, 4, 8], "{d:#?}");
}

#[test]
fn a_box_in_front_hides_the_middle_of_an_edge() {
    let back = prepare(&cuboid([0.; 3], [1.; 3]), &Options::default()).unwrap();
    let front = prepare(&cuboid([0.5, -2., 0.25], [1.5, -1., 0.75]), &Options::default()).unwrap();
    let view = View::new(Vec3::new(0., 1., 0.), Vec3::new(0., 0., 1.)).unwrap();
    let d = hidden_lines(&[back, front], &[identity(0, [0.; 3]), identity(1, [0.; 3])], &view, &Options::default(), &never).unwrap();
    // Edge 9 joins corners 1 (1,0,0) and 5 (1,0,1): t is z.
    let line = d.lines.iter().find(|l| l.placement == 0 && l.source == Source::Edge { edge: 9, tangent: false }).unwrap();
    let got: Vec<([f64; 2], Visibility)> = line.pieces.iter().map(|p| (p.t, p.visibility)).collect();
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(got[1].1, Visibility::Hidden);
    assert!((got[1].0[0] - 0.25).abs() < 1e-12 && (got[1].0[1] - 0.75).abs() < 1e-12, "{got:?}");
    assert_eq!((got[0].1, got[2].1), (Visibility::Visible, Visibility::Visible));
}

#[test]
fn quadratic_roots_with_no_linear_term() {
    // A ray from inside a sphere, square to the radius at its start (b = 0): both roots.
    let mut roots = surface::quadratic(1., 0., -100.);
    roots.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(roots.len(), 2, "{roots:?}");
    assert!((roots[0].0 + 10.).abs() < 1e-12 && (roots[1].0 - 10.).abs() < 1e-12, "{roots:?}");
    let mut roots = surface::quadratic(1., -0., -100.);
    roots.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert!((roots[0].0 + 10.).abs() < 1e-12 && (roots[1].0 - 10.).abs() < 1e-12, "{roots:?}");
}

#[test]
fn a_torus_seen_along_its_axis_has_both_equator_silhouettes() {
    // A whole-meridian torus record: the outer equator sits where the closed profile's ends meet.
    let frame = crate::brep::Frame { origin: [0.; 3], x_axis: [1., 0., 0.], y_axis: [0., 1., 0.] };
    let tau = std::f64::consts::TAU;
    let s = surface::S3::new(&Surface::Torus { frame, major_radius: 20., minor_radius: 5., longitude: [0., tau], meridian: [0., tau] }).unwrap();
    let mut radii: Vec<f64> = s
        .rev_silhouette(Vec3::new(0., 0., -1.))
        .iter()
        .map(|(c, _)| match c {
            curve::C3::Arc { r, .. } => r[0],
            _ => panic!("circles expected"),
        })
        .collect();
    radii.sort_by(f64::total_cmp);
    assert_eq!(radii.len(), 2, "{radii:?}");
    assert!((radii[0] - 15.).abs() < 1e-9 && (radii[1] - 25.).abs() < 1e-9, "{radii:?}");
}

#[test]
fn far_apart_bodies_do_not_merge_nearby_visible_edges() {
    // A 5 mm cube and a box 100 m away: coincidence is judged at the chords' own accuracy,
    // not at a fraction of the scene's size, so none of the cube's edges is taken as a duplicate.
    let near = prepare(&cuboid([0.; 3], [5.; 3]), &Options::default()).unwrap();
    let far = prepare(&cuboid([100_000.; 3], [100_001.; 3]), &Options::default()).unwrap();
    let view = View::new(Vec3::new(-1., -0.7, -0.4), Vec3::new(0., 0., 1.)).unwrap();
    let d = hidden_lines(&[near, far], &[identity(0, [0.; 3]), identity(1, [0.; 3])], &view, &Options::default(), &never).unwrap();
    let covered_visible = d
        .lines
        .iter()
        .filter(|l| l.placement == 0)
        .flat_map(|l| l.pieces.iter())
        .filter(|p| p.visibility == Visibility::Visible && p.covered)
        .count();
    assert_eq!(covered_visible, 0);
}

#[test]
fn a_thin_torus_tube_stops_a_ray() {
    // R 100, r 0.1: a ray across the tube meets it twice within 0.2 mm.
    let roots = surface::torus_roots(Vec3::new(100., -50., 0.), Vec3::new(0., 1., 0.), Vec3::new(0., 0., 1.), 100., 0.1, [0., 200.]);
    assert!(roots.iter().filter(|(_, grazing)| !grazing).count() >= 2, "{roots:?}");
}

#[test]
fn a_thin_tube_on_a_huge_torus_stops_a_ray() {
    // R 100 m, r 1 mm: inside the tube the implicit function stays far below a size^4 floor.
    // A radial ray from 5 mm inside the centre circle crosses the tube at 4 and 6 mm.
    let roots = surface::torus_roots(Vec3::new(100_000. - 5., 0., 0.), Vec3::new(1., 0., 0.), Vec3::new(0., 0., 1.), 100_000., 1., [0., 10.]);
    let hits: Vec<f64> = roots.iter().filter(|(_, grazing)| !grazing).map(|(l, _)| *l).collect();
    // The implicit form cancels terms of 4e20 here: the roots are good to about 1e-6 mm.
    assert!(hits.len() == 2 && (hits[0] - 4.).abs() < 1e-5 && (hits[1] - 6.).abs() < 1e-5, "{roots:?}");
}

#[test]
fn a_hair_thin_tube_on_a_kilometre_torus_stops_a_ray() {
    // R 1 km, r 0.1 mm (R/r = 1e7): roots at 0.9 and 1.1 mm, good to about 1e-4 mm here.
    let roots = surface::torus_roots(Vec3::new(1e6 - 1., 0., 0.), Vec3::new(1., 0., 0.), Vec3::new(0., 0., 1.), 1e6, 0.1, [0., 2.]);
    let hits: Vec<f64> = roots.iter().filter(|(_, grazing)| !grazing).map(|(l, _)| *l).collect();
    assert!(hits.len() == 2 && (hits[0] - 0.9).abs() < 1e-3 && (hits[1] - 1.1).abs() < 1e-3, "{roots:?}");
}

#[test]
fn a_ray_from_the_hole_crosses_a_thin_large_torus() {
    // R 4 m, r 0.01 mm, from the centre outward over 8 m: only the shell around the tube is sampled.
    let roots = surface::torus_roots(Vec3::new(0., 0., 0.), Vec3::new(1., 0., 0.), Vec3::new(0., 0., 1.), 4000., 0.01, [0., 8000.]);
    let hits: Vec<f64> = roots.iter().filter(|(_, grazing)| !grazing).map(|(l, _)| *l).collect();
    assert!(hits.len() == 2 && (hits[0] - 3999.99).abs() < 1e-6 && (hits[1] - 4000.01).abs() < 1e-6, "{roots:?}");
}

#[test]
fn a_torus_seen_nearly_along_its_axis_keeps_its_silhouettes() {
    // Meridian starting off the equator and a view 1e-4 rad from the axis: the silhouette lives
    // in narrow profile intervals around the equators, which must not fall between samples.
    let frame = crate::brep::Frame { origin: [0.; 3], x_axis: [1., 0., 0.], y_axis: [0., 1., 0.] };
    let tau = std::f64::consts::TAU;
    let shift = std::f64::consts::PI / 720.;
    let s =
        surface::S3::new(&Surface::Torus { frame, major_radius: 1000., minor_radius: 500., longitude: [0., tau], meridian: [shift, shift + tau] })
            .unwrap();
    let d = Vec3::new(1e-4, 0., -1.).normalized().unwrap();
    let curves = s.rev_silhouette(d);
    // Both the outer (radius about 1500) and the inner (about 500) silhouettes are there.
    let radii: Vec<f64> = curves
        .iter()
        .flat_map(|(c, _)| {
            let [t0, t1] = c.domain();
            (0..=8).map(move |i| c.at(t0 + (t1 - t0) * i as f64 / 8.))
        })
        .map(|p| p.x.hypot(p.y))
        .collect();
    assert!(radii.iter().any(|r| *r > 1400.) && radii.iter().any(|r| *r < 600.), "{radii:?}");
}

#[test]
fn a_shallow_cut_through_a_torus_tube_is_found() {
    // R 20, r 5, a ray along the axis 0.01 mm inside the outer equator: a 0.63 mm chord.
    let roots = surface::torus_roots(Vec3::new(24.99, 0., -10.), Vec3::new(0., 0., 1.), Vec3::new(0., 0., 1.), 20., 5., [0., 20.]);
    let hits: Vec<f64> = roots.iter().filter(|(_, grazing)| !grazing).map(|(l, _)| *l).collect();
    let half = (25f64 - 4.99f64.powi(2)).sqrt();
    assert!(hits.len() == 2 && (hits[0] - (10. - half)).abs() < 1e-6 && (hits[1] - (10. + half)).abs() < 1e-6, "{roots:?}");
}
