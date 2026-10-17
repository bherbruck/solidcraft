use serde::Serialize;
use solidcraft_geom::{Aabb3, Mesh, Vec3};

use crate::Result;
use crate::body::Body;
use crate::topo::{TopoCounts, merged_topology};

/// Measured properties of a body.
#[derive(Clone, Debug, Serialize)]
pub struct BodyMeasure {
    pub volume: f64,
    pub area: f64,
    pub centroid: Vec3,
    pub bbox: Aabb3,
    /// Raw kernel topology counts (closed curves and surfaces may be split in pieces).
    pub faces: usize,
    pub edges: usize,
    pub vertices: usize,
    pub shells: usize,
    /// Counts after merging split pieces of the same analytic surface/curve (comparable with
    /// systems that use periodic faces and edges).
    pub merged: TopoCounts,
}

/// Per-face sums: area, 6 × signed volume and 24 × first moments (divergence theorem, origin 0),
/// and the sum of the area vectors (zero over a closed surface).
fn face_sums(m: &Mesh, nf: usize) -> Vec<(f64, f64, Vec3, Vec3)> {
    let mut out = vec![(0.0, 0.0, Vec3::ZERO, Vec3::ZERO); nf];
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        let (Some([a, b, c]), Some(slot)) = (m.tri(t), out.get_mut(*f as usize)) else { continue };
        let d = a.dot(b.cross(c));
        let n = (b - a).cross(c - a) * 0.5;
        slot.0 += n.len();
        slot.1 += d;
        slot.2 += (a + b + c) * d;
        slot.3 += n;
    }
    out
}

/// Whether the mesh is closed: every edge, its ends matched by position to within `quantum`,
/// is crossed as often one way as the other.
fn is_closed(m: &Mesh, quantum: f64) -> bool {
    let key = |p: Vec3| [p.x, p.y, p.z].map(|x| (x / quantum).round() as i64);
    let mut net: std::collections::HashMap<([i64; 3], [i64; 3]), i32> = std::collections::HashMap::new();
    for t in &m.triangles {
        let Some([a, b, c]) = m.tri(t) else { return false };
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let (p, q) = (key(p), key(q));
            if p < q {
                *net.entry((p, q)).or_insert(0) += 1;
            } else if q < p {
                *net.entry((q, p)).or_insert(0) -= 1;
            }
        }
    }
    !m.triangles.is_empty() && net.values().all(|n| *n == 0)
}

/// Area, 6 × volume and 24 × first moments of a closed mesh, taken about `o` (near the body,
/// so that a body far from the origin does not cancel its terms in rounding) and returned about
/// the origin.
fn closed_sums(m: &Mesh, o: Vec3) -> (f64, f64, Vec3) {
    let (mut area, mut v6, mut mom) = (0.0, 0.0, Vec3::ZERO);
    for t in &m.triangles {
        let Some([a, b, c]) = m.tri(t) else { continue };
        area += (b - a).cross(c - a).len() * 0.5;
        let (a, b, c) = (a - o, b - o, c - o);
        let d = a.dot(b.cross(c));
        v6 += d;
        mom += (a + b + c) * d;
    }
    (area, v6, o * (4.0 * v6) + mom)
}

/// Smallest radius of curvature along the mesh's edge polylines (straight edges ignored).
fn min_edge_radius(m: &Mesh) -> Option<f64> {
    let mut best: Option<f64> = None;
    for e in &m.edges {
        for w in e.windows(3) {
            let (Some(&a), Some(&b), Some(&c)) = (w.first(), w.get(1), w.get(2)) else { continue };
            let (ab, bc, ca) = (b - a, c - b, a - c);
            let cross = ab.cross(bc).len();
            if !(cross > 1e-12 * ab.len() * bc.len()) {
                continue;
            }
            let r = ab.len() * bc.len() * ca.len() / (2.0 * cross);
            if r.is_finite() && r > 0.0 && best.is_none_or(|x| r < x) {
                best = Some(r);
            }
        }
    }
    best
}

/// Measure a body from a fine tessellation (chord tolerance 5e-5 of the body size, finer for
/// small radii down to 5e-6). Faces whose fine tessellation disagrees with a coarse one (a
/// meshing failure) use the coarse result.
pub fn measure(b: &Body) -> Result<BodyMeasure> {
    if let Some((pos, tris)) = b.triangle_mesh() {
        // Exact for a mesh body; topology counts are facets, sides and vertices.
        let m = b.tessellate(1.0)?;
        let mm = m.measure();
        let edges = b.mesh.as_ref().map(|m| m.edge_count()).unwrap_or(0);
        let merged =
            TopoCounts { faces: tris.len(), edges, vertices: pos.len(), face_types: [("mesh".to_string(), tris.len())].into_iter().collect() };
        return Ok(BodyMeasure {
            volume: mm.volume,
            area: mm.area,
            centroid: mm.centroid,
            bbox: m.bounds(),
            faces: tris.len(),
            edges,
            vertices: pos.len(),
            shells: 1,
            merged,
        });
    }
    let size = b.size();
    let coarse = b.tessellate_with((size * 1e-3).max(1e-3), true)?;
    // Chord error relative to a radius sets the volume error: keep it near 3e-4 of the
    // smallest radius (thin tubes, small fillets).
    let tight = min_edge_radius(&coarse).map(|r| r * 3e-4).unwrap_or(f64::INFINITY);
    let fine_tol = (size * 5e-5).min(tight).max(size * 5e-6).max(1e-4);
    let fine = b.tessellate_with(fine_tol, true)?;
    let medium = b.tessellate_with((fine_tol * 5.0).max(2e-4), true)?;
    let nf = b.face_count();
    let (sf, sm, sc) = (face_sums(&fine, nf), face_sums(&medium, nf), face_sums(&coarse, nf));
    let close = |x: f64, y: f64| (x - y).abs() <= 0.01 * x.abs().max(y.abs()) + 1e-9;
    let (mut area, mut v6, mut mom, mut normal) = (0.0, 0.0, Vec3::ZERO, Vec3::ZERO);
    for ((f, m), c) in sf.iter().zip(&sm).zip(&sc) {
        // Trust the finest tessellation that agrees with the next coarser one.
        let pick = if close(f.0, m.0) {
            f
        } else if close(m.0, c.0) {
            m
        } else {
            f
        };
        area += pick.0;
        v6 += pick.1;
        mom += pick.2;
        normal += pick.3;
    }
    // A closed surface's area vectors sum to zero. Where they don't, the meshes leave a gap: the
    // reference mesher can leave a crack where a trimmed face meets a hole cut into it (a notch
    // through a cone). The volume beside the gap is then left out, and the sums close the gap by
    // a cone to the origin instead, which moves the volume with the body's distance from the
    // origin (a crack of 0.3 mm² moved a part ring's volume by 7 % 1000 mm away). When the gap
    // could move the volume by more than 1e-5 of it within the body's size, and the display
    // plan, which shares every edge's nodes between its faces, is closed, measure that.
    if normal.len() * size * 2.0 > 1e-5 * v6.abs()
        && let Some(plan) = b.display_plan(fine_tol).ok().map(|p| p.evaluate()).filter(|m| is_closed(m, (size * 1e-9).max(1e-12)))
    {
        (area, v6, mom) = closed_sums(&plan, plan.bounds().center());
    }
    let volume = v6 / 6.0;
    let centroid = if v6.abs() > 1e-12 { mom / (4.0 * v6) } else { fine.bounds().center() };
    Ok(BodyMeasure {
        volume,
        area,
        centroid,
        bbox: coarse.bounds().union(&fine.bounds()),
        faces: b.face_count(),
        edges: b.edge_count(),
        vertices: b.vertex_count(),
        shells: b.shell_count(),
        merged: merged_topology(b, &b.tessellate((size * 1e-3).max(1e-3))?)?,
    })
}
