//! Surface bodies (open shells) and the operations that turn them into solids: Patch (a face
//! filling a closed loop of edges — planar, or B-spline patches for a loop that is not — or a
//! planar sketch region), Stitch (surfaces sharing edges sewn
//! into one body, a solid when they close), Trim (a surface cut by a tool, one side kept),
//! Thicken (a surface offset along its normals into a solid), copying faces out of a body, and
//! sheets swept from sketch curves (surface extrude, revolve, sweep and loft).

use std::collections::HashMap;

use mt::{BoundedCurve, InnerSpace, MetricSpace, ParametricCurve, ParametricSurface3D, SearchNearestParameter};
use solidcraft_geom::{Plane, Region2, Seg2, Vec2, Vec3};
use truck_modeling as mt;

use crate::body::{Body, from_p3, p3};
use crate::history::{Change, EdgeOrigin, FaceOrigin, FaceProv};
use crate::record::{self, Rebuild, Side, rebuilt_body};
use crate::{KernelError, Result, guard};

impl Body {
    /// A surface body: some shell does not close.
    pub fn is_surface(&self) -> bool {
        !self.is_mesh() && self.solid.boundaries().iter().any(|s| s.shell_condition() != mt::ShellCondition::Closed)
    }
}

/// A body of faces as they are (no orientation fix: a surface has no inside).
pub(crate) fn surface_body(faces: Vec<mt::Face>) -> Body {
    surface_of(mt::Shell::from(faces))
}

/// A body of a shell as it is.
fn surface_of(shell: mt::Shell) -> Body {
    Body {
        solid: std::sync::Arc::new(mt::Solid::new_unchecked(vec![shell])),
        size: 0.0,
        volume: Some(std::sync::Arc::new(std::sync::Mutex::new(None))),
        coarse_missing: None,
        mesh: None,
        color: None,
        paint: None,
        splits: None,
        face_sources: None,
        history: None,
        display_of: None,
        regularization_protected: false,
    }
}

/// A body from faces: a solid when they close, else a surface; with the history of the
/// operation that made the faces from `operands` as `rec` records.
pub(crate) fn body_of(faces: Vec<mt::Face>, operands: &[&mt::Solid], rec: &Rebuild) -> Result<Body> {
    let shell = mt::Shell::from(faces);
    if shell.shell_condition() == mt::ShellCondition::Closed {
        crate::record::rebuilt(mt::Solid::new_unchecked(vec![shell]), operands, rec)
    } else {
        Ok(rebuilt_body(surface_of(shell), operands, rec))
    }
}

/// A planar surface filling a sketch region. Its history has no operands: the face is new.
pub fn patch_region(plane: &Plane, region: &Region2) -> Result<Body> {
    let f = crate::build::face(plane, region)?;
    let mut rec = Rebuild::default();
    rec.made(&f, Vec::new());
    Ok(rebuilt_body(surface_body(vec![f]), &[], &rec))
}

/// A chain's edge seeds numbered across all the chains: the segment at `i` of the chain that
/// starts at segment `offset` is segment `offset + i`.
fn numbered(seeds: &[(mt::EdgeID, usize)], offset: usize) -> Vec<(mt::EdgeID, Vec<usize>)> {
    seeds.iter().map(|(id, i)| (*id, vec![offset + i])).collect()
}

/// The faces truck swept from the edges `seeds`, each with the segments under the edge whose
/// column it lies in (none where the faces don't follow the pattern; see [`record::follow`]).
fn swept_sides(shell: &mt::Shell, seeds: &[(mt::EdgeID, Vec<usize>)], fixed: &[mt::VertexID]) -> Vec<Side> {
    let provs = record::columns(shell.face_iter(), seeds, None, record::Column::ToEnd, fixed);
    shell.face_iter().enumerate().map(|(k, f)| (f.clone(), provs.as_ref().and_then(|p| p.get(k).cloned().flatten()))).collect()
}

/// A body of a sheet builder's faces: a surface, or a solid where they close; with the history
/// of a body built from the chains' segments (none unless every face's is known).
fn sheet_body(sides: Vec<Side>) -> Result<Body> {
    let history: Option<Vec<FaceProv>> = sides.iter().map(|(_, p)| p.clone()).collect();
    let shell = mt::Shell::from(sides.into_iter().map(|(f, _)| f).collect::<Vec<_>>());
    if shell.shell_condition() == mt::ShellCondition::Closed {
        return match crate::body::Solid::try_new(vec![shell.clone()]) {
            Ok(solid) => record::attach(Body::new(solid)?, &[], history, Default::default()),
            Err(_) => crate::record::rebuilt(mt::Solid::new_unchecked(vec![shell]), &[], &Rebuild::default()),
        };
    }
    record::attach(surface_of(shell), &[], history, Default::default())
}

/// Most curve chains one surface extrude takes.
const MAX_CHAINS: usize = 10_000;

/// A surface extruded from chains of sketch segments on `plane`: each chain (open, or closed
/// such as a profile's boundary) moved straight from `start` to `end` along the plane normal
/// (offsets in mm). The walls it sweeps form one surface body with no caps. Its history has no
/// operands: every face is new.
pub fn extrude_sheet(plane: &Plane, chains: &[Vec<Seg2>], start: f64, end: f64) -> Result<Body> {
    if !crate::build::frame_ok(plane) {
        return Err(KernelError::Invalid("extrude frame is not finite and orthonormal".into()));
    }
    if !(start.is_finite() && end.is_finite() && start.abs() < 1e7 && end.abs() < 1e7) {
        return Err(KernelError::Invalid("extrude distance is not finite".into()));
    }
    let (lo, hi) = if end >= start { (start, end) } else { (end, start) };
    if hi - lo < 1e-6 {
        return Err(KernelError::Invalid("extrude distance is zero".into()));
    }
    if chains.is_empty() || chains.len() > MAX_CHAINS {
        return Err(KernelError::Invalid("pick the curves or profiles to extrude".into()));
    }
    let base = plane.offset(lo);
    let along = crate::body::v3(plane.normal() * (hi - lo));
    let mut sides = Vec::new();
    let mut offset = 0;
    for chain in chains {
        let (wire, seeds) = crate::build::chain_wire_seeds(&base, chain)?;
        let walls: mt::Shell = guard("surface extrude", || Ok(mt::builder::tsweep(&wire, along)))?;
        sides.extend(swept_sides(&walls, &numbered(&seeds, offset), &[]));
        offset += chain.len();
    }
    sheet_body(sides)
}

/// A surface revolved from chains of sketch segments on `plane` about the sketch line through
/// `axis_origin` along `axis_dir`: each chain turned from `start` through `sweep` radians (a
/// full turn at most), positive angles carrying it toward the sketch normal. A chain end on the
/// axis stays put there (a cone's apex, a sphere's pole). The walls form one surface body with
/// no caps, or a solid where they close (a full turn of a closed curve, or of a curve from the
/// axis back to it); its history has no operands: every face is new.
pub fn revolve_sheet(plane: &Plane, chains: &[Vec<Seg2>], axis_origin: Vec2, axis_dir: Vec2, start: f64, sweep: f64) -> Result<Body> {
    use std::f64::consts::TAU;
    if !crate::build::frame_ok(plane) || !axis_origin.is_finite() || !axis_dir.is_finite() {
        return Err(KernelError::Invalid("revolve frame is not finite and orthonormal".into()));
    }
    if !(start.is_finite() && sweep.is_finite() && start.abs() <= TAU && sweep.abs() <= TAU + 1e-9) {
        return Err(KernelError::Invalid("revolve angles must be finite, a full turn at most".into()));
    }
    if sweep.abs() < 1e-6 {
        return Err(KernelError::Invalid("revolve angle is zero".into()));
    }
    if chains.is_empty() || chains.len() > MAX_CHAINS {
        return Err(KernelError::Invalid("pick the curves or profiles to revolve".into()));
    }
    let d = axis_dir.normalized().ok_or_else(|| KernelError::Invalid("revolve axis has no direction".into()))?;
    let side = |p: Vec2| d.cross(p - axis_origin);
    let on_axis = |p: Vec2| side(p).abs() < 1e-7;
    let (origin, n) = (plane.to_world(axis_origin), plane.normal());
    let axis = plane.dir_to_world(d).normalized().ok_or_else(|| KernelError::Invalid("revolve frame has no axis".into()))?;
    let mut sides = Vec::new();
    let mut offset = 0;
    for chain in chains {
        // On one side of the axis; the point farthest from it fixes the turning sense.
        let pts: Vec<Vec2> = chain.iter().flat_map(|s| s.polyline(1e-3)).collect();
        if pts.iter().any(|p| side(*p) < -1e-7) && pts.iter().any(|p| side(*p) > 1e-7) {
            return Err(KernelError::Invalid("the curves cross the revolve axis".into()));
        }
        let far = pts.iter().copied().max_by(|a, b| side(*a).abs().total_cmp(&side(*b).abs())).filter(|p| !on_axis(*p));
        let Some(far) = far else { return Err(KernelError::Invalid("the curves lie on the revolve axis".into())) };
        let radial = plane.to_world(far) - origin;
        let a = if axis.cross(radial).dot(n) < 0.0 { -axis } else { axis };
        let (first, last) = match (chain.first(), chain.last()) {
            (Some(f), Some(l)) => (f.start(), l.end()),
            _ => return Err(KernelError::Invalid("an empty curve chain".into())),
        };
        let closed = first.dist(last) <= 1e-6;
        if closed && pts.iter().any(|p| on_axis(*p)) {
            return Err(KernelError::Invalid("not supported yet: a closed curve touching the revolve axis".into()));
        }
        // The chain starts `start` round: its sketch frame turned there, so the wire's own edges
        // are the ones swept (and its faces can be followed back to them).
        let turn = |v: Vec3| crate::build::rotate_about(v, Vec3::ZERO, a, start);
        let turned = Plane { origin: crate::build::rotate_about(plane.origin, origin, a, start), x: turn(plane.x), y: turn(plane.y) };
        let (wire, seeds) = crate::build::chain_wire_seeds(&turned, chain)?;
        let fixed: Vec<mt::VertexID> = [(on_axis(first), wire.front_vertex()), (on_axis(last), wire.back_vertex())]
            .into_iter()
            .filter_map(|(apex, v)| v.filter(|_| apex).map(|v| v.id()))
            .collect();
        let walls: mt::Shell = guard("surface revolve", || {
            Ok(match (on_axis(first), on_axis(last)) {
                (true, _) => mt::builder::cone(&wire, crate::body::v3(a), mt::Rad(sweep)),
                (false, true) => mt::builder::cone(&wire.inverse(), crate::body::v3(a), mt::Rad(sweep)),
                (false, false) => mt::builder::rsweep(&wire, p3(origin), crate::body::v3(a), mt::Rad(sweep)),
            })
        })?;
        sides.extend(swept_sides(&walls, &numbered(&seeds, offset), &fixed));
        offset += chain.len();
    }
    sheet_body(sides)
}

/// A surface swept from chains of sketch segments on `plane` along a path that starts near
/// them. Along a path whose direction runs on across every joint, lines slide the chains and
/// arcs turn them, exactly; any other path (curves followed as polylines) goes through
/// [`sweep_sheet_points`]. The walls form one surface body with no caps (a solid where they
/// close); its history has no operands: every face is new.
pub fn sweep_sheet(plane: &Plane, chains: &[Vec<Seg2>], path: &[crate::PathSeg]) -> Result<Body> {
    use crate::PathSeg;
    if !crate::build::frame_ok(plane) {
        return Err(KernelError::Invalid("sweep frame is not finite and orthonormal".into()));
    }
    if chains.is_empty() || chains.len() > MAX_CHAINS {
        return Err(KernelError::Invalid("pick the curves or profiles to sweep".into()));
    }
    if path.is_empty() || path.len() > 1000 {
        return Err(KernelError::Invalid("the path needs 1…1000 segments".into()));
    }
    let ends: Vec<(Vec3, Vec3)> =
        path.iter().map(PathSeg::tangents).collect::<Option<_>>().ok_or_else(|| KernelError::Invalid("a path segment has no direction".into()))?;
    if ends.windows(2).any(|w| w[0].1.dot(w[1].0) < (1e-3f64).cos()) {
        return sweep_sheet_points(plane, chains, &crate::build::path_polyline(path)?);
    }
    let mut sides = Vec::new();
    let mut offset = 0;
    for chain in chains {
        let (mut cur, seeds) = crate::build::chain_wire_seeds(plane, chain)?;
        // Each edge's segments: the next stretch of the path starts from the edges the last one
        // carried them onto, in the same order.
        let mut seeds = numbered(&seeds, offset);
        for seg in path {
            let (part, map): (mt::Shell, Box<dyn Fn(Vec3) -> Vec3>) = match *seg {
                PathSeg::Line { a, b } => {
                    (guard("surface sweep", || Ok(mt::builder::tsweep(&cur, crate::body::v3(b - a))))?, Box::new(move |p| p + (b - a)))
                }
                PathSeg::Arc { center, axis, angle, .. } => {
                    let k = axis.normalized().ok_or_else(|| KernelError::Invalid("a path arc has no axis".into()))?;
                    let part = guard("surface sweep", || Ok(mt::builder::rsweep(&cur, p3(center), crate::body::v3(k), mt::Rad(angle))))?;
                    (part, Box::new(move |p| crate::build::rotate_about(p, center, k, angle)))
                }
            };
            let next = far_wire(&part, &cur, map.as_ref())?;
            sides.extend(swept_sides(&part, &seeds, &[]));
            seeds = next.edge_iter().map(|e| e.id()).zip(seeds.into_iter().map(|(_, n)| n)).collect();
            cur = next;
        }
        offset += chain.len();
    }
    sheet_body(sides)
}

/// A surface swept from chains of sketch segments on `plane` along a 3D polyline path: one smooth
/// B-spline wall per segment where the path has no corners, else ruled walls between the path's
/// points, mitred at the corners (see [`sweep_sheet`]).
pub fn sweep_sheet_points(plane: &Plane, chains: &[Vec<Seg2>], path: &[Vec3]) -> Result<Body> {
    sweep_sheet_points_with(plane, chains, path, &crate::SweepControl::default())
}

/// [`sweep_sheet`] with sweep options. The default options are exactly [`sweep_sheet`]; any
/// other orientation, a twist or an end scale follow the path as a polyline through stations
/// (see [`sweep_sheet_points_with`]).
pub fn sweep_sheet_with(plane: &Plane, chains: &[Vec<Seg2>], path: &[crate::PathSeg], ctl: &crate::SweepControl) -> Result<Body> {
    if ctl.is_default() {
        return sweep_sheet(plane, chains, path);
    }
    ctl.check()?;
    if path.is_empty() || path.len() > 1000 {
        return Err(KernelError::Invalid("the path needs 1…1000 segments".into()));
    }
    if path.iter().any(|s| s.tangents().is_none()) {
        return Err(KernelError::Invalid("a path segment has no direction".into()));
    }
    sweep_sheet_points_with(plane, chains, &crate::build::path_polyline(path)?, ctl)
}

/// [`sweep_sheet_points`] with sweep options: orientation, twist and end scale.
pub fn sweep_sheet_points_with(plane: &Plane, chains: &[Vec<Seg2>], path: &[Vec3], ctl: &crate::SweepControl) -> Result<Body> {
    ctl.check()?;
    if !crate::build::frame_ok(plane) {
        return Err(KernelError::Invalid("sweep frame is not finite and orthonormal".into()));
    }
    if chains.is_empty() || chains.len() > MAX_CHAINS {
        return Err(KernelError::Invalid("pick the curves or profiles to sweep".into()));
    }
    let ruled = sheet_body(crate::sweep_path::ruled_sheet(plane, chains, path, ctl)?)?;
    // Smooth walls where the path has no corners, kept when their area agrees with the ruled
    // walls' (a fit that went wrong falls back to those).
    if let Ok(Some(sides)) = crate::sweep_path::smooth_sheet(plane, chains, path, ctl)
        && let Ok(smooth) = sheet_body(sides)
        && let (Ok(a), Ok(b)) = (crate::measure(&smooth), crate::measure(&ruled))
        && (a.area - b.area).abs() <= 0.02 * b.area.abs()
    {
        return Ok(smooth);
    }
    Ok(ruled)
}

/// The far side of a sweep of the wire `start`: for each of its edges, in order and the same
/// way round, the edge of `swept` that `map` carries it onto (matched by its ends and middle).
fn far_wire(swept: &mt::Shell, start: &mt::Wire, map: &dyn Fn(Vec3) -> Vec3) -> Result<mt::Wire> {
    let near = |p: Vec3, q: Vec3| p.dist(q) <= 1e-6 * (1.0 + p.len().max(q.len()));
    let mid = |e: &mt::Edge| {
        let c = e.oriented_curve();
        let (t0, t1) = c.range_tuple();
        from_p3(c.subs((t0 + t1) / 2.0))
    };
    let mut out = Vec::new();
    for e in start.edge_iter() {
        let (a, b, m) = (map(from_p3(e.front().point())), map(from_p3(e.back().point())), map(mid(e)));
        let found = swept.edge_iter().find_map(|f| {
            let (fa, fb) = (from_p3(f.front().point()), from_p3(f.back().point()));
            if !near(mid(&f), m) {
                None
            } else if near(fa, a) && near(fb, b) {
                Some(f)
            } else if near(fa, b) && near(fb, a) {
                Some(f.inverse())
            } else {
                None
            }
        });
        out.push(found.ok_or_else(|| KernelError::Failed("sweep: lost the curves along the path".into()))?);
    }
    Ok(out.into())
}

/// A surface lofted through two or more sections, each a chain of sketch segments on its plane,
/// all open or all closed: ruled between two sections, one smooth face per segment through
/// three or more. Closed sections are matched as the solid loft matches them; open ones are
/// split to the same number of segments (longest first) and each turned to run the way the one
/// before it does. Walls only, no caps; the history has no operands: every face is new.
pub fn loft_sheet(sections: &[(Plane, Vec<Seg2>)]) -> Result<Body> {
    if sections.len() < 2 || sections.len() > 50 {
        return Err(KernelError::Invalid("a loft needs 2…50 sections".into()));
    }
    let closed: Vec<bool> = sections
        .iter()
        .map(|(_, c)| match (c.first(), c.last()) {
            (Some(f), Some(l)) if c.len() <= 4096 => Ok(l.end().dist(f.start()) <= 1e-6),
            _ => Err(KernelError::Invalid("a loft section has no curves, or too many".into())),
        })
        .collect::<Result<_>>()?;
    let all_closed = closed.iter().all(|c| *c);
    if !all_closed && closed.iter().any(|c| *c) {
        return Err(KernelError::Invalid("the loft sections must be all open or all closed".into()));
    }
    for (plane, chain) in sections {
        // Checks the frame, the numbers and that each chain runs end to end.
        if !crate::build::frame_ok(plane) {
            return Err(KernelError::Invalid("a loft section's frame is not finite and orthonormal".into()));
        }
        crate::build::chain_wire(plane, chain)?;
    }
    let (wires, sources) = if all_closed {
        let loops: Vec<(Plane, solidcraft_geom::Loop2)> = sections.iter().map(|(p, c)| (*p, solidcraft_geom::Loop2 { segs: c.clone() })).collect();
        let (wires, sources, _) = crate::build::loft_wires(&loops)?;
        (wires, sources)
    } else {
        let n = sections.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
        let mut wires = Vec::with_capacity(sections.len());
        let mut sources = Vec::with_capacity(sections.len());
        let mut prev: Option<(Vec3, Vec3)> = None;
        // Segments are numbered through the sections in the order given.
        let mut offset = 0;
        for (plane, chain) in sections {
            // Each piece with the segment it is (part of).
            let mut segs: Vec<(Seg2, usize)> = chain.iter().enumerate().map(|(i, s)| (*s, offset + i)).collect();
            offset += chain.len();
            // Split the longest segment in half until the counts match.
            while segs.len() < n {
                let Some(i) = segs.iter().enumerate().max_by(|a, b| a.1.0.length().total_cmp(&b.1.0.length())).map(|(i, _)| i) else { break };
                let (s, k) = segs[i];
                let (a, b) = s.split_half();
                segs.splice(i..=i, [(a, k), (b, k)]);
            }
            let ends = |c: &[(Seg2, usize)]| c.first().zip(c.last()).map(|(f, l)| (plane.to_world(f.0.start()), plane.to_world(l.0.end())));
            if let (Some((ps, pe)), Some((s, e))) = (prev, ends(&segs))
                && s.dist(ps) + e.dist(pe) > s.dist(pe) + e.dist(ps)
            {
                segs = segs.iter().rev().map(|(s, k)| (s.reversed(), *k)).collect();
            }
            prev = ends(&segs);
            let pieces: Vec<Seg2> = segs.iter().map(|(s, _)| *s).collect();
            wires.push(crate::build::uniform_chain_wire(plane, &pieces, false)?);
            sources.push(segs.iter().map(|(_, k)| vec![*k]).collect::<Vec<_>>());
        }
        (wires, sources)
    };
    let faces = guard("surface loft", || {
        let smooth = if wires.len() >= 3 { crate::build::smooth_loft_faces(&wires, &sources, all_closed).ok() } else { None };
        match smooth {
            Some(f) => Ok(f),
            None => crate::build::ruled_loft_faces(&wires, &sources),
        }
    })?;
    sheet_body(faces)
}

/// The edge of a body nearest a point (by samples along each edge).
pub(crate) fn edge_near(body: &Body, p: Vec3) -> Option<mt::Edge> {
    let mut best: Option<(f64, mt::Edge)> = None;
    for e in body.solid.edge_iter() {
        let c = e.curve();
        let (t0, t1) = c.range_tuple();
        for k in 0..=32 {
            let d = from_p3(c.subs(t0 + (t1 - t0) * k as f64 / 32.0)).dist(p);
            if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, e.clone()));
            }
        }
    }
    best.filter(|(d, _)| *d <= body.size() * 0.05 + 1e-6).map(|(_, e)| e)
}

/// A surface filling a closed loop of a body's edges (each picked by a point on it): one face on
/// the body's own edges, planar when the loop is certainly planar ([`crate::plane_fit`]), else a
/// B-spline height field over the loop's plane ([`crate::fill`]; loops that do not bound one are
/// refused). Its history is over `[body]`: the face is new, its edges the body's own.
pub fn patch_edges(body: &Body, at: &[Vec3]) -> Result<Body> {
    body.require_brep("patch")?;
    if at.is_empty() || at.len() > 10_000 {
        return Err(KernelError::Invalid("pick the edges of a closed loop".into()));
    }
    guard("patch", || {
        let chain = picked_loop(body, at)?;
        let Some(face) = crate::plane_fit::planar_face(&chain, crate::fill::planarity_tolerance(body))? else {
            // Not certainly planar: a fitted surface.
            return crate::fill::fill_loop(body, &chain);
        };
        // A new face on the body's own edges.
        let mut rec = Rebuild::default();
        rec.made(&face, Vec::new());
        Ok(rebuilt_body(surface_body(vec![face]), &[&body.solid], &rec))
    })
}

/// The edges of `body` picked by points on them, chained into one closed loop (each turned to
/// run along it). Call it under [`guard`]: sampling an intersection curve can fail in truck.
pub(crate) fn picked_loop(body: &Body, at: &[Vec3]) -> Result<Vec<mt::Edge>> {
    match picked_chain(body, at)? {
        (chain, true) => Ok(chain),
        (_, false) => Err(KernelError::Invalid("the edges do not form a closed loop".into())),
    }
}

/// The edges of `body` picked by points on them, chained end to end (each turned to run along
/// the chain), and whether the chain closes: a closed loop starts with the last edge picked; an
/// open chain runs from the free end met first in pick order. Edges that branch, or form more
/// than one chain, are refused. Call it under [`guard`], as [`picked_loop`].
pub(crate) fn picked_chain(body: &Body, at: &[Vec3]) -> Result<(Vec<mt::Edge>, bool)> {
    let mut edges: Vec<mt::Edge> = Vec::new();
    for p in at {
        let e = edge_near(body, *p).ok_or_else(|| KernelError::Invalid(format!("no edge at {p:?}")))?;
        if !edges.iter().any(|x| x.id() == e.id()) {
            edges.push(e);
        }
    }
    let mut uses: HashMap<mt::VertexID, usize> = HashMap::new();
    for e in &edges {
        *uses.entry(e.front().id()).or_default() += 1;
        *uses.entry(e.back().id()).or_default() += 1;
    }
    if uses.values().any(|n| *n > 2) {
        return Err(KernelError::Invalid("the edges branch: pick one chain".into()));
    }
    let free = |v: &mt::Vertex| uses.get(&v.id()) == Some(&1);
    let open_end = edges.iter().find_map(|e| [e.front(), e.back()].into_iter().find(|v| free(v)).cloned());
    let mut pool = edges;
    let (first, closed) = match &open_end {
        Some(v) => {
            let k = pool.iter().position(|e| e.front().id() == v.id() || e.back().id() == v.id());
            let e = k.map(|k| pool.remove(k)).ok_or_else(|| KernelError::Invalid("no edges".into()))?;
            (if e.front().id() == v.id() { e } else { e.inverse() }, false)
        }
        None => (pool.pop().ok_or_else(|| KernelError::Invalid("no edges".into()))?, true),
    };
    let start = first.front().id();
    let mut at_v = first.back().clone();
    let mut chain = vec![first];
    while !(closed && at_v.id() == start) {
        let Some(k) = pool.iter().position(|e| e.front().id() == at_v.id() || e.back().id() == at_v.id()) else { break };
        let e = pool.remove(k);
        let e = if e.front().id() == at_v.id() { e } else { e.inverse() };
        at_v = e.back().clone();
        chain.push(e);
    }
    if !pool.is_empty() {
        let what = if closed { "the edges form more than one loop" } else { "the edges form more than one chain" };
        return Err(KernelError::Invalid(what.into()));
    }
    Ok((chain, closed))
}

/// Faces of a body copied out as a surface body (faces picked by points on them). Its history is
/// over `[body]`: the faces and their edges are the body's.
pub fn copy_faces(body: &Body, at: &[Vec3]) -> Result<Body> {
    body.require_brep("copy faces")?;
    let faces: Vec<&mt::Face> = body.solid.face_iter().collect();
    let mut chosen = Vec::new();
    for p in at {
        // The face whose mesh passes nearest the point.
        let i = nearest_face(body, *p)?;
        if !chosen.contains(&i) {
            chosen.push(i);
        }
    }
    let copied = surface_body(chosen.into_iter().filter_map(|i| faces.get(i).map(|f| (*f).clone())).collect());
    Ok(rebuilt_body(copied, &[&body.solid], &Rebuild::default()))
}

/// The faces picked by points on them (each once, in the order picked); every face when no
/// point is given.
fn picked_faces(body: &Body, at: &[Vec3]) -> Result<Vec<usize>> {
    if at.len() > 10_000 {
        return Err(KernelError::Invalid("too many faces picked".into()));
    }
    if at.is_empty() {
        return Ok((0..body.face_count()).collect());
    }
    let mut chosen = Vec::new();
    for p in at {
        let i = nearest_face(body, *p)?;
        if !chosen.contains(&i) {
            chosen.push(i);
        }
    }
    Ok(chosen)
}

/// A body's faces taken apart: each picked face (by points on it; every face when none are
/// given) its own surface body, and the faces left, if any, one surface body. A solid comes
/// apart into surfaces. Histories are over `[body]`: each face is the body's.
pub fn unstitch(body: &Body, at: &[Vec3]) -> Result<(Vec<Body>, Option<Body>)> {
    body.require_brep("unstitch")?;
    let chosen = picked_faces(body, at)?;
    let faces: Vec<&mt::Face> = body.solid.face_iter().collect();
    if faces.len() < 2 {
        return Err(KernelError::Invalid("the body has one face: nothing to unstitch".into()));
    }
    let piece = |picked: Vec<mt::Face>| rebuilt_body(surface_body(picked), &[&body.solid], &Rebuild::default());
    let pieces: Vec<Body> = chosen.iter().filter_map(|i| faces.get(*i)).map(|f| piece(vec![(*f).clone()])).collect();
    let rest: Vec<mt::Face> = faces.iter().enumerate().filter(|(i, _)| !chosen.contains(i)).map(|(_, f)| (*f).clone()).collect();
    Ok((pieces, (!rest.is_empty()).then(|| piece(rest))))
}

/// A surface body with the picked faces (by points on them; every face when none are given)
/// turned over: their normals point the other way. A solid is refused (its faces face out).
/// The history is over `[body]`: each face is the body's, turned over or not.
pub fn reverse_faces(body: &Body, at: &[Vec3]) -> Result<Body> {
    body.require_brep("reverse normal")?;
    if !body.is_surface() {
        return Err(KernelError::Invalid("a solid's faces face out: unstitch it into surfaces first".into()));
    }
    let chosen = picked_faces(body, at)?;
    let mut rec = Rebuild::default();
    let faces: Vec<mt::Face> = body
        .solid
        .face_iter()
        .enumerate()
        .map(|(i, f)| {
            if !chosen.contains(&i) {
                return f.clone();
            }
            let turned = f.inverse();
            rec.whole(&turned, f);
            turned
        })
        .collect();
    Ok(rebuilt_body(surface_body(faces), &[&body.solid], &rec))
}

/// A body with some of its faces (indices in its face order) taken away and the gap left open:
/// a surface body of the faces left. Deleting every face is refused. The history is over
/// `[body]`: each face left is the body's.
pub fn remove_faces(body: &Body, faces: &[usize]) -> Result<Body> {
    body.require_brep("Delete Face")?;
    if faces.is_empty() {
        return Err(KernelError::Invalid("no faces given".into()));
    }
    let left: Vec<mt::Face> = body.solid.face_iter().enumerate().filter(|(i, _)| !faces.contains(i)).map(|(_, f)| f.clone()).collect();
    if left.is_empty() {
        return Err(KernelError::Invalid("that would delete every face of the body".into()));
    }
    Ok(rebuilt_body(surface_body(left), &[&body.solid], &Rebuild::default()))
}

/// Index of the face nearest a point (over its mesh triangles).
pub(crate) fn nearest_face(body: &Body, p: Vec3) -> Result<usize> {
    let m = body.tessellate(body.size() * 2e-3)?;
    let mut best: Option<(f64, u32)> = None;
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        let Some([a, b, c]) = m.tri(t) else { continue };
        let d = ((a + b + c) / 3.0).dist(p).min(a.dist(p)).min(b.dist(p)).min(c.dist(p));
        if best.is_none_or(|(x, _)| d < x) {
            best = Some((d, *f));
        }
    }
    best.map(|(_, f)| f as usize).ok_or_else(|| KernelError::Invalid("no face there".into()))
}

/// Sew surface bodies along edges that coincide (within `tol`): one body, a solid when the
/// faces close up. Faces are turned so that neighbours agree. The result's history is over
/// `bodies`: each face the one it was sewn from, each edge the edges sewn into it.
pub fn stitch(bodies: &[&Body], tol: f64) -> Result<Body> {
    if bodies.is_empty() {
        return Err(KernelError::Invalid("nothing to stitch".into()));
    }
    for b in bodies {
        b.require_brep("stitch")?;
    }
    let size = bodies.iter().map(|b| b.size()).fold(0.0, f64::max);
    let tol = if tol.is_finite() && tol > 0.0 { tol } else { size * 1e-6 + 1e-6 };
    guard("stitch", || {
        let faces: Vec<mt::Face> = bodies.iter().flat_map(|b| b.solid.face_iter().cloned().collect::<Vec<_>>()).collect();
        // Shared vertices by position, shared edges by their ends and middle.
        let mut verts: Vec<mt::Vertex> = Vec::new();
        let mut vmap: HashMap<mt::VertexID, mt::Vertex> = HashMap::new();
        let mut canon = |v: &mt::Vertex, verts: &mut Vec<mt::Vertex>| -> mt::Vertex {
            if let Some(c) = vmap.get(&v.id()) {
                return c.clone();
            }
            let p = v.point();
            let c = match verts.iter().find(|u| u.point().distance(p) <= tol) {
                Some(u) => u.clone(),
                None => {
                    verts.push(v.clone());
                    v.clone()
                }
            };
            vmap.insert(v.id(), c.clone());
            c
        };
        let mut edges: Vec<(mt::Edge, mt::Point3)> = Vec::new();
        let mut out = Vec::with_capacity(faces.len());
        let mut rec = Rebuild::default();
        for f in &faces {
            let mut wires = Vec::new();
            for w in f.absolute_boundaries() {
                let mut nw = Vec::new();
                for e in w.edge_iter() {
                    let abs = if e.orientation() { e.clone() } else { e.inverse() };
                    let (a, b) = (canon(abs.front(), &mut verts), canon(abs.back(), &mut verts));
                    let c = abs.curve();
                    let (t0, t1) = c.range_tuple();
                    let mid = c.subs(0.5 * (t0 + t1));
                    let found = edges.iter().find_map(|(x, m)| {
                        if m.distance(mid) > tol * 10.0 {
                            return None;
                        }
                        if x.front().id() == a.id() && x.back().id() == b.id() {
                            Some(x.clone())
                        } else if x.front().id() == b.id() && x.back().id() == a.id() {
                            Some(x.inverse())
                        } else {
                            None
                        }
                    });
                    let ne = match found {
                        Some(x) => x,
                        None => {
                            let x = mt::Edge::new_unchecked(&a, &b, c);
                            edges.push((x.clone(), mid));
                            x
                        }
                    };
                    rec.piece(&ne, &abs);
                    nw.push(if e.orientation() { ne } else { ne.inverse() });
                }
                wires.push(mt::Wire::from(nw));
            }
            let mut nf = mt::Face::new_unchecked(wires, f.surface());
            if !f.orientation() {
                nf.invert();
            }
            rec.whole(&nf, f);
            out.push(nf);
        }
        // Neighbours run their shared edges opposite ways.
        let mut uses: HashMap<mt::EdgeID, Vec<(usize, bool)>> = HashMap::new();
        for (i, f) in out.iter().enumerate() {
            for e in f.boundaries().iter().flat_map(|w| w.edge_iter()) {
                uses.entry(e.id()).or_default().push((i, e.orientation()));
            }
        }
        let mut flip: Vec<Option<bool>> = vec![None; out.len()];
        for s in 0..out.len() {
            if flip[s].is_some() {
                continue;
            }
            flip[s] = Some(false);
            let mut stack = vec![s];
            while let Some(i) = stack.pop() {
                let fl = flip[i].unwrap_or(false);
                let Some(f) = out.get(i) else { continue };
                for e in f.boundaries().iter().flat_map(|w| w.edge_iter()) {
                    let d = e.orientation() != fl;
                    for &(j, dj) in uses.get(&e.id()).into_iter().flatten() {
                        if j != i && flip[j].is_none() {
                            flip[j] = Some(dj == d);
                            stack.push(j);
                        }
                    }
                }
            }
        }
        let out: Vec<mt::Face> = out.into_iter().zip(flip).map(|(f, fl)| if fl == Some(true) { f.inverse() } else { f }).collect();
        let operands: Vec<&mt::Solid> = bodies.iter().map(|b| &*b.solid).collect();
        body_of(out, &operands, &rec)
    })
}

/// How a face moves along its normal: a plane slides, a circular cylinder changes radius.
#[derive(Clone, Copy)]
pub(crate) enum Offset {
    Plane { n: Vec3 },
    Cylinder { o: Vec3, axis: Vec3, r: f64, out: f64 },
}

impl Offset {
    pub(crate) fn of(f: &mt::Face) -> Option<Offset> {
        match crate::splitface::analytic_field(&f.oriented_surface())? {
            crate::splitface::Analytic::Plane { n } => Some(Offset::Plane { n }),
            crate::splitface::Analytic::Cylinder { o, axis, r, out } => Some(Offset::Cylinder { o, axis, r, out }),
        }
    }
    pub(crate) fn map(&self, p: Vec3, d: f64) -> Vec3 {
        match *self {
            Offset::Plane { n } => p + n * d,
            // Scaled about the axis (control points of a NURBS circle stay exact).
            Offset::Cylinder { o, axis, r, out } => {
                let v = p - o;
                let radial = v - axis * v.dot(axis);
                p + radial * (out * d / r)
            }
        }
    }
}

fn map_pt(off: &Offset, p: mt::Point3, d: f64) -> mt::Point3 {
    p3(off.map(from_p3(p), d))
}

/// A curve moved by an offset (control points mapped; exact for lines, and for circles square
/// to a cylinder's axis or in a plane).
pub(crate) fn map_curve(off: &Offset, c: &mt::Curve, d: f64) -> Result<mt::Curve> {
    if let Some(moved) = map_control_points(c, |p| off.map(p, d)) {
        return Ok(moved);
    }
    // Anything else: a cubic through moved samples.
    let (t0, t1) = c.range_tuple();
    let pts: Vec<Vec3> = (0..=32).map(|k| off.map(from_p3(c.subs(t0 + (t1 - t0) * k as f64 / 32.0)), d)).collect();
    Ok(mt::Curve::BSplineCurve(crate::build::interpolate_cubic(&pts).ok_or_else(|| KernelError::Failed("offset edge".into()))?))
}

/// A line, B-spline or NURBS curve with its control points mapped by `f` (the exact image of
/// the curve under an affine `f`); `None` for a curve without control points (an
/// intersection curve).
pub(crate) fn map_control_points(c: &mt::Curve, f: impl Fn(Vec3) -> Vec3) -> Option<mt::Curve> {
    let map = |p: mt::Point3| p3(f(from_p3(p)));
    Some(match c {
        mt::Curve::Line(l) => mt::Curve::Line(mt::Line(map(l.0), map(l.1))),
        mt::Curve::BSplineCurve(b) => {
            let pts: Vec<mt::Point3> = b.control_points().iter().map(|p| map(*p)).collect();
            mt::Curve::BSplineCurve(mt::BSplineCurve::new(b.knot_vec().clone(), pts))
        }
        mt::Curve::NurbsCurve(n) => {
            let nb = n.non_rationalized();
            let pts: Vec<mt::Vector4> = nb
                .control_points()
                .iter()
                .map(|h| {
                    let w = h.w;
                    let q = map(mt::Point3::new(h.x / w, h.y / w, h.z / w));
                    mt::Vector4::new(q.x * w, q.y * w, q.z * w, w)
                })
                .collect();
            mt::Curve::NurbsCurve(mt::NurbsCurve::new(mt::BSplineCurve::new(nb.knot_vec().clone(), pts)))
        }
        mt::Curve::IntersectionCurve(_) => return None,
    })
}

/// A surface moved by an offset (control points mapped; exact for planes and NURBS cylinders).
pub(crate) fn map_surface(off: &Offset, s: &mt::Surface, d: f64) -> Result<mt::Surface> {
    Ok(match s {
        mt::Surface::Plane(pl) => {
            let (o, a, b) = (pl.origin(), pl.u_axis(), pl.v_axis());
            let o2 = map_pt(off, o, d);
            mt::Surface::Plane(mt::Plane::new(o2, o2 + a, o2 + b))
        }
        mt::Surface::BSplineSurface(b) => {
            let rows: Vec<Vec<mt::Point3>> = b.control_points().iter().map(|r| r.iter().map(|p| map_pt(off, *p, d)).collect()).collect();
            mt::Surface::BSplineSurface(mt::BSplineSurface::new((b.uknot_vec().clone(), b.vknot_vec().clone()), rows))
        }
        mt::Surface::NurbsSurface(n) => {
            let nb = n.non_rationalized();
            let rows: Vec<Vec<mt::Vector4>> = nb
                .control_points()
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|h| {
                            let w = h.w;
                            let q = map_pt(off, mt::Point3::new(h.x / w, h.y / w, h.z / w), d);
                            mt::Vector4::new(q.x * w, q.y * w, q.z * w, w)
                        })
                        .collect()
                })
                .collect();
            mt::Surface::NurbsSurface(mt::NurbsSurface::new(mt::BSplineSurface::new((nb.uknot_vec().clone(), nb.vknot_vec().clone()), rows)))
        }
        _ => return Err(KernelError::Invalid("not supported yet: thickening this kind of surface".into())),
    })
}

/// How far Thicken's fitted faces may stray from the true offset (mm).
const THICKEN_TOLERANCE: f64 = 0.01;

/// A surface body thickened into a solid: offset along the faces' normals by `thickness`
/// (negative: against them), or by half each way when `symmetric`. Planes and circular
/// cylinders move exactly, other faces onto fitted surfaces (see `surface_offset`); a thickness
/// that would fold a face over is refused. Neighbouring faces must meet smoothly (their offsets
/// agree).
pub fn thicken(body: &Body, thickness: f64, symmetric: bool) -> Result<Body> {
    body.require_brep("thicken")?;
    if !(thickness.is_finite() && thickness.abs() > 1e-9) {
        return Err(KernelError::Invalid("thickness must be non-zero".into()));
    }
    let (d0, d1) = if symmetric { (-thickness / 2.0, thickness / 2.0) } else { (0.0, thickness) };
    let (lo, hi) = (d0.min(d1), d0.max(d1));
    let size = body.size();
    let tol = size * 1e-7 + 1e-9;
    guard("thicken", || {
        let faces: Vec<mt::Face> = body.solid.face_iter().cloned().collect();
        if faces.is_empty() {
            return Err(KernelError::Invalid("nothing to thicken".into()));
        }
        // Planes and circular cylinders move exactly; other faces onto surfaces fitted within the
        // offset tolerance (Offset Surface's).
        let offs: Vec<crate::surface_offset::FaceMover> =
            faces.iter().map(|f| crate::surface_offset::FaceMover::new(f, &[lo, hi], THICKEN_TOLERANCE)).collect::<Result<_>>()?;
        let near = |a: Vec3, b: Vec3| a.dist(b) <= tol * 1e3;
        // Each side's copies of every vertex and edge. Faces that move one to the same place share
        // the copy (they meet smoothly there); across a sharp edge each face has its own.
        let mut vcopies: [HashMap<mt::VertexID, Vec<mt::Vertex>>; 2] = [HashMap::new(), HashMap::new()];
        let mut ecopies: [HashMap<mt::EdgeID, Vec<mt::Edge>>; 2] = [HashMap::new(), HashMap::new()];
        // Each face's copies, by side: (face, vertex) → vertex, (face, edge) → edge.
        let mut vof: [HashMap<(usize, mt::VertexID), mt::Vertex>; 2] = [HashMap::new(), HashMap::new()];
        let mut eof: [HashMap<(usize, mt::EdgeID), mt::Edge>; 2] = [HashMap::new(), HashMap::new()];
        // The faces using each edge, in order.
        let mut users: HashMap<mt::EdgeID, Vec<usize>> = HashMap::new();
        let mut abs_edges: HashMap<mt::EdgeID, mt::Edge> = HashMap::new();
        for (fi, (f, off)) in faces.iter().zip(&offs).enumerate() {
            for e in f.absolute_boundaries().iter().flat_map(|w| w.edge_iter()) {
                let abs = if e.orientation() { e.clone() } else { e.inverse() };
                let list = users.entry(abs.id()).or_default();
                if !list.contains(&fi) {
                    list.push(fi);
                }
                abs_edges.entry(abs.id()).or_insert_with(|| abs.clone());
                for (side, d) in [lo, hi].into_iter().enumerate() {
                    for v in [abs.front(), abs.back()] {
                        if vof[side].contains_key(&(fi, v.id())) {
                            continue;
                        }
                        let at = off.point(from_p3(v.point()), d)?;
                        let list = vcopies[side].entry(v.id()).or_default();
                        let copy = match list.iter().find(|c| near(from_p3(c.point()), at)) {
                            Some(c) => c.clone(),
                            None => {
                                let c = mt::Vertex::new(p3(at));
                                list.push(c.clone());
                                c
                            }
                        };
                        vof[side].insert((fi, v.id()), copy);
                    }
                    let (Some(a), Some(b)) = (vof[side].get(&(fi, abs.front().id())).cloned(), vof[side].get(&(fi, abs.back().id())).cloned()) else {
                        continue;
                    };
                    let c = abs.curve();
                    let (t0, t1) = c.range_tuple();
                    let mid = off.point(from_p3(c.subs(0.5 * (t0 + t1))), d)?;
                    let list = ecopies[side].entry(abs.id()).or_default();
                    let shared = list.iter().find(|m| {
                        m.front().id() == a.id() && m.back().id() == b.id() && {
                            let mc = m.curve();
                            let (s0, s1) = mc.range_tuple();
                            near(from_p3(mc.subs(0.5 * (s0 + s1))), mid)
                        }
                    });
                    let copy = match shared {
                        Some(m) => m.clone(),
                        None => {
                            let m = mt::Edge::new_unchecked(&a, &b, off.curve(&c, d)?);
                            list.push(m.clone());
                            m
                        }
                    };
                    eof[side].insert((fi, abs.id()), copy);
                }
            }
        }
        // Lines joining a vertex's copy on one side to its copy on the other, once each.
        let mut risers: HashMap<(mt::VertexID, mt::VertexID), mt::Edge> = HashMap::new();
        let mut riser = |a: &mt::Vertex, b: &mt::Vertex| -> mt::Edge {
            risers.entry((a.id(), b.id())).or_insert_with(|| mt::Edge::new_unchecked(a, b, mt::Curve::Line(mt::Line(a.point(), b.point())))).clone()
        };
        let moved_face = |fi: usize, f: &mt::Face, off: &crate::surface_offset::FaceMover, side: usize, d: f64| -> Result<mt::Face> {
            let wires: Vec<mt::Wire> = f
                .absolute_boundaries()
                .iter()
                .map(|w| {
                    w.edge_iter()
                        .filter_map(|e| {
                            let abs_id = if e.orientation() { e.id() } else { e.inverse().id() };
                            let m = eof[side].get(&(fi, abs_id))?.clone();
                            Some(if e.orientation() { m } else { m.inverse() })
                        })
                        .collect::<Vec<_>>()
                        .into()
                })
                .collect();
            let mut nf = mt::Face::new_unchecked(wires, off.surface(d)?);
            if !f.orientation() {
                nf.invert();
            }
            Ok(nf)
        };
        let mut out = Vec::new();
        // History (one operand, the surface): a copy at no distance is the face itself, a copy
        // further out its offset, a wall is made along the edge it stands on, a corner patch from
        // the sharp edges that meet at its corner.
        let operand = crate::record::Operand::new(body);
        let edge_index = &operand.edges;
        let mut history = Vec::new();
        let copy = |i: usize, d: f64| {
            if d == 0.0 {
                crate::record::from_face(0, i, Change::Kept)
            } else {
                crate::record::generated(vec![FaceOrigin::Offset { operand: 0, face: i }])
            }
        };
        let edge_origin = |id: &mt::EdgeID| -> Result<FaceOrigin> {
            let edge = edge_index.get(id).copied().ok_or_else(|| KernelError::Failed("thicken: an edge outside the surface".into()))?;
            Ok(FaceOrigin::Edge { operand: 0, edge })
        };
        for (i, (f, off)) in faces.iter().zip(&offs).enumerate() {
            // The lower copy faces back, the upper one forward.
            out.push(moved_face(i, f, off, 0, lo)?.inverse());
            out.push(moved_face(i, f, off, 1, hi)?);
            history.extend([copy(i, lo), copy(i, hi)]);
            // Walls along free edges, outward across the edge.
            for w in f.boundaries() {
                for e in w.edge_iter() {
                    let abs_id = if e.orientation() { e.id() } else { e.inverse().id() };
                    if users.get(&abs_id).map_or(0, Vec::len) != 1 {
                        continue;
                    }
                    let (Some(le), Some(he)) = (eof[0].get(&(i, abs_id)), eof[1].get(&(i, abs_id))) else { continue };
                    let (le, he) = if e.orientation() { (le.clone(), he.clone()) } else { (le.inverse(), he.inverse()) };
                    let (sa, sb) = (riser(le.front(), he.front()), riser(le.back(), he.back()));
                    // A → B low, B → B' up, B' → A' high back, A' → A down.
                    let wire: mt::Wire = vec![le.clone(), sb, he.inverse(), sa.inverse()].into();
                    let ruled = mt::builder::homotopy(&le, &he);
                    let surf = ruled.surface();
                    // The wall's outward normal: across the edge, away from the face.
                    let c = le.oriented_curve();
                    let (t0, t1) = c.range_tuple();
                    let tm = 0.5 * (t0 + t1);
                    let p = c.subs(tm);
                    let tangent = c.der(tm);
                    let n =
                        f.oriented_surface().search_nearest_parameter(p, None, 50).map(|(u, v)| f.oriented_surface().normal(u, v)).unwrap_or(tangent);
                    let outward = tangent.cross(n);
                    let sn = surf.search_nearest_parameter(p, None, 50).map(|(u, v)| surf.normal(u, v)).unwrap_or(outward);
                    let wall = if sn.dot(outward) >= 0.0 {
                        mt::Face::new_unchecked(vec![wire], surf)
                    } else {
                        let mut x = mt::Face::new_unchecked(vec![wire.inverse()], surf);
                        x.invert();
                        x
                    };
                    out.push(wall);
                    history.push(crate::record::generated(vec![edge_origin(&abs_id)?]));
                }
            }
        }
        // Sharp edges: where the two faces' copies of an edge part on a side, a wall closes the
        // gap; where they would cross (an inside corner on that side), refused.
        let mut sharp_at: HashMap<mt::VertexID, Vec<mt::EdgeID>> = HashMap::new();
        let mut gaps = Vec::new();
        for (id, fs) in &users {
            let [fa, fb] = fs.as_slice() else { continue };
            let Some(abs) = abs_edges.get(id) else { continue };
            for (side, d) in [lo, hi].into_iter().enumerate() {
                let (Some(ea), Some(eb)) = (eof[side].get(&(*fa, *id)), eof[side].get(&(*fb, *id))) else { continue };
                if ea.id() == eb.id() {
                    continue;
                }
                // Which way the faces bend across the edge: each lies behind the other's normal
                // (convex toward the normals) or in front of it.
                let c = abs.curve();
                let (t0, t1) = c.range_tuple();
                let p = from_p3(c.subs(0.5 * (t0 + t1)));
                let normal = |fi: usize| -> Option<Vec3> {
                    let s = faces.get(fi)?.oriented_surface();
                    let (u, v) = s.search_nearest_parameter(p3(p), None, 50)?;
                    Some(from_p3(mt::Point3::new(0.0, 0.0, 0.0) + s.normal(u, v)))
                };
                let centre = |fi: usize| -> Option<Vec3> {
                    let pts: Vec<Vec3> =
                        faces.get(fi)?.absolute_boundaries().iter().flat_map(|w| w.vertex_iter().map(|v| from_p3(v.point()))).collect();
                    Some(pts.iter().fold(Vec3::ZERO, |a, q| a + *q) * (1.0 / pts.len().max(1) as f64))
                };
                let (Some(na), Some(ca), Some(cb)) = (normal(*fa), centre(*fa), centre(*fb)) else {
                    return Err(KernelError::Failed("thicken: the faces at a sharp edge".into()));
                };
                // Convex toward the normals: the other face falls away behind this one's normal.
                let convex = na.dot(cb - p) < 0.0 && normal(*fb).is_some_and(|nb| nb.dot(ca - p) < 0.0);
                if convex != (d > 0.0) {
                    return Err(KernelError::Invalid(
                        "not supported yet: thickening into an inside corner across a sharp edge (try the other side, or symmetric off)".into(),
                    ));
                }
                gaps.push((*id, side, ea.clone(), eb.clone()));
                for v in [abs.front(), abs.back()] {
                    let list = sharp_at.entry(v.id()).or_default();
                    if !list.contains(id) {
                        list.push(*id);
                    }
                }
            }
        }
        // Each gap wall runs along one face's copy of the edge and back along the other's, joined
        // at the ends by lines between the two copies of each end vertex. It is turned so that it
        // crosses the first face's copy the other way from that face.
        let mut links: HashMap<(mt::VertexID, mt::VertexID), mt::Edge> = HashMap::new();
        let mut link = |a: &mt::Vertex, b: &mt::Vertex| -> mt::Edge {
            if let Some(e) = links.get(&(b.id(), a.id())) {
                return e.inverse();
            }
            links.entry((a.id(), b.id())).or_insert_with(|| mt::Edge::new_unchecked(a, b, mt::Curve::Line(mt::Line(a.point(), b.point())))).clone()
        };
        let used_forward = |faces: &[mt::Face], e: &mt::Edge| -> Option<bool> {
            faces
                .iter()
                .flat_map(|f| f.boundaries().into_iter().flat_map(|w| w.edge_iter().cloned().collect::<Vec<_>>()))
                .find(|x| x.id() == e.id())
                .map(|x| x.front().id() == e.front().id())
        };
        for (id, _side, ea, eb) in gaps {
            let wire: mt::Wire = vec![ea.clone(), link(ea.back(), eb.back()), eb.inverse(), link(eb.front(), ea.front())].into();
            let surf = mt::builder::homotopy(&ea, &eb).surface();
            let wall = mt::Face::new_unchecked(vec![wire], surf);
            let wall = if used_forward(&out, &ea).unwrap_or(false) { wall.inverse() } else { wall };
            out.push(wall);
            history.push(crate::record::generated(vec![edge_origin(&id)?]));
        }
        // Corners: whatever is left open is closed by plane patches (where three faces meet at a
        // sharp corner, a triangle), each from the sharp edges meeting there.
        let open = mt::Shell::from(out.clone()).extract_boundaries();
        for w in open {
            let patch = mt::builder::try_attach_plane(&[w.inverse()])
                .map_err(|_| KernelError::Invalid("not supported yet: thickening a corner where the gap does not close flat".into()))?;
            let mut origins = Vec::new();
            for v in w.vertex_iter() {
                for (vid, list) in &sharp_at {
                    let at = vcopies.iter().flat_map(|m| m.get(vid).into_iter().flatten()).any(|c| c.id() == v.id());
                    if at {
                        for e in list {
                            let o = edge_origin(e)?;
                            if !origins.contains(&o) {
                                origins.push(o);
                            }
                        }
                    }
                }
            }
            if origins.is_empty() {
                return Err(KernelError::Failed("the thickened surface does not close".into()));
            }
            out.push(patch);
            history.push(crate::record::generated(origins));
        }
        let shell = mt::Shell::from(out);
        if shell.shell_condition() != mt::ShellCondition::Closed {
            return Err(KernelError::Failed("the thickened surface does not close".into()));
        }
        // The edges copied at no distance are the surface's own edges, rebuilt.
        let mut copies = HashMap::new();
        for (side, d) in [lo, hi].into_iter().enumerate() {
            if d != 0.0 {
                continue;
            }
            for (id, list) in &ecopies[side] {
                if let Some(&edge) = edge_index.get(id) {
                    for c in list {
                        copies.insert(c.id(), EdgeOrigin::Edge { operand: 0, edge });
                    }
                }
            }
        }
        crate::record::attach(Body::new(mt::Solid::new_unchecked(vec![shell]))?, &[operand], Some(history), copies)
    })
}

/// A surface body trimmed by a tool: its faces are cut where the tool meets them and the
/// pieces on the side of `keep` are kept. The result's history is over `[body]`.
pub fn trim(body: &Body, tool: &crate::SplitTool, keep: Vec3) -> Result<Body> {
    body.require_brep("trim")?;
    let total = body.solid.face_iter().count();
    let all: Vec<usize> = (0..total).collect();
    let (split, _) = crate::splitface::split_any(body, &all, tool)?;
    let side = crate::splitface::side_of_point(tool, keep)?;
    guard("trim", || {
        let mut kept = Vec::new();
        for f in split.solid.face_iter() {
            let x = crate::splitface::face_side(f, tool, split.size())?;
            if (x >= 0.0) == (side >= 0.0) {
                kept.push(f.clone());
            }
        }
        if kept.is_empty() {
            return Err(KernelError::Invalid("nothing is left on that side of the tool".into()));
        }
        // The kept pieces are the split's own; its history carries them back to `body`.
        let trimmed = rebuilt_body(surface_body(kept), &[&split.solid], &Rebuild::default());
        Ok(crate::record::after(trimmed, &split, &crate::record::counts(&[body])))
    })
}

/// A planar surface extended: the chosen straight edges (picked by points on them) move
/// outward in the face's plane by `distance`, their neighbours lengthened to meet them. The
/// result's history is over `[body]`: the face and each edge the one it was moved from.
pub fn extend(body: &Body, at: &[Vec3], distance: f64) -> Result<Body> {
    body.require_brep("extend")?;
    if !(distance.is_finite() && distance > 0.0) {
        return Err(KernelError::Invalid("the extension distance must be positive".into()));
    }
    let faces: Vec<mt::Face> = body.solid.face_iter().cloned().collect();
    let [face] = faces.as_slice() else { return Err(KernelError::Invalid("not supported yet: extending a surface of more than one face".into())) };
    let Some(crate::splitface::Analytic::Plane { n }) = crate::splitface::analytic_field(&face.oriented_surface()) else {
        return Err(KernelError::Invalid("not supported yet: extending a surface that is not planar".into()));
    };
    let chosen: Vec<mt::EdgeID> = at
        .iter()
        .map(|p| edge_near(body, *p).map(|e| e.id()).ok_or_else(|| KernelError::Invalid(format!("no edge at {p:?}"))))
        .collect::<Result<_>>()?;
    guard("extend", || {
        let bounds = face.boundaries();
        let [outer] = bounds.as_slice() else { return Err(KernelError::Invalid("not supported yet: extending a surface with holes".into())) };
        let edges: Vec<mt::Edge> = outer.edge_iter().cloned().collect();
        let m = edges.len();
        // Each edge as a line (point, direction) moved outward when chosen.
        let mut lines = Vec::with_capacity(m);
        for e in &edges {
            let (a, b) = (from_p3(e.front().point()), from_p3(e.back().point()));
            if !matches!(e.curve(), mt::Curve::Line(_)) {
                return Err(KernelError::Invalid("not supported yet: extending a surface with curved edges".into()));
            }
            let d = (b - a).normalized().ok_or_else(|| KernelError::Invalid("a zero-length edge".into()))?;
            // Outward: right of the edge seen along the normal (the face is on its left).
            let out = d.cross(n);
            let shift = if chosen.contains(&e.id()) || chosen.contains(&e.inverse().id()) { distance } else { 0.0 };
            lines.push((a + out * shift, d));
        }
        // Corners: where consecutive lines meet.
        let meet = |(p, d): (Vec3, Vec3), (q, e): (Vec3, Vec3)| -> Option<Vec3> {
            let w = q - p;
            let c = d.cross(e);
            let cc = c.dot(c);
            if cc < 1e-24 {
                return None;
            }
            Some(p + d * (w.cross(e).dot(c) / cc))
        };
        let mut corners = Vec::with_capacity(m);
        for i in 0..m {
            let (Some(prev), Some(cur)) = (lines.get((i + m - 1) % m), lines.get(i)) else { continue };
            let p = meet(*prev, *cur).unwrap_or(cur.0);
            corners.push(mt::Vertex::new(p3(p)));
        }
        let mut rec = Rebuild::default();
        let wire: mt::Wire = (0..m)
            .filter_map(|i| {
                let (a, b, old) = (corners.get(i)?, corners.get((i + 1) % m)?, edges.get(i)?);
                let e = mt::Edge::new_unchecked(a, b, mt::Curve::Line(mt::Line(a.point(), b.point())));
                rec.piece(&e, old);
                Some(e)
            })
            .collect::<Vec<_>>()
            .into();
        let mut f = mt::builder::try_attach_plane(&[wire]).map_err(|e| KernelError::Failed(format!("extend: {e}")))?;
        if f.oriented_surface().normal(0.0, 0.0).dot(crate::body::v3(n)) < 0.0 {
            f = f.inverse();
        }
        rec.face(&f, face);
        Ok(rebuilt_body(surface_body(vec![f]), &[&body.solid], &rec))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{box_solid, cylinder, measure};
    use solidcraft_geom::{Loop2, Vec2};

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs().max(1e-9)
    }

    /// Walls swept from an open line, an open arc and a closed square: areas are length × height,
    /// no caps, and stitching two patches onto the square's walls closes the box.
    #[test]
    fn extrude_sheet_of_open_and_closed_chains() {
        let line = vec![Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }];
        let s = extrude_sheet(&Plane::XY, std::slice::from_ref(&line), 0.0, 5.0).unwrap();
        assert!(s.is_surface() && s.face_count() == 1);
        assert!(rel(measure(&s).unwrap().area, 50.0) < 1e-9);
        // A quarter arc of radius 4, swept from -1 to 2: area r θ h.
        let arc = vec![Seg2::Arc { center: Vec2::new(0.0, 0.0), radius: 4.0, start: 0.0, sweep: std::f64::consts::FRAC_PI_2 }];
        let a = measure(&extrude_sheet(&Plane::XY, &[arc], 2.0, -1.0).unwrap()).unwrap().area;
        let want = 4.0 * std::f64::consts::FRAC_PI_2 * 3.0;
        assert!(rel(a, want) < 1e-3, "{a} vs {want}"); // (measured on meshes)
        // A line then a line back up: one chain, two faces.
        let bent = vec![line[0], Seg2::Line { a: Vec2::new(10.0, 0.0), b: Vec2::new(10.0, 6.0) }];
        assert_eq!(extrude_sheet(&Plane::XY, &[bent], 0.0, 1.0).unwrap().face_count(), 2);
        // A closed square: four walls, open at both ends; with a patch on each end, a solid box.
        let sq = Region2 {
            outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)]),
            holes: vec![],
        };
        let walls = extrude_sheet(&Plane::XY, std::slice::from_ref(&sq.outer.segs), 0.0, 4.0).unwrap();
        assert!(walls.is_surface() && walls.face_count() == 4);
        assert!(rel(measure(&walls).unwrap().area, 160.0) < 1e-9);
        let (bottom, top) = (patch_region(&Plane::XY, &sq).unwrap(), patch_region(&Plane::XY.offset(4.0), &sq).unwrap());
        let closed = stitch(&[&walls, &bottom, &top], 1e-6).unwrap();
        assert!(!closed.is_surface());
        assert!(rel(measure(&closed).unwrap().volume, 400.0) < 1e-9);
    }

    /// Sheets revolved about the sketch's y axis, against their analytic areas: a cylinder (and a
    /// quarter of one, started a quarter turn round), cones with the apex at either end, a sphere
    /// from a half circle, and a square's ring (Pappus: 2π × centroid distance × length).
    #[test]
    fn revolve_sheet_areas() {
        use std::f64::consts::{FRAC_PI_2, PI};
        let sheet = |chain: Vec<Seg2>, start: f64, sweep: f64| {
            revolve_sheet(&Plane::XY, &[chain], Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), start, sweep).unwrap()
        };
        let rev = |chain: Vec<Seg2>, start: f64, sweep: f64| measure(&sheet(chain, start, sweep)).unwrap().area;
        let line = |a: (f64, f64), b: (f64, f64)| Seg2::Line { a: Vec2::new(a.0, a.1), b: Vec2::new(b.0, b.1) };
        let close = |a: f64, want: f64| assert!(rel(a, want) < 2e-3, "{a} vs {want}"); // (measured on meshes)
        close(rev(vec![line((3.0, 0.0), (3.0, 4.0))], 0.0, 2.0 * PI), 2.0 * PI * 3.0 * 4.0);
        close(rev(vec![line((3.0, 0.0), (3.0, 4.0))], FRAC_PI_2, FRAC_PI_2), FRAC_PI_2 * 3.0 * 4.0);
        close(rev(vec![line((0.0, 0.0), (3.0, 4.0))], 0.0, 2.0 * PI), PI * 3.0 * 5.0);
        close(rev(vec![line((3.0, 4.0), (0.0, 0.0))], 0.0, 2.0 * PI), PI * 3.0 * 5.0);
        assert!(sheet(vec![line((3.0, 0.0), (3.0, 4.0))], 0.0, 1.0).is_surface());
        // Closing shells are solids, outward: a sphere from a half circle, a square's ring.
        let half = Seg2::Arc { center: Vec2::new(0.0, 0.0), radius: 2.0, start: -FRAC_PI_2, sweep: PI };
        let ball = measure(&sheet(vec![half], 0.0, 2.0 * PI)).unwrap();
        close(ball.area, 4.0 * PI * 4.0);
        close(ball.volume, 4.0 / 3.0 * PI * 8.0);
        assert!(sheet(vec![half], 0.0, PI).is_surface());
        let sq = Loop2::polygon(&[Vec2::new(2.0, 0.0), Vec2::new(3.0, 0.0), Vec2::new(3.0, 1.0), Vec2::new(2.0, 1.0)]);
        let ring = measure(&sheet(sq.segs.clone(), 0.0, 2.0 * PI)).unwrap();
        close(ring.area, 2.0 * PI * (2.0 + 3.0 + 2.5 + 2.5));
        close(ring.volume, 2.0 * PI * 2.5);
    }

    #[test]
    fn revolve_sheet_refuses_bad_input() {
        let rev = |chain: Vec<Seg2>, sweep: f64| revolve_sheet(&Plane::XY, &[chain], Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), 0.0, sweep);
        let line = |a: (f64, f64), b: (f64, f64)| Seg2::Line { a: Vec2::new(a.0, a.1), b: Vec2::new(b.0, b.1) };
        assert!(rev(vec![line((-1.0, 0.0), (1.0, 1.0))], 1.0).is_err(), "crosses the axis");
        assert!(rev(vec![line((0.0, 0.0), (0.0, 1.0))], 1.0).is_err(), "on the axis");
        assert!(rev(vec![line((1.0, 0.0), (1.0, 1.0))], 0.0).is_err(), "zero angle");
        assert!(rev(vec![line((1.0, 0.0), (1.0, 1.0))], 7.0).is_err(), "more than a turn");
        assert!(rev(vec![line((1.0, 0.0), (1.0, 1.0))], f64::NAN).is_err());
        let touching = Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)]);
        assert!(rev(touching.segs, 1.0).is_err(), "a closed curve touching the axis");
        assert!(revolve_sheet(&Plane::XY, &[], Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), 0.0, 1.0).is_err());
        assert!(revolve_sheet(&Plane::XY, &[vec![line((1.0, 0.0), (1.0, 1.0))]], Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0), 0.0, 1.0).is_err());
    }

    /// A line along x swept up z: straight (length × height), then on round a quarter turn
    /// (a quarter cylinder of radius 5 about an axis along x: 10 × 5π/2, the arc taking up the
    /// line's far edge), and along a path with a corner (mitred ruled walls, 10 × both legs); a
    /// closed square's four walls; a 3D point path.
    #[test]
    fn sweep_sheet_areas() {
        use crate::PathSeg;
        use std::f64::consts::FRAC_PI_2;
        let line = vec![Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }];
        let up = PathSeg::Line { a: Vec3::ZERO, b: Vec3::new(0.0, 0.0, 5.0) };
        let area = |b: &Body| measure(b).unwrap().area;
        let straight = sweep_sheet(&Plane::XY, std::slice::from_ref(&line), &[up]).unwrap();
        assert!(straight.is_surface() && rel(area(&straight), 50.0) < 1e-9);
        let bend = PathSeg::Arc { a: Vec3::new(0.0, 0.0, 5.0), center: Vec3::new(0.0, 5.0, 5.0), axis: -Vec3::X, angle: FRAC_PI_2 };
        let smooth = sweep_sheet(&Plane::XY, std::slice::from_ref(&line), &[up, bend]).unwrap();
        assert!(smooth.is_surface());
        let want = 50.0 + 10.0 * 5.0 * FRAC_PI_2;
        assert!(rel(area(&smooth), want) < 2e-3, "{} vs {want}", area(&smooth)); // (measured on meshes)
        let over = PathSeg::Line { a: Vec3::new(0.0, 0.0, 5.0), b: Vec3::new(0.0, 5.0, 5.0) };
        let cornered = sweep_sheet(&Plane::XY, std::slice::from_ref(&line), &[up, over]).unwrap();
        assert!(cornered.is_surface() && rel(area(&cornered), 100.0) < 1e-9, "{}", area(&cornered));
        let sq = Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 4.0), Vec2::new(0.0, 4.0)]);
        let walls = sweep_sheet(&Plane::XY, std::slice::from_ref(&sq.segs), &[up]).unwrap();
        assert!(walls.is_surface() && walls.face_count() == 4 && rel(area(&walls), 80.0) < 1e-9);
        let pts: Vec<Vec3> = (0..=10).map(|i| Vec3::new(0.0, 0.0, i as f64)).collect();
        assert!(rel(area(&sweep_sheet_points(&Plane::XY, std::slice::from_ref(&line), &pts).unwrap()), 100.0) < 1e-6);
        // A smooth point path: a quarter circle of radius 30 in the YZ plane. One smooth wall (not
        // forty ruled strips), a piece of cylinder: 10 × 15π.
        let arc: Vec<Vec3> = (0..=40)
            .map(|i| {
                let t = i as f64 / 40.0 * FRAC_PI_2;
                Vec3::new(0.0, 30.0 * (1.0 - t.cos()), 30.0 * t.sin())
            })
            .collect();
        let curved = sweep_sheet_points(&Plane::XY, std::slice::from_ref(&line), &arc).unwrap();
        assert_eq!(curved.face_count(), 1, "smooth wall");
        let want = 10.0 * 30.0 * FRAC_PI_2;
        assert!(rel(area(&curved), want) < 2e-3, "{} vs {want}", area(&curved)); // (measured on meshes)
        // A bent chain along the same path: two smooth walls joined by one shared seam edge.
        let bent = vec![line[0], Seg2::Line { a: Vec2::new(10.0, 0.0), b: Vec2::new(10.0, 5.0) }];
        let two = sweep_sheet_points(&Plane::XY, &[bent], &arc).unwrap();
        assert_eq!(two.face_count(), 2);
        let edges: std::collections::HashSet<_> = two.solid.edge_iter().map(|e| e.id()).collect();
        assert_eq!(edges.len(), 7, "the walls share their seam");
    }

    #[test]
    fn sweep_sheet_refuses_bad_input() {
        use crate::PathSeg;
        let line = vec![Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }];
        let up = PathSeg::Line { a: Vec3::ZERO, b: Vec3::new(0.0, 0.0, 5.0) };
        assert!(sweep_sheet(&Plane::XY, std::slice::from_ref(&line), &[]).is_err());
        assert!(sweep_sheet(&Plane::XY, &[], &[up]).is_err());
        assert!(sweep_sheet(&Plane::XY, std::slice::from_ref(&line), &[PathSeg::Line { a: Vec3::ZERO, b: Vec3::ZERO }]).is_err());
        let no_axis = PathSeg::Arc { a: Vec3::ZERO, center: Vec3::new(0.0, 5.0, 0.0), axis: Vec3::ZERO, angle: 1.0 };
        assert!(sweep_sheet(&Plane::XY, std::slice::from_ref(&line), &[no_axis]).is_err());
        assert!(sweep_sheet_points(&Plane::XY, std::slice::from_ref(&line), &[Vec3::ZERO]).is_err());
        assert!(sweep_sheet_points(&Plane::XY, std::slice::from_ref(&line), &[Vec3::ZERO, Vec3::new(f64::NAN, 0.0, 0.0)]).is_err());
    }

    /// Sheets lofted between open lines and closed loops: a ruled strip (even when the second
    /// line runs the other way, or is in two pieces), a smooth strip through three, the walls
    /// between two squares, and a cylinder through three circles.
    #[test]
    fn loft_sheet_areas() {
        use std::f64::consts::PI;
        let at = |z: f64| Plane::XY.offset(z);
        let line = |a: f64, b: f64| Seg2::Line { a: Vec2::new(a, 0.0), b: Vec2::new(b, 0.0) };
        let area = |b: &Body| measure(b).unwrap().area;
        let strip = loft_sheet(&[(at(0.0), vec![line(0.0, 10.0)]), (at(5.0), vec![line(0.0, 10.0)])]).unwrap();
        assert!(strip.is_surface() && rel(area(&strip), 50.0) < 1e-6, "{}", area(&strip));
        let turned = loft_sheet(&[(at(0.0), vec![line(0.0, 10.0)]), (at(5.0), vec![line(10.0, 0.0)])]).unwrap();
        assert!(rel(area(&turned), 50.0) < 1e-6, "not twisted: {}", area(&turned));
        let pieces = loft_sheet(&[(at(0.0), vec![line(0.0, 10.0)]), (at(5.0), vec![line(0.0, 5.0), line(5.0, 10.0)])]).unwrap();
        assert!(rel(area(&pieces), 50.0) < 1e-6 && pieces.face_count() == 2, "{}", area(&pieces));
        let three = loft_sheet(&[(at(0.0), vec![line(0.0, 10.0)]), (at(5.0), vec![line(0.0, 10.0)]), (at(10.0), vec![line(0.0, 10.0)])]).unwrap();
        assert!(rel(area(&three), 100.0) < 1e-3, "{}", area(&three)); // (measured on meshes)
        let sq = Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)]);
        let walls = loft_sheet(&[(at(0.0), sq.segs.clone()), (at(4.0), sq.segs.clone())]).unwrap();
        assert!(walls.is_surface() && walls.face_count() == 4 && rel(area(&walls), 160.0) < 1e-6, "{}", area(&walls));
        let circle = Loop2::circle(Vec2::ZERO, 2.0).segs;
        let tube = loft_sheet(&[(at(0.0), circle.clone()), (at(5.0), circle.clone()), (at(10.0), circle)]).unwrap();
        assert!(tube.is_surface());
        let want = 2.0 * PI * 2.0 * 10.0;
        assert!(rel(area(&tube), want) < 2e-3, "{} vs {want}", area(&tube)); // (measured on meshes)
    }

    #[test]
    fn loft_sheet_refuses_bad_input() {
        let at = |z: f64| Plane::XY.offset(z);
        let line = vec![Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }];
        let sq = Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)]).segs;
        assert!(loft_sheet(&[(at(0.0), line.clone())]).is_err(), "one section");
        assert!(loft_sheet(&[(at(0.0), line.clone()), (at(1.0), sq)]).is_err(), "open and closed");
        assert!(loft_sheet(&[(at(0.0), line.clone()), (at(1.0), vec![])]).is_err(), "an empty section");
        let gap = vec![line[0], Seg2::Line { a: Vec2::new(11.0, 0.0), b: Vec2::new(12.0, 0.0) }];
        assert!(loft_sheet(&[(at(0.0), line.clone()), (at(1.0), gap)]).is_err(), "a section with a gap");
        let nan = vec![Seg2::Line { a: Vec2::new(f64::NAN, 0.0), b: Vec2::new(1.0, 0.0) }];
        assert!(loft_sheet(&[(at(0.0), line), (at(1.0), nan)]).is_err());
    }

    #[test]
    fn extrude_sheet_refuses_bad_input() {
        let line = vec![Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }];
        assert!(extrude_sheet(&Plane::XY, std::slice::from_ref(&line), 0.0, 0.0).is_err());
        assert!(extrude_sheet(&Plane::XY, std::slice::from_ref(&line), 0.0, f64::NAN).is_err());
        assert!(extrude_sheet(&Plane::XY, &[], 0.0, 1.0).is_err());
        assert!(extrude_sheet(&Plane::XY, &[vec![]], 0.0, 1.0).is_err());
        let gap = vec![line[0], Seg2::Line { a: Vec2::new(11.0, 0.0), b: Vec2::new(12.0, 0.0) }];
        assert!(extrude_sheet(&Plane::XY, &[gap], 0.0, 1.0).is_err());
        let nan = vec![Seg2::Line { a: Vec2::new(0.0, f64::NAN), b: Vec2::new(1.0, 0.0) }];
        assert!(extrude_sheet(&Plane::XY, &[nan], 0.0, 1.0).is_err());
    }

    /// A box taken apart into its six faces (each a 10 × 10 surface) and stitched back into
    /// the same solid; two picked faces come off on their own, the other four stay together;
    /// a one-face body has nothing to take apart.
    #[test]
    fn unstitch_a_box_and_stitch_it_back() {
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let (pieces, rest) = unstitch(&b, &[]).unwrap();
        assert!(rest.is_none() && pieces.len() == 6);
        for p in &pieces {
            assert!(p.is_surface() && p.face_count() == 1 && rel(measure(p).unwrap().area, 100.0) < 1e-9);
        }
        let back = stitch(&pieces.iter().collect::<Vec<_>>(), 1e-6).unwrap();
        assert!(!back.is_surface() && rel(measure(&back).unwrap().volume, 1000.0) < 1e-9);
        let (two, rest) = unstitch(&b, &[Vec3::new(5.0, 5.0, 10.0), Vec3::new(5.0, 5.0, 0.0)]).unwrap();
        let rest = rest.unwrap();
        assert!(two.len() == 2 && rest.is_surface() && rest.face_count() == 4);
        assert!(unstitch(&two[0], &[]).is_err());
    }

    /// A patch thickened along its normal grows up; turned over, the same thickness grows down.
    /// A solid's faces are not turned over.
    #[test]
    fn reverse_faces_turns_the_thicken_side() {
        let r = Region2 {
            outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)]),
            holes: vec![],
        };
        let s = patch_region(&Plane::XY, &r).unwrap();
        let up = measure(&thicken(&s, 2.0, false).unwrap()).unwrap().bbox;
        let turned = reverse_faces(&s, &[]).unwrap();
        assert!(turned.is_surface() && turned.face_count() == 1);
        let down = measure(&thicken(&turned, 2.0, false).unwrap()).unwrap().bbox;
        assert!((up.min.z - down.min.z - 2.0).abs() < 1e-9 && (up.max.z - down.max.z - 2.0).abs() < 1e-9, "{up:?} vs {down:?}");
        assert!(reverse_faces(&box_solid(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)).unwrap(), &[]).is_err());
    }

    /// A box's top taken away without healing: a five-face surface; patching the opening and
    /// stitching closes the same box. Every face, or none, is refused.
    #[test]
    fn remove_faces_leaves_the_gap_open() {
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let top = nearest_face(&b, Vec3::new(5.0, 5.0, 10.0)).unwrap();
        let open = remove_faces(&b, &[top]).unwrap();
        assert!(open.is_surface() && open.face_count() == 5);
        assert!(rel(measure(&open).unwrap().area, 500.0) < 1e-9);
        let lid = patch_edges(&open, &[Vec3::new(5.0, 0.0, 10.0), Vec3::new(10.0, 5.0, 10.0), Vec3::new(5.0, 10.0, 10.0), Vec3::new(0.0, 5.0, 10.0)])
            .unwrap();
        let back = stitch(&[&open, &lid], 1e-6).unwrap();
        assert!(!back.is_surface() && rel(measure(&back).unwrap().volume, 1000.0) < 1e-9);
        assert!(remove_faces(&b, &(0..6).collect::<Vec<_>>()).is_err());
        assert!(remove_faces(&b, &[]).is_err());
    }

    /// Faces that are not planes or cylinders thicken onto fitted surfaces. A sixth of a sphere
    /// (one of its six patches) thickened out by 1 is a sixth of the shell between radii 10 and
    /// 11; a torus patch (a symmetric share of the torus, by its area) thickened out by 1 is that
    /// share of the shell 2π²R((r+1)² − r²); thickened in past the sphere's centre, refused.
    #[test]
    fn thicken_curved_faces_onto_fitted_surfaces() {
        use crate::{sphere, torus};
        use std::f64::consts::PI;
        let ball = sphere(Vec3::ZERO, 10.0).unwrap();
        let patch = copy_faces(&ball, &[Vec3::new(0.0, 0.0, 10.0)]).unwrap();
        assert!(rel(measure(&patch).unwrap().area, 4.0 * PI * 100.0 / 6.0) < 2e-3, "one sixth");
        let shell = thicken(&patch, 1.0, false).unwrap();
        assert!(!shell.is_surface());
        let want = 4.0 / 3.0 * PI * (1331.0 - 1000.0) / 6.0;
        let v = measure(&shell).unwrap().volume;
        assert!(rel(v, want) < 3e-3, "{v} vs {want}"); // (measured on meshes)
        assert!(thicken(&patch, -10.0, false).is_err());
        // A torus patch (R 20, r 5): between the tube radii r and r + 1 over its own angles, θ round
        // the tube and φ round the axis (read off its mesh): the volume is
        // Δφ·[R((r+1)² − r²)/2·Δθ + ((r+1)³ − r³)/3·(sin θ1 − sin θ0)].
        let ring = torus(Vec3::ZERO, 20.0, 5.0).unwrap();
        let face = copy_faces(&ring, &[Vec3::new(25.0, 0.0, 0.0)]).unwrap();
        let mesh = face.tessellate(0.01).unwrap();
        let angles: Vec<(f64, f64)> = mesh.positions.iter().map(|p| (p.z.atan2((p.x * p.x + p.y * p.y).sqrt() - 20.0), p.y.atan2(p.x))).collect();
        let span = |f: &dyn Fn(&(f64, f64)) -> f64| {
            let (lo, hi) = angles.iter().map(f).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(x), b.max(x)));
            (lo, hi)
        };
        let ((t0, t1), (p0, p1)) = (span(&|a| a.0), span(&|a| a.1));
        assert!(t1 - t0 < PI && p1 - p0 < PI, "a patch within half a turn each way: {t0}..{t1}, {p0}..{p1}");
        let (r, rr, big) = (5.0f64, 6.0f64, 20.0);
        let want = (p1 - p0) * (big * (rr * rr - r * r) / 2.0 * (t1 - t0) + (rr.powi(3) - r.powi(3)) / 3.0 * (t1.sin() - t0.sin()));
        let v = measure(&thicken(&face, 1.0, false).unwrap()).unwrap().volume;
        assert!(rel(v, want) < 3e-3, "{v} vs {want}"); // (measured on meshes)
    }

    /// Three faces of a box meeting at a corner (sharp edges), thickened out by 1: three slabs,
    /// a wedge along each sharp edge and a corner tetrahedron, 3·100 + 3·(½·1²·10) + 1/6; a closed
    /// solid that every check accepts. Thickened into the inside corner, refused.
    #[test]
    fn thicken_a_box_corner_across_its_sharp_edges() {
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let corner = copy_faces(&b, &[Vec3::new(5.0, 5.0, 10.0), Vec3::new(10.0, 5.0, 5.0), Vec3::new(5.0, 10.0, 5.0)]).unwrap();
        assert!(corner.is_surface() && corner.face_count() == 3);
        let t = thicken(&corner, 1.0, false).unwrap();
        assert!(!t.is_surface());
        let want = 300.0 + 15.0 + 1.0 / 6.0;
        let v = measure(&t).unwrap().volume;
        assert!(rel(v, want) < 1e-9, "{v} vs {want}");
        assert!(Body::new(t.solid.as_ref().clone()).is_ok());
        assert!(thicken(&corner, -1.0, false).is_err());
        assert!(thicken(&corner, 1.0, true).is_err(), "half of it goes into the inside corner");
    }

    /// A saddle B-spline patch thickened by 1.5 holds the volume its offsets sweep: by Steiner,
    /// the offset's area is quadratic in the distance, so Simpson's rule over sampled offsets
    /// (Offset Surface at 0, ¾ and 1.5) is exact.
    #[test]
    fn thicken_a_bspline_patch_by_its_sampled_offsets() {
        let b = box_solid(Vec3::new(-14.0, -14.0, 0.0), Vec3::new(14.0, 14.0, 20.0)).unwrap();
        let tool = crate::cylinder(Vec3::new(0.0, -60.0, 50.0), Vec3::Y, 40.0, 120.0).unwrap();
        let cup = crate::boolean(&b, &tool, crate::BoolOp::Cut).unwrap().unwrap();
        let z = |x: f64| 50.0 - (1600.0 - x * x).sqrt();
        let rim = [Vec3::new(14.0, 0.0, z(14.0)), Vec3::new(0.0, 14.0, z(0.0)), Vec3::new(-14.0, 0.0, z(-14.0)), Vec3::new(0.0, -14.0, z(0.0))];
        let cover = patch_edges(&cup, &rim).unwrap();
        let t = 1.5;
        let area = |d: f64| measure(&crate::offset_surface(&cover, &[0], d, 0.01).unwrap()).unwrap().area;
        let want = t / 6.0 * (area(0.0) + 4.0 * area(t / 2.0) + area(t));
        let v = measure(&thicken(&cover, t, false).unwrap()).unwrap().volume;
        assert!(rel(v, want) < 2e-3, "{v} vs {want}"); // (measured on meshes)
    }

    /// A cone frustum's side (radii 10 and 5, 10 high) thickened out by 1: a cone has no Gaussian
    /// curvature, so its offset's area grows linearly and the volume is t·π·L·(r1 + r2 + t·cos β),
    /// L the slant length and β the angle between the normal and the radius (cos β = 10/L).
    #[test]
    fn thicken_a_cone_frustum_side() {
        let region = Region2 {
            outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(5.0, 10.0), Vec2::new(0.0, 10.0)]),
            holes: vec![],
        };
        let xz = Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Z };
        let frustum = crate::revolve(&xz, &[region], Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), 2.0 * std::f64::consts::PI).unwrap().remove(0);
        // The side comes in several pieces round the axis: picked all the way round.
        let round: Vec<Vec3> =
            (0..8).map(|k| f64::from(k) * std::f64::consts::FRAC_PI_4).map(|a| Vec3::new(7.5 * a.cos(), 7.5 * a.sin(), 5.0)).collect();
        let side = copy_faces(&frustum, &round).unwrap();
        let l = 125f64.sqrt();
        assert!(rel(measure(&side).unwrap().area, std::f64::consts::PI * 15.0 * l) < 2e-3, "the whole side");
        let v = measure(&thicken(&side, 1.0, false).unwrap()).unwrap().volume;
        let want = std::f64::consts::PI * l * (15.0 + 10.0 / l);
        assert!(rel(v, want) < 3e-3, "{v} vs {want}"); // (measured on meshes)
    }

    #[test]
    fn patch_and_thicken_a_square() {
        let r = Region2 {
            outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)]),
            holes: vec![],
        };
        let s = patch_region(&Plane::XY, &r).unwrap();
        assert!(s.is_surface());
        let t = thicken(&s, 2.0, false).unwrap();
        assert!(!t.is_surface());
        assert!(rel(measure(&t).unwrap().volume, 200.0) < 1e-9);
        let t = thicken(&s, 2.0, true).unwrap();
        let m = measure(&t).unwrap();
        assert!(rel(m.volume, 200.0) < 1e-9 && (m.bbox.min.z + 1.0).abs() < 1e-9, "{:?}", m.bbox);
    }

    #[test]
    fn thicken_a_cylinder_side_and_a_box_top() {
        let c = cylinder(Vec3::ZERO, Vec3::Z, 10.0, 20.0).unwrap();
        // One half of the side, offset outward by 1: half of a ring.
        let half = copy_faces(&c, &[Vec3::new(7.0, 7.0, 10.0)]).unwrap();
        let v = measure(&thicken(&half, 1.0, false).unwrap()).unwrap().volume;
        let want = std::f64::consts::PI * (121.0 - 100.0) / 2.0 * 20.0;
        assert!(rel(v, want) < 3e-4, "{v} vs {want}"); // (measured on meshes)
        // Both halves (meeting smoothly): the whole ring, inward.
        let d = 10.0 / 2f64.sqrt();
        let side = copy_faces(&c, &[Vec3::new(d, d, 10.0), Vec3::new(-d, -d, 10.0)]).unwrap();
        assert_eq!(side.face_count(), 2);
        let v = measure(&thicken(&side, -1.0, false).unwrap()).unwrap().volume;
        let want = std::f64::consts::PI * (100.0 - 81.0) * 20.0;
        assert!(rel(v, want) < 3e-4, "{v} vs {want}"); // (measured on meshes)
        // Two faces meeting at a sharp edge: two slabs and the wedge closing the gap between
        // them, its ends patched: 2·100 + ½·1²·10.
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let two = copy_faces(&b, &[Vec3::new(5.0, 5.0, 10.0), Vec3::new(5.0, 0.0, 5.0)]).unwrap();
        let v = measure(&thicken(&two, 1.0, false).unwrap()).unwrap().volume;
        assert!(rel(v, 205.0) < 1e-9, "{v}");
    }

    #[test]
    fn stitch_six_faces_into_a_box_and_patch_a_hole() {
        let b = box_solid(Vec3::ZERO, Vec3::new(10.0, 20.0, 30.0)).unwrap();
        let pts = [
            Vec3::new(5.0, 10.0, 0.0),
            Vec3::new(5.0, 10.0, 30.0),
            Vec3::new(0.0, 10.0, 15.0),
            Vec3::new(10.0, 10.0, 15.0),
            Vec3::new(5.0, 0.0, 15.0),
            Vec3::new(5.0, 20.0, 15.0),
        ];
        let parts: Vec<Body> = pts.iter().map(|p| copy_faces(&b, &[*p]).unwrap()).collect();
        let refs: Vec<&Body> = parts.iter().collect();
        let s = stitch(&refs, 0.0).unwrap();
        assert!(!s.is_surface());
        assert!(rel(measure(&s).unwrap().volume, 6000.0) < 1e-9);
        // Five faces leave an open box; patching the top's edges closes it again.
        let open = stitch(&[refs[0], refs[2], refs[3], refs[4], refs[5]], 0.0).unwrap();
        assert!(open.is_surface());
        let lid =
            patch_edges(&open, &[Vec3::new(5.0, 0.0, 30.0), Vec3::new(10.0, 10.0, 30.0), Vec3::new(5.0, 20.0, 30.0), Vec3::new(0.0, 10.0, 30.0)])
                .unwrap();
        let closed = stitch(&[&open, &lid], 0.0).unwrap();
        assert!(!closed.is_surface());
        assert!(rel(measure(&closed).unwrap().volume, 6000.0) < 1e-9);
    }

    #[test]
    fn extend_a_planar_surface() {
        let r = Region2 {
            outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)]),
            holes: vec![],
        };
        let s = patch_region(&Plane::XY, &r).unwrap();
        let e = extend(&s, &[Vec3::new(10.0, 5.0, 0.0)], 5.0).unwrap();
        assert!(rel(measure(&e).unwrap().area, 150.0) < 1e-9);
        let e = extend(&s, &[Vec3::new(10.0, 5.0, 0.0), Vec3::new(5.0, 10.0, 0.0)], 5.0).unwrap();
        assert!(rel(measure(&e).unwrap().area, 225.0) < 1e-9);
        assert!(extend(&s, &[Vec3::new(10.0, 5.0, 0.0)], -1.0).is_err());
    }

    #[test]
    fn trim_keeps_one_side() {
        let r = Region2 {
            outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)]),
            holes: vec![],
        };
        let s = patch_region(&Plane::XY, &r).unwrap();
        let pl = Plane::new(Vec3::new(3.0, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap();
        let t = trim(&s, &crate::SplitTool::Plane(pl), Vec3::new(1.0, 5.0, 0.0)).unwrap();
        assert!(rel(measure(&t).unwrap().area, 30.0) < 1e-9);
        let t = trim(&s, &crate::SplitTool::Plane(pl), Vec3::new(8.0, 5.0, 0.0)).unwrap();
        assert!(rel(measure(&t).unwrap().area, 70.0) < 1e-9);
    }
}
