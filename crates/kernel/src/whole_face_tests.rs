use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2, Vec3};

use super::*;
use crate::{BoolOp, boolean, box_solid, measure};

fn square(a: Vec2, b: Vec2) -> Region2 {
    let p = Vec2::new;
    let line = |a: Vec2, b: Vec2| Seg2::Line { a, b };
    Region2 { outer: Loop2 { segs: vec![line(a, p(b.x, a.y)), line(p(b.x, a.y), b), line(b, p(a.x, b.y)), line(p(a.x, b.y), a)] }, holes: vec![] }
}

#[test]
fn a_profile_matches_the_face_it_outlines_and_no_other() {
    let block = box_solid(Vec3::ZERO, Vec3::new(20.0, 10.0, 5.0)).unwrap();
    let top = Plane { origin: Vec3::new(0.0, 0.0, 5.0), ..Plane::XY };
    let whole = matching_face(&block, &top, &square(Vec2::new(0.0, 0.0), Vec2::new(20.0, 10.0)), &[0.0]).unwrap();
    assert!((whole.normal - Vec3::Z).len() < 1e-9 && whole.height.abs() < 1e-9, "{whole:?}");
    // A smaller rectangle on the same face is not the face.
    assert!(matching_face(&block, &top, &square(Vec2::new(1.0, 1.0), Vec2::new(19.0, 9.0)), &[0.0]).is_none());
    // Nor is the bottom face, though its outline is the same.
    assert!(matching_face(&block, &top, &square(Vec2::ZERO, Vec2::new(20.0, 10.0)), &[-3.0]).is_none());
}

#[test]
fn a_cut_over_the_whole_top_takes_only_the_face_and_a_step_beside_it_does_not() {
    let block = box_solid(Vec3::ZERO, Vec3::new(20.0, 10.0, 5.0)).unwrap();
    let top =
        matching_face(&block, &Plane { origin: Vec3::new(0.0, 0.0, 5.0), ..Plane::XY }, &square(Vec2::ZERO, Vec2::new(20.0, 10.0)), &[0.0]).unwrap();
    assert!(cut_takes_only_the_face(&block, top, 2.0));
    // A step standing beside the top face: a slab would take it too.
    let step = box_solid(Vec3::new(20.0, 0.0, 0.0), Vec3::new(30.0, 10.0, 8.0)).unwrap();
    let stepped = boolean(&block, &step, BoolOp::Union).unwrap().unwrap();
    let face = matching_face(&stepped, &Plane { origin: Vec3::new(0.0, 0.0, 5.0), ..Plane::XY }, &square(Vec2::ZERO, Vec2::new(20.0, 10.0)), &[0.0])
        .unwrap();
    assert!(!cut_takes_only_the_face(&stepped, face, 2.0));
}

#[test]
fn a_prism_from_the_face_joins_by_its_shared_edges() {
    let block = box_solid(Vec3::ZERO, Vec3::new(20.0, 10.0, 5.0)).unwrap();
    let plane = Plane { origin: Vec3::new(0.0, 0.0, 5.0), ..Plane::XY };
    let region = square(Vec2::ZERO, Vec2::new(20.0, 10.0));
    let face = matching_face(&block, &plane, &region, &[0.0]).unwrap();
    let built = crate::extrude(&plane, std::slice::from_ref(&region), 0.0, 3.0).unwrap().remove(0);
    let tool = face_prism(&block, face, 3.0, true, &built).unwrap();
    assert!(tool.history().is_some_and(|h| h.operands == 1));
    let joined = boolean(&block, &tool, BoolOp::Union).unwrap().unwrap();
    assert!((measure(&joined).unwrap().volume - 20.0 * 10.0 * 8.0).abs() < 1e-6);
}

/// The block of #2040: one side at 45°, its top (and, with `bottom`, its bottom) rounded 3.8 mm
/// at both ends.
fn rounded_block(bottom: bool) -> Body {
    let p = Vec2::new;
    let line = |a: Vec2, b: Vec2| Seg2::Line { a, b };
    let h = 8.485281374238571;
    let segs = vec![
        line(p(0.0, 0.0), p(11.514718625761429, 0.0)),
        line(p(11.514718625761429, 0.0), p(20.0, h)),
        line(p(20.0, h), p(0.0, h)),
        line(p(0.0, h), p(0.0, 0.0)),
    ];
    let plane = Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Z };
    let block = crate::extrude(&plane, &[Region2 { outer: Loop2 { segs }, holes: vec![] }], 0.0, 10.0).unwrap().remove(0);
    let mut edges = vec![Vec3::new(10.0, -10.0, h), Vec3::new(10.0, 0.0, h)];
    if bottom {
        edges.extend([Vec3::new(5.757359312880714, 0.0, 0.0), Vec3::new(5.757359312880714, -10.0, 0.0)]);
    }
    crate::fillet(&block, &edges, 3.8).unwrap()
}

fn slanted_face(b: &Body) -> WholeFace {
    let n = Vec3::new(1.0, 0.0, -1.0).normalized().unwrap();
    let face = b.solid.face_iter().position(|f| crate::blend::plane_normal(f).is_some_and(|m| m.dot(n) > 1.0 - 1e-9)).unwrap();
    WholeFace { face, normal: n, height: 0.0 }
}

/// Into the slanted face, the walls from its top edge and the rounds beside it leave the body
/// (n_N · −n_P = +0.707 down to 0), those from the end faces lie in them (0), and those from the
/// bottom edge and its rounds go into it (−0.707 up to 0): the bottom ones are kept, the others
/// moved off.
#[test]
fn the_walls_into_a_slanted_face_touch_at_the_top_and_cut_at_the_bottom() {
    let b = rounded_block(true);
    let face = slanted_face(&b);
    let mesh = b.tessellate(outline_tol(&b) * 0.2).unwrap();
    let walls = walls(&mesh, face.face, face.normal, 1e-6).unwrap();
    assert_eq!(walls.iter().filter(|(_, w)| *w == Wall::Cuts).count(), 3, "{walls:?}");
    assert_eq!(walls.iter().filter(|(_, w)| *w == Wall::Touches).count(), 5, "{walls:?}");
    assert!(!cut_takes_only_the_face(&b, face, 3.0));
    assert!(cut_tool(&b, face, 3.0, None).is_some());
}

/// A cut 3 mm into the slanted face of the block rounded along its top: the walls from the top
/// are moved off it, the one from the bottom edge kept. What it takes: in the face's plane, the
/// body's section at depth t is the block's section (10 wide, h high, its top corners rounded)
/// moved down the face by t, so the cut takes √2 ∫₀³ (10 (h − t/√2) − 2 (1 − π/4) r²) dt.
#[test]
fn a_cut_into_a_slanted_face_beside_rounds_takes_what_lies_over_it() {
    let b = rounded_block(false);
    let face = slanted_face(&b);
    let (tool, _) = cut_tool(&b, face, 3.0, None).unwrap();
    let cut = boolean(&b, &tool, BoolOp::Cut).unwrap().unwrap();
    assert!(cut.validity().is_empty(), "{:?}", cut.validity());
    let (h, r, d, w) = (8.485281374238571_f64, 3.8_f64, 3.0_f64, 10.0_f64);
    let corner = (1.0 - std::f64::consts::FRAC_PI_4) * r * r;
    let taken = 2f64.sqrt() * w * h * d - w * d * d / 2.0 - 2.0 * 2f64.sqrt() * corner * d;
    let v = measure(&b).unwrap().volume - measure(&cut).unwrap().volume;
    assert!((v - taken).abs() < 0.05, "took {v}, expected {taken}");
}
