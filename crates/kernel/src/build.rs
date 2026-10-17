//! Building bodies: extrude and revolve planar regions, primitives.

use mt::builder;
use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2, Vec3};
use truck_modeling as mt;

use crate::body::{Body, Solid, from_p3, p3, v3};
use crate::history::FaceProv;
use crate::numbered::{self, NumberedLoop, NumberedRegion};
use crate::record::{self, Side};
use crate::{KernelError, Result, guard};

const MAX_SEGS: usize = 20_000;

fn finite(x: f64, what: &str) -> Result<f64> {
    if x.is_finite() && x.abs() < 1e7 { Ok(x) } else { Err(KernelError::Invalid(format!("{what} = {x}"))) }
}

/// A closed wire in 3D from a sketch loop, with each edge's segment and that segment's
/// position in the loop (a big arc makes two edges).
fn wire_sources(plane: &Plane, lp: &Loop2) -> Result<(mt::Wire, Vec<(mt::Edge, Seg2, usize)>)> {
    let n = lp.segs.len();
    if n == 0 || n > MAX_SEGS {
        return Err(KernelError::Invalid(format!("loop with {n} segments")));
    }
    if !lp.is_closed(1e-6) {
        return Err(KernelError::Invalid("profile loop is not closed".into()));
    }
    chain_sources(plane, &lp.segs, true)
}

/// An open wire in 3D through a chain of sketch segments, each starting where the one before
/// ends (a closed chain makes a closed wire).
pub(crate) fn chain_wire(plane: &Plane, segs: &[Seg2]) -> Result<mt::Wire> {
    chain_wire_seeds(plane, segs).map(|(wire, _)| wire)
}

/// [`chain_wire`], with each edge's id and the position in the chain of the segment it was
/// built from (a big arc makes two edges), in wire order.
pub(crate) fn chain_wire_seeds(plane: &Plane, segs: &[Seg2]) -> Result<(mt::Wire, Vec<(mt::EdgeID, usize)>)> {
    let n = segs.len();
    if n == 0 || n > MAX_SEGS {
        return Err(KernelError::Invalid(format!("curve chain with {n} segments")));
    }
    if !segs.iter().all(seg_ok) {
        return Err(KernelError::Invalid("non-finite curve".into()));
    }
    for (s, t) in segs.iter().zip(segs.iter().skip(1)) {
        if s.end().dist(t.start()) > 1e-6 {
            return Err(KernelError::Invalid("the curves are not connected end to end".into()));
        }
    }
    let closed = segs.first().zip(segs.last()).is_some_and(|(f, l)| l.end().dist(f.start()) <= 1e-6);
    let (wire, sources) = chain_sources(plane, segs, closed)?;
    Ok((wire, sources.into_iter().map(|(e, _, i)| (e.id(), i)).collect()))
}

/// The edges of a chain of segments (closed: the last one ends on the first's start vertex),
/// with each edge's segment and that segment's position in the chain (a big arc makes two
/// edges).
fn chain_sources(plane: &Plane, segs: &[Seg2], closed: bool) -> Result<(mt::Wire, Vec<(mt::Edge, Seg2, usize)>)> {
    let n = segs.len();
    let mut verts: Vec<mt::Vertex> = segs.iter().map(|s| builder::vertex(p3(plane.to_world(s.start())))).collect();
    if !closed && let Some(last) = segs.last() {
        verts.push(builder::vertex(p3(plane.to_world(last.end()))));
    }
    let mut edges = Vec::with_capacity(n);
    let mut sources = Vec::with_capacity(n);
    for (i, s) in segs.iter().enumerate() {
        let (Some(a), Some(b)) = (verts.get(i), verts.get(if closed { (i + 1) % n } else { i + 1 })) else { continue };
        for e in seg_edges(plane, s, a, b)? {
            sources.push((e.clone(), *s, i));
            edges.push(e);
        }
    }
    Ok((edges.into(), sources))
}

/// The edges of one segment from `a` to `b` (a big arc makes two).
fn seg_edges(plane: &Plane, s: &Seg2, a: &mt::Vertex, b: &mt::Vertex) -> Result<Vec<mt::Edge>> {
    Ok(match *s {
        Seg2::Line { .. } => vec![builder::line(a, b)],
        Seg2::Arc { sweep, .. } => {
            if sweep.abs() < 1e-9 {
                return Err(KernelError::Invalid("zero-length arc".into()));
            }
            // Split big arcs so every piece is well conditioned.
            if sweep.abs() > std::f64::consts::PI * 1.5 {
                let vm = builder::vertex(p3(plane.to_world(s.mid())));
                vec![
                    builder::circle_arc(a, &vm, p3(plane.to_world(s.point_at(0.25)))),
                    builder::circle_arc(&vm, b, p3(plane.to_world(s.point_at(0.75)))),
                ]
            } else {
                vec![builder::circle_arc(a, b, p3(plane.to_world(s.mid())))]
            }
        }
        Seg2::Cubic { .. } | Seg2::Conic { .. } | Seg2::Bezier { .. } => {
            vec![crate::freeform::edge(plane, s, a, b).ok_or_else(|| KernelError::Invalid("profile curve".into()))?]
        }
    })
}

/// An edge of a profile wire: the segment it was built from and the profile segments under it.
pub(crate) struct ProfileEdge {
    pub edge: mt::Edge,
    pub segment: Seg2,
    pub numbers: Vec<usize>,
}

/// A wire built from a profile loop, with its edges in wire order.
pub(crate) struct ProfileWire {
    pub wire: mt::Wire,
    pub edges: Vec<ProfileEdge>,
}

/// The wires of a region oriented as the builders build it ([`NumberedRegion::oriented`]), each
/// with what its edges were built from.
fn region_wires(plane: &Plane, r: &NumberedRegion) -> Result<Vec<ProfileWire>> {
    r.loops()
        .map(|lp| {
            let (wire, s) = wire_sources(plane, &lp.lp)?;
            let edges = s.into_iter().map(|(edge, segment, i)| ProfileEdge { edge, segment, numbers: lp.numbers(i).to_vec() }).collect();
            Ok(ProfileWire { wire, edges })
        })
        .collect()
}

/// The wires alone.
fn wires_of(profile: &[ProfileWire]) -> Vec<mt::Wire> {
    profile.iter().map(|p| p.wire.clone()).collect()
}

/// The profile edges as sweep seeds: each edge with the profile segments under it.
fn seeds(profile: &[ProfileWire]) -> Vec<(mt::EdgeID, Vec<usize>)> {
    profile.iter().flat_map(|p| &p.edges).map(|e| (e.edge.id(), e.numbers.clone())).collect()
}

/// A region numbered on its own, as given.
fn numbered_region(r: &Region2) -> Result<NumberedRegion> {
    numbered::number_regions(std::slice::from_ref(r)).pop().ok_or_else(|| KernelError::Invalid("profile".into()))
}

/// A region numbered on its own and oriented as the builders build it.
fn oriented(r: &Region2) -> Result<NumberedRegion> {
    numbered_region(r).map(|n| n.oriented())
}

/// A planar face from a region (outer loop with holes).
pub(crate) fn face(plane: &Plane, r: &Region2) -> Result<mt::Face> {
    attach_profile(&wires_of(&region_wires(plane, &oriented(r)?)?))
}

fn attach_profile(wires: &[mt::Wire]) -> Result<mt::Face> {
    guard("face", || builder::try_attach_plane(wires).map_err(|e| KernelError::Invalid(format!("profile: {e}"))))
}

fn region_ok(r: &Region2) -> Result<()> {
    if r.area().abs() < 1e-9 {
        return Err(KernelError::Invalid("profile has no area".into()));
    }
    if !std::iter::once(&r.outer).chain(&r.holes).flat_map(|l| &l.segs).all(seg_ok) {
        return Err(KernelError::Invalid("non-finite profile".into()));
    }
    Ok(())
}

/// A segment with finite, well-formed numbers.
fn seg_ok(s: &Seg2) -> bool {
    match *s {
        Seg2::Line { a, b } => a.is_finite() && b.is_finite(),
        Seg2::Arc { center, radius, start, sweep } => {
            center.is_finite() && radius.is_finite() && radius > 1e-9 && start.is_finite() && sweep.is_finite()
        }
        Seg2::Cubic { p0, p1, p2, p3 } => p0.is_finite() && p1.is_finite() && p2.is_finite() && p3.is_finite(),
        Seg2::Conic { a, apex, b, w } => a.is_finite() && apex.is_finite() && b.is_finite() && w.is_finite() && w > 1e-9,
        Seg2::Bezier { n, p } => (4..=solidcraft_geom::MAX_BEZIER_DEGREE as u8).contains(&n) && p.iter().all(|q| q.is_finite()),
    }
}

/// A finite, orthonormal sketch frame.
pub(crate) fn frame_ok(plane: &Plane) -> bool {
    plane.origin.is_finite()
        && plane.x.is_finite()
        && plane.y.is_finite()
        && (plane.x.len2() - 1.).abs() <= 1e-8
        && (plane.y.len2() - 1.).abs() <= 1e-8
        && plane.x.dot(plane.y).abs() <= 1e-8
}

/// Extrude each region from `start` to `end` along the plane normal (offsets in mm, `end > start`).
pub fn extrude(plane: &Plane, regions: &[Region2], start: f64, end: f64) -> Result<Vec<Body>> {
    if !frame_ok(plane) {
        return Err(KernelError::Invalid("extrude frame is not finite and orthonormal".into()));
    }
    let (start, end) = (finite(start, "start")?, finite(end, "end")?);
    let (lo, hi) = if end >= start { (start, end) } else { (end, start) };
    if hi - lo < 1e-6 {
        return Err(KernelError::Invalid("extrude distance is zero".into()));
    }
    if regions.is_empty() {
        return Err(KernelError::Invalid("no profile selected".into()));
    }
    let n = plane.normal();
    let mut out = Vec::new();
    for r in regions {
        region_ok(r)?;
        let base = plane.offset(lo);
        let profile = region_wires(&base, &oriented(r)?)?;
        let wires = wires_of(&profile);
        let f = guard("extrude profile", || builder::try_attach_plane(&wires).map_err(|e| KernelError::Invalid(format!("profile: {e}"))))?;
        let solid: Solid = guard("extrude", || Ok(builder::tsweep(&f, v3(n * (hi - lo)))))?;
        let body = Body::new(solid)?;
        // tsweep retains each actual input boundary edge on its generated side: one pass over
        // the faces matches topology identity (never face order, surface samples or
        // centroids) for both the face sources and the history. The profile face is the start
        // cap, the one face holding no profile edge the end cap.
        let (sources, history) = guard("extrude source identities", || {
            let by_edge: std::collections::HashMap<mt::EdgeID, &ProfileEdge> =
                profile.iter().flat_map(|p| &p.edges).map(|e| (e.edge.id(), e)).collect();
            let mut faces: Vec<(mt::FaceID, Vec<&ProfileEdge>)> = Vec::new();
            let mut visits = 0usize;
            for generated in body.solid.face_iter() {
                let mut matching = Vec::new();
                for edge in generated.absolute_boundaries().iter().flatten() {
                    visits += 1;
                    if let Some(e) = by_edge.get(&edge.id()) {
                        matching.push(*e);
                    }
                }
                faces.push((generated.id(), matching));
            }
            let mut ends = faces.iter().filter(|(id, matching)| *id != f.id() && matching.is_empty()).map(|(id, _)| *id);
            let end = match (ends.next(), ends.next()) {
                (Some(id), None) => Some(id),
                _ => None,
            };
            let upper = end.filter(|_| faces.len() <= 50_000 && visits <= 200_000);
            let small = r.outer.segs.len() + r.holes.iter().map(|hole| hole.segs.len()).sum::<usize>() <= 4096;
            let mut sources = Vec::with_capacity(faces.len());
            let mut history = Some(Vec::with_capacity(faces.len()));
            for (id, matching) in &faces {
                let (source, prov) = if *id == f.id() {
                    let source = upper.and_then(|_| {
                        small.then(|| crate::FaceSource::ExtrudeCap {
                            high: false,
                            region: r.clone(),
                            plane: Plane { origin: base.origin, x: plane.x, y: -plane.y },
                        })
                    });
                    (source, Some(record::cap(false)))
                } else if Some(*id) == upper {
                    (small.then(|| crate::FaceSource::ExtrudeCap { high: true, region: r.clone(), plane: plane.offset(hi) }), Some(record::cap(true)))
                } else if let [side] = matching.as_slice() {
                    let basis = match side.segment {
                        Seg2::Line { a, b } => {
                            let d = b - a;
                            d.normalized().and_then(|direction| {
                                let foot = a - direction * a.dot(direction);
                                Plane::new(plane.to_world(foot), plane.dir_to_world(direction), n)
                            })
                        }
                        _ => None,
                    };
                    (
                        Some(crate::FaceSource::ExtrudeSide { segment: side.segment, plane: basis, coalesced: false }),
                        Some(record::profile(&side.numbers)),
                    )
                } else if Some(*id) == end {
                    (None, Some(record::cap(true)))
                } else {
                    (None, None)
                };
                sources.push(source);
                history = history.zip(prov).map(|(mut h, p)| {
                    h.push(p);
                    h
                });
            }
            Ok((sources, history))
        })?;
        let body = body.with_face_sources(Some(std::sync::Arc::new(sources)));
        out.push(record::attach(body, &[], history, Default::default())?);
    }
    Ok(out)
}

/// Choose the per-profile axis sign whose positive rotation follows the sketch normal.
pub fn canonical_revolve_axis(plane: &Plane, region: &Region2, axis_origin: Vec2, axis_dir: Vec2) -> Result<Vec3> {
    if !axis_origin.is_finite() || !axis_dir.is_finite() || !plane.origin.is_finite() || !plane.x.is_finite() || !plane.y.is_finite() {
        return Err(KernelError::Invalid("non-finite revolve frame".into()));
    }
    if (plane.x.len2() - 1.).abs() > 1e-8 || (plane.y.len2() - 1.).abs() > 1e-8 || plane.x.dot(plane.y).abs() > 1e-8 {
        return Err(KernelError::Invalid("revolve frame is not orthonormal".into()));
    }
    region_ok(region)?;
    let direction = axis_dir.normalized().ok_or_else(|| KernelError::Invalid("revolve axis has no direction".into()))?;
    let axis = plane.dir_to_world(direction).normalized().ok_or_else(|| KernelError::Invalid("revolve frame has no axis".into()))?;
    let origin = plane.to_world(axis_origin);
    let center = plane.to_world(region.centroid());
    let radial = center - origin - axis * (center - origin).dot(axis);
    Ok(if axis.cross(radial).dot(plane.normal()) < 0. { -axis } else { axis })
}

/// Revolve each region about the sketch axis by `angle` radians.
/// This legacy API preserves its historical sweep sign behavior.
pub fn revolve(plane: &Plane, regions: &[Region2], axis_origin: Vec2, axis_dir: Vec2, angle: f64) -> Result<Vec<Body>> {
    revolved(plane, regions, axis_origin, axis_dir, angle, false)
}

/// [`revolve`] for bodies that are final (new bodies, not a boolean's tools): a partial turn's
/// planar faces come out whole too, as a full turn's always do (#726). A tool is left as swept:
/// healed, its planar faces meet a target exactly where they are tangent to it, and the
/// boolean leaves pieces there that touch. The boolean heals its own result anyway.
pub fn revolve_whole(plane: &Plane, regions: &[Region2], axis_origin: Vec2, axis_dir: Vec2, angle: f64) -> Result<Vec<Body>> {
    revolved(plane, regions, axis_origin, axis_dir, angle, true)
}

fn revolved(plane: &Plane, regions: &[Region2], axis_origin: Vec2, axis_dir: Vec2, angle: f64, whole: bool) -> Result<Vec<Body>> {
    let angle = finite(angle, "angle")?;
    if angle.abs() < 1e-6 {
        return Err(KernelError::Invalid("revolve angle is zero".into()));
    }
    let d = axis_dir.normalized().ok_or_else(|| KernelError::Invalid("revolve axis has no direction".into()))?;
    if regions.is_empty() {
        return Err(KernelError::Invalid("no profile selected".into()));
    }
    // The profile must lie on one side of the axis.
    let o = plane.to_world(axis_origin);
    let axis = plane.dir_to_world(d);
    let mut out = Vec::new();
    for r in regions {
        region_ok(r)?;
        let (mut neg, mut pos) = (false, false);
        for p in r.outer.polyline(1e-3) {
            let s = d.cross(p - axis_origin);
            neg |= s < -1e-7;
            pos |= s > 1e-7;
        }
        if neg && pos {
            return Err(KernelError::Invalid("the profile crosses the revolve axis".into()));
        }
        // Points on the axis (within solver tolerance) go exactly onto it, so the sweep does not
        // create hair-thin faces.
        let snap = |p: Vec2| {
            let along = (p - axis_origin).dot(d);
            let q = axis_origin + d * along;
            if p.dist(q) < 1e-7 { q } else { p }
        };
        let snap_loop = |l: &NumberedLoop| {
            l.map_segs(|s| match *s {
                Seg2::Line { a, b } => Seg2::Line { a: snap(a), b: snap(b) },
                arc => arc,
            })
        };
        // Full circles start at an odd angle: their seam points then don't lie in the planes
        // through the profile's centre that other bodies tend to cut along. Each new arc comes
        // from every given arc it overlaps.
        let reseam = |nl: NumberedLoop| -> NumberedLoop {
            let l = &nl.lp;
            let c = match l.segs.first() {
                Some(Seg2::Arc { center, radius, .. }) => Some((*center, *radius)),
                _ => None,
            };
            // (A circle touching the axis keeps its vertex there.)
            let touches = c.is_some_and(|(center, radius)| (d.cross(center - axis_origin).abs() - radius).abs() < 1e-7);
            match c {
                Some((center, radius))
                    if !touches
                        && l.segs.iter().all(
                            |s| matches!(s, Seg2::Arc { center: c2, radius: r2, .. } if c2.dist(center) < 1e-9 && (r2 - radius).abs() < 1e-9),
                        )
                        && (l
                            .segs
                            .iter()
                            .map(|s| match s {
                                Seg2::Arc { sweep, .. } => *sweep,
                                _ => 0.0,
                            })
                            .sum::<f64>()
                            .abs()
                            - std::f64::consts::TAU)
                            .abs()
                            < 1e-6 =>
                {
                    let ccw = l.signed_area() > 0.0;
                    let n = Loop2::circle_from(center, radius, 0.4142);
                    nl.resplit_circle(if ccw { n } else { n.reversed() })
                }
                _ => nl,
            }
        };
        // Adjacent profiles revolved together leave the axis edge split where they met: one
        // edge again, so the builders for a profile with an edge on the axis take it (swept
        // pieces of the axis are zero-area faces that booleans fail on).
        let on_axis = |p: Vec2| d.cross(p - axis_origin).abs() < 1e-9;
        let axis_run = |s: &Seg2, t: &Seg2| {
            let (a, b, c) = (s.start(), s.end(), t.end());
            on_axis(a) && on_axis(b) && on_axis(c) && (b - a).dot(c - b) > 0.0
        };
        let given = numbered_region(r)?.map_loops(|l| reseam(snap_loop(l).merged_lines(axis_run)));
        let r = &given.region();
        // A half disc on the axis revolved fully is a sphere: build it without poles. Every
        // patch is swept from the half disc's arcs.
        if angle.abs() >= std::f64::consts::TAU - 1e-9
            && r.holes.is_empty()
            && let Some((c, rad)) = half_disc(&r.outer, axis_origin, d)
        {
            let arcs: Vec<usize> = r
                .outer
                .segs
                .iter()
                .enumerate()
                .filter(|(_, s)| matches!(s, Seg2::Arc { .. }))
                .flat_map(|(i, _)| given.outer.numbers(i).to_vec())
                .collect();
            out.push(sphere_solid(plane.to_world(c), rad, [Vec3::X, Vec3::Y, Vec3::Z], &arcs)?);
            continue;
        }
        // A full turn of a profile that comes back to the axis along a curve (a half slot about
        // its centreline, a dome at the far end): truck's cone gives the faces
        // swept from it the wrong region (their pole is at the end of their curve), and cuts with
        // such a body failed (#1853). It is built as part turns are, a third of a turn at a time.
        if angle.abs() >= std::f64::consts::TAU - 1e-9
            && r.holes.is_empty()
            && pole_at_end(&r.outer, axis_origin, d)
            && let Some(b) = revolve_on_axis(plane, &given.outer, axis_origin, d, angle)?
        {
            out.push(b);
            continue;
        }
        // A full revolution of a profile with one edge on the axis: sweep the rest of the loop
        // as a cone-like shell (no zero-area faces, which booleans can't handle).
        if angle.abs() >= std::f64::consts::TAU - 1e-9
            && r.holes.is_empty()
            && let Some(b) = revolve_touching_axis(plane, &given.outer, axis_origin, d)?
        {
            out.push(b);
            continue;
        }
        let profile = region_wires(plane, &given.oriented())?;
        let f = attach_profile(&wires_of(&profile))?;
        // Partial revolves sweep toward the side the sketch normal points to (as features
        // extrude toward it).
        let c = plane.to_world(r.centroid());
        let radial = (c - o) - axis * (c - o).dot(axis);
        let toward = axis.cross(radial).dot(plane.normal());
        let angle = if toward < 0.0 { -angle } else { angle };
        // A profile with an edge on the axis: no zero-area face where that edge turns on itself.
        if r.holes.is_empty()
            && let Some(b) = revolve_on_axis(plane, &given.outer, axis_origin, d, angle)?
        {
            out.push(b);
            continue;
        }
        // Edges on the axis (a spherical shell's two radial lines, say) sweep into degenerate faces
        // there, which truck only tessellates reliably with the axis through the world origin. Off
        // it, the body measured too large and booleans with it failed or removed the wrong side.
        // Build such a profile there and move it into place, as `revolve_touching_axis` does.
        let axis_edges = r.outer.segs.iter().filter(|s| matches!(**s, Seg2::Line { a, b } if on_axis(a) && on_axis(b))).count();
        let shift = if axis_edges > 0 { o } else { Vec3::ZERO };
        let (profile, f) = if shift.len() > 0.0 {
            let local = Plane { origin: plane.origin - shift, ..*plane };
            let profile = region_wires(&local, &given.oriented())?;
            let f = attach_profile(&wires_of(&profile))?;
            (profile, f)
        } else {
            (profile, f)
        };
        let solid: Solid = guard("revolve", || Ok(builder::rsweep(&f, p3(o - shift), v3(axis), mt::Rad(angle))))?;
        let closed = angle.abs() >= std::f64::consts::TAU;
        let history = guard("revolve history", || Ok(record::swept_face(&solid, &f, &seeds(&profile), closed)))?;
        let solid = if shift.len() > 0.0 { builder::translated(&solid, v3(shift)) } else { solid };
        let (solid, history) = if closed || whole { turned_whole(solid, history) } else { (solid, history) };
        out.push(record::attach(Body::new(solid)?, &[], history, Default::default())?);
    }
    Ok(out)
}

/// A revolved solid with its planar faces whole (#726): the sweep turns each profile edge into
/// two or three patches, the planar ones on revolved surfaces, which stayed apart until a later
/// feature healed the body, so a pick on one took part of the face. Healed as booleans heal
/// their results, each face is still generated, from the profile edges of the patches merged
/// into it. The solid as it was when healing fails or can't be followed.
fn turned_whole(solid: Solid, faces: Option<Vec<FaceProv>>) -> (Solid, Option<Vec<FaceProv>>) {
    let Some(given) = faces.as_ref() else { return (solid, faces) };
    let Ok(size) = Body::new(solid.clone()).map(|b| b.size()) else { return (solid, faces) };
    let healed = guard("heal revolve", || {
        let (healed, map) = crate::heal::heal_traced(solid.clone(), size, &[]);
        let failed = || KernelError::Failed("heal revolve".into());
        if !crate::boolean_record::edges_evaluate(&healed) {
            return Err(failed());
        }
        let carried = map
            .ok_or_else(failed)?
            .faces
            .iter()
            .map(|under| {
                let mut origins: Vec<_> = under.iter().map(|i| given.get(*i).map(|f| f.origins.clone())).collect::<Option<Vec<_>>>()?.concat();
                origins.sort();
                origins.dedup();
                Some(FaceProv { origins, change: crate::history::Change::Generated })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(failed)?;
        Ok((healed, carried))
    });
    match healed {
        Ok((healed, carried)) => (healed, Some(carried)),
        Err(_) => (solid, faces),
    }
}

/// Axis-aligned box between two corners.
pub fn box_solid(a: Vec3, b: Vec3) -> Result<Body> {
    let (lo, hi) = (a.min(b), a.max(b));
    let s = hi - lo;
    if !(s.x > 1e-6 && s.y > 1e-6 && s.z > 1e-6 && s.is_finite() && s.len() < 1e7) {
        return Err(KernelError::Invalid("box needs a positive size in x, y and z".into()));
    }
    let r = Region2 {
        outer: Loop2::polygon(&[Vec2::new(lo.x, lo.y), Vec2::new(hi.x, lo.y), Vec2::new(hi.x, hi.y), Vec2::new(lo.x, hi.y)]),
        holes: vec![],
    };
    let mut v = extrude(&Plane::XY, &[r], lo.z, hi.z)?;
    let body = v.pop().ok_or_else(|| KernelError::Failed("box".into()))?;
    let sources = body.face_sources().ok_or_else(|| KernelError::Failed("box construction source".into()))?;
    let mut roles = std::collections::HashSet::new();
    let mut certified = Vec::new();
    for source in sources {
        let role = match source {
            Some(crate::FaceSource::ExtrudeCap { high, .. }) => Some((2u8, *high)),
            Some(crate::FaceSource::ExtrudeSide { segment: Seg2::Line { a, b }, .. }) if a.x == b.x && a.x == lo.x => Some((0, false)),
            Some(crate::FaceSource::ExtrudeSide { segment: Seg2::Line { a, b }, .. }) if a.x == b.x && a.x == hi.x => Some((0, true)),
            Some(crate::FaceSource::ExtrudeSide { segment: Seg2::Line { a, b }, .. }) if a.y == b.y && a.y == lo.y => Some((1, false)),
            Some(crate::FaceSource::ExtrudeSide { segment: Seg2::Line { a, b }, .. }) if a.y == b.y && a.y == hi.y => Some((1, true)),
            _ => None,
        }
        .ok_or_else(|| KernelError::Failed("box role correspondence".into()))?;
        if !roles.insert(role) {
            return Err(KernelError::Failed("duplicate box role".into()));
        }
        let (axis, high) = role;
        let sign = if high { 1. } else { -1. };
        let plane = match axis {
            0 => Plane { origin: Vec3::X * if high { hi.x } else { lo.x }, x: Vec3::Y, y: Vec3::Z * sign },
            1 => Plane { origin: Vec3::Y * if high { hi.y } else { lo.y }, x: Vec3::X, y: -Vec3::Z * sign },
            _ => Plane { origin: Vec3::Z * if high { hi.z } else { lo.z }, x: Vec3::X, y: Vec3::Y * sign },
        };
        certified.push(Some(crate::FaceSource::Box { producer: None, axis, high, plane }));
    }
    if roles.len() != 6 {
        return Err(KernelError::Failed("box role count".into()));
    }
    Ok(body.with_face_sources(Some(std::sync::Arc::new(certified))))
}

/// Where primitives start their circles (radians from the frame's x axis).
pub(crate) const SEAM_ANGLE: f64 = 0.6131;

/// Cylinder with base centre `base`, axis direction `axis`, radius and height.
pub fn cylinder(base: Vec3, axis: Vec3, radius: f64, height: f64) -> Result<Body> {
    let plane = Plane::from_normal(base, axis).ok_or_else(|| KernelError::Invalid("cylinder axis".into()))?;
    let radius = finite(radius, "radius")?;
    if radius <= 1e-6 {
        return Err(KernelError::Invalid("radius must be positive".into()));
    }
    // The seams at an odd angle: on the frame's axes they would lie exactly where faces placed
    // on a grid cut the cylinder (along a seam edge, which booleans don't survive).
    let r = Region2 { outer: Loop2::circle_from(Vec2::ZERO, radius, SEAM_ANGLE), holes: vec![] };
    let mut v = extrude(&plane, &[r], 0.0, finite(height, "height")?)?;
    v.pop().ok_or_else(|| KernelError::Failed("cylinder".into()))
}

/// Sphere: revolve a half disc.
pub fn sphere(center: Vec3, radius: f64) -> Result<Body> {
    sphere_patches(center, radius)
}

/// Torus about the Z axis through `center`.
pub fn torus(center: Vec3, major: f64, minor: f64) -> Result<Body> {
    let (major, minor) = (finite(major, "major radius")?, finite(minor, "minor radius")?);
    if !(minor > 1e-6 && major > minor) {
        return Err(KernelError::Invalid("torus needs major > minor > 0".into()));
    }
    let plane = Plane { origin: center, ..Plane::XZ };
    let r = Region2 { outer: Loop2::circle(Vec2::new(major, 0.0), minor), holes: vec![] };
    let mut v = revolve(&plane, &[r], Vec2::ZERO, Vec2::Y, std::f64::consts::TAU)?;
    v.pop().ok_or_else(|| KernelError::Failed("torus".into()))
}

/// Offset every segment of a loop to its right by `d` (outward for a region's outer loop and
/// holes alike), each segment as one of the same kind (`None`: a line the offset swallows, whose
/// neighbours now meet at one point). Loops of arcs on one circle, and loops of lines, arcs and
/// free curves (spline and conic pieces, such as the outlines of text) where the arcs meet their
/// neighbours tangentially are supported.
fn offset_loop(lp: &Loop2, d: f64) -> Result<Vec<Option<Seg2>>> {
    if d == 0.0 {
        return Ok(lp.segs.iter().copied().map(Some).collect());
    }
    let all_arcs = lp.segs.iter().all(|s| matches!(s, Seg2::Arc { .. }));
    if all_arcs {
        return lp
            .segs
            .iter()
            .map(|s| match *s {
                Seg2::Arc { center, radius, start, sweep } => {
                    let r = radius + sweep.signum() * d;
                    if r > 1e-6 {
                        Ok(Some(Seg2::Arc { center, radius: r, start, sweep }))
                    } else {
                        Err(KernelError::Invalid("the taper closes the profile".into()))
                    }
                }
                _ => Err(KernelError::Failed("taper: profile loop".into())),
            })
            .collect();
    }
    offset_segments(lp, d)
}

/// Where the line through `p` along `tin` and the one through `q` along `tout` meet once each
/// has moved `d` to its right (`q` moved, where they run on together).
fn mitre(p: Vec2, tin: Vec2, q: Vec2, tout: Vec2, d: f64) -> Vec2 {
    let right = |t: Vec2| Vec2::new(t.y, -t.x);
    let (p0, p1) = (p + right(tin) * d, q + right(tout) * d);
    let den = tin.cross(tout);
    if den.abs() < 1e-12 { p1 } else { p0 + tin * ((p1 - p0).cross(tout) / den) }
}

/// A free curve's control polygon moved `d` to its right, from `a` to `b` (where its moved
/// neighbours meet it): each leg moves `d` to its right and each inner control point goes where
/// its two moved legs meet. For a curve that turns little against `d` this stays close to the
/// true offset, and its ends run along the curve's end tangents, so its side meets its
/// neighbours' along straight rulings.
fn offset_polygon(ctrl: &[Vec2], a: Vec2, b: Vec2, d: f64) -> Option<Vec<Vec2>> {
    let legs: Vec<Option<Vec2>> = ctrl.iter().zip(ctrl.iter().skip(1)).map(|(p, q)| (*q - *p).normalized()).collect();
    // A leg of no length takes the direction of the nearest leg after it, else before it.
    let dirs: Vec<Vec2> = (0..legs.len())
        .map(|i| legs.iter().skip(i).flatten().next().or_else(|| legs.iter().take(i).rev().flatten().next()).copied())
        .collect::<Option<_>>()?;
    let m = ctrl.len();
    let mut out = Vec::with_capacity(m);
    for (i, p) in ctrl.iter().enumerate() {
        out.push(if i == 0 {
            a
        } else if i + 1 == m {
            b
        } else {
            mitre(*p, *dirs.get(i - 1)?, *p, *dirs.get(i)?, d)
        });
    }
    out.iter().all(|p| p.is_finite()).then_some(out)
}

/// Samples a moved free curve is fitted to, and checked against.
const OFFSET_SAMPLES: usize = 16;

/// Where the point at `t` of the free curve `s` goes once moved `d` to its right (along its
/// normal there). `None` where it has no tangent.
fn true_offset(s: &Seg2, t: f64, d: f64) -> Option<Vec2> {
    let tangent = s.derivative(t).normalized()?;
    Some(s.point_at(t) + Vec2::new(tangent.y, -tangent.x) * d)
}

/// The free curve `s` moved `d` to its right, as one curve of its kind from `a` to `b` (where
/// its moved neighbours meet it; its own moved ends where it runs on into them). It is fitted to
/// the true offset between the curve's own moved ends: a cubic keeps its end tangents and takes
/// the handle lengths that fit best (least squares over [`OFFSET_SAMPLES`] points); a conic or a
/// higher Bézier moves its control polygon ([`offset_polygon`]), its middle then put on the true
/// offset. Its ends then go to `a` and `b`, along its end tangents (where moved neighbours meet
/// it), their handles kept pointing the same way. Close for a curve that turns little against
/// `d`; [`offset_splits`] cuts the others into such pieces first.
fn offset_curve(s: &Seg2, a: Vec2, b: Vec2, d: f64) -> Option<Seg2> {
    let (n0, n1) = (true_offset(s, 0.0, d)?, true_offset(s, 1.0, d)?);
    let (t0, t3) = (s.start_tangent(), s.end_tangent());
    let middle = |moved: &Seg2| Some(true_offset(s, 0.5, d)? - moved.point_at(0.5));
    // The control points fitted between the natural ends.
    let mut ctrl: Vec<Vec2> = match *s {
        Seg2::Cubic { p0, p1, p2, p3 } => {
            // The moved cubic n0·(B0+B1) + n1·(B2+B3) + α·t0·B1 − β·t3·B2: linear in α, β.
            let (mut m, mut r) = ([[0.0f64; 2]; 2], [0.0f64; 2]);
            for k in 1..OFFSET_SAMPLES {
                let t = k as f64 / OFFSET_SAMPLES as f64;
                let u = 1.0 - t;
                let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                let rest = true_offset(s, t, d)? - (n0 * (b0 + b1) + n1 * (b2 + b3));
                let (c1, c2) = (t0 * b1, t3 * -b2);
                m[0][0] += c1.dot(c1);
                m[0][1] += c1.dot(c2);
                m[1][1] += c2.dot(c2);
                r[0] += c1.dot(rest);
                r[1] += c2.dot(rest);
            }
            let det = m[0][0] * m[1][1] - m[0][1] * m[0][1];
            let (alpha, beta) = if det.abs() > 1e-12 * (m[0][0] * m[1][1]).max(1e-30) {
                ((r[0] * m[1][1] - r[1] * m[0][1]) / det, (m[0][0] * r[1] - m[0][1] * r[0]) / det)
            } else {
                (f64::NAN, f64::NAN)
            };
            if alpha.is_finite() && beta.is_finite() && alpha > 0.0 && beta > 0.0 {
                vec![n0, n0 + t0 * alpha, n1 - t3 * beta, n1]
            } else {
                // Handles that would turn back: the moved control polygon, its middle on the offset.
                let mut q = offset_polygon(&[p0, p1, p2, p3], n0, n1, d)?;
                let [q0, q1, q2, q3] = q[..] else { return None };
                let shift = middle(&Seg2::Cubic { p0: q0, p1: q1, p2: q2, p3: q3 })? * (8.0 / 6.0);
                for point in q.iter_mut().take(3).skip(1) {
                    *point += shift;
                }
                q
            }
        }
        Seg2::Conic { a: c0, apex, b: c1, w } => {
            let mut q = offset_polygon(&[c0, apex, c1], n0, n1, d)?;
            let [q0, qa, q1] = q[..] else { return None };
            // The middle is (a + 2w·apex + b) / (2 + 2w).
            let shift = middle(&Seg2::Conic { a: q0, apex: qa, b: q1, w })? * ((2.0 + 2.0 * w) / (2.0 * w));
            if let Some(apex) = q.get_mut(1) {
                *apex += shift;
            }
            q
        }
        Seg2::Bezier { n, p } => {
            let k = (n as usize).clamp(1, solidcraft_geom::MAX_BEZIER_DEGREE);
            let mut q = offset_polygon(p.get(..=k)?, n0, n1, d)?;
            let mut all = p;
            for (dst, src) in all.iter_mut().zip(&q) {
                *dst = *src;
            }
            // The middle weighs the inner points by binomials summing to 2^k − 2.
            let shift = middle(&Seg2::Bezier { n, p: all })? * (2f64.powi(k as i32) / (2f64.powi(k as i32) - 2.0));
            for point in q.iter_mut().take(k).skip(1) {
                *point += shift;
            }
            q
        }
        Seg2::Line { .. } | Seg2::Arc { .. } => return None,
    };
    // The ends to where the neighbours meet; a handle the move would turn round goes with it.
    let last = ctrl.len().checked_sub(1)?;
    if last < 2 {
        return None;
    }
    let (ea, eb) = (a - n0, b - n1);
    if let Some(h) = ctrl.get_mut(1)
        && (*h - a).dot(t0) <= 0.0
    {
        *h += ea;
    }
    if let Some(h) = ctrl.get_mut(last - 1)
        && (b - *h).dot(t3) <= 0.0
    {
        *h += eb;
    }
    if let Some(first) = ctrl.first_mut() {
        *first = a;
    }
    if let Some(end) = ctrl.last_mut() {
        *end = b;
    }
    Some(match (*s, &ctrl[..]) {
        (Seg2::Cubic { .. }, &[p0, p1, p2, p3]) => Seg2::Cubic { p0, p1, p2, p3 },
        (Seg2::Conic { w, .. }, &[a, apex, b]) => Seg2::Conic { a, apex, b, w },
        (Seg2::Bezier { n, p }, q) => {
            let mut all = p;
            for (dst, src) in all.iter_mut().zip(q) {
                *dst = *src;
            }
            Seg2::Bezier { n, p: all }
        }
        _ => return None,
    })
}

/// How far a moved free curve strays from the free curve `s`'s true offset by `d`: the largest
/// distance from its points to the true offset's (on polylines of [`OFFSET_SAMPLES`] spans).
pub(crate) fn offset_error(s: &Seg2, d: f64) -> Option<f64> {
    let (a, b) = (true_offset(s, 0.0, d)?, true_offset(s, 1.0, d)?);
    let moved = offset_curve(s, a, b, d)?;
    let n = OFFSET_SAMPLES * 2;
    let truth: Vec<Vec2> = (0..=n).map(|k| true_offset(s, k as f64 / n as f64, d)).collect::<Option<_>>()?;
    let near = |p: Vec2| {
        truth
            .iter()
            .zip(truth.iter().skip(1))
            .map(|(x, y)| Seg2::Line { a: *x, b: *y })
            .map(|l| l.point_at(l.closest_param(p)).dist(p))
            .fold(f64::INFINITY, f64::min)
    };
    Some((0..=n).map(|k| near(moved.point_at(k as f64 / n as f64))).fold(0.0, f64::max))
}

/// How close a moved free curve is kept to its true offset by `d`: 2 % of it, or 10 µm (a
/// slope's offset is a fraction of a millimetre on text: finer would cut letters into many
/// pieces, and every piece a face the booleans meet).
pub(crate) fn offset_tolerance(d: f64) -> f64 {
    (d.abs() * 2e-2).max(1e-2)
}

/// Deepest halving of a free curve in [`offset_splits`] (at most 2^6 pieces).
const MAX_OFFSET_DEPTH: u32 = 6;

/// Whether the free curve `s` bends anywhere tighter than `d` (its radius of curvature below
/// |d|, on [`OFFSET_SAMPLES`] points): moved that far towards its centre, its true offset folds
/// over itself there.
fn tighter_than(s: &Seg2, d: f64) -> bool {
    let h = 1e-4;
    (0..=OFFSET_SAMPLES).any(|k| {
        let t = (k as f64 / OFFSET_SAMPLES as f64).clamp(h, 1.0 - h);
        let (v, a) = (s.derivative(t), (s.derivative(t + h) - s.derivative(t - h)) * (0.5 / h));
        let speed = v.len();
        speed > 1e-12 && v.cross(a).abs() * d.abs() >= speed.powi(3)
    })
}

/// The loop `lp` with each free curve cut into pieces whose offset by up to `d` (either way)
/// [`offset_curve`] fits within a small part of `d`, and the segment of `lp` each piece comes
/// from. Lines and arcs stay whole, and so does a piece bending tighter than `d`
/// ([`tighter_than`]): its true offset folds there, and a piece following it would carry the
/// fold, where one moved piece smooths it over.
pub(crate) fn offset_splits(lp: &Loop2, d: f64) -> (Loop2, Vec<usize>) {
    let tol = offset_tolerance(d);
    let mut segs = Vec::with_capacity(lp.segs.len());
    let mut from = Vec::with_capacity(lp.segs.len());
    for (i, s) in lp.segs.iter().enumerate() {
        if s.is_line_or_arc() || d == 0.0 {
            segs.push(*s);
            from.push(i);
            continue;
        }
        // Spans still to check, the last pushed checked first: the right half goes in first.
        let mut todo = vec![(*s, 0u32)];
        while let Some((piece, depth)) = todo.pop() {
            let close = |d: f64| offset_error(&piece, d).is_some_and(|e| e <= tol);
            if depth < MAX_OFFSET_DEPTH && !tighter_than(&piece, d) && !(close(d) && close(-d)) {
                let (left, right) = piece.split_at(0.5);
                todo.push((right, depth + 1));
                todo.push((left, depth + 1));
                continue;
            }
            segs.push(piece);
            from.push(i);
        }
    }
    (Loop2 { segs }, from)
}

/// [`offset_loop`] for a loop of lines, arcs and free curves. Each arc keeps its centre and
/// angles and its radius moves by `d` (so its side stays an exact cone), each line moves `d` to
/// its right, and each free curve moves its control polygon ([`offset_polygon`]). Where two
/// moved segments part (a corner the offset goes round) they are carried on along their end
/// tangents to where they meet; where they run into each other (a corner the offset goes into)
/// both are cut where they cross. A line shorter than the offset can turn around between its
/// moved neighbours (the flat tip of a letter's stroke): it goes, and its neighbours meet. An arc
/// has to meet its neighbours tangentially: at a corner its side would meet the next one along a
/// conic, not a straight ruling.
fn offset_segments(lp: &Loop2, d: f64) -> Result<Vec<Option<Seg2>>> {
    let closes = || KernelError::Invalid("the taper closes the profile".into());
    let at_corner = || KernelError::Failed("not supported yet: tapered profiles where an arc meets another edge at a corner".into());
    let bad = || KernelError::Failed("taper: profile loop".into());
    let right = |t: Vec2| Vec2::new(t.y, -t.x);
    let n = lp.segs.len();
    let moved: Vec<Option<Seg2>> = lp
        .segs
        .iter()
        .map(|s| match *s {
            Seg2::Arc { center, radius, start, sweep } => {
                let r = radius + sweep.signum() * d;
                if r > 1e-6 { Ok(Some(Seg2::Arc { center, radius: r, start, sweep })) } else { Err(closes()) }
            }
            _ => Ok(None),
        })
        .collect::<Result<_>>()?;
    let mut gone = vec![false; n];
    // Each round takes away at least one line, or ends.
    let pieces = loop {
        let kept: Vec<usize> = (0..n).filter(|&i| !gone.get(i).copied().unwrap_or(true)).collect();
        if kept.len() < 2 {
            return Err(closes());
        }
        // Where each kept segment starts and ends once moved, and the corners where two moved
        // segments run into each other (the one before, the one after).
        let (mut starts, mut ends) = (vec![Vec2::ZERO; n], vec![Vec2::ZERO; n]);
        let mut into: Vec<(usize, usize)> = Vec::new();
        for (j, &i) in kept.iter().enumerate() {
            let Some(&k) = kept.get((j + kept.len() - 1) % kept.len()) else { return Err(bad()) };
            let (Some(prev), Some(s), Some(prev_moved), Some(this_moved)) = (lp.segs.get(k), lp.segs.get(i), moved.get(k), moved.get(i)) else {
                return Err(bad());
            };
            let (tin, tout) = (prev.end_tangent(), s.start_tangent());
            let adjacent = (k + 1) % n == i;
            let turn = tin.cross(tout);
            let tangent = adjacent && turn.abs() < 1e-6 && tin.dot(tout) > 0.0;
            let (end, start) = match (prev_moved, this_moved) {
                (Some(a), _) if tangent => (a.end(), a.end()),
                (None, Some(b)) if tangent => (b.start(), b.start()),
                // Two sides running back towards each other across a gone tip: the stroke is
                // thinner than the taper.
                (None, None) if !adjacent && turn.abs() < 1e-9 => return Err(closes()),
                // Running on smoothly (the pieces of one curve): both move to the same point.
                // (The tangents' crossing is ill-conditioned there.)
                (None, None) if tangent => {
                    let m = s.start() + right(tout) * d;
                    (m, m)
                }
                (None, None) if turn * d < 0.0 => {
                    into.push((k, i));
                    (prev.end() + right(tin) * d, s.start() + right(tout) * d)
                }
                (None, None) => {
                    let m = mitre(prev.end(), tin, s.start(), tout, d);
                    (m, m)
                }
                _ => return Err(at_corner()),
            };
            if let (Some(e), Some(st)) = (ends.get_mut(k), starts.get_mut(i)) {
                (*e, *st) = (end, start);
            }
        }
        let mut pieces: Vec<Option<Seg2>> = vec![None; n];
        for &i in &kept {
            let (Some(s), Some(m), Some(&a), Some(&b)) = (lp.segs.get(i), moved.get(i), starts.get(i), ends.get(i)) else { return Err(bad()) };
            let piece = match (m, *s) {
                (Some(arc), _) => {
                    if arc.start().dist(a) > 1e-6 || arc.end().dist(b) > 1e-6 {
                        return Err(at_corner());
                    }
                    *arc
                }
                (None, Seg2::Line { .. }) => Seg2::Line { a, b },
                (None, Seg2::Arc { .. }) => return Err(bad()),
                (None, curve) => offset_curve(&curve, a, b, d).ok_or_else(closes)?,
            };
            if let Some(slot) = pieces.get_mut(i) {
                *slot = Some(piece);
            }
        }
        // Cut the two pieces at each corner the offset goes into where they cross; pieces that
        // don't cross meet where their end tangents do.
        for &(k, i) in &into {
            let (Some(Some(p)), Some(Some(q))) = (pieces.get(k).copied(), pieces.get(i).copied()) else { return Err(bad()) };
            let (p, q) = match meet(&p, &q) {
                Some((tp, tq, x)) => (with_end(&p.split_at(tp).0, x), with_start(&q.split_at(tq).1, x)),
                None => {
                    let x = mitre(p.end(), p.end_tangent(), q.start(), q.start_tangent(), 0.0);
                    (with_end(&p, x), with_start(&q, x))
                }
            };
            if let Some(a) = pieces.get_mut(k) {
                *a = Some(p);
            }
            if let Some(b) = pieces.get_mut(i) {
                *b = Some(q);
            }
        }
        let mut more = false;
        for &i in &kept {
            // A line that turned around has been swallowed by its neighbours.
            if let (Some(Seg2::Line { a: s0, b: s1 }), Some(Some(Seg2::Line { a, b }))) = (lp.segs.get(i), pieces.get(i))
                && (*b - *a).dot(*s1 - *s0) <= 0.0
                && let Some(g) = gone.get_mut(i)
            {
                *g = true;
                more = true;
            }
        }
        if !more {
            break pieces;
        }
    };
    let out = Loop2 { segs: pieces.iter().flatten().copied().collect() };
    if out.signed_area().signum() != lp.signed_area().signum() || out.signed_area().abs() < 1e-9 {
        return Err(closes());
    }
    Ok(pieces)
}

/// Where the end of `p` crosses the start of `q`, nearest the corner between them: the
/// parameters on each and the point. On polylines of a few dozen points each, the point taken
/// on a line where one is, else half way between the two crossing pieces.
///
/// Its limit: curves are sampled at [`MEET_SAMPLES`] points, so two that cross twice within one
/// sample span (a tight curve against its neighbour) can be missed. The corner then gets the
/// mitre of the end tangents instead, and the loop that makes, if it crosses itself, is refused
/// by [`offset_crosses`] rather than built.
fn meet(p: &Seg2, q: &Seg2) -> Option<(f64, f64, Vec2)> {
    const SAMPLES: usize = MEET_SAMPLES;
    let samples = |s: &Seg2| -> Vec<(f64, Vec2)> {
        let n = if matches!(s, Seg2::Line { .. }) { 1 } else { SAMPLES };
        (0..=n).map(|k| k as f64 / n as f64).map(|t| (t, s.point_at(t))).collect()
    };
    let (sp, sq) = (samples(p), samples(q));
    let mut best: Option<(f64, f64)> = None;
    for (a, b) in sp.iter().zip(sp.iter().skip(1)) {
        for (c, e) in sq.iter().zip(sq.iter().skip(1)) {
            let (r, s) = (b.1 - a.1, e.1 - c.1);
            let den = r.cross(s);
            if den.abs() < 1e-15 {
                continue;
            }
            let (u, v) = ((c.1 - a.1).cross(s) / den, (c.1 - a.1).cross(r) / den);
            if !((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)) {
                continue;
            }
            let (tp, tq) = (a.0 + (b.0 - a.0) * u, c.0 + (e.0 - c.0) * v);
            // Nearest the corner: little of `p` left behind its crossing, little of `q` before.
            if best.is_none_or(|(bp, bq)| (1.0 - tp) + tq < (1.0 - bp) + bq) {
                best = Some((tp, tq));
            }
        }
    }
    let (tp, tq) = best?;
    // Not at an end: a piece cut to nothing has no length left to rule.
    if tp <= 1e-9 || tq >= 1.0 - 1e-9 {
        return None;
    }
    // On a line where one meets it, so a line stays on its line (its side stays flat).
    let x = match (p, q) {
        (Seg2::Line { .. }, _) => p.point_at(tp),
        (_, Seg2::Line { .. }) => q.point_at(tq),
        _ => (p.point_at(tp) + q.point_at(tq)) * 0.5,
    };
    Some((tp, tq, x))
}

/// `s` ending at `x` (its last control point moved there).
fn with_end(s: &Seg2, x: Vec2) -> Seg2 {
    match *s {
        Seg2::Line { a, .. } => Seg2::Line { a, b: x },
        Seg2::Cubic { p0, p1, p2, .. } => Seg2::Cubic { p0, p1, p2, p3: x },
        Seg2::Conic { a, apex, w, .. } => Seg2::Conic { a, apex, b: x, w },
        Seg2::Bezier { n, p } => {
            let mut q = p;
            if let Some(last) = q.get_mut((n as usize).clamp(1, solidcraft_geom::MAX_BEZIER_DEGREE)) {
                *last = x;
            }
            Seg2::Bezier { n, p: q }
        }
        Seg2::Arc { .. } => *s,
    }
}

/// `s` starting at `x` (its first control point moved there).
fn with_start(s: &Seg2, x: Vec2) -> Seg2 {
    match *s {
        Seg2::Line { b, .. } => Seg2::Line { a: x, b },
        Seg2::Cubic { p1, p2, p3, .. } => Seg2::Cubic { p0: x, p1, p2, p3 },
        Seg2::Conic { apex, b, w, .. } => Seg2::Conic { a: x, apex, b, w },
        Seg2::Bezier { n, p } => {
            let mut q = p;
            if let Some(first) = q.first_mut() {
                *first = x;
            }
            Seg2::Bezier { n, p: q }
        }
        Seg2::Arc { .. } => *s,
    }
}

/// Points a free curve is sampled at by [`meet`].
const MEET_SAMPLES: usize = 48;

/// Most pairs of polyline pieces [`offset_crosses`] compares before it gives up.
const CROSSING_BUDGET: usize = 20_000_000;

/// Whether the offset loops of a region cross themselves or each other, on polylines within
/// `tol` of them: the taper is too steep for the profile's details (a stroke thinner than the
/// taper, a curve tighter than it, two loops it brings together). `None` when there is too
/// much to compare within [`CROSSING_BUDGET`]: then the offset is not known to be sound.
fn offset_crosses(loops: &[Vec<Option<Seg2>>], tol: f64) -> Option<bool> {
    let lines: Vec<Vec<Vec2>> = loops
        .iter()
        .map(|l| {
            let mut pts: Vec<Vec2> = Vec::new();
            for s in l.iter().flatten() {
                let p = s.polyline(tol);
                pts.extend(p.iter().skip(if pts.is_empty() { 0 } else { 1 }));
            }
            pts
        })
        .collect();
    // Each piece: its loop, its place in the loop, its ends.
    let mut pieces: Vec<(usize, usize, usize, Vec2, Vec2)> = Vec::new();
    for (k, pts) in lines.iter().enumerate() {
        for (i, (a, b)) in pts.iter().zip(pts.iter().skip(1)).enumerate() {
            pieces.push((k, i, pts.len() - 1, *a, *b));
        }
    }
    pieces.sort_by(|p, q| p.3.x.min(p.4.x).total_cmp(&q.3.x.min(q.4.x)));
    let side = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    let mut work = 0usize;
    for (n, &(k, i, len, a, b)) in pieces.iter().enumerate() {
        let right = a.x.max(b.x);
        for &(k2, i2, _, c, d) in pieces.iter().skip(n + 1) {
            if c.x.min(d.x) > right {
                break;
            }
            work += 1;
            if work > CROSSING_BUDGET {
                return None;
            }
            // Neighbours in one loop share an end.
            if k == k2 && (i.abs_diff(i2) <= 1 || i.abs_diff(i2) == len.saturating_sub(1)) {
                continue;
            }
            if a.y.max(b.y) < c.y.min(d.y) || c.y.max(d.y) < a.y.min(b.y) {
                continue;
            }
            let (d1, d2, d3, d4) = (side(c, d, a), side(c, d, b), side(a, b, c), side(a, b, d));
            if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
                return Some(true);
            }
        }
    }
    Some(false)
}

/// The side faces of one loop of a tapered extrude through `levels` of it, bottom to top (the
/// loop offset in a plane, one entry per profile segment, `None` where the offset swallowed a
/// line), with the bottom and top wires. Between two levels each side is ruled between a
/// segment's two offsets, through straight edges shared with its neighbours; a line swallowed
/// at one level makes a flat triangle up to where its neighbours meet. `numbers`: the profile
/// segments under each segment.
fn taper_loop(levels: &[(Plane, &[Option<Seg2>])], numbers: impl Fn(usize) -> Vec<usize>) -> Result<(Vec<Side>, mt::Wire, mt::Wire)> {
    let bad = || KernelError::Failed("taper: profile loop".into());
    let n = levels.first().map_or(0, |(_, segs)| segs.len());
    if levels.len() < 2 || levels.iter().any(|(_, segs)| segs.len() != n) {
        return Err(bad());
    }
    // Each level's vertices (each segment's start: its own, or for a swallowed one the next kept
    // one's) and each kept segment's edges.
    type Level = (Vec<mt::Vertex>, Vec<Option<Vec<mt::Edge>>>);
    let built: Vec<Level> = levels
        .iter()
        .map(|(plane, segs)| {
            let own: Vec<Option<mt::Vertex>> = segs.iter().map(|s| s.map(|s| builder::vertex(p3(plane.to_world(s.start()))))).collect();
            let v: Vec<mt::Vertex> =
                (0..n).map(|i| (0..n).find_map(|k| own.get((i + k) % n).cloned().flatten()).ok_or_else(bad)).collect::<Result<_>>()?;
            let edges = (0..n)
                .map(|i| match (segs.get(i).copied().flatten(), v.get(i), v.get((i + 1) % n)) {
                    (Some(s), Some(a), Some(b)) => seg_edges(plane, &s, a, b).map(Some),
                    _ => Ok(None),
                })
                .collect::<Result<_>>()?;
            Ok((v, edges))
        })
        .collect::<Result<_>>()?;
    let mut rulings: std::collections::HashMap<(mt::VertexID, mt::VertexID), mt::Edge> = std::collections::HashMap::new();
    let mut ruling = |a: &mt::Vertex, b: &mt::Vertex| rulings.entry((a.id(), b.id())).or_insert_with(|| builder::line(a, b)).clone();
    let flat = |wire: mt::Wire| builder::try_attach_plane(&[wire]).map_err(|e| KernelError::Failed(format!("taper sides: {e}")));
    let mut sides = Vec::new();
    for (((bv, bottom), (tv, top)), (low_segs, high_segs)) in
        built.iter().zip(built.iter().skip(1)).zip(levels.iter().map(|(_, s)| s).zip(levels.iter().skip(1).map(|(_, s)| s)))
    {
        for i in 0..n {
            let j = (i + 1) % n;
            let (Some(b0), Some(b1), Some(t0), Some(t1)) = (bv.get(i), bv.get(j), tv.get(i), tv.get(j)) else { return Err(bad()) };
            let prov = Some(record::profile(&numbers(i)));
            match (bottom.get(i).and_then(Option::as_ref), top.get(i).and_then(Option::as_ref)) {
                (Some(low), Some(high)) => {
                    if low.len() != high.len() {
                        return Err(bad());
                    }
                    // A line and its moved copy run side by side: their side is a plane, kept
                    // one (a flat side the booleans meet exactly, however slight the taper).
                    let lines = matches!(
                        (low_segs.get(i).copied().flatten(), high_segs.get(i).copied().flatten()),
                        (Some(Seg2::Line { .. }), Some(Seg2::Line { .. }))
                    );
                    for (e, t) in low.iter().zip(high) {
                        let wire: mt::Wire = vec![e.clone(), ruling(e.back(), t.back()), t.inverse(), ruling(e.front(), t.front()).inverse()].into();
                        if lines {
                            sides.push((flat(wire)?, prov.clone()));
                            continue;
                        }
                        let surface = mt::BSplineSurface::homotopy(e.oriented_curve().lift_up(), t.oriented_curve().lift_up());
                        sides.push((mt::Face::new(vec![wire], mt::Surface::NurbsSurface(mt::NurbsSurface::new(surface))), prov.clone()));
                    }
                }
                (Some(low), None) => {
                    let [e] = &low[..] else { return Err(bad()) };
                    sides.push((flat(vec![e.clone(), ruling(e.back(), t1), ruling(e.front(), t0).inverse()].into())?, prov));
                }
                (None, Some(high)) => {
                    let [t] = &high[..] else { return Err(bad()) };
                    sides.push((flat(vec![ruling(b1, t.back()), t.inverse(), ruling(b0, t.front()).inverse()].into())?, prov));
                }
                (None, None) => {}
            }
        }
    }
    let wire =
        |level: Option<&Level>| -> mt::Wire { level.map(|(_, edges)| edges.iter().flatten().flatten().cloned().collect()).unwrap_or_default() };
    Ok((sides, wire(built.first()), wire(built.last())))
}

/// Extrude one region by `length` along `dir_sign` × the plane normal with a taper angle
/// (radians; positive grows the profile).
pub fn extrude_tapered(plane: &Plane, region: &Region2, length: f64, dir_sign: f64, taper: f64) -> Result<Body> {
    extrude_tapered_between(plane, region, 0.0, length, 0.0, dir_sign, taper)
}

/// [`extrude_tapered`] from `from` to `to` along `dir_sign` × the plane normal, the region
/// being the section at 0: its sides run on through it, so `from` below 0 starts the solid
/// that far before the profile (into a face the profile lies on), at the profile shrunk or
/// grown by the taper. With a `foot` (a height, less than `to - from`), the sides stand
/// straight for that height above `from` and only then slope, up to the same top: a face the
/// solid stands on can then be pushed into the body it joins, as a prism's can.
pub fn extrude_tapered_between(plane: &Plane, region: &Region2, from: f64, to: f64, foot: f64, dir_sign: f64, taper: f64) -> Result<Body> {
    let (from, to, foot, taper) = (finite(from, "start")?, finite(to, "distance")?, finite(foot, "foot")?, finite(taper, "taper")?);
    if to - from <= 1e-6 {
        return Err(KernelError::Invalid("extrude distance is zero".into()));
    }
    if !(0.0..(to - from) * 0.5).contains(&foot) {
        return Err(KernelError::Invalid(format!("taper foot = {foot}")));
    }
    if taper.abs() >= std::f64::consts::FRAC_PI_2 - 1e-3 {
        return Err(KernelError::Invalid("taper must be between −90° and 90°".into()));
    }
    region_ok(region)?;
    let (t, sign) = (taper.tan(), dir_sign.signum());
    let (bottom_plane, top_plane) = (plane.offset(from * sign), plane.offset(to * sign));
    let loops0 = oriented(region)?;
    let split: Vec<(Loop2, Vec<usize>)> = loops0.loops().map(|l| offset_splits(&l.lp, from.abs().max(to.abs()) * t)).collect();
    let ends = |h: f64| split.iter().map(|(lp, _)| offset_loop(lp, h * t)).collect::<Result<Vec<_>>>();
    let (bottoms, tops) = (ends(from)?, ends(to)?);
    // Polylines within a small part of the larger offset of it.
    let tol = (from.abs().max(to.abs()) * t.abs() * 0.02).max(1e-6);
    if t != 0.0 {
        match (offset_crosses(&bottoms, tol), offset_crosses(&tops, tol)) {
            (Some(false), Some(false)) => {}
            (Some(true), _) | (_, Some(true)) => {
                return Err(KernelError::Invalid(
                    "the taper closes the profile: at this distance its sides would cross (a stroke, a curve or a gap narrower than the taper); try a smaller taper or distance".into(),
                ));
            }
            _ => {
                return Err(KernelError::TooLarge(
                    "the tapered profile has too many details to check that its sides stay apart; taper fewer profiles at a time".into(),
                ));
            }
        }
    }
    guard("tapered extrude", || {
        let mut sides: Vec<Side> = Vec::new();
        let (mut w0, mut w1): (Vec<mt::Wire>, Vec<mt::Wire>) = (Vec::new(), Vec::new());
        for (((l, (_, origin)), b), t) in loops0.loops().zip(&split).zip(&bottoms).zip(&tops) {
            let mut levels = vec![(bottom_plane, &b[..])];
            if foot > 0.0 {
                levels.push((plane.offset((from + foot) * sign), &b[..]));
            }
            levels.push((top_plane, &t[..]));
            let (s, below, above) = taper_loop(&levels, |i| origin.get(i).map(|&k| l.numbers(k).to_vec()).unwrap_or_default())?;
            sides.extend(s);
            w0.push(below);
            w1.push(above);
        }
        let (faces, prov): (Vec<mt::Face>, Vec<Option<FaceProv>>) = sides.into_iter().unzip();
        // The sides, then the bottom and top as pushed below.
        let history: Option<Vec<FaceProv>> = prov.into_iter().chain([Some(record::cap(false)), Some(record::cap(true))]).collect();
        let bottom = builder::try_attach_plane(&w0.iter().map(|w| w.inverse()).collect::<Vec<_>>())
            .map_err(|e| KernelError::Failed(format!("taper bottom: {e}")))?;
        let top = builder::try_attach_plane(&w1).map_err(|e| KernelError::Failed(format!("taper top: {e}")))?;
        let mut tries: Vec<(mt::Face, mt::Face)> = vec![(bottom.clone(), top.clone())];
        tries.push((bottom.inverse(), top.inverse()));
        let mut last = String::new();
        for (b, t) in tries {
            let mut all = faces.clone();
            all.push(b);
            all.push(t);
            let shell: mt::Shell = all.into();
            match Solid::try_new(vec![shell]) {
                Ok(s) => return record::attach(Body::new(s)?, &[], history, Default::default()),
                Err(e) => last = e.to_string(),
            }
        }
        Err(KernelError::Failed(format!("tapered extrude: {last}")))
    })
}

/// Revolve 360° a loop that has exactly one line segment on the axis, without degenerate faces.
/// A loop made of a half circle and its diameter on the axis: (centre, radius).
fn half_disc(lp: &Loop2, axis_origin: Vec2, d: Vec2) -> Option<(Vec2, f64)> {
    let on_axis = |p: Vec2| d.cross(p - axis_origin).abs() < 1e-7 * (1.0 + p.len());
    let mut arcs = Vec::new();
    for s in &lp.segs {
        match *s {
            Seg2::Line { a, b } if on_axis(a) && on_axis(b) => {}
            Seg2::Arc { center, radius, sweep, .. } if on_axis(center) => arcs.push((center, radius, sweep)),
            _ => return None,
        }
    }
    let (c, r, _) = *arcs.first()?;
    let total: f64 = arcs.iter().map(|a| a.2.abs()).sum();
    let same = arcs.iter().all(|(c2, r2, _)| c2.dist(c) < 1e-7 * (1.0 + r) && (r2 - r).abs() < 1e-7 * (1.0 + r));
    (same && (total - std::f64::consts::PI).abs() < 1e-6).then_some((c, r))
}

/// Whether the loop has one edge on the axis and comes back to it along a curve (the segment
/// before the axis edge is not a line). A line there (a drill point) sweeps a cone, which
/// truck's cone builds right.
fn pole_at_end(lp: &Loop2, axis_origin: Vec2, d: Vec2) -> bool {
    let on_axis = |p: Vec2| d.cross(p - axis_origin).abs() < 1e-9;
    let n = lp.segs.len();
    let axis_segs: Vec<usize> =
        (0..n).filter(|i| lp.segs.get(*i).is_some_and(|s| matches!(s, Seg2::Line { a, b } if on_axis(*a) && on_axis(*b)))).collect();
    let [k] = axis_segs[..] else { return false };
    lp.segs.get((k + n - 1) % n).is_some_and(|s| !on_axis(s.start()) && !matches!(s, Seg2::Line { .. }))
}

/// The edge from `a` to `b` along the profile segment `sg` (in `plane`), for a revolve's sweep.
/// One that ends on the axis (`ends_on_axis`) is built from there and turned round, so that the
/// surface swept from its curve has its pole at the curve's start. truck's revolved surface
/// reads a point on its axis at the start of its curve with the angle of the point before it,
/// but one at the end with an angle of 2π when it has no hint, past a part turn's span: the face
/// came out as another region (#1853).
fn swept_edge(plane: &Plane, sg: &Seg2, a: &mt::Vertex, b: &mt::Vertex, ends_on_axis: bool) -> mt::Edge {
    if ends_on_axis {
        return swept_edge(plane, &sg.reversed(), b, a, false).inverse();
    }
    match *sg {
        Seg2::Line { .. } => builder::line(a, b),
        Seg2::Arc { .. } => builder::circle_arc(a, b, p3(plane.to_world(sg.mid()))),
        Seg2::Cubic { .. } | Seg2::Conic { .. } | Seg2::Bezier { .. } => {
            crate::freeform::edge(plane, sg, a, b).unwrap_or_else(|| builder::line(a, b))
        }
    }
}

/// Where [`revolve_on_axis`]'s full turns start, from the profile: an angle no plane a part is
/// commonly cut along lies at (as full circles start, see `reseam`).
const FULL_TURN_START: f64 = 0.4142;

/// A revolve of a loop with one edge on the axis: the other edges sweep their faces, a third of a
/// turn at most each. A partial turn is closed by two caps (the profile at the start and at the
/// end), which share the axis edge; a full turn ends where it started. `None` when the loop
/// isn't like that.
fn revolve_on_axis(plane: &Plane, numbered: &NumberedLoop, axis_origin: Vec2, d: Vec2, angle: f64) -> Result<Option<Body>> {
    use mt::{BoundedCurve, ParametricCurve, ParametricSurface3D, SearchNearestParameter, Transformed};
    let lp = &numbered.lp;
    let on_axis = |p: Vec2| d.cross(p - axis_origin).abs() < 1e-9;
    let n = lp.segs.len();
    let axis_segs: Vec<usize> =
        (0..n).filter(|i| lp.segs.get(*i).is_some_and(|s| matches!(s, Seg2::Line { a, b } if on_axis(*a) && on_axis(*b)))).collect();
    let [k] = axis_segs[..] else { return Ok(None) };
    // The open chain after the axis segment, back around to it: only its ends on the axis.
    let chain: Vec<Seg2> = (1..n).filter_map(|j| lp.segs.get((k + j) % n).copied()).collect();
    if chain.is_empty() || chain.iter().skip(1).any(|s| on_axis(s.start())) || chain.iter().any(|s| on_axis(s.mid())) {
        return Ok(None);
    }
    let full = angle.abs() >= std::f64::consts::TAU - 1e-9;
    // Faces of a third of a turn at most, as truck sweeps a full turn: a boolean could not
    // intersect a half turn's faces and the half circles bounding them (a half slot turned half a
    // turn into a plate, #1853), and a sphere's face needs room for its poles (below).
    let pieces = (angle.abs() / (std::f64::consts::TAU / 3.0) - 1e-9).ceil().max(1.0) as usize;
    let step = angle / pieces as f64;
    // Built with the axis through the world origin, then moved into place.
    let shift = plane.to_world(axis_origin);
    let local = Plane { origin: plane.origin - shift, ..*plane };
    let plane = &local;
    let axis = plane.dir_to_world(d);
    let turned = |by: f64| mt::Matrix4::from_axis_angle(v3(axis), mt::Rad(by));
    let turn = |m: &mt::Matrix4, p: Vec3| from_p3(mt::EuclideanSpace::from_vec((m * p3(p).to_homogeneous()).truncate()));
    guard("revolve", || {
        let fail = |m: &str| KernelError::Failed(format!("revolve: {m}"));
        let pts: Vec<Vec3> = chain.iter().map(|s| plane.to_world(s.start())).chain(chain.last().map(|s| plane.to_world(s.end()))).collect();
        let last = pts.len() - 1;
        let pv: Vec<mt::Vertex> = pts.iter().map(|p| builder::vertex(p3(*p))).collect();
        let mut e: Vec<mt::Edge> = Vec::new();
        for (i, sg) in chain.iter().enumerate() {
            let (Some(a), Some(b)) = (pv.get(i), pv.get(i + 1)) else { return Err(fail("chain")) };
            e.push(swept_edge(plane, sg, a, b, on_axis(sg.end())));
        }
        // The profile's vertices and edges where each piece starts and ends: turned by the step,
        // the ends staying where they are (on the axis). A part turn starts on the profile. A full
        // turn starts at an odd angle and ends where it started: the profile lies in the plane
        // it was drawn on, often a face of the body cut, and a seam lying in that face as well
        // left the cut nothing it could intersect there (#1853).
        let start = if full { FULL_TURN_START } else { 0.0 };
        let mut levels: Vec<(Vec<mt::Vertex>, Vec<mt::Edge>)> = Vec::new();
        for j in 0..=pieces {
            if j == 0 && !full {
                levels.push((pv.clone(), e.clone()));
                continue;
            }
            if let (true, Some(first)) = (j == pieces && full, levels.first()) {
                levels.push(first.clone());
                continue;
            }
            let rot = turned(start + step * j as f64);
            let qv: Vec<mt::Vertex> = pts
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    if i == 0 || i == last {
                        pv.get(i).cloned().unwrap_or_else(|| builder::vertex(p3(*p)))
                    } else {
                        builder::vertex(p3(turn(&rot, *p)))
                    }
                })
                .collect();
            let mut er: Vec<mt::Edge> = Vec::new();
            for (i, edge) in e.iter().enumerate() {
                let (Some(qa), Some(qb)) = (qv.get(i), qv.get(i + 1)) else { return Err(fail("chain")) };
                let curve = edge.curve().transformed(rot);
                er.push(if edge.orientation() { mt::Edge::new(qa, qb, curve) } else { mt::Edge::new(qb, qa, curve).inverse() });
            }
            levels.push((qv, er));
        }
        // Side faces: each edge's swept surface, oriented to agree with its boundary, from the
        // profile segment the edge was built from.
        let mut faces: Vec<(mt::Face, FaceProv)> = Vec::new();
        for (j, piece) in levels.windows(2).enumerate() {
            let [(pv, e), (qv, er)] = piece else { return Err(fail("pieces")) };
            let half = turned(start + step * (j as f64 + 0.5));
            // Arcs swept by the vertices off the axis.
            let mut arcs: Vec<Option<mt::Edge>> = vec![None; pts.len()];
            for i in 1..last {
                let (Some(a), Some(b), Some(p)) = (pv.get(i), qv.get(i), pts.get(i)) else { continue };
                if let Some(slot) = arcs.get_mut(i) {
                    *slot = Some(builder::circle_arc(a, b, p3(turn(&half, *p))));
                }
            }
            for (i, sg) in chain.iter().enumerate() {
                let (Some(ei), Some(eri)) = (e.get(i), er.get(i)) else { return Err(fail("edges")) };
                let mut ws: Vec<mt::Edge> = vec![ei.clone()];
                if let Some(Some(c)) = arcs.get(i + 1) {
                    ws.push(c.clone());
                }
                ws.push(eri.inverse());
                if let Some(Some(c)) = arcs.get(i) {
                    ws.push(c.inverse());
                }
                let wire: mt::Wire = ws.into();
                let radial = matches!(*sg, Seg2::Line { a, b } if (b - a).normalized().is_some_and(|u| u.dot(d).abs() < 1e-9));
                let f = if radial {
                    builder::try_attach_plane(&[wire]).map_err(|er| fail(&er.to_string()))?
                } else {
                    // The surface turns from the piece's start through |step|, so its angle runs
                    // from 0 across the face. Inverting it turns its axis round, which puts the
                    // face at 2π - |step| to 2π: its start then lies on the angle's seam, where
                    // points (and a pole without a hint) read as 0, the far side, and the face
                    // meshed and intersected as the wrong region (#1853). The face is turned
                    // round instead.
                    let (dir, by) = if step < 0.0 { (-axis, -step) } else { (axis, step) };
                    let surface = match *sg {
                        // An arc about a centre on the axis that reaches it sweeps part of a sphere
                        // with its pole there, and a boolean can't follow an intersection into a
                        // pole: a cut by the plane through the axis (the face the profile was drawn
                        // on) failed (#1853). The face lies on the sphere turned about the axis
                        // square to the revolve's and to the face's middle, as `sphere_solid`'s
                        // patches do: its poles are a quarter turn from that middle, past the
                        // face's third of a turn, and its seam runs round the back.
                        Seg2::Arc { center, radius, .. } if on_axis(center) && (on_axis(sg.start()) || on_axis(sg.end())) => {
                            let c = plane.to_world(center);
                            let out = turn(&half, plane.to_world(sg.mid())) - c;
                            let out = (out - axis * out.dot(axis)).normalized().ok_or_else(|| fail("sphere"))?;
                            let pole = axis.cross(out);
                            let meridian = builder::circle_arc(
                                &builder::vertex(p3(c + pole * radius)),
                                &builder::vertex(p3(c - pole * radius)),
                                p3(c - out * radius),
                            );
                            mt::Surface::RevolutedCurve(mt::Processor::new(mt::RevolutedCurve::by_revolution(
                                meridian.oriented_curve(),
                                p3(c),
                                v3(pole),
                            )))
                        }
                        _ => {
                            let template = builder::rsweep(ei, mt::Point3::new(0.0, 0.0, 0.0), v3(dir), mt::Rad(by));
                            template.face_iter().next().map(|f| f.surface()).ok_or_else(|| fail("swept face"))?
                        }
                    };
                    // The loop's turning (Newell) against the surface normal halfway round.
                    let mut nw = Vec3::ZERO;
                    let poly: Vec<Vec3> = wire
                        .edge_iter()
                        .flat_map(|ed| {
                            let c = ed.oriented_curve();
                            let (t0, t1) = c.range_tuple();
                            (0..8).map(move |j| from_p3(c.subs(t0 + (t1 - t0) * j as f64 / 8.0))).collect::<Vec<_>>()
                        })
                        .collect();
                    for (j, p) in poly.iter().enumerate() {
                        if let Some(q) = poly.get((j + 1) % poly.len()) {
                            nw += p.cross(*q);
                        }
                    }
                    let probe = turn(&half, plane.to_world(sg.mid()));
                    let sn = surface.search_nearest_parameter(p3(probe), None, 100).map(|(u, v)| surface.normal(u, v));
                    if sn.is_some_and(|m| nw.dot(Vec3::new(m.x, m.y, m.z)) < 0.0) {
                        mt::Face::try_new(vec![wire.inverse()], surface).map_err(|er| fail(&er.to_string()))?.inverse()
                    } else {
                        mt::Face::try_new(vec![wire], surface).map_err(|er| fail(&er.to_string()))?
                    }
                };
                faces.push((f, record::profile(numbered.numbers((k + 1 + i) % n))));
            }
        }
        let (faces, mut history): (Vec<mt::Face>, Vec<FaceProv>) = faces.into_iter().unzip();
        let close = |solid: Solid, history: Vec<FaceProv>| {
            let solid = if shift.len() > 0.0 { builder::translated(&solid, v3(shift)) } else { solid };
            record::attach(Body::new(solid)?, &[], Some(history), Default::default()).map(Some)
        };
        if full {
            return close(Solid::try_new(vec![faces.into()]).map_err(|er| fail(&er.to_string()))?, history);
        }
        // Caps: the profile at the start and at the end, sharing the axis edge.
        let ((Some(a0), Some(an)), Some((_, er))) = ((pv.first(), pv.get(last)), levels.last()) else { return Err(fail("ends")) };
        let axis_edge = builder::line(an, a0);
        let start: mt::Wire = e.iter().rev().map(|x| x.inverse()).chain(std::iter::once(axis_edge.inverse())).collect::<Vec<_>>().into();
        let end: mt::Wire = er.iter().cloned().chain(std::iter::once(axis_edge.clone())).collect::<Vec<_>>().into();
        let cs = builder::try_attach_plane(&[start]).map_err(|er| fail(&er.to_string()))?;
        let ce = builder::try_attach_plane(&[end]).map_err(|er| fail(&er.to_string()))?;
        history.extend([record::cap(false), record::cap(true)]);
        let mut last_err = String::new();
        for flip in 0..4 {
            let mut all = faces.clone();
            all.push(if flip & 1 == 0 { cs.clone() } else { cs.inverse() });
            all.push(if flip & 2 == 0 { ce.clone() } else { ce.inverse() });
            match Solid::try_new(vec![all.into()]) {
                Ok(solid) => return close(solid, history),
                Err(er) => last_err = er.to_string(),
            }
        }
        Err(fail(&last_err))
    })
}

fn revolve_touching_axis(plane: &Plane, numbered: &NumberedLoop, axis_origin: Vec2, d: Vec2) -> Result<Option<Body>> {
    let lp = &numbered.lp;
    let on_axis = |p: Vec2| d.cross(p - axis_origin).abs() < 1e-9;
    let n = lp.segs.len();
    let axis_segs: Vec<usize> =
        (0..n).filter(|i| lp.segs.get(*i).is_some_and(|s| matches!(s, Seg2::Line { a, b } if on_axis(*a) && on_axis(*b)))).collect();
    let [k] = axis_segs[..] else { return Ok(None) };
    // The open chain after the axis segment, back around to it, with the profile segments
    // under each piece.
    let mut chain: Vec<Seg2> = (1..n).filter_map(|j| lp.segs.get((k + j) % n).copied()).collect();
    let mut chain_src: Vec<&[usize]> = (1..n).map(|j| numbered.numbers((k + j) % n)).collect();
    // A single arc (a sphere) is split off-centre: a seam on the equator would lie in the very
    // planes other bodies are often cut with.
    if let [Seg2::Arc { center, radius, start, sweep }] = chain[..] {
        let f = 0.37;
        chain = vec![
            Seg2::Arc { center, radius, start, sweep: sweep * f },
            Seg2::Arc { center, radius, start: start + sweep * f, sweep: sweep * (1.0 - f) },
        ];
        chain_src = vec![numbered.numbers((k + 1) % n); 2];
    }
    if chain.is_empty() || chain.iter().skip(1).take(chain.len().saturating_sub(2)).any(|s| on_axis(s.start()) && on_axis(s.end())) {
        return Ok(None);
    }
    // Built with the axis through the world origin (truck tessellates such cones reliably), then
    // moved into place.
    let shift = plane.to_world(axis_origin);
    let local = Plane { origin: plane.origin - shift, ..*plane };
    let plane = &local;
    // Radial lines at the ends become planar caps (no degenerate disc centres).
    let radial = |sg: &Seg2| matches!(*sg, Seg2::Line { a, b } if (b - a).normalized().is_some_and(|u| u.dot(d).abs() < 1e-9));
    let mut mid: &[Seg2] = &chain;
    let mut mid_src: &[&[usize]] = &chain_src;
    // The segments of the radial lines that become caps.
    let (mut start_cap, mut end_cap) = (None, None);
    if mid.first().is_some_and(|f| radial(f) && on_axis(f.start())) {
        mid = mid.get(1..).unwrap_or(&[]);
        start_cap = mid_src.first().copied();
        mid_src = mid_src.get(1..).unwrap_or(&[]);
    }
    if mid.last().is_some_and(|l| radial(l) && on_axis(l.end())) {
        mid = mid.get(..mid.len().saturating_sub(1)).unwrap_or(&[]);
        end_cap = mid_src.last().copied();
        mid_src = mid_src.get(..mid_src.len().saturating_sub(1)).unwrap_or(&[]);
    }
    if mid.is_empty() {
        return Ok(None);
    }
    let apex_start = mid.first().is_some_and(|f| on_axis(f.start()));
    let apex_end = mid.last().is_some_and(|l| on_axis(l.end()));
    guard("revolve", || {
        let verts: Vec<mt::Vertex> = mid
            .iter()
            .map(|s| builder::vertex(p3(plane.to_world(s.start()))))
            .chain(mid.last().map(|s| builder::vertex(p3(plane.to_world(s.end())))))
            .collect();
        let mut edges: Vec<mt::Edge> = Vec::new();
        for (i, s) in mid.iter().enumerate() {
            let (Some(a), Some(b)) = (verts.get(i), verts.get(i + 1)) else { continue };
            edges.push(match *s {
                Seg2::Line { .. } => builder::line(a, b),
                Seg2::Arc { .. } => builder::circle_arc(a, b, p3(plane.to_world(s.mid()))),
                Seg2::Cubic { .. } | Seg2::Conic { .. } | Seg2::Bezier { .. } => match crate::freeform::edge(plane, s, a, b) {
                    Some(e) => e,
                    None => builder::line(a, b),
                },
            });
        }
        // Each edge with the profile segments under it, before the wire may be turned round.
        let seeds: Vec<(mt::EdgeID, Vec<usize>)> = edges.iter().map(|e| e.id()).zip(mid_src.iter().map(|s| s.to_vec())).collect();
        let mut w: mt::Wire = edges.into();
        let axis = plane.dir_to_world(d);
        let mut shell = if apex_start || apex_end {
            if !apex_start {
                // builder::cone wants the apex first.
                w = w.inverse();
            }
            builder::cone(&w, v3(axis), mt::Rad(std::f64::consts::TAU))
        } else {
            builder::rsweep(&w, p3(plane.to_world(axis_origin)), v3(axis), mt::Rad(std::f64::consts::TAU))
        };
        let sides = shell.len();
        // History. truck sweeps each wire edge round into a column of faces (triangles at an
        // apex, which stays put); each cap is the disc a trimmed radial line sweeps, found by
        // the profile vertex on its rim.
        let fixed: Vec<mt::VertexID> =
            [(apex_start, verts.first()), (apex_end, verts.last())].into_iter().filter_map(|(apex, v)| v.filter(|_| apex).map(|v| v.id())).collect();
        let mut history: Option<Vec<FaceProv>> = (seeds.len() == mid.len())
            .then(|| record::columns(shell.face_iter(), &seeds, None, record::Column::ToEnd, &fixed))
            .flatten()
            .and_then(|sides| sides.into_iter().collect());
        let (first_vertex, last_vertex) = (verts.first().map(|v| v.id()), verts.last().map(|v| v.id()));
        for b in shell.extract_boundaries() {
            let at = |v: Option<mt::VertexID>| v.is_some_and(|v| b.vertex_iter().any(|x| x.id() == v));
            let src = if at(first_vertex) {
                start_cap
            } else if at(last_vertex) {
                end_cap
            } else {
                None
            };
            history = history.zip(src).map(|(mut h, numbers)| {
                h.push(record::profile(numbers));
                h
            });
            let cap = builder::try_attach_plane(&[b]).map_err(|e| KernelError::Failed(format!("revolve cap: {e}")))?;
            shell.push(cap);
        }
        let solid = Solid::try_new(vec![shell.clone()]).or_else(|_| {
            // Caps may come out facing inwards: flip them.
            let mut s2 = shell.clone();
            for f in s2.face_iter_mut().skip(sides) {
                f.invert();
            }
            Solid::try_new(vec![s2])
        });
        let solid = solid.map_err(|e| KernelError::Failed(format!("revolve: {e}")))?;
        let solid = if shift.len() > 0.0 { builder::translated(&solid, v3(shift)) } else { solid };
        record::attach(Body::new(solid)?, &[], history, Default::default()).map(Some)
    })
}

/// Close a ruled shell (side faces between matching wires, with their provenance as built)
/// with planar caps into a body, whose history is the sides' and the caps': the profile at
/// `w0` is the start cap and at `w1` the end cap (the other way round when `from_end`).
pub(crate) fn cap_and_close(sides: Vec<Side>, w0: &[mt::Wire], w1: &[mt::Wire], from_end: bool) -> Result<Body> {
    let (mut faces, prov): (Vec<mt::Face>, Vec<Option<FaceProv>>) = sides.into_iter().unzip();
    let history: Option<Vec<FaceProv>> = prov.into_iter().chain([Some(record::cap(from_end)), Some(record::cap(!from_end))]).collect();
    let bottom =
        builder::try_attach_plane(&w0.iter().map(|w| w.inverse()).collect::<Vec<_>>()).map_err(|e| KernelError::Failed(format!("cap: {e}")))?;
    let top = builder::try_attach_plane(w1).map_err(|e| KernelError::Failed(format!("cap: {e}")))?;
    let mut last = String::new();
    for (b, t) in [(bottom.clone(), top.clone()), (bottom.inverse(), top.inverse()), (bottom.clone(), top.inverse()), (bottom.inverse(), top)] {
        let mut all = std::mem::take(&mut faces);
        let keep = all.clone();
        all.push(b);
        all.push(t);
        let shell: mt::Shell = all.into();
        match Solid::try_new(vec![shell]) {
            Ok(s) => return record::attach(Body::new(s)?, &[], history, Default::default()),
            Err(e) => last = e.to_string(),
        }
        faces = keep;
    }
    Err(KernelError::Failed(format!("closing the shell: {last}")))
}

/// A sweep whose walls already close, including separate shells around profile holes.
pub(crate) fn close_sweep(sides: Vec<Side>) -> Result<Body> {
    let sources: std::collections::HashMap<_, _> = sides.iter().map(|(f, p)| (f.id(), p.clone())).collect();
    let shell: mt::Shell = sides.into_iter().map(|(f, _)| f).collect();
    let solid = Solid::try_new(shell.connected_components()).map_err(|e| KernelError::Failed(format!("closing the sweep: {e}")))?;
    let history = solid.face_iter().map(|f| sources.get(&f.id()).cloned().flatten()).collect();
    record::attach(Body::new(solid)?, &[], history, Default::default())
}

/// Ruled loft through two or more planar sections (outer loops only). The sections and the
/// same sections in reverse order give the same body.
pub fn loft(sections: &[(Plane, Loop2)]) -> Result<Body> {
    let (wires, edge_sources, reversed) = loft_wires(sections)?;
    // The wires run from the first section given, or (turned round) the last.
    // Three sections or more: one smooth surface through them all for each segment.
    if wires.len() >= 3
        && let Ok(b) = guard("loft", || {
            let faces = smooth_loft_faces(&wires, &edge_sources, true)?;
            let (Some(w0), Some(w1)) = (wires.first(), wires.last()) else { return Err(KernelError::Failed("smooth loft: sections".into())) };
            cap_and_close(faces, std::slice::from_ref(w0), std::slice::from_ref(w1), reversed)
        })
    {
        return Ok(b);
    }
    guard("loft", || {
        let sides = ruled_loft_faces(&wires, &edge_sources)?;
        let (Some(first), Some(last)) = (wires.first(), wires.last()) else { return Err(KernelError::Failed("loft".into())) };
        cap_and_close(sides, std::slice::from_ref(first), std::slice::from_ref(last), reversed)
    })
}

/// The wires of a loft's closed sections, matched: oriented alike, resampled to the same
/// segment count and turned to start alike; with the profile segments under each wire edge, and
/// whether the sections were turned round into the canonical order.
pub(crate) fn loft_wires(sections: &[(Plane, Loop2)]) -> Result<(Vec<mt::Wire>, Vec<Vec<Vec<usize>>>, bool)> {
    if sections.len() < 2 || sections.len() > 50 {
        return Err(KernelError::Invalid("a loft needs 2…50 sections".into()));
    }
    let n = sections.iter().map(|(_, l)| l.segs.len()).max().unwrap_or(0);
    if n == 0 || n > MAX_SEGS {
        return Err(KernelError::Invalid("empty section".into()));
    }
    // A canonical order, so that reversing the sections changes nothing: the one whose
    // first-to-last step has a positive largest component. The side faces' parameterisation
    // follows the order, and the boolean worked on one and failed on the other (a cut whose
    // loft touches the body along an edge: an internal panic, or "shell not oriented and
    // closed").
    let centre = |(p, l): &(Plane, Loop2)| p.to_world(l.centroid());
    let (Some(first), Some(last)) = (sections.first(), sections.last()) else { return Err(KernelError::Invalid("no sections".into())) };
    let step = centre(last) - centre(first);
    let big = [step.x, step.y, step.z].into_iter().max_by(|a, b| a.abs().total_cmp(&b.abs())).unwrap_or(0.0);
    let reversed = big < 0.0;
    // The sections' segments are numbered in the order given.
    let mut ordered: Vec<(Plane, NumberedLoop)> =
        sections.iter().map(|(p, _)| *p).zip(numbered::number_loops(sections.iter().map(|(_, l)| l))).collect();
    if reversed {
        ordered.reverse();
    }
    // Orient every section counter-clockwise about the first section's normal (`Body::new`
    // turns the solid outward), then match segment counts and starting points to the section
    // with the most segments.
    let n0 = ordered.first().map(|(p, _)| p.normal()).unwrap_or(Vec3::Z);
    let oriented: Vec<(Plane, NumberedLoop)> = ordered
        .iter()
        .map(|(p, l)| {
            let ccw = p.normal().dot(n0) >= 0.0;
            (*p, l.oriented(!ccw))
        })
        .collect();
    let lead = oriented.iter().max_by_key(|(_, l)| l.lp.segs.len()).ok_or_else(|| KernelError::Invalid("no sections".into()))?;
    let lead_c = lead.0.to_world(lead.1.lp.centroid());
    let lead_dirs: Vec<Vec3> = lead.1.lp.segs.iter().map(|s| lead.0.to_world(s.start()) - lead_c).collect();
    let mut wires: Vec<mt::Wire> = Vec::new();
    // Per section, the profile segments under each of its wire's edges.
    let mut edge_sources: Vec<Vec<Vec<usize>>> = Vec::new();
    for (p, l) in &oriented {
        let c = l.lp.centroid();
        // Break angles: the lead section's vertex directions seen in this plane.
        let angles: Vec<f64> = lead_dirs.iter().map(|d| Vec2::new(d.dot(p.x), d.dot(p.y)).angle()).collect();
        let mut r = l.resample(n, &angles);
        // Start where every vertex lines up best with the lead's vertex of the same number (the
        // largest sum of dot products of their directions from the centroids). The one vertex
        // closest in direction to the lead's first vertex twisted non-convex sections (#881): a
        // zig-zag's centroid sits off its outline, so its first vertex can point away from the
        // lead's.
        let dirs: Vec<Vec2> = lead_dirs.iter().map(|d| Vec2::new(d.dot(p.x), d.dot(p.y))).collect();
        let m = r.lp.segs.len();
        let fit =
            |k: usize| -> f64 { dirs.iter().enumerate().filter_map(|(i, d)| r.lp.segs.get((i + k) % m).map(|s| (s.start() - c).dot(*d))).sum() };
        if let Some(k) = (0..m).min_by(|a, b| fit(*b).total_cmp(&fit(*a))) {
            r.rotate_left(k);
        }
        wires.push(uniform_wire(p, &r.lp)?);
        edge_sources.push(r.from);
    }
    Ok((wires, edge_sources, reversed))
}

/// Ruled faces between each two neighbouring section wires; each comes from the section edges
/// it spans.
pub(crate) fn ruled_loft_faces(wires: &[mt::Wire], edge_sources: &[Vec<Vec<usize>>]) -> Result<Vec<Side>> {
    let mut sides: Vec<Side> = Vec::new();
    for (k, w) in wires.windows(2).enumerate() {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        let shell = builder::try_wire_homotopy(a, b).map_err(|e| KernelError::Failed(format!("loft: {e}")))?;
        match (edge_sources.get(k), edge_sources.get(k + 1)) {
            (Some(na), Some(nb)) => sides.extend(record::ruled(&shell, a, b, na, nb)),
            _ => sides.extend(shell.face_iter().map(|f| (f.clone(), None))),
        }
    }
    Ok(sides)
}

/// Points across the sections (one per section, in order) as a smooth B-spline through them at the
/// parameters `u` (rising from 0 to 1, one per section): cubic through four or more, quadratic
/// through three.
fn across(pts: &[Vec3], u: &[f64]) -> Option<mt::BSplineCurve<mt::Point3>> {
    match pts {
        [a, b, c] => {
            // The quadratic Bezier through `b` at t = u[1].
            let t = *u.get(1)?;
            if !(t > 0.0 && t < 1.0) {
                return None;
            }
            let m = (*b - *a * ((1.0 - t) * (1.0 - t)) - *c * (t * t)) * (1.0 / (2.0 * t * (1.0 - t)));
            mt::BSplineCurve::try_new(mt::KnotVec::bezier_knot(2), vec![p3(*a), p3(m), p3(*c)]).ok()
        }
        _ => interpolate_cubic_at(pts, u),
    }
}

/// The parameters of the sections along a loft: in proportion to the distance between them (the
/// mean over the wires' vertices), so that unevenly spaced sections get a surface that follows
/// them. At a uniform parameter a section close to its neighbour makes the surface overshoot
/// between the others (by several times the section's size when the spacing differs tenfold).
/// Evenly spaced sections give the uniform parameters.
fn section_parameters(edges: &[Vec<mt::Edge>]) -> Vec<f64> {
    let m = edges.len();
    let uniform = || (0..m).map(|i| i as f64 / (m.max(2) - 1) as f64).collect::<Vec<f64>>();
    let corners: Vec<Vec<Vec3>> = edges.iter().map(|es| es.iter().map(|e| from_p3(e.front().point())).collect()).collect();
    let mut u = vec![0.0];
    for w in corners.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { return uniform() };
        let n = a.len().min(b.len());
        if n == 0 {
            return uniform();
        }
        let step = a.iter().zip(b).map(|(p, q)| p.dist(*q)).sum::<f64>() / n as f64;
        u.push(u.last().copied().unwrap_or(0.0) + step);
    }
    let Some(total) = u.last().copied().filter(|t| t.is_finite() && *t > 0.0) else { return uniform() };
    let u: Vec<f64> = u.iter().map(|x| x / total).collect();
    // A section on top of its neighbour: no parameter for it, so the uniform ones.
    if u.windows(2).any(|w| w.get(1).zip(w.first()).is_none_or(|(b, a)| b - a < 1e-6)) {
        return uniform();
    }
    u
}

/// A loft whose side faces are B-spline surfaces through every section (smooth across the
/// middle sections, which leave no edges). Every section wire comes from `uniform_wire`, so the
/// k-th edges of all sections share one knot vector and their control points pair up. Face k
/// comes from the profile segments under the k-th edge of every section (`numbers`: by
/// section, by edge); the caps as [`cap_and_close`] takes them (`from_end`).
/// One smooth face through every section for each wire edge (cubic across four sections or
/// more, quadratic across three), neighbours sharing their seams. `closed` wires join their
/// last edge to their first; open ones have a seam at each end.
pub(crate) fn smooth_loft_faces(wires: &[mt::Wire], numbers: &[Vec<Vec<usize>>], closed: bool) -> Result<Vec<Side>> {
    let fail = |m: &str| KernelError::Failed(format!("smooth loft: {m}"));
    let edges: Vec<Vec<mt::Edge>> = wires.iter().map(|w| w.edge_iter().cloned().collect()).collect();
    let n = edges.first().map(Vec::len).ok_or_else(|| fail("no sections"))?;
    if edges.iter().any(|e| e.len() != n) {
        return Err(fail("sections with different edge counts"));
    }
    let curve_of = |e: &mt::Edge| -> Option<mt::BSplineCurve<mt::Point3>> {
        match e.curve() {
            mt::Curve::BSplineCurve(c) if e.orientation() => Some(c),
            _ => None,
        }
    };
    let (Some(first), Some(last)) = (edges.first(), edges.last()) else { return Err(fail("sections")) };
    let along = section_parameters(&edges);
    // Seams: through the k-th vertex of every section (an open wire's last one at the end of
    // its last edge).
    let vertex = |e: &mt::Edge, end: bool| if end { e.back().clone() } else { e.front().clone() };
    let seam_count = if closed { n } else { n + 1 };
    let mut seams: Vec<mt::Edge> = Vec::with_capacity(seam_count);
    for k in 0..seam_count {
        let (at, end) = if k < n { (k, false) } else { (n - 1, true) };
        let pts: Vec<Vec3> = edges.iter().filter_map(|es| es.get(at).map(|e| from_p3(vertex(e, end).point()))).collect();
        let c = across(&pts, &along).ok_or_else(|| fail("seam"))?;
        let (Some(a), Some(b)) = (first.get(at), last.get(at)) else { return Err(fail("seam")) };
        seams.push(mt::Edge::new(&vertex(a, end), &vertex(b, end), mt::Curve::BSplineCurve(c)));
    }
    let mut faces = Vec::with_capacity(n);
    for k in 0..n {
        let curves: Vec<mt::BSplineCurve<mt::Point3>> =
            edges.iter().map(|es| es.get(k).and_then(curve_of)).collect::<Option<_>>().ok_or_else(|| fail("section curves"))?;
        let c0 = curves.first().ok_or_else(|| fail("section curves"))?;
        let uk = c0.knot_vec().clone();
        let m = c0.control_points().len();
        if curves.iter().any(|c| c.control_points().len() != m || c.knot_vec() != &uk) {
            return Err(fail("sections parameterised differently"));
        }
        let mut ctrl: Vec<Vec<mt::Point3>> = Vec::with_capacity(m);
        let mut vk = None;
        for j in 0..m {
            let pts: Vec<Vec3> = curves.iter().filter_map(|c| c.control_points().get(j).map(|p| from_p3(*p))).collect();
            let c = across(&pts, &along).ok_or_else(|| fail("surface"))?;
            vk = Some(c.knot_vec().clone());
            ctrl.push(c.control_points().clone());
        }
        let vk = vk.ok_or_else(|| fail("surface"))?;
        let surface = mt::BSplineSurface::try_new((uk, vk), ctrl).map_err(|e| fail(&e.to_string()))?;
        let (Some(e0), Some(e1), Some(s0), Some(s1)) = (first.get(k), last.get(k), seams.get(k), seams.get((k + 1) % seam_count)) else {
            return Err(fail("face"));
        };
        // u along the section edge, v across the sections: counter-clockwise in (u, v).
        let w: mt::Wire = vec![e0.clone(), s1.clone(), e1.inverse(), s0.inverse()].into();
        let face = mt::Face::try_new(vec![w], mt::Surface::BSplineSurface(surface)).map_err(|e| fail(&e.to_string()))?;
        let under: Vec<&usize> = numbers.iter().filter_map(|s| s.get(k)).flatten().collect();
        faces.push((face, (under.len() >= numbers.len().max(1)).then(|| record::profile(under))));
    }
    Ok(faces)
}

/// A 3D path segment for sweeps.
#[derive(Clone, Copy, Debug)]
pub enum PathSeg {
    Line {
        a: Vec3,
        b: Vec3,
    },
    /// Arc around `center`, rotating by `angle` about `axis` (right hand) from `a`.
    Arc {
        a: Vec3,
        center: Vec3,
        axis: Vec3,
        angle: f64,
    },
}

impl PathSeg {
    /// Unit directions of the path where the segment starts and ends.
    pub(crate) fn tangents(&self) -> Option<(Vec3, Vec3)> {
        match *self {
            PathSeg::Line { a, b } => {
                let t = (b - a).normalized()?;
                Some((t, t))
            }
            PathSeg::Arc { a, center, axis, angle } => {
                let k = axis.normalized()?;
                let end = rotate_about(a, center, k, angle);
                Some((k.cross(a - center).normalized()?, k.cross(end - center).normalized()?))
            }
        }
    }

    /// Points along the segment (an arc within `tol` of its chords), without its start.
    pub(crate) fn points_after_start(&self, tol: f64) -> Vec<Vec3> {
        match *self {
            PathSeg::Line { b, .. } => vec![b],
            PathSeg::Arc { a, center, axis, angle } => {
                let Some(k) = axis.normalized() else { return vec![a] };
                let r = (a - center).len();
                let step = if r > tol { 2.0 * (1.0 - tol / r).acos() } else { angle.abs() };
                let n = ((angle.abs() / step.max(1e-6)).ceil() as usize).clamp(1, 512);
                (1..=n).map(|i| rotate_about(a, center, k, angle * i as f64 / n as f64)).collect()
            }
        }
    }
}

/// `p` turned by `angle` about the unit axis `k` through `center` (right hand).
pub(crate) fn rotate_about(p: Vec3, center: Vec3, k: Vec3, angle: f64) -> Vec3 {
    let v = p - center;
    let (s, c) = angle.sin_cos();
    center + v * c + k.cross(v) * s + k * (k.dot(v) * (1.0 - c))
}

/// The path as a polyline: its first point, then each segment's points (arcs within 0.01 mm of
/// their chords).
pub fn path_polyline(path: &[PathSeg]) -> Result<Vec<Vec3>> {
    let first = match path.first() {
        Some(PathSeg::Line { a, .. } | PathSeg::Arc { a, .. }) => *a,
        None => return Err(KernelError::Invalid("the path needs 1…1000 segments".into())),
    };
    let mut pts = vec![first];
    for seg in path {
        pts.extend(seg.points_after_start(1e-2));
    }
    Ok(pts)
}

/// [`sweep`] with sweep options. The default options are exactly [`sweep`]; any other
/// orientation, a twist or an end scale follow the path as a polyline through stations (see
/// [`crate::sweep_path_with`]).
pub fn sweep_with(plane: &Plane, region: &Region2, path: &[PathSeg], ctl: &crate::SweepControl) -> Result<Body> {
    if ctl.is_default() {
        return sweep(plane, region, path);
    }
    ctl.check()?;
    region_ok(region)?;
    if path.is_empty() || path.len() > 1000 {
        return Err(KernelError::Invalid("the path needs 1…1000 segments".into()));
    }
    if path.iter().any(|s| s.tangents().is_none()) {
        return Err(KernelError::Invalid("a path segment has no direction".into()));
    }
    crate::sweep_path::sweep_path_with(plane, region, &path_polyline(path)?, ctl)
}

/// Sweep a planar region (outer loop) along a chain of path segments, keeping the profile
/// perpendicular to the path (lines translate it, arcs rotate it). A path with sharp corners
/// is swept as a polyline, mitred at the corners.
pub fn sweep(plane: &Plane, region: &Region2, path: &[PathSeg]) -> Result<Body> {
    region_ok(region)?;
    if path.is_empty() || path.len() > 1000 {
        return Err(KernelError::Invalid("the path needs 1…1000 segments".into()));
    }
    // Segment by segment the profile only follows a path whose direction runs on across each
    // joint: at a corner the next line would slide the profile within its own plane.
    let ends: Vec<(Vec3, Vec3)> =
        path.iter().map(PathSeg::tangents).collect::<Option<_>>().ok_or_else(|| KernelError::Invalid("a path segment has no direction".into()))?;
    let corner = ends.windows(2).any(|w| w[0].1.dot(w[1].0) < (1e-3f64).cos());
    if corner {
        return crate::sweep_path::sweep_path(plane, region, &path_polyline(path)?);
    }
    let points = path_polyline(path)?;
    if crate::sweep_path::closed_path(&points) {
        // Sketch circles have two half arcs. Sweep the whole turn in one operation so truck
        // shares the closing edges itself. Other closed paths use stations with shared ends.
        if let Some((center, axis, angle)) = full_circle(path) {
            return guard("closed sweep", || {
                let profile = region_wires(plane, &oriented(region)?)?;
                let mut sides = Vec::new();
                for p in &profile {
                    let shell: mt::Shell = builder::rsweep(&p.wire, p3(center), v3(axis), mt::Rad(angle));
                    let numbers = seeds(std::slice::from_ref(p));
                    let prov = record::columns(shell.face_iter(), &numbers, None, record::Column::ToEnd, &[]);
                    sides.extend(shell.face_iter().enumerate().map(|(i, f)| (f.clone(), prov.as_ref().and_then(|p| p.get(i)).cloned().flatten())));
                }
                close_sweep(sides)
            });
        }
        return crate::sweep_path::sweep_path(plane, region, &points);
    }
    // The outer loop and each hole, swept along the path; the ends are capped with holes.
    let profile = region_wires(plane, &oriented(region)?)?;
    let starts = wires_of(&profile);
    guard("sweep", || {
        let mut shell = mt::Shell::new();
        let mut ends = Vec::new();
        for w0 in &starts {
            let mut cur = w0.clone();
            for seg in path {
                let mut part: mt::Shell = match *seg {
                    PathSeg::Line { a, b } => builder::tsweep(&cur, v3(b - a)),
                    PathSeg::Arc { center, axis, angle, .. } => {
                        let ax = axis.normalized().ok_or_else(|| KernelError::Invalid("arc axis".into()))?;
                        builder::rsweep(&cur, p3(center), v3(ax), mt::Rad(angle))
                    }
                };
                let next = part
                    .extract_boundaries()
                    .into_iter()
                    .find(|w| w.edge_iter().all(|e| !cur.edge_iter().any(|c| c.id() == e.id())))
                    .ok_or_else(|| KernelError::Failed("sweep: lost the profile".into()))?;
                shell.append(&mut part);
                cur = next.inverse();
            }
            ends.push(cur);
        }
        // A straight profile edge square to an arc's axis turns in its own plane: that face is
        // given a plane, so that it is found lying on a planar face of another body.
        let mut bounds = solidcraft_geom::Aabb3::EMPTY;
        shell.vertex_iter().for_each(|v| bounds.add(from_p3(v.point())));
        let size = bounds.size().len();
        if size.is_finite()
            && let Some(flat) = crate::heal::planar_surfaces(&shell, (size * 1e-7).max(1e-9))
        {
            shell = flat;
        }
        // truck sweeps the faces: each profile edge's run along the whole path, path segment
        // after segment.
        let sides: Vec<Side> = match record::columns(shell.face_iter(), &seeds(&profile), None, record::Column::ToEnd, &[]) {
            Some(prov) => shell.face_iter().cloned().zip(prov).collect(),
            None => shell.face_iter().map(|f| (f.clone(), None)).collect(),
        };
        cap_and_close(sides, &starts, &ends, false)
    })
}

/// A connected, single turn about one centre and axis, possibly split into sketch arcs.
fn full_circle(path: &[PathSeg]) -> Option<(Vec3, Vec3, f64)> {
    let PathSeg::Arc { a: first, center, axis, .. } = *path.first()? else { return None };
    let axis = axis.normalized()?;
    let mut end = first;
    let mut total = 0.0;
    for seg in path {
        let PathSeg::Arc { a, center: c, axis: k, angle } = *seg else { return None };
        if a.dist(end) > 1e-7 || c.dist(center) > 1e-7 || k.normalized()?.dist(axis) > 1e-9 || !angle.is_finite() {
            return None;
        }
        total += angle;
        end = rotate_about(a, c, axis, angle);
    }
    ((total.abs() - std::f64::consts::TAU).abs() < 1e-9).then_some((center, axis, total))
}

/// Cox–de Boor basis values N_{j,3}(u) for all j.
fn cubic_basis(knots: &[f64], n_ctrl: usize, u: f64) -> Vec<f64> {
    let p = 3;
    let m = knots.len();
    let mut n = vec![0.0; m.saturating_sub(1)];
    let last = n_ctrl.saturating_sub(1);
    for (j, v) in n.iter_mut().enumerate() {
        let (Some(&a), Some(&b)) = (knots.get(j), knots.get(j + 1)) else { continue };
        // Half-open spans, with the last span closed at the end.
        if (a <= u && u < b) || (u >= b && (b - a) > 0.0 && j == last && (u - b).abs() < 1e-12) {
            *v = 1.0;
        }
    }
    for k in 1..=p {
        for j in 0..m.saturating_sub(1 + k) {
            let (Some(&tj), Some(&tjk), Some(&tj1), Some(&tjk1)) = (knots.get(j), knots.get(j + k), knots.get(j + 1), knots.get(j + k + 1)) else {
                continue;
            };
            let left = if tjk - tj > 0.0 { (u - tj) / (tjk - tj) * n.get(j).copied().unwrap_or(0.0) } else { 0.0 };
            let right = if tjk1 - tj1 > 0.0 { (tjk1 - u) / (tjk1 - tj1) * n.get(j + 1).copied().unwrap_or(0.0) } else { 0.0 };
            if let Some(v) = n.get_mut(j) {
                *v = left + right;
            }
        }
    }
    n.truncate(n_ctrl);
    n
}

/// Cubic B-spline through points at uniform parameters (clamped, knots by averaging).
pub(crate) fn interpolate_cubic(pts: &[Vec3]) -> Option<mt::BSplineCurve<mt::Point3>> {
    let m = pts.len();
    let u: Vec<f64> = (0..m).map(|i| i as f64 / (m.max(2) - 1) as f64).collect();
    interpolate_cubic_at(pts, &u)
}

/// [`interpolate_cubic`] by chord length: the parameters (0 to 1) in proportion to the distance
/// along the points. Points closer together toward an end (where a curve leaves another
/// tangentially) then don't slow the curve to a stop there, where a uniform parameter lets it
/// double back on itself.
pub(crate) fn interpolate_cubic_chordal(pts: &[Vec3]) -> Option<(mt::BSplineCurve<mt::Point3>, Vec<f64>)> {
    let mut u = vec![0.0];
    for w in pts.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { return None };
        u.push(u.last().copied().unwrap_or(0.0) + a.dist(*b));
    }
    let total = u.last().copied().filter(|t| t.is_finite() && *t > 0.0)?;
    let u: Vec<f64> = u.iter().map(|x| x / total).collect();
    if u.windows(2).any(|w| w.first() >= w.get(1)) {
        return None;
    }
    interpolate_cubic_at(pts, &u).map(|c| (c, u))
}

/// The cubic B-spline through `pts` at parameters `u` (rising from 0 to 1), its knots averaged.
fn interpolate_cubic_at(pts: &[Vec3], u: &[f64]) -> Option<mt::BSplineCurve<mt::Point3>> {
    let m = pts.len();
    if m < 4 || u.len() != m {
        return None;
    }
    let mut knots = vec![0.0; 4];
    for j in 1..(m - 3) {
        knots.push((j..j + 3).filter_map(|i| u.get(i)).sum::<f64>() / 3.0);
    }
    knots.extend([1.0; 4]);
    // Dense solve of N · P = Q for each coordinate.
    let mut a: Vec<Vec<f64>> = u.iter().map(|ui| cubic_basis(&knots, m, *ui)).collect();
    let mut q: Vec<[f64; 3]> = pts.iter().map(|p| [p.x, p.y, p.z]).collect();
    for col in 0..m {
        let piv = (col..m).max_by(|x, y| a[*x][col].abs().total_cmp(&a[*y][col].abs()))?;
        if a[piv][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, piv);
        q.swap(col, piv);
        for r in 0..m {
            if r == col {
                continue;
            }
            let f = a[r][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            let prow = a[col].clone();
            for (x, v) in a[r].iter_mut().zip(&prow).skip(col) {
                *x -= f * v;
            }
            let qc = q[col];
            for k in 0..3 {
                q[r][k] -= f * qc[k];
            }
        }
    }
    let ctrl: Vec<mt::Point3> = (0..m).map(|i| mt::Point3::new(q[i][0] / a[i][i], q[i][1] / a[i][i], q[i][2] / a[i][i])).collect();
    mt::BSplineCurve::try_new(mt::KnotVec::from(knots), ctrl).ok()
}

/// A closed wire whose edges are cubic B-splines parameterised (nearly) by arc length, so that
/// ruled surfaces between sections pair points proportionally along each edge.
pub(crate) fn uniform_wire(plane: &Plane, lp: &Loop2) -> Result<mt::Wire> {
    uniform_chain_wire(plane, &lp.segs, true)
}

/// [`uniform_wire`] of a chain of segments, open or `closed`.
pub(crate) fn uniform_chain_wire(plane: &Plane, segs: &[Seg2], closed: bool) -> Result<mt::Wire> {
    const SAMPLES: usize = 24;
    let n = segs.len();
    let mut verts: Vec<mt::Vertex> = segs.iter().map(|s| builder::vertex(p3(plane.to_world(s.start())))).collect();
    if !closed && let Some(last) = segs.last() {
        verts.push(builder::vertex(p3(plane.to_world(last.end()))));
    }
    let mut edges = Vec::with_capacity(n);
    for (i, s) in segs.iter().enumerate() {
        let (Some(a), Some(b)) = (verts.get(i), verts.get(if closed { (i + 1) % n } else { i + 1 })) else { continue };
        let pts: Vec<Vec3> = (0..SAMPLES).map(|k| plane.to_world(s.point_at(k as f64 / (SAMPLES - 1) as f64))).collect();
        let curve = interpolate_cubic(&pts).ok_or_else(|| KernelError::Failed("loft curve".into()))?;
        edges.push(mt::Edge::new(a, b, mt::Curve::BSplineCurve(curve)));
    }
    Ok(edges.into())
}

/// A sphere as six patches (the cube's faces projected onto it) on revolved surfaces whose poles
/// and seams lie outside each patch. No face has a singular point, which the boolean needs; the
/// patches are one analytic sphere, so measured topology counts it as one face.
pub fn sphere_patches(center: Vec3, radius: f64) -> Result<Body> {
    sphere_patches_rotated(center, radius, [Vec3::X, Vec3::Y, Vec3::Z])
}

/// [`sphere_patches`] with the patch layout turned to the frame `rot` (orthonormal columns).
pub fn sphere_patches_rotated(center: Vec3, radius: f64, rot: [Vec3; 3]) -> Result<Body> {
    // Every patch is swept from the arc (segment 0) of the half disc a sphere revolves.
    sphere_solid(center, radius, rot, &[0])
}

/// [`sphere_patches_rotated`], every patch swept from profile segments `arcs`.
fn sphere_solid(center: Vec3, radius: f64, rot: [Vec3; 3], arcs: &[usize]) -> Result<Body> {
    guard("sphere", || {
        let shell: mt::Shell = sphere_patch_faces(center, radius, rot)?.into();
        let solid = Solid::try_new(vec![shell]).map_err(|e| KernelError::Failed(format!("sphere: {e}")))?;
        record::attach(Body::new(solid)?, &[], Some(vec![record::profile(arcs); 6]), Default::default())
    })
}

/// The six outward patch faces of [`sphere_patches_rotated`]; truck calls, so callers guard them.
pub(crate) fn sphere_patch_faces(center: Vec3, radius: f64, rot: [Vec3; 3]) -> Result<Vec<mt::Face>> {
    let radius = finite(radius, "radius")?;
    let turn = |v: Vec3| rot[0] * v.x + rot[1] * v.y + rot[2] * v.z;
    if radius <= 1e-6 || !center.is_finite() {
        return Err(KernelError::Invalid("radius must be positive".into()));
    }
    let k = radius / 3f64.sqrt();
    let corner = |sx: f64, sy: f64, sz: f64| center + turn(Vec3::new(sx * k, sy * k, sz * k));
    // Cube faces: outward direction, rotation axis for the patch's surface (perpendicular to it),
    // and the four corner sign triples counter-clockwise seen from outside.
    let faces: [(Vec3, Vec3, [[f64; 3]; 4]); 6] = [
        (Vec3::X, Vec3::Z, [[1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [1.0, 1.0, 1.0], [1.0, -1.0, 1.0]]),
        (-Vec3::X, Vec3::Z, [[-1.0, 1.0, -1.0], [-1.0, -1.0, -1.0], [-1.0, -1.0, 1.0], [-1.0, 1.0, 1.0]]),
        (Vec3::Y, Vec3::Z, [[1.0, 1.0, -1.0], [-1.0, 1.0, -1.0], [-1.0, 1.0, 1.0], [1.0, 1.0, 1.0]]),
        (-Vec3::Y, Vec3::Z, [[-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, -1.0, 1.0], [-1.0, -1.0, 1.0]]),
        (Vec3::Z, Vec3::X, [[-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0]]),
        (-Vec3::Z, Vec3::X, [[-1.0, 1.0, -1.0], [1.0, 1.0, -1.0], [1.0, -1.0, -1.0], [-1.0, -1.0, -1.0]]),
    ];
    let key = |s: [f64; 3]| ((s[0] > 0.0) as usize) | (((s[1] > 0.0) as usize) << 1) | (((s[2] > 0.0) as usize) << 2);
    let mut verts: std::collections::HashMap<usize, mt::Vertex> = std::collections::HashMap::new();
    let mut vert = |s: [f64; 3]| verts.entry(key(s)).or_insert_with(|| builder::vertex(p3(corner(s[0], s[1], s[2])))).clone();
    let mut edges: std::collections::HashMap<(usize, usize), mt::Edge> = std::collections::HashMap::new();
    let mut out: Vec<mt::Face> = Vec::new();
    for (dir, axis, cs) in faces {
        let (dir, axis) = (turn(dir), turn(axis));
        let mut wire: Vec<mt::Edge> = Vec::new();
        for i in 0..4 {
            let (a, b) = (cs[i], cs[(i + 1) % 4]);
            let (ka, kb) = (key(a), key(b));
            let e = match edges.get(&(ka.min(kb), ka.max(kb))) {
                Some(e) => e.clone(),
                None => {
                    let (lo, hi) = if ka < kb { (a, b) } else { (b, a) };
                    let (va, vb) = (vert(lo), vert(hi));
                    let mid = (corner(lo[0], lo[1], lo[2]) + corner(hi[0], hi[1], hi[2])) * 0.5 - center;
                    let transit = center + mid.normalized().unwrap_or(dir) * radius;
                    let e = builder::circle_arc(&va, &vb, p3(transit));
                    edges.insert((ka.min(kb), ka.max(kb)), e.clone());
                    e
                }
            };
            wire.push(if ka < kb { e } else { e.inverse() });
        }
        // The meridian runs pole to pole through the side away from the patch.
        let (np, sp, back) = (center + axis * radius, center - axis * radius, center - dir * radius);
        let meridian = builder::circle_arc(&builder::vertex(p3(np)), &builder::vertex(p3(sp)), p3(back));
        let mut surface =
            mt::Surface::RevolutedCurve(mt::Processor::new(mt::RevolutedCurve::by_revolution(meridian.oriented_curve(), p3(center), v3(axis))));
        // Outward normal at the patch centre.
        let probe = p3(center + dir * radius);
        use mt::{ParametricSurface3D, SearchParameter};
        if let Some((u, v)) = surface.search_parameter(probe, mt::SPHint2D::None, 100) {
            let n = surface.normal(u, v);
            if mt::InnerSpace::dot(n, v3(dir)) < 0.0 {
                mt::Invertible::invert(&mut surface);
            }
        }
        out.push(mt::Face::new(vec![wire.into()], surface));
    }
    Ok(out)
}
