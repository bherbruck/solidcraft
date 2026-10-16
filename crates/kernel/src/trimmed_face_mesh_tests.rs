//! Tessellation of trimmed loft faces after a boolean: no triangle may leave the face.

use std::f64::consts::FRAC_PI_2;

use solidcraft_geom::{Loop2, Plane, Seg2, Vec2, Vec3};
use truck_modeling::{self as mt, SearchNearestParameter};

use crate::*;

/// Counter-clockwise rounded rectangle centred on the origin.
fn rounded(w: f64, h: f64, r: f64) -> Loop2 {
    let (x, y) = (w / 2.0, h / 2.0);
    let arc = |cx: f64, cy: f64, start: f64| Seg2::Arc { center: Vec2::new(cx, cy), radius: r, start, sweep: FRAC_PI_2 };
    Loop2 {
        segs: vec![
            arc(x - r, -y + r, -FRAC_PI_2),
            Seg2::Line { a: Vec2::new(x, -y + r), b: Vec2::new(x, y - r) },
            arc(x - r, y - r, 0.0),
            Seg2::Line { a: Vec2::new(x - r, y), b: Vec2::new(-x + r, y) },
            arc(-x + r, y - r, FRAC_PI_2),
            Seg2::Line { a: Vec2::new(-x, y - r), b: Vec2::new(-x, -y + r) },
            arc(-x + r, -y + r, 2.0 * FRAC_PI_2),
            Seg2::Line { a: Vec2::new(-x + r, -y), b: Vec2::new(x - r, -y) },
        ],
    }
}

/// The down tube of a bicycle frame: rounded-rectangle sections lofted along a bend.
fn tube() -> Body {
    let s = |o: [f64; 3], y: [f64; 3], w: f64, h: f64, r: f64| {
        (Plane::new(Vec3::new(o[0], o[1], o[2]), Vec3::Y, Vec3::new(y[0], y[1], y[2])).unwrap(), rounded(w, h, r))
    };
    loft(&[
        s([8.0, 0.0, 3.0], [-0.178, 0.0, 0.984], 38.0, 36.0, 10.8),
        s([80.0, 0.0, 16.0], [-0.211, 0.0, 0.977], 60.0, 52.0, 15.6),
        s([175.0, 0.0, 40.0], [-0.596, 0.0, 0.803], 76.0, 66.0, 19.8),
        s([314.248, 0.0, 271.657], [-0.857, 0.0, 0.515], 74.0, 66.0, 19.8),
        s([422.553, 0.0, 451.835], [-0.857, 0.0, 0.515], 66.0, 60.0, 18.0),
        s([484.441, 0.0, 554.793], [-0.857, 0.0, 0.515], 46.0, 44.0, 13.2),
    ])
    .unwrap()
}

/// Each face's mesh area, and how far its triangles' centroids lie off its surface at most.
fn per_face(body: &Body, mesh: &solidcraft_geom::Mesh) -> Vec<(f64, f64)> {
    use mt::ParametricSurface;
    let faces: Vec<&mt::Face> = body.solid.face_iter().collect();
    let mut out = vec![(0.0, 0.0f64); faces.len()];
    for (t, &f) in mesh.triangles.iter().zip(&mesh.tri_face) {
        let ([a, b, c], Some(face), Some(slot)) = (mesh.tri(t).unwrap(), faces.get(f as usize), out.get_mut(f as usize)) else { continue };
        slot.0 += (b - a).cross(c - a).len() / 2.0;
        let g = (a + b + c) / 3.0;
        let surface = face.oriented_surface();
        let (u, v) = surface.search_nearest_parameter(mt::Point3::new(g.x, g.y, g.z), None, 50).unwrap();
        slot.1 = slot.1.max(crate::body::from_p3(surface.subs(u, v)).dist(g));
    }
    out
}

/// A lofted tube whose side faces are ruled across (straight section edges) and long along
/// the loft, joined to a cylinder that trims their ends: the display mesh of a trimmed side
/// fanned one trimming node to boundary nodes hundreds of millimetres away (Delaunay in raw
/// parameters on a strip stretched twentyfold), a flat sheet off the surface.
#[test]
fn trimmed_ruled_loft_faces_mesh_on_their_surfaces() {
    let tube = tube();
    let shell = cylinder(Vec3::new(0.0, -36.5, 0.0), Vec3::Y, 23.0, 73.0).unwrap();
    let joined = boolean(&tube, &shell, BoolOp::Union).unwrap().unwrap();
    let tol = (joined.size() * 1e-3).max(1e-3);
    let display = per_face(&joined, &joined.reference_display_mesh_angular(tol, DISPLAY_ANGLE).unwrap());
    let fine = per_face(&joined, &joined.tessellate(tol / 8.0).unwrap());
    for (fi, ((area, off), (fine_area, _))) in display.iter().zip(&fine).enumerate() {
        // A centroid sags at most about the chord tolerance (the bad sheet was 7 mm off at 0.6).
        assert!(*off < 2.0 * tol, "face {fi}: a triangle lies {off:.2} mm off the surface (tolerance {tol:.2})");
        // Covered once: the area of a much finer mesh (the bad face had two and a half times it).
        assert!((area - fine_area).abs() < 0.02 * fine_area, "face {fi}: mesh area {area:.0} mm², finer mesh {fine_area:.0} mm²");
    }
}
