//! Display tessellation: each face's triangulation in its own parameters, with every edge
//! sampled once so neighbouring faces share their boundary points exactly, handed over as a
//! [`TessPlan`] for the CPU or GPU evaluator.
//!
//! Per face, cheapest first:
//! - planar faces: a constrained Delaunay triangulation of the edge nodes alone (a plane needs
//!   no interior points);
//! - faces whose boundary runs along their parameter box (cylinder halves, sphere and torus
//!   patches, fillets): a structured grid zipped to the edge nodes;
//! - four-cornered faces whose sides are not parameter lines (mitred fillet strips, cross
//!   hole walls): a Coons blend of the four sides, zipped the same way;
//! - anything else (trimmed faces, holes): constrained Delaunay with interior seeds and
//!   Ruppert refinement.
//!
//! A face the planner cannot handle (its surface has no evaluable form, a boundary that does
//! not project or crosses itself) fails the whole plan, and the caller meshes the body the
//! old way.

pub(crate) mod cdt;
pub(crate) mod collar;
pub(crate) mod coons;
pub(crate) mod cutgrid;
pub(crate) mod edges;
pub(crate) mod grid;
pub(crate) mod motorcycle;
pub(crate) mod pieces;
pub(crate) mod seam;
pub(crate) mod size;
pub(crate) mod ssi;
pub(crate) mod surface;
pub(crate) mod uv;

use std::collections::HashMap;

use solidcraft_geom::Vec3;
use solidcraft_geom::tess::{EvalSurface, FacePlan, TessPlan};
use truck_modeling as mt;

use crate::body::{Solid, edge_order};
use crate::{KernelError, Result};

/// How the faces of a plan were meshed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlanStats {
    pub planar: usize,
    pub structured: usize,
    pub delaunay: usize,
}

/// Curved faces from which their sizes are worked out on the worker pool.
const PARALLEL_SIZES: usize = 8;
/// Curved faces from which faces are planned on the worker pool (measured on the oracle
/// parts: two curved faces plan faster serially, four break even, six run twice as fast).
const PARALLEL_FACES: usize = 4;
/// Intersection curves (a Newton solve per point), or edges of any kind, from which edges are
/// sampled on the worker pool.
const PARALLEL_CURVES: usize = 2;
const PARALLEL_EDGES: usize = 256;

/// Most lines a curved face's grid gets in one direction.
const MAX_LINES: usize = 1024;
/// Most points Ruppert refinement may add to one face, beyond its seeds.
const MAX_REFINE: usize = 20_000;

fn unsupported(what: impl std::fmt::Display) -> KernelError {
    KernelError::Failed(format!("not supported yet: tessellation: {what}"))
}

/// Plan the display tessellation of `solid` with chord tolerance `tol` and, when given, a
/// facet angle no facet may turn more than (where it sags above the angle's floor).
pub(crate) fn plan(solid: &Solid, tol: f64, turn: Option<size::Turn>) -> Result<(TessPlan, PlanStats)> {
    let faces: Vec<&mt::Face> = solid.face_iter().collect();
    let (edges, edge_faces) = edge_order(faces.iter().copied());
    let rules = size::Rules::new(tol, size::extent(solid.vertex_iter().map(|v| crate::body::from_p3(v.point()))), turn);
    // Each face's surface and target side length, then every edge once at the smaller size of
    // its faces; faces then read their nodes. A planar face costs microseconds and a curved
    // one about a hundred, so the pool is used only where curved faces (or intersection
    // curves) make up the work.
    let evals: Vec<Option<EvalSurface>> = faces.iter().map(|f| surface::eval_surface(&f.surface())).collect();
    let curved = evals.iter().filter(|e| !matches!(e, Some(EvalSurface::Plane { .. }))).count();
    let sizes =
        crate::par::map("face sizes", &evals, curved >= PARALLEL_SIZES, |eval| Ok(eval.as_ref().map(|s| size::face_size(s, &uv::domain(s), &rules))));
    let sized: Vec<(Option<EvalSurface>, Option<(f64, [f64; 2])>)> =
        evals.into_iter().zip(sizes).map(|(e, s)| s.map(|s| (e, s))).collect::<Result<_>>()?;
    let face_h = |f: usize| sized.get(f).and_then(|(_, s)| s.map(|(h, _)| h)).unwrap_or(rules.max_len);
    let edge_h: Vec<f64> = edge_faces.iter().map(|fs| fs.iter().map(|&f| face_h(f)).fold(rules.max_len, f64::min)).collect();
    let jobs: Vec<(&mt::Edge, f64)> = edges.iter().zip(edge_h).collect();
    let solved = edges.iter().filter(|e| matches!(e.curve(), mt::Curve::IntersectionCurve(_))).count();
    let sampled = crate::par::map("edge nodes", &jobs, solved >= PARALLEL_CURVES || edges.len() >= PARALLEL_EDGES, |(e, h)| {
        Ok(edges::sample_edge(e, &rules, *h))
    });
    let mut nodes: HashMap<_, edges::Nodes> = HashMap::with_capacity(edges.len());
    for (e, n) in edges.iter().zip(sampled) {
        nodes.insert(e.id(), n?);
    }
    relax_crowded_faces(&faces, &jobs, &rules, &mut nodes);
    divide_collapsed_faces(&faces, &jobs, &rules, &mut nodes);
    let oriented = |e: &mt::Edge| -> Option<Vec<Vec3>> {
        let mut pts = nodes.get(&e.id())?.points.clone();
        if !e.orientation() {
            pts.reverse();
        }
        Some(pts)
    };
    let indexed: Vec<(usize, &mt::Face)> = faces.iter().copied().enumerate().collect();
    let planned = crate::par::map("face tessellation", &indexed, curved >= PARALLEL_FACES, |(fi, face)| {
        let fi = *fi;
        let (eval, size) = match sized.get(fi) {
            Some((Some(eval), Some(size))) => (eval.clone(), *size),
            _ => return Err(unsupported(format!("face {fi}: surface not evaluable"))),
        };
        // Boundary nodes, loop by loop (each edge's last node is the next edge's first), with
        // their parameters on this face where the edge knows them (an intersection edge on
        // this face's surface).
        let mut loops3: Vec<Vec<Vec3>> = Vec::new();
        let mut known: Vec<Vec<Option<[f64; 2]>>> = Vec::new();
        for wire in face.absolute_boundaries() {
            let (mut pts, mut uvs) = (Vec::new(), Vec::new());
            for e in wire.edge_iter() {
                let n = nodes.get(&e.id()).ok_or_else(|| unsupported("edge without nodes"))?;
                let (mut ep, mut eq) = (n.points.clone(), n.params_on(&eval).unwrap_or_else(|| vec![None; n.points.len()]));
                if !e.orientation() {
                    ep.reverse();
                    eq.reverse();
                }
                ep.pop();
                eq.pop();
                pts.extend(ep);
                uvs.extend(eq);
            }
            if pts.len() >= 2 {
                loops3.push(pts);
                known.push(uvs);
            }
        }
        let mut stats = PlanStats::default();
        let replan = |stats: &mut PlanStats| match plan_face(fi, &eval, &loops3, &known, &rules, size, stats) {
            Ok(fp) => Ok(fp),
            // A boundary that doubles straight back on itself (a zero-width slit in the face,
            // which some STEP writers leave where two surfaces were trimmed apart) encloses no
            // area but crosses itself in (u, v). Dropping the slits leaves the same face.
            Err(first) => {
                let (slit3, slitq) = drop_slits(&loops3, &known, rules.tol * 1e-2);
                if slit3 == loops3 {
                    return Err(first);
                }
                *stats = PlanStats::default();
                plan_face(fi, &eval, &slit3, &slitq, &rules, size, stats).map_err(|_| first)
            }
        };
        let fp = replan(&mut stats)?;
        let fp = FacePlan {
            flip: !face.orientation(),
            triangles: if face.orientation() { fp.triangles } else { fp.triangles.into_iter().map(|[a, b, c]| [a, c, b]).collect() },
            ..fp
        };
        Ok((fp, eval, stats))
    });
    let mut plan = TessPlan::default();
    let mut stats = PlanStats::default();
    for r in planned {
        let (fp, eval, s) = r?;
        let surface = u32::try_from(plan.surfaces.len()).map_err(|_| unsupported("too many faces"))?;
        plan.surfaces.push(eval);
        plan.faces.push(FacePlan { surface: Some(surface), ..fp });
        stats.planar += s.planar;
        stats.structured += s.structured;
        stats.delaunay += s.delaunay;
    }
    plan.edges = edges.iter().filter_map(oriented).collect();
    plan.edge_faces = edge_faces.into_iter().map(|fs| fs.into_iter().map(|f| u32::try_from(f).unwrap_or(u32::MAX)).collect()).collect();
    Ok((plan, stats))
}

/// Whether every curved face's boundary lies on the face's surface, within `accept`: a point
/// off the surface's own parameter range is off it. The planner needs this of every face; a
/// blend that runs on past the end of a neighbouring round's surface does not have it.
pub(crate) fn boundaries_on_surfaces(solid: &Solid, accept: f64) -> bool {
    use mt::{BoundedCurve, ParametricCurve};
    for face in solid.face_iter() {
        let Some(eval) = surface::eval_surface(&face.surface()) else { continue };
        if matches!(eval, EvalSurface::Plane { .. }) {
            continue;
        }
        let dom = uv::domain(&eval);
        let mut prev: Option<[f64; 2]> = None;
        for e in face.edge_iter() {
            let c = e.oriented_curve();
            let (lo, hi) = c.range_tuple();
            for k in 0..=32 {
                let p = crate::body::from_p3(c.subs(lo + (hi - lo) * f64::from(k) / 32.0));
                match uv::project(&eval, &dom, p, prev, accept) {
                    Some(uv) => prev = Some(uv),
                    None => return false,
                }
            }
        }
    }
    true
}

/// Bounded work for the facet angle, as the reference mesher bounds it: a face whose boundary
/// would get more than `MAX_ANGLE_BOUNDARY` segments has its curved edges sampled again at twice
/// the angle, as often as it takes, down to the tolerance alone (a plate with a thousand small
/// holes is not worth a hundred times the work to draw each hole round). An edge keeps one set
/// of nodes for both its faces, at the coarser of their levels, so every rim of a crowded face
/// gets the same share whatever the order of the faces.
fn relax_crowded_faces(faces: &[&mt::Face], jobs: &[(&mt::Edge, f64)], rules: &size::Rules, nodes: &mut HashMap<mt::EdgeID, edges::Nodes>) {
    use truck_meshalgo::tessellation::Deflection;
    let Some(turn) = rules.turn else { return };
    let h_of: HashMap<mt::EdgeID, (&mt::Edge, f64)> = jobs.iter().map(|(e, h)| (e.id(), (*e, *h))).collect();
    let boundaries: Vec<Vec<mt::EdgeID>> =
        faces.iter().map(|f| f.absolute_boundaries().iter().flat_map(|w| w.edge_iter().map(|e| e.id())).collect()).collect();
    let mut level: HashMap<mt::EdgeID, u8> = HashMap::new();
    for l in 1..=Deflection::MAX_RELAX {
        let segments = |id: &mt::EdgeID| nodes.get(id).map_or(0, |n| n.points.len().saturating_sub(1));
        let mut crowded: Vec<mt::EdgeID> = boundaries
            .iter()
            .filter(|ids| ids.iter().map(segments).sum::<usize>() > Deflection::MAX_ANGLE_BOUNDARY)
            .flatten()
            .filter(|id| level.get(*id).copied().unwrap_or(0) < l)
            .filter(|id| h_of.get(*id).is_some_and(|(e, _)| !matches!(e.curve(), mt::Curve::Line(_))))
            .copied()
            .collect();
        crowded.sort_unstable_by_key(|id| format!("{id:?}"));
        crowded.dedup();
        if crowded.is_empty() {
            break;
        }
        let relaxed = size::Rules { turn: turn.relaxed(l), ..*rules };
        for id in crowded {
            if let Some((e, h)) = h_of.get(&id) {
                nodes.insert(id, edges::sample_edge(e, &relaxed, *h));
                level.insert(id, l);
            }
        }
    }
}

/// Every loop with its zero-width slits removed: wherever the boundary steps out to a node and
/// straight back to where it came from, the pair is dropped. A slit encloses no area, so the
/// face is the same without it, and without it the boundary no longer runs along itself. The
/// two sides need only agree within `eps` (a hundredth of the chord tolerance): a STEP writer
/// may draw them as two curves a few nanometres apart, which no mesh could tell apart anyway.
fn drop_slits(loops3: &[Vec<Vec3>], known: &[Vec<Option<[f64; 2]>>], eps: f64) -> (Vec<Vec<Vec3>>, Vec<Vec<Option<[f64; 2]>>>) {
    /// Slits dropped from one loop before giving up (the search is a scan per slit).
    const MAX_SLITS: usize = 4096;
    let mut out3: Vec<Vec<Vec3>> = Vec::with_capacity(loops3.len());
    let mut outq: Vec<Vec<Option<[f64; 2]>>> = Vec::with_capacity(loops3.len());
    for (li, pts) in loops3.iter().enumerate() {
        let mut pts = pts.clone();
        let mut qs = known.get(li).cloned().unwrap_or_else(|| vec![None; pts.len()]);
        qs.resize(pts.len(), None);
        for _ in 0..MAX_SLITS {
            let n = pts.len();
            if n < 3 {
                break;
            }
            let same = |a: usize, b: usize| pts.get(a).zip(pts.get(b)).is_some_and(|(p, q)| p.dist(*q) <= eps);
            let tip = (0..n).find(|&k| same((k + n - 1) % n, (k + 1) % n));
            let Some(k) = tip else { break };
            // Drop the tip and the node that repeats the one before it.
            let mut drop = [k, (k + 1) % n];
            drop.sort_unstable();
            for &i in drop.iter().rev() {
                if i < pts.len() {
                    pts.remove(i);
                }
                if i < qs.len() {
                    qs.remove(i);
                }
            }
        }
        if pts.len() >= 2 {
            out3.push(pts);
            outq.push(qs);
        }
    }
    (out3, outq)
}

/// Bring every loop of a face into the same period as its outer boundary. A loop is projected
/// on its own, and a periodic surface repeats, so a hole can come out a whole turn away from the
/// boundary it lies in — where it no longer cuts the face, and the face is drawn over its hole.
fn align_periods(loops: &mut [seam::Loop], dom: &uv::Domain) {
    for d in 0..2 {
        let Some(per) = dom.period.get(d).copied().flatten().filter(|p| *p > 0.0) else { continue };
        let middle = |l: &seam::Loop| {
            let (lo, hi) = l.uv.iter().filter_map(|q| q.get(d)).fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
            0.5 * (lo + hi)
        };
        let Some(outer) = loops.first().map(middle) else { continue };
        if !outer.is_finite() {
            continue;
        }
        for l in loops.iter_mut().skip(1) {
            let shift = ((outer - middle(l)) / per).round() * per;
            if shift != 0.0 && shift.is_finite() {
                for q in l.uv.iter_mut() {
                    if let Some(x) = q.get_mut(d) {
                        *x += shift;
                    }
                }
            }
        }
    }
}

/// Nodes a face's boundary needs before it can hold a triangle.
const MIN_BOUNDARY_NODES: usize = 3;

/// A face whose whole boundary comes to fewer than three nodes has no triangle to stand on: it is
/// a sliver no wider than the tolerance (a chord with the shallow arc beside it, which the edge
/// sampler rightly leaves as one segment each). Its edges are sampled again, halved until the
/// boundary has nodes enough, so the sliver is drawn instead of failing the whole body's plan.
/// The nodes stay shared, so the faces beside it follow the same points.
fn divide_collapsed_faces(faces: &[&mt::Face], jobs: &[(&mt::Edge, f64)], rules: &size::Rules, nodes: &mut HashMap<mt::EdgeID, edges::Nodes>) {
    /// Halvings before giving up (a boundary of two edges reaches four nodes in one).
    const MAX_HALVINGS: usize = 8;
    let h_of: HashMap<mt::EdgeID, (&mt::Edge, f64)> = jobs.iter().map(|(e, h)| (e.id(), (*e, *h))).collect();
    for f in faces {
        let ids: Vec<mt::EdgeID> = f.absolute_boundaries().iter().flat_map(|w| w.edge_iter().map(|e| e.id())).collect();
        let count = |nodes: &HashMap<mt::EdgeID, edges::Nodes>| {
            ids.iter().map(|id| nodes.get(id).map_or(0, |n| n.points.len().saturating_sub(1))).sum::<usize>()
        };
        if count(nodes) >= MIN_BOUNDARY_NODES {
            continue;
        }
        for _ in 0..MAX_HALVINGS {
            for id in &ids {
                let Some((e, h)) = h_of.get(id) else { continue };
                let longest =
                    nodes.get(id).map_or(0.0, |n| n.points.windows(2).filter_map(|w| Some(w.first()?.dist(*w.get(1)?))).fold(0.0, f64::max));
                let finer = if longest > 0.0 { (longest * 0.5).min(*h) } else { *h };
                nodes.insert(*id, edges::sample_edge(e, rules, finer));
            }
            if count(nodes) >= MIN_BOUNDARY_NODES {
                break;
            }
        }
    }
}

/// One face: boundary loops in 3D (shared edge nodes) to parameters and triangles.
/// `size`: the face's target side length (mm) and the surface's mean speed along u and v.
fn plan_face(
    fi: usize,
    s: &EvalSurface,
    loops3: &[Vec<Vec3>],
    known: &[Vec<Option<[f64; 2]>>],
    rules: &size::Rules,
    size: (f64, [f64; 2]),
    stats: &mut PlanStats,
) -> Result<FacePlan> {
    let tol = rules.tol;
    let (h, speed) = size;
    let dom = uv::domain(s);
    let accept = (tol * 10.0).max(1e-6);
    let fail = |why: &str| unsupported(format!("face {fi}: {why}"));
    // Project every loop, unwrapping periodic parameters along it.
    let mut projected: Vec<seam::Loop> = Vec::with_capacity(loops3.len());
    for (li, pts) in loops3.iter().enumerate() {
        let mut luv: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
        let mut prev: Option<[f64; 2]> = None;
        for (k, p) in pts.iter().enumerate() {
            // Known parameters (an intersection edge's), else a projection seeded by the
            // previous node.
            let given = known.get(li).and_then(|l| l.get(k)).copied().flatten();
            let mut q = match given {
                Some(q) => q,
                None => uv::project(s, &dom, *p, prev, accept).ok_or_else(|| fail("boundary does not lie on the surface"))?,
            };
            if let Some(pr) = prev {
                for d in 0..2 {
                    q[d] = uv::unwrap(q[d], pr[d], dom.period[d]);
                }
            }
            luv.push(q);
            prev = Some(q);
        }
        projected.push(seam::Loop { uv: luv, pts: pts.clone() });
    }
    align_periods(&mut projected, &dom);
    // A loop that winds around a periodic direction does not close in (u, v): the face is cut
    // along a parameter line into one period (a seamless cylinder face, a helical trim).
    let wound: Vec<usize> = (0..2).filter(|&d| dom.period[d].is_some_and(|per| seam::winds(&projected, d, per))).collect();
    // A face cut along a seam is a long winding strip whose sides are the cut lines and the
    // boundary in turn: Delaunay with sag refinement meshes it (the patch layouts below assume
    // boundary all round, and leave fans across the strip's corners off the surface).
    let seamed = !wound.is_empty();
    match wound.as_slice() {
        [] => {}
        [d] => {
            let per = dom.period[*d].unwrap_or(std::f64::consts::TAU);
            projected = seam::cut(s, &projected, *d, per, rules, h).ok_or_else(|| fail("boundary winds around the surface"))?;
        }
        _ => return Err(fail("boundary winds around the surface both ways")),
    }
    // Split poles.
    let mut fixed: Vec<Vec3> = Vec::new();
    let mut uvs: Vec<[f64; 2]> = Vec::new();
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for seam::Loop { uv: luv, pts } in &projected {
        let start = uvs.len();
        let n = luv.len();
        for (k, (q, p)) in luv.iter().zip(pts).enumerate() {
            // At a pole the parameter along the degenerate direction is free: one copy with
            // the previous node's value, one with the next node's.
            let (prev, next) = (luv.get((k + n - 1) % n).copied(), luv.get((k + 1) % n).copied());
            let pole = (0..2).find(|&d| uv::degenerate(s, *q, d));
            match (pole, prev, next) {
                (Some(d), Some(a), Some(b)) => {
                    let mut qa = *q;
                    let mut qb = *q;
                    qa[d] = a[d];
                    qb[d] = b[d];
                    uvs.push(qa);
                    fixed.push(*p);
                    if (qa[d] - qb[d]).abs() > 1e-12 {
                        uvs.push(qb);
                        fixed.push(*p);
                    }
                }
                _ => {
                    uvs.push(*q);
                    fixed.push(*p);
                }
            }
        }
        ranges.push(start..uvs.len());
    }
    if uvs.len() < MIN_BOUNDARY_NODES {
        return Err(fail("degenerate boundary"));
    }
    let mut bbox = [(f64::MAX, f64::MIN); 2];
    for p in &uvs {
        for d in 0..2 {
            bbox[d] = (bbox[d].0.min(p[d]), bbox[d].1.max(p[d]));
        }
    }
    // Constrained Delaunay in (u, v) scaled to millimetres: equilateral seeds of side `h`,
    // refined where triangles leave the surface, are too long or bend too much, then shaped by
    // Ruppert refinement.
    let delaunay = || {
        let polys: Vec<&[[f64; 2]]> = ranges.iter().filter_map(|r| uvs.get(r.clone())).collect();
        let seeds = cdt::lattice(bbox, [h / speed[0], h / speed[1]], &polys, speed);
        let eval = |u: f64, v: f64| s.derivs(u, v).map(|(p, su, sv)| (p, su.cross(sv).normalized()));
        let sag = cdt::Sag { eval: &eval, tol, h, turn: rules.turn, scale: speed };
        let fill = cdt::Fill { seeds: &seeds, refine: Some((sag, MAX_REFINE)), shape: !matches!(s, EvalSurface::Plane { .. }) };
        cdt::triangulate(&uvs, &ranges, fill).ok_or_else(|| fail("boundary crosses itself"))
    };
    // Planar and curved faces alike: a structured grid where the boundary runs along the
    // parameter box (a rectangle, a cylinder half), a Coons grid on four corners, else Delaunay.
    let planar = matches!(s, EvalSurface::Plane { .. });
    let (extra, triangles) = {
        let mut us = uv::divide(s, 0, bbox[0], bbox[1], rules, h, MAX_LINES);
        let mut vs = uv::divide(s, 1, bbox[1], bbox[0], rules, h, MAX_LINES);
        thin(&mut us, &mut vs, grid::MAX_FACE_GRID);
        let structured = match (ranges.len(), if seamed { None } else { grid::quad(&uvs) }) {
            (1, Some(q)) => {
                // Interior lines: the boundary's own where opposite sides agree (refined where the
                // surface turns more between them inside the face), else the division.
                let lines = |d: usize, div: &Vec<f64>| {
                    let inner = |l: Vec<f64>| l.into_iter().filter(|x| *x > bbox[d].0 && *x < bbox[d].1).collect::<Vec<f64>>();
                    match grid::aligned_lines(&uvs, &q, d) {
                        Some(aligned) => {
                            let ends: Vec<f64> = std::iter::once(bbox[d].0).chain(aligned).chain(std::iter::once(bbox[d].1)).collect();
                            inner(uv::refine(s, d, &ends, bbox[1 - d], rules, h, MAX_LINES))
                        }
                        None => inner(div.clone()),
                    }
                };
                let (lu, lv) = (lines(0, &us), lines(1, &vs));
                let strip = match (lu.is_empty(), lv.is_empty()) {
                    (false, true) => grid::strip(&uvs, &q, 0),
                    (true, false) => grid::strip(&uvs, &q, 1),
                    _ => None,
                };
                strip.map(|t| (Vec::new(), t)).or_else(|| grid::structured(&uvs, &q, &grid::interior(lu, bbox[0]), &grid::interior(lv, bbox[1])))
            }
            _ => None,
        };
        let structured = match structured {
            None if ranges.len() == 1 && !seamed => {
                let fine = |a: [f64; 2], b: [f64; 2]| uv::fine_between(s, rules, h, a, b);
                coons::coons(&uvs, &fixed, speed, grid::MAX_FACE_GRID, h, Some(&fine))
            }
            other => other,
        };
        match structured {
            Some(r) => {
                if planar {
                    stats.planar += 1;
                } else {
                    stats.structured += 1;
                }
                r
            }
            None => {
                // A curved face neither box nor four-cornered: cut into four-sided patches, else
                // the iso-line grid cut along its motorcycle traces and trimmed by its boundary,
                // else Delaunay. A plane: the butterfly of a round face, else Delaunay (a plane
                // is shaded the same by any triangles, and its Delaunay needs no refinement).
                let face = Region { s, uvs: &uvs, fixed: &fixed, ranges: &ranges, rules, h, speed, us: &us, vs: &vs };
                let trimmed = if seamed {
                    None
                } else if planar {
                    if ranges.len() == 1 { plan_pieces(&face) } else { None }
                } else {
                    plan_pieces(&face).or_else(|| {
                        let traces = motorcycle::traces(&uvs, &ranges, speed, 0.25 * h);
                        cutgrid::cut_grid(&uvs, &ranges, &us, &vs, &traces, speed)
                    })
                };
                match trimmed {
                    Some(r) => {
                        stats.structured += 1;
                        r
                    }
                    None => {
                        if planar {
                            stats.planar += 1;
                        } else {
                            stats.delaunay += 1;
                        }
                        delaunay()?
                    }
                }
            }
        }
    };
    // At a pole the boundary has two copies of one node: a triangle on both copies has two
    // corners at one point and no area (its other two sides are one segment both ways).
    let pole_pair = |i: u32, j: u32| i != j && fixed.get(i as usize).zip(fixed.get(j as usize)).is_some_and(|(p, q)| p == q);
    let mut triangles = triangles;
    triangles.retain(|t| !(pole_pair(t[0], t[1]) || pole_pair(t[1], t[2]) || pole_pair(t[2], t[0])));
    if !closed_boundary(&ranges, &fixed, &triangles) {
        return Err(fail("triangulation does not follow the boundary"));
    }
    let mut all = uvs;
    all.extend(extra);
    Ok(FacePlan { face: u32::try_from(fi).unwrap_or(u32::MAX), surface: None, uv: all, fixed, normals: Vec::new(), flip: false, triangles })
}

/// A face's (u, v) region and what meshing it needs.
struct Region<'a> {
    s: &'a EvalSurface,
    uvs: &'a [[f64; 2]],
    fixed: &'a [Vec3],
    ranges: &'a [std::ops::Range<usize>],
    rules: &'a size::Rules,
    h: f64,
    speed: [f64; 2],
    us: &'a [f64],
    vs: &'a [f64],
}

/// A trimmed face cut into patches (squares round its holes or a butterfly in a round face,
/// joined to the outer loop by motorcycle traces), each meshed on its own (a Coons blend on its
/// best four corners; else the trimmed grid; else Delaunay), all sharing the cuts' nodes. The
/// cut nodes and patch interiors follow the boundary nodes. `None` when the cuts do not make
/// patches.
fn plan_pieces(f: &Region) -> Option<(Vec<[f64; 2]>, Vec<[u32; 3]>)> {
    // The face's global seed: one spacing in every direction, the finest its size rules ask for
    // along either parameter (the median gap between the grid lines they give, in mm). On a
    // cylinder that is the spacing round it, which the cuts along its axis then get too.
    let median_gap = |lines: &[f64], speed: f64| -> f64 {
        let mut gaps: Vec<f64> = lines.windows(2).filter_map(|w| Some((w.get(1)? - w.first()?) * speed)).filter(|g| *g > 0.0).collect();
        gaps.sort_by(f64::total_cmp);
        gaps.get(gaps.len() / 2).copied().unwrap_or(f64::INFINITY)
    };
    let seed = median_gap(f.us, f.speed[0]).min(median_gap(f.vs, f.speed[1])).min(f.h);
    let segments = collar::segments(f.uvs, f.ranges, f.speed, seed, matches!(f.s, EvalSurface::Plane { .. }))?;
    let s = f.s;
    let fine = |a: [f64; 2], b: [f64; 2]| uv::fine_between(s, f.rules, f.h, a, b);
    // A cut's seeds: spaced from the boundary's own spacing at each end, growing geometrically
    // between them, never wider than the global seed, and split further wherever the surface
    // between two seeds fails the size rules.
    let seeds = |a: [f64; 2], b: [f64; 2], sa: f64, sb: f64| -> Vec<f64> { graded(a, b, sa, sb, seed, f.speed, &fine) };
    let (cut_uv, pieces) = pieces::cut(f.uvs, f.ranges, &segments, f.speed, &seeds)?;
    let n = f.uvs.len();
    let mut uv_all: Vec<[f64; 2]> = f.uvs.to_vec();
    uv_all.extend_from_slice(&cut_uv);
    let mut pos: Vec<Vec3> = f.fixed.to_vec();
    for q in &cut_uv {
        pos.push(s.derivs(q[0], q[1])?.0);
    }
    let mut extra: Vec<[f64; 2]> = cut_uv;
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for pc in &pieces {
        // The patch's own loops, numbered from zero.
        let mut ids: Vec<u32> = pc.ring.clone();
        let mut ranges = vec![std::ops::Range { start: 0, end: pc.ring.len() }];
        for hole in &pc.holes {
            ranges.push(ids.len()..ids.len() + hole.len());
            ids.extend_from_slice(hole);
        }
        let luv: Vec<[f64; 2]> = ids.iter().map(|&k| uv_all.get(k as usize).copied()).collect::<Option<_>>()?;
        let lpos: Vec<Vec3> = ids.iter().map(|&k| pos.get(k as usize).copied()).collect::<Option<_>>()?;
        let mut bbox = [(f64::MAX, f64::MIN); 2];
        for p in &luv {
            for d in 0..2 {
                bbox[d] = (bbox[d].0.min(p[d]), bbox[d].1.max(p[d]));
            }
        }
        let clip = |lines: &[f64], (a, b): (f64, f64)| -> Vec<f64> {
            std::iter::once(a).chain(lines.iter().copied().filter(|x| *x > a && *x < b)).chain(std::iter::once(b)).collect()
        };
        // A patch whose outline changes between boundary and cut at exactly four nodes (a
        // quarter of a ring, a butterfly wing round a hole) takes those as its corners, so each
        // side is all boundary or all cut; any other patch takes the best four of its corners.
        let is_cut = |a: u32, b: u32| -> bool {
            let (a, b) = (a as usize, b as usize);
            a >= n || b >= n || !f.ranges.iter().any(|r| r.contains(&a) && r.contains(&b) && (a.abs_diff(b) == 1 || a.abs_diff(b) == r.len() - 1))
        };
        let ring = &pc.ring;
        let switches: Vec<usize> = (0..ring.len())
            .filter(|&k| {
                let (p, c, x) = (ring.get((k + ring.len() - 1) % ring.len()), ring.get(k), ring.get((k + 1) % ring.len()));
                matches!((p, c, x), (Some(&p), Some(&c), Some(&x)) if is_cut(p, c) != is_cut(c, x))
            })
            .collect();
        // The patch is a rectangle in (u, v) with corners cut by free boundary (Lyon et al. §4;
        // completed virtually in §4.2): its bounding box. A grid needs four real sides, so each
        // corner is the node furthest out towards the box's corner (in the box's own
        // proportions): an uncut corner is the corner itself, and on an arc cutting a corner it
        // is the arc's 45° point, so the arc becomes a rounded corner shared by two sides.
        let completed = || -> Option<[usize; 4]> {
            let m = ring.len();
            let at = |k: usize| luv.get(k).map(|q| [q[0] * f.speed[0], q[1] * f.speed[1]]);
            let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
            for k in 0..m {
                let q = at(k)?;
                for d in 0..2 {
                    lo[d] = lo[d].min(q[d]);
                    hi[d] = hi[d].max(q[d]);
                }
            }
            let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
            if !(w > 0.0 && h > 0.0) {
                return None;
            }
            let mut pick = [0usize; 4];
            for (slot, dir) in pick.iter_mut().zip([[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]) {
                *slot = (0..m).max_by(|&x, &y| {
                    let reach = |k: usize| at(k).map_or(f64::MIN, |q| dir[0] * (q[0] - lo[0]) / w + dir[1] * (q[1] - lo[1]) / h);
                    reach(x).total_cmp(&reach(y))
                })?;
            }
            // Four distinct corners in the loop's (counter-clockwise) order.
            let start = pick[0];
            let rel = pick.map(|k| (k + m - start) % m);
            (rel[0] < rel[1] && rel[1] < rel[2] && rel[2] < rel[3]).then_some(pick)
        };
        let quad = if pc.holes.is_empty() {
            // Every patch is four-sided by construction: its corners are where its outline turns.
            coons::coons(&luv, &lpos, f.speed, grid::MAX_FACE_GRID, f.h, None).or_else(|| {
                match switches[..] {
                    [a, b, c, d] => coons::coons_at(&luv, &lpos, [a, b, c, d], grid::MAX_FACE_GRID, f.h, None),
                    _ => None,
                }
                .or_else(|| completed().and_then(|c| coons::coons_at(&luv, &lpos, c, grid::MAX_FACE_GRID, f.h, None)))
            })
        } else {
            None
        };
        let local = quad
            .or_else(|| if pc.holes.is_empty() { coons::coons_best(&luv, &lpos, f.speed, grid::MAX_FACE_GRID, f.h) } else { None })
            .or_else(|| cutgrid::cut_grid(&luv, &ranges, &clip(f.us, bbox[0]), &clip(f.vs, bbox[1]), &[], f.speed))
            .or_else(|| {
                let polys: Vec<&[[f64; 2]]> = ranges.iter().filter_map(|r| luv.get(r.clone())).collect();
                let seeds = cdt::lattice(bbox, [f.h / f.speed[0], f.h / f.speed[1]], &polys, f.speed);
                let eval = |u: f64, v: f64| s.derivs(u, v).map(|(p, su, sv)| (p, su.cross(sv).normalized()));
                let sag = cdt::Sag { eval: &eval, tol: f.rules.tol, h: f.h, turn: f.rules.turn, scale: f.speed };
                cdt::triangulate(&luv, &ranges, cdt::Fill { seeds: &seeds, refine: Some((sag, MAX_REFINE)), shape: true })
            })?;
        let (lextra, ltris) = local;
        let base = u32::try_from(n + extra.len()).ok()?;
        let m = ids.len();
        for t in ltris {
            let map = |i: u32| -> Option<u32> {
                let i = i as usize;
                if i < m { ids.get(i).copied() } else { base.checked_add(u32::try_from(i - m).ok()?) }
            };
            tris.push([map(t[0])?, map(t[1])?, map(t[2])?]);
        }
        extra.extend(lextra);
    }
    Some((extra, tris))
}

/// Seeds on the segment `a`–`b` in (u, v) (fractions strictly between 0 and 1): node spacing
/// `sa` at `a` and `sb` at `b` (in the metric `speed`, each at most `h`), growing geometrically
/// between them (sizes `sa · q^k` summing to the length), then each interval split in half while
/// `fine` fails between its ends.
fn graded(a: [f64; 2], b: [f64; 2], sa: f64, sb: f64, h: f64, speed: [f64; 2], fine: &dyn Fn([f64; 2], [f64; 2]) -> bool) -> Vec<f64> {
    let len = ((b[0] - a[0]) * speed[0]).hypot((b[1] - a[1]) * speed[1]);
    let cap = |x: f64| {
        if x.is_finite() && x > 0.0 {
            x.min(if h > 0.0 { h } else { f64::INFINITY })
        } else if h > 0.0 {
            h
        } else {
            len
        }
    };
    let (sa, sb) = (cap(sa), cap(sb));
    let mut cuts: Vec<f64> = Vec::new();
    if len.is_finite() && len > sa.max(sb) && sa > 0.0 && sb > 0.0 {
        let q = (len - sa) / (len - sb);
        let sizes: Vec<f64> = if (q - 1.0).abs() < 1e-3 || !(q > 0.0) || !q.is_finite() {
            // Even spacing at the mean of the two.
            let k = (len / (0.5 * (sa + sb))).round().clamp(1.0, 4096.0) as usize;
            vec![len / k as f64; k]
        } else {
            let k = (1.0 + (sb / sa).ln() / q.ln()).round().clamp(1.0, 4096.0) as usize;
            let raw: Vec<f64> = (0..k).map(|j| sa * q.powi(j as i32)).collect();
            let total: f64 = raw.iter().sum();
            raw.into_iter().map(|x| x * len / total).collect()
        };
        let mut at = 0.0;
        for s in sizes.iter().take(sizes.len().saturating_sub(1)) {
            at += s;
            cuts.push(at / len);
        }
    }
    // The size rules between seeds (sag, facet angle).
    let point = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let mut all: Vec<f64> = std::iter::once(0.0).chain(cuts).chain(std::iter::once(1.0)).collect();
    for _ in 0..12 {
        let mut next = Vec::with_capacity(all.len() * 2);
        let mut split = false;
        for w in all.windows(2) {
            let (Some(&x), Some(&y)) = (w.first(), w.get(1)) else { continue };
            next.push(x);
            if !fine(point(x), point(y)) {
                next.push(0.5 * (x + y));
                split = true;
            }
        }
        next.push(1.0);
        all = next;
        if !split || all.len() > 4096 {
            break;
        }
    }
    all.into_iter().filter(|t| *t > 0.0 && *t < 1.0).collect()
}

/// Every boundary segment (consecutive nodes of a loop) is the side of exactly one triangle:
/// the face covers its boundary with nothing missing or doubled, so it meets its neighbours
/// exactly. Pole copies of one node (consecutive in a loop, at one point) are one vertex: the
/// segment between them has no length, and a side ending on either copy is the same side.
fn closed_boundary(ranges: &[std::ops::Range<usize>], fixed: &[Vec3], triangles: &[[u32; 3]]) -> bool {
    let mut one: Vec<u32> = (0..fixed.len() as u32).collect();
    let root = |one: &[u32], mut i: u32| {
        while let Some(&j) = one.get(i as usize).filter(|j| **j != i) {
            i = j;
        }
        i
    };
    let segment = |r: &std::ops::Range<usize>, k: usize| ((r.start + k) as u32, (r.start + (k + 1) % r.len()) as u32);
    for r in ranges {
        for k in 0..r.len() {
            let (a, b) = segment(r, k);
            if fixed.get(a as usize).zip(fixed.get(b as usize)).is_some_and(|(p, q)| p == q) {
                let (ra, rb) = (root(&one, a), root(&one, b));
                if let Some(x) = one.get_mut(rb.max(ra) as usize) {
                    *x = ra.min(rb);
                }
            }
        }
    }
    let side = |a: u32, b: u32| {
        let (a, b) = (root(&one, a), root(&one, b));
        (a != b).then_some((a.min(b), a.max(b)))
    };
    let mut sides: HashMap<(u32, u32), u32> = HashMap::with_capacity(triangles.len() * 3);
    for t in triangles {
        for e in 0..3 {
            if let Some(k) = side(t[e], t[(e + 1) % 3]) {
                *sides.entry(k).or_default() += 1;
            }
        }
    }
    ranges.iter().all(|r| {
        (0..r.len()).all(|k| {
            let (a, b) = segment(r, k);
            side(a, b).is_none_or(|k| sides.get(&k).copied() == Some(1))
        })
    })
}

/// Keep at most `cap` grid points by taking every k-th line in both directions (ends kept).
fn thin(us: &mut Vec<f64>, vs: &mut Vec<f64>, cap: usize) {
    let total = us.len().saturating_mul(vs.len());
    if total <= cap {
        return;
    }
    let f = (total as f64 / cap.max(4) as f64).sqrt();
    let pick = |d: &mut Vec<f64>| {
        let n = ((d.len() as f64 / f).ceil() as usize).max(2);
        if d.len() > n {
            let len = d.len();
            *d = (0..n).filter_map(|i| d.get(i * (len - 1) / (n - 1)).copied()).collect();
        }
    };
    pick(us);
    pick(vs);
}

#[cfg(test)]
mod tests;
