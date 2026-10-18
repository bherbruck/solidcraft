use crate::*;

fn cube_mesh(s: f64) -> Mesh {
    let p = |x: f64, y: f64, z: f64| Vec3::new(x * s, y * s, z * s);
    let positions = vec![p(0., 0., 0.), p(1., 0., 0.), p(1., 1., 0.), p(0., 1., 0.), p(0., 0., 1.), p(1., 0., 1.), p(1., 1., 1.), p(0., 1., 1.)];
    let triangles =
        vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7]];
    let n = triangles.len();
    Mesh {
        normals: vec![Vec3::Z; 8],
        positions,
        triangles,
        tri_face: vec![0; n],
        edges: vec![],
        seams: vec![],
        smooth_edges: vec![],
        sharp_edges: vec![],
        edge_faces: vec![],
    }
}

#[test]
fn display_edge_masks_keep_identity_and_break_only_certified_sharp_patches() {
    let mut mesh =
        Mesh { edges: vec![vec![Vec3::ZERO, Vec3::Z]; 2], edge_faces: vec![vec![0, 1], vec![1, 2]], seams: vec![true, true], ..Mesh::default() };
    let stored = serde_json::to_value(&mesh).unwrap();
    assert_eq!(mesh.surface_patch(0), vec![0, 1, 2]);
    mesh.smooth_edges = vec![true, false];
    assert_eq!(mesh.surface_patch(0), vec![0, 1, 2]);
    assert!(!mesh.shaded_edge_visible(0));
    mesh.smooth_edges[0] = false;
    mesh.sharp_edges = vec![true, false];
    assert!(mesh.shaded_edge_visible(0));
    assert!(mesh.edge_pickable(0));
    assert!(!mesh.edge_pickable(1));
    assert_eq!(mesh.surface_patch(0), vec![0]);
    assert_eq!(mesh.surface_patch(1), vec![1, 2]);
    assert_eq!(mesh.edges.len(), 2);
    assert_eq!(mesh.face_edges(1), vec![0, 1]);
    assert_eq!(serde_json::to_value(&mesh).unwrap(), stored);
    let reopened: Mesh = serde_json::from_value(stored).unwrap();
    assert!(reopened.smooth_edges.is_empty() && reopened.sharp_edges.is_empty());
    assert!(Mesh::default().shaded_edge_visible(0));
}

#[test]
fn cube_measure() {
    let m = cube_mesh(2.0).measure();
    assert!((m.volume - 8.0).abs() < 1e-12);
    assert!((m.area - 24.0).abs() < 1e-12);
    assert!(m.centroid.dist(Vec3::new(1.0, 1.0, 1.0)) < 1e-12);
}

#[test]
fn raycast_hits_front_face() {
    let m = cube_mesh(2.0);
    let (t, _) = m.raycast(Vec3::new(1.0, 1.0, 10.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
    assert!((t - 8.0).abs() < 1e-9);
    assert!(m.raycast(Vec3::new(5.0, 5.0, 10.0), Vec3::new(0.0, 0.0, -1.0)).is_none());
}

/// A ray through a vertex or edge that triangles share never slips between them: a grid of
/// triangles on a square, hit at every inner vertex and edge midpoint from oblique directions
/// (on the square's own border nothing is shared, and a ray grazing it may miss).
#[test]
fn raycast_never_slips_between_triangles() {
    let n = 6;
    let positions: Vec<Vec3> =
        (0..=n).flat_map(|j| (0..=n).map(move |i| Vec3::new(f64::from(i) * 40.0 / 6.0 + 100.0, f64::from(j) * 5.0, 20.0))).collect();
    let id = |i: u32, j: u32| j * (n + 1) + i;
    let mut triangles = Vec::new();
    for j in 0..n {
        for i in 0..n {
            triangles.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
            triangles.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    let tri_face = vec![0; triangles.len()];
    let m = Mesh { positions, triangles, tri_face, ..Default::default() };
    let dirs = [Vec3::new(-0.57735, 0.57735, -0.57735), Vec3::new(0.3, -0.7, -0.648), Vec3::new(-0.123456789, -0.2, -1.0), Vec3::new(0.0, 0.0, -1.0)];
    let mut targets = Vec::new();
    for j in 1..2 * n {
        for i in 1..2 * n {
            targets.push(Vec3::new(f64::from(i) * 20.0 / 6.0 + 100.0, f64::from(j) * 2.5, 20.0));
        }
    }
    for d in dirs {
        for p in &targets {
            let o = *p - d * 50.0;
            let hit = m.raycast(o, d);
            assert!(hit.is_some_and(|(t, _)| (t - 50.0).abs() < 1e-6), "{p:?} along {d:?}: {hit:?}");
        }
    }
}

#[test]
fn planes() {
    assert_eq!(Plane::XY.normal(), Vec3::Z);
    assert_eq!(Plane::XZ.normal(), Vec3::Y);
    assert_eq!(Plane::YZ.normal(), Vec3::X);
    let p = Plane::XZ.to_world(Vec2::new(3.0, 4.0));
    assert_eq!(p, Vec3::new(3.0, 0.0, -4.0));
    assert_eq!(Plane::XZ.to_local(p), Vec2::new(3.0, 4.0));
    let hit = Plane::XY.offset(5.0).intersect_ray(Vec3::new(1.0, 2.0, 10.0), Vec3::new(0.0, 0.0, -2.0)).unwrap();
    assert!(hit.dist(Vec3::new(1.0, 2.0, 5.0)) < 1e-12);
    assert!(Plane::new(Vec3::ZERO, Vec3::X, Vec3::X).is_none());
    let f = Plane::from_normal(Vec3::ZERO, Vec3::new(0.0, 0.0, 3.0)).unwrap();
    assert!((f.normal() - Vec3::Z).len() < 1e-12);
    assert!(Plane::from_normal(Vec3::ZERO, Vec3::new(f64::NAN, 0.0, 1.0)).is_none());
}

#[test]
fn vec_ops_and_serde() {
    let a = Vec3::new(1.0, 2.0, 3.0);
    assert_eq!(a.cross(Vec3::X), Vec3::new(0.0, 3.0, -2.0));
    assert_eq!(Vec2::new(3.0, 4.0).len(), 5.0);
    assert!(Vec2::ZERO.normalized().is_none());
    assert!(Vec3::new(0.5, 0.0, 0.0).dist_to_segment(Vec3::ZERO, Vec3::X) < 1e-12);
    assert_eq!(serde_json::to_string(&a).unwrap(), "[1.0,2.0,3.0]");
    let b: Vec2 = serde_json::from_str("[1,2]").unwrap();
    assert_eq!(b, Vec2::new(1.0, 2.0));
}

#[test]
fn aabb() {
    let mut b = Aabb3::EMPTY;
    assert!(b.is_empty());
    b.add(Vec3::new(1.0, 2.0, 3.0));
    b.add(Vec3::new(f64::NAN, 0.0, 0.0));
    b.add(Vec3::new(-1.0, 0.0, 0.0));
    assert_eq!(b.size(), Vec3::new(2.0, 2.0, 3.0));
}

#[test]
fn tangent_chains_follow_smooth_edges() {
    // A line, then an arc leaving it tangentially, then a sharp corner.
    let mut m = Mesh { positions: vec![Vec3::ZERO, Vec3::new(30.0, 30.0, 0.0)], ..Default::default() };
    m.edges.push(vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0)]);
    let arc: Vec<Vec3> = (0..=16)
        .map(|k| {
            let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::FRAC_PI_2 * k as f64 / 16.0;
            Vec3::new(10.0 + 5.0 * a.cos(), 5.0 + 5.0 * a.sin(), 0.0)
        })
        .collect();
    m.edges.push(arc);
    m.edges.push(vec![Vec3::new(15.0, 5.0, 0.0), Vec3::new(5.0, 5.0, 0.0)]);
    m.edge_faces = vec![vec![0], vec![0, 1], vec![1]];
    let mut c = m.tangent_chain(0, 5f64.to_radians());
    c.sort();
    assert_eq!(c, vec![0, 1]);
    assert_eq!(m.tangent_chain(2, 5f64.to_radians()), vec![2]);
    assert_eq!(m.face_edges(1), vec![1, 2]);
}

#[test]
fn circular_rims_group_only_closed_cocircular_edges() {
    let arc = |start: f64, sweep: f64, yscale: f64| -> Vec<Vec3> {
        (0..=32)
            .map(|i| {
                let a = start + sweep * i as f64 / 32.0;
                Vec3::new(10.0 * a.cos(), yscale * 10.0 * a.sin(), 5.0)
            })
            .collect()
    };
    let pi = std::f64::consts::PI;
    let mut m = Mesh {
        positions: vec![Vec3::new(-10.0, -10.0, 5.0), Vec3::new(10.0, 10.0, 5.0)],
        edges: vec![arc(0.0, pi, 1.0), arc(2.0 * pi, -pi, 1.0)],
        ..Default::default()
    };
    assert_eq!(m.circular_rim(0), vec![0, 1]);
    assert_eq!(m.circular_rim(1), vec![0, 1]);
    // A hidden seam cannot form a selectable rim.
    m.seams = vec![false, true];
    assert_eq!(m.circular_rim(0), vec![0]);
    m.seams.clear();
    m.edges[1] = arc(pi, pi / 2.0, 1.0);
    assert_eq!(m.circular_rim(0), vec![0]);
    // A closed, smooth ellipse is not a circle.
    m.edges = vec![arc(0.0, pi, 0.7), arc(pi, pi, 0.7)];
    assert_eq!(m.circular_rim(0), vec![0]);
    assert_eq!(m.circular_rim(99), vec![99]);
}

#[test]
fn surface_patches_cross_only_hidden_seams() {
    let mut m = Mesh { edge_faces: vec![vec![0, 1], vec![1, 2], vec![2, 3]], seams: vec![true, true, false], ..Default::default() };
    assert_eq!(m.surface_patch(0), vec![0, 1, 2]);
    assert_eq!(m.surface_patch(2), vec![0, 1, 2]);
    assert_eq!(m.surface_patch(3), vec![3]);
    m.seams.clear();
    assert_eq!(m.surface_patch(0), vec![0]);
}

#[test]
fn cubic_and_conic_segments_measure_exactly() {
    use crate::{Loop2, Seg2, Vec2};
    // An ellipse (a = 5, b = 3) as four quarter conics: area π a b.
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let (a, b) = (5.0, 3.0);
    let q = |p0: Vec2, c: Vec2, p1: Vec2| Seg2::Conic { a: p0, apex: c, b: p1, w };
    let e = Loop2 {
        segs: vec![
            q(Vec2::new(a, 0.0), Vec2::new(a, b), Vec2::new(0.0, b)),
            q(Vec2::new(0.0, b), Vec2::new(-a, b), Vec2::new(-a, 0.0)),
            q(Vec2::new(-a, 0.0), Vec2::new(-a, -b), Vec2::new(0.0, -b)),
            q(Vec2::new(0.0, -b), Vec2::new(a, -b), Vec2::new(a, 0.0)),
        ],
    };
    assert!((e.signed_area() - std::f64::consts::PI * a * b).abs() < 1e-9, "{}", e.signed_area());
    for s in &e.segs {
        let m = s.point_at(0.37);
        assert!(((m.x / a).powi(2) + (m.y / b).powi(2) - 1.0).abs() < 1e-12);
    }
    // A cubic on a straight line has the line's length; reversing flips the area sign.
    let c = Seg2::Cubic { p0: Vec2::new(0.0, 0.0), p1: Vec2::new(1.0, 0.0), p2: Vec2::new(2.0, 0.0), p3: Vec2::new(3.0, 0.0) };
    assert!((c.length() - 3.0).abs() < 1e-12);
    // Square with one bulging cubic side: area = 1 + the cubic's area over the chord.
    let bulge = Seg2::Cubic { p0: Vec2::new(1.0, 0.0), p1: Vec2::new(1.5, 1.0 / 3.0), p2: Vec2::new(1.5, 2.0 / 3.0), p3: Vec2::new(1.0, 1.0) };
    let sq = Loop2 {
        segs: vec![
            Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(1.0, 0.0) },
            bulge,
            Seg2::Line { a: Vec2::new(1.0, 1.0), b: Vec2::new(0.0, 1.0) },
            Seg2::Line { a: Vec2::new(0.0, 1.0), b: Vec2::new(0.0, 0.0) },
        ],
    };
    // x(t) - 1 = 1.5 t(1-t) (Bernstein: 3·0.5·t(1-t)²+3·0.5·t²(1-t)), y = t: ∫ 1.5 t(1-t) dt = 0.25.
    assert!((sq.signed_area() - 1.25).abs() < 1e-12, "{}", sq.signed_area());
    let rev = bulge.reversed();
    assert!((rev.area_term() + bulge.area_term()).abs() < 1e-12);
    assert!(bulge.polyline(1e-4).len() > 4);
    // Halves cover the same curve.
    let (l, r) = bulge.split_half();
    assert!(l.point_at(0.5).dist(bulge.point_at(0.25)) < 1e-12);
    assert!(r.point_at(0.5).dist(bulge.point_at(0.75)) < 1e-12);
    // Conic halves are re-parameterised but stay on the ellipse.
    let (l2, r2) = e.segs[0].split_half();
    for t in [0.1, 0.5, 0.9] {
        for q in [l2.point_at(t), r2.point_at(t)] {
            assert!(((q.x / a).powi(2) + (q.y / b).powi(2) - 1.0).abs() < 1e-12, "{q:?}");
        }
    }
    for (seg, (l, r)) in [(bulge, (l, r)), (e.segs[0], (l2, r2))] {
        assert!(l.end().dist(r.start()) < 1e-12);
        assert!((l.area_term() + r.area_term() - seg.area_term()).abs() < 1e-12);
    }
}

#[test]
fn split_at_and_closest_param() {
    use crate::{Seg2, Vec2};
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let quarter = Seg2::Conic { a: Vec2::new(10.0, 0.0), apex: Vec2::new(10.0, 10.0), b: Vec2::new(0.0, 10.0), w };
    for t in [0.2, 0.5, 0.8] {
        let (l, r) = quarter.split_at(t);
        assert!(l.end().dist(quarter.point_at(t)) < 1e-12 && r.start().dist(l.end()) < 1e-12);
        for s in [0.3, 0.7] {
            assert!((l.point_at(s).len() - 10.0).abs() < 1e-12 && (r.point_at(s).len() - 10.0).abs() < 1e-12);
        }
        assert!((l.area_term() + r.area_term() - quarter.area_term()).abs() < 1e-12);
    }
    let c = Seg2::Cubic { p0: Vec2::new(0.0, 0.0), p1: Vec2::new(1.0, 2.0), p2: Vec2::new(3.0, 2.0), p3: Vec2::new(4.0, 0.0) };
    let q = c.point_at(0.37);
    assert!((c.closest_param(q + Vec2::new(0.0, 0.0)) - 0.37).abs() < 1e-9);
    let (l, r) = c.split_at(0.37);
    assert!(l.end().dist(q) < 1e-12 && r.start().dist(q) < 1e-12);
    assert!((quarter.closest_param(Vec2::new(20.0, 20.0)) - 0.5).abs() < 1e-9);
}

/// An axis-aligned box as a closed mesh, with `n` x `n` quads on each side.
fn box_mesh(min: Vec3, max: Vec3, n: usize) -> Mesh {
    let mut m = Mesh::default();
    let s = max - min;
    let at = |c: [f64; 3]| min + Vec3::new(s.x * c[0], s.y * c[1], s.z * c[2]);
    for axis in 0..3 {
        for side in [0.0, 1.0] {
            for i in 0..n {
                for j in 0..n {
                    let corner = |a: usize, b: usize| {
                        let mut c = [0.0; 3];
                        c[axis] = side;
                        c[(axis + 1) % 3] = a as f64 / n as f64;
                        c[(axis + 2) % 3] = b as f64 / n as f64;
                        at(c)
                    };
                    let k = m.positions.len() as u32;
                    m.positions.extend([corner(i, j), corner(i + 1, j), corner(i + 1, j + 1), corner(i, j + 1)]);
                    m.triangles.extend(if side > 0.0 { [[k, k + 1, k + 2], [k, k + 2, k + 3]] } else { [[k, k + 2, k + 1], [k, k + 3, k + 2]] });
                }
            }
        }
    }
    m
}

#[test]
fn meshes_cross_only_where_their_surfaces_pass_through_each_other() {
    let cube = |x: f64, y: f64, n: usize| box_mesh(Vec3::new(x, y, 0.0), Vec3::new(x + 10.0, y + 10.0, 10.0), n);
    let a = cube(0.0, 0.0, 1);
    // A corner 0.15 deep in the other cube (0.02 % of its volume), and a deep overlap.
    for (x, y, n) in [(9.85, 3.0, 1), (9.85, 3.0, 7), (9.85, 9.85, 2), (5.0, 5.0, 3)] {
        assert!(a.crosses(&cube(x, y, n), 0.01), "{x} {y} {n}");
        assert!(cube(x, y, n).crosses(&a, 0.01), "{x} {y} {n}");
    }
    // Apart, touching on a face or an edge, and the same body.
    for (x, y, n) in [(10.5, 0.0, 1), (10.0, 0.0, 1), (10.0, 3.0, 5), (10.0, 10.0, 2), (0.0, 0.0, 1), (0.0, 0.0, 4)] {
        assert!(!a.crosses(&cube(x, y, n), 0.01), "{x} {y} {n}");
    }
    // One inside the other, standing on a face, and sharing a face plane from inside.
    let inner = box_mesh(Vec3::new(2.0, 2.0, 2.0), Vec3::new(8.0, 8.0, 8.0), 2);
    let on_top = box_mesh(Vec3::new(2.0, 2.0, 10.0), Vec3::new(8.0, 8.0, 14.0), 2);
    let flush = box_mesh(Vec3::new(2.0, 2.0, 4.0), Vec3::new(8.0, 8.0, 10.0), 2);
    assert!(!a.crosses(&inner, 0.01) && !inner.crosses(&a, 0.01));
    assert!(!a.crosses(&on_top, 0.01) && !a.crosses(&flush, 0.01));
    // A slab overlap between faces that are parallel or in one plane has no crossing.
    assert!(!a.crosses(&cube(9.85, 0.0, 1), 0.01));
    // Passing through by less than the tolerance doesn't count.
    assert!(!a.crosses(&cube(9.995, 3.0, 1), 0.01));
    assert!(!a.crosses(&cube(9.85, 3.0, 1), f64::NAN));
    // Crossings left out where the caller knows better: only those, by where they are.
    let corner = cube(9.85, 9.85, 2);
    assert!(!a.crosses_except(&corner, 0.01, |_| true));
    assert!(!a.crosses_except(&corner, 0.01, |p| p.x > 9.8 && p.y > 9.8));
    assert!(a.crosses_except(&corner, 0.01, |p| p.x < 9.8));
}

/// A closed box mesh `[x0, x0+a] x [y0, y0+b] x [z0, z0+c]`, outward-facing.
fn offset_box(o: Vec3, a: f64, b: f64, c: f64) -> Mesh {
    let p = |x: f64, y: f64, z: f64| o + Vec3::new(x * a, y * b, z * c);
    let positions = vec![p(0., 0., 0.), p(1., 0., 0.), p(1., 1., 0.), p(0., 1., 0.), p(0., 0., 1.), p(1., 0., 1.), p(1., 1., 1.), p(0., 1., 1.)];
    let triangles =
        vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7]];
    Mesh { positions, triangles, ..Default::default() }
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs().max(1e-12)
}

#[test]
fn mass_properties_of_a_box_match_the_closed_form() {
    let (a, b, c) = (20.0, 30.0, 40.0);
    let m = offset_box(Vec3::ZERO, a, b, c).mass_properties().expect("a closed box has mass properties");
    let v = a * b * c;
    assert!(close(m.volume, v, 1e-12), "{m:?}");
    assert!((m.centroid - Vec3::new(10.0, 15.0, 20.0)).len() < 1e-9, "{m:?}");
    let i = m.inertia;
    assert!(close(i[0][0], v * (b * b + c * c) / 12.0, 1e-9), "{i:?}");
    assert!(close(i[1][1], v * (a * a + c * c) / 12.0, 1e-9), "{i:?}");
    assert!(close(i[2][2], v * (a * a + b * b) / 12.0, 1e-9), "{i:?}");
    for (r, cc) in [(0, 1), (0, 2), (1, 2)] {
        assert!(i[r][cc].abs() < 1e-6 && i[cc][r].abs() < 1e-6, "{i:?}");
    }
}

#[test]
fn mass_properties_of_a_cylinder_are_close() {
    // A cylinder of radius 10 and height 30 along z, 256 sides, capped by fans.
    let (r, h, n) = (10.0, 30.0, 256usize);
    let mut positions = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, h)];
    for k in 0..n {
        let t = std::f64::consts::TAU * k as f64 / n as f64;
        positions.push(Vec3::new(r * t.cos(), r * t.sin(), 0.0));
        positions.push(Vec3::new(r * t.cos(), r * t.sin(), h));
    }
    let mut triangles = Vec::new();
    for k in 0..n as u32 {
        let (b0, t0) = (2 + 2 * k, 3 + 2 * k);
        let (b1, t1) = (2 + 2 * ((k + 1) % n as u32), 3 + 2 * ((k + 1) % n as u32));
        triangles.push([0, b1, b0]);
        triangles.push([1, t0, t1]);
        triangles.push([b0, b1, t1]);
        triangles.push([b0, t1, t0]);
    }
    let m = Mesh { positions, triangles, ..Default::default() }.mass_properties().expect("closed");
    let v = std::f64::consts::PI * r * r * h;
    assert!(close(m.volume, v, 1e-3), "{m:?}");
    assert!(close(m.inertia[2][2], m.volume * r * r / 2.0, 1e-3), "{m:?}");
    assert!(close(m.inertia[0][0], m.volume * (3.0 * r * r + h * h) / 12.0, 1e-3), "{m:?}");
}

#[test]
fn mass_properties_are_translation_invariant_about_the_centroid() {
    let at_origin = offset_box(Vec3::ZERO, 20.0, 30.0, 40.0).mass_properties().unwrap();
    let moved = offset_box(Vec3::new(100.0, -50.0, 7.0), 20.0, 30.0, 40.0).mass_properties().unwrap();
    assert!((moved.centroid - Vec3::new(110.0, -35.0, 27.0)).len() < 1e-9, "{moved:?}");
    for r in 0..3 {
        for c in 0..3 {
            assert!((moved.inertia[r][c] - at_origin.inertia[r][c]).abs() <= 1e-9 * at_origin.inertia[0][0], "{r}{c}");
        }
    }
}

#[test]
fn mass_properties_of_an_inward_facing_box_are_positive() {
    let mut m = offset_box(Vec3::ZERO, 2.0, 3.0, 4.0);
    for t in &mut m.triangles {
        t.swap(1, 2);
    }
    let p = m.mass_properties().unwrap();
    assert!(close(p.volume, 24.0, 1e-12) && p.inertia[0][0] > 0.0, "{p:?}");
}

#[test]
fn mass_properties_of_an_open_or_empty_mesh_are_none() {
    let mut open = offset_box(Vec3::ZERO, 20.0, 30.0, 40.0);
    open.triangles.truncate(10);
    assert!(open.mass_properties().is_none(), "a box missing a face has no mass");
    assert!(Mesh::default().mass_properties().is_none());
}

#[test]
fn mass_properties_need_consistently_oriented_faces() {
    // Every edge still has two triangles, but one face of the box points inwards: its signed
    // volume would cancel part of the rest.
    let mut m = offset_box(Vec3::ZERO, 20.0, 30.0, 40.0);
    for t in m.triangles.iter_mut().take(2) {
        t.swap(1, 2);
    }
    assert!(m.mass_properties().is_none());
}

mod close_pairs {
    use super::*;

    /// A small deterministic generator, so the random cases below repeat from run to run.
    struct Lcg(u64);

    impl Lcg {
        fn unit(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }

        fn point(&mut self) -> Vec3 {
            Vec3::new(self.unit(), self.unit(), self.unit())
        }
    }

    fn boxed(tri: [Vec3; 3]) -> BoxedTriangle {
        let mut bounds = Aabb3::EMPTY;
        for p in tri {
            bounds.add(p);
        }
        (tri, bounds)
    }

    /// `count` small triangles scattered over a cube of side `spread` at `origin`. On the lattice their
    /// corners sit on whole numbers, so many touch exactly, share edges or lie in one plane.
    fn scatter(rng: &mut Lcg, count: usize, origin: Vec3, spread: f64, size: f64, lattice: bool) -> Vec<BoxedTriangle> {
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let centre = origin + rng.point() * spread;
            let mut tri = [Vec3::ZERO; 3];
            for corner in &mut tri {
                let mut p = centre + (rng.point() - Vec3::new(0.5, 0.5, 0.5)) * (2.0 * size);
                if lattice {
                    p = Vec3::new(p.x.round(), p.y.round(), p.z.round());
                }
                *corner = p;
            }
            out.push(boxed(tri));
        }
        out
    }

    /// A pair that "meets" for these tests: two corners within `gap` of each other.
    fn corners_meet(t: &[Vec3; 3], u: &[Vec3; 3], gap: f64) -> bool {
        t.iter().any(|p| u.iter().any(|q| p.dist(*q) <= gap))
    }

    fn boxes_close(x: &Aabb3, y: &Aabb3, gap: f64) -> bool {
        x.min.x <= y.max.x + gap
            && y.min.x <= x.max.x + gap
            && x.min.y <= y.max.y + gap
            && y.min.y <= x.max.y + gap
            && x.min.z <= y.max.z + gap
            && y.min.z <= x.max.z + gap
    }

    /// Every pair tried, which is how the contact tests worked before they used a grid.
    fn every_pair(a: &[BoxedTriangle], b: &[BoxedTriangle], gap: f64) -> bool {
        a.iter().any(|(x, xb)| b.iter().any(|(y, yb)| boxes_close(xb, yb, gap) && corners_meet(x, y, gap)))
    }

    /// A flat grid of `n` x `n` squares, two triangles each, at height `z`.
    fn flat_grid(n: usize, z: f64) -> Vec<BoxedTriangle> {
        let p = |i: usize, j: usize| Vec3::new(i as f64, j as f64, z);
        let mut out = Vec::with_capacity(2 * n * n);
        for i in 0..n {
            for j in 0..n {
                out.push(boxed([p(i, j), p(i + 1, j), p(i + 1, j + 1)]));
                out.push(boxed([p(i, j), p(i + 1, j + 1), p(i, j + 1)]));
            }
        }
        out
    }

    /// The grid hands `test` the pairs that trying every pair finds: on random triangles, on
    /// triangles that touch exactly (whole-number corners) and far from the origin.
    #[test]
    fn the_grid_finds_the_pairs_that_trying_every_pair_finds() {
        let mut rng = Lcg(0x232);
        let (mut met, mut apart) = (0, 0);
        for case in 0..600usize {
            let lattice = case % 3 == 0;
            let origin = if case % 5 == 0 { Vec3::new(1.0e6, -2.0e6, 3.0e6) } else { Vec3::ZERO };
            let gap = [1e-9, 0.01, 0.2][(case / 3) % 3];
            let (na, nb) = (1 + (rng.unit() * 150.0) as usize, 1 + (rng.unit() * 150.0) as usize);
            let shift = (rng.point() - Vec3::new(0.5, 0.5, 0.5)) * 8.0;
            let size = 0.3 + rng.unit();
            let a = scatter(&mut rng, na, origin, 5.0, size, lattice);
            let b = scatter(&mut rng, nb, origin + shift, 5.0, size, lattice);
            let want = every_pair(&a, &b, gap);
            let mut budget = usize::MAX;
            let got = find_close_pair(&a, &b, gap, &mut budget, |(x, _), (y, _)| corners_meet(x, y, gap));
            assert_eq!(got, Some(want), "case {case}: {na} x {nb} triangles, gap {gap}, lattice {lattice}");
            if want {
                met += 1;
            } else {
                apart += 1;
            }
        }
        assert!(met > 50 && apart > 50, "the cases should cover both answers: {met} meet, {apart} apart");
        let mut budget = usize::MAX;
        assert_eq!(find_close_pair(&[], &flat_grid(2, 0.0), 0.1, &mut budget, |_, _| true), Some(false));
        assert_eq!(find_close_pair(&flat_grid(2, 0.0), &[], 0.1, &mut budget, |_, _| true), Some(false));
    }

    /// Two flat meshes of 16,200 triangles each, one above the other: all their boxes overlap, which
    /// is 262 million pairs and far past a budget of 40 million.
    #[test]
    fn big_meshes_with_overlapping_boxes_are_searched_without_trying_every_pair() {
        let (low, high) = (flat_grid(90, 0.0), flat_grid(90, 0.5));
        assert!(low.len() * high.len() > 200_000_000);
        let mut budget = 40_000_000;
        assert_eq!(find_close_pair(&low, &high, 0.1, &mut budget, |(x, _), (y, _)| corners_meet(x, y, 0.1)), Some(false));
        let mut budget = 40_000_000;
        assert_eq!(find_close_pair(&low, &high, 0.6, &mut budget, |(x, _), (y, _)| corners_meet(x, y, 0.6)), Some(true));
    }

    /// A pile of triangles in one place is still a product of pairs: the budget refuses it.
    #[test]
    fn a_pile_of_triangles_in_one_place_runs_out_of_budget() {
        let pile = |height: f64| {
            let up = Vec3::Z * height;
            vec![boxed([up, Vec3::X + up, Vec3::Y + up]); 7000]
        };
        let (low, high) = (pile(0.0), pile(0.5));
        let mut budget = 40_000_000;
        // Their boxes are 0.5 apart, so within a gap of 1 every pair is a candidate: 49 million tests.
        assert_eq!(find_close_pair(&low, &high, 1.0, &mut budget, |_, _| false), None);
    }

    /// The grid's own work counts: a budget too small to file a big mesh is refused even when no
    /// pair is ever tested.
    #[test]
    fn filing_and_visiting_cells_count_against_the_budget() {
        let (low, high) = (flat_grid(30, 0.0), flat_grid(30, 5.0));
        let mut budget = 100;
        assert_eq!(find_close_pair(&low, &high, 0.1, &mut budget, |_, _| false), None);
        let mut budget = 10_000_000;
        assert_eq!(find_close_pair(&low, &high, 0.1, &mut budget, |_, _| false), Some(false));
        assert!(budget < 10_000_000, "work was counted");
    }
}

mod blend {
    use super::*;

    /// `j!` for the small orders the Hermite and quotient identities use.
    fn fact(j: usize) -> f64 {
        (1..=j).fold(1.0, |a, b| a * b as f64)
    }

    /// The derivatives 1…`order` a Bézier's control polygon carries at its end `at_end`: the j-th
    /// difference of the controls, scaled by `n!/(n − j)!`. This is the textbook endpoint identity
    /// of a Bézier, written here independently of the library's.
    fn control_derivatives(ctrl: &[Vec2], at_end: bool, order: usize) -> Vec<Vec2> {
        let c: Vec<Vec2> = if at_end { ctrl.iter().rev().copied().collect() } else { ctrl.to_vec() };
        let n = c.len() - 1;
        let (mut diffs, mut scale) = (c, 1.0);
        let mut out = Vec::new();
        for j in 1..=order {
            diffs = diffs.windows(2).map(|w| w[1] - w[0]).collect();
            scale *= (n + 1 - j) as f64;
            out.push(diffs.first().copied().unwrap_or_default() * scale);
        }
        // The reversed controls describe `P(1 − s)`, so reversing again to read the original
        // parameterisation negates the odd orders.
        if at_end {
            for (i, v) in out.iter_mut().enumerate() {
                if i % 2 == 0 {
                    *v = -*v;
                }
            }
        }
        out
    }

    #[test]
    fn hermite_bezier_meets_the_derivative_data_at_both_ends() {
        let (p0, p1) = (Vec2::new(1.0, 2.0), Vec2::new(-3.0, 4.5));
        let d0 = [Vec2::new(2.0, 1.0), Vec2::new(-0.5, 3.0), Vec2::new(1.5, 1.0), Vec2::new(0.25, -2.0)];
        let d1 = [Vec2::new(-1.0, 0.5), Vec2::new(2.0, -1.5), Vec2::new(-0.75, 0.5), Vec2::new(3.0, 1.25)];
        for k in 0..=MAX_DERIVATIVE_ORDER {
            let cps = hermite_bezier(p0, p1, &d0[..k], &d1[..k]).unwrap();
            assert_eq!(cps.len(), 2 * k + 2, "degree 2k+1 has 2k+2 controls");
            assert_eq!(cps[0], p0);
            assert_eq!(*cps.last().unwrap(), p1);
            for (got, want) in control_derivatives(&cps, false, k).iter().zip(&d0[..k]) {
                assert!(got.dist(*want) < 1e-11, "k={k} at t=0: {got:?} vs {want:?}");
            }
            for (got, want) in control_derivatives(&cps, true, k).iter().zip(&d1[..k]) {
                assert!(got.dist(*want) < 1e-11, "k={k} at t=1: {got:?} vs {want:?}");
            }
            // The piece is exactly the single Bézier `bezier` hands back, and its ends are the two
            // joints.
            let seg = bezier(&cps).unwrap();
            assert!(seg.start().dist(p0) < 1e-12 && seg.end().dist(p1) < 1e-12);
            for (got, want) in seg.derivatives(0.0, k).unwrap().iter().zip(&d0[..k]) {
                assert!(got.dist(*want) < 1e-10, "k={k} at t=0: {got:?} vs {want:?}");
            }
        }
    }

    #[test]
    fn hermite_bezier_refuses_hostile_data() {
        let (p0, p1) = (Vec2::ZERO, Vec2::X);
        let d = [Vec2::Y, Vec2::X];
        assert!(hermite_bezier(p0, p1, &d, &d).is_some());
        assert!(hermite_bezier(p0, p1, &d, &d[..1]).is_none(), "the ends must agree on the order");
        assert!(hermite_bezier(p0, p1, &[Vec2::ZERO; 5], &[Vec2::ZERO; 5]).is_none(), "order above G4");
        assert!(hermite_bezier(Vec2::new(f64::NAN, 0.0), p1, &[], &[]).is_none());
        assert!(hermite_bezier(p0, p1, &[Vec2::new(f64::INFINITY, 0.0)], &[Vec2::Y]).is_none());
        assert!(hermite_bezier(p0, p1, &[], &[Vec2::Y]).is_none());
    }

    #[test]
    fn seg_derivatives_are_exact_for_lines_and_polynomial_pieces() {
        let line = Seg2::Line { a: Vec2::new(-1.0, 2.0), b: Vec2::new(3.0, 5.0) };
        let d = line.derivatives(0.0, MAX_DERIVATIVE_ORDER).unwrap();
        assert!(d[0].dist(Vec2::new(4.0, 3.0)) < 1e-12);
        assert_eq!(d[1].len() + d[2].len() + d[3].len(), 0.0, "a line has no curvature");
        let cubic = Seg2::Cubic { p0: Vec2::new(0.0, 0.0), p1: Vec2::new(3.0, 9.0), p2: Vec2::new(7.0, -4.0), p3: Vec2::new(11.0, 2.0) };
        let ctrl = [Vec2::new(0.0, 0.0), Vec2::new(3.0, 9.0), Vec2::new(7.0, -4.0), Vec2::new(11.0, 2.0)];
        for (t, at_end) in [(0.0, false), (1.0, true)] {
            for (got, want) in cubic.derivatives(t, MAX_DERIVATIVE_ORDER).unwrap().iter().zip(control_derivatives(&ctrl, at_end, 4)) {
                assert!(got.dist(want) < 1e-9, "t={t}: {got:?} vs {want:?}");
            }
        }
        for t in [0.0, 0.35, 1.0] {
            assert!(cubic.derivatives(t, 1).unwrap()[0].dist(cubic.derivative(t)) < 1e-9, "t={t}");
        }
        // A degree-9 piece (the G4 blend's own shape) at both ends.
        let pts: Vec<Vec2> = (0..10).map(|i| Vec2::new(i as f64 * 0.7, (i as f64 * 1.3).sin())).collect();
        let seg = bezier(&pts).unwrap();
        assert!(matches!(seg, Seg2::Bezier { n: 9, .. }));
        for (t, at_end) in [(0.0, false), (1.0, true)] {
            for (got, want) in seg.derivatives(t, MAX_DERIVATIVE_ORDER).unwrap().iter().zip(control_derivatives(&pts, at_end, 4)) {
                assert!(got.dist(want) < 1e-9, "t={t}: {got:?} vs {want:?}");
            }
        }
    }

    #[test]
    fn seg_derivatives_follow_the_circular_recurrence() {
        let arc = Seg2::Arc { center: Vec2::new(2.0, -1.0), radius: 5.0, start: 0.7, sweep: -1.9 };
        let Seg2::Arc { center, sweep, .. } = arc else { panic!("an arc") };
        for t in [0.0, 0.4, 1.0] {
            let d = arc.derivatives(t, MAX_DERIVATIVE_ORDER).unwrap();
            let p = arc.point_at(t);
            let sw2 = sweep * sweep;
            // On a circle every derivative follows from the second: P″ = −sw²(P − c), and the rest
            // differentiate that relation, so a line and an arc have zero curvature rates.
            assert!(d[1].dist((p - center) * -sw2) < 1e-9, "t={t}");
            assert!(d[2].dist(d[0] * -sw2) < 1e-9, "t={t}");
            assert!(d[3].dist(d[1] * -sw2) < 1e-9, "t={t}");
            assert!(d[0].dist(arc.derivative(t)) < 1e-9, "t={t}");
        }
    }

    #[test]
    fn seg_derivatives_of_a_conic_satisfy_the_quotient_identity() {
        let conic = Seg2::Conic { a: Vec2::new(-2.0, 0.0), apex: Vec2::new(0.0, 3.0), b: Vec2::new(4.0, 0.5), w: 0.7 };
        let Seg2::Conic { a, apex, b, w } = conic else { panic!("a conic") };
        // The rational quadratic's numerator and weight polynomials, straight from their Bernstein
        // form, and their power coefficients at `t` by differences (exact for a quadratic).
        let n_at = |x: f64| a * ((1.0 - x) * (1.0 - x)) + apex * (2.0 * w * x * (1.0 - x)) + b * (x * x);
        let d_at = |x: f64| (1.0 - x) * (1.0 - x) + 2.0 * w * x * (1.0 - x) + x * x;
        for t in [0.0, 0.25, 1.0] {
            let h = 1e-3;
            // Taylor coefficients at `t` (differences are exact for a quadratic): f, f′/1!, f″/2!.
            let coef_v = |f: &dyn Fn(f64) -> Vec2| [f(t), (f(t + h) - f(t - h)) / (2.0 * h), (f(t + h) - f(t) * 2.0 + f(t - h)) / (2.0 * h * h)];
            let coef_s = |f: &dyn Fn(f64) -> f64| [f(t), (f(t + h) - f(t - h)) / (2.0 * h), (f(t + h) - f(t) * 2.0 + f(t - h)) / (2.0 * h * h)];
            let n = coef_v(&n_at);
            let d = coef_s(&d_at);
            let got = conic.derivatives(t, MAX_DERIVATIVE_ORDER).unwrap();
            // Taylor coefficients of P: P(t) and P⁽ʲ⁾(t)/j!. Leibniz on N = P·D then reads
            // Σ xᵢ·dⱼ₋ᵢ = nⱼ at every order, which the quotient rule must reproduce.
            let x: Vec<Vec2> = std::iter::once(conic.point_at(t)).chain(got.iter().enumerate().map(|(i, v)| *v / fact(i + 1))).collect();
            for j in 0..=4usize {
                let lhs: Vec2 = (0..=j).fold(Vec2::ZERO, |acc, i| acc + x[i] * d.get(j - i).copied().unwrap_or(0.0));
                let rhs = n.get(j).copied().unwrap_or(Vec2::ZERO);
                assert!(lhs.dist(rhs) < 1e-8 * rhs.len().max(1.0), "t={t} j={j}: {lhs:?} vs {rhs:?}");
            }
            assert!(got[0].dist(conic.derivative(t)) < 1e-8, "t={t}");
        }
    }

    #[test]
    fn seg_derivatives_refuse_hostile_input_and_degenerate_pieces() {
        let c = Seg2::Cubic { p0: Vec2::ZERO, p1: Vec2::X, p2: Vec2::Y, p3: Vec2::X + Vec2::Y };
        assert!(c.derivatives(0.0, MAX_DERIVATIVE_ORDER + 1).is_none(), "no order above G4");
        assert!(c.derivatives(f64::NAN, 2).is_none());
        let nan = Seg2::Line { a: Vec2::new(f64::NAN, 0.0), b: Vec2::X };
        assert!(nan.derivatives(0.0, 1).is_none());
        // A conic whose weight vanishes is a straight chord drawn at zero speed: its derivatives
        // are finite, but the first is zero, which is what a blend refuses.
        let flat = Seg2::Conic { a: Vec2::ZERO, apex: Vec2::new(1.0, 1.0), b: Vec2::new(2.0, 0.0), w: 0.0 };
        let d = flat.derivatives(0.0, 1).unwrap();
        assert_eq!(d[0], Vec2::ZERO);
    }
}
