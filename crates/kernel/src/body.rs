use serde::Serialize;
use solidcraft_geom::{Mesh, Vec3};
use truck_meshalgo::prelude::*;
use truck_modeling as mt;

use crate::{KernelError, Result, guard};

#[path = "planar_regularize.rs"]
mod planar_regularize;
#[path = "planar_sources.rs"]
mod planar_sources;
#[path = "shell_groups.rs"]
pub(crate) mod shell_groups;

pub(crate) type Solid = mt::Solid;

/// A solid body (one or more closed shells), or a mesh body (triangles only, like an imported
/// 3MF or STL: it renders, measures and exports, but B-rep features cannot use it). Cheap to
/// clone; never mutated in place.
#[derive(Clone, Debug)]
pub struct Body {
    pub(crate) solid: std::sync::Arc<Solid>,
    /// Bounding-box diagonal, measured by [`Body::new`] while it read the vertices anyway; 0
    /// means "not measured, walk the geometry". Every kernel tolerance is a fraction of it.
    pub(crate) size: f64,
    /// The last volume measured for this solid and the tolerance it was measured at, shared by
    /// clones because they share the solid. A boolean measures its operands, and a later boolean
    /// takes the result as an operand again, so the same body is measured twice otherwise.
    pub(crate) volume: Option<std::sync::Arc<std::sync::Mutex<Option<(u64, f64)>>>>,
    /// Missing-face check from the orientation mesh, reusable at exactly its tolerance.
    pub(crate) coarse_missing: Option<(f64, usize)>,
    /// Set for mesh bodies (`solid` is then empty).
    pub(crate) mesh: Option<std::sync::Arc<crate::meshbody::TriMesh>>,
    /// Display colour (linear RGB 0..1) from an imported file; operations drop it.
    pub(crate) color: Option<[f32; 3]>,
    /// Opacity and face colours for export (set by the document; operations drop it).
    pub(crate) paint: Option<std::sync::Arc<Paint>>,
    /// Lines the user split faces along (Split Face), as polylines: export keeps the faces on
    /// either side apart where it would otherwise join pieces of one surface.
    pub(crate) splits: Option<std::sync::Arc<Vec<Vec<Vec3>>>>,
    /// Exact construction/import source identities, in this body's face order.
    pub(crate) face_sources: Option<std::sync::Arc<Vec<Option<FaceSource>>>>,
    /// Sticky transient veto: authored face boundaries must survive later rebuilds.
    pub(crate) regularization_protected: bool,
    /// Exact history of the operation that made this body, relative to that operation's
    /// operands; `None` when the operation does not report one.
    pub(crate) history: Option<std::sync::Arc<crate::history::History>>,
    /// A placed copy of a solid that another body shows already (an instance of a part a STEP
    /// assembly lists many times): its display mesh is that body's, placed by the matrix, so the
    /// part is tessellated once rather than once per instance. Dropped by anything that changes
    /// what the body shows (its geometry is never changed in place).
    pub(crate) display_of: Option<std::sync::Arc<(std::sync::Arc<SharedDisplay>, [[f64; 4]; 4])>>,
}

/// The display mesh of a body that several placed copies show (see `Body::display_of`): the
/// body, and its mesh once made.
#[derive(Debug)]
pub(crate) struct SharedDisplay {
    pub(crate) original: Body,
    mesh: std::sync::OnceLock<Option<Mesh>>,
}

impl SharedDisplay {
    pub(crate) fn new(original: Body) -> Self {
        SharedDisplay { original: Body { display_of: None, ..original }, mesh: std::sync::OnceLock::new() }
    }
}

/// `m` placed by the rigid matrix `t` (by columns, translation in `t[3]`, as the kernel's
/// transforms take it): positions, normals and edge polylines move, everything indexed stays.
/// `None` for anything but a rotation and translation (a reflection would turn the triangles
/// inside out).
fn placed_mesh(m: &Mesh, t: &[[f64; 4]; 4]) -> Option<Mesh> {
    let r = |v: Vec3, w: f64| {
        Vec3::new(
            t[0][0] * v.x + t[1][0] * v.y + t[2][0] * v.z + w * t[3][0],
            t[0][1] * v.x + t[1][1] * v.y + t[2][1] * v.z + w * t[3][1],
            t[0][2] * v.x + t[1][2] * v.y + t[2][2] * v.z + w * t[3][2],
        )
    };
    let det = t[0][0] * (t[1][1] * t[2][2] - t[1][2] * t[2][1]) - t[0][1] * (t[1][0] * t[2][2] - t[1][2] * t[2][0])
        + t[0][2] * (t[1][0] * t[2][1] - t[1][1] * t[2][0]);
    if !(det > 0.0 && (det - 1.0).abs() < 1e-6) || t.iter().flatten().any(|x| !x.is_finite()) {
        return None;
    }
    Some(Mesh {
        positions: m.positions.iter().map(|p| r(*p, 1.0)).collect(),
        normals: m.normals.iter().map(|n| r(*n, 0.0)).collect(),
        edges: m.edges.iter().map(|e| e.iter().map(|p| r(*p, 1.0)).collect()).collect(),
        ..m.clone()
    })
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FaceSource {
    ExtrudeSide {
        segment: solidcraft_geom::Seg2,
        plane: Option<solidcraft_geom::Plane>,
        coalesced: bool,
    },
    CertifiedExtrudeSide {
        producer: u64,
        sketch: u64,
        curve: String,
        plane: Option<solidcraft_geom::Plane>,
    },
    ExtrudeCap {
        high: bool,
        region: solidcraft_geom::Region2,
        plane: solidcraft_geom::Plane,
    },
    CertifiedExtrudeCap {
        producer: u64,
        sketch: u64,
        high: bool,
        outer: Vec<String>,
        holes: Vec<Vec<String>>,
        plane: solidcraft_geom::Plane,
    },
    Box {
        producer: Option<u64>,
        axis: u8,
        high: bool,
        plane: solidcraft_geom::Plane,
    },
    /// A cylinder primitive's cap: `high` at the far end (its `top`), else at the base.
    CylinderCap {
        producer: u64,
        high: bool,
        plane: solidcraft_geom::Plane,
    },
    ShellPlanar {
        producer: Option<u64>,
        source: Box<FaceSource>,
        inner: bool,
        plane: solidcraft_geom::Plane,
    },
    Imported {
        item: u64,
        face: u64,
        payload_sha256: Option<String>,
        producer: Option<u64>,
    },
}

pub type CapSignature = (Vec<String>, Vec<Vec<String>>);
type FaceSources = std::sync::Arc<Vec<Option<FaceSource>>>;

impl FaceSource {
    fn plane_mut(&mut self) -> Option<&mut solidcraft_geom::Plane> {
        match self {
            Self::ExtrudeSide { plane, .. } | Self::CertifiedExtrudeSide { plane, .. } => plane.as_mut(),
            Self::ExtrudeCap { plane, .. }
            | Self::CertifiedExtrudeCap { plane, .. }
            | Self::Box { plane, .. }
            | Self::CylinderCap { plane, .. }
            | Self::ShellPlanar { plane, .. } => Some(plane),
            Self::Imported { .. } => None,
        }
    }
}

/// How a body looks beyond its colour, for export: its opacity and its faces' own colours.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Paint {
    /// 1 = opaque.
    pub opacity: f32,
    pub faces: Vec<FacePaint>,
}

/// A face's own colour: the B-rep face index (`face_iter` order), sRGB as 0..1, opacity.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FacePaint {
    pub face: usize,
    pub color: [f32; 3],
    pub opacity: f32,
}

/// A B-rep edge as seen by the rest of the application.
#[derive(Clone, Debug, Serialize)]
pub struct EdgeInfo {
    pub index: usize,
    /// Polyline approximation.
    pub points: Vec<Vec3>,
    /// Point halfway along the polyline (used to re-find the edge).
    pub mid: Vec3,
    pub length: f64,
}

/// A B-rep face.
#[derive(Clone, Debug, Serialize)]
pub struct FaceInfo {
    pub index: usize,
    pub area: f64,
    pub centroid: Vec3,
    /// Unit normal if the face is planar.
    pub plane_normal: Option<Vec3>,
}

type MeshedShell = truck_topology::Shell<mt::Point3, PolylineCurve<mt::Point3>, Option<PolygonMesh>>;

/// The most one facet of the display mesh turns (radians): a curved face or edge is divided
/// until no facet spans more of it than this, however small its radius is beside the body.
/// 20 degrees: a quarter turn is drawn with at least five facets, a full circle with at least 18.
pub const DISPLAY_ANGLE: f64 = std::f64::consts::PI / 9.0;

/// The sag, as a fraction of the chord tolerance, below which the angle no longer divides a
/// facet: at [`DISPLAY_ANGLE`] and the application's tolerance of 1/1000 of the body, a radius
/// under about 1/15000 of the body gets fewer facets than the angle asks for, and one under
/// a millionth of it only those of the chord tolerance.
const DISPLAY_ANGLE_FLOOR: f64 = 1e-3;

/// The finest angle a display mesh is divided by (one degree).
const MIN_DISPLAY_ANGLE: f64 = std::f64::consts::PI / 180.0;

/// Triangulate each shell (not the solid: truck's solid constructor panics on the open or
/// non-manifold shells an import can produce). Boundaries that lie on their surface only within
/// tolerance (imports, boolean results) are projected onto it.
fn mesh_shells(solid: &Solid, tol: f64) -> Vec<MeshedShell> {
    mesh_shells_within(solid, Deflection { rulings: true, ..Deflection::chord(tol) })
}

/// [`mesh_shells`] divided by a chord tolerance and, when it has one, a bound on the angle a
/// facet spans. Each edge is divided once for both its faces, so the faces still share its
/// points.
fn mesh_shells_within(solid: &Solid, deflection: Deflection) -> Vec<MeshedShell> {
    solid
        .boundaries()
        .iter()
        // The robust search tries the exact one first, so exact geometry costs nothing extra; a
        // face can also come back as an empty mesh rather than none (boolean results with
        // approximate intersection curves), so always use it.
        .map(|sh| sh.robust_triangulation_within(deflection))
        .collect()
}

/// A face's triangles (corner positions and normals, in the mesh's winding) with every edge
/// whose middle sags more than `tol` from the surface split there, found through the mesh's
/// surface parameters. A split depends on the edge alone, so neighbours stay conforming.
/// `None` when nothing needs splitting or the parameters don't match the surface.
#[allow(clippy::type_complexity)]
fn refined(pm: &PolygonMesh, surf: &mt::Surface, tol: f64) -> Option<Vec<[(Vec3, Vec3); 3]>> {
    refined_with_cache(pm, surf, tol, true)
}

#[allow(clippy::type_complexity)]
fn refined_with_cache(pm: &PolygonMesh, surf: &mt::Surface, tol: f64, memoize: bool) -> Option<Vec<[(Vec3, Vec3); 3]>> {
    use mt::{ParametricSurface, ParametricSurface3D};
    let (pos, uvs, nor) = (pm.positions(), pm.uv_coords(), pm.normals());
    if uvs.is_empty() {
        return None;
    }
    type V = (Vec3, (f64, f64), Vec3);
    let mut tris: Vec<[V; 3]> = Vec::new();
    for tri in pm.faces().triangle_iter() {
        let mut t = [(Vec3::ZERO, (0.0, 0.0), Vec3::Z); 3];
        for (slot, v) in t.iter_mut().zip(tri.iter()) {
            let p = pos.get(v.pos)?;
            let uv = uvs.get(v.uv?)?;
            let n = v.nor.and_then(|i| nor.get(i)).map(|n| Vec3::new(n.x, n.y, n.z)).unwrap_or(Vec3::Z);
            *slot = (Vec3::new(p.x, p.y, p.z), (uv.x, uv.y), n);
        }
        tris.push(t);
    }
    // The parameters must be the surface's own.
    let check = tris.iter().take(8).flat_map(|t| t.iter()).all(|(p, (u, v), _)| from_p3(surf.subs(*u, *v)).dist(*p) < tol * 4.0 + 1e-9);
    // Bounded work: a face may grow to a few times its triangles (wild surfaces sag
    // everywhere and are left as meshed).
    let cap = (tris.len() * 8).clamp(2_000, 60_000);
    if !check || tris.len() > 50_000 {
        return None;
    }
    // Edges on the face's boundary are shared with the next face: never split them.
    let k = |x: &V| ((x.1.0 * 1e9).round() as i64, (x.1.1 * 1e9).round() as i64);
    let ek = |a: &V, b: &V| if k(a) < k(b) { (k(a), k(b)) } else { (k(b), k(a)) };
    // Only exact finite values identify reusable evaluations. In particular,
    // the rounded UV keys used for boundary incidence cannot identify a point
    // whose derivatives or midpoint may be reused.
    let vertex_key = |v: &V| {
        let values = [v.0.x, v.0.y, v.0.z, v.1.0, v.1.1, v.2.x, v.2.y, v.2.z];
        values.iter().all(|x| x.is_finite()).then(|| values.map(f64::to_bits))
    };
    let cache_limit = cap * 3;
    let mut singular_cache = std::collections::HashMap::<(u64, u64), bool>::new();
    let mut changed = false;
    for _ in 0..4 {
        let mut uses: std::collections::HashMap<((i64, i64), (i64, i64)), u32> = std::collections::HashMap::new();
        for [a, b, c] in &tris {
            for e in [ek(a, b), ek(b, c), ek(c, a)] {
                *uses.entry(e).or_default() += 1;
            }
        }
        let mut edge_cache = std::collections::HashMap::<([u64; 8], [u64; 8]), Option<V>>::new();
        let mut at = |a: &V, b: &V| -> Option<V> {
            let key = if memoize { vertex_key(a).zip(vertex_key(b)).map(|(a, b)| if a < b { (a, b) } else { (b, a) }) } else { None };
            if let Some(key) = key
                && let Some(value) = edge_cache.get(&key)
            {
                return *value;
            }
            let mut calculate = || -> Option<V> {
                if uses.get(&ek(a, b)).copied().unwrap_or(0) < 2 {
                    return None;
                }
                // Not at a degenerate point (a cone's apex: its parameters there are arbitrary).
                let mut singular = |x: &V| {
                    let key = (x.1.0.to_bits(), x.1.1.to_bits());
                    let cacheable = memoize && x.1.0.is_finite() && x.1.1.is_finite();
                    if cacheable && let Some(value) = singular_cache.get(&key) {
                        return *value;
                    }
                    let (du, dv) = (surf.uder(x.1.0, x.1.1), surf.vder(x.1.0, x.1.1));
                    let c = du.cross(dv);
                    let value = c.x * c.x + c.y * c.y + c.z * c.z
                        <= 1e-12 * (du.x * du.x + du.y * du.y + du.z * du.z + dv.x * dv.x + dv.y * dv.y + dv.z * dv.z).powi(2) + 1e-300;
                    if cacheable && singular_cache.len() < cache_limit {
                        singular_cache.insert(key, value);
                    }
                    value
                };
                if singular(a) || singular(b) {
                    return None;
                }
                let (u, v) = ((a.1.0 + b.1.0) / 2.0, (a.1.1 + b.1.1) / 2.0);
                let q = from_p3(surf.subs(u, v));
                // Within tolerance, or so far off that the parameters wrapped (a seam).
                let sag = q.dist((a.0 + b.0) * 0.5);
                if sag <= tol || sag >= 0.5 * a.0.dist(b.0) {
                    return None;
                }
                let n = surf.normal(u, v);
                let mut n = Vec3::new(n.x, n.y, n.z);
                // Keep the mesh's normal side.
                if n.dot(a.2 + b.2) < 0.0 {
                    n = -n;
                }
                Some((q, (u, v), n))
            };
            let value = calculate();
            if let Some(key) = key
                && edge_cache.len() < cache_limit
                && value.is_none_or(|v| vertex_key(&v).is_some())
            {
                edge_cache.insert(key, value);
            }
            value
        };
        let mut next = Vec::with_capacity(tris.len());
        let mut any = false;
        for t in &tris {
            let [a, b, c] = t;
            let m = [at(a, b), at(b, c), at(c, a)];
            any |= m.iter().any(Option::is_some);
            match m {
                [None, None, None] => next.push(*t),
                [Some(x), Some(y), Some(z)] => {
                    next.extend([[*a, x, z], [x, *b, y], [z, y, *c], [x, y, z]]);
                }
                [Some(x), None, None] => next.extend([[*a, x, *c], [x, *b, *c]]),
                [None, Some(y), None] => next.extend([[*a, *b, y], [*a, y, *c]]),
                [None, None, Some(z)] => next.extend([[*a, *b, z], [z, *b, *c]]),
                [Some(x), Some(y), None] => next.extend([[*a, x, *c], [x, *b, y], [x, y, *c]]),
                [None, Some(y), Some(z)] => next.extend([[*a, *b, z], [z, *b, y], [z, y, *c]]),
                [Some(x), None, Some(z)] => next.extend([[*a, x, z], [x, *b, *c], [x, *c, z]]),
            }
        }
        tris = next;
        changed |= any;
        if !any || tris.len() > cap {
            break;
        }
    }
    changed.then(|| tris.into_iter().map(|t| t.map(|(p, _, n)| (p, n))).collect())
}

/// Revolved surfaces are periodic in their angle; a face whose angles start at 0 is found at 0
/// or 2π, which confuses the boolean's loop projection. Each revolved face's surface is turned
/// (same geometry) so the face sits around angle π, well away from the wrap.
fn rephase_revolved(solid: Solid) -> Solid {
    use mt::{BoundedCurve, ParametricCurve, SearchNearestParameter};
    let r = guard("rephase", || {
        let mut changed = false;
        let mut shells = Vec::new();
        for sh in solid.boundaries() {
            let mut faces = Vec::new();
            for f in sh.face_iter() {
                let mt::Surface::RevolutedCurve(proc_) = f.surface() else {
                    faces.push(f.clone());
                    continue;
                };
                let surf = f.surface();
                // The face's middle angle.
                let mut acc = Vec3::ZERO;
                let mut n = 0.0f64;
                for w in f.absolute_boundaries() {
                    for e in w.edge_iter() {
                        let c = e.oriented_curve();
                        let (t0, t1) = c.range_tuple();
                        for k in 0..4 {
                            acc += from_p3(c.subs(t0 + (t1 - t0) * k as f64 / 4.0));
                            n += 1.0;
                        }
                    }
                }
                let Some((_, v)) = surf.search_nearest_parameter(p3(acc / n.max(1.0)), None, 100) else {
                    faces.push(f.clone());
                    continue;
                };
                let phi = v.rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                if phi.abs() < 0.3 {
                    faces.push(f.clone());
                    continue;
                }
                let new_proc = proc_.map_ref(|rc| {
                    let (o, a) = (rc.origin(), rc.axis());
                    let rot = mt::Matrix4::from_translation(mt::EuclideanSpace::to_vec(o))
                        * mt::Matrix4::from_axis_angle(a, mt::Rad(phi))
                        * mt::Matrix4::from_translation(-mt::EuclideanSpace::to_vec(o));
                    mt::RevolutedCurve::by_revolution(mt::Transformed::transformed(rc.entity_curve(), rot), o, a)
                });
                match mt::Face::try_new(f.absolute_boundaries().clone(), mt::Surface::RevolutedCurve(new_proc)) {
                    Ok(mut nf) => {
                        if !f.orientation() {
                            nf.invert();
                        }
                        faces.push(nf);
                        changed = true;
                    }
                    Err(_) => faces.push(f.clone()),
                }
            }
            shells.push(mt::Shell::from(faces));
        }
        if !changed {
            return Ok(None);
        }
        Ok(Solid::try_new(shells).ok())
    });
    match r {
        Ok(Some(s)) => s,
        _ => solid,
    }
}

pub(crate) fn p3(v: Vec3) -> mt::Point3 {
    mt::Point3::new(v.x, v.y, v.z)
}
pub(crate) fn v3(v: Vec3) -> mt::Vector3 {
    mt::Vector3::new(v.x, v.y, v.z)
}
pub(crate) fn from_p3(p: mt::Point3) -> Vec3 {
    Vec3::new(p.x, p.y, p.z)
}

/// The distinct edges of `faces` in order of first use, and the faces (by position) each one
/// bounds: the one edge order of a body, used by [`Body::edges`], tessellations and
/// [`crate::history`].
pub(crate) fn edge_order<'a, P: 'a, C: 'a, S: 'a>(
    faces: impl IntoIterator<Item = &'a truck_topology::Face<P, C, S>>,
) -> (Vec<truck_topology::Edge<P, C>>, Vec<Vec<usize>>) {
    let mut index = std::collections::HashMap::new();
    let (mut edges, mut bounds): (Vec<truck_topology::Edge<P, C>>, Vec<Vec<usize>>) = (Vec::new(), Vec::new());
    for (fi, f) in faces.into_iter().enumerate() {
        for e in f.edge_iter() {
            let i = *index.entry(e.id()).or_insert_with(|| {
                edges.push(e.clone());
                bounds.push(Vec::new());
                edges.len() - 1
            });
            if let Some(slot) = bounds.get_mut(i)
                && !slot.contains(&fi)
            {
                slot.push(fi);
            }
        }
    }
    (edges, bounds)
}

fn polyline_mid(pts: &[Vec3]) -> (Vec3, f64) {
    let len: f64 = pts.windows(2).map(|w| w.first().zip(w.get(1)).map(|(a, b)| a.dist(*b)).unwrap_or(0.0)).sum();
    let mut acc = 0.0;
    for w in pts.windows(2) {
        if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
            let l = a.dist(*b);
            if acc + l >= len * 0.5 && l > 0.0 {
                return (a.lerp(*b, (len * 0.5 - acc) / l), len);
            }
            acc += l;
        }
    }
    (pts.first().copied().unwrap_or_default(), len)
}

/// Deflection the orientation mesh uses: coarse enough for a large solid, fine enough that a
/// small one still tessellates. `unmeshed_faces` counts at the same value, so the count the
/// orientation mesh produced is reused instead of meshing the body again.
fn orientation_tolerance(span: f64) -> f64 {
    (span * 0.01).clamp(1e-3, 1.0)
}

/// Bounding-box diagonal of a solid's vertices (1 when it has no finite extent).
fn span_of(solid: &Solid) -> f64 {
    let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = -lo;
    for v in solid.vertex_iter() {
        let p = from_p3(v.point());
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    if lo.x.is_finite() { (hi - lo).len() } else { 1.0 }
}

/// Equal shared values: the same allocation (a clone of the body), else equal contents.
fn same_shared<T: PartialEq>(a: &Option<std::sync::Arc<T>>, b: &Option<std::sync::Arc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b) || a == b,
        (None, None) => true,
        _ => false,
    }
}

impl Body {
    /// The very same body (shared data), not just an equal-looking one. Its history (how an
    /// operation made it) is not part of it: an operation that leaves a body as it is (a cut by
    /// a tool clear of it) gives the same body, with that operation's history.
    pub fn same(&self, other: &Body) -> bool {
        std::sync::Arc::ptr_eq(&self.solid, &other.solid)
            && match (&self.mesh, &other.mesh) {
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && self.color == other.color
            && same_shared(&self.paint, &other.paint)
            && same_shared(&self.splits, &other.splits)
            && same_shared(&self.face_sources, &other.face_sources)
            && self.regularization_protected == other.regularization_protected
    }

    pub(crate) fn new(solid: Solid) -> Result<Body> {
        if solid.boundaries().is_empty() {
            return Err(KernelError::Failed("empty result".into()));
        }
        let mut solid = rephase_revolved(solid);
        // Orient outward: a closed solid must have positive volume (meshed finely enough that
        // every face comes out: a face missing from a coarse mesh can flip the sign).
        let span = span_of(&solid);
        let tol = orientation_tolerance(span);
        let orient = |tol: f64| {
            guard("orient", || {
                let shells = mesh_shells(&solid, tol);
                let volume = shells.iter().map(|s| s.to_polygon().volume()).sum::<f64>();
                let missing = shells
                    .iter()
                    .flat_map(|s| s.face_iter())
                    .filter(|f| f.surface().is_none_or(|m| m.faces().triangle_iter().next().is_none()))
                    .count();
                Ok((volume, missing))
            })
        };
        let (mut v, mut missing) = orient(tol)?;
        // A long thin round part (a through hole's tool: a cylinder whose vertices all lie on
        // its seam, so they span its length only) meshed that coarsely can come out with a
        // negative volume when it is outward already: check finely before turning it inside out.
        let fine = (tol * 0.02).max(1e-4);
        if v < 0.0 && fine < tol {
            (v, missing) = orient(fine)?;
        }
        if v < 0.0 {
            solid.not();
        }
        Ok(Body {
            solid: std::sync::Arc::new(solid),
            size: if span.is_finite() && span > 0.0 { span } else { 0.0 },
            volume: Some(std::sync::Arc::new(std::sync::Mutex::new(None))),
            coarse_missing: (v >= 0.0).then_some((tol, missing)),
            mesh: None,
            color: None,
            paint: None,
            splits: None,
            face_sources: None,
            regularization_protected: false,
            history: None,
            display_of: None,
        })
    }

    /// Like [`Body::new`], for a solid made by placing an already-oriented body (see
    /// [`crate::ops::transform_matrix`]): a placement keeps the faces pointing the way they did
    /// unless it mirrors them, so its orientation is read from the matrix rather than meshed
    /// again. Only for closed solids; `coarse` is the input's cached missing-face count, which a
    /// rigid placement preserves exactly.
    pub(crate) fn new_placed(solid: Solid, flip: bool, coarse: Option<(f64, usize)>) -> Result<Body> {
        if solid.boundaries().is_empty() {
            return Err(KernelError::Failed("empty result".into()));
        }
        let mut solid = rephase_revolved(solid);
        let span = span_of(&solid);
        if flip {
            solid.not();
        }
        Ok(Body {
            solid: std::sync::Arc::new(solid),
            size: if span.is_finite() && span > 0.0 { span } else { 0.0 },
            volume: Some(std::sync::Arc::new(std::sync::Mutex::new(None))),
            coarse_missing: coarse,
            mesh: None,
            color: None,
            paint: None,
            splits: None,
            face_sources: None,
            regularization_protected: false,
            history: None,
            display_of: None,
        })
    }

    pub(crate) fn inherit_regularization_protection(mut self, input: &Body) -> Self {
        self.regularization_protected |= input.regularization_protected;
        self
    }

    /// Exact history of the operation that made this body, relative to that operation's
    /// operands (see [`crate::history`]); `None` when the operation does not report one.
    pub fn history(&self) -> Option<&crate::history::History> {
        self.history.as_deref()
    }

    /// Attach the history of the operation that made this body from `operands`, after checking
    /// it describes this body.
    pub(crate) fn with_history(mut self, history: crate::history::History, operands: &[&Body]) -> Result<Self> {
        history.check(&self, operands)?;
        self.history = Some(std::sync::Arc::new(history));
        Ok(self)
    }

    /// [`Body::with_history`], given the operands' face and edge counts.
    pub(crate) fn with_history_counts(mut self, history: crate::history::History, operands: &[(usize, usize)]) -> Result<Self> {
        history.check_counts((self.face_count(), self.edge_count()), operands)?;
        self.history = Some(std::sync::Arc::new(history));
        Ok(self)
    }

    /// This body with `history` if it describes the body made from operands with these face
    /// and edge counts, else with none: an operation whose history could not be recorded still
    /// gives its result.
    pub(crate) fn with_checked_history(mut self, history: Option<crate::history::History>, operands: &[(usize, usize)]) -> Self {
        let counts = (self.face_count(), self.edge_count());
        self.history = history.and_then(|h| match h.check_counts(counts, operands) {
            Ok(()) => Some(std::sync::Arc::new(h)),
            Err(e) => {
                log::debug!("no history: {e}");
                None
            }
        });
        self
    }

    /// This body with no history: an operation whose history is not known.
    pub(crate) fn without_history(mut self) -> Self {
        self.history = None;
        self
    }

    pub fn face_sources(&self) -> Option<&[Option<FaceSource>]> {
        self.face_sources.as_deref().map(Vec::as_slice)
    }

    /// A current wall reconstructed from an exact contiguous section run.
    /// This transient witness grants no durable source/producer identity.
    pub fn face_was_coalesced(&self, index: usize) -> bool {
        matches!(self.face_sources().and_then(|s| s.get(index)), Some(Some(FaceSource::ExtrudeSide { coalesced: true, .. })))
    }

    /// An exact construction/import plane; never inferred from tessellation.
    pub fn face_plane(&self, index: usize) -> Option<solidcraft_geom::Plane> {
        if index >= 50_000 {
            return None;
        }
        let source = self.face_sources()?.get(index)?.as_ref()?;
        match source {
            FaceSource::ExtrudeSide { plane, .. } | FaceSource::CertifiedExtrudeSide { plane, .. } => *plane,
            FaceSource::ExtrudeCap { plane, .. }
            | FaceSource::CertifiedExtrudeCap { plane, .. }
            | FaceSource::Box { plane, .. }
            | FaceSource::CylinderCap { plane, .. }
            | FaceSource::ShellPlanar { plane, .. } => Some(*plane),
            FaceSource::Imported { .. } => self.planar_face(index),
        }
    }

    /// Exact current planar surface for an explicitly fixed snapshot, without source lineage.
    pub fn planar_face(&self, index: usize) -> Option<solidcraft_geom::Plane> {
        if index >= 50_000 {
            return None;
        }
        guard("exact planar face", || {
            let plane = self.solid.face_iter().nth(index).and_then(|face| match face.oriented_surface() {
                mt::Surface::Plane(plane) => {
                    let u = plane.u_axis();
                    let v = plane.v_axis();
                    solidcraft_geom::Plane::new(from_p3(plane.origin()), Vec3::new(u.x, u.y, u.z), Vec3::new(v.x, v.y, v.z))
                }
                _ => None,
            });
            Ok(plane)
        })
        .ok()
        .flatten()
    }

    /// Certify the two caps of a body built by [`crate::cylinder`]. Its wall stays uncertified.
    pub fn with_cylinder_certificate(mut self, producer: u64) -> Result<Self> {
        if producer == 0 {
            return Err(KernelError::Invalid("invalid cylinder producer".into()));
        }
        let Some(sources) = &self.face_sources else {
            return Err(KernelError::Failed("cylinder construction source".into()));
        };
        let mut sources = sources.as_ref().clone();
        let mut ends = [false; 2];
        for source in sources.iter_mut() {
            if let Some(FaceSource::ExtrudeCap { high, plane, .. }) = source {
                let end = ends.get_mut(usize::from(*high)).ok_or_else(|| KernelError::Failed("cylinder cap role".into()))?;
                if std::mem::replace(end, true) {
                    return Err(KernelError::Failed("duplicate cylinder cap role".into()));
                }
                *source = Some(FaceSource::CylinderCap { producer, high: *high, plane: *plane });
            }
        }
        if ends != [true; 2] {
            return Err(KernelError::Failed("cylinder cap role count".into()));
        }
        self.face_sources = Some(std::sync::Arc::new(sources));
        Ok(self)
    }

    pub fn with_box_certificate(mut self, producer: u64) -> Result<Self> {
        if producer == 0 {
            return Err(KernelError::Invalid("invalid box producer".into()));
        }
        if let Some(sources) = &self.face_sources {
            let mut sources = sources.as_ref().clone();
            for source in sources.iter_mut().flatten() {
                if let FaceSource::Box { producer: id, .. } = source {
                    *id = Some(producer);
                }
            }
            self.face_sources = Some(std::sync::Arc::new(sources));
        }
        Ok(self)
    }

    /// Cap signatures are in face order; unknown or ambiguous source curves stay uncertified.
    pub fn with_extrude_cap_certificates(mut self, producer: u64, sketch: u64, signatures: &[Option<CapSignature>]) -> Result<Self> {
        if producer == 0 || sketch == 0 {
            return Err(KernelError::Invalid("invalid cap producer".into()));
        }
        if let Some(sources) = &self.face_sources {
            if sources.len() != signatures.len() {
                return Err(KernelError::Invalid("cap certificate count mismatch".into()));
            }
            let mut sources = sources.as_ref().clone();
            for (source, signature) in sources.iter_mut().zip(signatures) {
                if let Some(FaceSource::ExtrudeCap { high, plane, .. }) = source {
                    let Some((outer, holes)) = signature else {
                        *source = None;
                        continue;
                    };
                    if outer.len() > 4096 || holes.len() > 4096 || holes.iter().any(|hole| hole.len() > 4096) {
                        return Err(KernelError::Invalid("oversized cap curve signature".into()));
                    }
                    let count = outer.len() + holes.iter().map(Vec::len).sum::<usize>();
                    if outer.is_empty()
                        || count > 4096
                        || holes.len() > 4096
                        || outer.iter().chain(holes.iter().flatten()).map(String::len).sum::<usize>() > 65_536
                        || outer.iter().chain(holes.iter().flatten()).any(|id| id.is_empty() || id.len() > 128)
                    {
                        return Err(KernelError::Invalid("invalid cap curve signature".into()));
                    }
                    let mut outer = outer.clone();
                    outer.sort();
                    outer.dedup();
                    let mut holes = holes.clone();
                    for hole in &mut holes {
                        if hole.is_empty() {
                            return Err(KernelError::Invalid("empty cap hole signature".into()));
                        }
                        hole.sort();
                        hole.dedup();
                    }
                    holes.sort();
                    if holes.windows(2).any(|pair| pair[0] == pair[1]) || holes.iter().flatten().any(|id| outer.binary_search(id).is_ok()) {
                        return Err(KernelError::Invalid("ambiguous cap hole signature".into()));
                    }
                    *source = Some(FaceSource::CertifiedExtrudeCap { producer, sketch, high: *high, outer, holes, plane: *plane });
                }
            }
            self.face_sources = Some(std::sync::Arc::new(sources));
        }
        Ok(self)
    }

    pub(crate) fn transformed_sources(&self, m: [[f64; 4]; 4]) -> Result<Option<FaceSources>> {
        let Some(sources) = &self.face_sources else { return Ok(None) };
        let vector = |p: Vec3| {
            Vec3::new(
                m[0][0] * p.x + m[1][0] * p.y + m[2][0] * p.z,
                m[0][1] * p.x + m[1][1] * p.y + m[2][1] * p.z,
                m[0][2] * p.x + m[1][2] * p.y + m[2][2] * p.z,
            )
        };
        let translation = Vec3::new(m[3][0], m[3][1], m[3][2]);
        let reflected = vector(Vec3::X).dot(vector(Vec3::Y).cross(vector(Vec3::Z))) < 0.;
        let mut sources = sources.as_ref().clone();
        for source in sources.iter_mut().flatten() {
            if let Some(plane) = source.plane_mut() {
                *plane = solidcraft_geom::Plane::new(
                    vector(plane.origin) + translation,
                    vector(plane.x),
                    vector(plane.y) * if reflected { -1. } else { 1. },
                )
                .ok_or_else(|| KernelError::Invalid("degenerate transformed face plane".into()))?;
            }
        }
        Ok(Some(std::sync::Arc::new(sources)))
    }

    /// Bind constructed edges to uniquely proven source curves without changing face names.
    pub fn with_extrude_certificates(mut self, producer: u64, sketch: u64, curves: &[Option<String>]) -> Result<Self> {
        if producer == 0 || sketch == 0 || curves.iter().flatten().any(|curve| curve.is_empty() || curve.len() > 128) {
            return Err(KernelError::Invalid("invalid constructed side certificate".into()));
        }
        if let Some(sources) = &self.face_sources {
            if sources.len() != curves.len() {
                return Err(KernelError::Invalid("side certificate count mismatch".into()));
            }
            let sources = sources
                .iter()
                .zip(curves)
                .map(|(source, curve)| match (source, curve) {
                    (Some(FaceSource::ExtrudeSide { plane, .. }), Some(curve)) => {
                        Some(FaceSource::CertifiedExtrudeSide { producer, sketch, curve: curve.clone(), plane: *plane })
                    }
                    (Some(FaceSource::ExtrudeSide { .. }), None) => None,
                    _ => source.clone(),
                })
                .collect();
            self.face_sources = Some(std::sync::Arc::new(sources));
        }
        Ok(self)
    }

    pub fn with_import_certificate(mut self, digest: &str, producer: u64) -> Result<Self> {
        if producer == 0 || digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
            return Err(KernelError::Invalid("invalid immutable import certificate".into()));
        }
        if let Some(sources) = &self.face_sources {
            let mut sources = sources.as_ref().clone();
            for source in sources.iter_mut().flatten() {
                if let FaceSource::Imported { payload_sha256, producer: origin, .. } = source {
                    *payload_sha256 = Some(digest.into());
                    *origin = Some(producer);
                }
            }
            self.face_sources = Some(std::sync::Arc::new(sources));
        }
        Ok(self)
    }

    pub(crate) fn with_face_sources(mut self, sources: Option<std::sync::Arc<Vec<Option<FaceSource>>>>) -> Self {
        self.face_sources = sources;
        self
    }

    /// A deep copy of the solid that can be mutated without affecting other bodies.
    pub(crate) fn deep_copy(&self) -> Solid {
        mt::builder::clone(&*self.solid)
    }

    /// Is this a mesh body (no B-rep)?
    pub fn is_mesh(&self) -> bool {
        self.mesh.is_some()
    }

    /// Error for operations that need a B-rep.
    pub(crate) fn require_brep(&self, what: &str) -> Result<()> {
        if self.is_mesh() {
            Err(KernelError::Invalid(format!("{what} needs a solid body; mesh bodies can only be moved, measured and exported")))
        } else {
            Ok(())
        }
    }

    /// Display colour from an imported file.
    pub fn color(&self) -> Option<[f32; 3]> {
        self.color
    }

    /// The same body with a display colour.
    pub fn with_color(mut self, c: Option<[f32; 3]>) -> Body {
        self.color = c.filter(|c| c.iter().all(|x| x.is_finite())).map(|c| c.map(|x| x.clamp(0.0, 1.0)));
        self
    }

    /// Lines faces were split along on purpose (Split Face), as polylines.
    pub fn split_lines(&self) -> Option<&std::sync::Arc<Vec<Vec<Vec3>>>> {
        self.splits.as_ref()
    }

    pub(crate) fn split_keep(&self) -> &[Vec<Vec3>] {
        self.splits.as_ref().map(|l| l.as_slice()).unwrap_or(&[])
    }

    /// The same body with the lines its faces were split along on purpose.
    /// This body shows `shared`'s mesh placed by `t` (it is that body's solid, placed by `t`).
    pub(crate) fn with_display_of(mut self, shared: std::sync::Arc<SharedDisplay>, t: [[f64; 4]; 4]) -> Body {
        if self.splits.is_none() && !self.is_mesh() {
            self.display_of = Some(std::sync::Arc::new((shared, t)));
        }
        self
    }

    pub fn with_split_lines(mut self, s: Option<std::sync::Arc<Vec<Vec<Vec3>>>>) -> Body {
        self.splits = s.filter(|l| !l.is_empty());
        // Split lines are drawn: the shared mesh does not have them.
        if self.splits.is_some() {
            self.display_of = None;
        }
        self.regularization_protected |= self.splits.is_some();
        self
    }

    /// Opacity and face colours for export, if any.
    pub fn paint(&self) -> Option<&Paint> {
        self.paint.as_deref()
    }

    /// The same body with opacity and face colours for export (`None` or opaque with no face
    /// colours clears them).
    pub fn with_paint(mut self, p: Option<Paint>) -> Body {
        let clean = |x: f32| if x.is_finite() { x.clamp(0.0, 1.0) } else { 1.0 };
        self.paint = p
            .map(|mut p| {
                p.opacity = clean(p.opacity);
                p.faces.retain(|f| f.color.iter().all(|x| x.is_finite()));
                for f in &mut p.faces {
                    f.color = f.color.map(|x| x.clamp(0.0, 1.0));
                    f.opacity = clean(f.opacity);
                }
                p
            })
            .filter(|p| p.opacity < 1.0 || !p.faces.is_empty())
            .map(std::sync::Arc::new);
        self.regularization_protected |= self.paint().is_some_and(|paint| !paint.faces.is_empty());
        self
    }

    /// Characteristic size (bounding box diagonal) for tolerances. [`Body::new`] measured it
    /// while it read the vertices anyway, so the answer is cached; a body built by hand carries
    /// 0 and is walked here once, and mesh bodies always measure their triangles.
    pub fn size(&self) -> f64 {
        if self.mesh.is_none() && self.size > 0.0 {
            return self.size;
        }
        let mut b = solidcraft_geom::Aabb3::EMPTY;
        if let Some(m) = &self.mesh {
            for p in &m.positions {
                b.add(*p);
            }
        }
        for v in self.solid.vertex_iter() {
            b.add(from_p3(v.point()));
        }
        let d = b.diagonal();
        if d.is_finite() && d > 0.0 { d } else { 1.0 }
    }

    /// The body's volume from a mesh at `tol`, measured once per (solid, tolerance): clones of
    /// a body share the measurement, so an operand a later boolean takes again is not meshed
    /// twice for the same number. `None` when the body cannot be meshed.
    pub fn volume_at(&self, tol: f64) -> Option<f64> {
        let key = if tol.is_finite() && tol > 0.0 { tol } else { 0.05 };
        let key = key.to_bits();
        if let Some(cell) = &self.volume
            && let Ok(cached) = cell.lock()
            && let Some((cached_tol, volume)) = *cached
            && cached_tol == key
        {
            return Some(volume);
        }
        let volume = self.tessellate(f64::from_bits(key)).ok()?.measure().volume;
        if let Some(cell) = &self.volume
            && let Ok(mut slot) = cell.lock()
        {
            *slot = Some((key, volume));
        }
        Some(volume)
    }

    /// Rough heap footprint, for the evaluation cache's byte budget: the B-rep topology is what
    /// makes a kept timeline snapshot expensive, so it dominates the estimate.
    pub fn retained_bytes(&self) -> usize {
        if let Some(m) = &self.mesh {
            return m.positions.len() * 24 + m.triangles.len() * 12;
        }
        // Counted on the borrowed boundaries: `Solid::edge_iter` and `vertex_iter` clone every
        // face's wires to walk them, which made this estimate (taken after every feature, over
        // every body) cost more than evaluating most features. Each edge use is one edge and
        // one vertex in those iterators, so the counts are the same.
        let (mut faces, mut uses) = (0usize, 0usize);
        for face in self.solid.face_iter() {
            faces += 1;
            uses += face.absolute_boundaries().iter().map(|w| w.len()).sum::<usize>();
        }
        faces * 2048 + uses * (512 + 96)
    }

    /// Distinct edges in a stable order (the order of `edges()` and `tessellate().edges`).
    pub(crate) fn unique_edges(solid: &Solid) -> Vec<mt::Edge> {
        let mut seen = std::collections::HashSet::new();
        solid.edge_iter().filter(|e| seen.insert(e.id())).collect()
    }

    /// Triangulate with chord tolerance `tol` (mm). Triangles carry their face index.
    pub fn tessellate(&self, tol: f64) -> Result<Mesh> {
        self.tessellate_with(tol, false)
    }

    /// Like [`Body::tessellate`]; with `refine`, triangles that cut through curved faces are
    /// split until within `tol` of the surface (for measuring).
    pub fn tessellate_with(&self, tol: f64, refine: bool) -> Result<Mesh> {
        self.tessellate_within(tol, f64::INFINITY, refine)
    }

    /// [`Body::tessellate_with`] in which no facet turns more than `angle` radians either
    /// (infinite: the chord tolerance alone).
    fn tessellate_within(&self, tol: f64, angle: f64, refine: bool) -> Result<Mesh> {
        crate::profile::timed(crate::profile::Kind::Tessellate, || self.tessellate_within_untimed(tol, angle, refine))
    }

    fn tessellate_within_untimed(&self, tol: f64, angle: f64, refine: bool) -> Result<Mesh> {
        let tol = if tol.is_finite() && tol > 0.0 { tol } else { 0.05 };
        let tol = tol.max(self.size() * 1e-6);
        if let Some(m) = &self.mesh {
            return Ok(m.to_mesh());
        }
        // A face trimmed by a helix (a modelled thread from another CAD system) turns more than
        // once around its cylinder; truck meshes it with triangles across the solid (#1289).
        // Such a body is meshed from its display plan, which cuts the face along a seam.
        if guard("tessellate", || Ok(crate::tess::seam::turns_more_than_once(&self.solid))).unwrap_or(false) {
            let turn = (angle.is_finite() && angle > 0.0)
                .then(|| crate::tess::size::Turn { angle: angle.max(MIN_DISPLAY_ANGLE), floor: tol * DISPLAY_ANGLE_FLOOR });
            match guard("tessellate", || crate::tess::plan(&self.solid, tol, turn)) {
                Ok((plan, _)) => return Ok(plan.evaluate()),
                Err(e) => log::debug!("helical faces: no display plan ({e}); meshing the reference way"),
            }
        }
        let deflection = if angle.is_finite() && angle > 0.0 {
            Deflection { chord: tol, angle: angle.max(MIN_DISPLAY_ANGLE), floor: tol * DISPLAY_ANGLE_FLOOR, rulings: true }
        } else {
            Deflection { rulings: true, ..Deflection::chord(tol) }
        };
        guard("tessellate", || {
            let meshed = mesh_shells_within(&self.solid, deflection);
            let mut out = Mesh::default();
            let solid_faces: Vec<&mt::Face> = self.solid.face_iter().collect();
            for (fi, face) in meshed.iter().flat_map(|s| s.face_iter()).enumerate() {
                let Some(pm) = face.surface() else { continue };
                let flip = !face.orientation();
                let base = u32::try_from(out.positions.len()).unwrap_or(u32::MAX);
                // Curved faces: split triangles that cut through the surface.
                let refinement = if refine {
                    solid_faces.get(fi).and_then(|sf| {
                        let surface = sf.surface();
                        if matches!(surface, mt::Surface::Plane(_)) { None } else { refined(&pm, &surface, tol) }
                    })
                } else {
                    None
                };
                if let Some(tris) = refinement {
                    let fid = u32::try_from(fi).unwrap_or(u32::MAX);
                    for t in tris {
                        let mut idx = [0u32; 3];
                        for (slot, (p, n)) in idx.iter_mut().zip(t) {
                            out.positions.push(p);
                            out.normals.push(if flip { -n } else { n });
                            *slot = u32::try_from(out.positions.len() - 1).unwrap_or(0);
                        }
                        out.triangles.push(if flip { [idx[0], idx[2], idx[1]] } else { idx });
                        out.tri_face.push(fid);
                    }
                    continue;
                }
                let pos = pm.positions();
                let nor = pm.normals();
                // Expand to per-corner vertices so normals stay per face (crisp edges).
                for tri in pm.faces().triangle_iter() {
                    let mut idx = [0u32; 3];
                    for (k, v) in tri.iter().enumerate() {
                        let Some(p) = pos.get(v.pos) else { continue };
                        let n = v.nor.and_then(|ni| nor.get(ni)).map(|n| Vec3::new(n.x, n.y, n.z)).unwrap_or(Vec3::Z);
                        let n = if flip { -n } else { n };
                        out.positions.push(Vec3::new(p.x, p.y, p.z));
                        out.normals.push(n);
                        if let Some(slot) = idx.get_mut(k) {
                            *slot = u32::try_from(out.positions.len() - 1).unwrap_or(0);
                        }
                    }
                    let t = if flip { [idx[0], idx[2], idx[1]] } else { idx };
                    out.triangles.push(t);
                    out.tri_face.push(u32::try_from(fi).unwrap_or(u32::MAX));
                }
                let _ = base;
            }
            // Fill missing normals from geometry.
            for t in &out.triangles {
                let (Some(a), Some(b), Some(c)) =
                    (out.positions.get(t[0] as usize), out.positions.get(t[1] as usize), out.positions.get(t[2] as usize))
                else {
                    continue;
                };
                let fnrm = (b - a).cross(c - a).normalized().unwrap_or(Vec3::Z);
                for k in t {
                    if let Some(n) = out.normals.get_mut(*k as usize)
                        && (n.dot(fnrm) < -0.2 || n.len() < 0.5)
                    {
                        *n = fnrm;
                    }
                }
            }
            let (edges, faces) = edge_order(meshed.iter().flat_map(|s| s.face_iter()));
            out.edges = edges.iter().map(|e| e.oriented_curve().0.iter().map(|p| Vec3::new(p.x, p.y, p.z)).collect()).collect();
            out.edge_faces = faces.into_iter().map(|fs| fs.into_iter().map(|f| u32::try_from(f).unwrap_or(u32::MAX)).collect()).collect();
            Ok(out)
        })
    }

    /// The display tessellation as a plan for the CPU or GPU evaluator: every face's
    /// triangulation in its own parameters, with each edge sampled once (chord tolerance
    /// `tol`, mm) so faces meet exactly.
    pub fn display_plan(&self, tol: f64) -> Result<solidcraft_geom::tess::TessPlan> {
        self.plan_within(tol, None)
    }

    /// Like [`Body::display_plan`], and no facet turns more than `angle` radians (see
    /// [`Body::display_mesh_angular`]).
    pub fn display_plan_angular(&self, tol: f64, angle: f64) -> Result<solidcraft_geom::tess::TessPlan> {
        self.plan_within(tol, Some(angle))
    }

    fn plan_within(&self, tol: f64, angle: Option<f64>) -> Result<solidcraft_geom::tess::TessPlan> {
        crate::profile::timed(crate::profile::Kind::Tessellate, || self.plan_within_untimed(tol, angle))
    }

    fn plan_within_untimed(&self, tol: f64, angle: Option<f64>) -> Result<solidcraft_geom::tess::TessPlan> {
        if self.is_mesh() {
            return Err(KernelError::Invalid("a mesh body has no surfaces to tessellate".into()));
        }
        let tol = if tol.is_finite() && tol > 0.0 { tol } else { 0.05 };
        let tol = tol.max(self.size() * 1e-6);
        // The same angle and floor as the reference mesher's angular display mesh.
        let turn =
            angle.filter(|a| a.is_finite()).map(|a| crate::tess::size::Turn { angle: a.max(MIN_DISPLAY_ANGLE), floor: tol * DISPLAY_ANGLE_FLOOR });
        guard("tessellate", || crate::tess::plan(&self.solid, tol, turn).map(|(plan, _)| plan))
    }

    /// Planar faces whose boundary crosses itself: two of its loops crossing each other, or a
    /// loop crossing itself. Such a face has no inside, and its mesh covers what it shouldn't
    /// and leaves openings beside it, while the shell is still closed and every face meshes
    /// (rounds that overlap where a wall is thinner than both, #871). Boundaries that only
    /// touch (at a vertex they share, or within the size's millionth) don't count.
    pub fn crossed_face_loops(&self) -> Result<usize> {
        Ok(self.crossed_faces()?.len())
    }

    /// The faces [`Body::crossed_face_loops`] counts, each as the mean of its boundary's
    /// points: the same for a face an operation carried over unchanged.
    pub fn crossed_faces(&self) -> Result<Vec<Vec3>> {
        use mt::{BoundedCurve, ParameterDivision1D};
        if self.is_mesh() {
            return Ok(Vec::new());
        }
        const MAX_SEGMENTS: usize = 20_000;
        let size = self.size();
        let tol = (size * 1e-6).max(1e-9);
        let chord = (size * 1e-3).max(1e-4);
        let mut crossed = Vec::new();
        for f in self.solid.face_iter() {
            let mt::Surface::Plane(pl) = f.oriented_surface() else { continue };
            let o = from_p3(pl.origin());
            let (Some(u), Some(n)) = (
                Vec3::new(pl.u_axis().x, pl.u_axis().y, pl.u_axis().z).normalized(),
                Vec3::new(pl.normal().x, pl.normal().y, pl.normal().z).normalized(),
            ) else {
                continue;
            };
            let v = n.cross(u);
            let flat = |p: mt::Point3| {
                let d = from_p3(p) - o;
                (d.dot(u), d.dot(v))
            };
            // Each loop's segments, in the face's plane.
            let mut loops: Vec<Vec<((f64, f64), (f64, f64))>> = Vec::new();
            let mut total = 0usize;
            for w in f.boundaries() {
                let mut segs = Vec::new();
                for e in w.edge_iter() {
                    let pts = guard("face loops", || {
                        Ok(match e.curve() {
                            mt::Curve::Line(_) => vec![e.front().point(), e.back().point()],
                            c => c.parameter_division(c.range_tuple(), chord).1,
                        })
                    })?;
                    segs.extend(pts.windows(2).filter_map(|q| Some((flat(*q.first()?), flat(*q.get(1)?)))));
                }
                total = total.saturating_add(segs.len());
                if total > MAX_SEGMENTS {
                    break;
                }
                loops.push(segs);
            }
            if total > MAX_SEGMENTS {
                continue;
            }
            // Proper crossings only: each segment's ends strictly on either side of the other.
            let side = |a: (f64, f64), b: (f64, f64), p: (f64, f64)| {
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len = (dx * dx + dy * dy).sqrt();
                if len <= tol { 0.0 } else { (dx * (p.1 - a.1) - dy * (p.0 - a.0)) / len }
            };
            let cross = |s: &((f64, f64), (f64, f64)), t: &((f64, f64), (f64, f64))| {
                let (d1, d2) = (side(s.0, s.1, t.0), side(s.0, s.1, t.1));
                let (d3, d4) = (side(t.0, t.1, s.0), side(t.0, t.1, s.1));
                ((d1 > tol && d2 < -tol) || (d1 < -tol && d2 > tol)) && ((d3 > tol && d4 < -tol) || (d3 < -tol && d4 > tol))
            };
            let all: Vec<(usize, &((f64, f64), (f64, f64)))> =
                loops.iter().enumerate().flat_map(|(l, segs)| segs.iter().map(move |s| (l, s))).collect();
            let bad = all.iter().enumerate().any(|(i, (_, s))| all.iter().skip(i + 1).any(|(_, t)| cross(s, t)));
            if bad {
                let ends: Vec<(f64, f64)> = all.iter().map(|(_, s)| s.0).collect();
                let k = ends.len().max(1) as f64;
                let (su, sv) = ends.iter().fold((0.0, 0.0), |acc, p| (acc.0 + p.0, acc.1 + p.1));
                crossed.push(o + u * (su / k) + v * (sv / k));
            }
        }
        Ok(crossed)
    }

    /// Faces with no triangles in a coarse tessellation (the mesher failed or gave up).
    pub fn unmeshed_faces(&self) -> usize {
        if self.is_mesh() {
            return 0;
        }
        let tol = (self.size() * 1e-2).max(1e-3);
        if let Some((cached_tol, missing)) = self.coarse_missing
            && cached_tol == tol
        {
            return missing;
        }
        let Ok(m) = self.tessellate(tol) else { return self.face_count() };
        let mut has = vec![false; self.face_count()];
        for f in &m.tri_face {
            if let Some(h) = has.get_mut(*f as usize) {
                *h = true;
            }
        }
        has.iter().filter(|h| !**h).count()
    }

    /// Display mesh: like [`Body::tessellate`], with seam edges flagged. Meshed from the
    /// body's [`Body::display_plan`] (shared edge nodes, structured grids), or the way
    /// [`Body::tessellate`] meshes when no plan can be made.
    pub fn display_mesh(&self, tol: f64) -> Result<Mesh> {
        crate::profile::timed(crate::profile::Kind::Tessellate, || self.display_mesh_untimed(tol))
    }

    fn display_mesh_untimed(&self, tol: f64) -> Result<Mesh> {
        if self.is_mesh() {
            return self.flagged(self.tessellate(tol)?);
        }
        let m = match self.display_plan(tol) {
            Ok(plan) => plan.evaluate(),
            Err(e) => {
                log::debug!("display mesh: {e}; meshing the reference way");
                self.tessellate(tol)?
            }
        };
        self.flagged(m)
    }

    /// Display mesh whose curved faces and edges look curved whatever the body's size: like
    /// [`Body::display_mesh`], and no facet turns more than `angle` radians (see
    /// [`DISPLAY_ANGLE`]). A chord tolerance that follows the body's size is larger than the
    /// sagitta of a small radius on a large body (a sheet-metal bend, a small hole), which the
    /// tolerance alone leaves as one flat facet.
    pub fn display_mesh_angular(&self, tol: f64, angle: f64) -> Result<Mesh> {
        crate::profile::timed(crate::profile::Kind::Tessellate, || self.display_mesh_angular_untimed(tol, angle))
    }

    fn display_mesh_angular_untimed(&self, tol: f64, angle: f64) -> Result<Mesh> {
        if self.is_mesh() {
            return self.flagged(self.tessellate(tol)?);
        }
        match self.display_plan_angular(tol, angle) {
            Ok(plan) => self.display_mesh_of(&plan),
            Err(e) => {
                log::debug!("display mesh: {e}; meshing the reference way");
                self.reference_display_mesh_angular(tol, angle)
            }
        }
    }

    /// The display mesh of a plan made from this body ([`Body::display_plan`],
    /// [`Body::display_plan_angular`]): evaluated on the CPU, with its edges flagged.
    pub fn display_mesh_of(&self, plan: &solidcraft_geom::tess::TessPlan) -> Result<Mesh> {
        crate::profile::timed(crate::profile::Kind::Tessellate, || self.display_mesh_of_untimed(plan))
    }

    fn display_mesh_of_untimed(&self, plan: &solidcraft_geom::tess::TessPlan) -> Result<Mesh> {
        self.flagged(plan.evaluate())
    }

    /// The reference mesher's angular display mesh: what [`Body::display_mesh_angular`] falls
    /// back to when no plan can be made.
    pub fn reference_display_mesh_angular(&self, tol: f64, angle: f64) -> Result<Mesh> {
        if let Some((shared, t)) = self.display_of.as_deref()
            && let Some(placed) =
                shared.mesh.get_or_init(|| shared.original.reference_display_mesh_angular(tol, angle).ok()).as_ref().and_then(|m| placed_mesh(m, t))
        {
            return Ok(placed);
        }
        if self.is_mesh() {
            return self.flagged(self.tessellate(tol)?);
        }
        self.flagged(self.tessellate_within(tol, angle, false)?)
    }

    /// A tessellation with its edges flagged for display (seams, tangent joins, protected creases).
    fn flagged(&self, mut m: Mesh) -> Result<Mesh> {
        if self.is_mesh() {
            return Ok(m);
        }
        m.seams = crate::topo::seam_flags(self, &m);
        (m.smooth_edges, m.sharp_edges) = crate::smooth_edges::flags(self, &m);
        Ok(m)
    }

    /// Edges with polylines (index = position in [`Body::tessellate`]'s `edges`).
    pub fn edges(&self, tol: f64) -> Result<Vec<EdgeInfo>> {
        let m = self.tessellate(tol)?;
        Ok(m.edges
            .into_iter()
            .enumerate()
            .map(|(index, points)| {
                let (mid, length) = polyline_mid(&points);
                EdgeInfo { index, points, mid, length }
            })
            .collect())
    }

    /// Faces with area and centroid.
    pub fn faces(&self, tol: f64) -> Result<Vec<FaceInfo>> {
        let m = self.tessellate(tol)?;
        let nf = self.face_count();
        let mut acc: Vec<(f64, Vec3, Vec3, bool)> = vec![(0.0, Vec3::ZERO, Vec3::ZERO, true); nf];
        for (t, f) in m.triangles.iter().zip(&m.tri_face) {
            let Some([a, b, c]) = m.tri(t) else { continue };
            let cr = (b - a).cross(c - a);
            let ar = cr.len() * 0.5;
            if let Some(slot) = acc.get_mut(*f as usize) {
                slot.0 += ar;
                slot.1 += (a + b + c) * (ar / 3.0);
                if let Some(n) = cr.normalized() {
                    if slot.2 == Vec3::ZERO {
                        slot.2 = n;
                    } else if slot.2.dot(n) < 1.0 - 1e-6 {
                        slot.3 = false;
                    }
                }
            }
        }
        Ok(acc
            .into_iter()
            .enumerate()
            .map(|(index, (area, c, n, planar))| FaceInfo {
                index,
                area,
                centroid: if area > 0.0 { c / area } else { Vec3::ZERO },
                plane_normal: (planar && n != Vec3::ZERO).then_some(n),
            })
            .collect())
    }

    /// Index of the edge closest to `p` (by polyline distance).
    pub fn nearest_edge(&self, p: Vec3, tol: f64) -> Result<Option<(usize, f64)>> {
        let edges = self.edges(tol)?;
        let mut best: Option<(usize, f64)> = None;
        for e in &edges {
            for w in e.points.windows(2) {
                if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
                    let d = p.dist_to_segment(*a, *b);
                    if best.is_none_or(|(_, bd)| d < bd) {
                        best = Some((e.index, d));
                    }
                }
            }
        }
        Ok(best)
    }

    /// Is the point inside the solid?
    pub fn contains(&self, p: Vec3) -> bool {
        self.tessellate(self.size() * 2e-3).map(|m| m.contains(p)).unwrap_or(false)
    }

    /// B-rep faces (a mesh body is one face; [`crate::measure`] counts its triangles).
    pub fn face_count(&self) -> usize {
        match &self.mesh {
            Some(_) => 1,
            None => self.solid.face_iter().count(),
        }
    }
    /// B-rep edges (none for a mesh body).
    pub fn edge_count(&self) -> usize {
        match &self.mesh {
            Some(_) => 0,
            None => Self::unique_edges(&self.solid).len(),
        }
    }
    /// B-rep vertices (none for a mesh body).
    pub fn vertex_count(&self) -> usize {
        if self.mesh.is_some() {
            return 0;
        }
        let mut seen = std::collections::HashSet::new();
        self.solid.vertex_iter().filter(|v| seen.insert(v.id())).count()
    }
    /// What is wrong with the body as a solid: shells that aren't closed and consistently
    /// oriented, faces that don't mesh, a volume that isn't positive. Empty when it is sound.
    pub fn validity(&self) -> Vec<String> {
        if let Some(m) = &self.mesh {
            return if crate::meshbody::is_closed(m) { Vec::new() } else { vec!["the mesh is not closed".into()] };
        }
        let mut out = Vec::new();
        for (i, sh) in self.solid.boundaries().iter().enumerate() {
            let c = sh.shell_condition();
            if c != truck_topology::shell::ShellCondition::Closed {
                out.push(format!("shell {i} is {c:?}, not closed"));
            }
        }
        let missing = self.unmeshed_faces();
        if missing > 0 {
            out.push(format!("{missing} face(s) don't mesh"));
        }
        match crate::measure(self) {
            Ok(m) if m.volume.is_finite() && m.volume > 0.0 => {}
            Ok(m) => out.push(format!("volume {}", m.volume)),
            Err(e) => out.push(format!("no measure: {e}")),
        }
        out
    }

    pub fn shell_count(&self) -> usize {
        match &self.mesh {
            Some(_) => 1,
            None => self.solid.boundaries().len(),
        }
    }
}

impl Body {
    /// The separate pieces of a body: each outer shell with the voids inside it. A body in one
    /// piece comes back as itself. Pieces of a body in several carry their history over
    /// `[self]` (their faces and edges are its own), so whoever names them can follow it.
    pub fn lumps(&self) -> Result<Vec<Body>> {
        let pieces = self.lumps_with_clearance(0.0)?;
        if pieces.len() < 2 {
            return Ok(pieces);
        }
        let counts = [(self.face_count(), self.edge_count())];
        let histories = crate::boolean_record::parts_of(&pieces, self);
        Ok(pieces.into_iter().zip(histories).map(|(piece, history)| piece.with_checked_history(history, &counts)).collect())
    }

    /// Whether this body and `other` share material or touch (within a fine mesh tolerance).
    pub fn meets(&self, other: &Body) -> Result<bool> {
        self.require_brep("contact test")?;
        other.require_brep("contact test")?;
        let tol = (self.size().max(other.size()) * 1e-4).max(1e-6);
        let (a, b) = (self.tessellate(tol)?, other.tessellate(tol)?);
        shell_groups::meshes_meet(&a, &b, 2.0 * tol)
    }

    /// A boolean shortcut needs clearance larger than both shell mesh deflections.
    pub(crate) fn separated_lumps(&self) -> Result<Vec<Body>> {
        self.lumps_with_clearance(4.0 * (self.size() * 1e-3).max(1e-3))
    }

    /// A union's pieces must be clear of each other by the contact distance of [`Body::meets`];
    /// closer shells are touching material that was never fused (#396).
    pub(crate) fn union_lumps(&self) -> Result<Vec<Body>> {
        self.lumps_with_clearance(2.0 * (self.size() * 1e-4).max(1e-6))
    }

    fn lumps_with_clearance(&self, clearance: f64) -> Result<Vec<Body>> {
        if self.mesh.is_some() || self.solid.boundaries().len() < 2 {
            return Ok(vec![self.clone()]);
        }
        let tol = (self.size() * 1e-3).max(1e-3);
        guard("lumps", || {
            let shells: Vec<mt::Shell> = self.solid.boundaries().clone();
            if shells.len() > shell_groups::MAX_SHELLS
                || shells.iter().flat_map(mt::Shell::vertex_iter).take(shell_groups::MAX_POINTS + 1).count() > shell_groups::MAX_POINTS
                || shells.iter().flat_map(mt::Shell::face_iter).take(shell_groups::MAX_TRIANGLES + 1).count() > shell_groups::MAX_TRIANGLES
            {
                return Err(KernelError::Failed("lumps: topology exceeds the shell grouping limit".into()));
            }
            let (mut point_count, mut triangle_count) = (0usize, 0usize);
            let data: Vec<(f64, Mesh)> = shells
                .iter()
                .map(|sh| {
                    let meshed = sh.robust_triangulation(tol);
                    if meshed.face_iter().any(|f| f.surface().is_none_or(|m| m.faces().triangle_iter().next().is_none())) {
                        return Err(KernelError::Failed("lumps: cannot mesh a complete shell boundary".into()));
                    }
                    let pm = meshed.to_polygon();
                    point_count = point_count
                        .checked_add(pm.positions().len())
                        .ok_or_else(|| KernelError::Failed("lumps: mesh exceeds the shell grouping limit".into()))?;
                    triangle_count = triangle_count
                        .checked_add(pm.faces().triangle_iter().take(shell_groups::MAX_TRIANGLES + 1).count())
                        .ok_or_else(|| KernelError::Failed("lumps: mesh exceeds the shell grouping limit".into()))?;
                    if point_count > shell_groups::MAX_POINTS || triangle_count > shell_groups::MAX_TRIANGLES {
                        return Err(KernelError::Failed("lumps: mesh exceeds the shell grouping limit".into()));
                    }
                    let mut m = Mesh::default();
                    for p in pm.positions() {
                        m.positions.push(Vec3::new(p.x, p.y, p.z));
                    }
                    for t in pm.faces().triangle_iter() {
                        let idx = |k: usize| {
                            t.get(k).and_then(|x| u32::try_from(x.pos).ok()).ok_or_else(|| KernelError::Failed("lumps: invalid mesh index".into()))
                        };
                        let indices = [idx(0)?, idx(1)?, idx(2)?];
                        let [a, b, c] = m.tri(&indices).ok_or_else(|| KernelError::Failed("lumps: invalid mesh triangle".into()))?;
                        // STEP sphere poles can produce zero-area duplicate triangles.
                        // They have no boundary area or volume and are not ray crossings.
                        if (b - a).cross(c - a) == Vec3::ZERO {
                            continue;
                        }
                        m.triangles.push(indices);
                        m.tri_face.push(0);
                    }
                    let origin = m.positions.first().copied().ok_or_else(|| KernelError::Failed("lumps: empty mesh".into()))?;
                    let v = m
                        .triangles
                        .iter()
                        .map(|t| {
                            m.tri(t)
                                .map(|[a, b, c]| (a - origin).dot((b - origin).cross(c - origin)) / 6.0)
                                .ok_or_else(|| KernelError::Failed("lumps: invalid mesh triangle".into()))
                        })
                        .collect::<Result<Vec<_>>>()?
                        .into_iter()
                        .sum();
                    Ok((v, m))
                })
                .collect::<Result<_>>()?;
            let (volumes, meshes): (Vec<_>, Vec<_>) = data.into_iter().unzip();
            let groups = shell_groups::groups(&meshes, &volumes, (self.size() * 1e-8).max(1e-9).max(clearance))?;
            if groups.len() == 1 {
                return Ok(vec![self.clone()]);
            }
            // Grouping keeps existing shells; certificates belong to their
            // exact face identities, not the new lump's positional face index.
            // This is not propagation through a geometry-changing operation.
            let sources_by_face = if let Some(sources) = &self.face_sources {
                if sources.len() != self.solid.face_iter().count() {
                    return Err(KernelError::Invalid("lumps: face source table does not match the body".into()));
                }
                let mut by_id = std::collections::HashMap::new();
                for (face, source) in self.solid.face_iter().zip(sources.iter()) {
                    if by_id.insert(face.id(), Some((face.orientation(), source))).is_some() {
                        // A repeated identity has no unique source association.
                        by_id.insert(face.id(), None);
                    }
                }
                Some(by_id)
            } else {
                None
            };
            groups
                .into_iter()
                .map(|g| {
                    let shells = g
                        .into_iter()
                        .map(|i| shells.get(i).cloned().ok_or_else(|| KernelError::Failed("lumps: missing shell".into())))
                        .collect::<Result<Vec<_>>>()?;
                    let result = Solid::try_new(shells).map_err(|e| KernelError::Failed(format!("lump: {e}"))).and_then(Body::new)?;
                    let sources = sources_by_face.as_ref().map(|old| {
                        result
                            .solid
                            .face_iter()
                            .map(|face| match old.get(&face.id()) {
                                Some(Some((orientation, source))) if *orientation == face.orientation() => (*source).clone(),
                                _ => None,
                            })
                            .collect()
                    });
                    Ok(result.with_face_sources(sources.map(std::sync::Arc::new)).inherit_regularization_protection(self))
                })
                .collect()
        })
    }
}

#[cfg(test)]
#[path = "body_lump_tests.rs"]
mod lump_tests;

#[cfg(test)]
#[path = "refine_cache_tests.rs"]
mod refine_cache_tests;

#[cfg(test)]
#[path = "display_mesh_tests.rs"]
mod display_mesh_tests;

#[cfg(test)]
#[path = "regularization_protection_tests.rs"]
mod regularization_protection_tests;
