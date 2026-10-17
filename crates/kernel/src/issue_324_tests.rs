//! #324: a round on a block's top edge that would overlap a through hole is refused, and the
//! largest radius the refusal offers fits.

use solidcraft_geom::Vec3;

use crate::{Body, BoolOp, build::box_solid, build::cylinder, ops::boolean};

/// A 40 x 20 x 10 block with a through hole of radius 3 at (20, `y`).
fn block_with_hole(y: f64) -> Body {
    let block = box_solid(Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 20.0, 10.0)).unwrap();
    let hole = cylinder(Vec3::new(20.0, y, -1.0), Vec3::Z, 3.0, 12.0).unwrap();
    boolean(&block, &hole, BoolOp::Cut).unwrap().unwrap()
}

/// The round on the top edge at y = 20 covers the top face from y = 20 - r, and the hole's
/// wall reaches y = 17.5: a radius over 2.5 runs into the hole. The refusal of 4 mm offered
/// 2.721 mm, which was refused the same way: the curved wall's mesh, with its tolerance, was
/// first seen inside the round's region 0.22 mm past where the wall really enters it.
#[test]
fn the_radius_offered_beside_a_hole_fits() {
    let b = block_with_hole(14.5);
    let edge = Vec3::new(10.0, 20.0, 10.0);
    let m = crate::fillet(&b, &[edge], 4.0).unwrap_err().to_string();
    let offered: f64 = m
        .split("the largest radius that fits is ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no radius offered: {m}"));
    assert!(offered > 2.0 && offered <= 2.5, "{m}");
    let r = crate::fillet(&b, &[edge], offered).unwrap();
    assert!(r.validity().is_empty(), "{:?}", r.validity());
    // The block, the hole and the round's corner: the round stays clear of the hole.
    let want = 8000.0 - std::f64::consts::PI * 9.0 * 10.0 - 40.0 * offered * offered * (1.0 - std::f64::consts::FRAC_PI_4);
    let v = r.volume_at(0.005).unwrap();
    assert!((v - want).abs() < 0.05, "{v} against {want}");
}
