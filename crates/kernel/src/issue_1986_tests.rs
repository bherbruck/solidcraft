//! A chamfer along a joined cylindrical wall, refused because the wall was sewn one face per
//! slab (#1986).
//!
//! The prism boolean slices both operands at every level either of them ends at and sews a
//! wall per slab. A straight wall split that way is put back together by the coplanar healing
//! afterwards, so it is one face again; a cylindrical one was not, and one cylinder came out
//! as a stack of co-cylindrical faces. A blend along the top of such a wall then reads the
//! face beside it as one slab tall and refuses, although the wall is whole and many times
//! that: here two sectors are joined, the inner one stopping 2 mm below the top, and the 3 mm
//! chamfer on the top outer arc edge was refused while 1 mm was taken.

use std::f64::consts::PI;

use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2, Vec3};

use crate::*;

/// The sectors' radii: the outer one stands between [`R_MID`] and [`R_OUT`], the inner one
/// between [`R_IN`] and [`R_MID`].
const R_IN: f64 = 3.0;
const R_MID: f64 = 6.0;
const R_OUT: f64 = 10.0;
/// The level the inner sector ends at, and the top both the outer one and the join reach.
const JOIN: f64 = 18.0;
const TOP: f64 = 20.0;
/// Half of either sector's angle: both run from -60 to +60 degrees.
const HALF: f64 = PI / 3.0;
/// The chamfer asked for: larger than the `TOP - JOIN` slab, well inside the faces beside it
/// (4 mm of top face, a wall `TOP` tall).
const SIZE: f64 = 3.0;

/// A counter-clockwise annular sector between `r0` and `r1`, from -[`HALF`] to +[`HALF`].
fn sector(r0: f64, r1: f64) -> Region2 {
    let at = |r: f64, a: f64| Vec2::new(r * a.cos(), r * a.sin());
    Region2 {
        outer: Loop2 {
            segs: vec![
                Seg2::Line { a: at(r0, -HALF), b: at(r1, -HALF) },
                Seg2::Arc { center: Vec2::ZERO, radius: r1, start: -HALF, sweep: 2.0 * HALF },
                Seg2::Line { a: at(r1, HALF), b: at(r0, HALF) },
                Seg2::Arc { center: Vec2::ZERO, radius: r0, start: HALF, sweep: -2.0 * HALF },
            ],
        },
        holes: Vec::new(),
    }
}

fn prism(r0: f64, r1: f64, top: f64) -> Body {
    extrude(&Plane::XY, &[sector(r0, r1)], 0.0, top).expect("the sector extrudes").pop().expect("one body")
}

/// The outer sector with the inner one joined to it: the join's levels are 0, [`JOIN`] and
/// [`TOP`], so the outer wall is the same arc in two successive slabs.
fn joined() -> Body {
    boolean(&prism(R_MID, R_OUT, TOP), &prism(R_IN, R_MID, JOIN), BoolOp::Union).expect("the join").expect("a result")
}

/// How many faces of `body` lie on the cylinder of radius `r`.
fn walls_of_radius(body: &Body, r: f64) -> usize {
    (0..body.face_count())
        .filter(|i| matches!(face_surface(body, *i), Some(FaceSurface::Cylinder { radius, .. }) if (radius - r).abs() < 1e-6))
        .count()
}

/// The volume of an annular sector of [`HALF`] either side, between `r0` and `r1`, `h` tall.
fn wall_volume(r0: f64, r1: f64, h: f64) -> f64 {
    HALF * (r1 * r1 - r0 * r0) * h
}

#[test]
fn a_joined_cylindrical_wall_is_one_face() {
    let body = joined();
    assert_eq!(walls_of_radius(&body, R_OUT), 1, "one cylinder is one face, not one per slab");
    assert_eq!(walls_of_radius(&body, R_MID), 1, "the inner sector's wall above the join is one face");
    assert!(body.validity().is_empty(), "{:?}", body.validity());
    // The join itself is unchanged by how its walls are sewn.
    let want = wall_volume(R_MID, R_OUT, TOP) + wall_volume(R_IN, R_MID, JOIN);
    let got = measure(&body).expect("the join measures").volume;
    assert!((got - want).abs() < 2e-3 * want, "the join holds {got} mm³, want {want} mm³");
}

#[test]
fn the_top_arc_edge_takes_a_chamfer_larger_than_one_slab() {
    let body = joined();
    let before = measure(&body).expect("the join measures").volume;
    let chamfered = chamfer(&body, &[Vec3::new(R_OUT, 0.0, TOP)], SIZE).expect("the chamfer");
    assert!(chamfered.validity().is_empty(), "{:?}", chamfered.validity());
    // A chamfer along one arc is one cone, and the wall it ran down is still one face.
    let cones = (0..chamfered.face_count()).filter(|i| matches!(face_surface(&chamfered, *i), Some(FaceSurface::Cone { .. }))).count();
    assert_eq!(cones, 1, "a chamfer along one arc edge is one conical face");
    assert_eq!(walls_of_radius(&chamfered, R_OUT), 1, "the chamfered wall stays one face");
    // The wedge taken: a triangle of legs `SIZE` swept round, so ∫(r - (R_OUT - SIZE))·r dr
    // over the radii it covers, times the angle.
    let wedge = 2.0 * HALF * (SIZE * SIZE * SIZE / 3.0 + (R_OUT - SIZE) * SIZE * SIZE / 2.0);
    let took = before - measure(&chamfered).expect("the chamfer measures").volume;
    assert!((took - wedge).abs() < 2e-3 * wedge, "the chamfer took {took} mm³, want {wedge} mm³");
}
