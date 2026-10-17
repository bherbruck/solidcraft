//! Does a fillet or chamfer fit where it is asked for (#1080)? A blend of a straight edge
//! between two planar faces takes away (or, on a concave edge, fills) at most the wedge between
//! the edge and its two setback lines. Any other face of the body inside that wedge means the
//! blend runs past a neighbouring face's boundary into other geometry (a hole, a pocket, an
//! earlier round), and two such wedges overlapping mean two blends cross. Either way the result
//! the operations build is not a blend of those edges, so it is refused with the largest size
//! that does fit. After the build, the volume taken away is checked against the wedges too.

use solidcraft_geom::Vec3;
use truck_modeling as mt;

use crate::blend::{ChamferSide, plane_normal, vtx};
use crate::body::Body;
use crate::{KernelError, Result};

/// What is asked for on every edge.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Size {
    /// A round of this radius.
    Round(f64),
    /// An equal-distance chamfer.
    Chamfer(f64),
    /// A chamfer with a second distance or an angle (see [`crate::chamfer_sides`]).
    Sides(f64, ChamferSide, bool),
}

impl Size {
    fn scaled(self, k: f64) -> Size {
        match self {
            Size::Round(r) => Size::Round(r * k),
            Size::Chamfer(s) => Size::Chamfer(s * k),
            Size::Sides(d, ChamferSide::Distance(d2), f) => Size::Sides(d * k, ChamferSide::Distance(d2 * k), f),
            Size::Sides(d, a, f) => Size::Sides(d * k, a, f),
        }
    }

    fn value(self) -> f64 {
        match self {
            Size::Round(v) | Size::Chamfer(v) | Size::Sides(v, _, _) => v,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Size::Round(_) => "fillet",
            _ => "chamfer",
        }
    }
}

/// A straight edge between two planar faces: its ends, the unit directions into each face
/// (perpendicular to the edge) and the faces' outward normals.
struct Frame {
    p0: Vec3,
    p1: Vec3,
    /// Into face a and face b, from the edge.
    ta: Vec3,
    tb: Vec3,
    na: Vec3,
    nb: Vec3,
    /// The faces (indices in the body's face order) the blend may meet: the two beside the
    /// edge and every face at its ends.
    allowed: Vec<usize>,
    ends: [mt::VertexID; 2],
    /// Another edge of the two faces runs on from an end tangentially: the blend runs on along
    /// that chain past this edge.
    chained: bool,
}

/// One edge's region: the triangle edge, setback on face a, setback on face b, swept along the
/// edge, as half-spaces `n·x <= c` and as its five boundary polygons.
struct Wedge {
    planes: [(Vec3, f64); 5],
    faces: Vec<Vec<Vec3>>,
    /// Cross-section area times length: the most a blend of the edge takes away or fills.
    volume: f64,
}

impl Frame {
    /// The setbacks along face a and face b for `size`; `None` where the faces are (nearly)
    /// tangent or the size doesn't make a blend.
    fn setbacks(&self, size: Size) -> Option<(f64, f64)> {
        let cos = self.ta.dot(self.tb).clamp(-1.0, 1.0);
        let phi = cos.acos();
        if !(1e-3..std::f64::consts::PI - 1e-3).contains(&phi) {
            return None;
        }
        match size {
            // The round touches each face at r / tan(φ/2) from the edge.
            Size::Round(r) => {
                let t = r / (phi * 0.5).tan();
                Some((t, t))
            }
            Size::Chamfer(s) => Some((s, s)),
            Size::Sides(d1, side, flip) => {
                // As `chamfer_tool_sides`: side 1 is the face facing up most, unless flipped.
                let up = Vec3::new(1e-6, 1e-3, 1.0);
                let a_is_first = (self.na.dot(up) >= self.nb.dot(up)) != flip;
                let d2 = match side {
                    ChamferSide::Distance(d) => d,
                    ChamferSide::Angle(a) => {
                        let den = (phi + a).sin();
                        if den <= 1e-9 {
                            return None;
                        }
                        d1 * a.sin() / den
                    }
                };
                Some(if a_is_first { (d1, d2) } else { (d2, d1) })
            }
        }
    }

    fn wedge(&self, size: Size) -> Option<Wedge> {
        let (sa, sb) = self.setbacks(size)?;
        if !(sa.is_finite() && sb.is_finite() && sa > 0.0 && sb > 0.0) {
            return None;
        }
        self.prism(self.ta * sa, self.tb * sb)
    }

    /// For a round, the arc of its cross-section from the setback on face a to the one on face
    /// b, in [`ARC_PIECES`] pieces, as offsets from the edge; `None` for a chamfer.
    fn arc(&self, size: Size) -> Option<Vec<Vec3>> {
        let Size::Round(r) = size else { return None };
        let (sa, sb) = self.setbacks(size)?;
        let half = 0.5 * self.ta.dot(self.tb).clamp(-1.0, 1.0).acos();
        // The ball's centre, on the bisector, `r` from both faces.
        let centre = (self.ta + self.tb).normalized()? * (r / half.sin());
        let (a, b) = (self.ta * sa - centre, self.tb * sb - centre);
        (0..=ARC_PIECES).map(|k| Some(centre + (a + (b - a) * (k as f64 / ARC_PIECES as f64)).normalized()? * r)).collect()
    }

    /// What a blend of the edge at `size` changes, within the wedge: for a round, the fan of
    /// prisms from the edge to the pieces of its arc (between the edge and the arc lies all it
    /// takes away or fills; the ball's side of the arc is left as it is), for a chamfer the
    /// wedge itself.
    fn pieces(&self, size: Size) -> Option<Vec<Wedge>> {
        match self.arc(size) {
            Some(arc) => arc.windows(2).map(|w| self.prism(*w.first()?, *w.get(1)?)).collect(),
            None => Some(vec![self.wedge(size)?]),
        }
    }

    /// The blend's own surface at `size`, as polygons: the strips between its arc's pieces, or
    /// the chamfer's face.
    fn surface(&self, size: Size) -> Option<Vec<Vec<Vec3>>> {
        match self.arc(size) {
            Some(arc) => arc
                .windows(2)
                .map(|w| {
                    let (u, v) = (*w.first()?, *w.get(1)?);
                    Some(vec![self.p0 + u, self.p0 + v, self.p1 + v, self.p1 + u])
                })
                .collect(),
            None => Some(vec![self.wedge(size)?.faces.get(2)?.clone()]),
        }
    }

    /// The triangle between the edge and the offsets `u` and `w` across it, swept along the edge.
    fn prism(&self, u: Vec3, w: Vec3) -> Option<Wedge> {
        let d = (self.p1 - self.p0).normalized()?;
        let (a0, b0) = (self.p0 + u, self.p0 + w);
        let (a1, b1) = (self.p1 + u, self.p1 + w);
        // Each side's plane, facing away from the triangle's third corner.
        let side = |p: Vec3, q: Vec3, opposite: Vec3| -> Option<(Vec3, f64)> {
            let mut n = d.cross(q - p).normalized()?;
            if n.dot(opposite - p) > 0.0 {
                n = -n;
            }
            Some((n, n.dot(p)))
        };
        let planes = [side(self.p0, a0, b0)?, side(self.p0, b0, a0)?, side(a0, b0, self.p0)?, (-d, -d.dot(self.p0)), (d, d.dot(self.p1))];
        let faces =
            vec![vec![self.p0, a0, a1, self.p1], vec![self.p0, b0, b1, self.p1], vec![a0, b0, b1, a1], vec![self.p0, a0, b0], vec![self.p1, a1, b1]];
        let area = 0.5 * u.cross(w).len();
        Some(Wedge { planes, faces, volume: area * self.p0.dist(self.p1) })
    }
}

/// Pieces a round's arc is divided into for [`Frame::pieces`] and [`Frame::surface`]: each
/// chord sags under 0.5 % of the radius.
const ARC_PIECES: usize = 8;

/// `poly` clipped to the half-spaces `n·x <= c`, each moved `eps` inwards (empty when nothing
/// is left).
fn clip(poly: &[Vec3], planes: &[(Vec3, f64)], eps: f64) -> Vec<Vec3> {
    let mut cur: Vec<Vec3> = poly.to_vec();
    for (n, c) in planes {
        let c = c - eps;
        let mut next = Vec::with_capacity(cur.len() + 2);
        for (i, p) in cur.iter().enumerate() {
            let Some(q) = cur.get((i + 1) % cur.len()) else { continue };
            let (dp, dq) = (n.dot(*p) - c, n.dot(*q) - c);
            if dp <= 0.0 {
                next.push(*p);
            }
            if (dp < 0.0 && dq > 0.0) || (dp > 0.0 && dq < 0.0) {
                next.push(*p + (*q - *p) * (dp / (dp - dq)));
            }
        }
        if next.is_empty() {
            return next;
        }
        cur = next;
    }
    cur
}

/// `poly` clipped to the half-spaces, each moved `eps` inwards: is anything left?
fn reaches_inside(poly: &[Vec3], planes: &[(Vec3, f64)], eps: f64) -> bool {
    !clip(poly, planes, eps).is_empty()
}

/// Most polygons [`reaches_inside_beyond`] clips before it gives up and answers yes.
const MAX_CLIPS: usize = 4096;

/// Whether the part of `poly` outside every one of the wedges `trimmed` (by more than `out`)
/// reaches inside `planes` (by `eps`). A face beside or at the end of another edge the same
/// feature blends is trimmed back to that blend, so its part inside that edge's wedge is gone
/// (#1668).
fn reaches_inside_beyond(poly: &[Vec3], planes: &[(Vec3, f64)], eps: f64, trimmed: &[&Wedge], out: f64) -> bool {
    let mut parts = vec![poly.to_vec()];
    for w in trimmed {
        // Outside a convex wedge is outside one of its planes (pieces may overlap: only
        // whether anything is left counts).
        let mut next = Vec::new();
        for part in &parts {
            for (n, c) in &w.planes {
                let piece = clip(part, &[(-*n, -*c)], out);
                if !piece.is_empty() {
                    next.push(piece);
                }
            }
            if next.len() > MAX_CLIPS {
                return true;
            }
        }
        parts = next;
    }
    parts.iter().any(|p| reaches_inside(p, planes, eps))
}

/// Whether two sets of convex prisms overlap (by more than `eps`): a face of one reaches
/// inside the other.
fn overlap(a: &[Wedge], b: &[Wedge], eps: f64) -> bool {
    a.iter().any(|x| {
        b.iter().any(|y| y.faces.iter().any(|p| reaches_inside(p, &x.planes, eps)) || x.faces.iter().any(|p| reaches_inside(p, &y.planes, eps)))
    })
}

/// The body's straight edges between planar faces nearest the points (each once; `None` for a
/// point whose edge is something else, which the check leaves alone).
struct Selection {
    frames: Vec<Option<Frame>>,
    /// The body's triangles, with their face and whether that face is planar.
    tris: Vec<([Vec3; 3], usize)>,
    planar: Vec<bool>,
    size: f64,
    chord: f64,
    /// The body's volume and area from that mesh.
    coarse: (f64, f64),
}

fn select(body: &Body, pts: &[Vec3]) -> Result<Selection> {
    // As the blends take it: coplanar neighbours as one face, collinear pieces as one edge.
    let healed = crate::boolean_record::healed(body)?;
    crate::guard("blend fit", || select_in(&healed, pts))
}

fn select_in(body: &Body, pts: &[Vec3]) -> Result<Selection> {
    let size = body.size();
    let chord = (size * 1e-3).max(1e-4);
    let mesh = body.tessellate(chord)?;
    let solid = &*body.solid;
    let faces: Vec<&mt::Face> = solid.face_iter().collect();
    let planar: Vec<bool> = faces.iter().map(|f| plane_normal(f).is_some()).collect();
    let all = Body::unique_edges(solid);
    let mut taken: Vec<mt::EdgeID> = Vec::new();
    let mut frames = Vec::new();
    for p in pts {
        // The edge nearest the point, as the blends pick it.
        let best = mesh
            .edges
            .iter()
            .enumerate()
            .map(|(i, poly)| (i, poly.windows(2).filter_map(|w| Some(p.dist_to_segment(*w.first()?, *w.get(1)?))).fold(f64::INFINITY, f64::min)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        // Points on the same edge (pieces healed into one) count once.
        let frame = match best.and_then(|(i, dist)| all.get(i).filter(|_| dist <= size * 0.05 + 1e-3)) {
            Some(e) if taken.contains(&e.id()) => continue,
            Some(e) => {
                taken.push(e.id());
                frame(&faces, e)
            }
            None => None,
        };
        frames.push(frame);
    }
    let m = mesh.measure();
    let coarse = (m.volume, m.area);
    let tris = mesh.triangles.iter().zip(&mesh.tri_face).filter_map(|(t, f)| Some((mesh.tri(t)?, *f as usize))).collect();
    Ok(Selection { frames, tris, planar, size, chord, coarse })
}

fn frame(faces: &[&mt::Face], e: &mt::Edge) -> Option<Frame> {
    if !matches!(e.curve(), mt::Curve::Line(_)) {
        return None;
    }
    let id = e.id();
    let ends = [e.front().id(), e.back().id()];
    let (p0, p1) = (vtx(e.absolute_front()), vtx(e.absolute_back()));
    let d = (p1 - p0).normalized()?;
    // Each face along the edge: its outward normal and the direction into it (its boundary runs
    // counter-clockwise about the normal, so the face lies to the left of the edge's use).
    let mut beside = Vec::new();
    let mut allowed = Vec::new();
    let mut chained = false;
    for (i, f) in faces.iter().enumerate() {
        let mut uses: Vec<mt::Edge> = f.boundaries().iter().flat_map(|w| w.edge_iter()).filter(|x| x.id() == id).cloned().collect();
        let at_end = f.boundaries().iter().flat_map(|w| w.edge_iter()).any(|x| ends.contains(&x.front().id()) || ends.contains(&x.back().id()));
        if at_end {
            allowed.push(i);
        }
        if uses.is_empty() {
            continue;
        }
        let n = plane_normal(f)?;
        let u = uses.pop()?;
        if !uses.is_empty() {
            return None;
        }
        let dir = (vtx(u.back()) - vtx(u.front())).normalized()?;
        beside.push((n, n.cross(dir).normalized()?));
        chained |= f.boundaries().iter().flat_map(|w| w.edge_iter()).filter(|x| x.id() != id).any(|x| {
            ends.iter().any(|v| *v == x.front().id() && tangent_at(x, false).is_some_and(|t| t.dot(d).abs() > 1.0 - 1e-6))
                || ends.iter().any(|v| *v == x.back().id() && tangent_at(x, true).is_some_and(|t| t.dot(d).abs() > 1.0 - 1e-6))
        });
    }
    let [(na, ta), (nb, tb)] = beside[..] else { return None };
    // Both directions perpendicular to the edge, within their faces.
    if ta.dot(d).abs() > 1e-6 || tb.dot(d).abs() > 1e-6 {
        return None;
    }
    Some(Frame { p0, p1, ta, tb, na, nb, allowed, ends, chained })
}

/// The unit tangent of an edge (as used) at its start or end.
fn tangent_at(e: &mt::Edge, at_end: bool) -> Option<Vec3> {
    use mt::{BoundedCurve, ParametricCurve};
    let c = e.oriented_curve();
    let (t0, t1) = c.range_tuple();
    let d = c.der(if at_end { t1 } else { t0 });
    Vec3::new(d.x, d.y, d.z).normalized()
}

/// What a blend that doesn't fit runs into.
enum Clash {
    Face,
    Blend,
}

/// What a blend at `size` runs into first, if anything. Curved faces are meshed to within the
/// chord: they count when their mesh reaches into a wedge by more than twice the chord, or, when
/// `strict`, when it comes within the chord of one (the surface may then be inside it).
fn first_clash(sel: &Selection, size: Size, strict: bool) -> Option<Clash> {
    let wedges: Vec<Option<Wedge>> = sel.frames.iter().map(|f| f.as_ref().and_then(|f| f.wedge(size))).collect();
    // Planar faces are exact in the mesh; curved ones are within the chord.
    let flat = (sel.size * 1e-7).max(1e-9);
    let curved = if strict { -sel.chord } else { sel.chord * 2.0 };
    // Planar faces beside an edge lie in its wedge's side planes: outside it means further.
    let out = (sel.size * 1e-6).max(1e-8);
    for (i, (f, w)) in sel.frames.iter().zip(&wedges).enumerate() {
        let (Some(f), Some(w)) = (f, w) else { continue };
        let hit = sel.tris.iter().filter(|(_, face)| !f.allowed.contains(face)).any(|(t, face)| {
            let eps = if sel.planar.get(*face).copied().unwrap_or(false) { flat } else { curved };
            // The other blended edges this face runs along or ends them trim it back.
            let trimmed: Vec<&Wedge> = sel
                .frames
                .iter()
                .zip(&wedges)
                .enumerate()
                .filter(|(j, (fj, _))| *j != i && fj.as_ref().is_some_and(|fj| fj.allowed.contains(face)))
                .filter_map(|(_, (_, wj))| wj.as_ref())
                .collect();
            reaches_inside_beyond(t, &w.planes, eps, &trimmed, out)
        });
        if hit {
            return Some(Clash::Face);
        }
    }
    // Two blends whose edges don't meet must not overlap (those meeting at a vertex mitre).
    let eps = (sel.size * 1e-7).max(1e-9);
    for (i, (fi, wi)) in sel.frames.iter().zip(&wedges).enumerate() {
        let (Some(fi), Some(wi)) = (fi, wi) else { continue };
        for (fj, wj) in sel.frames.iter().zip(&wedges).skip(i + 1) {
            let (Some(fj), Some(wj)) = (fj, wj) else { continue };
            if fi.ends.iter().any(|v| fj.ends.contains(v)) {
                continue;
            }
            let meet = wj.faces.iter().any(|p| reaches_inside(p, &wi.planes, eps)) || wi.faces.iter().any(|p| reaches_inside(p, &wj.planes, eps));
            if meet && blends_cross(fi, wi, fj, wj, size, eps) {
                return Some(Clash::Blend);
            }
        }
    }
    None
}

/// Two blends whose wedges overlap: do they cross? Rounds on both sides of a wall (the outside
/// round's wedge reaches past the inside corner, which the inside round fills) need not: they
/// cross only where what one changes (between its edge and its arc) overlaps what the other
/// does, or where either's surface runs into the other's wedge (#1668).
fn blends_cross(fi: &Frame, wi: &Wedge, fj: &Frame, wj: &Wedge, size: Size, eps: f64) -> bool {
    let (Some(pi), Some(pj), Some(si), Some(sj)) = (fi.pieces(size), fj.pieces(size), fi.surface(size), fj.surface(size)) else { return true };
    overlap(&pi, &pj, eps) || sj.iter().any(|p| reaches_inside(p, &wi.planes, eps)) || si.iter().any(|p| reaches_inside(p, &wj.planes, eps))
}

/// The step the largest fitting size is given to: three significant digits.
fn step(v: f64) -> f64 {
    10f64.powf(v.log10().floor() - 3.0)
}

/// Why a blend of `edges` at `size` doesn't fit `body` and the largest size that does (to
/// three significant digits; `None` when not even the smallest step fits), or `None` when it
/// fits or the check can't tell (edges other than straight ones between planar faces).
fn misfit(body: &Body, edges: &[Vec3], size: Size) -> Result<Option<(Clash, Option<f64>)>> {
    if body.is_mesh() || edges.is_empty() || !size.value().is_finite() || size.value() <= 0.0 {
        return Ok(None);
    }
    misfit_in(&select(body, edges)?, size)
}

/// [`misfit`] on the selection.
fn misfit_in(sel: &Selection, size: Size) -> Result<Option<(Clash, Option<f64>)>> {
    if sel.frames.iter().all(Option::is_none) || !size.value().is_finite() || size.value() <= 0.0 {
        return Ok(None);
    }
    let Some(clash) = first_clash(sel, size, false) else { return Ok(None) };
    // Bisection on the size's steps: `hi` doesn't fit, `lo` does (or is 0). Strictly: a size
    // offered as fitting must fit, though a curved face that only touches the wedge at it would
    // not be refused (a hole whose rim the round's band just reaches, #324).
    let asked = size.value();
    let q = step(asked);
    let (mut lo, mut hi) = (0u64, (asked / q).ceil() as u64);
    for _ in 0..20 {
        if hi.saturating_sub(lo) <= 1 {
            break;
        }
        let mid = lo + (hi - lo) / 2;
        if first_clash(sel, size.scaled(mid as f64 * q / asked), true).is_none() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(Some((clash, (lo > 0).then_some(lo as f64 * q))))
}

/// "; the largest radius that fits is 5 mm", or nothing.
fn fits_note(size: Size, largest: Option<f64>) -> String {
    let unit = if matches!(size, Size::Round(_)) { "radius" } else { "size" };
    largest.map(|v| format!("; the largest {unit} that fits is {} mm", show(v))).unwrap_or_default()
}

/// Refuse a blend of `edges` at `size` that runs into other faces of `body` or into another of
/// the blends, with [`KernelError::TooLarge`] naming the largest size that fits.
pub(crate) fn check(body: &Body, edges: &[Vec3], size: Size) -> Result<()> {
    if body.is_mesh() || edges.is_empty() {
        return Ok(());
    }
    check_in(&select(body, edges)?, size)
}

/// [`check`] on the selection.
fn check_in(sel: &Selection, size: Size) -> Result<()> {
    let Some((clash, largest)) = misfit_in(sel, size)? else { return Ok(()) };
    let what = size.what();
    let into = match clash {
        Clash::Face => "it runs past the faces beside the edge into other geometry".to_string(),
        Clash::Blend => format!("the {what}s on these edges would cross each other"),
    };
    Err(KernelError::TooLarge(format!("{} mm is too large for this {what}: {into}{}", show(size.value()), fits_note(size, largest))))
}

/// A blend's refusal `e`, with the largest size that fits added when the size is why.
pub(crate) fn explain(body: &Body, edges: &[Vec3], size: Size, e: KernelError) -> KernelError {
    let note = match misfit(body, edges, size) {
        Ok(Some((_, largest @ Some(_)))) => fits_note(size, largest),
        _ => return e,
    };
    match e {
        KernelError::TooLarge(m) => KernelError::TooLarge(format!("{m}{note}")),
        KernelError::Failed(m) => KernelError::Failed(format!("{m}{note}")),
        other => other,
    }
}

/// A blend of `edges` at `size` that turned `input` into `out`, if it fits ([`check`]) and
/// changes no more than it can ([`check_volume`]).
pub(crate) fn accept(input: &Body, out: Body, edges: &[Vec3], size: Size) -> Result<Body> {
    if input.is_mesh() || out.is_mesh() || edges.is_empty() {
        return Ok(out);
    }
    let sel = select(input, edges)?;
    check_in(&sel, size)?;
    let out = volume_in(&sel, input, out, size)?;
    right_way(input, out, edges, size)
}

/// Whether the edge of `body` nearest `p` is convex (`Some(true)`: a chamfer takes material
/// away) or concave (`Some(false)`: it fills), from the two faces' outward normals and the
/// directions into each face across the edge at `p`. `None` unless that is clear-cut: faces
/// meeting at a few degrees of tangent or of a knife edge, an edge not between two faces.
fn convex_at(body: &Body, p: Vec3) -> Option<bool> {
    use mt::{BoundedCurve, ParametricCurve, SearchNearestParameter};
    let tol = (body.size() * 1e-4).max(1e-4);
    let (i, _) = body.nearest_edge(p, tol).ok()??;
    let edge = Body::unique_edges(&body.solid).into_iter().nth(i)?;
    let mut sides = Vec::new();
    for f in body.solid.face_iter() {
        for e in f.boundary_iters().into_iter().flatten().filter(|e| e.id() == edge.id()) {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let t = c.search_nearest_parameter(crate::body::p3(p), None, 100)?.clamp(t0.min(t1), t0.max(t1));
            let d = c.der(t);
            let dir = Vec3::new(d.x, d.y, d.z).normalized()?;
            let (_, n, _) = crate::curveblend::project(f, p, None)?;
            // A face's boundary runs counter-clockwise about its outward normal: its inside is
            // to the left of the edge.
            sides.push((n, n.cross(dir).normalized()?));
        }
    }
    let [(n1, t1), (n2, t2)] = sides[..] else { return None };
    // About 3°: the faces neither nearly tangent nor folded onto each other.
    const CLEAR: f64 = 0.05;
    if t1.dot(t2) > 1.0 - CLEAR || t1.dot(t2) < -1.0 + CLEAR {
        return None;
    }
    let (a, b) = (n1.dot(t2), n2.dot(t1));
    if a < -CLEAR && b < -CLEAR {
        Some(true)
    } else if a > CLEAR && b > CLEAR {
        Some(false)
    } else {
        None
    }
}

/// A chamfer of convex edges takes material away and one of concave edges fills it: a result
/// that went the other way (a chamfer on a curved edge running off a face it skims, which the
/// wedges can't bound) is refused rather than kept. Rounds are left to the wedges: a round
/// rolled along a smooth chain may run on round convex and concave stretches alike.
fn right_way(input: &Body, out: Body, edges: &[Vec3], size: Size) -> Result<Body> {
    if matches!(size, Size::Round(_)) {
        return Ok(out);
    }
    let ways: Option<Vec<bool>> = edges.iter().map(|p| convex_at(input, *p)).collect();
    let Some(ways) = ways else { return Ok(out) };
    let (convex, concave) = (ways.iter().all(|w| *w), ways.iter().all(|w| !*w));
    if !(convex || concave) {
        return Ok(out);
    }
    let (Ok(a), Ok(b)) = (crate::measure(input), crate::measure(&out)) else { return Ok(out) };
    let slack = a.volume.abs() * 1e-5 + 1e-6;
    let grew = b.volume - a.volume;
    if (convex && grew > slack) || (concave && grew < -slack) {
        return Err(KernelError::TooLarge(format!(
            "{} mm is too large for this {}: on {} edge it would {} the part by {} mm³ instead (it runs off a face next to the edge)",
            show(size.value()),
            size.what(),
            if convex { "a convex" } else { "a concave" },
            if convex { "grow" } else { "shrink" },
            // A fraction of a mm³ is typical here: to the hundredth.
            show((grew.abs() * 100.0).round() / 100.0)
        )));
    }
    Ok(out)
}

/// After a blend of `edges` at `size` turned `input` into `out`: refused when the volume taken
/// away (or added) is more than the blends' wedges hold, which only a result that ran into
/// other geometry does (#871's shelled box lost 240 mm³ more than any round could take).
#[cfg(test)]
pub(crate) fn check_volume(input: &Body, out: Body, edges: &[Vec3], size: Size) -> Result<Body> {
    if input.is_mesh() || out.is_mesh() || edges.is_empty() {
        return Ok(out);
    }
    let Ok(sel) = select(input, edges) else { return Ok(out) };
    volume_in(&sel, input, out, size)
}

/// [`check_volume`] on the selection.
fn volume_in(sel: &Selection, input: &Body, out: Body, size: Size) -> Result<Body> {
    if sel.frames.is_empty() {
        return Ok(out);
    }
    let mut bound = 0.0;
    for f in &sel.frames {
        // An edge the wedges can't describe: no bound.
        let Some(w) = f.as_ref().and_then(|f| f.wedge(size)) else { return Ok(out) };
        let Some(f) = f.as_ref().filter(|f| !f.chained) else { return Ok(out) };
        // Past an edge's ends a chamfer may still cut under a sloping end face.
        let (sa, sb) = f.setbacks(size).unwrap_or((0.0, 0.0));
        let len = f.p0.dist(f.p1);
        bound += w.volume * (len + 2.0 * sa.max(sb)) / len.max(1e-12);
    }
    // Well within the bound on the meshes the check made anyway: accepted. Otherwise measured
    // finely, as a coarse mesh of a cone or a blend is off by more than a small blend.
    if let Ok(m) = out.tessellate(sel.chord).map(|m| m.measure()) {
        let (va, aa) = sel.coarse;
        // A chord's sag over every face bounds what the meshes miss.
        let slack = (aa + m.area) * sel.chord;
        if (va - m.volume).abs() + slack <= bound * 1.02 {
            return Ok(out);
        }
    }
    let (Ok(ma), Ok(mb)) = (crate::measure(input), crate::measure(&out)) else { return Ok(out) };
    let slack = ma.volume.abs() * 1e-4 + (ma.area + mb.area) * sel.size * 1e-5;
    let changed = (ma.volume - mb.volume).abs();
    if changed > bound * 1.02 + slack {
        return Err(KernelError::TooLarge(format!(
            "{} mm is too large for this {}: the result changes {} mm³ of the part, more than the {} can (at most {} mm³)",
            show(size.value()),
            size.what(),
            show_rounded(changed),
            size.what(),
            show_rounded(bound)
        )));
    }
    Ok(out)
}

/// A length in mm, without trailing zeros.
fn show(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn show_rounded(v: f64) -> String {
    format!("{v:.0}")
}
