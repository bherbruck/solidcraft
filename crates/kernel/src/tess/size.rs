//! Target triangle size: one side length per face, so triangles come out about equilateral
//! (well-shaped triangles shade evenly) and as small as the surface needs.
//!
//! A face's size is the smallest of the body's longest side, the length over which the surface
//! normal turns the caller's facet angle (when one is given, see [`Turn`]), and the chord that
//! sags the tolerance; an edge takes the smaller size of its two faces, so a curved face and
//! its flat neighbour meet at the same spacing.

use solidcraft_geom::Vec3;
use solidcraft_geom::tess::EvalSurface;

use super::uv::Domain;

/// Longest side, as a fraction of the body's extent.
const MAX_LEN_FRACTION: f64 = 1.0 / 5.0;
/// Longest side relative to the side the curvature asks for: along a straight direction (a
/// cylinder's axis) cells may be this many times longer than across it. 2: cells stay close to
/// square (patches mesh as grids) without doubling the rows along every straight surface.
const ASPECT: f64 = 2.0;
/// Shortest side the aspect limit may ask for, as a fraction of the longest side. A bend whose
/// radius is tiny next to the body (a hem) would otherwise tie the whole length of its
/// straight edges to that radius; along a straight direction the normal does not turn, so
/// long triangles there shade exactly, and the turn and sag limits still divide the bend
/// across.
const MIN_LEN_FRACTION: f64 = 1.0 / 4.0;
/// Curvature samples per parameter direction.
const SAMPLES: usize = 9;

/// The most one facet may turn (radians): an edge segment's tangent or a cell's surface normal.
/// The angle divides only where a facet would sag more than `floor` (mm), so a radius far below
/// the tolerance keeps its chord facets.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Turn {
    pub angle: f64,
    pub floor: f64,
}

impl Turn {
    /// Whether a facet that turns `angle` and sags `sag` from the surface must be divided.
    pub(crate) fn exceeds(&self, angle: f64, sag: f64) -> bool {
        sag > self.floor && angle > self.angle
    }

    /// The angle doubled `level` times, as the reference mesher relaxes it for a crowded face;
    /// from its last level, or past half a turn, no angle at all (the tolerance alone).
    pub(crate) fn relaxed(&self, level: u8) -> Option<Turn> {
        use truck_meshalgo::tessellation::Deflection;
        let angle = self.angle * f64::from(1u32 << u32::from(level.min(Deflection::MAX_RELAX)));
        (level == 0 || (level < Deflection::MAX_RELAX && angle < std::f64::consts::PI)).then_some(Turn { angle, ..*self })
    }
}

/// The chord tolerance, the longest side and the facet angle of one body.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    pub tol: f64,
    pub max_len: f64,
    pub turn: Option<Turn>,
}

impl Rules {
    /// Rules for a body of extent `extent` (see [`extent`]).
    pub(crate) fn new(tol: f64, extent: f64, turn: Option<Turn>) -> Rules {
        let max_len = if extent.is_finite() && extent > 0.0 { extent * MAX_LEN_FRACTION } else { f64::INFINITY };
        Rules { tol, max_len: max_len.max(tol), turn }
    }

    /// The side length for a curvature `k` (1/mm): the chord sags at most the tolerance across
    /// it, and the normal turns at most the facet angle where that turn sags above its floor.
    fn for_curvature(&self, k: f64) -> f64 {
        if !(k.is_finite() && k > 0.0) {
            return self.max_len;
        }
        let r = 1.0 / k;
        let mut across = (8.0 * r * self.tol).sqrt();
        if let Some(t) = self.turn
            && r * (1.0 - (0.5 * t.angle).cos()) > t.floor
        {
            across = across.min(r * t.angle);
        }
        let aspect = ASPECT * across;
        self.max_len.min(aspect.max(self.max_len * MIN_LEN_FRACTION)).max(self.tol)
    }
}

/// The body's extent: twice the largest distance of a vertex from the vertices' centroid. Unlike
/// a bounding box diagonal it does not change when the body is turned, so a placed copy meshes
/// like the original.
pub(crate) fn extent(points: impl Iterator<Item = Vec3>) -> f64 {
    let points: Vec<Vec3> = points.filter(|p| p.is_finite()).collect();
    if points.is_empty() {
        return 0.0;
    }
    let c = points.iter().fold(Vec3::ZERO, |a, p| a + *p) * (1.0 / points.len() as f64);
    2.0 * points.iter().map(|p| p.dist(c)).fold(0.0, f64::max)
}

/// A face's target side length and the surface's mean speed along u and v (mm per parameter
/// unit), from samples over the surface's parameter box (the whole box of a trimmed surface,
/// so the size is never larger than the face needs).
pub(crate) fn face_size(s: &EvalSurface, dom: &Domain, rules: &Rules) -> (f64, [f64; 2]) {
    if let EvalSurface::Plane { du, dv, .. } = s {
        return (rules.max_len, [du.len(), dv.len()]);
    }
    let (Some(ru), Some(rv)) = (dom.range[0], dom.range[1]) else { return (rules.max_len, [1.0, 1.0]) };
    // A flat spline (sheet-metal walls, chamfers): no curvature to look for, only the speeds.
    let samples = if planar(s) { 3 } else { SAMPLES };
    let normal = |u: f64, v: f64| -> Option<(Vec3, Vec3, [f64; 2])> {
        let (p, su, sv) = s.derivs(u, v)?;
        Some((p, su.cross(sv).normalized()?, [su.len(), sv.len()]))
    };
    let step = [(ru.1 - ru.0) * 1e-3, (rv.1 - rv.0) * 1e-3];
    let mut k_max: f64 = 0.0;
    let (mut speed, mut count) = ([0.0; 2], 0usize);
    for i in 0..samples {
        for j in 0..samples {
            // Cell centres, clear of the box edges (poles, seams).
            let u = ru.0 + (ru.1 - ru.0) * (i as f64 + 0.5) / samples as f64;
            let v = rv.0 + (rv.1 - rv.0) * (j as f64 + 0.5) / samples as f64;
            let Some((p, n, sp)) = normal(u, v) else { continue };
            speed = [speed[0] + sp[0], speed[1] + sp[1]];
            count += 1;
            if samples < SAMPLES {
                continue;
            }
            // How fast the normal turns per mm, along u and along v (twist included).
            for (du, dv) in [(step[0], 0.0), (0.0, step[1])] {
                let Some((q, m, _)) = normal(u + du, v + dv) else { continue };
                let ds = p.dist(q);
                if ds > 0.0 {
                    k_max = k_max.max((m - n).len() / ds);
                }
            }
        }
    }
    let speed = if count > 0 { [speed[0] / count as f64, speed[1] / count as f64] } else { [1.0, 1.0] };
    (rules.for_curvature(k_max), speed.map(|x| if x.is_finite() && x > 0.0 { x } else { 1.0 }))
}

/// A spline surface whose control points lie in one plane, with positive weights: the
/// surface lies in that plane (it is inside the control points' convex hull).
fn planar(s: &EvalSurface) -> bool {
    let EvalSurface::Nurbs { ctrl, .. } = s else { return false };
    if ctrl.iter().any(|h| !(h[3] > 0.0)) {
        return false;
    }
    let pts: Vec<Vec3> = ctrl.iter().map(|h| Vec3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3])).collect();
    let Some(&a) = pts.first() else { return false };
    let size = pts.iter().map(|p| p.dist(a)).fold(0.0, f64::max);
    // The two points that span the widest triangle with the first give the plane.
    let Some(&b) = pts.iter().max_by(|p, q| p.dist(a).total_cmp(&q.dist(a))) else { return false };
    let Some(n) = pts.iter().map(|p| (b - a).cross(*p - a)).max_by(|x, y| x.len().total_cmp(&y.len())).and_then(Vec3::normalized) else {
        return false;
    };
    size > 0.0 && pts.iter().all(|p| (*p - a).dot(n).abs() <= 1e-9 * size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcraft_geom::tess::EvalCurve;

    const THIRTY: Option<Turn> = Some(Turn { angle: std::f64::consts::PI / 6.0, floor: 0.0 });

    #[test]
    fn a_cylinder_is_sized_by_its_radius() {
        let s = EvalSurface::Revolved {
            profile: EvalCurve { degree: 1, knots: vec![0.0, 0.0, 1.0, 1.0], ctrl: vec![[3.0, 0.0, 0.0, 1.0], [3.0, 0.0, 40.0, 1.0]] },
            origin: Vec3::ZERO,
            axis: Vec3::Z,
            transform: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
            swap: false,
        };
        // A 20 mm body: sides from 1 mm (the floor, a quarter of the longest) to 4 mm.
        let rules = Rules::new(0.01, 20.0, THIRTY);
        let (h, speed) = face_size(&s, &super::super::uv::domain(&s), &rules);
        // r = 3: 30° of turn is 1.57 mm; the 0.01 mm sag chord is √(8·3·0.01) = 0.49 mm, times the
        // aspect, unless that is under the floor.
        assert!((h - (ASPECT * (8.0 * 3.0 * 0.01f64).sqrt()).max(20.0 * MAX_LEN_FRACTION * MIN_LEN_FRACTION)).abs() < 0.01, "{h}");
        assert!((speed[0] - 40.0).abs() < 1e-6 && (speed[1] - 3.0).abs() < 1e-6, "{speed:?}");
        // Coarser: the turn limit governs.
        let (h, _) = face_size(&s, &super::super::uv::domain(&s), &Rules::new(0.5, 100.0, THIRTY));
        assert!((h - (ASPECT * 3.0 * std::f64::consts::PI / 6.0).max(100.0 * MAX_LEN_FRACTION * MIN_LEN_FRACTION)).abs() < 0.01, "{h}");
        // Without a facet angle the sag alone: √(8·3·0.5) = 3.46 mm, four times along.
        let (h, _) = face_size(&s, &super::super::uv::domain(&s), &Rules::new(0.5, 100.0, None));
        assert!((h - ASPECT * (8.0 * 3.0 * 0.5f64).sqrt()).abs() < 0.01, "{h}");
        // A turn that sags below its floor does not divide.
        let floored = Some(Turn { angle: std::f64::consts::PI / 6.0, floor: 1.0 });
        let (h, _) = face_size(&s, &super::super::uv::domain(&s), &Rules::new(0.5, 100.0, floored));
        assert!((h - ASPECT * (8.0 * 3.0 * 0.5f64).sqrt()).abs() < 0.01, "{h}");
    }

    #[test]
    fn a_flat_spline_is_planar_and_a_bent_one_is_not() {
        let patch = |z: f64| EvalSurface::Nurbs {
            degree: [1, 1],
            knots: [vec![0.0, 0.0, 1.0, 1.0], vec![0.0, 0.0, 1.0, 1.0]],
            count: [2, 2],
            ctrl: vec![[0.0, 0.0, 0.0, 1.0], [4.0, 0.0, 0.0, 1.0], [0.0, 3.0, 0.0, 1.0], [4.0, 3.0, z, 1.0]],
            closed: [false, false],
        };
        assert!(planar(&patch(0.0)));
        assert!(!planar(&patch(0.5)));
        let rules = Rules::new(0.01, 100.0, THIRTY);
        let (h, speed) = face_size(&patch(0.0), &super::super::uv::domain(&patch(0.0)), &rules);
        assert_eq!(h, rules.max_len);
        assert!((speed[0] - 4.0).abs() < 1e-9 && (speed[1] - 3.0).abs() < 1e-9, "{speed:?}");
        // The twisted patch (a hyperbolic paraboloid) has curvature: a smaller side.
        assert!(face_size(&patch(0.5), &super::super::uv::domain(&patch(0.5)), &rules).0 < rules.max_len);
    }

    #[test]
    fn a_tiny_radius_does_not_shrink_the_side_below_the_floor() {
        let s = EvalSurface::Revolved {
            profile: EvalCurve { degree: 1, knots: vec![0.0, 0.0, 1.0, 1.0], ctrl: vec![[0.05, 0.0, 0.0, 1.0], [0.05, 0.0, 80.0, 1.0]] },
            origin: Vec3::ZERO,
            axis: Vec3::Z,
            transform: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
            swap: false,
        };
        let rules = Rules::new(0.1, 100.0, THIRTY);
        let (h, _) = face_size(&s, &super::super::uv::domain(&s), &rules);
        assert!((h - rules.max_len * MIN_LEN_FRACTION).abs() < 1e-9, "{h}");
    }

    #[test]
    fn a_plane_takes_the_longest_side() {
        let s = EvalSurface::Plane { origin: Vec3::ZERO, du: Vec3::X, dv: Vec3::Y };
        let rules = Rules::new(0.01, 100.0, THIRTY);
        assert_eq!(face_size(&s, &super::super::uv::domain(&s), &rules).0, 20.0);
    }
}
