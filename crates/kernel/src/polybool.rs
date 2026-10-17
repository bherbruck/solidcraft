//! Exact-plane booleans for bodies whose faces are all planar (BSP-tree CSG on polygons, in
//! the classic style), used when the B-rep boolean fails — typically on coincident faces, such
//! as a wall flush with the side of its base. The polygons are rebuilt into a B-rep: coplanar
//! pieces are merged into faces, T-junctions are split so neighbouring faces share edges.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use solidcraft_geom::{Mesh, Vec3};
use truck_modeling::{self as mt, builder};

use crate::body::{Body, Solid, edge_order, from_p3, p3};
use crate::boolean_record::{self as recorded, Traced};
use crate::heal::find;
use crate::history::{EdgeOrigin, FaceOrigin, History};
use crate::{BoolOp, KernelError, Result, guard, record};

const MAX_DEPTH: usize = 4000;
const MAX_POLYS: usize = 200_000;

#[derive(Clone, Copy, Debug)]
struct PlaneEq {
    n: Vec3,
    w: f64,
}

impl PlaneEq {
    fn flipped(self) -> PlaneEq {
        PlaneEq { n: -self.n, w: -self.w }
    }
}

#[derive(Clone, Debug)]
struct Poly {
    v: Vec<Vec3>,
    plane: PlaneEq,
    /// The operand face this is (a piece of).
    src: FaceOrigin,
}

impl Poly {
    fn flip(&mut self) {
        self.v.reverse();
        self.plane = self.plane.flipped();
    }
}

const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

/// Split `p` by `pl` into the four buckets.
fn split(pl: PlaneEq, p: &Poly, eps: f64, cf: &mut Vec<Poly>, cb: &mut Vec<Poly>, f: &mut Vec<Poly>, b: &mut Vec<Poly>) {
    let mut ptype = 0u8;
    let types: Vec<u8> =
        p.v.iter()
            .map(|v| {
                let t = pl.n.dot(*v) - pl.w;
                let ty = if t < -eps {
                    BACK
                } else if t > eps {
                    FRONT
                } else {
                    COPLANAR
                };
                ptype |= ty;
                ty
            })
            .collect();
    match ptype {
        COPLANAR => {
            if pl.n.dot(p.plane.n) > 0.0 {
                cf.push(p.clone());
            } else {
                cb.push(p.clone());
            }
        }
        FRONT => f.push(p.clone()),
        BACK => b.push(p.clone()),
        _ => {
            let (mut fv, mut bv) = (Vec::new(), Vec::new());
            let n = p.v.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let (Some(&ti), Some(&tj), Some(&vi), Some(&vj)) = (types.get(i), types.get(j), p.v.get(i), p.v.get(j)) else { continue };
                if ti != BACK {
                    fv.push(vi);
                }
                if ti != FRONT {
                    bv.push(vi);
                }
                if (ti | tj) == SPANNING {
                    let den = pl.n.dot(vj - vi);
                    if den.abs() > 1e-300 {
                        let t = (pl.w - pl.n.dot(vi)) / den;
                        let x = vi.lerp(vj, t);
                        fv.push(x);
                        bv.push(x);
                    }
                }
            }
            if fv.len() >= 3 {
                f.push(Poly { v: fv, plane: p.plane, src: p.src });
            }
            if bv.len() >= 3 {
                b.push(Poly { v: bv, plane: p.plane, src: p.src });
            }
        }
    }
}

#[derive(Default)]
struct Node {
    plane: Option<PlaneEq>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polys: Vec<Poly>,
}

impl Node {
    fn build(&mut self, polys: Vec<Poly>, eps: f64, depth: usize) -> Result<()> {
        if polys.is_empty() {
            return Ok(());
        }
        if depth > MAX_DEPTH {
            return Err(KernelError::Failed("planar boolean: tree too deep".into()));
        }
        let pl = match self.plane {
            Some(p) => p,
            None => {
                let p = polys.first().map(|p| p.plane).ok_or_else(|| KernelError::Failed("bsp".into()))?;
                self.plane = Some(p);
                p
            }
        };
        let (mut f, mut b) = (Vec::new(), Vec::new());
        let mut cf = Vec::new();
        let mut cb = Vec::new();
        for p in &polys {
            split(pl, p, eps, &mut cf, &mut cb, &mut f, &mut b);
        }
        self.polys.extend(cf);
        self.polys.extend(cb);
        if !f.is_empty() {
            self.front.get_or_insert_with(Default::default).build(f, eps, depth + 1)?;
        }
        if !b.is_empty() {
            self.back.get_or_insert_with(Default::default).build(b, eps, depth + 1)?;
        }
        Ok(())
    }
    fn invert(&mut self) {
        for p in &mut self.polys {
            p.flip();
        }
        self.plane = self.plane.map(PlaneEq::flipped);
        if let Some(f) = &mut self.front {
            f.invert();
        }
        if let Some(b) = &mut self.back {
            b.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }
    fn clip_polys(&self, polys: Vec<Poly>, eps: f64) -> Vec<Poly> {
        let Some(pl) = self.plane else { return polys };
        let (mut f, mut b) = (Vec::new(), Vec::new());
        let (mut cf, mut cb) = (Vec::new(), Vec::new());
        for p in &polys {
            split(pl, p, eps, &mut cf, &mut cb, &mut f, &mut b);
        }
        f.extend(cf);
        b.extend(cb);
        let f = match &self.front {
            Some(n) => n.clip_polys(f, eps),
            None => f,
        };
        let b = match &self.back {
            Some(n) => n.clip_polys(b, eps),
            None => Vec::new(),
        };
        let mut out = f;
        out.extend(b);
        out
    }
    fn clip_to(&mut self, other: &Node, eps: f64) {
        let polys = std::mem::take(&mut self.polys);
        self.polys = other.clip_polys(polys, eps);
        if let Some(f) = &mut self.front {
            f.clip_to(other, eps);
        }
        if let Some(b) = &mut self.back {
            b.clip_to(other, eps);
        }
    }
    fn all(&self, out: &mut Vec<Poly>) {
        out.extend(self.polys.iter().cloned());
        if let Some(f) = &self.front {
            f.all(out);
        }
        if let Some(b) = &self.back {
            b.all(out);
        }
    }
}

/// Why a body's polygons can't be had: in words that finish "the {role} has …" or "the
/// {role}'s …".
#[derive(Debug, PartialEq)]
enum NotPlanar {
    /// Faces on curved surfaces, counted by kind.
    Curved(std::collections::BTreeMap<&'static str, usize>),
    /// Something else (a face that could not be meshed, too many polygons).
    Other(String),
}

impl NotPlanar {
    fn describe(&self, role: &str) -> String {
        match self {
            NotPlanar::Curved(kinds) => {
                let n: usize = kinds.values().sum();
                let list: Vec<&str> = kinds.keys().copied().collect();
                format!("the {role} has {n} curved face{} ({})", if n == 1 { "" } else { "s" }, list.join(", "))
            }
            NotPlanar::Other(m) => format!("the {role}'s {m}"),
        }
    }
}

/// The kind of curved surface `face` lies on, or None when it lies on a plane. `flat`: whether
/// its mesh is flat. The meshes here are as coarse as a thousandth of the body, which spans a
/// round much smaller than that (a 2 mm corner of a 1.2 m channel) with one flat strip of
/// facets: the surface decides, and the mesh only for a surface none of these kinds fits.
fn curved_kind(face: &mt::Face, flat: bool, tol: f64) -> Option<&'static str> {
    use crate::offset::{Surf, surf_of};
    if matches!(face.surface(), mt::Surface::Plane(_)) {
        return None;
    }
    match surf_of(face, tol) {
        Some(Surf::Plane { .. }) => None,
        Some(Surf::Cylinder { .. }) => Some("cylinder"),
        Some(Surf::Cone { .. }) => Some("cone"),
        Some(Surf::Sphere { .. }) => Some("sphere"),
        Some(Surf::Torus { .. }) => Some("torus"),
        None => (!flat).then_some("freeform"),
    }
}

/// The tolerance [`curved_kind`] tells surfaces apart by: face_surface's, with the body's
/// bounds computed once. Walking to each indexed face separately made curved-body diagnostics
/// quadratic.
fn kind_tol(b: &Body) -> f64 {
    (b.size() * 1e-7).max(1e-9) * 100.0
}

/// The plane of each face (from its mesh, which must be flat), or what stands in the way: the
/// curved faces by the kind of surface they lie on.
fn face_planes(b: &Body, faces: &[crate::FaceInfo]) -> std::result::Result<Vec<PlaneEq>, NotPlanar> {
    let surface_tol = kind_tol(b);
    let mut surfaces = b.solid.face_iter();
    let mut curved: std::collections::BTreeMap<&'static str, usize> = std::collections::BTreeMap::new();
    let mut out = Vec::with_capacity(faces.len());
    for (index, f) in faces.iter().enumerate() {
        let Some(surface) = surfaces.next() else {
            return Err(NotPlanar::Other("mesh is inconsistent".into()));
        };
        if f.index != index {
            return Err(NotPlanar::Other("mesh is inconsistent".into()));
        }
        if f.area <= 0.0 {
            return Err(NotPlanar::Other(format!("face {} could not be meshed", f.index)));
        }
        match (f.plane_normal, curved_kind(surface, f.plane_normal.is_some(), surface_tol)) {
            (Some(n), None) => out.push(PlaneEq { n, w: n.dot(f.centroid) }),
            (None, None) => return Err(NotPlanar::Other(format!("face {} is planar but its mesh is not flat", f.index))),
            (_, Some(kind)) => *curved.entry(kind).or_insert(0) += 1,
        }
    }
    if surfaces.next().is_some() {
        return Err(NotPlanar::Other("mesh is inconsistent".into()));
    }
    if curved.is_empty() { Ok(out) } else { Err(NotPlanar::Curved(curved)) }
}

/// Polygons of a body whose faces are all planar, as pieces of its faces as operand `operand`.
fn planar_polys(b: &Body, operand: usize) -> std::result::Result<Vec<Poly>, NotPlanar> {
    let tol = (b.size() * 1e-3).max(1e-3);
    let unmeshed = |e: KernelError| NotPlanar::Other(format!("faces could not be meshed ({e})"));
    let faces = b.faces(tol).map_err(unmeshed)?;
    let mesh = b.tessellate(tol).map_err(unmeshed)?;
    let planes = face_planes(b, &faces)?;
    if mesh.triangles.len() > MAX_POLYS {
        return Err(NotPlanar::Other(format!("mesh is too detailed ({} triangles)", mesh.triangles.len())));
    }
    let bad_mesh = || NotPlanar::Other("mesh is inconsistent".into());
    let mut out = Vec::new();
    for (t, fi) in mesh.triangles.iter().zip(&mesh.tri_face) {
        let [a, bb, c] = mesh.tri(t).ok_or_else(bad_mesh)?;
        let plane = *planes.get(*fi as usize).ok_or_else(bad_mesh)?;
        // Snap the vertices onto the exact face plane.
        let snap = |p: Vec3| p - plane.n * (plane.n.dot(p) - plane.w);
        let mut v = vec![snap(a), snap(bb), snap(c)];
        if (v[1] - v[0]).cross(v[2] - v[0]).dot(plane.n) < 0.0 {
            v.reverse();
        }
        out.push(Poly { v, plane, src: FaceOrigin::Face { operand, face: *fi as usize } });
    }
    Ok(out)
}

/// Polygons of any body: planar faces exactly, curved ones as their triangles (within `tol`);
/// pieces of its faces as operand `operand`.
fn faceted_polys(b: &Body, tol: f64, operand: usize) -> Option<Vec<Poly>> {
    let faces = b.faces(tol).ok()?;
    let mesh = b.tessellate(tol).ok()?;
    let planes: Vec<Option<PlaneEq>> = faces.iter().map(|f| f.plane_normal.map(|n| PlaneEq { n, w: n.dot(f.centroid) })).collect();
    let mut out = Vec::new();
    for (t, fi) in mesh.triangles.iter().zip(&mesh.tri_face) {
        let [a, bb, c] = mesh.tri(t)?;
        let plane = match planes.get(*fi as usize)? {
            Some(pl) => *pl,
            None => {
                let Some(n) = (bb - a).cross(c - a).normalized() else { continue };
                PlaneEq { n, w: n.dot(a) }
            }
        };
        let snap = |p: Vec3| p - plane.n * (plane.n.dot(p) - plane.w);
        let mut v = vec![snap(a), snap(bb), snap(c)];
        if (v[1] - v[0]).cross(v[2] - v[0]).dot(plane.n) < 0.0 {
            v.reverse();
        }
        out.push(Poly { v, plane, src: FaceOrigin::Face { operand, face: *fi as usize } });
    }
    (out.len() <= MAX_POLYS).then_some(out)
}

/// Boolean of a body, its curved faces faceted to `tol`, with a closed triangle mesh (wound
/// outward); the result has planar faces only. For tools the B-rep boolean cannot take, such as
/// the helical groove of a thread.
///
/// The result's history is over `[a, b]`, the mesh's faces being its triangles (and it having
/// no edges): a face of the result lies on faces of `a` (a facet of a curved one) or on
/// triangles of `b`; an edge between facets of one face of `a`, or between triangles of `b`,
/// has no origin.
pub fn faceted_boolean(a: &Body, b: &Mesh, op: BoolOp, tol: f64) -> Result<Option<Body>> {
    a.require_brep("a boolean")?;
    if !(tol.is_finite() && tol > 0.0) {
        return Err(KernelError::Invalid("facet tolerance".into()));
    }
    let Some(pa) = faceted_polys(a, tol, 0) else {
        return Err(KernelError::Failed("the body is too detailed to facet".into()));
    };
    let mut pb = Vec::with_capacity(b.triangles.len());
    for (face, t) in b.triangles.iter().enumerate() {
        let Some([p, q, r]) = b.tri(t) else { continue };
        let Some(n) = (q - p).cross(r - p).normalized() else { continue };
        pb.push(Poly { v: vec![p, q, r], plane: PlaneEq { n, w: n.dot(p) }, src: FaceOrigin::Face { operand: 1, face } });
    }
    let edges = OperandEdges::of(a);
    let sources = Sources { counts: vec![(a.face_count(), edges.ends.len()), (b.triangles.len(), 0)], edges: vec![Some(edges), None] };
    if pa.len() + pb.len() > MAX_POLYS {
        return Err(KernelError::Failed("the bodies are too detailed to facet".into()));
    }
    let (lo, hi) = b
        .positions
        .iter()
        .fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(l, h), p| (l.min(*p), h.max(*p)));
    let size = if b.positions.is_empty() { a.size() } else { a.size().max((hi - lo).len()) };
    // Fine facets of curved faces meet at shallow angles, where the coplanar test decides
    // whether the pieces' boundaries come out consistent: try a few tolerances, and take the
    // first whose result is watertight and agrees with the complementary boolean (A − B and
    // A ∩ B add up to A; A ∪ B is A plus B less A ∩ B).
    // The BSP takes each list's first polygon as its splitting plane. Facets in sweep order
    // (a thread's groove, turn after turn) make every splitter cut nothing, so the tree is as
    // deep as the list is long: quadratic time and memory, and a long thread ran out of depth
    // (#1250). The same lists in a fixed shuffled order give a shallow tree.
    let (pa, pb) = (shuffled(pa), shuffled(pb));
    let (va, vb) = (polys_volume(&pa), polys_volume(&pb));
    let mut last = KernelError::Failed("faceted boolean".into());
    for rel in [1e-8, 1e-7, 3e-8, 1e-9, 3e-7] {
        let eps = size * rel;
        let out = csg(pa.clone(), pb.clone(), op, eps)?;
        let both = csg(pa.clone(), pb.clone(), BoolOp::Intersect, eps)?;
        let (vo, vi) = (polys_volume(&out), polys_volume(&both));
        let expect = match op {
            BoolOp::Cut => va - vi,
            BoolOp::Union => va + vb - vi,
            BoolOp::Intersect => vi,
        };
        if (vo - expect).abs() > 1e-6 * (va.abs() + vb.abs()) || vi < -1e-9 * (va.abs() + vb.abs()) {
            last = KernelError::Failed(format!("faceted boolean: inconsistent volumes ({vo} vs {expect})"));
            continue;
        }
        if out.is_empty() {
            return Ok(None);
        }
        match rebuild(out, size, &sources) {
            Ok(b) => return Ok(Some(b)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// `polys` in a fixed pseudo-random order (the same every run, so results are reproducible).
fn shuffled(mut polys: Vec<Poly>) -> Vec<Poly> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ polys.len() as u64;
    let mut next = || {
        // SplitMix64.
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..polys.len()).rev() {
        polys.swap(i, (next() % (i as u64 + 1)) as usize);
    }
    polys
}

/// Boolean of two all-planar bodies.
pub fn planar_boolean(a: &Body, b: &Body, op: BoolOp) -> Result<Option<Body>> {
    a.require_brep("a boolean")?;
    b.require_brep("a boolean")?;
    let (pa, pb) = match (planar_polys(a, 0), planar_polys(b, 1)) {
        (Ok(pa), Ok(pb)) => (pa, pb),
        (ra, rb) => {
            if let Some(r) = sliced_planar_boolean(a, b, op) {
                return r;
            }
            let why: Vec<String> =
                [(ra.err(), "target"), (rb.err(), "tool")].into_iter().filter_map(|(e, role)| e.map(|e| e.describe(role))).collect();
            return Err(KernelError::Failed(format!(
                "not supported yet: the exact fallback for where the bodies meet (faces that touch, or cross at a shallow angle) takes planar faces only, and {}",
                why.join(" and ")
            )));
        }
    };
    let size = a.size().max(b.size());
    polys_boolean(pa, pb, size, op, size * 1e-9, &Sources::of(&[a, b]))
}

/// [`planar_boolean`] of a body with curved faces and an all-planar one, when a slab between
/// two parallel planes holds `b` and none of `a`'s curved faces (a pocket cut beside a round
/// hole): `a` is split across the slab, the middle piece takes the exact boolean, and the
/// pieces outside are joined back on. `None` when no such slab exists.
pub(crate) fn sliced_planar_boolean(a: &Body, b: &Body, op: BoolOp) -> Option<Result<Option<Body>>> {
    let pb = planar_polys(b, 1).ok()?;
    let size = a.size().max(b.size());
    let tol = (a.size() * 1e-3).max(1e-3);
    let infos = a.faces(tol).ok()?;
    let mesh = a.tessellate(tol).ok()?;
    // Each face's plane normal where it is planar (see [`curved_kind`]): a small round meshed
    // flat is no plane, and taken for one the middle piece would rebuild it as a chamfer.
    let surface_tol = kind_tol(a);
    let planes: Vec<Option<Vec3>> =
        a.solid.face_iter().zip(&infos).map(|(f, info)| info.plane_normal.filter(|_| curved_kind(f, true, surface_tol).is_none())).collect();
    let mut curved: BTreeMap<u32, solidcraft_geom::Aabb3> = BTreeMap::new();
    for (t, fi) in mesh.triangles.iter().zip(&mesh.tri_face) {
        if planes.get(*fi as usize)?.is_some() {
            continue;
        }
        let bx = curved.entry(*fi).or_insert(solidcraft_geom::Aabb3::EMPTY);
        for p in mesh.tri(t)? {
            bx.add(p);
        }
    }
    // All planar: the exact boolean itself, not this.
    if curved.is_empty() {
        return None;
    }
    let mut tool = solidcraft_geom::Aabb3::EMPTY;
    for p in pb.iter().flat_map(|p| &p.v) {
        tool.add(*p);
    }
    let at = |v: Vec3, k: usize| [v.x, v.y, v.z].get(k).copied().unwrap_or(0.0);
    let axes = [Vec3::X, Vec3::Y, Vec3::Z];
    // Planar faces of `a` square to an axis: a cut must not land in one of their planes.
    let flat: Vec<(Vec3, Vec3)> = planes.iter().zip(&infos).filter_map(|(n, f)| Some(((*n)?, f.centroid))).collect();
    let gap = size * 1e-4;
    let place = |k: usize, near: f64, far: f64| -> Option<f64> {
        let axis = *axes.get(k)?;
        [0.5, 0.37, 0.63, 0.25, 0.75]
            .iter()
            .map(|t| near + (far - near) * t)
            .find(|x| !flat.iter().any(|(n, c)| n.dot(axis).abs() > 1.0 - 1e-9 && (at(*c, k) - x).abs() < gap))
    };
    // An axis along which every curved face lies clear of the tool: the cuts either side.
    let (k, lo, hi) = (0..3).find_map(|k| {
        let (t0, t1) = (at(tool.min, k), at(tool.max, k));
        let (mut below, mut above) = (None::<f64>, None::<f64>);
        for bx in curved.values() {
            let (c0, c1) = (at(bx.min, k), at(bx.max, k));
            if c1 < t0 - gap {
                below = Some(below.map_or(c1, |x| x.max(c1)));
            } else if c0 > t1 + gap {
                above = Some(above.map_or(c0, |x| x.min(c0)));
            } else {
                return None;
            }
        }
        let lo = match below {
            Some(c) => Some(place(k, t0, c)?),
            None => None,
        };
        let hi = match above {
            Some(c) => Some(place(k, t1, c)?),
            None => None,
        };
        Some((k, lo, hi))
    })?;
    let (u, w) = match k {
        0 => (Vec3::Y, Vec3::Z),
        1 => (Vec3::Z, Vec3::X),
        _ => (Vec3::X, Vec3::Y),
    };
    let axis = *axes.get(k)?;
    let run = || -> Result<Option<Body>> {
        let fail = |m: &str| KernelError::Failed(format!("planar boolean beside curved faces: {m}"));
        // Every piece with its history over [a, b, then the boxes of each cut: the one away
        // from the middle, the one around it]; the boxes' faces are gone from the result (glued
        // away), so they are dropped at the end.
        let n = 6;
        let mut middle: Traced = (a.clone(), recorded::kept(a, 0, n));
        let mut outside: Vec<Traced> = Vec::new();
        for (slot, (cut, keep_positive)) in [(2, (lo, true)), (4, (hi, false))] {
            let Some(x) = cut else { continue };
            let plane = solidcraft_geom::Plane::new(axis * x, u, w).ok_or_else(|| fail("plane"))?;
            // Split along the plane (an imprint: no boolean); two booleans with half boxes
            // where the split can't be sorted into one piece kept and the rest away.
            if let Some((away, kept)) = split_beside(&middle, &plane, axis, x, keep_positive) {
                outside.extend(away);
                middle = kept;
                continue;
            }
            let (away, kept) = (crate::ops::half_box(&middle.0, &plane, !keep_positive)?, crate::ops::half_box(&middle.0, &plane, keep_positive)?);
            let (h_away, h_kept) = (recorded::kept(&away, slot, n), recorded::kept(&kept, slot + 1, n));
            if let Some(o) = crate::ops::boolean(&middle.0, &away, BoolOp::Intersect)? {
                let h = recorded::of_pair(&o, &middle.1, &h_away);
                outside.push((o, h));
            }
            let m = crate::ops::boolean(&middle.0, &kept, BoolOp::Intersect)?.ok_or_else(|| fail("nothing beside the tool"))?;
            let h = recorded::of_pair(&m, &middle.1, &h_kept);
            middle = (m, h);
        }
        let pm = planar_polys(&middle.0, 0).map_err(|e| fail(&e.describe("middle piece")))?;
        let r = polys_boolean(pm, pb, size, op, size * 1e-9, &Sources::of(&[&middle.0, b]))?;
        reassembled(a, b, op, &middle, r, outside, n, &fail)
    };
    Some(run())
}

/// The middle piece's boolean with `b` (`r`, over `[middle, b]`) and the pieces of `a` split
/// away from the middle (`outside`) glued back into one result over `[a, b]`: the operands past
/// the second (`n` in all, the boxes a piece was cut with) are dropped. `fail` names the route
/// in its errors. Shared with [`crate::prism_local`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn reassembled(
    a: &Body,
    b: &Body,
    op: BoolOp,
    middle: &Traced,
    r: Option<Body>,
    outside: Vec<Traced>,
    n: usize,
    fail: &dyn Fn(&str) -> KernelError,
) -> Result<Option<Body>> {
    let size = a.size().max(b.size());
    let h_b = recorded::kept(b, 1, n);
    let counts = record::counts(&[a, b]);
    if op == BoolOp::Intersect {
        return Ok(r.map(|r| {
            let h = dropped(recorded::of_pair(&r, &middle.1, &h_b));
            r.with_checked_history(h, &counts)
        }));
    }
    // The rebuild leaves the cut faces' edges split where other polygons were cut; healed,
    // they match the pieces outside edge for edge.
    let pieces: Vec<Traced> = match r {
        Some(m) => {
            let healed = recorded::heal_body(&m, m.deep_copy(), size, a.split_keep(), &record::counts(&[&middle.0, b]))?;
            let h = recorded::of_pair(&healed, &middle.1, &h_b);
            vec![(healed, h)]
        }
        None => Vec::new(),
    };
    // Each piece in its lumps: a cut plane crossing a frame or a ring leaves the middle (or
    // a piece outside) in several parts, which meet the others along several cut faces.
    let lumps = |(whole, h): Traced| -> Result<Vec<Traced>> {
        if whole.solid.boundaries().len() == 1 {
            return Ok(vec![(whole, h)]);
        }
        let parts = whole.lumps()?;
        let histories = recorded::parts_of(&parts, &whole);
        Ok(parts
            .into_iter()
            .zip(histories)
            .map(|(p, hp)| {
                let h = match (hp, h.as_ref()) {
                    (Some(hp), Some(h)) => recorded::composed(hp.substituted(0, h)),
                    _ => None,
                };
                (p, h)
            })
            .collect())
    };
    let mut all: Vec<Traced> = Vec::new();
    for piece in pieces.into_iter().chain(outside) {
        all.extend(lumps(piece)?);
    }
    // Glue pieces that meet along their cut faces until one is left; every cut face pair
    // goes (a pair left behind would be a slit through the result).
    let mut joined: Vec<Traced> = Vec::new();
    while let Some(mut cur) = all.pop() {
        loop {
            let found = all.iter().enumerate().find_map(|(i, (o, _))| Some((i, crate::coplanar::glue(&cur.0, o)?.ok()?)));
            let Some((i, j)) = found else { break };
            let other = all.swap_remove(i);
            let h = recorded::of_pair(&j, &cur.1, &other.1);
            cur = (j, h);
        }
        joined.push(cur);
    }
    if joined.iter().any(|(x, _)| x.solid.boundaries().len() != 1) {
        return Err(fail("a piece in several parts"));
    }
    // A join onto one piece is one piece: anything apart is a cut face that did not glue.
    if op == BoolOp::Union && a.solid.boundaries().len() == 1 && joined.len() != 1 {
        return Err(fail("a piece does not join back"));
    }
    let pieces = joined;
    recorded::join(pieces.into_iter().map(|(p, h)| (p, dropped(h))).collect(), &counts)
}

/// `piece` split by `plane` (at `x` along `axis`): the parts on the side away from the tool
/// and the one part on its side (positive along `axis` when `keep_positive`), each with its
/// history over the operands `piece`'s is over. `None` when the split leaves no part on either
/// side, or several on the tool's.
pub(crate) fn split_beside(piece: &Traced, plane: &solidcraft_geom::Plane, axis: Vec3, x: f64, keep_positive: bool) -> Option<(Vec<Traced>, Traced)> {
    let parts = crate::splitface::split_body(&piece.0, &crate::splitface::SplitTool::Plane(*plane)).ok()?;
    if parts.len() < 2 {
        return None;
    }
    let tol = piece.0.size() * 1e-6 + 1e-9;
    let (mut away, mut kept) = (Vec::new(), Vec::new());
    for part in parts {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for v in part.solid.vertex_iter() {
            let t = from_p3(v.point()).dot(axis) - x;
            lo = lo.min(t);
            hi = hi.max(t);
        }
        let h = match (part.history(), piece.1.as_ref()) {
            (Some(hp), Some(h)) => recorded::composed(hp.clone().substituted(0, h)),
            _ => None,
        };
        let positive = if lo >= -tol {
            true
        } else if hi <= tol {
            false
        } else {
            return None;
        };
        if positive == keep_positive { kept.push((part, h)) } else { away.push((part, h)) }
    }
    let kept = kept.pop().filter(|_| kept.is_empty())?;
    (!away.is_empty()).then_some((away, kept))
}

/// `h` over `[a, b]`: the operands after them (the boxes a piece was cut with) dropped, which
/// fails where something of them is left.
fn dropped(h: Option<History>) -> Option<History> {
    let h = h?;
    let map: Vec<Option<usize>> = (0..h.operands).map(|op| (op < 2).then_some(op)).collect();
    recorded::composed(h.renumbered(&map, 2))
}

fn polys_boolean(pa: Vec<Poly>, pb: Vec<Poly>, size: f64, op: BoolOp, eps: f64, sources: &Sources) -> Result<Option<Body>> {
    let polys = csg(pa, pb, op, eps)?;
    if polys.is_empty() {
        return Ok(None);
    }
    rebuild(polys, size, sources).map(Some)
}

/// What a rebuilt body's history is over: each operand's face and edge counts and, where it is
/// a B-rep, its edges in its edge order (the faces each bounds, and its ends).
struct Sources {
    counts: Vec<(usize, usize)>,
    edges: Vec<Option<OperandEdges>>,
}

/// An operand's edges in its edge order (each one's ends), and those between each pair of its
/// faces (lesser face first).
struct OperandEdges {
    ends: Vec<(Vec3, Vec3)>,
    between: BTreeMap<(usize, usize), Vec<usize>>,
}

impl OperandEdges {
    fn of(b: &Body) -> OperandEdges {
        let (edges, bounds) = edge_order(b.solid.face_iter());
        let mut between: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
        for (i, fs) in bounds.iter().enumerate() {
            if let [f, g] = fs.as_slice() {
                between.entry((*f.min(g), *f.max(g))).or_default().push(i);
            }
        }
        let ends = edges.iter().map(|e| (from_p3(e.absolute_front().point()), from_p3(e.absolute_back().point()))).collect();
        OperandEdges { ends, between }
    }
}

impl Sources {
    fn of(bodies: &[&Body]) -> Sources {
        let edges: Vec<OperandEdges> = bodies.iter().map(|b| OperandEdges::of(b)).collect();
        let counts = bodies.iter().zip(&edges).map(|(b, e)| (b.face_count(), e.ends.len())).collect();
        Sources { counts, edges: edges.into_iter().map(Some).collect() }
    }

    /// The origins of an edge between a piece of `p` and a piece of `q` running from `ends.0`
    /// to `ends.1`: where faces of two operands meet; the edge between two faces of one
    /// operand (the one the piece lies along, when they share several); none between pieces of
    /// one face, or between triangles of a mesh operand. `None` when it can't be told.
    fn edge(&self, p: FaceOrigin, q: FaceOrigin, ends: (Vec3, Vec3), tol: f64) -> Option<Vec<EdgeOrigin>> {
        let (FaceOrigin::Face { operand: x, face: f }, FaceOrigin::Face { operand: y, face: g }) = (p, q) else { return None };
        if x != y {
            return Some(vec![EdgeOrigin::meet(p, q)]);
        }
        let Some(edges) = self.edges.get(x)?.as_ref() else { return Some(Vec::new()) };
        if f == g {
            return Some(Vec::new());
        }
        let between = edges.between.get(&(f.min(g), f.max(g)))?;
        let along: Vec<usize> = match between.as_slice() {
            [_] => between.clone(),
            _ => between
                .iter()
                .copied()
                .filter(|i| edges.ends.get(*i).is_some_and(|(s, t)| ends.0.dist_to_segment(*s, *t) <= tol && ends.1.dist_to_segment(*s, *t) <= tol))
                .collect(),
        };
        match along.as_slice() {
            [edge] => Some(vec![EdgeOrigin::Edge { operand: x, edge: *edge }]),
            _ => None,
        }
    }
}

/// Enclosed volume of a closed set of polygons (divergence theorem).
fn polys_volume(ps: &[Poly]) -> f64 {
    let mut v6 = 0.0;
    for p in ps {
        let Some(&o) = p.v.first() else { continue };
        for w in p.v.windows(2).skip(1) {
            v6 += o.dot(w[0].cross(w[1]));
        }
    }
    v6 / 6.0
}

/// The polygons of a BSP boolean (csg.js style).
fn csg(pa: Vec<Poly>, pb: Vec<Poly>, op: BoolOp, eps: f64) -> Result<Vec<Poly>> {
    let (mut na, mut nb) = (Node::default(), Node::default());
    na.build(pa, eps, 0)?;
    nb.build(pb, eps, 0)?;
    match op {
        BoolOp::Union => {
            na.clip_to(&nb, eps);
            nb.clip_to(&na, eps);
            nb.invert();
            nb.clip_to(&na, eps);
            nb.invert();
            let mut extra = Vec::new();
            nb.all(&mut extra);
            na.build(extra, eps, 0)?;
        }
        BoolOp::Cut => {
            na.invert();
            na.clip_to(&nb, eps);
            nb.clip_to(&na, eps);
            nb.invert();
            nb.clip_to(&na, eps);
            nb.invert();
            let mut extra = Vec::new();
            nb.all(&mut extra);
            na.build(extra, eps, 0)?;
            na.invert();
        }
        BoolOp::Intersect => {
            na.invert();
            nb.clip_to(&na, eps);
            nb.invert();
            na.clip_to(&nb, eps);
            nb.clip_to(&na, eps);
            let mut extra = Vec::new();
            nb.all(&mut extra);
            na.build(extra, eps, 0)?;
            na.invert();
        }
    }
    let mut polys = Vec::new();
    na.all(&mut polys);
    Ok(polys)
}

/// `shell`'s connected components, each face in the shell's order and the components by their
/// first face. truck's own come out in the hash order of face addresses, so the same rebuild
/// gave a body with its faces in a different order from run to run (#707).
pub(crate) fn components_in_order(shell: &mt::Shell) -> Vec<mt::Shell> {
    let pos: HashMap<mt::FaceID, usize> = shell.face_iter().enumerate().map(|(i, f)| (f.id(), i)).collect();
    let key = |f: &mt::Face| pos.get(&f.id()).copied().unwrap_or(usize::MAX);
    let mut comps: Vec<Vec<mt::Face>> = shell
        .connected_components()
        .into_iter()
        .map(|c| {
            let mut faces: Vec<mt::Face> = c.into_iter().collect();
            faces.sort_by_key(|f| key(f));
            faces
        })
        .collect();
    comps.sort_by_key(|c| c.first().map(key).unwrap_or(usize::MAX));
    comps.into_iter().map(mt::Shell::from).collect()
}

/// Position key for vertex merging.
fn key(p: Vec3, q: f64) -> (i64, i64, i64) {
    ((p.x / q).round() as i64, (p.y / q).round() as i64, (p.z / q).round() as i64)
}

/// Turn polygons into a B-rep solid: merge by plane, split T-junctions, chain boundary loops.
/// Its history over the operands the polygons are pieces of (`sources`): each face the
/// operand faces whose pieces it joins (pieces joined across the edges between them), each edge
/// what the pieces either side of it say (see [`Sources::edge`]).
fn rebuild(polys: Vec<Poly>, size: f64, sources: &Sources) -> Result<Body> {
    let q = size * 1e-7;
    // Global vertex table.
    let mut index: BTreeMap<(i64, i64, i64), usize> = BTreeMap::new();
    let mut verts: Vec<Vec3> = Vec::new();
    // Points within `q` share a vertex, also across the rounding grid's cell boundaries.
    let mut id = |p: Vec3, verts: &mut Vec<Vec3>| -> usize {
        let k = key(p, q);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(&i) = index.get(&(k.0 + dx, k.1 + dy, k.2 + dz))
                        && verts.get(i).is_some_and(|v| (*v - p).len() <= q)
                    {
                        return i;
                    }
                }
            }
        }
        verts.push(p);
        index.insert(k, verts.len() - 1);
        verts.len() - 1
    };
    let mut loops: Vec<(PlaneEq, Vec<usize>)> = Vec::new();
    let mut srcs: Vec<FaceOrigin> = Vec::new();
    for p in &polys {
        let mut l: Vec<usize> = p.v.iter().map(|v| id(*v, &mut verts)).collect();
        l.dedup();
        if l.first() == l.last() && l.len() > 1 {
            l.pop();
        }
        if l.len() >= 3 {
            loops.push((p.plane, l));
            srcs.push(p.src);
        }
    }
    // Split every polygon edge at vertices lying on it (T-junctions). Vertices are binned in a
    // grid sized to the typical edge, so each edge only looks at its neighbourhood.
    let tol = size * 1e-7;
    let edge_len: Vec<f64> = loops
        .iter()
        .flat_map(|(_, l)| (0..l.len()).filter_map(move |i| Some((*l.get(i)?, *l.get((i + 1) % l.len())?))))
        .filter_map(|(a, b)| Some((*verts.get(b)? - *verts.get(a)?).len()))
        .collect();
    let mean = edge_len.iter().sum::<f64>() / edge_len.len().max(1) as f64;
    let cell = mean.max(size * 1e-4).max(1e-9);
    let ckey = |p: Vec3| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64, (p.z / cell).floor() as i64);
    let mut grid: BTreeMap<(i64, i64, i64), Vec<usize>> = BTreeMap::new();
    for (k, v) in verts.iter().enumerate() {
        grid.entry(ckey(*v)).or_default().push(k);
    }
    let mut directed: Vec<(usize, usize, usize)> = Vec::new(); // (plane group, from, to)
    // The polygons (by loop) each directed edge comes from.
    let mut owners: BTreeMap<(usize, usize, usize), Vec<usize>> = BTreeMap::new();
    let mut groups: Vec<PlaneEq> = Vec::new();
    let mut group_of: BTreeMap<(i64, i64, i64, i64), usize> = BTreeMap::new();
    for (li, (pl, l)) in loops.iter().enumerate() {
        let gk = ((pl.n.x * 1e8).round() as i64, (pl.n.y * 1e8).round() as i64, (pl.n.z * 1e8).round() as i64, (pl.w / (tol * 10.0)).round() as i64);
        let g = *group_of.entry(gk).or_insert_with(|| {
            groups.push(*pl);
            groups.len() - 1
        });
        let n = l.len();
        for i in 0..n {
            let (Some(&a), Some(&b)) = (l.get(i), l.get((i + 1) % n)) else { continue };
            let (Some(&pa), Some(&pb)) = (verts.get(a), verts.get(b)) else { continue };
            let d = pb - pa;
            let len2 = d.len2();
            let (lo, hi) = (ckey(pa.min(pb) - Vec3::new(tol, tol, tol)), ckey(pa.max(pb) + Vec3::new(tol, tol, tol)));
            let cells = (hi.0 - lo.0 + 1) * (hi.1 - lo.1 + 1) * (hi.2 - lo.2 + 1);
            let candidates: Vec<usize> = if cells > 4096 {
                (0..verts.len()).collect()
            } else {
                let mut c = Vec::new();
                for x in lo.0..=hi.0 {
                    for y in lo.1..=hi.1 {
                        for z in lo.2..=hi.2 {
                            if let Some(ks) = grid.get(&(x, y, z)) {
                                c.extend(ks.iter().copied());
                            }
                        }
                    }
                }
                c
            };
            let mut on: Vec<(f64, usize)> = candidates
                .into_iter()
                .filter(|k| *k != a && *k != b)
                .filter_map(|k| {
                    let v = verts.get(k)?;
                    let t = (*v - pa).dot(d) / len2.max(1e-300);
                    (t > 1e-9 && t < 1.0 - 1e-9 && v.dist_to_segment(pa, pb) < tol).then_some((t, k))
                })
                .collect();
            on.sort_by(|x, y| x.0.total_cmp(&y.0));
            let mut prev = a;
            for (_, k) in on {
                directed.push((g, prev, k));
                owners.entry((g, prev, k)).or_default().push(li);
                prev = k;
            }
            directed.push((g, prev, b));
            owners.entry((g, prev, b)).or_default().push(li);
        }
    }
    // Interior edges cancel against their reverse within the same plane group.
    let mut count: BTreeMap<(usize, usize, usize), i64> = BTreeMap::new();
    for (g, a, b) in &directed {
        *count.entry((*g, *a, *b)).or_insert(0) += 1;
    }
    let mut boundary: Vec<(usize, usize, usize)> = Vec::new();
    for (&(g, a, b), &c) in &count {
        let rev = count.get(&(g, b, a)).copied().unwrap_or(0);
        for _ in 0..(c - rev).max(0) {
            boundary.push((g, a, b));
        }
    }
    boundary.sort_unstable();
    // Pieces either side of an edge inside a plane group lie in one face.
    let mut parent: Vec<usize> = (0..loops.len()).collect();
    for ((g, a, b), here) in &owners {
        let (Some(there), Some(&first)) = (owners.get(&(*g, *b, *a)), here.first()) else { continue };
        for other in here.iter().chain(there) {
            let (rp, rq) = (find(&mut parent, first), find(&mut parent, *other));
            if let Some(slot) = parent.get_mut(rp) {
                *slot = rq;
            }
        }
    }
    let roots: Vec<usize> = (0..loops.len()).map(|li| find(&mut parent, li)).collect();
    let mut joined: BTreeMap<usize, BTreeSet<FaceOrigin>> = BTreeMap::new();
    for (root, src) in roots.iter().zip(&srcs) {
        joined.entry(*root).or_default().insert(*src);
    }
    // What lies on each side of each boundary edge (from, to): the faces of its polygons.
    let mut side: BTreeMap<(usize, usize), BTreeSet<FaceOrigin>> = BTreeMap::new();
    for (g, a, b) in &boundary {
        let set = side.entry((*a, *b)).or_default();
        for li in owners.get(&(*g, *a, *b)).into_iter().flatten() {
            set.extend(srcs.get(*li));
        }
    }
    // The faces of one boundary loop's polygons.
    let loop_faces = |g: usize, lp: &[usize]| -> Option<BTreeSet<FaceOrigin>> {
        let (a, b) = (*lp.first()?, *lp.get(1)?);
        let li = *owners.get(&(g, a, b))?.first()?;
        joined.get(roots.get(li)?).cloned()
    };
    guard("planar boolean rebuild", || {
        let mut verts = verts.clone();
        // Every plane group's boundary loops (vertex indices).
        let mut all_loops: Vec<Vec<Vec<usize>>> = Vec::new();
        for (g, _) in groups.iter().enumerate() {
            let mut segs: Vec<(usize, usize)> = boundary.iter().filter(|e| e.0 == g).map(|e| (e.1, e.2)).collect();
            let mut loops_g: Vec<Vec<usize>> = Vec::new();
            while let Some((s, t)) = segs.pop() {
                let mut lp = vec![s, t];
                let mut cur = t;
                let mut guard_n = 0;
                while cur != s && guard_n < 100_000 {
                    guard_n += 1;
                    let Some(k) = segs.iter().position(|(a, _)| *a == cur) else { break };
                    let (_, nb) = segs.remove(k);
                    lp.push(nb);
                    cur = nb;
                }
                if cur == s {
                    lp.pop();
                    // A walk through a vertex twice (two loops touching there) is split into
                    // simple loops at that vertex.
                    let mut stack: Vec<usize> = Vec::new();
                    for v in lp {
                        if let Some(at) = stack.iter().position(|x| *x == v) {
                            let sub: Vec<usize> = stack.split_off(at);
                            if sub.len() >= 3 {
                                loops_g.push(sub);
                            }
                        }
                        stack.push(v);
                    }
                    if stack.len() >= 3 {
                        loops_g.push(stack);
                    }
                }
            }
            all_loops.push(loops_g);
        }
        // The original vertex of each vertex index (pinched vertices get copies, below).
        let mut orig: Vec<usize> = (0..verts.len()).collect();
        split_pinched_vertices(&mut all_loops, &mut verts, &mut orig);
        let tv: Vec<mt::Vertex> = verts.iter().map(|v| builder::vertex(p3(*v))).collect();
        let mut edges: BTreeMap<(usize, usize), mt::Edge> = BTreeMap::new();
        let mut faces: Vec<mt::Face> = Vec::new();
        let mut face_from: HashMap<mt::FaceID, Option<BTreeSet<FaceOrigin>>> = HashMap::new();
        for (g, plane) in groups.iter().enumerate() {
            let Some(loops_g) = all_loops.get(g) else { continue };
            let mut wires: Vec<mt::Wire> = Vec::new();
            for lp in loops_g {
                let n = lp.len();
                let mut es = Vec::with_capacity(n);
                for i in 0..n {
                    let (Some(&a), Some(&b)) = (lp.get(i), lp.get((i + 1) % n)) else { continue };
                    let k = (a.min(b), a.max(b));
                    let e = match edges.get(&k) {
                        Some(e) => e.clone(),
                        None => {
                            let (Some(va), Some(vb)) = (tv.get(k.0), tv.get(k.1)) else { continue };
                            let e = builder::line(va, vb);
                            edges.insert(k, e.clone());
                            e
                        }
                    };
                    es.push(if a < b { e } else { e.inverse() });
                }
                wires.push(es.into());
            }
            if wires.is_empty() {
                continue;
            }
            // Outer loops are counter-clockwise about the plane normal; holes go with the
            // outer loop that contains them.
            let area = |lp: &Vec<usize>| -> f64 {
                let n = lp.len();
                let mut s = Vec3::ZERO;
                for i in 0..n {
                    if let (Some(a), Some(b)) = (lp.get(i).and_then(|k| verts.get(*k)), lp.get((i + 1) % n).and_then(|k| verts.get(*k))) {
                        s += a.cross(*b);
                    }
                }
                s.dot(plane.n) * 0.5
            };
            let (outer, holes): (Vec<usize>, Vec<usize>) = (0..loops_g.len()).partition(|i| loops_g.get(*i).is_some_and(|l| area(l) > 0.0));
            let u = plane.n.any_perp();
            let w = plane.n.cross(u);
            let to2 = |p: Vec3| (p.dot(u), p.dot(w));
            let inside = |pt: Vec3, lp: &Vec<usize>| {
                let (px, py) = to2(pt);
                let n = lp.len();
                let mut c = false;
                for i in 0..n {
                    let (Some(a), Some(b)) = (lp.get(i).and_then(|k| verts.get(*k)), lp.get((i + 1) % n).and_then(|k| verts.get(*k))) else {
                        continue;
                    };
                    let ((ax, ay), (bx, by)) = (to2(*a), to2(*b));
                    if (ay > py) != (by > py) && px < ax + (py - ay) / (by - ay) * (bx - ax) {
                        c = !c;
                    }
                }
                c
            };
            for o in &outer {
                let Some(ol) = loops_g.get(*o) else { continue };
                let mut ws = vec![wires.get(*o).cloned().ok_or_else(|| KernelError::Failed("wire".into()))?];
                for h in &holes {
                    let Some(hl) = loops_g.get(*h) else { continue };
                    let probe = hl.first().and_then(|k| verts.get(*k)).copied().unwrap_or_default();
                    // The innermost outer loop containing the hole.
                    let owner = outer.iter().filter(|x| loops_g.get(**x).is_some_and(|l| inside(probe, l))).min_by(|x, y| {
                        let ax = loops_g.get(**x).map(area).unwrap_or(0.0);
                        let ay = loops_g.get(**y).map(area).unwrap_or(0.0);
                        ax.total_cmp(&ay)
                    });
                    if owner == Some(o) {
                        ws.push(wires.get(*h).cloned().ok_or_else(|| KernelError::Failed("wire".into()))?);
                    }
                }
                let from = loop_faces(g, &ol.iter().map(|k| orig.get(*k).copied().unwrap_or(*k)).collect::<Vec<_>>());
                // A loop a hair off flat (fine facets of a curved face) takes the group's plane.
                let face = match builder::try_attach_plane(&ws) {
                    Ok(f) => f,
                    Err(e) => {
                        let o = plane.n * plane.w;
                        let surface = mt::Plane::new(p3(o), p3(o + u), p3(o + w));
                        mt::Face::try_new(ws, mt::Surface::Plane(surface))
                            .map_err(|e2| KernelError::Failed(format!("planar boolean face: {e}; {e2}")))?
                    }
                };
                face_from.insert(face.id(), from);
                faces.push(face);
            }
        }
        let shell: mt::Shell = faces.into();
        let parts = components_in_order(&shell);
        let solid = Solid::try_new(parts).map_err(|e| KernelError::Failed(format!("planar boolean: {e}")))?;
        let regions = solid.face_iter().map(|f| face_from.get(&f.id()).cloned().flatten().map(|o| (o, false))).collect::<Option<Vec<_>>>();
        let tol = size * 1e-6;
        let edge_from = edges
            .iter()
            .map(|((a, b), e)| {
                let ends = (*verts.get(*a)?, *verts.get(*b)?);
                let (a, b) = (orig.get(*a)?, orig.get(*b)?);
                let mut origins = BTreeSet::new();
                for p in side.get(&(*a, *b))? {
                    for q in side.get(&(*b, *a))? {
                        origins.extend(sources.edge(*p, *q, ends, tol)?);
                    }
                }
                Some((e.id(), origins.into_iter().collect()))
            })
            .collect::<Option<HashMap<mt::EdgeID, Vec<EdgeOrigin>>>>();
        record::build(solid, &sources.counts, regions.map(record::regions), edge_from.as_ref())
    })
}

/// A vertex where the solid's surface pinches, as where two blocks meet at one corner only
/// (#679): the faces around it form several fans that share no edge there. Each fan past the
/// first gets its own copy of the vertex (same point), so every vertex of the shell is a
/// manifold one.
fn split_pinched_vertices(loops: &mut [Vec<Vec<usize>>], verts: &mut Vec<Vec3>, orig: &mut Vec<usize>) {
    // Every corner: (group, loop, position), by vertex.
    let mut corners: BTreeMap<usize, Vec<(usize, usize, usize)>> = BTreeMap::new();
    for (g, ls) in loops.iter().enumerate() {
        for (li, l) in ls.iter().enumerate() {
            for (i, v) in l.iter().enumerate() {
                corners.entry(*v).or_default().push((g, li, i));
            }
        }
    }
    let mut renames: Vec<((usize, usize, usize), usize)> = Vec::new();
    let mut ids: Vec<usize> = corners.keys().copied().collect();
    ids.sort_unstable();
    for v in ids {
        let Some(cs) = corners.get(&v) else { continue };
        if cs.len() < 2 {
            continue;
        }
        // The two neighbours of each corner; corners sharing an edge (v, x) are in one fan.
        let nbrs: Vec<[usize; 2]> = cs
            .iter()
            .map(|(g, li, i)| {
                let l = loops.get(*g).and_then(|ls| ls.get(*li)).map(Vec::as_slice).unwrap_or(&[]);
                let n = l.len().max(1);
                [l.get((i + n - 1) % n).copied().unwrap_or(v), l.get((i + 1) % n).copied().unwrap_or(v)]
            })
            .collect();
        let mut parent: Vec<usize> = (0..cs.len()).collect();
        for i in 0..cs.len() {
            for j in i + 1..cs.len() {
                let (Some(a), Some(b)) = (nbrs.get(i), nbrs.get(j)) else { continue };
                if a.iter().any(|x| b.contains(x)) {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    if let Some(slot) = parent.get_mut(ri) {
                        *slot = rj;
                    }
                }
            }
        }
        let mut fan_id: BTreeMap<usize, usize> = BTreeMap::new();
        for (k, c) in cs.iter().enumerate() {
            let root = find(&mut parent, k);
            let next = fan_id.len();
            let fan = *fan_id.entry(root).or_insert(next);
            if fan > 0 {
                // A copy per extra fan.
                let copy = verts.len() + fan - 1;
                renames.push((*c, copy));
            }
        }
        let p = verts.get(v).copied().unwrap_or_default();
        for _ in 1..fan_id.len() {
            verts.push(p);
            orig.push(v);
        }
    }
    for ((g, li, i), new) in renames {
        if let Some(slot) = loops.get_mut(g).and_then(|ls| ls.get_mut(li)).and_then(|l| l.get_mut(i)) {
            *slot = new;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn compound_curved_diagnostics_count_every_surface_kind() {
        // Many independent shells exercise the error path without performing a costly
        // Boolean. Each cylinder has two analytic half-cylinder walls and two flat caps.
        let mut shells = Vec::new();
        for i in 0..64 {
            let cylinder = crate::cylinder(Vec3::X * (i as f64 * 6.0), Vec3::Z, 2.0, 5.0).unwrap();
            shells.extend(cylinder.solid.boundaries().iter().cloned());
        }
        let sphere = crate::sphere(Vec3::new(0.0, 12.0, 0.0), 2.0).unwrap();
        let sphere_faces = sphere.face_count();
        shells.extend(sphere.solid.boundaries().iter().cloned());
        let body = guard("diagnostic compound", || Body::new(Solid::try_new(shells).unwrap())).unwrap();
        let faces = body.faces(0.02).unwrap();
        let error = face_planes(&body, &faces).err().unwrap();
        let expected = std::collections::BTreeMap::from([("cylinder", 128), ("sphere", sphere_faces)]);
        assert_eq!(error, NotPlanar::Curved(expected));
        assert_eq!(error.describe("tool"), format!("the tool has {} curved faces (cylinder, sphere)", 128 + sphere_faces));
    }

    #[test]
    fn planar_mesh_inconsistency_does_not_become_curved_classification() {
        let body = crate::box_solid(Vec3::ZERO, Vec3::new(4.0, 5.0, 6.0)).unwrap();
        let mut faces = body.faces(0.01).unwrap();
        assert_eq!(face_planes(&body, &faces).unwrap().len(), 6);
        faces[0].plane_normal = None;
        assert_eq!(face_planes(&body, &faces).err().unwrap(), NotPlanar::Other("face 0 is planar but its mesh is not flat".into()));
        faces[0].area = 0.0;
        assert_eq!(face_planes(&body, &faces).err().unwrap(), NotPlanar::Other("face 0 could not be meshed".into()));
        faces[0].index = 999;
        assert_eq!(face_planes(&body, &faces).err().unwrap(), NotPlanar::Other("mesh is inconsistent".into()));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod small_round_tests {
    use std::f64::consts::FRAC_PI_2;

    use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2};

    use super::*;

    /// An `l` x `w` x 1 plate on XY with its four corners rounded (radius `r`), and a hole of
    /// diameter 4 through it 100 from one end when `hole`.
    fn rounded_plate(l: f64, w: f64, r: f64, hole: bool) -> Body {
        let (x, y) = (l / 2.0, w / 2.0);
        let corners = [(x - r, y - r), (r - x, y - r), (r - x, r - y), (x - r, r - y)];
        let mut segs = Vec::new();
        for (k, (cx, cy)) in corners.iter().enumerate() {
            let start = k as f64 * FRAC_PI_2;
            let at = |t: f64| Vec2::new(cx + r * t.cos(), cy + r * t.sin());
            segs.push(Seg2::Arc { center: Vec2::new(*cx, *cy), radius: r, start, sweep: FRAC_PI_2 });
            let (nx, ny) = corners[(k + 1) % 4];
            let next = start + FRAC_PI_2;
            segs.push(Seg2::Line { a: at(next), b: Vec2::new(nx + r * next.cos(), ny + r * next.sin()) });
        }
        let holes = if hole { vec![Loop2::circle(Vec2::new(x - 100.0, 0.0), 2.0).reversed()] } else { Vec::new() };
        crate::extrude_planar_regions(&Plane::XY, &[Region2 { outer: Loop2 { segs }, holes }], 0.0, 1.0).unwrap()
    }

    /// Faces of `b` on cylinders of radius `r`.
    fn cylinders(b: &Body, r: f64) -> usize {
        (0..b.face_count())
            .filter(|&i| matches!(crate::face_surface(b, i), Some(crate::FaceSurface::Cylinder { radius, .. }) if (radius - r).abs() < 1e-9))
            .count()
    }

    /// A pocket cut into the middle of a long plate by the exact fallback leaves the plate's
    /// small corner rounds round. Meshed at a thousandth of the 1200 mm plate (1.2 mm), each
    /// 2 mm quarter round (its chord 0.59 mm inside the arc) is one flat strip of facets, which
    /// was taken for a plane: the whole plate went to the polygon boolean (and, beside a round
    /// hole, the rounds went into the planar slab around the pocket), and the rounds came back
    /// flat chamfers, each taking r^2 (pi/4 - 1/2) = 1.14 mm^3 more than its round.
    #[test]
    fn a_long_plates_small_rounds_stay_round_beside_a_pocket() {
        for hole in [false, true] {
            let plate = rounded_plate(1200.0, 40.0, 2.0, hole);
            assert_eq!(cylinders(&plate, 2.0), 4 + 2 * usize::from(hole));
            // 10 x 5, 0.1 deep from the top.
            let tool = crate::box_solid(Vec3::new(-5.0, -2.5, 0.9), Vec3::new(5.0, 2.5, 1.0)).unwrap();
            let r = planar_boolean(&plate, &tool, BoolOp::Cut).unwrap().unwrap();
            assert_eq!(cylinders(&r, 2.0), 4 + 2 * usize::from(hole), "hole {hole}");
            // The pocket's 10 x 5 x 0.1 and nothing else (a flattened round takes 1.14 more).
            let removed = crate::measure(&plate).unwrap().volume - crate::measure(&r).unwrap().volume;
            assert!((removed - 5.0).abs() < 1e-3, "hole {hole}: removed {removed}");
        }
    }
}
