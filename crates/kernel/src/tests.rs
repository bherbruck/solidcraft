use std::f64::consts::PI;

use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2, Vec3};

use crate::*;

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-12)
}

fn rect(w: f64, h: f64) -> Region2 {
    Region2 { outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(w, 0.0), Vec2::new(w, h), Vec2::new(0.0, h)]), holes: vec![] }
}

#[test]
fn box_extrude_measures() {
    let b = extrude(&Plane::XY, &[rect(40.0, 30.0)], 0.0, 20.0).unwrap().pop().unwrap();
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, 24000.0) < 1e-9, "{}", m.volume);
    assert!(rel(m.area, 5200.0) < 1e-9);
    assert_eq!((m.faces, m.edges, m.vertices), (6, 12, 8));
    assert_eq!((m.merged.faces, m.merged.edges, m.merged.vertices), (6, 12, 8));
    assert!(m.centroid.dist(Vec3::new(20.0, 15.0, 10.0)) < 1e-6);
}

#[test]
fn cylinder_measures_match_analytic() {
    let b = extrude(&Plane::XY, &[Region2 { outer: Loop2::circle(Vec2::ZERO, 15.0), holes: vec![] }], 0.0, 50.0).unwrap().pop().unwrap();
    let m = measure(&b).unwrap();
    let v = PI * 225.0 * 50.0;
    let a = 2.0 * PI * 225.0 + 2.0 * PI * 15.0 * 50.0;
    assert!(rel(m.volume, v) < 5e-4, "{} vs {v}", m.volume);
    assert!(rel(m.area, a) < 5e-4, "{} vs {a}", m.area);
    assert_eq!((m.merged.faces, m.merged.edges, m.merged.vertices), (3, 2, 2), "{:?}", m.merged);
    assert_eq!(m.merged.face_types.get("cylinder"), Some(&1));
}

#[test]
fn extrude_on_other_planes_and_offsets() {
    let b = extrude(&Plane::XZ, &[rect(10.0, 5.0)], -2.0, 3.0).unwrap().pop().unwrap();
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, 250.0) < 1e-9);
    // XZ normal is +Y: the body spans y in [-2, 3].
    assert!((m.bbox.min.y + 2.0).abs() < 1e-6 && (m.bbox.max.y - 3.0).abs() < 1e-6, "{:?}", m.bbox);
}

#[test]
fn region_with_hole() {
    let r = Region2 { outer: rect(40.0, 30.0).outer, holes: vec![Loop2::circle(Vec2::new(20.0, 15.0), 5.0).reversed()] };
    let b = extrude(&Plane::XY, &[r], 0.0, 10.0).unwrap().pop().unwrap();
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, (1200.0 - PI * 25.0) * 10.0) < 5e-4, "{}", m.volume);
}

#[test]
fn through_cut_preserves_a_larger_blind_pocket() {
    let plate = box_solid(Vec3::new(-30.0, -28.0, 0.0), Vec3::new(60.0, 22.0, 8.0)).unwrap();
    let pocket = cylinder(Vec3::ZERO, Vec3::Z, 11.0, 6.0).unwrap();
    let through = cylinder(Vec3::new(0.0, 0.0, -1.0), Vec3::Z, 9.5, 10.0).unwrap();
    let expected = 90.0 * 50.0 * 8.0 - PI * 11.0_f64.powi(2) * 6.0 - PI * 9.5_f64.powi(2) * 2.0;
    for reverse in [false, true] {
        let (first, second) = if reverse { (&through, &pocket) } else { (&pocket, &through) };
        let cut = boolean(&plate, first, BoolOp::Cut).unwrap().unwrap();
        let result = boolean(&cut, second, BoolOp::Cut).unwrap().unwrap();
        let m = measure(&result).unwrap();
        assert!(rel(m.volume, expected) < 5e-4, "reverse={reverse}: {} vs {expected}", m.volume);
        assert_eq!(m.merged.faces, 9);
        assert!(!result.tessellate(0.01).unwrap().contains(Vec3::new(10.0, 0.0, 3.0)));
    }
}

#[test]
fn join_a_cylinder_on_its_chamfered_top() {
    for seam in [0.0, crate::build::SEAM_ANGLE] {
        let region = Region2 { outer: Loop2::circle_from(Vec2::ZERO, 10.0, seam), holes: vec![] };
        let base = extrude(&Plane::XY, &[region], 0.0, 20.0).unwrap().pop().unwrap();
        let base = chamfer(&base, &[Vec3::new(10.0, 0.0, 20.0)], 2.0).unwrap();
        let region = Region2 { outer: Loop2::circle_from(Vec2::new(0.0, -3.774758283725534e-15), 8.0, seam), holes: vec![] };
        let plane = Plane { origin: Vec3::new(0.0, 0.0, 20.000000000000007), ..Plane::XY };
        let tool = extrude(&plane, &[region], 0.0, 5.0).unwrap().pop().unwrap();
        let expected = measure(&base).unwrap().volume + PI * 64.0 * 5.0;
        let joined = boolean(&base, &tool, BoolOp::Union).unwrap().unwrap();
        assert!(rel(measure(&joined).unwrap().volume, expected) < 5e-4);
        assert_eq!(joined.lumps().unwrap().len(), 1);
    }
}

#[test]
fn box_fillet_cut_volume() {
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
    let f = fillet(&b, &[Vec3::new(0.0, 0.0, 10.0)], 3.0).unwrap();
    let mf = measure(&f).unwrap();
    let expect_f = 24000.0 - (9.0 - PI * 9.0 / 4.0) * 20.0;
    assert!(rel(mf.volume, expect_f) < 1e-4, "{} vs {expect_f}", mf.volume);
    assert_eq!(mf.merged.faces, 7);
    let hole = cylinder(Vec3::new(20.0, 15.0, -5.0), Vec3::Z, 5.0, 30.0).unwrap();
    let cut = boolean(&f, &hole, BoolOp::Cut).unwrap().unwrap();
    let mc = measure(&cut).unwrap();
    let expect = expect_f - PI * 25.0 * 20.0;
    assert!(rel(mc.volume, expect) < 1e-3, "{} vs {expect}", mc.volume);
}

#[test]
fn union_and_intersect() {
    let a = box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
    let b = box_solid(Vec3::new(5.0, 5.0, 5.0), Vec3::new(15.0, 15.0, 15.0)).unwrap();
    let u = boolean(&a, &b, BoolOp::Union).unwrap().unwrap();
    assert!(rel(measure(&u).unwrap().volume, 2000.0 - 125.0) < 1e-6);
    let i = boolean(&a, &b, BoolOp::Intersect).unwrap().unwrap();
    assert!(rel(measure(&i).unwrap().volume, 125.0) < 1e-6);
}

#[test]
fn revolve_washer_and_sphere() {
    let r =
        Region2 { outer: Loop2::polygon(&[Vec2::new(10.0, 0.0), Vec2::new(15.0, 0.0), Vec2::new(15.0, 5.0), Vec2::new(10.0, 5.0)]), holes: vec![] };
    let b = revolve(&Plane::XZ, std::slice::from_ref(&r), Vec2::ZERO, Vec2::Y, 2.0 * PI).unwrap().pop().unwrap();
    let m = measure(&b).unwrap();
    let v = PI * (225.0 - 100.0) * 5.0;
    assert!(rel(m.volume, v) < 5e-4, "{} vs {v}", m.volume);
    let half = revolve(&Plane::XZ, &[r], Vec2::ZERO, Vec2::Y, PI).unwrap().pop().unwrap();
    assert!(rel(measure(&half).unwrap().volume, v / 2.0) < 5e-4);
    let s = sphere(Vec3::new(1.0, 2.0, 3.0), 10.0).unwrap();
    let ms = measure(&s).unwrap();
    assert!(rel(ms.volume, 4.0 / 3.0 * PI * 1000.0) < 1e-3, "{}", ms.volume);
}

#[test]
fn revolve_adjacent_profiles_on_the_axis() {
    // Two rectangles on the axis, revolved together: their merged outline keeps a vertex
    // where the shared edge met the axis. The swept axis pieces used to be zero-area faces,
    // and cutting the body's flat end out of another failed.
    let r = Region2 {
        outer: Loop2::polygon(&[
            Vec2::new(0.0, 0.0),
            Vec2::new(30.0, 0.0),
            Vec2::new(60.0, 0.0),
            Vec2::new(60.0, 20.0),
            Vec2::new(30.0, 20.0),
            Vec2::new(0.0, 20.0),
        ]),
        holes: vec![],
    };
    let b = revolve(&Plane::XZ, &[r], Vec2::ZERO, Vec2::X, 2.0 * PI).unwrap().pop().unwrap();
    assert!(rel(measure(&b).unwrap().volume, PI * 400.0 * 60.0) < 5e-4);
    let faces = b.faces(1e-3).unwrap();
    assert!(faces.iter().all(|f| f.area > 1e-6), "{:?}", faces.iter().map(|f| f.area).collect::<Vec<_>>());
    let disc = cylinder(Vec3::new(-1.0, 0.0, 0.0), Vec3::X, 40.0, 3.0).unwrap();
    let cut = boolean(&disc, &b, BoolOp::Cut).unwrap().unwrap();
    let want = PI * 1600.0 * 3.0 - PI * 400.0 * 2.0;
    assert!(rel(measure(&cut).unwrap().volume, want) < 1e-3, "{} vs {want}", measure(&cut).unwrap().volume);
}

#[test]
fn chamfer_and_step() {
    let b = box_solid(Vec3::ZERO, Vec3::new(20.0, 20.0, 20.0)).unwrap();
    let c = chamfer(&b, &[Vec3::new(20.0, 10.0, 20.0)], 2.0).unwrap();
    assert!(rel(measure(&c).unwrap().volume, 8000.0 - 2.0 * 20.0) < 1e-4);
    let s = step_export(&[&c], "SolidCraft").unwrap();
    assert!(s.starts_with("ISO-10303-21;"));
    assert!(s.contains("MANIFOLD_SOLID_BREP") || s.contains("CLOSED_SHELL"));
}

#[test]
fn hostile_inputs_are_errors() {
    assert!(extrude(&Plane::XY, &[rect(1.0, 1.0)], 0.0, 0.0).is_err());
    assert!(extrude(&Plane::XY, &[rect(1.0, 1.0)], 0.0, f64::NAN).is_err());
    assert!(extrude(&Plane::XY, &[], 0.0, 1.0).is_err());
    assert!(extrude(&Plane::XY, &[rect(0.0, 1.0)], 0.0, 1.0).is_err());
    assert!(box_solid(Vec3::ZERO, Vec3::new(1.0, 0.0, 1.0)).is_err());
    assert!(cylinder(Vec3::ZERO, Vec3::ZERO, 1.0, 1.0).is_err());
    let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
    assert!(fillet(&b, &[Vec3::new(0.0, 0.0, 5.0)], -1.0).is_err());
    assert!(fillet(&b, &[Vec3::new(500.0, 0.0, 5.0)], 1.0).is_err());
    assert!(fillet(&b, &[Vec3::new(0.0, 0.0, 5.0)], 100.0).is_err());
    let r = Region2 { outer: Loop2::polygon(&[Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)]), holes: vec![] };
    assert!(revolve(&Plane::XZ, &[r], Vec2::ZERO, Vec2::Y, 1.0).is_err(), "crosses axis");
    assert!(transform(&b, Vec3::new(f64::NAN, 0.0, 0.0), Vec3::ZERO, Vec3::Z, 0.0).is_err());
}

#[test]
fn fillets_on_several_edges_and_angles() {
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
    // Two opposite vertical edges, then a horizontal top edge between them is not touching either.
    let f = fillet(&b, &[Vec3::new(0.0, 0.0, 10.0), Vec3::new(40.0, 30.0, 10.0)], 4.0).unwrap();
    let corner = 16.0 - PI * 4.0;
    assert!(rel(measure(&f).unwrap().volume, 24000.0 - 2.0 * corner * 20.0) < 2e-4);
    let top = fillet(&b, &[Vec3::new(20.0, 0.0, 20.0)], 2.0).unwrap();
    assert!(rel(measure(&top).unwrap().volume, 24000.0 - (4.0 - PI) * 40.0) < 2e-4);
    // Triangular prism: 60° edges.
    let tri = Region2 { outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(30.0, 0.0), Vec2::new(15.0, 25.980762113533)]), holes: vec![] };
    let p = extrude(&Plane::XY, &[tri], 0.0, 10.0).unwrap().pop().unwrap();
    let r = 2.0;
    let fp = fillet(&p, &[Vec3::new(0.0, 0.0, 5.0)], r).unwrap();
    // Removed area for interior angle φ: r²(cot(φ/2)) − r²(π − φ)/2.
    let phi = PI / 3.0;
    let removed = r * r / (phi / 2.0).tan() - r * r * (PI - phi) / 2.0;
    let v0 = measure(&p).unwrap().volume;
    assert!(rel(measure(&fp).unwrap().volume, v0 - removed * 10.0) < 2e-4, "{} vs {}", measure(&fp).unwrap().volume, v0 - removed * 10.0);
    // Adjacent edges meeting at a corner: their blends meet in a mitre.
    let two = fillet(&b, &[Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 15.0, 20.0)], 2.0).unwrap();
    let cut = 24000.0 - measure(&two).unwrap().volume;
    let a = 4.0 * (1.0 - PI / 4.0);
    assert!(cut > a * (20.0 + 30.0 - 2.0) && cut < a * 50.0, "{cut}");
    // Concave edge where a post stands on the plate: the round adds material, its end caps
    // extending the post's end faces down onto the plate.
    let u = boolean(&b, &box_solid(Vec3::new(10.0, 10.0, 15.0), Vec3::new(20.0, 20.0, 30.0)).unwrap(), BoolOp::Union).unwrap().unwrap();
    let f = fillet(&u, &[Vec3::new(15.0, 10.0, 20.0)], 1.0).unwrap();
    let added = measure(&f).unwrap().volume - measure(&u).unwrap().volume;
    assert!(rel(added, (1.0 - PI / 4.0) * 10.0) < 1e-3, "{added}");
    assert!(f.validity().is_empty());
    assert_eq!(f.lumps().unwrap().len(), 1);
}

#[test]
fn fillet_ends_in_an_end_face_at_an_angle() {
    // #215: the round ends in the end face's plane along an ellipse (its section) when that face
    // is not square to the edge. A prism on a trapezoid (0,0) (40,0) (30,20) (0,20), 10 high: the
    // top edge along y = 0 ends at x = 0 (square) and in the face x = 40 - y/2 (at an angle).
    let r: f64 = 2.0;
    let (area, c) = ((1.0 - PI / 4.0) * r * r, r * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI)));
    let trapezoid = |far: f64| {
        let pts = [Vec2::new(0.0, 0.0), Vec2::new(40.0, 0.0), Vec2::new(far, 20.0), Vec2::new(0.0, 20.0)];
        extrude(&Plane::XY, &[Region2 { outer: Loop2::polygon(&pts), holes: vec![] }], 0.0, 10.0).unwrap().pop().unwrap()
    };
    // The removed section's length at depth y into the top face is 40 + (far - 40) y / 20; its
    // mean over the section is taken at the section's centroid (c from the edge).
    for far in [30.0, 50.0] {
        let b = trapezoid(far);
        let v0 = measure(&b).unwrap().volume;
        let f = fillet(&b, &[Vec3::new(20.0, 0.0, 10.0)], r).unwrap();
        let want = v0 - area * (40.0 + (far - 40.0) * c / 20.0);
        let m = measure(&f).unwrap();
        assert!(rel(m.volume, want) < 2e-4, "far {far}: {} vs {want}", m.volume);
        assert_eq!(f.lumps().unwrap().len(), 1);
        assert!(f.validity().is_empty(), "far {far}: {:?}", f.validity());
    }
}

#[test]
fn fillet_ending_across_two_faces() {
    // #215: a plate's outline chamfered, then the mitre where two chamfers meet at a corner
    // rounded. The mitre runs from the top face (ending in it at an angle) down to the corner
    // where two side faces meet; the round ends across both, cutting back the vertical edge.
    let (r, k) = (2.0, 5.0);
    let b = box_solid(Vec3::ZERO, Vec3::new(100.0, 50.0, 20.0)).unwrap();
    let outline = [Vec3::new(50.0, 0.0, 20.0), Vec3::new(100.0, 25.0, 20.0), Vec3::new(50.0, 50.0, 20.0), Vec3::new(0.0, 25.0, 20.0)];
    let c = chamfer(&b, &outline, k).unwrap();
    let f = fillet(&c, &[Vec3::new(k / 2.0, k / 2.0, 20.0 - k / 2.0)], r).unwrap();
    assert!(f.validity().is_empty(), "{:?}", f.validity());
    assert_eq!(f.lumps().unwrap().len(), 1);
    // The removed material, by definition: inside the plate, between the two chamfers and the
    // round, i.e. outside the round's cylinder and on the corner's side of both lines where it
    // touches the chamfers (each at the setback s = r cot(φ/2) from the mitre, φ = 120° between
    // the chamfers). Sampled at pseudo-random points (a regular grid aliases with the region's
    // thin slivers along the chamfers): about 1.89 mm³, standard error about 0.02.
    let d = Vec3::new(1.0, 1.0, 1.0).normalized().unwrap();
    let (n1, n2) = (Vec3::new(0.0, -1.0, 1.0).normalized().unwrap(), Vec3::new(-1.0, 0.0, 1.0).normalized().unwrap());
    // In each chamfer, square to the mitre and away from the other chamfer.
    let into = |n: Vec3, other: Vec3| {
        let t = n.cross(d).normalized().unwrap();
        if t.dot(other) < 0.0 { t } else { -t }
    };
    let (t1, t2) = (into(n1, n2), into(n2, n1));
    let phi = (-n1.dot(n2)).acos();
    let s = r / (phi / 2.0).tan();
    let corner = Vec3::new(0.0, 0.0, 20.0 - k);
    let axis_at = corner + t1 * s - n1 * r;
    let removed_at = |q: Vec3| {
        let u = q - axis_at;
        q.z - q.x <= 15.0 && q.z - q.y <= 15.0 && (q - corner).dot(t1) < s && (q - corner).dot(t2) < s && (u - d * u.dot(d)).len() > r
    };
    let (lo, span) = (Vec3::new(0.0, 0.0, 20.0 - k - 2.0 * r), k + 2.0 * r);
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut unit = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let samples = 4_000_000;
    let hits = (0..samples).filter(|_| removed_at(lo + Vec3::new(unit(), unit(), unit()) * span)).count();
    let removed = span.powi(3) * hits as f64 / samples as f64;
    let lost = measure(&c).unwrap().volume - measure(&f).unwrap().volume;
    assert!((lost - removed).abs() < 0.07, "{lost} vs {removed}");
}

#[test]
fn loop_fillet_as_tall_as_its_wall_consumes_it() {
    // The removed cross-section (a square of side r less a quarter disc) and its centroid's
    // distance from the wall: r(10 − 3π) / (3(4 − π)).
    let spandrel = |r: f64| ((1.0 - PI / 4.0) * r * r, r * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI)));
    // #190: a disc 13 in radius and 2 high, its top circle rounded by 2. Pappus for the ring.
    let disc = cylinder(Vec3::ZERO, Vec3::Z, 13.0, 2.0).unwrap();
    let f = fillet(&disc, &[Vec3::new(0.0, 13.0, 2.0), Vec3::new(0.0, -13.0, 2.0)], 2.0).unwrap();
    let (a, c) = spandrel(2.0);
    let want = PI * 169.0 * 2.0 - a * 2.0 * PI * (13.0 - c);
    let m = measure(&f).unwrap();
    assert!(rel(m.volume, want) < 2e-4, "{} vs {want}", m.volume);
    // Top, round and bottom: the wall is gone.
    assert_eq!(m.merged.faces, 3, "{:?}", m.merged);
    assert!(f.validity().is_empty(), "{:?}", f.validity());
    // A 40 x 30 x 5 plate, its whole top outline rounded by 5 (mitred at the corners).
    let plate = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 5.0)).unwrap();
    let top = [Vec3::new(20.0, 0.0, 5.0), Vec3::new(40.0, 15.0, 5.0), Vec3::new(20.0, 30.0, 5.0), Vec3::new(0.0, 15.0, 5.0)];
    let f = fillet(&plate, &top, 5.0).unwrap();
    let (a, c) = spandrel(5.0);
    let want = 6000.0 - a * 2.0 * ((40.0 - 2.0 * c) + (30.0 - 2.0 * c));
    let m = measure(&f).unwrap();
    assert!(rel(m.volume, want) < 2e-4, "{} vs {want}", m.volume);
    assert!(f.validity().is_empty(), "{:?}", f.validity());
    // Taller than the wall: still refused, the body unchanged.
    for r in [2.0 + 1e-3, 2.5] {
        let e = fillet(&disc, &[Vec3::new(0.0, 13.0, 2.0), Vec3::new(0.0, -13.0, 2.0)], r).unwrap_err();
        assert!(e.to_string().contains("taller than the walls"), "{r}: {e}");
    }
}

#[test]
fn fillet_consuming_a_side_face_makes_a_full_round() {
    // 14 x 6 x 10: r = 3 on the four vertical edges uses up both 6 mm end faces (#199).
    let b = box_solid(Vec3::new(14.0, 0.0, 0.0), Vec3::new(28.0, 6.0, 10.0)).unwrap();
    let mids = [Vec3::new(14.0, 0.0, 5.0), Vec3::new(28.0, 0.0, 5.0), Vec3::new(14.0, 6.0, 5.0), Vec3::new(28.0, 6.0, 5.0)];
    let full = 840.0 - (4.0 - PI) * 9.0 * 10.0;
    for order in [mids.to_vec(), mids.iter().rev().copied().collect()] {
        let f = fillet(&b, &order, 3.0).unwrap();
        let m = measure(&f).unwrap();
        assert!(rel(m.volume, full) < 2e-4, "{} vs {full}", m.volume);
        // Top, bottom, the two long sides and four quarter rounds; the end faces are gone.
        assert_eq!(m.faces, 8, "{m:?}");
        assert_eq!(f.lumps().unwrap().len(), 1);
        assert!(f.validity().is_empty(), "{:?}", f.validity());
    }
    // One end only: two rounds meeting on one end face.
    let f = fillet(&b, &mids[1..4].iter().step_by(2).copied().collect::<Vec<_>>(), 3.0).unwrap();
    assert!(rel(measure(&f).unwrap().volume, 840.0 - (4.0 - PI) * 9.0 * 5.0) < 2e-4);
    assert!(f.validity().is_empty());
    // A 6 x 6 post rounded on all four edges with r = 3 is a cylinder.
    let post = box_solid(Vec3::ZERO, Vec3::new(6.0, 6.0, 10.0)).unwrap();
    let corners = [Vec3::new(0.0, 0.0, 5.0), Vec3::new(6.0, 0.0, 5.0), Vec3::new(6.0, 6.0, 5.0), Vec3::new(0.0, 6.0, 5.0)];
    let c = fillet(&post, &corners, 3.0).unwrap();
    assert!(rel(measure(&c).unwrap().volume, PI * 9.0 * 10.0) < 2e-4, "{}", measure(&c).unwrap().volume);
    assert!(c.validity().is_empty());
    // Past the limit there is no face left to consume: still refused, the body unchanged.
    for r in [3.0 + 1e-3, 3.5] {
        let e = fillet(&b, &mids, r).unwrap_err();
        assert!(e.to_string().contains("larger than the neighbouring faces"), "{r}: {e}");
    }
    assert_eq!(measure(&b).unwrap().volume, 840.0);
}

#[test]
fn fillet_max_finds_the_largest_radius_that_works() {
    let too_large = |b: &Body, edges: &[Vec3], r: f64| matches!(fillet(b, edges, r), Err(KernelError::TooLarge(_)));
    // Two rounds on a 6 mm face: up to 3 (a full round), although after the first round at
    // 3.5 the next edge alone would take only 2.5.
    let b = box_solid(Vec3::new(14.0, 0.0, 0.0), Vec3::new(28.0, 6.0, 10.0)).unwrap();
    let mids = [Vec3::new(14.0, 0.0, 5.0), Vec3::new(28.0, 0.0, 5.0), Vec3::new(14.0, 6.0, 5.0), Vec3::new(28.0, 6.0, 5.0)];
    assert!(too_large(&b, &mids, 3.5));
    let (r, body) = fillet_max(&b, &mids, 3.5).unwrap();
    assert_eq!(r, 3.0);
    assert!(rel(measure(&body).unwrap().volume, 840.0 - (4.0 - PI) * 90.0) < 2e-4);
    // A 2 mm disc's top edge: up to 2 (the wall used up).
    let disc = cylinder(Vec3::ZERO, Vec3::Z, 13.0, 2.0).unwrap();
    let top = [Vec3::new(0.0, 13.0, 2.0), Vec3::new(0.0, -13.0, 2.0)];
    assert!(too_large(&disc, &top, 2.5));
    assert_eq!(fillet_max(&disc, &top, 2.5).map(|x| x.0), Some(2.0));
    // Walls of different heights: at the shortest one's height only that wall would be used up,
    // which is refused, so the largest radius is just below it (to the search's 3 digits).
    let notched = boolean(
        &box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap(),
        &box_solid(Vec3::new(35.0, -1.0, -1.0), Vec3::new(41.0, 5.0, 3.0)).unwrap(),
        BoolOp::Cut,
    )
    .unwrap()
    .unwrap();
    let outline = [Vec3::new(20.0, 0.0, 10.0), Vec3::new(40.0, 15.0, 10.0), Vec3::new(20.0, 30.0, 10.0), Vec3::new(0.0, 15.0, 10.0)];
    assert!(too_large(&notched, &outline, 8.0));
    let (r, _) = fillet_max(&notched, &outline, 8.0).unwrap();
    assert!((r - 6.999).abs() < 1e-9, "{r}");
    // No edge there: no radius.
    assert!(fillet_max(&b, &[Vec3::new(500.0, 0.0, 5.0)], 1.0).is_none());
}

/// Edges that share no face get their own largest radius: a 2 mm fin's top edge takes at most
/// its thickness, while a corner of the block it stands on, far from it, keeps the 4 asked for.
/// Edges that limit each other (the post's four, sharing faces) still share one.
#[test]
fn fillet_max_each_reduces_only_where_the_radius_does_not_fit() {
    let (block, fin) = (40.0 * 30.0 * 20.0, 2.0 * 30.0 * 10.0);
    let body = boolean(
        &box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap(),
        &box_solid(Vec3::new(19.0, 0.0, 20.0), Vec3::new(21.0, 30.0, 30.0)).unwrap(),
        BoolOp::Union,
    )
    .unwrap()
    .unwrap();
    let edges = [Vec3::new(21.0, 15.0, 30.0), Vec3::new(40.0, 0.0, 10.0)];
    assert!(matches!(fillet(&body, &edges, 4.0), Err(KernelError::TooLarge(_))));
    let (one, _) = fillet_max(&body, &edges, 4.0).unwrap();
    let (radii, b) = fillet_max_each(&body, &edges, 4.0).unwrap();
    assert_eq!(radii.get(1), Some(&4.0), "{radii:?}");
    let fin_r = radii.first().copied().unwrap();
    assert!(fin_r < 4.0 && (fin_r - one).abs() < 1e-9, "{radii:?}, one radius {one}");
    assert!(b.validity().is_empty(), "{:?}", b.validity());
    // Each round takes (1 − π/4)·r² along its edge: the fin's 30 long, the corner's 20.
    let removed = (1.0 - PI / 4.0) * (fin_r * fin_r * 30.0 + 16.0 * 20.0);
    assert!(rel(measure(&b).unwrap().volume, block + fin - removed) < 2e-4);
    // The post's four edges limit each other: one radius, as `fillet_max`.
    let post = box_solid(Vec3::new(14.0, 0.0, 0.0), Vec3::new(28.0, 6.0, 10.0)).unwrap();
    let mids = [Vec3::new(14.0, 0.0, 5.0), Vec3::new(28.0, 0.0, 5.0), Vec3::new(14.0, 6.0, 5.0), Vec3::new(28.0, 6.0, 5.0)];
    assert_eq!(fillet_max_each(&post, &mids, 3.5).map(|x| x.0), Some(vec![3.0; 4]));
}

/// A corner's rounds too large for its faces are refused as too large, so a fit can reduce
/// them, also where only the corner build finds it (the edge-by-edge blend failing otherwise).
#[test]
fn corner_rounds_larger_than_their_faces_are_too_large() {
    let (l, eps) = (20.0, 1e-9);
    let b = box_solid(Vec3::ZERO, Vec3::new(l, l, l)).unwrap();
    let corner = [Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 10.0, 0.0), Vec3::new(0.0, 0.0, 10.0)];
    // As wide as the faces: the first edge's round uses up its neighbour, then the edge-by-edge
    // blend can't find it; the corner build is what says the radius is too large.
    let e = fillet(&b, &corner, l).unwrap_err();
    assert!(matches!(e, KernelError::TooLarge(_)), "{e}");
    let (r, body) = fillet_max(&b, &corner, l).unwrap();
    assert!((r - 19.99).abs() < eps, "{r}");
    assert!(body.validity().is_empty(), "{:?}", body.validity());
    // Three rounds along the edges past the corner cube, and the cube less its sphere octant.
    let removed = 3.0 * (1.0 - PI / 4.0) * r * r * (l - r) + r * r * r * (1.0 - PI / 6.0);
    assert!(rel(measure(&body).unwrap().volume, l * l * l - removed) < 2e-4);
    // Different radii per edge, one wider than its faces.
    let sel: Vec<(Vec3, f64)> = corner.iter().copied().zip([21.0, 5.0, 5.0]).collect();
    let e = crate::blend_corner::fillet_radii(&b, &sel).unwrap_err();
    assert!(matches!(e, KernelError::TooLarge(_)), "{e}");
}

#[test]
fn tapered_extrudes() {
    // 40 x 40 square, 30 up, 10° outward: a frustum.
    let sq = Region2 {
        outer: Loop2::polygon(&[Vec2::new(-20.0, -20.0), Vec2::new(20.0, -20.0), Vec2::new(20.0, 20.0), Vec2::new(-20.0, 20.0)]),
        holes: vec![],
    };
    let b = extrude_tapered(&Plane::XY, &sq, 30.0, 1.0, 10f64.to_radians()).unwrap();
    let top = 40.0 + 2.0 * 30.0 * 10f64.to_radians().tan();
    let v = 30.0 / 3.0 * (1600.0 + top * top + 40.0 * top);
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, v) < 1e-6, "{} vs {v}", m.volume);
    assert_eq!(m.merged.faces, 6);
    // Inward taper on a circle (a cone frustum), downward.
    let c = Region2 { outer: Loop2::circle(Vec2::ZERO, 10.0), holes: vec![] };
    let b = extrude_tapered(&Plane::XY, &c, 10.0, -1.0, -20f64.to_radians()).unwrap();
    let r2 = 10.0 - 10.0 * 20f64.to_radians().tan();
    let v = PI * 10.0 / 3.0 * (100.0 + r2 * r2 + 10.0 * r2);
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, v) < 5e-4, "{} vs {v}", m.volume);
    assert!(m.bbox.max.z < 1e-6 && m.bbox.min.z > -10.0 - 1e-6, "{:?}", m.bbox);
    assert!(extrude_tapered(&Plane::XY, &c, 10.0, 1.0, -60f64.to_radians()).is_err(), "closes the profile");
}

/// A slot (two lines and two half-circles, tangent) tapered inward: every edge moves in by
/// d = z · tan 8°, so A(d) = A₀ − P·d + π·d² (#546). An arc meeting a line at a corner is refused.
#[test]
fn tapered_extrude_of_lines_and_arcs() {
    let (r, l, h, a) = (5.0, 20.0, 12.0, 8f64.to_radians());
    let slot = Loop2 {
        segs: vec![
            Seg2::Line { a: Vec2::new(0.0, -r), b: Vec2::new(l, -r) },
            Seg2::Arc { center: Vec2::new(l, 0.0), radius: r, start: -PI / 2.0, sweep: PI },
            Seg2::Line { a: Vec2::new(l, r), b: Vec2::new(0.0, r) },
            Seg2::Arc { center: Vec2::ZERO, radius: r, start: PI / 2.0, sweep: PI },
        ],
    };
    let b = extrude_tapered(&Plane::XY, &Region2 { outer: slot, holes: vec![] }, h, 1.0, -a).unwrap();
    let (a0, p, t) = (2.0 * r * l + PI * r * r, 2.0 * l + 2.0 * PI * r, a.tan());
    let v = a0 * h - p * t * h * h / 2.0 + PI * t * t * h.powi(3) / 3.0;
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, v) < 5e-4, "{} vs {v}", m.volume);
    assert_eq!(m.merged.faces, 6);
    // A quarter disc: the arc meets both lines at right angles.
    let quarter = Loop2 {
        segs: vec![
            Seg2::Line { a: Vec2::ZERO, b: Vec2::new(r, 0.0) },
            Seg2::Arc { center: Vec2::ZERO, radius: r, start: 0.0, sweep: PI / 2.0 },
            Seg2::Line { a: Vec2::new(0.0, r), b: Vec2::ZERO },
        ],
    };
    let e = extrude_tapered(&Plane::XY, &Region2 { outer: quarter, holes: vec![] }, h, 1.0, a).unwrap_err();
    assert!(e.to_string().contains("arc meets another edge at a corner"), "{e}");
}

/// A circle drawn as four conics (the way free curves come, as in text) tapered inward: each
/// conic's control polygon moves in by d, which for a circle is the exact smaller circle, so
/// the solid is a cone frustum.
#[test]
fn tapered_extrude_of_conics() {
    let (r, h, a) = (10.0, 12.0, 8f64.to_radians());
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let p = |x: f64, y: f64| Vec2::new(x, y);
    let disc = Loop2 {
        segs: vec![
            Seg2::Conic { a: p(r, 0.0), apex: p(r, r), b: p(0.0, r), w },
            Seg2::Conic { a: p(0.0, r), apex: p(-r, r), b: p(-r, 0.0), w },
            Seg2::Conic { a: p(-r, 0.0), apex: p(-r, -r), b: p(0.0, -r), w },
            Seg2::Conic { a: p(0.0, -r), apex: p(r, -r), b: p(r, 0.0), w },
        ],
    };
    let b = extrude_tapered(&Plane::XY, &Region2 { outer: disc, holes: vec![] }, h, 1.0, -a).unwrap();
    let r1 = r - h * a.tan();
    let v = PI * h / 3.0 * (r * r + r1 * r1 + r * r1);
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, v) < 1e-3, "{} vs {v}", m.volume);
    assert!((m.bbox.max.x - r).abs() < 1e-6 && (m.bbox.max.z - h).abs() < 1e-6, "{:?}", m.bbox);
}

/// A circle drawn as four cubics (as splines and text come) tapered inward. A cubic's control
/// polygon moved in is only close to the cubic's true offset, so the solid is checked against
/// the offset's volume within a tolerance: the section at depth d of a convex smooth loop has
/// area A₀ − P·d + π·d² (Steiner), so the volume is A₀·h − P·t·h²/2 + π·t²·h³/3.
#[test]
fn tapered_extrude_of_cubics_is_close_to_the_true_offset() {
    let (r, h, a) = (10.0, 12.0, 8f64.to_radians());
    let k = r * 0.552_284_749_830_793_4;
    let p = |x: f64, y: f64| Vec2::new(x, y);
    let disc = Loop2 {
        segs: vec![
            Seg2::Cubic { p0: p(r, 0.0), p1: p(r, k), p2: p(k, r), p3: p(0.0, r) },
            Seg2::Cubic { p0: p(0.0, r), p1: p(-k, r), p2: p(-r, k), p3: p(-r, 0.0) },
            Seg2::Cubic { p0: p(-r, 0.0), p1: p(-r, -k), p2: p(-k, -r), p3: p(0.0, -r) },
            Seg2::Cubic { p0: p(0.0, -r), p1: p(k, -r), p2: p(r, -k), p3: p(r, 0.0) },
        ],
    };
    let (area, perimeter) = (disc.signed_area().abs(), disc.segs.iter().map(|s| s.length()).sum::<f64>());
    let b = extrude_tapered(&Plane::XY, &Region2 { outer: disc, holes: vec![] }, h, 1.0, -a).unwrap();
    let t = a.tan();
    let v = area * h - perimeter * t * h * h / 2.0 + PI * t * t * h.powi(3) / 3.0;
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, v) < 5e-4, "{} vs {v}", m.volume);
}

/// An S-shaped cubic (one curve turning both ways) has no moved cubic close to its offset: it is
/// cut into pieces until each piece's moved curve stays within 2 % of the offset of the true
/// one (or 10 µm), both ways.
#[test]
fn a_tapered_s_curve_is_cut_into_pieces_that_follow_its_offset() {
    let s_curve = Seg2::Cubic { p0: Vec2::new(0.0, 0.0), p1: Vec2::new(5.0, 10.0), p2: Vec2::new(10.0, -10.0), p3: Vec2::new(15.0, 0.0) };
    let d = 0.5;
    assert!(crate::build::offset_error(&s_curve, d).unwrap() > 0.05 * d, "one piece is far off");
    let lp = Loop2 {
        segs: vec![
            s_curve,
            Seg2::Line { a: Vec2::new(15.0, 0.0), b: Vec2::new(15.0, -20.0) },
            Seg2::Line { a: Vec2::new(15.0, -20.0), b: Vec2::new(0.0, -20.0) },
            Seg2::Line { a: Vec2::new(0.0, -20.0), b: Vec2::new(0.0, 0.0) },
        ],
    };
    let (cut, from) = crate::build::offset_splits(&lp, d);
    let pieces: Vec<&Seg2> = cut.segs.iter().zip(&from).filter(|(_, i)| **i == 0).map(|(s, _)| s).collect();
    assert!(pieces.len() > 1 && from.iter().filter(|i| **i > 0).count() == 3, "{} pieces", pieces.len());
    for p in pieces {
        for d in [d, -d] {
            let e = crate::build::offset_error(p, d).unwrap();
            assert!(e <= crate::build::offset_tolerance(d) + 1e-9, "{e}");
        }
    }
}

/// A wedge whose tip is a flat line far shorter than the taper moves its sides in: the tip line
/// goes, the two long sides meet, and its side is the flat triangle up to there. The solid is a
/// prismatoid, its volume h/6 · (A₀ + 4·A_mid + A₁).
#[test]
fn tapered_extrude_swallows_a_short_tip() {
    let (h, a) = (2.0, 5f64.to_radians());
    let d = h * a.tan();
    let pts = [Vec2::new(0.0, -2.0), Vec2::new(20.0, -0.01), Vec2::new(20.0, 0.01), Vec2::new(0.0, 2.0)];
    let b = extrude_tapered(&Plane::XY, &Region2 { outer: Loop2::polygon(&pts), holes: vec![] }, h, 1.0, -a).unwrap();
    // The top: the three long lines moved in by d (to their left, the loop running
    // counter-clockwise), meeting each other.
    let moved = |p: Vec2, q: Vec2| {
        let t = (q - p).normalized().unwrap();
        (p + t.perp() * d, t)
    };
    let meet = |(p0, t0): (Vec2, Vec2), (p1, t1): (Vec2, Vec2)| p0 + t0 * ((p1 - p0).cross(t1) / t0.cross(t1));
    let (low, high, left) = (moved(pts[0], pts[1]), moved(pts[2], pts[3]), moved(pts[3], pts[0]));
    let top = [meet(left, low), meet(low, high), meet(high, left)];
    let area = |q: &[Vec2]| 0.5 * q.iter().zip(q.iter().cycle().skip(1)).map(|(a, b)| a.cross(*b)).sum::<f64>();
    // Halfway up, each side edge's midpoint (the two tip corners both run to the top's tip).
    let mid = [pts[0].lerp(top[0], 0.5), pts[1].lerp(top[1], 0.5), pts[2].lerp(top[1], 0.5), pts[3].lerp(top[2], 0.5)];
    let v = h / 6.0 * (area(&pts) + 4.0 * area(&mid) + area(&top));
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, v) < 1e-6, "{} vs {v}", m.volume);
    assert_eq!(m.merged.faces, 6, "bottom, top, three sides and the tip's triangle");
}

/// A taper from below the profile, with a foot: the sides stand straight for the foot's height,
/// then slope on to the top, and the section at the profile's plane is the profile.
#[test]
fn tapered_extrude_between_with_a_foot() {
    let a = 10f64.to_radians();
    let sq = Region2 {
        outer: Loop2::polygon(&[Vec2::new(-20.0, -20.0), Vec2::new(20.0, -20.0), Vec2::new(20.0, 20.0), Vec2::new(-20.0, 20.0)]),
        holes: vec![],
    };
    let (from, to, foot) = (-2.0, 10.0, 0.5);
    let b = extrude_tapered_between(&Plane::XY, &sq, from, to, foot, 1.0, a).unwrap();
    let side = |z: f64| 40.0 + 2.0 * z * a.tan();
    let (s0, s1, rise) = (side(from), side(to), to - from - foot);
    let v = foot * s0 * s0 + rise / 3.0 * (s0 * s0 + s1 * s1 + s0 * s1);
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, v) < 1e-6, "{} vs {v}", m.volume);
    assert!((m.bbox.min.z - from).abs() < 1e-6 && (m.bbox.max.z - to).abs() < 1e-6, "{:?}", m.bbox);
    assert_eq!(m.merged.faces, 10);
    assert!(extrude_tapered_between(&Plane::XY, &sq, from, to, 7.0, 1.0, a).is_err(), "a foot taller than half the solid");
}

/// A taper steeper than a profile's details is refused, and says why: a slot narrower than the
/// taper closes; a ring's hole smaller than it closes.
#[test]
fn tapered_extrude_refuses_a_taper_steeper_than_the_details() {
    let slot = Loop2::polygon(&[
        Vec2::new(0.0, 0.0),
        Vec2::new(10.0, 0.0),
        Vec2::new(10.0, 10.0),
        Vec2::new(5.1, 10.0),
        Vec2::new(5.1, 2.0),
        Vec2::new(4.9, 2.0),
        Vec2::new(4.9, 10.0),
        Vec2::new(0.0, 10.0),
    ]);
    let e = extrude_tapered(&Plane::XY, &Region2 { outer: slot, holes: vec![] }, 2.0, 1.0, 10f64.to_radians()).unwrap_err();
    assert!(e.to_string().contains("the taper closes the profile"), "{e}");
    let ring = Region2 { outer: Loop2::circle(Vec2::ZERO, 5.0), holes: vec![Loop2::circle(Vec2::ZERO, 0.3).reversed()] };
    let e = extrude_tapered(&Plane::XY, &ring, 2.0, 1.0, 10f64.to_radians()).unwrap_err();
    assert!(e.to_string().contains("the taper closes the profile"), "{e}");
}

#[test]
fn polyhedra_shell_and_draft() {
    let b = box_solid(Vec3::ZERO, Vec3::new(50.0, 40.0, 30.0)).unwrap();
    let s = shell(&b, &[Vec3::new(25.0, 20.0, 30.0)], 2.0).unwrap();
    let m = measure(&s).unwrap();
    assert!(rel(m.volume, 60000.0 - 46.0 * 36.0 * 28.0) < 1e-6, "{}", m.volume);
    assert_eq!(m.merged.faces, 11);
    // Draft two side faces 5° in, about the bottom, pulling up.
    let d = draft(&b, &[Vec3::new(0.0, 20.0, 15.0), Vec3::new(50.0, 20.0, 15.0)], &Plane::XY, Vec3::Z, 5f64.to_radians()).unwrap();
    let t = 30.0 * 5f64.to_radians().tan();
    assert!(rel(measure(&d).unwrap().volume, 40.0 * 30.0 * (50.0 + 50.0 - 2.0 * t) / 2.0) < 1e-6, "{}", measure(&d).unwrap().volume);
    let tet = convex_polyhedron(&[
        HalfSpace { n: Vec3::new(0.0, 0.0, -1.0), d: 0.0 },
        HalfSpace { n: Vec3::new(-1.0, 0.0, 0.0), d: 0.0 },
        HalfSpace { n: Vec3::new(0.0, -1.0, 0.0), d: 0.0 },
        HalfSpace { n: Vec3::new(1.0, 1.0, 1.0).normalized().unwrap(), d: 6.0 / 3f64.sqrt() },
    ])
    .unwrap();
    assert!(rel(measure(&tet).unwrap().volume, 36.0) < 1e-9);
    let cyl = cylinder(Vec3::ZERO, Vec3::Z, 5.0, 5.0).unwrap();
    let cup = shell(&cyl, &[Vec3::new(0.0, 0.0, 5.0)], 1.0).unwrap();
    assert!(rel(measure(&cup).unwrap().volume, PI * (25.0 * 5.0 - 16.0 * 4.0)) < 1e-4, "cup");
}

#[test]
fn loft_and_sweep() {
    let sq = Loop2::polygon(&[Vec2::new(-20.0, -20.0), Vec2::new(20.0, -20.0), Vec2::new(20.0, 20.0), Vec2::new(-20.0, 20.0)]);
    let top = Plane::XY.offset(40.0);
    let l = loft(&[(Plane::XY, sq), (top, Loop2::circle(Vec2::ZERO, 10.0))]).unwrap();
    let m = measure(&l).unwrap();
    println!("loft volume {} faces {:?}", m.volume, m.merged);
    assert!(m.volume > 314.0 * 40.0 && m.volume < 1600.0 * 40.0);
    // A tube: up 30, quarter bend of radius 20, across 30.
    let disc = Region2 { outer: Loop2::circle(Vec2::ZERO, 3.0), holes: vec![] };
    let path = [
        PathSeg::Line { a: Vec3::ZERO, b: Vec3::new(0.0, 0.0, 30.0) },
        PathSeg::Arc {
            a: Vec3::new(0.0, 0.0, 30.0),
            center: Vec3::new(20.0, 0.0, 30.0),
            axis: Vec3::new(0.0, 1.0, 0.0),
            angle: std::f64::consts::FRAC_PI_2,
        },
        PathSeg::Line { a: Vec3::new(20.0, 0.0, 50.0), b: Vec3::new(50.0, 0.0, 50.0) },
    ];
    let t = sweep(&Plane::XY, &disc, &path).unwrap();
    for tol in [0.1, 0.01, 0.003, 0.001] {
        println!("tube tess {tol}: {}", t.tessellate(tol).unwrap().measure().volume);
    }
    let area = PI * 9.0;
    let expect = area * (30.0 + 30.0 + 20.0 * std::f64::consts::FRAC_PI_2);
    assert!(rel(measure(&t).unwrap().volume, expect) < 1e-3, "{} vs {expect}", measure(&t).unwrap().volume);
}

#[test]
fn sweep_round_a_sharp_corner_is_mitred() {
    // A 2 × 2 square centred on an L of two 10 mm lines: lines alone would slide the profile
    // along the second leg within its own plane (an empty, inside-out body). Mitred, the
    // volume is the area times the path length.
    let sq =
        Region2 { outer: Loop2::polygon(&[Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)]), holes: vec![] };
    let path = [
        PathSeg::Line { a: Vec3::ZERO, b: Vec3::new(0.0, 0.0, 10.0) },
        PathSeg::Line { a: Vec3::new(0.0, 0.0, 10.0), b: Vec3::new(10.0, 0.0, 10.0) },
    ];
    let b = sweep(&Plane::XY, &sq, &path).unwrap();
    let m = measure(&b).unwrap();
    assert!(rel(m.volume, 80.0) < 1e-9, "{}", m.volume);
}

#[test]
fn moving_a_body_with_an_intersection_curve_edge_ends() {
    // A cylinder cut by a tilted box keeps an elliptical intersection curve as an edge. Moving
    // such a body scales the curve's tolerance by the matrix norm, whose root refinement cycled
    // forever for about one translation in eight (this one included): the app hung.
    let cyl = cylinder(Vec3::ZERO, Vec3::Z, 5.0, 40.0).unwrap();
    let block = box_solid(Vec3::new(-20.0, -20.0, 20.0), Vec3::new(20.0, 20.0, 60.0)).unwrap();
    let block = crate::ops::transform(&block, Vec3::ZERO, Vec3::new(0.0, 0.0, 20.0), Vec3::X, 0.4).unwrap();
    let cut = boolean(&cyl, &block, BoolOp::Cut).unwrap().unwrap();
    assert!(cut.solid.edge_iter().any(|e| matches!(e.curve(), truck_modeling::Curve::IntersectionCurve(_))));
    let t = Vec3::new(0.6911, -0.3337, -0.6127) * 2.334;
    let moved = crate::ops::transform(&cut, t, Vec3::ZERO, Vec3::Z, 0.0).unwrap();
    let (v0, v1) = (measure(&cut).unwrap().volume, measure(&moved).unwrap().volume);
    assert!(rel(v0, v1) < 1e-6, "{v0} vs {v1}");
}

/// Drain pads joined flush under a floor with a drain hole. The floor (100 × 80, top sloping
/// 1°, so not a prism) has a Ø20 hole through it at (50, 40).
/// - A ring whose bore is the hole's wall (coincident cylinders): used to fail.
/// - The same ring 2 mm into the floor: used to fail too.
/// - A disc across the hole: worked before, and must stay exact.
#[test]
fn drain_pads_flush_under_a_floor_with_a_hole() {
    let t = 1f64.to_radians().tan();
    let hs = |n: Vec3, p: Vec3| {
        let n = n.normalized().unwrap();
        HalfSpace { n, d: n.dot(p) }
    };
    let slab = convex_polyhedron(&[
        hs(Vec3::X, Vec3::new(100.0, 0.0, 0.0)),
        hs(-Vec3::X, Vec3::ZERO),
        hs(Vec3::Y, Vec3::new(0.0, 80.0, 0.0)),
        hs(-Vec3::Y, Vec3::ZERO),
        hs(-Vec3::Z, Vec3::ZERO),
        hs(Vec3::new(-t, 0.0, 1.0), Vec3::new(0.0, 0.0, 10.0)),
    ])
    .unwrap();
    let hole = cylinder(Vec3::new(50.0, 40.0, -5.0), Vec3::Z, 10.0, 30.0).unwrap();
    let floor = boolean(&slab, &hole, BoolOp::Cut).unwrap().unwrap();
    // Top z = 10 + t x: the slab's volume is its mean height (at x = 50) times its area, the
    // hole's likewise (its top is planar, centred at x = 50).
    let v_floor = (10.0 + t * 50.0) * (8000.0 - PI * 100.0);
    assert!(rel(measure(&floor).unwrap().volume, v_floor) < 2e-5, "{} vs {v_floor}", measure(&floor).unwrap().volume);
    let ring = |z0: f64, z1: f64| {
        let r = Region2 { outer: Loop2::circle(Vec2::new(50.0, 40.0), 20.0), holes: vec![Loop2::circle(Vec2::new(50.0, 40.0), 10.0).reversed()] };
        extrude(&Plane::XY, &[r], z0, z1).unwrap().pop().unwrap()
    };
    // The ring's part inside the floor adds nothing: 5 mm of ring below it either way.
    let v_ring = PI * (400.0 - 100.0) * 5.0;
    for (z0, z1) in [(-5.0, 0.0), (-5.0, 2.0)] {
        let (result, diagnostics) = capture_boolean_diagnostics(|| boolean(&floor, &ring(z0, z1), BoolOp::Union));
        let u = result.unwrap().unwrap();
        assert!(diagnostics.events.iter().any(|event| event.strategy == BooleanStrategy::CommonBore), "{diagnostics:?}");
        assert!(diagnostics.events.iter().all(|event| event.translation_mm.is_finite() && event.face_shift_mm.is_finite()));
        let v = measure(&u).unwrap().volume;
        assert!(rel(v, v_floor + v_ring) < 2e-5, "ring {z0}..{z1}: {v} vs {}", v_floor + v_ring);
        assert!(u.validity().is_empty(), "{:?}", u.validity());
    }
    let disc = cylinder(Vec3::new(50.0, 40.0, -5.0), Vec3::Z, 20.0, 5.0).unwrap();
    let u = boolean(&floor, &disc, BoolOp::Union).unwrap().unwrap();
    let v = measure(&u).unwrap().volume;
    let expect = v_floor + PI * 400.0 * 5.0;
    assert!(rel(v, expect) < 2e-5, "disc across the hole: {v} vs {expect}");
    // A boss ring standing on the sloping top around the hole, its foot just under the top
    // everywhere: it adds its annulus times its height above the top at the centre.
    let foot = 10.0 + t * 30.0 - 0.2;
    let u = boolean(&floor, &ring(foot, 14.0), BoolOp::Union).unwrap().unwrap();
    let v = measure(&u).unwrap().volume;
    let expect = v_floor + PI * 300.0 * (14.0 - (10.0 + t * 50.0));
    assert!(rel(v, expect) < 2e-5, "boss ring on the slope: {v} vs {expect}");
    assert!(u.validity().is_empty(), "{:?}", u.validity());
}

/// A trough whose floor falls 1° (or 2° toward a drain, in four faces), and short round pads
/// and bosses crossing the fall. The B-rep results were right, but the plausibility check meshed
/// them without refinement: on a round side trimmed to a thin band by the shallow cut, the plain
/// mesh spans the band with triangles cutting through the cylinder, so the check saw missing
/// material, rejected the result, and the planar fallback refused the round pad.
#[test]
fn round_pads_crossing_a_shallow_fall() {
    let hs = |n: Vec3, p: Vec3| {
        let n = n.normalized().unwrap();
        HalfSpace { n, d: n.dot(p) }
    };
    let walls = |floor: &[HalfSpace]| {
        let mut h = vec![
            hs(Vec3::X, Vec3::new(290.0, 0.0, 0.0)),
            hs(-Vec3::X, Vec3::new(10.0, 0.0, 0.0)),
            hs(Vec3::Y, Vec3::new(0.0, 140.0, 0.0)),
            hs(-Vec3::Y, Vec3::new(0.0, 10.0, 0.0)),
            hs(Vec3::Z, Vec3::new(0.0, 0.0, 90.0)),
        ];
        h.extend_from_slice(floor);
        convex_polyhedron(&h).unwrap()
    };
    let block = box_solid(Vec3::ZERO, Vec3::new(300.0, 150.0, 80.0)).unwrap();
    // One fall along x at 1°: floor z = 8 + t (x − 10).
    let t = 1f64.to_radians().tan();
    let fall = boolean(&block, &walls(&[hs(Vec3::new(t, 0.0, -1.0), Vec3::new(10.0, 0.0, 8.0))]), BoolOp::Cut).unwrap().unwrap();
    let v0 = measure(&fall).unwrap().volume;
    for x in [60.0, 100.0] {
        // A pad from z = 9 to 12 (its bottom just under the floor, its top 2–3 mm above).
        let pad = cylinder(Vec3::new(x, 75.0, 9.0), Vec3::Z, 15.0, 3.0).unwrap();
        let u = boolean(&fall, &pad, BoolOp::Union).unwrap().unwrap();
        // The part of the pad above the floor: the floor under the disc is planar, so its mean
        // height is the height at the centre (where the floor is below z = 9 everywhere).
        let floor = |x: f64| 8.0 + t * (x - 10.0);
        let above = if floor(x - 15.0) >= 9.0 { PI * 225.0 * (12.0 - floor(x)) } else { f64::NAN };
        let v = measure(&u).unwrap().volume;
        if above.is_finite() {
            assert!(rel(v, v0 + above) < 1e-5, "pad at x {x}: {v} vs {}", v0 + above);
        } else {
            assert!(v > v0 && v < v0 + PI * 225.0 * 3.0, "pad at x {x}: {v}");
        }
    }
    // Four faces falling 2° toward a drain point, and a pad over a valley edge and a wall.
    let t2 = 2f64.to_radians().tan();
    let d = Vec3::new(240.0, 110.0, 8.0);
    let hopper = walls(&[
        hs(Vec3::new(t2, 0.0, -1.0), d),
        hs(Vec3::new(-t2, 0.0, -1.0), d),
        hs(Vec3::new(0.0, t2, -1.0), d),
        hs(Vec3::new(0.0, -t2, -1.0), d),
    ]);
    let trough = boolean(&block, &hopper, BoolOp::Cut).unwrap().unwrap();
    let pad = cylinder(Vec3::new(270.0, 70.0, 8.0), Vec3::Z, 25.0, 5.0).unwrap();
    let u = boolean(&trough, &pad, BoolOp::Union).unwrap().unwrap();
    // Integrated over the disc (outside the wall at x = 290): about 6517 mm³ above the floor.
    let gain = measure(&u).unwrap().volume - measure(&trough).unwrap().volume;
    assert!((gain - 6517.0).abs() < 10.0, "{gain}");
}

#[test]
fn planar_booleans_with_coincident_faces() {
    // L-bracket: a wall flush with three sides of its base.
    let base = box_solid(Vec3::ZERO, Vec3::new(80.0, 50.0, 8.0)).unwrap();
    let wall = box_solid(Vec3::new(0.0, 42.0, 8.0), Vec3::new(80.0, 50.0, 50.0)).unwrap();
    let u = planar_boolean(&base, &wall, BoolOp::Union).unwrap().unwrap();
    let m = measure(&u).unwrap();
    assert!(rel(m.volume, 80.0 * 50.0 * 8.0 + 80.0 * 8.0 * 42.0) < 1e-9, "{}", m.volume);
    assert_eq!(m.merged.faces, 8, "{:?}", m.merged);
    // Through the public boolean (B-rep first, polygon fallback).
    let u2 = boolean(&base, &wall, BoolOp::Union).unwrap().unwrap();
    assert!(rel(measure(&u2).unwrap().volume, m.volume) < 1e-9);
    assert_eq!(u2.face_count(), 8, "coplanar wall and base faces must merge in the B-rep");
    assert!(u2.validity().is_empty());
    // Two boxes sharing a face, and a cut flush with a side.
    let a = box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
    let b = box_solid(Vec3::new(10.0, 0.0, 0.0), Vec3::new(20.0, 10.0, 10.0)).unwrap();
    let ab = boolean(&a, &b, BoolOp::Union).unwrap().unwrap();
    assert!(rel(measure(&ab).unwrap().volume, 2000.0) < 1e-9);
    assert_eq!(measure(&ab).unwrap().merged.faces, 6);
    let notch = box_solid(Vec3::new(0.0, 0.0, 5.0), Vec3::new(5.0, 10.0, 10.0)).unwrap();
    let c = boolean(&a, &notch, BoolOp::Cut).unwrap().unwrap();
    assert!(rel(measure(&c).unwrap().volume, 1000.0 - 250.0) < 1e-9);
    let i = boolean(&a, &notch, BoolOp::Intersect).unwrap().unwrap();
    assert!(rel(measure(&i).unwrap().volume, 250.0) < 1e-9);
}

/// The planar fallback runs after any failed B-rep boolean, not only on coincident faces; when
/// it can't run it says which body has which curved faces (it used to blame "coincident faces
/// on curved bodies" whatever the cause).
#[test]
fn planar_fallback_names_the_curved_faces() {
    let slab = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
    let pad = cylinder(Vec3::new(20.0, 15.0, 5.0), Vec3::Z, 8.0, 10.0).unwrap();
    let e = planar_boolean(&slab, &pad, BoolOp::Union).unwrap_err().to_string();
    assert!(e.contains("not supported yet"), "{e}");
    assert!(e.contains("the tool has 2 curved faces (cylinder)"), "{e}");
    assert!(!e.contains("target"), "{e}");
    assert!(!e.contains("coincident faces on curved bodies"), "{e}");
    let ball = sphere(Vec3::new(20.0, 15.0, 10.0), 5.0).unwrap();
    let e = planar_boolean(&ball, &pad, BoolOp::Cut).unwrap_err().to_string();
    assert!(e.contains("the target has 6 curved faces (sphere) and the tool has 2 curved faces (cylinder)"), "{e}");
    // All-planar bodies, a face of one at a shallow angle (1°): the fallback takes them.
    let slope = Vec3::new(-1f64.to_radians().tan(), 0.0, 1.0).normalized().unwrap();
    let wedge = convex_polyhedron(&[
        HalfSpace { n: Vec3::X, d: 30.0 },
        HalfSpace { n: -Vec3::X, d: -10.0 },
        HalfSpace { n: Vec3::Y, d: 20.0 },
        HalfSpace { n: -Vec3::Y, d: -10.0 },
        HalfSpace { n: -Vec3::Z, d: -5.0 },
        HalfSpace { n: slope, d: slope.dot(Vec3::new(10.0, 0.0, 11.0)) },
    ])
    .unwrap();
    let u = planar_boolean(&slab, &wedge, BoolOp::Union).unwrap().unwrap();
    let above = box_solid(Vec3::new(0.0, 0.0, 10.0), Vec3::new(40.0, 30.0, 20.0)).unwrap();
    let expect = 40.0 * 30.0 * 10.0 + measure(&boolean(&wedge, &above, BoolOp::Intersect).unwrap().unwrap()).unwrap().volume;
    assert!(rel(measure(&u).unwrap().volume, expect) < 1e-6, "{} vs {expect}", measure(&u).unwrap().volume);
}

#[test]
fn connected_logo_join_preserves_exact_faces_at_edge_contact() {
    let mut body = box_solid(Vec3::ZERO, Vec3::new(10.0, 20.0, 10.0)).unwrap();
    for (i, (lo, hi)) in
        [(Vec3::new(0.0, 0.0, 10.0), Vec3::new(10.0, 10.0, 20.0)), (Vec3::new(-10.0, 0.0, 10.0), Vec3::new(0.0, 10.0, 20.0))].into_iter().enumerate()
    {
        let tool = box_solid(lo, hi).unwrap();
        body = boolean(&body, &tool, BoolOp::Union).unwrap().unwrap();
        assert!(body.validity().is_empty());
        let m = measure(&body).unwrap();
        assert!((m.volume - (3000.0 + i as f64 * 1000.0)).abs() < 1e-6, "join {i}: {}", m.volume);
        assert!((m.bbox.max.z - 20.0).abs() < 1e-9, "join {i}: {}", m.bbox.max.z);
        let tops =
            body.faces(0.01).unwrap().iter().filter(|f| f.plane_normal.is_some_and(|n| n.z > 0.999999) && (f.centroid.z - 20.0).abs() < 1e-9).count();
        assert_eq!(tops, 1, "join {i}: the top must be a single selectable face");
        assert!(m.bbox.min.dist(Vec3::new(if i == 0 { 0.0 } else { -10.0 }, 0.0, 0.0)) < 1e-9);
        assert!(m.bbox.max.dist(Vec3::new(10.0, 20.0, 20.0)) < 1e-9);
    }
    let tool = box_solid(Vec3::new(-10.0, 10.0, 10.0), Vec3::new(0.0, 20.0, 20.0)).unwrap();
    let logo = boolean(&body, &tool, BoolOp::Union).unwrap().unwrap();
    assert!(logo.validity().is_empty());
    let m = measure(&logo).unwrap();
    assert!((m.volume - 5000.0).abs() < 1e-6);
    assert!(m.bbox.min.dist(Vec3::new(-10.0, 0.0, 0.0)) < 1e-9);
    assert!(m.bbox.max.dist(Vec3::new(10.0, 20.0, 20.0)) < 1e-9);
    let tops =
        logo.faces(0.01).unwrap().iter().filter(|f| f.plane_normal.is_some_and(|n| n.z > 0.999999) && (f.centroid.z - 20.0).abs() < 1e-9).count();
    assert_eq!(tops, 1);
    let text = step_export(&[&logo], "SolidCraft").unwrap();
    let imported = step_import(&text).unwrap();
    assert_eq!(imported.bodies.len(), 1);
    let roundtrip = &imported.bodies.first().unwrap().body;
    assert!(roundtrip.validity().is_empty());
    assert!((measure(roundtrip).unwrap().volume - 5000.0).abs() < 1e-6);
    // An intentional overlap removes the pinch without any automatic displacement.
    let overlap = box_solid(Vec3::new(-10.0, 10.0, 10.0), Vec3::new(0.1, 20.0, 20.0)).unwrap();
    let joined = boolean(&body, &overlap, BoolOp::Union).unwrap().unwrap();
    assert!(joined.validity().is_empty());
    let m = measure(&joined).unwrap();
    assert!((m.volume - 5010.0).abs() < 1e-6);
    assert!(m.bbox.min.dist(Vec3::new(-10.0, 0.0, 0.0)) < 1e-9);
    assert!(m.bbox.max.dist(Vec3::new(10.0, 20.0, 20.0)) < 1e-9);
}

#[test]
fn corner_boss_on_stepped_case_has_one_exact_top_face() {
    let outer = box_solid(Vec3::new(-50.0, -25.0, 0.0), Vec3::new(50.0, 25.0, 25.0)).unwrap();
    let cavity = box_solid(Vec3::new(-48.0, -23.0, 2.0), Vec3::new(48.0, 23.0, 26.0)).unwrap();
    let case = boolean(&outer, &cavity, BoolOp::Cut).unwrap().unwrap();
    let boss = cylinder(Vec3::new(-45.0, 20.0, 2.0), Vec3::Z, 5.0, 23.0).unwrap();
    let joined = boolean(&case, &boss, BoolOp::Union).unwrap().unwrap();
    let common = boolean(&case, &boss, BoolOp::Intersect).unwrap().unwrap();
    let cut = boolean(&case, &boss, BoolOp::Cut).unwrap().unwrap();
    let (a, b, u, i, c) =
        (measure(&case).unwrap(), measure(&boss).unwrap(), measure(&joined).unwrap(), measure(&common).unwrap(), measure(&cut).unwrap());
    assert!(rel(u.volume + i.volume, a.volume + b.volume) < 1e-4);
    assert!(rel(c.volume + i.volume, a.volume) < 1e-4);
    for body in [&joined, &common, &cut] {
        assert!(body.validity().is_empty());
    }
    assert!((u.bbox.max.z - 25.0).abs() < 1e-9, "the boss must not be nudged above the rim");
    let top_faces = |body: &Body| {
        body.faces(0.01).unwrap().iter().filter(|f| f.plane_normal.is_some_and(|n| n.z > 0.999999) && (f.centroid.z - 25.0).abs() < 1e-9).count()
    };
    assert_eq!(top_faces(&joined), 1, "the boss and both walls share one top face");
    let rounded = fillet(&joined, &[Vec3::new(-50.0, 25.0, 12.5)], 5.0).unwrap();
    assert!(rounded.validity().is_empty());
    assert_eq!(top_faces(&rounded), 1, "rounding the outside corner must retain the merged top");
}

#[test]
fn sphere_booleans() {
    let s = sphere(Vec3::ZERO, 15.0).unwrap();
    let m = measure(&s).unwrap();
    assert!(rel(m.volume, 4.0 / 3.0 * PI * 3375.0) < 1e-3, "{}", m.volume);
    assert_eq!((m.merged.faces, m.merged.edges), (1, 0));
    // A ball cut out of the top face of a slab (oracle 14).
    let b = box_solid(Vec3::new(-25.0, -25.0, -20.0), Vec3::new(25.0, 25.0, 0.0)).unwrap();
    let c = boolean(&b, &s, BoolOp::Cut).unwrap().unwrap();
    let m = measure(&c).unwrap();
    assert!(rel(m.volume, 50.0 * 50.0 * 20.0 - 2.0 / 3.0 * PI * 3375.0) < 1e-3, "{}", m.volume);
    assert_eq!((m.merged.faces, m.merged.edges, m.merged.vertices), (7, 13, 9));
}

#[test]
fn similar_spheres_meet() {
    // bherbruck/solidcraft#232: the contact test compared every pair of nearby triangles of the
    // two fine sphere meshes and gave up past its work limit, so their join was refused.
    let a = sphere(Vec3::ZERO, 10.0).unwrap();
    for (x, meet) in [(12.0, true), (4.0, true), (20.0, true), (20.5, false)] {
        let b = sphere(Vec3::new(x, 0.0, 0.0), 10.0).unwrap();
        assert_eq!(a.meets(&b).unwrap(), meet, "centres {x} mm apart");
    }
}

#[test]
#[ignore]
fn debug_convergence() {
    let b = extrude(&Plane::XY, &[Region2 { outer: Loop2::circle(Vec2::ZERO, 15.0), holes: vec![] }], 0.0, 50.0).unwrap().pop().unwrap();
    for t in [0.1, 0.05, 0.02, 0.01, 0.005, 0.002] {
        let m = b.tessellate(t).unwrap();
        let mm = m.measure();
        println!("tol {t}: tris {} vol {} err {:.3e}", m.triangles.len(), mm.volume, mm.volume / (PI * 225.0 * 50.0) - 1.0);
    }
    let m = b.tessellate(0.05).unwrap();
    for f in 0..4u32 {
        let ns: Vec<Vec3> =
            m.triangles.iter().zip(&m.tri_face).filter(|(_, ff)| **ff == f).flat_map(|(t, _)| t.iter().map(|k| m.normals[*k as usize])).collect();
        println!("face {f}: {} normals, first {:?}, max |n.z| {}", ns.len(), ns.first(), ns.iter().map(|n| n.z.abs()).fold(0.0, f64::max));
    }
}

#[test]
#[ignore]
fn debug_coplanar() {
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
    for (z0, z1) in [(0.0, 20.0), (-5.0, 20.0), (0.0, 25.0), (-5.0, 25.0), (5.0, 20.0), (5.0, 25.0)] {
        let c = cylinder(Vec3::new(20.0, 15.0, z0), Vec3::Z, 5.0, z1 - z0).unwrap();
        let r = boolean(&b, &c, BoolOp::Cut);
        println!("cut z {z0}..{z1}: {:?}", r.map(|o| o.map(|b| measure(&b).unwrap().volume)));
        let r = boolean(&b, &c, BoolOp::Union);
        println!("union z {z0}..{z1}: {:?}", r.map(|o| o.map(|b| measure(&b).unwrap().volume)));
    }
    let r =
        Region2 { outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(40.0, 0.0), Vec2::new(40.0, 30.0), Vec2::new(0.0, 30.0)]), holes: vec![] };
    for ang in [PI / 2.0, PI, 1.5 * PI, 2.0 * PI] {
        let q = revolve(&Plane::XY, std::slice::from_ref(&r), Vec2::ZERO, Vec2::Y, ang).unwrap().pop().unwrap();
        let m = measure(&q).unwrap();
        println!("revolve {ang}: vol {} expect {} faces {}", m.volume, ang / 2.0 * 1600.0 * 30.0, m.faces);
    }
}

#[test]
#[ignore]
fn debug_blind() {
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
    let t = box_solid(Vec3::new(10.0, 10.0, 5.0), Vec3::new(20.0, 20.0, 25.0)).unwrap();
    println!("box pocket: {:?}", boolean(&b, &t, BoolOp::Cut).map(|o| o.map(|b| measure(&b).unwrap().volume)));
    let t = box_solid(Vec3::new(10.0, 10.0, 5.0), Vec3::new(20.0, 20.0, 15.0)).unwrap();
    println!("box void: {:?}", boolean(&b, &t, BoolOp::Cut).map(|o| o.map(|b| measure(&b).unwrap().volume)));
    for r in [5.0, 4.3] {
        for z in [5.0, 7.3, 13.1] {
            let c = cylinder(Vec3::new(20.3, 15.1, z), Vec3::Z, r, 30.0).unwrap();
            println!("cyl r{r} z{z}: {:?}", boolean(&b, &c, BoolOp::Cut).map(|o| o.map(|b| measure(&b).unwrap().volume)));
        }
    }
    let c = cylinder(Vec3::new(20.0, 15.0, 25.0), -Vec3::Z, 5.0, 20.0).unwrap();
    println!("cyl down: {:?}", boolean(&b, &c, BoolOp::Cut).map(|o| o.map(|b| measure(&b).unwrap().volume)));
    let s = sphere(Vec3::new(20.0, 15.0, 20.0), 5.0).unwrap();
    println!("sphere: {:?}", boolean(&b, &s, BoolOp::Cut).map(|o| o.map(|b| measure(&b).unwrap().volume)));
}

#[test]
#[ignore]
fn debug_revolve_doc_case() {
    let r =
        Region2 { outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(40.0, 0.0), Vec2::new(40.0, 30.0), Vec2::new(0.0, 30.0)]), holes: vec![] };
    for ang in [PI / 2.0, -PI / 2.0] {
        let q = revolve(&Plane::XY, std::slice::from_ref(&r), Vec2::ZERO, Vec2::Y, ang).unwrap().pop().unwrap();
        let m = measure(&q).unwrap();
        println!("ang {ang}: vol {} bbox {:?}", m.volume, m.bbox);
    }
    let r2 = Region2 { outer: r.outer.reversed(), holes: vec![] };
    let q = revolve(&Plane::XY, std::slice::from_ref(&r2), Vec2::ZERO, Vec2::Y, PI / 2.0).unwrap().pop().unwrap();
    println!("reversed: {}", measure(&q).unwrap().volume);
}

#[test]
#[ignore]
fn debug_coplanar_union_rate() {
    let a = box_solid(Vec3::ZERO, Vec3::new(40.0, 40.0, 20.0)).unwrap();
    let b = extrude(&Plane::XY, &[Region2 { outer: Loop2::circle(Vec2::new(40.0, 20.0), 12.0), holes: vec![] }], 0.0, 40.0).unwrap().pop().unwrap();
    let mut ok = 0;
    for i in 0..20 {
        let sh = Vec3::new(0.0137 * i as f64, 0.0291 * i as f64, -0.0173 * i as f64);
        let (a2, b2) = (transform(&a, sh, Vec3::ZERO, Vec3::Z, 0.0).unwrap(), transform(&b, sh, Vec3::ZERO, Vec3::Z, 0.0).unwrap());
        for tol in [1e-3, 1e-2, 5e-2] {
            let r = guard("t", || {
                let (sa, sb) = (a2.deep_copy(), b2.deep_copy());
                Ok(truck_shapeops::or(&sa, &sb, tol))
            });
            let good = matches!(r, Ok(Some(_)));
            if good {
                ok += 1;
            }
            println!("shift {i} tol {tol}: {}", if good { "ok" } else { "fail" });
        }
    }
    println!("ok {ok}/60");
}

#[test]
fn chamfer_top_loop_mitres() {
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
    let edges = [Vec3::new(0.0, 15.0, 20.0), Vec3::new(20.0, 0.0, 20.0), Vec3::new(40.0, 15.0, 20.0), Vec3::new(20.0, 30.0, 20.0)];
    let c = chamfer(&b, &edges, 2.0).unwrap();
    let m = measure(&c).unwrap();
    assert!(rel(m.volume, 23730.666667) < 1e-6, "{}", m.volume);
    assert_eq!(m.merged.faces, 10);
}

#[test]
fn chamfer_sloped_basin_rim() {
    let slab = box_solid(Vec3::ZERO, Vec3::new(100.0, 60.0, 30.0)).unwrap();
    let rectangle = |x0, y0, x1, y1| Loop2::polygon(&[Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]);
    let pocket =
        loft(&[(Plane::XY.offset(30.0), rectangle(20.0, 4.0, 80.0, 50.0)), (Plane::XY.offset(10.0), rectangle(30.0, 20.0, 70.0, 40.0))]).unwrap();
    let basin = boolean(&slab, &pocket, BoolOp::Cut).unwrap().unwrap();
    let result = chamfer(&basin, &[Vec3::new(50.0, 4.0, 30.0)], 2.0).unwrap();
    // The removed triangular section has area s² sin(phi)/2. Its length varies
    // linearly between the sloped end walls, so its centroid gives the mean length.
    let wall_slope: f64 = (20.0 - 4.0) / (30.0 - 10.0); // horizontal run / vertical drop
    let drop = 2.0 / (1.0 + wall_slope.powi(2)).sqrt();
    let removed = drop * (60.0 - drop / 3.0);
    let before = measure(&basin).unwrap();
    let after = measure(&result).unwrap();
    assert!(rel(after.volume, before.volume - removed) < 1e-8, "{} vs {}", after.volume, before.volume - removed);
    assert_eq!(after.merged.faces, before.merged.faces + 1);
    assert_eq!(result.lumps().unwrap().len(), 1);
    assert!(result.validity().is_empty());
    let mesh = result.tessellate(0.01).unwrap();
    assert!(!mesh.contains(Vec3::new(50.0, 3.5, 29.9)));
    assert!(mesh.contains(Vec3::new(50.0, 3.5, 28.0)));
}

#[test]
fn chamfer_wider_than_sloped_basin_rim_is_refused() {
    for (margin, distance) in [(1.0, 2.0), (1.0, 3.0), (0.5, 4.0), (1.0, 1.0)] {
        let slab = box_solid(Vec3::ZERO, Vec3::new(100.0, 60.0, 30.0)).unwrap();
        let rectangle = |x0, y0, x1, y1| Loop2::polygon(&[Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]);
        let pocket =
            loft(&[(Plane::XY.offset(30.0), rectangle(20.0, margin, 80.0, 50.0)), (Plane::XY.offset(10.0), rectangle(30.0, 20.0, 70.0, 40.0))])
                .unwrap();
        let basin = boolean(&slab, &pocket, BoolOp::Cut).unwrap().unwrap();
        let before = measure(&basin).unwrap();
        let error = chamfer(&basin, &[Vec3::new(50.0, margin, 30.0)], distance).unwrap_err();
        assert!(error.to_string().contains("larger than the neighbouring faces"), "{margin}, {distance}: {error}");
        assert_eq!(measure(&basin).unwrap().volume, before.volume);
        // A smaller chamfer on the same thin rim is still supported.
        let smaller = chamfer(&basin, &[Vec3::new(50.0, margin, 30.0)], margin * 0.5).unwrap();
        assert!(smaller.validity().is_empty());
        assert!(measure(&smaller).unwrap().volume < before.volume);
    }
}

#[test]
#[ignore]
fn debug_chamfer_cut() {
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
    // Prism along x: triangle in the YZ plane beyond the top-front edge.
    let plane = Plane { origin: Vec3::new(-5.0, 0.0, 0.0), x: Vec3::Y, y: Vec3::Z };
    let tri = Loop2::polygon(&[Vec2::new(2.3, 20.3), Vec2::new(-0.3, 17.7), Vec2::new(10.0, 30.0)]);
    let tri = if tri.signed_area() < 0.0 { tri.reversed() } else { tri };
    let tool = extrude(&plane, &[Region2 { outer: tri, holes: vec![] }], 0.0, 50.0).unwrap().pop().unwrap();
    println!("tool vol {}", measure(&tool).unwrap().volume);
    let r = boolean(&b, &tool, BoolOp::Cut).unwrap().unwrap();
    let m = measure(&r).unwrap();
    println!("cut: {} faces {} {:?}", m.volume, m.faces, m.merged);
    for f in r.faces(0.01).unwrap() {
        println!("face {} area {:.3} c {:?} n {:?}", f.index, f.area, f.centroid, f.plane_normal);
    }
    for tol in [0.1, 0.01, 0.001] {
        println!("tess {tol}: {}", r.tessellate(tol).unwrap().measure().volume);
    }
}

#[test]
#[ignore]
fn debug_revolved_hole_tool() {
    let b = box_solid(Vec3::ZERO, Vec3::new(60.0, 40.0, 15.0)).unwrap();
    // Drill point profile in the XZ plane through x = 50: revolve about the vertical line x = 50.
    let pl = Plane { origin: Vec3::new(50.0, 20.0, 0.0), x: Vec3::X, y: Vec3::Z };
    let r = 3.0;
    let tip = r / 59f64.to_radians().tan();
    let prof =
        Region2 { outer: Loop2::polygon(&[Vec2::new(0.0, 16.0), Vec2::new(0.0, 5.0 - tip), Vec2::new(r, 5.0), Vec2::new(r, 16.0)]), holes: vec![] };
    let tool = revolve(&pl, &[prof], Vec2::ZERO, Vec2::Y, std::f64::consts::TAU).unwrap().pop().unwrap();
    println!("tool vol {}", measure(&tool).unwrap().volume);
    let res = boolean(&b, &tool, BoolOp::Cut);
    println!("cut: {:?}", res.map(|o| o.map(|x| measure(&x).unwrap().volume)));
    let expect = 36000.0 - std::f64::consts::PI * 9.0 * 10.0 - std::f64::consts::PI * 9.0 * tip / 3.0;
    println!("expect {expect}");
}

#[test]
#[ignore]
fn debug_sphere() {
    let s = sphere(Vec3::new(1.0, 2.0, 3.0), 10.0).unwrap();
    let m = measure(&s).unwrap();
    println!("vol {} area {} faces {} bbox {:?}", m.volume, m.area, m.faces, m.bbox);
    for f in s.faces(0.05).unwrap() {
        println!("face {} area {:.3} c {:?}", f.index, f.area, f.centroid);
    }
    let b = box_solid(Vec3::new(-25.0, -25.0, -20.0), Vec3::new(25.0, 25.0, 0.0)).unwrap();
    for (name, c) in [("origin", Vec3::ZERO), ("up", Vec3::new(0.0, 0.0, 3.0)), ("off", Vec3::new(1.3, 2.1, 0.7))] {
        let sp = sphere(c, 15.0).unwrap();
        println!("{name} cut {:?}", boolean(&b, &sp, BoolOp::Cut).map(|o| o.map(|x| measure(&x).unwrap().volume)));
        println!("{name} and {:?}", boolean(&b, &sp, BoolOp::Intersect).map(|o| o.map(|x| measure(&x).unwrap().volume)));
    }
}

#[test]
#[ignore]
fn debug_countersink() {
    let b = box_solid(Vec3::ZERO, Vec3::new(60.0, 40.0, 15.0)).unwrap();
    for (name, pts) in [
        ("csink", vec![Vec2::new(0.0, 0.6), Vec2::new(6.6, 0.6), Vec2::new(3.0, -3.0), Vec2::new(3.0, -31.0), Vec2::new(0.0, -31.0)]),
        ("csink-thru-top", vec![Vec2::new(0.0, 0.6), Vec2::new(6.6, 0.6), Vec2::new(3.0, -3.0), Vec2::new(3.0, -16.3), Vec2::new(0.0, -16.3)]),
        (
            "cbore",
            vec![Vec2::new(0.0, 0.6), Vec2::new(7.0, 0.6), Vec2::new(7.0, -5.0), Vec2::new(4.0, -5.0), Vec2::new(4.0, -31.0), Vec2::new(0.0, -31.0)],
        ),
    ] {
        for (pn, pl) in [
            ("xz@top", Plane::new(Vec3::new(35.0, 20.0, 15.0), Vec3::X, Vec3::Z).unwrap()),
            ("yz@top", Plane::new(Vec3::new(35.0, 20.0, 15.0), Vec3::Y, Vec3::Z).unwrap()),
            ("diag", Plane::new(Vec3::new(35.0, 20.0, 15.0), Vec3::new(0.6, 0.8, 0.0), Vec3::Z).unwrap()),
        ] {
            let tool = revolve(&pl, &[Region2 { outer: Loop2::polygon(&pts), holes: vec![] }], Vec2::ZERO, Vec2::Y, std::f64::consts::TAU)
                .unwrap()
                .pop()
                .unwrap();
            println!(
                "{name} {pn}: tool {} cut {:?}",
                measure(&tool).unwrap().volume,
                boolean(&b, &tool, BoolOp::Cut).map(|o| o.map(|x| measure(&x).unwrap().volume))
            );
        }
    }
}

#[test]
#[ignore]
fn debug_cbore_seq() {
    let b = box_solid(Vec3::ZERO, Vec3::new(60.0, 40.0, 15.0)).unwrap();
    let pl = Plane::from_normal(Vec3::new(15.0, 20.0, 15.0), Vec3::Z).unwrap();
    for (s1, s2, r2top) in [(0.0, 0.0, 0.6), (0.613, 1.226, 0.6), (0.3, 0.9, 2.37)] {
        let t1 = extrude(&pl, &[Region2 { outer: Loop2::circle_from(Vec2::ZERO, 4.0, s1), holes: vec![] }], -31.0, 0.6).unwrap().pop().unwrap();
        let c1 = boolean(&b, &t1, BoolOp::Cut).unwrap().unwrap();
        let t2 = extrude(&pl, &[Region2 { outer: Loop2::circle_from(Vec2::ZERO, 7.0, s2), holes: vec![] }], -5.0, r2top).unwrap().pop().unwrap();
        println!(
            "seams {s1} {s2}: first {} second {:?}",
            measure(&c1).unwrap().volume,
            boolean(&c1, &t2, BoolOp::Cut).map(|o| o.map(|x| measure(&x).unwrap().volume))
        );
        let c2 = boolean(&b, &t2, BoolOp::Cut).unwrap().unwrap();
        println!("  swapped: {:?}", boolean(&c2, &t1, BoolOp::Cut).map(|o| o.map(|x| measure(&x).unwrap().volume)));
    }
}

#[test]
#[ignore]
fn debug_truck_sphere() {
    use truck_modeling::{self as mt, builder};
    let mk = |c: Vec3, r: f64, axis: Vec3| -> Body {
        let ax = axis.normalized().unwrap();
        let side = ax.any_perp();
        let v0 = builder::vertex(crate::body::p3(c + ax * r));
        let arc = builder::rsweep(&v0, crate::body::p3(c), crate::body::v3(side), mt::Rad(PI));
        let shell = builder::rsweep(&arc, crate::body::p3(c), crate::body::v3(ax), mt::Rad(2.0 * PI));
        Body::new(mt::Solid::new(vec![shell])).unwrap()
    };
    let b = box_solid(Vec3::new(-25.0, -25.0, -20.0), Vec3::new(25.0, 25.0, 0.0)).unwrap();
    for (name, axis) in [("z", Vec3::Z), ("x", Vec3::X), ("diag", Vec3::new(0.3, 0.5, 0.8))] {
        let sp = mk(Vec3::ZERO, 15.0, axis);
        println!("{name}: faces {} vol {}", sp.face_count(), measure(&sp).unwrap().volume);
        for op in [BoolOp::Cut, BoolOp::Intersect, BoolOp::Union] {
            let r = guard("t", || {
                let (sa, mut sb) = (b.deep_copy(), sp.deep_copy());
                let res = match op {
                    BoolOp::Cut => {
                        sb.not();
                        truck_shapeops::and(&sa, &sb, 0.01)
                    }
                    BoolOp::Intersect => truck_shapeops::and(&sa, &sb, 0.01),
                    BoolOp::Union => truck_shapeops::or(&sa, &sb, 0.01),
                };
                Ok(res.map(|s| s.face_iter().count()))
            });
            println!("  {op:?}: {r:?}");
        }
    }
}

#[test]
#[ignore]
fn debug_sphere_patches() {
    let sp = crate::build::sphere_patches(Vec3::ZERO, 15.0).unwrap();
    let m = measure(&sp).unwrap();
    println!("vol {} want {} area {} merged {:?}", m.volume, 4.0 / 3.0 * PI * 3375.0, m.area, m.merged);
    let b = box_solid(Vec3::new(-25.0, -25.0, -20.0), Vec3::new(25.0, 25.0, 0.0)).unwrap();
    for op in [BoolOp::Cut, BoolOp::Intersect, BoolOp::Union] {
        let r = boolean(&b, &sp, op).map(|o| {
            o.map(|x| {
                let m = measure(&x).unwrap();
                (m.volume, m.merged)
            })
        });
        println!("{op:?}: {r:?}");
    }
    let c = box_solid(Vec3::new(-20.0, -20.0, -20.0), Vec3::new(20.0, 20.0, 20.0)).unwrap();
    let a = 0.3f64;
    let (ca, sa) = (a.cos(), a.sin());
    let b2 = 0.5f64;
    let (cb, sb) = (b2.cos(), b2.sin());
    // Rz(a) * Rx(b)
    let rot = [Vec3::new(ca, sa, 0.0), Vec3::new(-sa * cb, ca * cb, sb), Vec3::new(sa * sb, -ca * sb, cb)];
    for (name, s2) in [
        ("plain", crate::build::sphere_patches(Vec3::ZERO, 25.0).unwrap()),
        ("turned", crate::build::sphere_patches_rotated(Vec3::ZERO, 25.0, rot).unwrap()),
    ] {
        println!("{name} vol {}", measure(&s2).unwrap().volume);
        println!(
            "{name} cube and sphere: {:?}",
            boolean(&c, &s2, BoolOp::Intersect).map(|o| o.map(|x| {
                let m = measure(&x).unwrap();
                (m.volume, m.merged)
            }))
        );
    }
}

#[test]
#[ignore]
fn debug_sphere_cases() {
    let s = crate::build::sphere_patches(Vec3::ZERO, 25.0).unwrap();
    let cases = [
        ("one plane", Vec3::new(-100.0, -100.0, -100.0), Vec3::new(100.0, 100.0, 20.0)),
        ("two planes", Vec3::new(-100.0, -100.0, -20.0), Vec3::new(100.0, 100.0, 20.0)),
        ("three", Vec3::new(-100.0, -100.0, -20.0), Vec3::new(20.0, 100.0, 20.0)),
        ("four", Vec3::new(-100.0, -20.0, -20.0), Vec3::new(20.0, 20.0, 20.0)),
        ("five", Vec3::new(-20.0, -20.0, -20.0), Vec3::new(20.0, 20.0, 100.0)),
        ("cube", Vec3::new(-20.0, -20.0, -20.0), Vec3::new(20.0, 20.0, 20.0)),
        ("cube big sphere-ish", Vec3::new(-24.0, -24.0, -24.0), Vec3::new(24.0, 24.0, 24.0)),
        ("cube off", Vec3::new(-20.3, -19.7, -20.1), Vec3::new(19.9, 20.2, 19.8)),
    ];
    for (name, lo, hi) in cases {
        let b = box_solid(lo, hi).unwrap();
        for op in [BoolOp::Intersect, BoolOp::Cut] {
            let r = boolean(&b, &s, op).map(|o| o.map(|x| measure(&x).unwrap().volume));
            println!("{name} {op:?}: {:?}", r.map_err(|e| e.to_string().chars().take(60).collect::<String>()));
        }
    }
}

#[test]
#[ignore]
fn debug_sphere_rotations() {
    let b = box_solid(Vec3::new(-20.0, -20.0, -20.0), Vec3::new(20.0, 20.0, 20.0)).unwrap();
    for k in 0..8 {
        let (a, c) = (0.37 * k as f64 + 0.1, 0.61 * k as f64 + 0.2);
        let (ca, sa, cc, sc) = (a.cos(), a.sin(), c.cos(), c.sin());
        let rot = [Vec3::new(ca, sa, 0.0), Vec3::new(-sa * cc, ca * cc, sc), Vec3::new(sa * sc, -ca * sc, cc)];
        let s = crate::build::sphere_patches_rotated(Vec3::ZERO, 25.0, rot).unwrap();
        let r = guard("t", || {
            let res = truck_shapeops::and(&b.deep_copy(), &s.deep_copy(), 0.01);
            Ok(res.map(|x| x.face_iter().count()))
        });
        println!("rot {k}: {r:?}");
    }
}

#[test]
#[ignore]
fn debug_cube_sphere_measure() {
    let b = box_solid(Vec3::new(-20.0, -20.0, -20.0), Vec3::new(20.0, 20.0, 20.0)).unwrap();
    let s = crate::build::sphere_patches(Vec3::ZERO, 25.0).unwrap();
    let r = guard("t", || Ok(truck_shapeops::and(&b.deep_copy(), &s.deep_copy(), 0.05))).unwrap().unwrap();
    let body = Body::new(r).unwrap();
    for tol in [0.5, 0.1, 0.02] {
        let m = body.tessellate(tol).unwrap();
        let mm = m.measure();
        println!("tol {tol}: vol {} area {} tris {}", mm.volume, mm.area, m.triangles.len());
    }
    for f in body.faces(0.05).unwrap() {
        println!("face {} area {:.2} c {:?} planar {:?}", f.index, f.area, f.centroid, f.plane_normal.is_some());
    }
}

#[test]
fn mesh_bodies_move_measure_and_refuse_solid_ops() {
    // Unit cube as 12 triangles with repeated corners (welded on construction).
    let c = |i: u32| Vec3::new((i & 1) as f64 * 10.0, ((i >> 1) & 1) as f64 * 10.0, ((i >> 2) & 1) as f64 * 10.0);
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut pos = Vec::new();
    let mut tris = Vec::new();
    for q in quads {
        for t in [[q[0], q[2], q[1]], [q[0], q[3], q[2]]] {
            let n = pos.len() as u32;
            pos.extend(t.iter().map(|i| c(*i)));
            tris.push([n, n + 1, n + 2]);
        }
    }
    let b = crate::mesh_body(&pos, &tris).unwrap();
    assert!(b.is_mesh() && b.is_closed_mesh());
    let m = measure(&b).unwrap();
    assert!((m.volume - 1000.0).abs() < 1e-9 && (m.area - 600.0).abs() < 1e-9, "{m:?}");
    assert_eq!((m.faces, m.edges, m.vertices), (12, 18, 8));
    let moved = crate::transform(&b, Vec3::new(5.0, 0.0, 0.0), Vec3::ZERO, Vec3::Z, 0.7).unwrap();
    assert!((measure(&moved).unwrap().volume - 1000.0).abs() < 1e-9);
    let mirrored = crate::transform_matrix(&b, [[-1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]).unwrap();
    assert!((measure(&mirrored).unwrap().volume - 1000.0).abs() < 1e-9);
    let solid = box_solid(Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0)).unwrap();
    assert!(fillet(&b, &[Vec3::new(5.0, 0.0, 0.0)], 1.0).unwrap_err().to_string().contains("mesh"));
    assert!(boolean(&solid, &b, BoolOp::Union).is_err());
    assert!(crate::step_export(&[&b], "t").is_err());
    assert!(crate::mesh_body(&pos, &[[0, 1, 99]]).is_err());
    assert!(crate::mesh_body(&[Vec3::new(f64::NAN, 0.0, 0.0)], &[[0, 0, 0]]).is_err());
    assert!(crate::mesh_body(&pos, &[[0, 0, 1]]).is_err());
}

#[test]
fn concave_fillet() {
    // An L made of a base and a wall (one solid), fillet the inside corner.
    let base = box_solid(Vec3::ZERO, Vec3::new(80.0, 50.0, 8.0)).unwrap();
    let wall = box_solid(Vec3::new(0.0, 42.0, 0.0), Vec3::new(80.0, 50.0, 50.0)).unwrap();
    let l = boolean(&base, &wall, BoolOp::Union).unwrap().unwrap();
    let v0 = measure(&l).unwrap().volume;
    let f = fillet(&l, &[Vec3::new(40.0, 42.0, 8.0)], 5.0).unwrap();
    let m = measure(&f).unwrap();
    assert!(rel(m.volume, v0 + (25.0 - 25.0 * PI / 4.0) * 80.0) < 1e-4, "{} vs {}", m.volume, v0 + (25.0 - 25.0 * PI / 4.0) * 80.0);
    assert_eq!(m.merged.face_types.get("cylinder"), Some(&1));
}

#[test]
#[ignore]
fn debug_pocket_corners() {
    let b = box_solid(Vec3::ZERO, Vec3::new(80.0, 60.0, 20.0)).unwrap();
    let p = box_solid(Vec3::new(15.0, 15.0, 12.0), Vec3::new(65.0, 45.0, 25.0)).unwrap();
    let mut cur = boolean(&b, &p, BoolOp::Cut).unwrap().unwrap();
    for e in [Vec3::new(65.0, 15.0, 16.0), Vec3::new(15.0, 15.0, 16.0), Vec3::new(65.0, 45.0, 16.0), Vec3::new(15.0, 45.0, 16.0)] {
        match fillet(&cur, &[e], 5.0) {
            Ok(n) => {
                println!("ok {e:?} vol {}", measure(&n).unwrap().volume);
                cur = n;
            }
            Err(err) => {
                println!("fail {e:?}: {err}");
                break;
            }
        }
    }
}

#[test]
fn pocket_floor_loop_fillet() {
    let b = box_solid(Vec3::ZERO, Vec3::new(80.0, 60.0, 20.0)).unwrap();
    let p = box_solid(Vec3::new(15.0, 15.0, 12.0), Vec3::new(65.0, 45.0, 25.0)).unwrap();
    let mut cur = boolean(&b, &p, BoolOp::Cut).unwrap().unwrap();
    cur = fillet(&cur, &[Vec3::new(65.0, 15.0, 16.0), Vec3::new(15.0, 15.0, 16.0), Vec3::new(65.0, 45.0, 16.0), Vec3::new(15.0, 45.0, 16.0)], 5.0)
        .unwrap();
    let v1 = measure(&cur).unwrap().volume;
    let floor: Vec<Vec3> = [(40.0, 15.0), (65.0, 30.0), (40.0, 45.0), (15.0, 30.0)].iter().map(|(x, y)| Vec3::new(*x, *y, 12.0)).collect();
    let mut pts = floor.clone();
    let d = 5.0 - 5.0 / 2f64.sqrt();
    pts.extend([(15.0 + d, 15.0 + d), (65.0 - d, 15.0 + d), (65.0 - d, 45.0 - d), (15.0 + d, 45.0 - d)].iter().map(|(x, y)| Vec3::new(*x, *y, 12.0)));
    let f = fillet(&cur, &pts, 2.0).unwrap();
    let m = measure(&f).unwrap();
    // Added: the fillet profile (r²(1 − π/4)) swept along the floor outline (Pappus for the
    // corners: the profile's centroid sits 2 − 4·2/(3(4 − π))·… inward; check loosely).
    let straight = 2.0 * (40.0 + 20.0);
    let added_lines = (4.0 - PI) * straight;
    assert!(m.volume > v1 + added_lines && m.volume < v1 + added_lines + 4.0 * 2.0 * PI * 5.0, "{} {}", m.volume, v1);
    println!("pocket: {:?} vol {}", m.merged, m.volume);
}

#[test]
fn round_every_edge_of_a_box() {
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
    let mut pts = Vec::new();
    for e in b.edges(0.1).unwrap() {
        pts.push(e.mid);
    }
    let f = fillet(&b, &pts, 3.0).unwrap();
    let m = measure(&f).unwrap();
    let r = 3.0f64;
    let (a, bb, c) = (40.0 - 2.0 * r, 30.0 - 2.0 * r, 20.0 - 2.0 * r);
    // Inner box grown by a ball (Steiner): V = abc + 2r(ab+bc+ca) + πr²(a+b+c) + 4/3πr³.
    let want = a * bb * c + 2.0 * r * (a * bb + bb * c + c * a) + PI * r * r * (a + bb + c) + 4.0 / 3.0 * PI * r.powi(3);
    assert!(rel(m.volume, want) < 1e-3, "{} vs {want}", m.volume);
    assert_eq!((m.merged.faces, m.merged.edges, m.merged.vertices), (26, 48, 24), "{:?}", m.merged);
}

#[test]
#[ignore]
fn debug_heal_sphere_cut() {
    let b = box_solid(Vec3::new(-25.0, -25.0, -20.0), Vec3::new(25.0, 25.0, 0.0)).unwrap();
    let s = sphere(Vec3::ZERO, 15.0).unwrap();
    let r = guard("t", || {
        let mut sb = s.deep_copy();
        sb.not();
        Ok(truck_shapeops::and(&b.deep_copy(), &sb, 0.05))
    })
    .unwrap()
    .unwrap();
    let raw = Body::new(r.clone()).unwrap();
    let healed = Body::new(crate::heal::heal(r, 60.0)).unwrap();
    for (name, x) in [("raw", raw), ("healed", healed)] {
        println!("{name}: vol {} faces {}", measure(&x).unwrap().volume, x.face_count());
        for f in x.faces(0.05).unwrap() {
            println!("  face {} area {:.2} c {:?}", f.index, f.area, f.centroid);
        }
    }
}

#[test]
#[ignore]
fn debug_coplanar_cylinder_union() {
    let a = box_solid(Vec3::ZERO, Vec3::new(40.0, 40.0, 20.0)).unwrap();
    let c = cylinder(Vec3::new(40.0, 20.0, 0.0), Vec3::Z, 12.0, 40.0).unwrap();
    for op in [BoolOp::Union, BoolOp::Cut, BoolOp::Intersect] {
        let r = guard("t", || {
            let (sa, mut sb) = (a.deep_copy(), c.deep_copy());
            Ok(match op {
                BoolOp::Union => truck_shapeops::or(&sa, &sb, 0.05),
                BoolOp::Cut => {
                    sb.not();
                    truck_shapeops::and(&sa, &sb, 0.05)
                }
                BoolOp::Intersect => truck_shapeops::and(&sa, &sb, 0.05),
            }
            .map(|s| s.face_iter().count()))
        });
        println!("{op:?}: {r:?}");
    }
}

#[test]
fn coplanar_curved_booleans() {
    // A box and a cylinder standing on the same plane, the cylinder's axis on the box's side.
    let a = box_solid(Vec3::ZERO, Vec3::new(40.0, 40.0, 20.0)).unwrap();
    let c = cylinder(Vec3::new(40.0, 20.0, 0.0), Vec3::Z, 12.0, 40.0).unwrap();
    let half = PI * 144.0 / 2.0;
    let u = boolean(&a, &c, BoolOp::Union).unwrap().unwrap();
    let m = measure(&u).unwrap();
    assert!(rel(m.volume, 32000.0 + half * 40.0 + half * 20.0) < 1e-3, "{}", m.volume);
    let cut = boolean(&a, &c, BoolOp::Cut).unwrap().unwrap();
    assert!(rel(measure(&cut).unwrap().volume, 32000.0 - half * 20.0) < 1e-3);
    let i = boolean(&a, &c, BoolOp::Intersect).unwrap().unwrap();
    assert!(rel(measure(&i).unwrap().volume, half * 20.0) < 1e-3);
    println!("union {:?}", m.merged);
}

#[test]
fn shell_non_convex() {
    // An L-shaped block opened at the top of its tall part.
    let base = box_solid(Vec3::ZERO, Vec3::new(60.0, 40.0, 10.0)).unwrap();
    let tower = box_solid(Vec3::ZERO, Vec3::new(20.0, 40.0, 40.0)).unwrap();
    let l = boolean(&base, &tower, BoolOp::Union).unwrap().unwrap();
    let s = shell(&l, &[Vec3::new(10.0, 20.0, 40.0)], 2.0).unwrap();
    // Outer volume minus the cavity (the L shrunk by 2, open upward).
    let outer = 60.0 * 40.0 * 10.0 + 20.0 * 40.0 * 30.0;
    let cavity = 56.0 * 36.0 * 6.0 + 16.0 * 36.0 * 32.0;
    let v = measure(&s).unwrap().volume;
    assert!(rel(v, outer - cavity) < 1e-6, "{v} vs {}", outer - cavity);
}

/// A basin: a slab with a tapered pocket and a 4 mm front rim, shelled from below. A wall
/// thicker than the rim (with the pocket's slope) would turn the top face inside out: refused,
/// not a body with wrong mass properties.
#[test]
fn shell_thicker_than_a_rim_is_refused() {
    let slab = box_solid(Vec3::ZERO, Vec3::new(100.0, 60.0, 30.0)).unwrap();
    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| Loop2::polygon(&[Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]);
    let at = |z: f64| Plane::new(Vec3::new(0.0, 0.0, z), Vec3::X, Vec3::Y).unwrap();
    let pocket = loft(&[(at(30.0), rect(20.0, 4.0, 80.0, 50.0)), (at(10.0), rect(30.0, 20.0, 70.0, 40.0))]).unwrap();
    let basin = boolean(&slab, &pocket, BoolOp::Cut).unwrap().unwrap();
    let open = [Vec3::new(50.0, 30.0, 0.0)];
    let e = shell(&basin, &open, 6.0).unwrap_err().to_string();
    assert!(e.contains("rim or ledge"), "{e}");
    let (whole, thin) = (measure(&basin).unwrap(), measure(&shell(&basin, &open, 1.0).unwrap()).unwrap());
    assert!(thin.volume > 0.0 && thin.volume < whole.volume, "{} vs {}", thin.volume, whole.volume);
    let (lo, hi) = (thin.bbox.min, thin.bbox.max);
    let c = thin.centroid;
    assert!(c.x > lo.x && c.x < hi.x && c.y > lo.y && c.y < hi.y && c.z > lo.z && c.z < hi.z, "{c:?}");
    assert!((c.x - 50.0).abs() < 1e-3, "{c:?}");
}

#[test]
#[ignore]
fn debug_drilled_hole() {
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 15.0)).unwrap();
    let r = 3.4;
    let half = (118f64 / 2.0).to_radians();
    let tip = r / half.tan();
    let (top, bottom) = (0.6, -12.0);
    let a = std::env::var("ANG").ok().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    let rp = Plane::new(Vec3::new(20.0, 15.0, 15.0), Vec3::new(a.cos(), a.sin(), 0.0), Vec3::Z).unwrap();
    let tipv = if std::env::var_os("NOTIP").is_some() { 0.0 } else { tip };
    let pts = [Vec2::new(0.0, top), Vec2::new(r, top), Vec2::new(r, bottom), Vec2::new(0.0, bottom - tipv)];
    let region = Region2 { outer: Loop2::polygon(&pts), holes: vec![] };
    let tool = revolve(&rp, &[region], Vec2::ZERO, Vec2::Y, std::f64::consts::TAU).unwrap().pop().unwrap();
    println!("tool faces {} vol {}", tool.face_count(), measure(&tool).unwrap().volume);
    let r = guard("t", || {
        let mut sb = tool.deep_copy();
        sb.not();
        Ok(truck_shapeops::and(&b.deep_copy(), &sb, 0.02).map(|s| s.face_iter().count()))
    });
    println!("cut: {r:?}");
}

#[test]
#[ignore]
fn debug_face_param_areas() {
    use truck_modeling::{BoundedCurve, ParametricCurve, SearchNearestParameter};
    let rp = Plane::new(Vec3::new(20.0, 15.0, 15.0), Vec3::X, Vec3::Z).unwrap();
    let pts = [Vec2::new(0.0, 0.6), Vec2::new(3.4, 0.6), Vec2::new(3.4, -12.0), Vec2::new(0.0, -12.0)];
    let region = Region2 { outer: Loop2::polygon(&pts), holes: vec![] };
    let tool = revolve(&rp, &[region], Vec2::ZERO, Vec2::Y, std::f64::consts::TAU).unwrap().pop().unwrap();
    let ex = cylinder(Vec3::new(20.0, 15.0, 3.0), Vec3::Z, 3.4, 12.6).unwrap();
    for (name, b) in [("revolve", &tool), ("extrude", &ex)] {
        for f in b.solid.face_iter() {
            let s = f.surface();
            let mut poly: Vec<(f64, f64)> = Vec::new();
            for w in f.absolute_boundaries() {
                for e in w.edge_iter() {
                    let c = e.oriented_curve();
                    let (t0, t1) = c.range_tuple();
                    for k in 0..8 {
                        let p = c.subs(t0 + (t1 - t0) * k as f64 / 8.0);
                        if let Some(uv) = s.search_nearest_parameter(p, poly.last().copied(), 100) {
                            poly.push(uv);
                        }
                    }
                }
            }
            let n = poly.len();
            let area: f64 = (0..n)
                .map(|i| {
                    let (a, b) = (poly[i], poly[(i + 1) % n]);
                    a.0 * b.1 - b.0 * a.1
                })
                .sum::<f64>()
                / 2.0;
            println!("{name}: orientation {} param area {area:.4} wires {}", f.orientation(), f.absolute_boundaries().len());
        }
    }
}

#[test]
#[ignore]
fn debug_revolved_tube_cut() {
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 15.0)).unwrap();
    let rp = Plane::new(Vec3::new(20.0, 15.0, 15.0), Vec3::X, Vec3::Z).unwrap();
    for (name, inner) in [("tube", 1.0), ("solid", 0.0)] {
        let pts = [Vec2::new(inner, 0.6), Vec2::new(3.4, 0.6), Vec2::new(3.4, -12.0), Vec2::new(inner, -12.0)];
        let region = Region2 { outer: Loop2::polygon(&pts), holes: vec![] };
        let tool = revolve(&rp, &[region], Vec2::ZERO, Vec2::Y, std::f64::consts::TAU).unwrap().pop().unwrap();
        let r = guard("t", || {
            let mut sb = tool.deep_copy();
            sb.not();
            Ok(truck_shapeops::and(&b.deep_copy(), &sb, 0.02).map(|s| s.face_iter().count()))
        });
        println!("{name}: faces {} cut {r:?}", tool.face_count());
    }
}

#[test]
fn circle_loop_fillets_and_chamfers() {
    // A boss on a plate: concave fillet where it meets the plate, convex at its top.
    let plate = box_solid(Vec3::new(-30.0, -30.0, 0.0), Vec3::new(30.0, 30.0, 8.0)).unwrap();
    let boss = cylinder(Vec3::new(0.0, 0.0, 8.0), Vec3::Z, 10.0, 15.0).unwrap();
    let b = boolean(&plate, &boss, BoolOp::Union).unwrap().unwrap();
    let v0 = measure(&b).unwrap().volume;
    let f = fillet(&b, &[Vec3::new(-10.0, 0.0, 8.0)], 3.0).unwrap();
    let v1 = measure(&f).unwrap().volume;
    // Concave: the profile area r²(1 − π/4) swept round a circle of radius ≈ 10 + r/3.
    let added = 9.0 * (1.0 - PI / 4.0) * 2.0 * PI * (10.0 + 3.0 * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI));
    assert!((v1 - v0 - added).abs() < 0.02 * added, "{} {}", v1 - v0, added);
    let c = chamfer(&f, &[Vec3::new(-10.0, 0.0, 23.0)], 1.0).unwrap();
    let v2 = measure(&c).unwrap().volume;
    // Convex chamfer: a 1×1 triangle round a circle of radius ≈ 10 − 1/3.
    let removed = 0.5 * 2.0 * PI * (10.0 - 1.0 / 3.0);
    assert!((v1 - v2 - removed).abs() < 0.02 * removed, "{} {}", v1 - v2, removed);
}

#[test]
fn trimmed_revolved_faces_measure_exactly() {
    use solidcraft_geom::Seg2;
    use std::f64::consts::FRAC_PI_2;
    // A slot with concave ends sketched on the plane x = 12.5, cut 4 deep into a revolved shaft
    // of radius 12.5 (the cut faces' triangulation spans wide angles unless refined).
    let segs = vec![
        Seg2::Arc { center: Vec2::new(-45.0, 0.0), radius: 4.0, start: FRAC_PI_2, sweep: PI },
        Seg2::Line { a: Vec2::new(-45.0, -4.0), b: Vec2::new(-75.0, -4.0) },
        Seg2::Arc { center: Vec2::new(-75.0, 0.0), radius: 4.0, start: -FRAC_PI_2, sweep: PI },
        Seg2::Line { a: Vec2::new(-75.0, 4.0), b: Vec2::new(-45.0, 4.0) },
    ];
    let pl = Plane::new(Vec3::new(12.5, 0.0, 0.0), Vec3::new(0.0, 0.0, -1.0), Vec3::Y).unwrap();
    let tool = extrude(&pl, &[Region2 { outer: Loop2 { segs }, holes: vec![] }], -4.0, 1.0).unwrap().pop().unwrap();
    let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, -30.0), (12.5, -30.0), (12.5, -90.0), (10.0, -90.0), (10.0, -120.0), (0.0, -120.0)];
    let segs = (0..8).map(|i| Seg2::Line { a: Vec2::new(pts[i].0, pts[i].1), b: Vec2::new(pts[(i + 1) % 8].0, pts[(i + 1) % 8].1) }).collect();
    let xz = Plane::new(Vec3::ZERO, Vec3::X, Vec3::new(0.0, 0.0, -1.0)).unwrap();
    let shaft = revolve(&xz, &[Region2 { outer: Loop2 { segs }, holes: vec![] }], Vec2::ZERO, Vec2::new(0.0, -1.0), 2.0 * PI).unwrap().pop().unwrap();
    let c = boolean(&shaft, &tool, BoolOp::Cut).unwrap().unwrap();
    // Removed: ∫∫ over the slot of (√(12.5² − y²) − 8.5), by a fine midpoint rule.
    let (n, mut want) = (800, 0.0);
    for i in 0..n {
        let z = 30.0 + 60.0 * (i as f64 + 0.5) / n as f64;
        for j in 0..n {
            let y = -4.0 + 8.0 * (j as f64 + 0.5) / n as f64;
            if (45.0..=75.0).contains(&z) && (z - 45.0).powi(2) + y * y >= 16.0 && (z - 75.0).powi(2) + y * y >= 16.0 {
                want += ((156.25 - y * y).sqrt() - 8.5) * (60.0 / n as f64) * (8.0 / n as f64);
            }
        }
    }
    let got = measure(&shaft).unwrap().volume - measure(&c).unwrap().volume;
    assert!((got - want).abs() < 0.01 * want, "removed {got}, want {want}");
}

#[test]
fn shell_with_cylindrical_faces() {
    // A box with its four vertical edges rounded, opened at the top.
    let b = box_solid(Vec3::ZERO, Vec3::new(100.0, 60.0, 30.0)).unwrap();
    let edges: Vec<Vec3> = [(0.0, 0.0), (100.0, 0.0), (100.0, 60.0), (0.0, 60.0)].iter().map(|(x, y)| Vec3::new(*x, *y, 15.0)).collect();
    let r = fillet(&b, &edges, 6.0).unwrap();
    let s = shell(&r, &[Vec3::new(50.0, 30.0, 30.0)], 2.0).unwrap();
    let outer = (100.0 * 60.0 - (4.0 - PI) * 36.0) * 30.0;
    let cavity = (96.0 * 56.0 - (4.0 - PI) * 16.0) * 28.0;
    let v = measure(&s).unwrap().volume;
    assert!(rel(v, outer - cavity) < 1e-4, "{v} vs {}", outer - cavity);
    // A plate with a round boss, opened underneath: the cavity follows the boss.
    let plate = box_solid(Vec3::new(-30.0, -30.0, 0.0), Vec3::new(30.0, 30.0, 10.0)).unwrap();
    let boss = cylinder(Vec3::new(0.0, 0.0, 5.0), Vec3::Z, 12.0, 25.0).unwrap();
    let pb = boolean(&plate, &boss, BoolOp::Union).unwrap().unwrap();
    let s = shell(&pb, &[Vec3::new(0.0, 0.0, 0.0)], 2.0).unwrap();
    let outer = 3600.0 * 10.0 + PI * 144.0 * 20.0;
    let cavity = 56.0 * 56.0 * 8.0 + PI * 100.0 * 20.0;
    let v = measure(&s).unwrap().volume;
    assert!(rel(v, outer - cavity) < 1e-4, "{v} vs {}", outer - cavity);
}

#[test]
fn draft_of_a_round_ended_pocket() {
    use solidcraft_geom::Seg2;
    use std::f64::consts::FRAC_PI_2;
    let block = box_solid(Vec3::ZERO, Vec3::new(80.0, 60.0, 25.0)).unwrap();
    let segs = vec![
        Seg2::Line { a: Vec2::new(25.0, 20.0), b: Vec2::new(55.0, 20.0) },
        Seg2::Arc { center: Vec2::new(55.0, 30.0), radius: 10.0, start: -FRAC_PI_2, sweep: PI },
        Seg2::Line { a: Vec2::new(55.0, 40.0), b: Vec2::new(25.0, 40.0) },
        Seg2::Arc { center: Vec2::new(25.0, 30.0), radius: 10.0, start: FRAC_PI_2, sweep: PI },
    ];
    let tool = extrude(&Plane::XY, &[Region2 { outer: Loop2 { segs }, holes: vec![] }], 15.0, 30.0).unwrap().pop().unwrap();
    let b = boolean(&block, &tool, BoolOp::Cut).unwrap().unwrap();
    let walls = [Vec3::new(15.0, 30.0, 20.0), Vec3::new(40.0, 20.0, 20.0), Vec3::new(65.0, 30.0, 20.0), Vec3::new(40.0, 40.0, 20.0)];
    let a = 5f64.to_radians();
    let d = draft(&b, &walls, &Plane::XY.offset(25.0), Vec3::Z, a).unwrap();
    // The pocket narrows going down: at depth t its walls stand t·tan(a) further in.
    let (n, mut pocket) = (2000, 0.0);
    for k in 0..n {
        let e = (k as f64 + 0.5) / n as f64 * 10.0 * a.tan();
        pocket += (30.0 * (20.0 - 2.0 * e) + PI * (10.0 - e).powi(2)) * 10.0 / n as f64;
    }
    let v = measure(&d).unwrap().volume;
    assert!(rel(v, 80.0 * 60.0 * 25.0 - pocket) < 1e-4, "{v} vs {}", 80.0 * 60.0 * 25.0 - pocket);
}

/// Rounded edges elsewhere on the body don't stop a draft: only the faces round the drafted
/// walls' corners must be walls or caps, the rest keep their surfaces and edges.
#[test]
fn draft_of_a_pocket_on_a_body_with_a_far_rounded_edge() {
    let block = box_solid(Vec3::ZERO, Vec3::new(80.0, 60.0, 25.0)).unwrap();
    let tool = box_solid(Vec3::new(25.0, 20.0, 15.0), Vec3::new(55.0, 40.0, 30.0)).unwrap();
    let b = boolean(&block, &tool, BoolOp::Cut).unwrap().unwrap();
    // The bottom edge along x at y = 0: a cylinder (and two sphere-free ends) far from the pocket.
    let r = 2.0;
    let b = fillet(&b, &[Vec3::new(40.0, 0.0, 0.0)], r).unwrap();
    let faces_before = measure(&b).unwrap().merged.faces;
    let walls = [Vec3::new(25.0, 30.0, 20.0), Vec3::new(40.0, 20.0, 20.0), Vec3::new(55.0, 30.0, 20.0), Vec3::new(40.0, 40.0, 20.0)];
    let a = 5f64.to_radians();
    let d = draft(&b, &walls, &Plane::XY.offset(25.0), Vec3::Z, a).unwrap();
    // The pocket narrows going down: at depth t its walls stand t·tan(a) further in.
    let (n, mut pocket) = (2000, 0.0);
    for k in 0..n {
        let e = (k as f64 + 0.5) / n as f64 * 10.0 * a.tan();
        pocket += (30.0 - 2.0 * e) * (20.0 - 2.0 * e) * 10.0 / n as f64;
    }
    let round = (r * r - PI * r * r / 4.0) * 80.0;
    let want = 80.0 * 60.0 * 25.0 - round - pocket;
    let m = measure(&d).unwrap();
    assert!(rel(m.volume, want) < 1e-4, "{} vs {want}", m.volume);
    assert_eq!(m.merged.faces, faces_before, "the round and every other face survive");
}

/// A round wall is stored in pieces (a cylinder's two halves); one pick drafts all of it (#547).
/// A hole drafted from the top narrows going down: a frustum from R 10 to 10 − 25 · tan 5°.
#[test]
fn draft_of_a_round_hole_from_one_pick() {
    let block = box_solid(Vec3::ZERO, Vec3::new(60.0, 60.0, 25.0)).unwrap();
    let hole = cylinder(Vec3::new(30.0, 30.0, -1.0), Vec3::Z, 10.0, 27.0).unwrap();
    let b = boolean(&block, &hole, BoolOp::Cut).unwrap().unwrap();
    let a = 5f64.to_radians();
    let d = draft(&b, &[Vec3::new(40.0, 30.0, 12.5)], &Plane::XY.offset(25.0), Vec3::Z, a).unwrap();
    let (big, small) = (10.0, 10.0 - 25.0 * a.tan());
    let want = 60.0 * 60.0 * 25.0 - PI * 25.0 / 3.0 * (big * big + big * small + small * small);
    let m = measure(&d).unwrap();
    assert!(rel(m.volume, want) < 1e-4, "{} vs {want}", m.volume);
}

#[test]
fn rounded_corners_rounded_again_make_spheres() {
    // Vertical edges first, then the top and bottom loops with the same radius: the corners
    // become sphere octants, as if every edge were rounded at once.
    let (a, b, c, r) = (40.0, 30.0, 20.0, 2.0);
    let bx = box_solid(Vec3::ZERO, Vec3::new(a, b, c)).unwrap();
    let vert: Vec<Vec3> = [(0.0, 0.0), (a, 0.0), (a, b), (0.0, b)].iter().map(|(x, y)| Vec3::new(*x, *y, c / 2.0)).collect();
    let f = fillet(&bx, &vert, r).unwrap();
    let f = fillet(&f, &[Vec3::new(a / 2.0, 0.0, c)], r).unwrap();
    let f = fillet(&f, &[Vec3::new(a / 2.0, 0.0, 0.0)], r).unwrap();
    let (ai, bi, ci) = (a - 2.0 * r, b - 2.0 * r, c - 2.0 * r);
    let area = 2.0 * (ai * bi + bi * ci + ai * ci) + PI * r * (ai + bi + ci) * 2.0 + 4.0 * PI * r * r;
    let vol = ai * bi * ci + 2.0 * r * (ai * bi + bi * ci + ai * ci) + PI * r * r * (ai + bi + ci) + 4.0 / 3.0 * PI * r * r * r;
    let m = measure(&f).unwrap();
    println!("area {} want {area}; volume {} want {vol}; faces {:?}", m.area, m.volume, m.merged);
    assert!(rel(m.volume, vol) < 1e-4, "volume {} vs {vol}", m.volume);
    assert!(rel(m.area, area) < 1e-4, "area {} vs {area}", m.area);
}

#[test]
fn every_edge_of_an_l_block() {
    // An L (60 × 40, legs 8 wide) 20 thick with all 18 edges rounded by 2: the inner vertical
    // edge is concave (its corners become tori), the others convex (sphere corners).
    let pts = [(0.0, 0.0), (60.0, 0.0), (60.0, 8.0), (8.0, 8.0), (8.0, 40.0), (0.0, 40.0)];
    let outer = Loop2::polygon(&pts.iter().map(|(x, y)| Vec2::new(*x, *y)).collect::<Vec<_>>());
    let l = extrude(&Plane::XY, &[Region2 { outer, holes: vec![] }], 0.0, 20.0).unwrap().pop().unwrap();
    let edges: Vec<Vec3> = l.edges(0.1).unwrap().iter().map(|e| e.mid).collect();
    let f = fillet(&l, &edges, 2.0).unwrap();
    let m = measure(&f).unwrap();
    // Area by pieces: walls (lengths less the roundings, 16 high), top and bottom (the L shrunk
    // by 2 with a radius-4 inner corner), quarter cylinders along every straight edge, ten
    // sphere octants, and two torus quarters (Pappus: arc π, centroid radius 4 − 4/π, angle π/2).
    let walls = 16.0 * (56.0 + 4.0 + 48.0 + 28.0 + 4.0 + 36.0);
    let caps = 2.0 * (56.0 * 4.0 + 4.0 * 32.0 + 16.0 - 4.0 * PI);
    let vertical = 6.0 * PI * 16.0;
    let horizontal = 2.0 * PI * (56.0 + 4.0 + 48.0 + 28.0 + 4.0 + 36.0);
    let spheres = 10.0 * 2.0 * PI;
    let tori = 2.0 * PI * (4.0 - 4.0 / PI) * PI / 2.0;
    let area = walls + caps + vertical + horizontal + spheres + tori;
    assert!(rel(m.area, area) < 1e-4, "area {} vs {area}", m.area);
    assert_eq!((m.merged.faces, m.merged.edges, m.merged.vertices), (38, 74, 38));
}

#[test]
fn overlapping_holes_and_mirror_joins() {
    // Overlapping through holes in a plate: done on the plate's section, exactly.
    let plate = box_solid(Vec3::new(-50.0, -50.0, 0.0), Vec3::new(50.0, 50.0, 5.0)).unwrap();
    let hole = |x: f64, y: f64| {
        let c = Loop2::circle(Vec2::new(x, y), 5.0);
        extrude(&Plane::XY, &[Region2 { outer: c, holes: vec![] }], -1.0, 6.0).unwrap().pop().unwrap()
    };
    let mut cur = plate;
    for (x, y) in [(30.0, 0.0), (25.0, 0.0)] {
        cur = boolean(&cur, &hole(x, y), BoolOp::Cut).unwrap().unwrap();
    }
    // Two discs of radius 5, centres 5 apart: their union's area is 2·25π − (50π/3 − 25√3/2).
    let lens = 50.0 * PI / 3.0 - 25.0 * 3f64.sqrt() / 2.0;
    let want = 50000.0 - (50.0 * PI - lens) * 5.0;
    assert!(rel(measure(&cur).unwrap().volume, want) < 1e-5, "{} vs {want}", measure(&cur).unwrap().volume);
    // A bracket with rounded corners joined with its mirror image across the face they share.
    let half = box_solid(Vec3::ZERO, Vec3::new(30.0, 40.0, 10.0)).unwrap();
    let half = fillet(&half, &[Vec3::new(30.0, 0.0, 5.0), Vec3::new(30.0, 40.0, 5.0)], 5.0).unwrap();
    let m = [[-1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    let other = transform_matrix(&half, m).unwrap();
    let both = boolean(&half, &other, BoolOp::Union).unwrap().unwrap();
    let v = measure(&half).unwrap().volume;
    let mb = measure(&both).unwrap();
    assert!(rel(mb.volume, 2.0 * v) < 1e-9, "{} vs {}", mb.volume, 2.0 * v);
    // One body: the shared face is gone and the faces either side of it merged.
    assert_eq!(mb.merged.faces, 10, "{:?}", mb.merged);
    // Two equal cylinders on one axis, overlapping: one longer cylinder.
    let c1 = cylinder(Vec3::ZERO, Vec3::Z, 10.0, 30.0).unwrap();
    let c2 = cylinder(Vec3::new(0.0, 0.0, 20.0), Vec3::Z, 10.0, 30.0).unwrap();
    let u = measure(&boolean(&c1, &c2, BoolOp::Union).unwrap().unwrap()).unwrap();
    let v1 = measure(&c1).unwrap().volume;
    assert!(rel(u.volume, v1 * 5.0 / 3.0) < 2e-4, "{} (one: {v1})", u.volume);
    assert_eq!(u.merged.faces, 3, "{:?}", u.merged);
}

#[test]
fn torus_cut_through_its_middle_plane() {
    // A torus whose tube centre lies in a box's top face: the box's top cuts the torus where
    // the tube is vertical. (The revolve profile's seam points used to lie in that plane.)
    let bx = box_solid(Vec3::new(-30.0, -30.0, -10.0), Vec3::new(30.0, 30.0, 0.0)).unwrap();
    let xz = Plane::new(Vec3::ZERO, Vec3::X, Vec3::new(0.0, 0.0, -1.0)).unwrap();
    let ring = Region2 { outer: Loop2::circle(Vec2::new(22.0, 0.0), 4.0), holes: vec![] };
    let torus = revolve(&xz, &[ring], Vec2::ZERO, Vec2::new(0.0, -1.0), 2.0 * PI).unwrap().pop().unwrap();
    let c = boolean(&bx, &torus, BoolOp::Cut).unwrap().unwrap();
    // Half the torus volume (2π²Rr²) is removed.
    let want = 36000.0 - PI * PI * 22.0 * 16.0;
    let v = measure(&c).unwrap().volume;
    assert!(rel(v, want) < 1e-4, "{v} vs {want}");
    // A revolved tube cut across: both walls cross the box's top face (nested loops on it).
    let tube =
        Region2 { outer: Loop2::polygon(&[Vec2::new(5.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, -30.0), Vec2::new(5.0, -30.0)]), holes: vec![] };
    let t = revolve(&xz, &[tube], Vec2::ZERO, Vec2::new(0.0, -1.0), 2.0 * PI).unwrap().pop().unwrap();
    let bx = box_solid(Vec3::new(-40.0, -40.0, -40.0), Vec3::new(40.0, 40.0, 10.0)).unwrap();
    let i = boolean(&t, &bx, BoolOp::Intersect).unwrap().unwrap();
    let v = measure(&i).unwrap().volume;
    assert!(rel(v, PI * 75.0 * 10.0) < 1e-4, "{v}");
}

#[test]
fn fillet_where_a_branch_pipe_meets_a_main_pipe() {
    // A branch (radius 10) on a main pipe (radius 15): the saddle curve between the two
    // cylinders, blended by rolling a ball of radius 3 along it.
    let main = cylinder(Vec3::new(-40.0, 0.0, 0.0), Vec3::X, 15.0, 80.0).unwrap();
    let branch = cylinder(Vec3::ZERO, Vec3::Z, 10.0, 35.0).unwrap();
    let tee = boolean(&main, &branch, BoolOp::Union).unwrap().unwrap();
    let v0 = measure(&tee).unwrap().volume;
    let f = fillet(&tee, &[Vec3::new(0.0, 10.0, 125f64.sqrt())], 3.0).unwrap();
    let m = measure(&f).unwrap();
    // The added material: less than r²(1 − π/4) (a right-angled corner's) swept round the
    // ≈ 65 mm saddle, as the pipes meet at obtuse angles, but well over a third of it.
    let bare = 9.0 * (1.0 - PI / 4.0) * 65.0;
    let added = m.volume - v0;
    assert!(added > 0.35 * bare && added < bare, "added {added} (bare {bare})");
    // Main cylinder, branch cylinder, the blend, the branch's top and the main's two ends.
    assert_eq!(m.merged.faces, 6, "{:?}", m.merged);
}

#[test]
fn fillet_an_open_curved_edge() {
    use solidcraft_geom::Seg2;
    // A D-shaped block: the round top edge only, ending at the flat wall's sharp corners.
    let d = Loop2 {
        segs: vec![
            Seg2::Arc { center: Vec2::ZERO, radius: 10.0, start: 0.0, sweep: PI },
            Seg2::Line { a: Vec2::new(-10.0, 0.0), b: Vec2::new(10.0, 0.0) },
        ],
    };
    let b = extrude(&Plane::XY, &[Region2 { outer: d, holes: vec![] }], 0.0, 15.0).unwrap().pop().unwrap();
    let v0 = measure(&b).unwrap().volume;
    let f = fillet(&b, &[Vec3::new(0.0, 10.0, 15.0)], 2.0).unwrap();
    // (A fine tessellation: the difference of two measured volumes magnifies their error.)
    let _ = v0;
    let fine = |x: &Body| x.tessellate_with(0.002, true).unwrap().measure().volume;
    let removed = fine(&b) - fine(&f);
    // Pappus: the corner's cross-section r²(1 − π/4), its centroid 0.2234 r in from the wall.
    let r = 2.0;
    let want = r * r * (1.0 - PI / 4.0) * (10.0 - r * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI))) * PI;
    assert!(rel(removed, want) < 1e-3, "{removed} vs {want}");
}

fn mixed_arc_and_line_prism() -> Body {
    use solidcraft_geom::Seg2;
    let d = Loop2 {
        segs: vec![
            Seg2::Arc { center: Vec2::ZERO, radius: 10.0, start: 0.0, sweep: PI },
            Seg2::Line { a: Vec2::new(-10.0, 0.0), b: Vec2::new(10.0, 0.0) },
        ],
    };
    extrude(&Plane::XY, &[Region2 { outer: d, holes: vec![] }], 0.0, 15.0).unwrap().pop().unwrap()
}

#[test]
fn fillet_smooth_arc_and_line_loop_keeps_exact_removal() {
    use solidcraft_geom::Seg2;
    // A stadium: four tangent joints, none of which should use a sharp-corner mitre.
    let outline = Loop2 {
        segs: vec![
            Seg2::Line { a: Vec2::new(-10.0, -10.0), b: Vec2::new(10.0, -10.0) },
            Seg2::Arc { center: Vec2::new(10.0, 0.0), radius: 10.0, start: -PI / 2.0, sweep: PI },
            Seg2::Line { a: Vec2::new(10.0, 10.0), b: Vec2::new(-10.0, 10.0) },
            Seg2::Arc { center: Vec2::new(-10.0, 0.0), radius: 10.0, start: PI / 2.0, sweep: PI },
        ],
    };
    let body = extrude(&Plane::XY, &[Region2 { outer: outline, holes: vec![] }], 0.0, 15.0).unwrap().pop().unwrap();
    let blended =
        fillet(&body, &[Vec3::new(0.0, -10.0, 15.0), Vec3::new(20.0, 0.0, 15.0), Vec3::new(0.0, 10.0, 15.0), Vec3::new(-20.0, 0.0, 15.0)], 0.5)
            .unwrap();
    assert!(blended.validity().is_empty(), "{:?}", blended.validity());
    let r = 0.5;
    let area = r * r * (1.0 - PI / 4.0);
    let centroid = r * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI));
    let want = area * (40.0 + 2.0 * PI * (10.0 - centroid));
    let fine = |b: &Body| b.tessellate_with(1e-4, true).unwrap().measure().volume;
    let removed = fine(&body) - fine(&blended);
    assert!(rel(removed, want) < 1e-3, "{removed} vs {want}");
}

#[test]
fn fillet_sharp_arc_and_arc_loop() {
    use solidcraft_geom::Seg2;
    // The intersection of two radius-10 disks whose centres are 10 mm apart.
    let outline = Loop2 {
        segs: vec![
            Seg2::Arc { center: Vec2::new(-5.0, 0.0), radius: 10.0, start: -PI / 3.0, sweep: 2.0 * PI / 3.0 },
            Seg2::Arc { center: Vec2::new(5.0, 0.0), radius: 10.0, start: 2.0 * PI / 3.0, sweep: 2.0 * PI / 3.0 },
        ],
    };
    let body = extrude(&Plane::XY, &[Region2 { outer: outline, holes: vec![] }], 0.0, 15.0).unwrap().pop().unwrap();
    let blended = fillet(&body, &[Vec3::new(5.0, 0.0, 15.0), Vec3::new(-5.0, 0.0, 15.0)], 0.5).unwrap();
    assert!(blended.validity().is_empty(), "{:?}", blended.validity());
    let mesh = blended.tessellate_with(1e-4, true).unwrap();
    for p in &mesh.positions {
        assert!((p.x - 5.0).hypot(p.y) <= 10.0 + 1e-3 && (p.x + 5.0).hypot(p.y) <= 10.0 + 1e-3, "outside the lens: {p:?}");
    }
    let area = |rho: f64| 2.0 * rho * rho * (5.0 / rho).acos() - 10.0 * (rho * rho - 25.0).sqrt();
    let dt = (PI / 2.0) / 8192.0;
    let want: f64 = (0..8192)
        .map(|i| {
            let theta = (f64::from(i) + 0.5) * dt;
            (area(10.0) - area(10.0 - 0.5 * (1.0 - theta.cos()))) * 0.5 * theta.cos() * dt
        })
        .sum();
    let removed = body.tessellate_with(1e-4, true).unwrap().measure().volume - mesh.measure().volume;
    assert!(rel(removed, want) < 1e-3, "{removed} vs {want}");
}

#[test]
fn fillet_tabbed_plate_outer_loop_and_partial_selection_after_hole() {
    use solidcraft_geom::Seg2;
    // Reconstruct the outer outline from its seven public pick coordinates:
    // R25 plate, x=+/-4 tab sides, R2 corners and a 4 mm straight top.
    // The hole's dimensions come from the public reproduction, not an original design file.
    let theta = (4.0_f64 / 25.0).acos();
    let y = (625.0_f64 - 16.0).sqrt();
    let corner_y = 280.0 / 9.0;
    let hole_y = 262.0 / 9.0;
    let outline = Loop2 {
        segs: vec![
            Seg2::Arc { center: Vec2::ZERO, radius: 25.0, start: PI - theta, sweep: PI + 2.0 * theta },
            Seg2::Line { a: Vec2::new(4.0, y), b: Vec2::new(4.0, corner_y) },
            Seg2::Arc { center: Vec2::new(2.0, corner_y), radius: 2.0, start: 0.0, sweep: PI / 2.0 },
            Seg2::Line { a: Vec2::new(2.0, corner_y + 2.0), b: Vec2::new(-2.0, corner_y + 2.0) },
            Seg2::Arc { center: Vec2::new(-2.0, corner_y), radius: 2.0, start: PI / 2.0, sweep: PI / 2.0 },
            Seg2::Line { a: Vec2::new(-4.0, corner_y), b: Vec2::new(-4.0, y) },
        ],
    };
    let body =
        extrude(&Plane::XY, &[Region2 { outer: outline, holes: vec![Loop2::circle(Vec2::new(0.0, hole_y), 1.5)] }], 0.0, 2.0).unwrap().pop().unwrap();
    let inner = fillet(&body, &[Vec3::new(1.5, hole_y, 2.0)], 0.5).unwrap();
    let edges = [
        Vec3::new(-24.91935125502191, -2.0064727829797544, 2.0),
        Vec3::new(24.919351255429795, -2.006472777913911, 2.0),
        Vec3::new(3.999999997492012, 27.894518234675537, 2.0),
        Vec3::new(3.4142135598750105, 32.525324672801695, 2.0),
        Vec3::new(-2.480541994032137e-9, 33.11111111046627, 2.0),
        Vec3::new(-3.4142135648720653, 32.52532467287703, 2.0),
        Vec3::new(-4.0000000025087745, 27.89451823429679, 2.0),
    ];
    // All 127 nonempty subsets: the whole rim, each individual edge, and every
    // contiguous or disjoint partial selection of the issue's seven picks.
    for mask in 1_u32..(1 << edges.len()) {
        let picks: Vec<_> = edges.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, p)| *p).collect();
        let blended = fillet(&inner, &picks, 0.5).unwrap_or_else(|e| panic!("{picks:?}: {e}"));
        assert!(blended.validity().is_empty(), "{:?}", blended.validity());
        let mesh = blended.display_mesh(0.01).unwrap();
        for p in &mesh.positions {
            let inside = p.x.hypot(p.y) <= 25.001
                || (p.x.abs() <= 4.001 && p.y >= y - 0.001 && p.y <= corner_y)
                || (p.x.abs() <= 2.001 && p.y >= corner_y && p.y <= corner_y + 2.001)
                || (p.x - 2.0).hypot(p.y - corner_y) <= 2.001
                || (p.x + 2.0).hypot(p.y - corner_y) <= 2.001;
            assert!(inside && p.z >= -0.001 && p.z <= 2.001, "outside the plate: {p:?}");
        }
    }
}

fn partial_mixed_run_prism() -> Body {
    use solidcraft_geom::Seg2;
    let outline = Loop2 {
        segs: vec![
            Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(40.0, 0.0) },
            Seg2::Line { a: Vec2::new(40.0, 0.0), b: Vec2::new(40.0, 20.0) },
            Seg2::Line { a: Vec2::new(40.0, 20.0), b: Vec2::new(30.0, 20.0) },
            Seg2::Arc { center: Vec2::new(20.0, 20.0), radius: 10.0, start: 0.0, sweep: PI },
            Seg2::Line { a: Vec2::new(10.0, 20.0), b: Vec2::new(0.0, 20.0) },
            Seg2::Line { a: Vec2::new(0.0, 20.0), b: Vec2::new(0.0, 0.0) },
        ],
    };
    extrude(&Plane::XY, &[Region2 { outer: outline, holes: vec![] }], 0.0, 15.0).unwrap().pop().unwrap()
}

#[test]
fn mixed_loop_blend_guards_selection_and_construction() {
    let body = mixed_arc_and_line_prism();
    let solid = body.deep_copy();
    let top = solid
        .boundaries()
        .iter()
        .flat_map(|shell| shell.face_iter())
        .find(|face| face.edge_iter().all(|edge| (edge.front().point().z - 15.0).abs() < 1e-9 && (edge.back().point().z - 15.0).abs() < 1e-9))
        .unwrap();
    let edges: Vec<_> = top.edge_iter().map(|edge| edge.id()).collect();
    // The internal test-only assertions check the active guard during both selection and build.
    assert!(!crate::guarded());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::loopblend::loop_blend(&solid, &edges, body.size(), 0.5, true)));
    assert!(matches!(result, Ok(Some(Ok(_)))), "{result:?}");
    assert!(!crate::guarded());
    // A degenerate profile still returns a clean error and restores the guard depth.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::loopblend::loop_blend(&solid, &edges, body.size(), 0.0, true)));
    assert!(matches!(result, Ok(Some(Err(_)))), "{result:?}");
    assert!(!crate::guarded());
}

#[test]
fn fillet_mixed_arc_and_line_perimeter_edges() {
    let body = mixed_arc_and_line_prism();
    let blended = fillet(&body, &[Vec3::new(0.0, 10.0, 15.0), Vec3::new(5.0, 0.0, 15.0)], 0.5).unwrap();
    assert!(blended.validity().is_empty(), "{:?}", blended.validity());
    let display_tol = (blended.size() * 1e-3).max(1e-3);
    for mesh in [
        blended.tessellate_with(1e-4, true).unwrap(),
        blended.display_mesh_angular(display_tol, crate::DISPLAY_ANGLE).unwrap(),
        blended.reference_display_mesh_angular(display_tol, crate::DISPLAY_ANGLE).unwrap(),
    ] {
        let bounds = mesh.bounds();
        let tol = 1e-3;
        assert!(
            bounds.min.x >= -10.0 - tol
                && bounds.max.x <= 10.0 + tol
                && bounds.min.y >= -tol
                && bounds.max.y <= 10.0 + tol
                && bounds.min.z >= -tol
                && bounds.max.z <= 15.0 + tol,
            "{bounds:?}"
        );
        for p in &mesh.positions {
            assert!(p.x.hypot(p.y) <= 10.0 + tol, "outside the source semicircle: {p:?}");
        }
    }
}

/// Integrate exact inward-offset D slices, including the arc/line corner overlaps.
/// At height H-r+r*sin(theta), the inset is d=r*(1-cos(theta)); the remaining
/// slice is the circular segment of radius R-d above y=d. No kernel geometry is used.
fn d_fillet_removed_reference(r: f64, steps: u32) -> f64 {
    let dt = (PI / 2.0) / f64::from(steps);
    (0..steps)
        .map(|i| {
            let theta = (f64::from(i) + 0.5) * dt;
            let d = r * (1.0 - theta.cos());
            let rho = 10.0 - d;
            let remaining = rho * rho * (d / rho).acos() - d * (rho * rho - d * d).sqrt();
            (PI * 50.0 - remaining) * r * theta.cos() * dt
        })
        .sum()
}

#[test]
fn fillet_mixed_arc_and_line_removed_volume_converges() {
    let body = mixed_arc_and_line_prism();
    let source_exact = PI * 100.0 * 15.0 / 2.0;
    let tolerances = [1e-4, 2.5e-5];
    let source_volumes = tolerances.map(|tol| body.tessellate_with(tol, true).unwrap().measure().volume);
    for r in [0.5, 0.25] {
        let want = d_fillet_removed_reference(r, 8192);
        assert!((want - d_fillet_removed_reference(r, 4096)).abs() < 1e-7, "reference quadrature must converge");
        if r == 0.5 {
            assert!((want - 2.715578447526581).abs() < 1e-7, "{want}");
        }
        let blended = fillet(&body, &[Vec3::new(0.0, 10.0, 15.0), Vec3::new(5.0, 0.0, 15.0)], r).unwrap();
        let mut previous = None;
        for (tol, source) in tolerances.into_iter().zip(source_volumes) {
            let volume = blended.tessellate_with(tol, true).unwrap().measure().volume;
            // Equal chord tolerances reduce (not necessarily cancel) the error from the
            // unchanged walls. The convergence and exact-source checks below guard this.
            let removed = source - volume;
            assert!(rel(removed, want) < 1e-3, "r={r}, tol={tol}: removed {removed}, expected {want}");
            if let Some(coarser) = previous {
                assert!(rel(removed, coarser) < 1e-3, "r={r}: removal has not converged: {coarser} -> {removed}");
                let exact_removed = source_exact - volume;
                // The source's inscribed semicylinder loses at most pi*R*H*tol.
                // Do not charge its chord error to the much smaller fillet volume.
                let source_error = (source_exact - source).abs();
                assert!(source_error < PI * 10.0 * 15.0 * tol, "source chord error {source_error}");
                assert!((exact_removed - want).abs() < source_error + want * 1e-3, "r={r}: exact-source removal {exact_removed}, expected {want}");
            }
            previous = Some(removed);
        }
    }
}

#[test]
fn loop_chamfer_mixed_arc_and_line_uses_offset_intersections() {
    let body = mixed_arc_and_line_prism();
    // Exercise the shared loop builder; public chamfer prefers boolean cutters first.
    let solid = body.deep_copy();
    let top = solid
        .face_iter()
        .find(|face| face.edge_iter().all(|edge| (edge.front().point().z - 15.0).abs() < 1e-9 && (edge.back().point().z - 15.0).abs() < 1e-9))
        .unwrap();
    let edges: Vec<_> = top.edge_iter().map(|edge| edge.id()).collect();
    let (result, history) = crate::loopblend::loop_blend(&solid, &edges, body.size(), 0.5, false).unwrap().unwrap();
    let blended = crate::record::rebuilt(result, &[&solid], &history).unwrap();
    assert!(blended.validity().is_empty(), "{:?}", blended.validity());
    // A chamfer has a linear inset through its height instead of a circular section.
    let step = 0.5 / 8192.0;
    let want: f64 = (0..8192)
        .map(|i| {
            let d = (f64::from(i) + 0.5) * step;
            let rho = 10.0 - d;
            let remaining = rho * rho * (d / rho).acos() - d * (rho * rho - d * d).sqrt();
            (PI * 50.0 - remaining) * step
        })
        .sum();
    // The cone's triangulation needs this chord tolerance for the small volume difference.
    let fine = |b: &Body| b.tessellate_with(2.5e-5, true).unwrap().measure().volume;
    let removed = fine(&body) - fine(&blended);
    assert!(rel(removed, want) < 1e-3, "{removed} vs {want}");
}

#[test]
fn fillet_mixed_arc_and_line_joint_preserves_both_section_radii() {
    use truck_modeling::{BoundedCurve, ParametricCurve};

    let r = 0.5;
    let body = mixed_arc_and_line_prism();
    let blended = fillet(&body, &[Vec3::new(0.0, 10.0, 15.0), Vec3::new(5.0, 0.0, 15.0)], r).unwrap();
    crate::guard("inspect mixed fillet mitres", || {
        let solid = blended.deep_copy();
        let mut mitres = std::collections::HashSet::new();
        for edge in solid.boundaries().iter().flat_map(|shell| shell.edge_iter()) {
            let a = crate::body::from_p3(edge.front().point());
            let b = crate::body::from_p3(edge.back().point());
            // Only the two corner profiles join the top contact to the wall contact.
            if (a.z - b.z).abs() < r - 1e-8 || a.z.min(b.z) < 15.0 - r - 1e-8 || a.x * b.x <= 0.0 {
                continue;
            }
            if !mitres.insert(edge.id()) {
                continue;
            }
            let curve = edge.oriented_curve();
            let (lo, hi) = curve.range_tuple();
            for i in 0..=257 {
                let p = crate::body::from_p3(curve.subs(lo + (hi - lo) * f64::from(i) / 257.0));
                // The joint lies on both the cylindrical line blend and the toroidal
                // arc blend, not their tangent-plane approximation.
                let radius2 = (p.y - r).powi(2) + (p.z - (15.0 - r)).powi(2);
                assert!((radius2 - r * r).abs() < 1e-7, "distorted line section at {p:?}: radius squared {radius2}");
                let radius2 = (p.x.hypot(p.y) - (10.0 - r)).powi(2) + (p.z - (15.0 - r)).powi(2);
                assert!((radius2 - r * r).abs() < 1e-7, "distorted arc section at {p:?}: radius squared {radius2}");
            }
        }
        assert_eq!(mitres.len(), 2, "both arc/line corners must be checked");
        Ok(())
    })
    .unwrap();
}

#[test]
fn fillet_partial_mixed_arc_and_line_perimeter_run() {
    let body = partial_mixed_run_prism();
    let blended = fillet(&body, &[Vec3::new(35.0, 20.0, 15.0), Vec3::new(20.0, 30.0, 15.0), Vec3::new(5.0, 20.0, 15.0)], 0.5).unwrap();
    assert!(blended.validity().is_empty(), "{:?}", blended.validity());
    let mesh = blended.tessellate_with(1e-4, true).unwrap();
    let bounds = mesh.bounds();
    let tol = 1e-3;
    assert!(
        bounds.min.x >= -tol
            && bounds.max.x <= 40.0 + tol
            && bounds.min.y >= -tol
            && bounds.max.y <= 30.0 + tol
            && bounds.min.z >= -tol
            && bounds.max.z <= 15.0 + tol,
        "{bounds:?}"
    );
    for p in &mesh.positions {
        if p.y > 20.0 + tol {
            assert!((p.x - 20.0).hypot(p.y - 20.0) <= 10.0 + tol, "outside the source semicircle: {p:?}");
        }
    }
}

/// Boolean survey: random boxes, cylinders and spheres combined every way; prints what fails.
#[test]
#[ignore]
fn boolean_survey() {
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % 1_000_000) as f64 / 1_000_000.0
    };
    let mut shapes: Vec<(String, Body)> = Vec::new();
    for i in 0..40 {
        let kind = i % 4;
        let (x, y, z) = ((rnd() - 0.5) * 20.0, (rnd() - 0.5) * 20.0, (rnd() - 0.5) * 20.0);
        // Snap some coordinates to a 5 mm grid: coincident faces and tangencies.
        let snap = |v: f64, s: bool| if s { (v / 5.0).round() * 5.0 } else { v };
        let s = rnd() < 0.5;
        let (x, y, z) = (snap(x, s), snap(y, s), snap(z, s));
        let a = 5.0 + snap(rnd() * 20.0, s);
        let ax = (rnd() * 3.0) as usize % 3;
        let b = match kind {
            0 => box_solid(Vec3::new(x, y, z), Vec3::new(x + a, y + a * 0.8, z + a * 0.6)),
            1 => cylinder(Vec3::new(x, y, z), [Vec3::X, Vec3::Y, Vec3::Z][ax], a * 0.4, a),
            2 => sphere(Vec3::new(x, y, z), a * 0.5),
            _ => box_solid(Vec3::new(x, y, z), Vec3::new(x + a * 0.5, y + a, z + a * 0.3)),
        };
        if let Ok(b) = b {
            shapes.push((format!("{}{i}@({x:.3},{y:.3},{z:.3}) a={a:.3} ax={ax}", ["box", "cyl", "sph", "slab"][kind]), b));
        }
    }
    let (mut ok, mut empty, mut fail) = (0, 0, Vec::new());
    for i in 0..shapes.len() {
        for j in i + 1..(i + 4).min(shapes.len()) {
            for op in [BoolOp::Union, BoolOp::Cut, BoolOp::Intersect] {
                let (na, a) = &shapes[i];
                let (nb, b) = &shapes[j];
                match boolean(a, b, op) {
                    Ok(Some(_)) => ok += 1,
                    Ok(None) => empty += 1,
                    Err(e) => fail.push(format!("{na} {op:?} {nb}: {}", e.to_string().chars().take(140).collect::<String>())),
                }
            }
        }
    }
    println!("ok {ok} empty {empty} failed {}", fail.len());
    for f in &fail {
        println!("  {f}");
    }
}

#[test]
fn booleans_found_by_the_survey() {
    // A tool wholly inside: a void (the solid keeps an inner shell).
    let sp = sphere(Vec3::new(-2.894, 7.008, 8.081), 8.869).unwrap();
    let slab = box_solid(Vec3::new(-5.0, 0.0, 5.0), Vec3::new(-2.5, 5.0, 6.5)).unwrap();
    let c = boolean(&sp, &slab, BoolOp::Cut).unwrap().unwrap();
    let want = 4.0 / 3.0 * PI * 8.869f64.powi(3) - 18.75;
    assert!(rel(measure(&c).unwrap().volume, want) < 1e-3, "{}", measure(&c).unwrap().volume);
    // A sphere clipping a box's corner: the result comes out outward-facing.
    let sp = sphere(Vec3::new(-8.226, 9.539, -1.251), 11.8435).unwrap();
    let bx = box_solid(Vec3::new(0.0, -5.0, -5.0), Vec3::new(15.0, 7.0, 4.0)).unwrap();
    let c = boolean(&bx, &sp, BoolOp::Cut).unwrap().unwrap();
    let v = measure(&c).unwrap().volume;
    assert!(v > 1400.0 && v < 1620.0, "{v}");
    // A cylinder and a box sharing an end plane, the box's side near-tangent to the cylinder.
    let cy = cylinder(Vec3::new(-5.0, 0.0, 0.0), Vec3::X, 8.0, 20.0).unwrap();
    let bx = box_solid(Vec3::new(0.0, -5.0, -5.0), Vec3::new(15.0, 7.0, 4.0)).unwrap();
    let u = boolean(&cy, &bx, BoolOp::Union).unwrap().unwrap();
    let vu = measure(&u).unwrap().volume;
    assert!(vu > PI * 64.0 * 20.0 && vu < PI * 64.0 * 20.0 + 1620.0, "{vu}");
}

#[test]
fn fillet_two_edges_meeting_at_a_corner() {
    // Two top edges of a box meeting at a corner (the vertical edge there stays sharp): the
    // fillets meet in a mitre and stop square at the far walls.
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
    let f = fillet(&b, &[Vec3::new(20.0, 0.0, 20.0), Vec3::new(0.0, 15.0, 20.0)], 3.0).unwrap();
    let removed = 24000.0 - measure(&f).unwrap().volume;
    // Each edge loses r²(1 − π/4) along its length; at the mitred corner the two cuts overlap
    // in a region of volume r³(1 − π/4)·… — bounded between the sum less one corner and the sum.
    let a = 9.0 * (1.0 - PI / 4.0);
    let (lo, hi) = (a * (40.0 + 30.0) - a * 3.0, a * (40.0 + 30.0));
    assert!(removed > lo && removed < hi, "removed {removed} in ({lo}, {hi})");
    // Three edges: front, left and back.
    let f3 = fillet(&b, &[Vec3::new(20.0, 0.0, 20.0), Vec3::new(0.0, 15.0, 20.0), Vec3::new(20.0, 30.0, 20.0)], 3.0).unwrap();
    assert!(measure(&f3).unwrap().volume < measure(&f).unwrap().volume);
}

#[test]
fn fillet_three_edges_at_a_corner() {
    // The three edges meeting at one corner of a box: the corner becomes a sphere octant.
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)).unwrap();
    let r = 3.0;
    let f = fillet(&b, &[Vec3::new(20.0, 0.0, 20.0), Vec3::new(0.0, 15.0, 20.0), Vec3::new(0.0, 0.0, 10.0)], r).unwrap();
    let removed = 24000.0 - measure(&f).unwrap().volume;
    // Three edges lose r²(1 − π/4) each along their length; the corner, a cube r³ less an
    // octant of the ball, was counted three times over and is removed once (r³ − πr³/6).
    let a = r * r * (1.0 - PI / 4.0);
    let want = a * (40.0 + 30.0 + 20.0) - 3.0 * a * r + (r * r * r - PI * r * r * r / 6.0);
    assert!(rel(removed, want) < 2e-3, "removed {removed} vs {want}");
    let m = measure(&f).unwrap().merged;
    assert_eq!(m.face_types.get("sphere"), Some(&1), "{m:?}");
}

#[test]
fn shell_with_spherical_and_conical_faces() {
    // A cylinder capped by a hemisphere (a dome), revolved, opened at the bottom.
    use solidcraft_geom::Seg2;
    let xz = Plane::new(Vec3::ZERO, Vec3::X, Vec3::new(0.0, 0.0, -1.0)).unwrap();
    let segs = vec![
        Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) },
        Seg2::Line { a: Vec2::new(10.0, 0.0), b: Vec2::new(10.0, -20.0) },
        Seg2::Arc { center: Vec2::new(0.0, -20.0), radius: 10.0, start: 0.0, sweep: -PI / 2.0 },
        Seg2::Line { a: Vec2::new(0.0, -30.0), b: Vec2::new(0.0, 0.0) },
    ];
    let b = revolve(&xz, &[Region2 { outer: Loop2 { segs }, holes: vec![] }], Vec2::ZERO, Vec2::new(0.0, -1.0), 2.0 * PI).unwrap().pop().unwrap();
    let s = shell(&b, &[Vec3::new(0.0, 0.0, 0.0)], 1.0).unwrap();
    let outer = PI * 100.0 * 20.0 + 2.0 / 3.0 * PI * 1000.0;
    let inner = PI * 81.0 * 20.0 + 2.0 / 3.0 * PI * 729.0;
    let v = measure(&s).unwrap().volume;
    assert!(rel(v, outer - inner) < 2e-3, "{v} vs {}", outer - inner);
    // A cone frustum (a revolved trapezoid), opened at its wide base.
    let prof =
        Region2 { outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(5.0, -10.0), Vec2::new(0.0, -10.0)]), holes: vec![] };
    let fr = revolve(&xz, &[prof], Vec2::ZERO, Vec2::new(0.0, -1.0), 2.0 * PI).unwrap().pop().unwrap();
    let v0 = measure(&fr).unwrap().volume;
    let s = shell(&fr, &[Vec3::new(0.0, 0.0, 0.0)], 1.0).unwrap();
    let v = measure(&s).unwrap().volume;
    assert!(v > 0.1 * v0 && v < 0.6 * v0, "{v} of {v0}");
}

#[test]
fn smooth_loft_through_three_sections() {
    // Squares 40 → 20 → 40 wide, 0, 20 and 40 up: the middle is pinched smoothly (no edge there).
    let sq = |s: f64| Loop2::polygon(&[Vec2::new(-s, -s), Vec2::new(s, -s), Vec2::new(s, s), Vec2::new(-s, s)]);
    let l = loft(&[(Plane::XY, sq(20.0)), (Plane::XY.offset(20.0), sq(10.0)), (Plane::XY.offset(40.0), sq(20.0))]).unwrap();
    let m = measure(&l).unwrap();
    // Smooth: between the ruled (two frustums, 2 · 20 · (1600 + 400 + 800)/3) and the box.
    let ruled = 2.0 * 20.0 * (1600.0 + 400.0 + 800.0) / 3.0;
    assert!(m.volume < ruled && m.volume > 40.0 * 400.0, "{}", m.volume);
    // Four sides and two caps; the middle section leaves no edges.
    assert_eq!(m.merged.faces, 6, "{:?}", m.merged);
    assert_eq!(m.merged.edges, 12, "{:?}", m.merged);
    // A circle, a square and a circle: valid and between the inner and outer bounds.
    let c = loft(&[
        (Plane::XY, Loop2::circle(Vec2::ZERO, 10.0)),
        (Plane::XY.offset(15.0), sq(12.0)),
        (Plane::XY.offset(30.0), Loop2::circle(Vec2::ZERO, 8.0)),
    ])
    .unwrap();
    let v = measure(&c).unwrap().volume;
    assert!(v > PI * 64.0 * 30.0 && v < 576.0 * 30.0, "{v}");
}

/// A loft from the basin's whole outline (10 up) down to a drain pad in a corner (1 up), cut
/// from a 3 mm shell: its top edges lie on the outer walls. One order of the sections failed
/// the cut ("shell not oriented and closed", or a panic inside the intersection), the other
/// worked; both orders now build the same body, valid and outward.
#[test]
fn loft_section_order_gives_the_same_body() {
    let r = |x0: f64, y0: f64, x1: f64, y1: f64| Loop2::polygon(&[Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]);
    let at = |z: f64| Plane::new(Vec3::new(0.0, 0.0, z), Vec3::X, Vec3::Y).unwrap();
    let big = (at(10.0), r(0.0, 0.0, 100.0, 60.0));
    let small = (at(1.0), r(80.0, 40.0, 90.0, 50.0));
    let basin = shell(&box_solid(Vec3::ZERO, Vec3::new(100.0, 60.0, 30.0)).unwrap(), &[Vec3::new(50.0, 30.0, 30.0)], 3.0).unwrap();
    // The material inside the loft: the floor from 1 to 3 up, the walls from 3 to 10.
    let v0 = measure(&basin).unwrap().volume;
    let removed = v0 - 41582.028;
    let mut volumes = Vec::new();
    for secs in [vec![big.clone(), small.clone()], vec![small.clone(), big.clone()]] {
        let l = loft(&secs).unwrap();
        let m = measure(&l).unwrap();
        assert!(m.volume > 0.0, "{}", m.volume);
        volumes.push(m.volume);
        let cut = boolean(&basin, &l, BoolOp::Cut).unwrap().unwrap();
        let v = measure(&cut).unwrap().volume;
        assert!((v0 - v - removed).abs() < 0.01 * removed, "{v} vs {}", v0 - removed);
        let joined = boolean(&basin, &l, BoolOp::Union).unwrap().unwrap();
        assert!(measure(&joined).unwrap().volume > v0);
    }
    assert!(rel(volumes[0], volumes[1]) < 1e-12, "{volumes:?}");
    // Reversed in z too (large below, small above, and sketched on planes facing down).
    let down = |z: f64| Plane::new(Vec3::new(0.0, 0.0, z), Vec3::Y, Vec3::X).unwrap();
    for secs in [
        vec![(at(0.0), r(0.0, 0.0, 100.0, 60.0)), (at(20.0), r(40.0, 20.0, 60.0, 40.0))],
        vec![(down(20.0), r(20.0, 40.0, 40.0, 60.0)), (at(0.0), r(0.0, 0.0, 100.0, 60.0))],
    ] {
        // Prismoid: h/6 · (A0 + 4 Amid + A1).
        let want = 20.0 / 6.0 * (6000.0 + 4.0 * 60.0 * 40.0 + 400.0);
        let back: Vec<(Plane, Loop2)> = secs.iter().rev().cloned().collect();
        for b in [loft(&secs).unwrap(), loft(&back).unwrap()] {
            let v = measure(&b).unwrap().volume;
            assert!(rel(v, want) < 1e-6, "{v} vs {want}");
        }
    }
}

#[test]
fn tangent_holes_pinch_alike() {
    // Holes at ±15 and ±25 touch along a line (x = ±20); ±30 overlaps ±25. A tangent line
    // contact does not split a face (#307, as the reference kernel counts it): the ±25 wall is one
    // face on both sides, so 6 planes and 6 hole walls.
    let mut b = box_solid(Vec3::new(-50.0, -20.0, 0.0), Vec3::new(50.0, 20.0, 5.0)).unwrap();
    for x in [15.0, 25.0, 30.0, -15.0, -25.0, -30.0] {
        let plane = Plane::XY.offset(-1.0);
        let disc = Region2 { outer: Loop2::circle(Vec2::new(x, 0.0), 5.0), holes: vec![] };
        let t = extrude(&plane, &[disc], 0.0, 7.0).unwrap().pop().unwrap();
        b = boolean(&b, &t, BoolOp::Cut).unwrap().unwrap();
    }
    let m = measure(&b).unwrap();
    assert_eq!(m.merged.faces, 12, "{:?}", m.merged);
}

/// A d10 tube bent at R5: the inside of the bend is a horn torus pinched to a point. One torus
/// face, with a degenerate loop at the pinch (counted as Fusion does: 5 faces, 6 edges, 4 vertices).
#[test]
fn tight_bend_counts() {
    use crate::build::PathSeg;
    let region = Region2 { outer: Loop2::circle(Vec2::ZERO, 5.0), holes: vec![] };
    let path = [
        PathSeg::Line { a: Vec3::ZERO, b: Vec3::new(0.0, 0.0, 20.0) },
        PathSeg::Arc {
            a: Vec3::new(0.0, 0.0, 20.0),
            center: Vec3::new(5.0, 0.0, 20.0),
            axis: Vec3::new(0.0, 1.0, 0.0),
            angle: std::f64::consts::FRAC_PI_2,
        },
        PathSeg::Line { a: Vec3::new(5.0, 0.0, 25.0), b: Vec3::new(25.0, 0.0, 25.0) },
    ];
    let b = sweep(&Plane::XY, &region, &path).unwrap();
    let m = measure(&b).unwrap().merged;
    assert_eq!((m.faces, m.edges, m.vertices), (5, 6, 4), "{m:?}");
}

#[test]
#[ignore]
fn boolean_identity_survey() {
    let n: u64 = std::env::var("N").ok().and_then(|x| x.parse().ok()).unwrap_or(100);
    let seeds: Vec<u64> = match std::env::var("SEEDS") {
        Ok(s) => s.split(',').filter_map(|x| x.parse().ok()).collect(),
        Err(_) => (0..n).collect(),
    };
    let mut bad = 0;
    for seed in seeds {
        let c = crate::fuzz::boolean_case(seed);
        if c.outcome != "ok" {
            bad += 1;
            println!("{seed}: {} {} | {:?} | {:?}", c.outcome, c.detail, c.a, c.b);
        }
    }
    println!("{bad}/{n} failed");
}

/// A quarter turn of a rectangle with an edge on the axis: no zero-area face along the axis, so
/// a cylinder can be cut through the corner (it failed in every boolean attempt before).
#[test]
fn partial_revolve_on_the_axis() {
    let sq =
        Region2 { outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0), Vec2::new(20.0, 7.5), Vec2::new(0.0, 7.5)]), holes: vec![] };
    let plane = Plane { y: Vec3::Z, ..Plane::XZ }; // This fixture's rectangle is above z = 0.
    let pie = revolve(&plane, &[sq], Vec2::ZERO, Vec2::Y, std::f64::consts::FRAC_PI_2).unwrap().pop().unwrap();
    assert!(pie.validity().is_empty(), "{:?}", pie.validity());
    let v = measure(&pie).unwrap().volume;
    assert!((v - PI * 400.0 / 4.0 * 7.5).abs() < 1e-3 * v, "{v}");
    assert_eq!(measure(&pie).unwrap().merged.faces, 5);
    let cyl = cylinder(Vec3::new(2.5, 0.0, 5.0), Vec3::Z, 3.75, 22.5).unwrap();
    let cut = boolean(&pie, &cyl, BoolOp::Cut).unwrap().unwrap();
    assert!(cut.validity().is_empty());
    let vc = measure(&cut).unwrap().volume;
    assert!(vc < v - 1.0 && vc > v - PI * 3.75 * 3.75 * 2.5, "{vc}");
}

/// Boolean identities on primitives the robustness runs found failing: cylinders tangent to box
/// faces from inside and out, crossing cylinders touching along a line (their seams used to lie
/// exactly where the other body cut them).
#[test]
fn boolean_identity_on_tangent_primitives() {
    // Independent seeds, one thread each.
    let cases: Vec<_> = std::thread::scope(|scope| {
        let workers = [1, 32, 169].map(|seed| scope.spawn(move || (seed, crate::fuzz::boolean_case(seed))));
        workers.into_iter().map(|w| w.join().unwrap()).collect()
    });
    for (seed, c) in cases {
        assert_eq!(c.outcome, "ok", "seed {seed}: {} {:?} {:?}", c.detail, c.a, c.b);
    }
}

/// A body in two pieces: booleans go a piece at a time.
#[test]
fn boolean_of_a_body_in_pieces() {
    let a = box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
    let b = box_solid(Vec3::new(20.0, 0.0, 0.0), Vec3::new(30.0, 10.0, 10.0)).unwrap();
    let two = boolean(&a, &b, BoolOp::Union).unwrap().unwrap();
    assert_eq!(two.shell_count(), 2);
    let tool = cylinder(Vec3::new(-5.0, 5.0, 5.0), Vec3::X, 2.0, 40.0).unwrap();
    let cut = boolean(&two, &tool, BoolOp::Cut).unwrap().unwrap();
    assert_eq!(cut.shell_count(), 2);
    assert!(cut.validity().is_empty(), "{:?}", cut.validity());
    let v = measure(&cut).unwrap().volume;
    assert!((v - (2000.0 - 2.0 * PI * 4.0 * 10.0)).abs() < 1.0, "{v}");
}

/// Measuring a body of 5000 faces (a modelled thread is like that) counts right (it took 13 s,
/// a per-face recomputation of the body's size; now under a second).
#[test]
fn measure_of_a_body_with_many_faces() {
    let n = 5000;
    let pts: Vec<Vec2> = (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            Vec2::new(20.0 * a.cos(), 20.0 * a.sin())
        })
        .collect();
    let b = extrude(&Plane::XY, &[Region2 { outer: Loop2::polygon(&pts), holes: vec![] }], 0.0, 10.0).unwrap().pop().unwrap();
    let m = measure(&b).unwrap();
    assert_eq!((m.merged.faces, m.merged.edges, m.merged.vertices), (n + 2, 3 * n, 2 * n), "{:?}", m.merged);
}

/// The time it takes (run alone, the suite's other tests share the machine):
/// `cargo test --release -p solidcraft-kernel perf_measure_5000_faces -- --ignored`.
#[test]
#[ignore]
fn perf_measure_5000_faces() {
    let pts: Vec<Vec2> = (0..5000)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / 5000.0;
            Vec2::new(20.0 * a.cos(), 20.0 * a.sin())
        })
        .collect();
    let b = extrude(&Plane::XY, &[Region2 { outer: Loop2::polygon(&pts), holes: vec![] }], 0.0, 10.0).unwrap().pop().unwrap();
    let t = std::time::Instant::now();
    measure(&b).unwrap();
    assert!(t.elapsed().as_secs_f64() < 1.5, "{:?}", t.elapsed());
}

/// A lip on a rounded, shelled rim (#29): the band along the rim's inner (or outer) edge raised,
/// its walls continuing the walls below. Exactly the band's area times its height is added.
#[test]
fn raise_band_on_a_rounded_rim() {
    let b = box_solid(Vec3::new(-40.0, -30.0, 0.0), Vec3::new(40.0, 30.0, 30.0)).unwrap();
    let f =
        fillet(&b, &[Vec3::new(40.0, 30.0, 15.0), Vec3::new(-40.0, 30.0, 15.0), Vec3::new(-40.0, -30.0, 15.0), Vec3::new(40.0, -30.0, 15.0)], 6.0)
            .unwrap();
    let sh = shell(&f, &[Vec3::new(0.0, 0.0, 30.0)], 2.0).unwrap();
    let v0 = measure(&sh).unwrap().volume;
    // A rounded rectangle's area.
    let ar = |w: f64, h: f64, r: f64| w * h - (4.0 - PI) * r * r;
    for outside in [false, true] {
        let lip = crate::lip::raise_band(&sh, Vec3::new(0.0, 29.0, 30.0), 1.0, 1.5, outside).unwrap();
        assert!(lip.validity().is_empty(), "{:?}", lip.validity());
        let band = if outside { ar(80.0, 60.0, 6.0) - ar(78.0, 58.0, 5.0) } else { ar(78.0, 58.0, 5.0) - ar(76.0, 56.0, 4.0) };
        let v = measure(&lip).unwrap().volume;
        assert!((v - (v0 + band * 1.5)).abs() < 1e-5 * v, "outside {outside}: {v} vs {}", v0 + band * 1.5);
    }
}

#[test]
fn orientation_mesh_reuses_missing_face_check_at_matching_tolerance() {
    for scale in [0.001, 1.0, 1000.0] {
        let body = box_solid(Vec3::ZERO, Vec3::new(10., 20., 30.) * scale).unwrap();
        let tol = (body.size() * 0.01).max(1e-3);
        let mesh = body.tessellate(tol).unwrap();
        let mut present = vec![false; body.face_count()];
        for face in mesh.tri_face {
            present[face as usize] = true;
        }
        assert_eq!(body.unmeshed_faces(), present.iter().filter(|p| !**p).count());
        if scale == 1.0 {
            assert_eq!(body.coarse_missing, Some((tol, 0)));
        }
    }
}

/// A trough whose floor falls about 1° toward a central drain, the fall starting right at the
/// trough edges: four faces meet at each bottom corner (two walls, two fall facets) and at the
/// drain (four facets). A tab on one end makes the body non-convex. The moved planes of four
/// faces don't meet at one point, so each such corner of the cavity becomes two corners joined
/// by a short edge. Checked against the volume integrated by hand.
#[test]
fn shell_where_four_faces_meet_at_a_corner() {
    let (w, l, ht, t) = (100.0, 60.0, 40.0, 2.0);
    // Drain depth: the long facets fall 1°.
    let h = 30.0 * 1f64.to_radians().tan();
    let facet = |n: Vec3, p: Vec3| {
        let n = n.normalized().unwrap();
        HalfSpace { n, d: n.dot(p) }
    };
    let facets = [
        facet(Vec3::new(0.0, -h, -30.0), Vec3::ZERO),
        facet(Vec3::new(0.0, h, -30.0), Vec3::new(0.0, l, 0.0)),
        facet(Vec3::new(-h, 0.0, -50.0), Vec3::ZERO),
        facet(Vec3::new(h, 0.0, -50.0), Vec3::new(w, 0.0, 0.0)),
    ];
    let mut hs = vec![
        HalfSpace { n: -Vec3::X, d: 0.0 },
        HalfSpace { n: Vec3::X, d: w },
        HalfSpace { n: -Vec3::Y, d: 0.0 },
        HalfSpace { n: Vec3::Y, d: l },
        HalfSpace { n: Vec3::Z, d: ht },
    ];
    hs.extend(facets);
    let trough = convex_polyhedron(&hs).unwrap();
    let pyramid = w * l * h / 3.0;
    assert!(rel(measure(&trough).unwrap().volume, w * l * ht + pyramid) < 1e-9);
    let tab = box_solid(Vec3::new(w, 20.0, 20.0), Vec3::new(w + 20.0, 40.0, ht)).unwrap();
    let body = boolean(&trough, &tab, BoolOp::Union).unwrap().unwrap();
    let s = shell(&body, &[Vec3::new(50.0, 30.0, ht)], t).unwrap();
    // The cavity: the walls in by t, above the floor facets moved in by t (midpoint rule; the
    // floor is linear between its creases).
    let floor = |x: f64, y: f64| facets.iter().map(|f| (f.d - t - f.n.x * x - f.n.y * y) / f.n.z).fold(f64::NEG_INFINITY, f64::max);
    let (nx, ny) = (960, 560);
    let (dx, dy) = ((w - 2.0 * t) / nx as f64, (l - 2.0 * t) / ny as f64);
    let mut cavity = 0.0;
    for i in 0..nx {
        for j in 0..ny {
            cavity += (ht - floor(t + (i as f64 + 0.5) * dx, t + (j as f64 + 0.5) * dy)) * dx * dy;
        }
    }
    let tab_cavity = 20.0 * (20.0 - 2.0 * t) * (20.0 - t);
    let want = w * l * ht + pyramid + 8000.0 - cavity - tab_cavity;
    let m = measure(&s).unwrap();
    assert!(rel(m.volume, want) < 1e-5, "{} vs {want}", m.volume);
}

/// The same trough cut as a pocket into a slab (four faces meet at each concave corner of the
/// pocket floor), shelled with its top open.
#[test]
fn shell_of_a_pocket_whose_floor_falls_to_a_drain() {
    let (px, py, pw, pl, top, depth, t) = (20.0, 15.0, 100.0, 60.0, 50.0, 30.0, 3.0);
    let h = 30.0 * 1f64.to_radians().tan();
    let z0 = top - depth;
    let facet = |n: Vec3, p: Vec3| {
        let n = n.normalized().unwrap();
        HalfSpace { n, d: n.dot(p) }
    };
    let facets = [
        facet(Vec3::new(0.0, -h, -30.0), Vec3::new(px, py, z0)),
        facet(Vec3::new(0.0, h, -30.0), Vec3::new(px, py + pl, z0)),
        facet(Vec3::new(-h, 0.0, -50.0), Vec3::new(px, py, z0)),
        facet(Vec3::new(h, 0.0, -50.0), Vec3::new(px + pw, py, z0)),
    ];
    let mut hs = vec![
        HalfSpace { n: -Vec3::X, d: -px },
        HalfSpace { n: Vec3::X, d: px + pw },
        HalfSpace { n: -Vec3::Y, d: -py },
        HalfSpace { n: Vec3::Y, d: py + pl },
        HalfSpace { n: Vec3::Z, d: top + 10.0 },
    ];
    hs.extend(facets);
    let pocket = convex_polyhedron(&hs).unwrap();
    let slab = box_solid(Vec3::ZERO, Vec3::new(140.0, 90.0, top)).unwrap();
    let basin = boolean(&slab, &pocket, BoolOp::Cut).unwrap().unwrap();
    let pocket_vol = pw * pl * depth + pw * pl * h / 3.0;
    assert!(rel(measure(&basin).unwrap().volume, 140.0 * 90.0 * top - pocket_vol) < 1e-9);
    let s = shell(&basin, &[Vec3::new(5.0, 5.0, top)], t).unwrap();
    // The cavity's columns: from t up to the top, or up to the pocket floor moved down by t
    // inside the pocket's walls moved out by t.
    let floor = |x: f64, y: f64| facets.iter().map(|f| (f.d + t - f.n.x * x - f.n.y * y) / f.n.z).fold(f64::NEG_INFINITY, f64::max);
    let (nx, ny) = (1340, 840);
    let (dx, dy) = ((140.0 - 2.0 * t) / nx as f64, (90.0 - 2.0 * t) / ny as f64);
    let mut cavity = 0.0;
    for i in 0..nx {
        for j in 0..ny {
            let (x, y) = (t + (i as f64 + 0.5) * dx, t + (j as f64 + 0.5) * dy);
            let inside = x > px - t && x < px + pw + t && y > py - t && y < py + pl + t;
            let roof = if inside { floor(x, y) } else { top };
            cavity += (roof - t) * dx * dy;
        }
    }
    let want = 140.0 * 90.0 * top - pocket_vol - cavity;
    let m = measure(&s).unwrap();
    assert!(rel(m.volume, want) < 1e-4, "{} vs {want}", m.volume);
}

#[test]
fn shell_open_beside_a_round_as_large_as_the_wall_is_refused() {
    // Opening the bottom of a box whose bottom outline is rounded: the cavity's side walls stand
    // the thickness in from the outside, where a round at least that large meets the bottom, so
    // the rim would have no thickness. Smaller rounds leave a rim.
    let b = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
    let loop_ = [Vec3::new(20.0, 0.0, 0.0), Vec3::new(40.0, 15.0, 0.0), Vec3::new(20.0, 30.0, 0.0), Vec3::new(0.0, 15.0, 0.0)];
    let bottom = [Vec3::new(20.0, 15.0, 0.0)];
    for r in [2.0, 2.4] {
        let f = fillet(&b, &loop_, r).unwrap();
        let e = shell(&f, &bottom, 2.0).unwrap_err();
        assert!(e.to_string().starts_with("the operation failed: not supported yet: the rim"), "{r}: {e}");
    }
    let f = fillet(&b, &loop_, 1.5).unwrap();
    assert!(shell(&f, &bottom, 2.0).unwrap().validity().is_empty());
}

#[test]
fn loft_joined_onto_a_box_top_is_one_shell() {
    // #396: a loft from a Ø12 circle on the box top (z = 20) to a hexagon at z = 35. The union
    // returned both shells side by side, touching on the disc, which an STL shows as two parts.
    let bx = box_solid(Vec3::new(-20.0, -15.0, 0.0), Vec3::new(20.0, 15.0, 20.0)).unwrap();
    let hex: Vec<Vec2> = (0..6).map(|k| Vec2::new(4.0 * (PI / 3.0 * k as f64).cos(), 4.0 * (PI / 3.0 * k as f64).sin())).collect();
    let loft_at = |x: f64| {
        let hex: Vec<Vec2> = hex.iter().map(|p| *p + Vec2::new(x, 0.0)).collect();
        loft(&[(Plane::XY.offset(20.0), Loop2::circle(Vec2::new(x, 0.0), 6.0)), (Plane::XY.offset(35.0), Loop2::polygon(&hex))]).unwrap()
    };
    let l = loft_at(0.0);
    let u = boolean(&bx, &l, BoolOp::Union).unwrap().unwrap();
    assert_eq!((u.shell_count(), u.face_count()), (1, 6 + 8 - 1), "the loft's bottom disc goes");
    assert!(u.validity().is_empty(), "{:?}", u.validity());
    let (v, vl) = (measure(&u).unwrap().volume, measure(&l).unwrap().volume);
    assert!(rel(v, 24000.0 + vl) < 1e-5, "{v} vs {}", 24000.0 + vl);
    // Either order of the operands.
    let w = boolean(&l, &bx, BoolOp::Union).unwrap().unwrap();
    assert_eq!(w.shell_count(), 1);
    assert!(rel(measure(&w).unwrap().volume, 24000.0 + vl) < 1e-5);
    // Overhanging the box edge (x = 20) the disc is not inside the top face: whatever the join
    // does, it never returns touching shells as one body.
    if let Ok(Some(r)) = boolean(&bx, &loft_at(17.0), BoolOp::Union) {
        assert!(r.shell_count() == 1 || r.union_lumps().is_ok(), "{} touching shells", r.shell_count());
    }
}

/// A disc whose circular edge is rounded smaller than the wall (#319): moved in by the
/// thickness the round collapses to a sharp edge, where the floor meets the side along a
/// circle (two arcs with the same ends), not a straight line.
#[test]
fn shell_of_a_disc_rounded_smaller_than_the_wall() {
    use std::f64::consts::PI;
    let (rd, h, r) = (15.0, 5.0, 1.5);
    let disc = cylinder(Vec3::new(0.0, 0.0, -h), Vec3::Z, rd, h).unwrap();
    // The round takes a corner of area r²(1 − π/4) whose centroid is r(10 − 3π)/(3(4 − π))
    // in from the rim, swept round the axis (Pappus).
    let corner = r * r * (1.0 - PI / 4.0) * 2.0 * PI * (rd - r * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI)));
    let (top, bottom) = ([Vec3::ZERO], [Vec3::new(0.0, 0.0, -h)]);
    for (edge_z, open) in [(-h, &top), (-h, &bottom), (0.0, &bottom)] {
        let rounded = fillet(&disc, &[Vec3::new(0.0, rd, edge_z), Vec3::new(0.0, -rd, edge_z)], r).unwrap();
        let v0 = PI * rd * rd * h - corner;
        for t in [1.6, 2.0] {
            let s = shell(&rounded, open, t).unwrap_or_else(|e| panic!("round at z {edge_z}, t {t}: {e}"));
            assert!(s.validity().is_empty(), "round at z {edge_z}, t {t}: {:?}", s.validity());
            // The cavity: a plain cylinder the thickness in from the side and the closed end.
            let want = v0 - PI * (rd - t).powi(2) * (h - t);
            let v = measure(&s).unwrap().volume;
            assert!(rel(v, want) < 1e-3, "round at z {edge_z}, t {t}: {v} vs {want}");
        }
    }
}

/// The cache-budget estimate counts edge and vertex uses on the borrowed boundaries; it must
/// equal what walking the cloned wires (`Solid::edge_iter`, `vertex_iter`) counts.
#[test]
fn retained_bytes_matches_the_iterator_counts() {
    let block = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
    let hole = cylinder(Vec3::new(20.0, 15.0, -5.0), Vec3::Z, 5.0, 30.0).unwrap();
    let drilled = boolean(&block, &hole, BoolOp::Cut).unwrap().unwrap();
    for body in [&block, &hole, &drilled] {
        let s = &body.solid;
        let walked = s.face_iter().count() * 2048 + s.edge_iter().count() * 512 + s.vertex_iter().count() * 96;
        assert_eq!(body.retained_bytes(), walked);
    }
}

/// #881: a pleated band lofted to a nearly flat one, both 18 vertices from (-26, 3), counter-
/// clockwise and mirror-symmetric about x = 0. The zig-zag's centroid sits well above its first
/// vertex, so that vertex pointed nowhere near the lead's: the loft started three vertices round,
/// twisted (14 820 mm³, centre of mass at x = 5.3). Vertex to vertex, it is a prismoid.
#[test]
fn loft_pairs_the_vertices_of_matching_non_convex_sections() {
    let bottom = [
        (-26.0, 3.0),
        (-24.76384, 3.0),
        (-16.0568, 14.97217),
        (-8.0, 4.90117),
        (0.0, 14.90117),
        (8.0, 4.90117),
        (16.0568, 14.97217),
        (24.76384, 3.0),
        (26.0, 3.0),
        (26.0, 0.0),
        (23.23616, 0.0),
        (15.9432, 10.02783),
        (8.0, 0.09883),
        (0.0, 10.09883),
        (-8.0, 0.09883),
        (-15.9432, 10.02783),
        (-23.23616, 0.0),
        (-26.0, 0.0),
    ];
    let top = [
        (-26.0, 3.0),
        (-24.02812, 3.0),
        (-16.0, 3.30105),
        (-8.0, 3.00105),
        (0.0, 3.30105),
        (8.0, 3.00105),
        (16.0, 3.30105),
        (24.02812, 3.0),
        (26.0, 3.0),
        (26.0, 0.0),
        (23.97188, 0.0),
        (16.0, 0.29895),
        (8.0, -0.00105),
        (0.0, 0.29895),
        (-8.0, -0.00105),
        (-16.0, 0.29895),
        (-23.97188, 0.0),
        (-26.0, 0.0),
    ];
    let v = |p: &[(f64, f64)]| p.iter().map(|&(x, y)| Vec2::new(x, y)).collect::<Vec<_>>();
    let mid: Vec<Vec2> = v(&bottom).into_iter().zip(v(&top)).map(|(a, b)| (a + b) * 0.5).collect();
    // Prismoid: h/6 · (A0 + 4 Amid + A1) = 17 914.43.
    let area = |p: &[Vec2]| Loop2::polygon(p).signed_area().abs();
    let want = 90.0 / 6.0 * (area(&v(&bottom)) + 4.0 * area(&mid) + area(&v(&top)));
    let secs = [(Plane::XY, Loop2::polygon(&v(&bottom))), (Plane::XY.offset(90.0), Loop2::polygon(&v(&top)))];
    let m = measure(&loft(&secs).unwrap()).unwrap();
    // Twisted ruled faces measure a little under the prismoid (17 884; a square lofted to a
    // skewed quad, one twisted face, 0.3 % under), so the bounds leave room for that.
    assert!(rel(m.volume, want) < 5e-3, "{} vs {want}", m.volume);
    assert!(m.centroid.x.abs() < 0.05, "centre of mass x {}", m.centroid.x);
}
