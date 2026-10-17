//! #1510: a body split by a plane that a face of the body lies in: the parts meet in the plane
//! and together are the body.

use solidcraft_geom::{Plane, Vec3};

use crate::{Body, BoolOp, build::box_solid, build::cylinder, ops::boolean, ops::split_by_plane};

fn cut(a: &Body, t: crate::Result<Body>) -> Body {
    boolean(a, &t.unwrap(), BoolOp::Cut).unwrap().unwrap()
}

/// A 2.5 mm plate with a through hole of radius 2.5 and a groove 4 mm wide and 1.5 deep from
/// the bottom, running from the hole's centre to the edge: the groove's ceiling lies at z = 1.5
/// and its boundary runs into the hole.
fn plate(width: f64) -> Body {
    let p = box_solid(Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 20.0, 2.5)).unwrap();
    let p = cut(&p, cylinder(Vec3::new(20.0, 10.0, -1.0), Vec3::Z, 2.5, 5.0));
    cut(&p, box_solid(Vec3::new(20.0, 10.0 - width / 2.0, -1.0), Vec3::new(41.0, 10.0 + width / 2.0, 1.5)))
}

#[test]
fn a_split_in_the_plane_of_a_face_keeps_all_of_the_body() {
    for width in [4.0, 5.0] {
        let b = plate(width);
        let v = b.volume_at(1e-3).unwrap();
        let plane = Plane::from_normal(Vec3::new(0.0, 0.0, 1.5), Vec3::Z).unwrap();
        let (parts, d) = crate::capture_boolean_diagnostics(|| split_by_plane(&b, &plane));
        let parts = parts.unwrap();
        assert_eq!(parts.len(), 2, "{width}");
        let vs: Vec<f64> = parts.iter().map(|p| p.volume_at(1e-3).unwrap()).collect();
        assert!((vs.iter().sum::<f64>() - v).abs() < 1e-6 * v, "{width}: {vs:?} against {v}; {:?}", d.summary());
        // Above: the plate from 1.5 to 2.5 less the hole; below: less the hole and the groove.
        let (r, h) = (2.5f64, width / 2.0);
        let hole = std::f64::consts::PI * r * r;
        // The strip 20 <= x <= 40, |y - 10| <= h, less the part of it in the hole.
        let groove = width * 20.0 - (h * (r * r - h * h).sqrt() + r * r * (h / r).asin());
        let want = [800.0 - hole, 1.5 * (800.0 - hole - groove)];
        for (got, want) in vs.iter().zip(want) {
            assert!((got - want).abs() < 0.05, "{width}: {vs:?} against {want}; {:?}", d.summary());
        }
        // Both parts end in the plane, not off it.
        let z: Vec<(f64, f64)> = parts.iter().map(|p| crate::measure(p).map(|m| (m.bbox.min.z, m.bbox.max.z)).unwrap()).collect();
        assert!((z[0].0 - 1.5).abs() < 1e-6 && (z[1].1 - 1.5).abs() < 1e-6, "{width}: {z:?}; {:?}", d.summary());
    }
}

/// A slab whose bottom rises 0.03 mm over its 700 mm length, its plane through the origin: its
/// normal is within 1e-9 of the split plane's, but it doesn't lie in the plane, so it is not
/// moved, and the part on the upper side is the whole slab.
#[test]
fn a_face_only_nearly_in_the_plane_is_not_moved() {
    use truck_modeling as mt;
    let v = |x: f64, y: f64, z: f64| mt::builder::vertex(crate::body::p3(Vec3::new(x, y, z)));
    let p = [v(0.0, 0.0, 0.0), v(0.0, 100.0, 0.0), v(700.0, 100.0, 0.03), v(700.0, 0.0, 0.03)];
    let wire: mt::Wire = (0..4).map(|i| mt::builder::line(&p[i], &p[(i + 1) % 4])).collect();
    let surface = mt::Plane::new(crate::body::p3(Vec3::ZERO), crate::body::p3(Vec3::Y), crate::body::p3(Vec3::new(700.0, 0.0, 0.03)));
    let bottom = mt::Face::new(vec![wire], mt::Surface::Plane(surface));
    let b = Body::new(mt::builder::tsweep(&bottom, mt::Vector3::new(0.0, 0.0, 10.0))).unwrap();
    let plane = Plane::from_normal(Vec3::ZERO, Vec3::Z).unwrap();
    assert!(crate::ops::off_plane(&b, &plane, true).is_none());
    let v0 = b.volume_at(1e-2).unwrap();
    let up = crate::ops::half_space(&b, &plane, true).unwrap().0.unwrap();
    let v1 = up.volume_at(1e-2).unwrap();
    assert!((v1 - v0).abs() < 1e-3 * v0, "{v1} against {v0}");
}

/// A box whose top face is 3e-5 above the plane, within the boolean's tolerance but off it: the
/// face bounds a real sliver, and it is not moved.
#[test]
fn a_face_close_to_the_plane_but_off_it_is_not_moved() {
    let b = box_solid(Vec3::new(0.0, 0.0, -10.0), Vec3::new(700.0, 100.0, 3e-5)).unwrap();
    let plane = Plane::from_normal(Vec3::ZERO, Vec3::Z).unwrap();
    assert!(crate::ops::off_plane(&b, &plane, true).is_none());
    assert!(crate::ops::off_plane(&b, &plane, false).is_none());
    // A face in the plane is moved.
    let plate = plate(4.0);
    let at = Plane::from_normal(Vec3::new(0.0, 0.0, 1.5), Vec3::Z).unwrap();
    assert!(crate::ops::off_plane(&plate, &at, true).is_some() && crate::ops::off_plane(&plate, &at, false).is_some());
}
