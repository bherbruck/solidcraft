//! Extend (Surface › Extend): boundary edges of a surface body pushed outward across their
//! face, by a distance, up to a plane or a face, or (natural only) back to the surface's own
//! bounds. Three ways to continue past an edge:
//!
//! - natural: the face's own surface continued and the face bounded again on it. Exact for a
//!   plane and a surface of revolution (cylinder, cone, sphere, torus); a B-spline surface's end
//!   spans continue as their own polynomials. Each point of the edge moves along the parameter
//!   line that leaves the edge square to it (in space), by the arc length asked for.
//! - tangent: a ruled run-off face along the edge, in the surface's tangent plane at each point
//!   (tangent-continuous, G1).
//! - curvature: a run-off face that also keeps the surface's normal curvature across the edge
//!   (curvature-continuous, G2): each section `C + s·R + ½·κ·s²·n`.
//!
//! The corners: where two extended edges meet, both run out along the bisector of their
//! outward directions (a mitre); next to an edge that is not extended, the extension's side
//! continues that neighbour (it is lengthened to meet the extension), or with `neighbours` off
//! runs straight out square to the edge, as it does where the edges meet smoothly. Along the
//! edge the run-out direction turns evenly between its ends, keeping the distance measured
//! square to the edge. Faces of one body extended at a corner they share use one side edge
//! there when their sides coincide, so the body's boundary stays closed.
//!
//! Refused rather than built: an extension that folds over (a concave edge pushed past its
//! centre of curvature), crosses itself, wraps all the way round its surface or runs into a
//! pole of it; edges between two faces, and edges of holes.

use std::collections::HashMap;

use mt::{BoundedCurve, ParametricCurve, ParametricSurface};
use solidcraft_geom::Vec3;
use solidcraft_geom::tess::EvalSurface;
use truck_modeling as mt;

use crate::body::{Body, from_p3, p3};
use crate::face_params::{FaceLoops, iso_curve, natural_span, parameters};
use crate::record::{Rebuild, Was, rebuilt_body};
use crate::splitface::ToolSide;
use crate::surfaces::{edge_near, surface_body};
use crate::{KernelError, Result, SplitTool, guard};

/// Samples along an edge a fitted curve or run-off starts with, and the most it is refined to.
const START_SAMPLES: usize = 17;
const MAX_SAMPLES: usize = 257;
/// Most edge picks.
const MAX_PICKS: usize = 10_000;
/// The run-out direction may lean at most this far from square to its edge (its cosine; about
/// 84°): further, the extension would run nearly along the edge.
const MIN_COS: f64 = 0.1;
/// Edges whose directions agree this closely where they meet (about 2.6°) meet smoothly.
const SMOOTH_COS: f64 = 0.999;
/// Steps a march along the surface takes over the distance it is asked for, and the most it
/// takes.
const MARCH_STEPS: usize = 64;
const MAX_MARCH_STEPS: usize = 8192;
/// Bisection steps that place where a march meets its target.
const BISECT_STEPS: usize = 60;

/// How a surface continues past its edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtendMode {
    /// The surface itself, continued.
    #[default]
    Natural,
    /// A ruled run-off in the surface's tangent plane (G1).
    Tangent,
    /// A run-off following the surface's normal curvature across the edge (G2).
    Curvature,
}

/// How far an extension goes.
#[derive(Clone, Copy, Debug)]
pub enum ExtendLimit<'a> {
    /// This far from the edge, measured across the surface square to it.
    Distance(f64),
    /// To where it meets a plane, or the surface of a face (extended).
    UpTo(&'a SplitTool<'a>),
    /// Back to the surface's own bounds (natural only), undoing a trim: a free-form surface's
    /// parameter box, an analytic surface's the box of the face's own span.
    Bounds,
}

/// What [`extend_surface`] does.
#[derive(Clone, Copy, Debug)]
pub struct ExtendOptions<'a> {
    pub mode: ExtendMode,
    pub limit: ExtendLimit<'a>,
    /// Lengthen the edges next to an extended edge to meet the extension (else its sides run
    /// square to the edge).
    pub neighbours: bool,
}

/// The boundary edges of `body` picked by points on them, extended as `opts` says. A natural
/// extension rebuilds each face on its continued surface; a run-off adds a face along each
/// edge. The result is a surface body whose history is over `[body]`: an extended face is the
/// face it was (trimmed differently) and its moved edges the edges they were moved from; a
/// run-off face is made from its edge.
pub fn extend_surface(body: &Body, at: &[Vec3], opts: &ExtendOptions) -> Result<Body> {
    body.require_brep("extend")?;
    if at.is_empty() {
        return Err(KernelError::Invalid("pick the edges to extend".into()));
    }
    if at.len() > MAX_PICKS {
        return Err(KernelError::Invalid("too many edges picked".into()));
    }
    if at.iter().any(|p| !p.is_finite()) {
        return Err(KernelError::Invalid("an edge point is not finite".into()));
    }
    match opts.limit {
        ExtendLimit::Distance(d) if !(d.is_finite() && d > 0.0 && d <= 1e6) => {
            return Err(KernelError::Invalid("the extension distance must be positive".into()));
        }
        ExtendLimit::Bounds if opts.mode != ExtendMode::Natural => {
            return Err(KernelError::Invalid("only a natural extension goes back to the surface's bounds".into()));
        }
        _ => {}
    }
    // A plane's straight edges pushed out with their neighbours lengthened: the exact planar
    // extension (what designs saved before the other modes build).
    if let (ExtendMode::Natural, ExtendLimit::Distance(d), true) = (opts.mode, opts.limit, opts.neighbours)
        && plain_planar(body)
    {
        return crate::surfaces::extend(body, at, d);
    }
    let side = match opts.limit {
        ExtendLimit::UpTo(tool) => Some(ToolSide::of(tool)?),
        _ => None,
    };
    guard("extend", || Extension::new(body, opts, side).run(at))
}

/// One planar face, one loop of straight edges: what [`crate::surfaces::extend`] takes.
fn plain_planar(body: &Body) -> bool {
    let faces: Vec<&mt::Face> = body.solid.face_iter().collect();
    let [face] = faces.as_slice() else { return false };
    let bounds = face.boundaries();
    let [outer] = bounds.as_slice() else { return false };
    matches!(crate::splitface::analytic_field(&face.oriented_surface()), Some(crate::splitface::Analytic::Plane { .. }))
        && outer.edge_iter().all(|e| matches!(e.curve(), mt::Curve::Line(_)))
}

/// A point of an edge with the surface's frame there.
#[derive(Clone, Copy, Debug)]
struct Local {
    /// On the edge.
    p: Vec3,
    /// Its parameters on the surface (unwrapped along the loop).
    q: [f64; 2],
    /// The surface's unit normal (`S_u × S_v`, as the parameters run).
    n: Vec3,
    /// The edge's unit direction as the loop runs, in the tangent plane.
    d: Vec3,
    /// Unit, in the tangent plane, square to the edge, away from the face.
    out: Vec3,
    su: Vec3,
    sv: Vec3,
}

/// Where the extension of a point of an edge ends.
#[derive(Clone, Copy, Debug)]
struct Far {
    /// The edge's point and frame.
    at: Local,
    /// The run-out direction (unit, in the tangent plane) and the run-off's length along it.
    r: Vec3,
    l: f64,
    /// The surface's normal curvature along `r` (curvature mode).
    kappa: f64,
    /// The far point, and (natural) its parameters.
    f: Vec3,
    fq: [f64; 2],
}

impl Far {
    /// The run-off's section at `s` along it (tangent and curvature modes).
    fn section(&self, s: f64) -> Vec3 {
        self.at.p + self.r * s + self.at.n * (0.5 * self.kappa * s * s)
    }
}

/// A corner of the outer loop (where edge `i − 1` ends and edge `i` starts) next to an
/// extended edge: the direction the extension runs out along there, and the loop index of the
/// neighbouring edge it lengthens (when it does).
#[derive(Clone, Copy, Debug)]
struct Corner {
    r: Vec3,
    lengthens: Option<usize>,
}

/// An extended edge: its loop index, curve range and the run-out angles (from square to the
/// edge, toward its direction) at its ends.
#[derive(Clone, Copy, Debug)]
struct Run {
    i: usize,
    t0: f64,
    t1: f64,
    phi0: f64,
    phi1: f64,
}

/// A face being extended: its loops in parameters and its outer loop's edges.
struct Frame {
    face: mt::Face,
    surface: mt::Surface,
    loops: FaceLoops,
    /// +1 when the outer loop runs counter-clockwise in the parameters, else −1.
    ccw: f64,
    edges: Vec<mt::Edge>,
    chosen: Vec<bool>,
}

impl Frame {
    fn new(face: &mt::Face, ids: &[mt::EdgeID], accept: f64) -> Result<Frame> {
        let loops = FaceLoops::of(face, accept, "extending")?;
        let outer = loops.wires.get(loops.outer).ok_or_else(|| KernelError::Invalid("the face has no boundary".into()))?;
        let edges: Vec<mt::Edge> = outer.edge_iter().cloned().collect();
        let chosen: Vec<bool> = edges.iter().map(|e| ids.contains(&e.id())).collect();
        if ids.iter().any(|id| !edges.iter().any(|e| e.id() == *id)) {
            return Err(KernelError::Invalid("not supported yet: extending an edge of a hole (Untrim takes holes away)".into()));
        }
        let area = loops.loops.get(loops.outer).map_or(0.0, |l| l.area());
        if !(area.abs() > 0.0) {
            return Err(KernelError::Invalid("the face has no area on its surface".into()));
        }
        Ok(Frame { face: face.clone(), surface: face.surface(), ccw: area.signum(), loops, edges, chosen })
    }

    fn m(&self) -> usize {
        self.edges.len()
    }

    /// The outer loop's edge `i` (as the loop runs) and its samples.
    fn samples(&self, i: usize) -> Result<&(mt::Curve, Vec<(f64, Vec3, [f64; 2])>)> {
        self.loops.loops.get(self.loops.outer).and_then(|l| l.edges.get(i)).ok_or_else(|| KernelError::Failed("extend: a loop edge".into()))
    }

    /// The curve range of edge `i` (as the loop runs).
    fn range(&self, i: usize) -> Result<(f64, f64)> {
        Ok(self.samples(i)?.0.range_tuple())
    }

    /// The point of edge `i` at curve parameter `t`, with the surface's frame there.
    fn local(&self, i: usize, t: f64, accept: f64) -> Result<Local> {
        let (curve, samples) = self.samples(i)?;
        let p = from_p3(curve.subs(t));
        let hint = samples.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs())).map(|s| s.2);
        let q = parameters(&self.loops.eval, &self.loops.dom, p, hint, accept)?;
        let (_, su, sv) = self.loops.eval.derivs(q[0], q[1]).ok_or_else(|| KernelError::Failed("extend: the surface at an edge".into()))?;
        let n = normal(su, sv)?;
        let dc = curve.der(t);
        let dc = Vec3::new(dc.x, dc.y, dc.z);
        let d = (dc - n * dc.dot(n)).normalized().ok_or_else(|| KernelError::Invalid("an edge to extend has no direction".into()))?;
        Ok(Local { p, q, n, d, out: d.cross(n) * self.ccw, su, sv })
    }

    /// The point of the surface at `q`.
    fn at(&self, q: [f64; 2]) -> Result<Vec3> {
        self.loops.eval.derivs(q[0], q[1]).map(|x| x.0).ok_or_else(|| KernelError::Failed("extend: the surface can't be evaluated there".into()))
    }

    fn is_plane(&self) -> bool {
        matches!(self.loops.eval, EvalSurface::Plane { .. })
    }
}

/// The unit normal from the parameter derivatives; refused at a pole (where it has none).
fn normal(su: Vec3, sv: Vec3) -> Result<Vec3> {
    let c = su.cross(sv);
    if !(c.len() > 1e-12 * (su.len2() + sv.len2())) {
        return Err(KernelError::Invalid("the extension runs into a pole of the surface (a point with no normal)".into()));
    }
    c.normalized().ok_or_else(|| KernelError::Invalid("the extension runs into a pole of the surface".into()))
}

/// The parameter step `(a, b)` with `a·S_u + b·S_v = w` for `w` in the tangent plane, its
/// near-zero parts snapped to zero (a run along a parameter line stays on it).
fn tangent_coords(su: Vec3, sv: Vec3, w: Vec3) -> Result<[f64; 2]> {
    let (e, f, g) = (su.dot(su), su.dot(sv), sv.dot(sv));
    let det = e * g - f * f;
    if !(det.abs() > 1e-24 * (e * g).max(1e-300)) {
        return Err(KernelError::Invalid("the extension runs into a pole of the surface".into()));
    }
    let (x, y) = (w.dot(su), w.dot(sv));
    let mut ab = [(g * x - f * y) / det, (e * y - f * x) / det];
    let big = ab[0].abs().max(ab[1].abs());
    for c in &mut ab {
        if c.abs() <= 1e-12 * big {
            *c = 0.0;
        }
    }
    Ok(ab)
}

/// An extension in progress: the body, what to do, and what faces extended at shared corners
/// share.
struct Extension<'a> {
    body: &'a Body,
    opts: &'a ExtendOptions<'a>,
    side: Option<ToolSide>,
    size: f64,
    /// How far a fitted curve or run-off may stray.
    tol: f64,
    /// How far a boundary point may be from its surface.
    accept: f64,
    /// Faces using each edge.
    users: HashMap<mt::EdgeID, usize>,
    /// The far corners made at each original vertex, and the side edges run out from it.
    corners: HashMap<mt::VertexID, Vec<mt::Vertex>>,
    sides: HashMap<mt::VertexID, Vec<mt::Edge>>,
    rec: Rebuild,
}

impl<'a> Extension<'a> {
    fn new(body: &'a Body, opts: &'a ExtendOptions<'a>, side: Option<ToolSide>) -> Extension<'a> {
        let size = body.size().max(1e-6);
        let mut users: HashMap<mt::EdgeID, usize> = HashMap::new();
        for f in body.solid.face_iter() {
            for w in f.boundaries().iter() {
                for e in w.edge_iter() {
                    *users.entry(e.id()).or_default() += 1;
                }
            }
        }
        Extension {
            body,
            opts,
            side,
            size,
            tol: (size * 1e-6).max(1e-7),
            accept: (size * 1e-5).max(1e-6),
            users,
            corners: HashMap::new(),
            sides: HashMap::new(),
            rec: Rebuild::default(),
        }
    }

    fn run(mut self, at: &[Vec3]) -> Result<Body> {
        let faces: Vec<mt::Face> = self.body.solid.face_iter().cloned().collect();
        // The picked edges, by the face they bound.
        let mut picks: Vec<(usize, Vec<mt::EdgeID>)> = Vec::new();
        for p in at {
            let e = edge_near(self.body, *p).ok_or_else(|| KernelError::Invalid(format!("no edge at {p:?}")))?;
            if self.users.get(&e.id()).copied().unwrap_or(0) != 1 {
                return Err(KernelError::Invalid("an edge between two faces can't be extended: pick edges on the surface's boundary".into()));
            }
            let fi = faces
                .iter()
                .position(|f| f.boundaries().iter().any(|w| w.edge_iter().any(|x| x.id() == e.id())))
                .ok_or_else(|| KernelError::Failed("extend: the face of an edge".into()))?;
            match picks.iter_mut().find(|(f, _)| *f == fi) {
                Some((_, ids)) if !ids.contains(&e.id()) => ids.push(e.id()),
                Some(_) => {}
                None => picks.push((fi, vec![e.id()])),
            }
        }
        picks.sort_by_key(|(f, _)| *f);
        let mut out = faces.clone();
        let mut added = Vec::new();
        for (fi, ids) in &picks {
            let face = faces.get(*fi).ok_or_else(|| KernelError::Failed("extend: a face".into()))?;
            let frame = Frame::new(face, ids, self.accept)?;
            match self.opts.mode {
                ExtendMode::Natural => {
                    let extended = self.natural(&frame)?;
                    if let Some(slot) = out.get_mut(*fi) {
                        *slot = extended;
                    }
                }
                ExtendMode::Tangent | ExtendMode::Curvature => added.extend(self.run_offs(&frame)?),
            }
        }
        out.extend(added);
        Ok(rebuilt_body(surface_body(out), &[&self.body.solid], &self.rec))
    }

    /// The corners of the frame's outer loop next to an extended edge (`None` elsewhere).
    fn corners_of(&self, fr: &Frame) -> Result<Vec<Option<Corner>>> {
        let m = fr.m();
        let mut out = Vec::with_capacity(m);
        for i in 0..m {
            let pi = (i + m - 1) % m;
            let (pc, nc) = (fr.chosen.get(pi).copied().unwrap_or(false), fr.chosen.get(i).copied().unwrap_or(false));
            if !pc && !nc {
                out.push(None);
                continue;
            }
            let prev = fr.local(pi, fr.range(pi)?.1, self.accept)?;
            let next = fr.local(i, fr.range(i)?.0, self.accept)?;
            let smooth = prev.d.dot(next.d) >= SMOOTH_COS;
            let (r, lengthens) = if pc && nc {
                let r = (prev.out + next.out)
                    .normalized()
                    .ok_or_else(|| KernelError::Invalid("two extended edges turn back on each other at a corner".into()))?;
                (r, None)
            } else if pc {
                if !self.opts.neighbours || smooth { (prev.out, None) } else { (-next.d, Some(i)) }
            } else if !self.opts.neighbours || smooth {
                (next.out, None)
            } else {
                (prev.d, Some(pi))
            };
            for (chosen, l) in [(pc, &prev), (nc, &next)] {
                if chosen && r.dot(l.out) < MIN_COS {
                    return Err(KernelError::Invalid(if lengthens.is_some() {
                        "an edge next to the extended edge meets it at too sharp an angle to be lengthened (extend without lengthening the neighbouring edges)".into()
                    } else {
                        "two extended edges meet at too sharp a corner".into()
                    }));
                }
            }
            out.push(Some(Corner { r, lengthens }));
        }
        Ok(out)
    }

    /// The extended edge `i` of the frame with its run-out angles from the corners at its ends.
    fn run_of(&self, fr: &Frame, corners: &[Option<Corner>], i: usize) -> Result<Run> {
        let m = fr.m();
        let (t0, t1) = fr.range(i)?;
        let (Some(Some(c0)), Some(Some(c1))) = (corners.get(i), corners.get((i + 1) % m)) else {
            return Err(KernelError::Failed("extend: the corners of an edge".into()));
        };
        let (a, b) = (fr.local(i, t0, self.accept)?, fr.local(i, t1, self.accept)?);
        Ok(Run { i, t0, t1, phi0: c0.r.dot(a.d).atan2(c0.r.dot(a.out)), phi1: c1.r.dot(b.d).atan2(c1.r.dot(b.out)) })
    }

    /// Where the extension of edge `run` at curve parameter `t` ends.
    fn far(&self, fr: &Frame, run: &Run, t: f64) -> Result<Far> {
        let at = fr.local(run.i, t, self.accept)?;
        let w = if run.t1 != run.t0 { (t - run.t0) / (run.t1 - run.t0) } else { 0.0 };
        let phi = run.phi0 + (run.phi1 - run.phi0) * w;
        let r = at.out * phi.cos() + at.d * phi.sin();
        let reach = |cos: f64| -> f64 {
            match self.opts.limit {
                ExtendLimit::Distance(d) => d / cos.max(MIN_COS),
                _ => 0.0,
            }
        };
        let ab = tangent_coords(at.su, at.sv, r)?;
        match self.opts.mode {
            ExtendMode::Tangent | ExtendMode::Curvature => {
                let kappa = if self.opts.mode == ExtendMode::Curvature { normal_curvature(&fr.surface, at.q, ab, at.n) } else { 0.0 };
                let mut far = Far { at, r, l: 0.0, kappa, f: at.p, fq: at.q };
                far.l = match self.opts.limit {
                    ExtendLimit::Distance(_) => reach(phi.cos()),
                    ExtendLimit::UpTo(_) => self.meet(|s| Ok(far.section(s)), at.p)?,
                    ExtendLimit::Bounds => return Err(KernelError::Invalid("only a natural extension goes back to the surface's bounds".into())),
                };
                far.f = far.section(far.l);
                Ok(far)
            }
            ExtendMode::Natural => {
                let line = |s: f64| [at.q[0] + ab[0] * s, at.q[1] + ab[1] * s];
                let s = match self.opts.limit {
                    ExtendLimit::Distance(_) => arc_param(fr, at.q, ab, reach(phi.cos()))?,
                    ExtendLimit::UpTo(_) => self.meet(|s| fr.at(line(s)), at.p)?,
                    ExtendLimit::Bounds => self.to_bounds(fr, at.q, ab)?,
                };
                let fq = line(s);
                let (f, su, sv) =
                    fr.loops.eval.derivs(fq[0], fq[1]).ok_or_else(|| KernelError::Failed("extend: the surface past its edge".into()))?;
                normal(su, sv)?;
                Ok(Far { at, r, l: s, kappa: 0.0, f, fq })
            }
        }
    }

    /// Where a run `point(s)` (s ≥ 0, from `start`) first crosses the target: stepped out until
    /// it changes sides, then bisected; zero from a point on the target. Refused when the run
    /// doesn't reach it within a generous length.
    fn meet(&self, point: impl Fn(f64) -> Result<Vec3>, start: Vec3) -> Result<f64> {
        let side = self.side.as_ref().ok_or_else(|| KernelError::Failed("extend: no target".into()))?;
        let f0 = side.at(start);
        if !f0.is_finite() {
            return Err(KernelError::Invalid("the edge's side of the target can't be told".into()));
        }
        if f0.abs() <= self.tol {
            return Ok(0.0);
        }
        let h = self.size / 16.0;
        let reach = self.size * 50.0;
        let (mut s, mut prev, mut walked) = (0.0, start, 0.0);
        for _ in 0..MAX_MARCH_STEPS {
            let next = point(s + h)?;
            walked += next.dist(prev);
            let f1 = side.at(next);
            if f1.is_finite() && f1 * f0 <= 0.0 {
                let (mut a, mut b) = (s, s + h);
                for _ in 0..BISECT_STEPS {
                    let m = 0.5 * (a + b);
                    let fm = side.at(point(m)?);
                    if fm.is_finite() && fm * f0 > 0.0 { a = m } else { b = m }
                }
                return Ok(0.5 * (a + b));
            }
            if walked > reach {
                break;
            }
            (s, prev) = (s + h, next);
        }
        Err(KernelError::Invalid("the extension does not reach the target".into()))
    }

    /// How far along the parameter line `q + s·ab` the surface's own bounds are (zero from a
    /// point on them).
    fn to_bounds(&self, fr: &Frame, q: [f64; 2], ab: [f64; 2]) -> Result<f64> {
        let outer = fr.loops.loops.get(fr.loops.outer).ok_or_else(|| KernelError::Invalid("the face has no boundary".into()))?;
        let analytic = crate::offset::surf_of(&fr.face, (self.size * 1e-7).max(1e-9) * 100.0).is_some();
        let span = natural_span(&fr.loops.eval, &fr.loops.dom, outer, analytic, self.accept)?;
        let big = ab[0].abs().max(ab[1].abs());
        let mut exit = f64::INFINITY;
        for (d, (lo, hi)) in span.iter().enumerate() {
            let (Some(&x), Some(&dx)) = (q.get(d), ab.get(d)) else { continue };
            if dx.abs() <= 1e-9 * big {
                continue;
            }
            let bound = if dx > 0.0 { *hi } else { *lo };
            exit = exit.min((bound - x) / dx);
        }
        if !exit.is_finite() {
            return Err(KernelError::Invalid("the extension does not reach its surface's bounds".into()));
        }
        let scale = (span[0].1 - span[0].0).abs().max((span[1].1 - span[1].0).abs()) / big.max(1e-300);
        Ok(if exit > 1e-9 * scale { exit } else { 0.0 })
    }

    /// The extension's far ends of edge `run` at `n` even curve parameters; refused when none
    /// moves (the edge lies on its target already).
    fn fars(&self, fr: &Frame, run: &Run, n: usize) -> Result<Vec<Far>> {
        let fars: Vec<Far> = (0..n).map(|k| self.far(fr, run, run.t0 + (run.t1 - run.t0) * k as f64 / (n - 1) as f64)).collect::<Result<_>>()?;
        if fars.iter().all(|f| !self.moved(f)) {
            return Err(KernelError::Invalid(match self.opts.limit {
                ExtendLimit::Bounds => "the edge already lies on its surface's bounds".into(),
                _ => "the edge already lies on the target".into(),
            }));
        }
        Ok(fars)
    }

    /// Whether the extension moves the point at all.
    fn moved(&self, f: &Far) -> bool {
        f.f.dist(f.at.p) > self.tol * 10.0
    }

    /// The far corner vertex at `v` (an original vertex) for the point `p`: one made there
    /// before (by this face or a neighbour) when it is at the same place.
    fn corner_vertex(&mut self, v: &mt::Vertex, p: Vec3) -> mt::Vertex {
        let near = self.tol * 10.0;
        if from_p3(v.point()).dist(p) <= near {
            return v.clone();
        }
        let list = self.corners.entry(v.id()).or_default();
        if let Some(found) = list.iter().find(|x| from_p3(x.point()).dist(p) <= near) {
            return found.clone();
        }
        let made = mt::Vertex::new(p3(p));
        list.push(made.clone());
        made
    }

    /// The side edge from the original vertex `v` to the far corner `f`, through `mid`: one made
    /// before (by this face or a neighbour) along the same path, else `make`'s curve.
    fn side_edge(&mut self, v: &mt::Vertex, f: &mt::Vertex, mid: Vec3, make: impl FnOnce() -> Result<mt::Curve>) -> Result<mt::Edge> {
        let near = self.tol * 10.0;
        if let Some(found) = self.sides.get(&v.id()).and_then(|list| {
            list.iter().find(|e| {
                let c = e.curve();
                let (t0, t1) = c.range_tuple();
                e.back().id() == f.id() && from_p3(c.subs(0.5 * (t0 + t1))).dist(mid) <= near * 10.0
            })
        }) {
            return Ok(found.clone());
        }
        let e = mt::Edge::try_new(v, f, make()?).map_err(|e| KernelError::Failed(format!("extend: a side edge: {e}")))?;
        self.sides.entry(v.id()).or_default().push(e.clone());
        Ok(e)
    }

    /// The face extended naturally: its surface continued and its outer loop bounded anew.
    fn natural(&mut self, fr: &Frame) -> Result<mt::Face> {
        let m = fr.m();
        let corners = self.corners_of(fr)?;
        let chosen = |i: usize| fr.chosen.get(i).copied().unwrap_or(false);
        let runs: Vec<Option<Run>> =
            (0..m).map(|i| if chosen(i) { self.run_of(fr, &corners, i).map(Some) } else { Ok(None) }).collect::<Result<_>>()?;
        // The far end at each corner next to an extended edge (from that edge's run).
        let far_at: Vec<Option<Far>> = (0..m)
            .map(|i| match (runs.get(i).copied().flatten(), runs.get((i + m - 1) % m).copied().flatten()) {
                (Some(r), _) => self.far(fr, &r, r.t0).map(Some),
                (None, Some(r)) => self.far(fr, &r, r.t1).map(Some),
                (None, None) => Ok(None),
            })
            .collect::<Result<_>>()?;
        // The new loop, piece by piece, in the old parameters; no side where a corner stays.
        let moves = |i: usize| far_at.get(i).copied().flatten().is_some_and(|f| self.moved(&f));
        let mut pieces: Vec<Piece> = Vec::new();
        for i in 0..m {
            let ni = (i + 1) % m;
            if chosen(i) {
                if !chosen((i + m - 1) % m) && moves(i) {
                    pieces.push(Piece::Side { corner: i, out: true });
                }
                pieces.push(Piece::Far(i));
                if !chosen(ni) && moves(ni) {
                    pieces.push(Piece::Side { corner: ni, out: false });
                }
            } else {
                pieces.push(Piece::Keep(i));
            }
        }
        // A kept edge on a parameter line, lengthened by the side next to it along that line,
        // becomes one edge (unless another face shares it).
        let mergeable = |i: usize| -> bool {
            let Some(e) = fr.edges.get(i) else { return false };
            self.users.get(&e.id()).copied().unwrap_or(0) == 1 && straight(fr, i)
        };
        let lengthens = |corner: usize, edge: usize| corners.get(corner).copied().flatten().and_then(|c| c.lengthens) == Some(edge);
        let n = pieces.len();
        let mut dropped = vec![false; n];
        let mut merged: Vec<Option<Piece>> = vec![None; n];
        for (k, piece) in pieces.iter().enumerate() {
            let Piece::Keep(i) = *piece else { continue };
            let (pk, nk) = ((k + n - 1) % n, (k + 1) % n);
            if pk == k || !mergeable(i) {
                continue;
            }
            let before = matches!(pieces.get(pk), Some(Piece::Side { corner, out: false }) if lengthens(*corner, i));
            let after = matches!(pieces.get(nk), Some(Piece::Side { corner, out: true }) if lengthens(*corner, i));
            if before || after {
                for (at, yes) in [(pk, before), (nk, after)] {
                    if let Some(d) = dropped.get_mut(at) {
                        *d |= yes;
                    }
                }
                if let Some(slot) = merged.get_mut(k) {
                    *slot = Some(Piece::Merged { edge: i, from_far: before, to_far: after });
                }
            }
        }
        let pieces: Vec<Piece> = pieces
            .iter()
            .enumerate()
            .filter(|(k, _)| !dropped.get(*k).copied().unwrap_or(false))
            .map(|(k, p)| merged.get(k).copied().flatten().unwrap_or(*p))
            .collect();
        // Far samples of each extended edge (refined later where a curve is fitted).
        let samples: Vec<Option<Vec<Far>>> = runs
            .iter()
            .map(|r| match r {
                Some(r) => {
                    let fars = self.fars(fr, r, START_SAMPLES)?;
                    check_no_fold(&fars)?;
                    Ok(Some(fars))
                }
                None => Ok(None),
            })
            .collect::<Result<_>>()?;
        // The new loop as a polygon in the old parameters: checked before anything is built.
        let corner_q = |i: usize| -> Result<[f64; 2]> {
            far_at.get(i).copied().flatten().map(|f| f.fq).ok_or_else(|| KernelError::Failed("extend: a corner".into()))
        };
        let vertex_q = |i: usize| -> Result<[f64; 2]> {
            fr.samples(i).and_then(|s| s.1.first().map(|x| x.2).ok_or_else(|| KernelError::Failed("extend: a vertex".into())))
        };
        let mut poly: Vec<[f64; 2]> = Vec::new();
        for p in &pieces {
            match *p {
                Piece::Keep(i) => {
                    let s = &fr.samples(i)?.1;
                    poly.extend(s.iter().take(s.len().saturating_sub(1)).map(|x| x.2));
                }
                Piece::Side { corner, out } => poly.push(if out { vertex_q(corner)? } else { corner_q(corner)? }),
                Piece::Far(i) => {
                    let s = samples.get(i).and_then(|s| s.as_ref()).ok_or_else(|| KernelError::Failed("extend: far samples".into()))?;
                    poly.extend(s.iter().take(s.len().saturating_sub(1)).map(|f| f.fq));
                }
                Piece::Merged { edge, from_far, .. } => poly.push(if from_far { corner_q(edge)? } else { vertex_q(edge)? }),
            }
        }
        check_loop(fr, &poly)?;
        // The surface continued over the new loop.
        let ext = continued(&fr.surface, &fr.loops.eval, &bounds_of(&poly))?;
        // The edges.
        let mut wire: Vec<mt::Edge> = Vec::with_capacity(pieces.len());
        for p in &pieces {
            match *p {
                Piece::Keep(i) => wire.push(fr.edges.get(i).cloned().ok_or_else(|| KernelError::Failed("extend: an edge".into()))?),
                Piece::Side { corner, out } => {
                    let far = far_at.get(corner).copied().flatten().ok_or_else(|| KernelError::Failed("extend: a corner".into()))?;
                    let v = fr.edges.get(corner).map(|e| e.front().clone()).ok_or_else(|| KernelError::Failed("extend: a vertex".into()))?;
                    let fv = self.corner_vertex(&v, far.f);
                    let (a, b) = (far.at.q, far.fq);
                    let mid = fr.at([0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])])?;
                    let tol = self.tol;
                    let e = self.side_edge(&v, &fv, mid, || uv_line(fr, &ext, a, b, tol))?;
                    let lengthened = corners.get(corner).copied().flatten().and_then(|c| c.lengthens).and_then(|i| fr.edges.get(i));
                    if let Some(old) = lengthened {
                        self.rec.piece(&e, old);
                    }
                    wire.push(if out { e } else { e.inverse() });
                }
                Piece::Far(i) => {
                    let run = runs.get(i).copied().flatten().ok_or_else(|| KernelError::Failed("extend: a run".into()))?;
                    let old = fr.edges.get(i).ok_or_else(|| KernelError::Failed("extend: an edge".into()))?;
                    let (Some(a), Some(b)) = (far_at.get(i).copied().flatten(), far_at.get((i + 1) % m).copied().flatten()) else {
                        return Err(KernelError::Failed("extend: the corners of an edge".into()));
                    };
                    let va = self.corner_vertex(old.front(), a.f);
                    let vb = self.corner_vertex(old.back(), b.f);
                    let first = samples.get(i).and_then(|s| s.as_ref()).ok_or_else(|| KernelError::Failed("extend: far samples".into()))?;
                    let curve = self.far_curve(fr, &ext, &run, first)?;
                    let e = mt::Edge::try_new(&va, &vb, curve).map_err(|e| KernelError::Failed(format!("extend: a moved edge: {e}")))?;
                    self.rec.piece(&e, old);
                    wire.push(e);
                }
                Piece::Merged { edge, from_far, to_far } => {
                    let old = fr.edges.get(edge).ok_or_else(|| KernelError::Failed("extend: an edge".into()))?;
                    let (a, va) = if from_far {
                        let f = far_at.get(edge).copied().flatten().ok_or_else(|| KernelError::Failed("extend: a corner".into()))?;
                        (f.fq, self.corner_vertex(old.front(), f.f))
                    } else {
                        (vertex_q(edge)?, old.front().clone())
                    };
                    let (b, vb) = if to_far {
                        let f = far_at.get((edge + 1) % m).copied().flatten().ok_or_else(|| KernelError::Failed("extend: a corner".into()))?;
                        (f.fq, self.corner_vertex(old.back(), f.f))
                    } else {
                        (vertex_q((edge + 1) % m)?, old.back().clone())
                    };
                    let b = unwrap_near(fr, b, a);
                    let curve = uv_line(fr, &ext, a, b, self.tol)?;
                    let e = mt::Edge::try_new(&va, &vb, curve).map_err(|e| KernelError::Failed(format!("extend: a lengthened edge: {e}")))?;
                    self.rec.piece(&e, old);
                    wire.push(e);
                }
            }
        }
        let holes = fr.loops.wires.iter().enumerate().filter(|(k, _)| *k != fr.loops.outer).map(|(_, w)| w.clone());
        let boundaries: Vec<mt::Wire> = std::iter::once(mt::Wire::from(wire)).chain(holes).collect();
        let mut f = mt::Face::try_new(boundaries, ext.surface).map_err(|e| KernelError::Failed(format!("extended face: {e}")))?;
        if !fr.face.orientation() {
            f.invert();
        }
        self.rec.face(&f, &fr.face);
        Ok(f)
    }

    /// The far edge of a natural extension: exact on a parameter line (and straight on a plane),
    /// else a cubic through the far points, refined until it follows them within the tolerance.
    fn far_curve(&self, fr: &Frame, ext: &Continued, run: &Run, first: &[Far]) -> Result<mt::Curve> {
        let (Some(a), Some(b)) = (first.first(), first.last()) else { return Err(KernelError::Failed("extend: far samples".into())) };
        if let Some(along_u) = iso(first.iter().map(|f| f.fq)) {
            let (w, from, to) = if along_u { (a.fq[1], a.fq[0], b.fq[0]) } else { (a.fq[0], a.fq[1], b.fq[1]) };
            return ext.iso(along_u, w, from, to);
        }
        if fr.is_plane() && first.iter().all(|f| f.f.dist_to_segment(a.f, b.f) <= self.tol) {
            return Ok(mt::Curve::Line(mt::Line(p3(a.f), p3(b.f))));
        }
        self.fit(|t| self.far(fr, run, t).map(|f| f.f), run.t0, run.t1)
    }

    /// A cubic through `point(t)` at even parameters from `t0` to `t1`, refined until it passes
    /// within the tolerance of the points between its samples.
    fn fit(&self, point: impl Fn(f64) -> Result<Vec3>, t0: f64, t1: f64) -> Result<mt::Curve> {
        let mut n = START_SAMPLES;
        loop {
            let at = |k: f64| t0 + (t1 - t0) * k / (n - 1) as f64;
            let pts: Vec<Vec3> = (0..n).map(|k| point(at(k as f64))).collect::<Result<_>>()?;
            let c = crate::build::interpolate_cubic(&pts).ok_or_else(|| KernelError::Failed("extend: fitting an edge".into()))?;
            let mut worst = 0.0f64;
            for k in 0..n - 1 {
                let u = (k as f64 + 0.5) / (n - 1) as f64;
                worst = worst.max(from_p3(c.subs(u)).dist(point(at(k as f64 + 0.5))?));
            }
            if worst <= self.tol {
                return Ok(mt::Curve::BSplineCurve(c));
            }
            if n >= MAX_SAMPLES {
                return Err(KernelError::Invalid(format!("the extended edge can't be fitted closely enough (off by up to {worst:.6} mm)")));
            }
            n = 2 * n - 1;
        }
    }

    /// The run-off faces of a tangent or curvature extension, one along each extended edge.
    fn run_offs(&mut self, fr: &Frame) -> Result<Vec<mt::Face>> {
        let m = fr.m();
        let corners = self.corners_of(fr)?;
        let mut out = Vec::new();
        for i in 0..m {
            if !fr.chosen.get(i).copied().unwrap_or(false) {
                continue;
            }
            let run = self.run_of(fr, &corners, i)?;
            let edge = fr.edges.get(i).cloned().ok_or_else(|| KernelError::Failed("extend: an edge".into()))?;
            let (surface, fars, far_curve) = self.run_off_surface(fr, &run)?;
            let (Some(a), Some(b)) = (fars.first().copied(), fars.last().copied()) else {
                return Err(KernelError::Failed("extend: run-off samples".into()));
            };
            if !self.moved(&a) || !self.moved(&b) {
                return Err(KernelError::Invalid("not supported yet: a run-off that ends at a corner already on its target".into()));
            }
            let (fa, fb) = (self.corner_vertex(edge.front(), a.f), self.corner_vertex(edge.back(), b.f));
            let curvature = self.opts.mode == ExtendMode::Curvature;
            let side = |f: Far| -> mt::Curve {
                if curvature {
                    let pts = vec![p3(f.at.p), p3(f.at.p + f.r * (0.5 * f.l)), p3(f.f)];
                    mt::Curve::BSplineCurve(mt::BSplineCurve::new(mt::KnotVec::bezier_knot(2), pts))
                } else {
                    mt::Curve::Line(mt::Line(p3(f.at.p), p3(f.f)))
                }
            };
            let start = self.side_edge(edge.front(), &fa, a.section(0.5 * a.l), || Ok(side(a)))?;
            let end = self.side_edge(edge.back(), &fb, b.section(0.5 * b.l), || Ok(side(b)))?;
            let far = mt::Edge::try_new(&fa, &fb, mt::Curve::BSplineCurve(far_curve))
                .map_err(|e| KernelError::Failed(format!("extend: a run-off edge: {e}")))?;
            let wire: mt::Wire = vec![edge.clone(), end, far.inverse(), start.inverse()].into();
            let mut f = mt::Face::try_new(vec![wire], mt::Surface::BSplineSurface(surface.clone()))
                .map_err(|e| KernelError::Failed(format!("run-off face: {e}")))?;
            // Facing the way the face it runs off faces.
            let mid = self.far(fr, &run, 0.5 * (run.t0 + run.t1))?;
            let facing = mid.at.n * if fr.face.orientation() { 1.0 } else { -1.0 };
            let own = surface.uder(0.5, 0.0).cross(surface.vder(0.5, 0.0));
            if Vec3::new(own.x, own.y, own.z).dot(facing) < 0.0 {
                f.invert();
            }
            self.rec.made(&f, vec![Was::Edge(edge.id())]);
            out.push(f);
        }
        Ok(out)
    }

    /// The run-off along edge `run`: degree 1 (tangent) or 2 (curvature) across it, a cubic
    /// through its sections along it, refined until it is within the tolerance of its exact
    /// sections (its near side of the edge itself) between the samples; and its samples.
    fn run_off_surface(&self, fr: &Frame, run: &Run) -> Result<(mt::BSplineSurface<mt::Point3>, Vec<Far>, mt::BSplineCurve<mt::Point3>)> {
        let curvature = self.opts.mode == ExtendMode::Curvature;
        let mut n = START_SAMPLES;
        loop {
            let fars = self.fars(fr, run, n)?;
            check_no_fold(&fars)?;
            let rows: Vec<Vec<Vec3>> = if curvature {
                vec![
                    fars.iter().map(|f| f.at.p).collect(),
                    fars.iter().map(|f| f.at.p + f.r * (0.5 * f.l)).collect(),
                    fars.iter().map(|f| f.f).collect(),
                ]
            } else {
                vec![fars.iter().map(|f| f.at.p).collect(), fars.iter().map(|f| f.f).collect()]
            };
            let curves: Vec<mt::BSplineCurve<mt::Point3>> = rows
                .iter()
                .map(|r| crate::build::interpolate_cubic(r))
                .collect::<Option<_>>()
                .ok_or_else(|| KernelError::Failed("extend: fitting a run-off".into()))?;
            let ku = curves.first().map(|c| c.knot_vec().clone()).ok_or_else(|| KernelError::Failed("extend: a run-off".into()))?;
            let count = curves.first().map_or(0, |c| c.control_points().len());
            let net: Vec<Vec<mt::Point3>> = (0..count).map(|k| curves.iter().filter_map(|c| c.control_points().get(k).copied()).collect()).collect();
            let surface = mt::BSplineSurface::try_new((ku, mt::KnotVec::bezier_knot(rows.len() - 1)), net)
                .map_err(|e| KernelError::Failed(format!("extend: a run-off surface: {e}")))?;
            let mut worst = 0.0f64;
            for k in 0..n - 1 {
                let u = (k as f64 + 0.5) / (n - 1) as f64;
                let exact = self.far(fr, run, run.t0 + (run.t1 - run.t0) * u)?;
                for sigma in [0.0, 0.5, 1.0] {
                    let want = exact.section(sigma * exact.l);
                    worst = worst.max(from_p3(surface.subs(u, sigma)).dist(want));
                }
            }
            if worst <= self.tol {
                check_regular(&surface, n)?;
                let far = curves.last().cloned().ok_or_else(|| KernelError::Failed("extend: a run-off".into()))?;
                return Ok((surface, fars, far));
            }
            if n >= MAX_SAMPLES {
                return Err(KernelError::Invalid(format!("the run-off can't be fitted closely enough (off by up to {worst:.6} mm)")));
            }
            n = 2 * n - 1;
        }
    }
}

/// A piece of a naturally extended face's new outer loop.
#[derive(Clone, Copy, Debug)]
enum Piece {
    /// Edge `i` of the old loop, kept.
    Keep(usize),
    /// The side from corner `corner`'s original vertex out to its far corner (`out`), or back.
    Side { corner: usize, out: bool },
    /// Edge `i` moved out.
    Far(usize),
    /// Edge `edge` lengthened along its parameter line, from its start's far corner
    /// (`from_far`) or vertex, to its end's far corner (`to_far`) or vertex.
    Merged { edge: usize, from_far: bool, to_far: bool },
}

/// Whether edge `i` of the frame runs straight in the surface's parameters: along a parameter
/// line, or a straight line on a plane.
fn straight(fr: &Frame, i: usize) -> bool {
    let Ok((curve, samples)) = fr.samples(i) else { return false };
    if fr.is_plane() {
        return matches!(curve, mt::Curve::Line(_));
    }
    iso(samples.iter().map(|s| s.2)).is_some()
}

/// Whether the parameters all lie on one parameter line: `Some(true)` along u (v the same),
/// `Some(false)` along v.
fn iso(qs: impl Iterator<Item = [f64; 2]> + Clone) -> Option<bool> {
    let span = |d: usize| -> (f64, f64) { qs.clone().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| (lo.min(q[d]), hi.max(q[d]))) };
    let (su, sv) = (span(0), span(1));
    let (wu, wv) = (su.1 - su.0, sv.1 - sv.0);
    let scale = wu.max(wv);
    if !(scale > 0.0) {
        return None;
    }
    let eps = 1e-9 * scale;
    if wv <= eps && wu > eps {
        Some(true)
    } else if wu <= eps && wv > eps {
        Some(false)
    } else {
        None
    }
}

/// `b` moved by whole periods to lie near `a`.
fn unwrap_near(fr: &Frame, b: [f64; 2], a: [f64; 2]) -> [f64; 2] {
    [0, 1].map(|d| crate::tess::uv::unwrap(b[d], a[d], fr.loops.dom.period[d]))
}

/// The curve along the straight parameter segment `a`–`b` (old parameters) on the continued
/// surface: exact along a parameter line or on a plane, else a fitted cubic.
fn uv_line(fr: &Frame, ext: &Continued, a: [f64; 2], b: [f64; 2], tol: f64) -> Result<mt::Curve> {
    let dir = [b[0] - a[0], b[1] - a[1]];
    let big = dir[0].abs().max(dir[1].abs());
    if fr.is_plane() {
        return Ok(mt::Curve::Line(mt::Line(p3(fr.at(a)?), p3(fr.at(b)?))));
    }
    if dir[1].abs() <= 1e-9 * big {
        return ext.iso(true, a[1], a[0], b[0]);
    }
    if dir[0].abs() <= 1e-9 * big {
        return ext.iso(false, a[0], a[1], b[1]);
    }
    let point = |t: f64| fr.at([a[0] + dir[0] * t, a[1] + dir[1] * t]);
    let mut n = START_SAMPLES;
    loop {
        let pts: Vec<Vec3> = (0..n).map(|k| point(k as f64 / (n - 1) as f64)).collect::<Result<_>>()?;
        let c = crate::build::interpolate_cubic(&pts).ok_or_else(|| KernelError::Failed("extend: fitting a side edge".into()))?;
        let mut worst = 0.0f64;
        for k in 0..n - 1 {
            let u = (k as f64 + 0.5) / (n - 1) as f64;
            worst = worst.max(from_p3(c.subs(u)).dist(point(u)?));
        }
        if worst <= tol {
            return Ok(mt::Curve::BSplineCurve(c));
        }
        if n >= MAX_SAMPLES {
            return Err(KernelError::Invalid(format!("a side of the extension can't be fitted closely enough (off by up to {worst:.6} mm)")));
        }
        n = 2 * n - 1;
    }
}

/// The parameter range `q + s·ab` covers from `q` for arc length `length` on the surface
/// (Simpson's rule over even steps, the last step bisected).
fn arc_param(fr: &Frame, q: [f64; 2], ab: [f64; 2], length: f64) -> Result<f64> {
    let speed = |s: f64| -> Result<f64> {
        let (_, su, sv) = fr
            .loops
            .eval
            .derivs(q[0] + ab[0] * s, q[1] + ab[1] * s)
            .ok_or_else(|| KernelError::Failed("extend: the surface past its edge".into()))?;
        normal(su, sv)?;
        Ok((su * ab[0] + sv * ab[1]).len())
    };
    // |S_u a + S_v b| = 1 at the start: steps of the length's share.
    let h = length / MARCH_STEPS as f64;
    let (mut s, mut walked, mut v0) = (0.0, 0.0, speed(0.0)?);
    for _ in 0..MAX_MARCH_STEPS {
        let (vm, v1) = (speed(s + 0.5 * h)?, speed(s + h)?);
        let step = h / 6.0 * (v0 + 4.0 * vm + v1);
        if walked + step >= length {
            let part = |x: f64| -> Result<f64> { Ok(x / 6.0 * (v0 + 4.0 * speed(s + 0.5 * x)? + speed(s + x)?)) };
            let (mut a, mut b) = (0.0, h);
            for _ in 0..BISECT_STEPS {
                let mid = 0.5 * (a + b);
                if walked + part(mid)? < length { a = mid } else { b = mid }
            }
            return Ok(s + 0.5 * (a + b));
        }
        (s, walked, v0) = (s + h, walked + step, v1);
    }
    Err(KernelError::Invalid("the extension runs off its surface".into()))
}

/// The surface's normal curvature along the tangent `a·S_u + b·S_v` (unit) at `q`, signed
/// toward `n`.
fn normal_curvature(s: &mt::Surface, q: [f64; 2], ab: [f64; 2], n: Vec3) -> f64 {
    let (uu, uv, vv) = (s.uuder(q[0], q[1]), s.uvder(q[0], q[1]), s.vvder(q[0], q[1]));
    let second = uu * (ab[0] * ab[0]) + uv * (2.0 * ab[0] * ab[1]) + vv * (ab[1] * ab[1]);
    let k = Vec3::new(second.x, second.y, second.z).dot(n);
    if k.is_finite() { k } else { 0.0 }
}

/// Refused where the far ends run back against the edge: the extension folds over (a concave
/// edge pushed past its centre of curvature).
fn check_no_fold(fars: &[Far]) -> Result<()> {
    for w in fars.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        if (b.f - a.f).dot(b.at.p - a.at.p) <= 0.0 {
            return Err(KernelError::Invalid(
                "the extension folds over itself (a concave edge pushed past its centre of curvature): extend it less".into(),
            ));
        }
    }
    Ok(())
}

/// Refused where a run-off's normal turns over anywhere across it: it folds.
fn check_regular(s: &mt::BSplineSurface<mt::Point3>, n: usize) -> Result<()> {
    for k in 0..n {
        let u = k as f64 / (n - 1) as f64;
        let base = s.uder(u, 0.0).cross(s.vder(u, 0.0));
        for j in 1..=4 {
            let v = j as f64 / 4.0;
            let here = s.uder(u, v).cross(s.vder(u, v));
            if !(here.x * base.x + here.y * base.y + here.z * base.z > 0.0) {
                return Err(KernelError::Invalid("the extension folds over itself: extend it less".into()));
            }
        }
    }
    Ok(())
}

/// Refused where the new outer loop (a polygon in the old parameters) crosses itself or the
/// face's holes, or spans a whole turn of a periodic surface.
fn check_loop(fr: &Frame, poly: &[[f64; 2]]) -> Result<()> {
    for d in 0..2 {
        if let Some(p) = fr.loops.dom.period.get(d).copied().flatten() {
            let (lo, hi) = poly.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| (lo.min(q[d]), hi.max(q[d])));
            if hi - lo >= p * (1.0 - 1e-9) {
                return Err(KernelError::Invalid("the extension would wrap all the way round its surface onto itself".into()));
            }
        }
    }
    let segs = |pts: &[[f64; 2]]| -> Vec<([f64; 2], [f64; 2])> {
        let n = pts.len();
        (0..n).filter_map(|i| Some((*pts.get(i)?, *pts.get((i + 1) % n)?))).collect()
    };
    let outer = segs(poly);
    let n = outer.len();
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            if let (Some(a), Some(b)) = (outer.get(i), outer.get(j))
                && crosses(*a, *b)
            {
                return Err(KernelError::Invalid("the extension crosses itself".into()));
            }
        }
    }
    for (k, lp) in fr.loops.loops.iter().enumerate() {
        if k == fr.loops.outer {
            continue;
        }
        for h in segs(&lp.polygon()) {
            if outer.iter().any(|o| crosses(*o, h)) {
                return Err(KernelError::Invalid("the extension crosses a hole of its face".into()));
            }
        }
    }
    Ok(())
}

/// Whether two segments properly cross (touching ends don't).
fn crosses((a, b): ([f64; 2], [f64; 2]), (c, d): ([f64; 2], [f64; 2])) -> bool {
    let orient = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let scale = [a, b, c, d].iter().map(|p| p[0].abs().max(p[1].abs())).fold(1e-300, f64::max);
    let eps = 1e-12 * scale * scale;
    let (o1, o2, o3, o4) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
    ((o1 > eps && o2 < -eps) || (o1 < -eps && o2 > eps)) && ((o3 > eps && o4 < -eps) || (o3 < -eps && o4 > eps))
}

/// The parameter box of the points.
fn bounds_of(poly: &[[f64; 2]]) -> [(f64, f64); 2] {
    let span = |d: usize| poly.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| (lo.min(q[d]), hi.max(q[d])));
    [span(0), span(1)]
}

/// A face's surface continued to cover a parameter box, and how its parameters relate to the
/// old ones (`new = old · scale + offset` per direction).
struct Continued {
    surface: mt::Surface,
    map: [(f64, f64); 2],
}

impl Continued {
    fn to_new(&self, d: usize, x: f64) -> f64 {
        let (scale, offset) = self.map.get(d).copied().unwrap_or((1.0, 0.0));
        x * scale + offset
    }

    /// The exact parameter line at `w` (old parameters; v = w when `along_u`) from `from` to `to`.
    fn iso(&self, along_u: bool, w: f64, from: f64, to: f64) -> Result<mt::Curve> {
        let (dw, dt) = if along_u { (1, 0) } else { (0, 1) };
        iso_curve(&self.surface, along_u, self.to_new(dw, w), self.to_new(dt, from), self.to_new(dt, to))
    }
}

/// `surface` continued to cover `need` (old parameters, with a margin): a B-spline surface's
/// end spans continued as their own polynomials, a surface of revolution's profile continued
/// (a line lengthened, a B-spline profile's end spans continued); a plane as it is. Exact on the
/// old surface.
fn continued(surface: &mt::Surface, eval: &EvalSurface, need: &[(f64, f64); 2]) -> Result<Continued> {
    let identity = [(1.0, 0.0), (1.0, 0.0)];
    let dom = crate::tess::uv::domain(eval);
    // The range each direction needs: `None` where the surface's own covers it.
    let wanted = |d: usize| -> Option<(f64, f64)> {
        let (lo, hi) = need.get(d).copied()?;
        let (a, b) = dom.range.get(d).copied().flatten()?;
        if dom.period.get(d).copied().flatten().is_some() {
            return None;
        }
        let margin = 0.01 * (hi - lo).max(b - a);
        let eps = 1e-12 * (b - a).abs().max(1e-300);
        (lo < a - eps || hi > b + eps)
            .then_some((lo.min(a) - if lo < a - eps { margin } else { 0.0 }, hi.max(b) + if hi > b + eps { margin } else { 0.0 }))
    };
    let unsupported = || KernelError::Invalid("not supported yet: continuing this kind of surface past its bounds".into());
    match surface {
        mt::Surface::Plane(_) => Ok(Continued { surface: surface.clone(), map: identity }),
        mt::Surface::BSplineSurface(_) | mt::Surface::NurbsSurface(_) => {
            let (Some(EvalSurface::Nurbs { degree, knots, count, ctrl, .. }), rational) =
                (Some(eval), matches!(surface, mt::Surface::NurbsSurface(_)))
            else {
                return Err(unsupported());
            };
            let [nu, nv] = *count;
            // rows[i][j]: control point i along u, j along v.
            let mut rows: Vec<Vec<[f64; 4]>> = (0..nu).map(|i| (0..nv).filter_map(|j| ctrl.get(j * nu + i).copied()).collect()).collect();
            let [mut ku, mut kv] = knots.clone();
            if let Some(range) = wanted(0) {
                (ku, rows) = continue_span(&ku, degree[0], &rows, range).ok_or_else(unsupported)?;
            }
            if let Some(range) = wanted(1) {
                let cols = transpose(&rows);
                let (k, cols) = continue_span(&kv, degree[1], &cols, range).ok_or_else(unsupported)?;
                kv = k;
                rows = transpose(&cols);
            }
            Ok(Continued { surface: nurbs_surface(ku, kv, rows, rational)?, map: identity })
        }
        mt::Surface::RevolutedCurve(proc) => {
            let pd = if proc.orientation() { 0 } else { 1 };
            let Some((lo, hi)) = wanted(pd) else { return Ok(Continued { surface: surface.clone(), map: identity }) };
            let rev = proc.entity();
            let (profile, map) = match rev.entity_curve() {
                mt::Curve::Line(l) => {
                    let at = |t: f64| l.0 + (l.1 - l.0) * t;
                    let mut map = identity;
                    if let Some(slot) = map.get_mut(pd) {
                        *slot = (1.0 / (hi - lo), -lo / (hi - lo));
                    }
                    (mt::Curve::Line(mt::Line(at(lo), at(hi))), map)
                }
                c @ (mt::Curve::BSplineCurve(_) | mt::Curve::NurbsCurve(_)) => {
                    let ec = crate::tess::surface::eval_curve(c).ok_or_else(unsupported)?;
                    let rows: Vec<Vec<[f64; 4]>> = ec.ctrl.iter().map(|p| vec![*p]).collect();
                    let (k, rows) = continue_span(&ec.knots, ec.degree, &rows, (lo, hi)).ok_or_else(unsupported)?;
                    (nurbs_curve(k, rows.iter().filter_map(|r| r.first().copied()).collect(), matches!(c, mt::Curve::NurbsCurve(_)))?, identity)
                }
                mt::Curve::IntersectionCurve(_) => return Err(unsupported()),
            };
            let turned = proc.map_ref(|r| mt::RevolutedCurve::by_revolution(profile, r.origin(), r.axis()));
            Ok(Continued { surface: mt::Surface::RevolutedCurve(turned), map })
        }
    }
}

fn transpose(rows: &[Vec<[f64; 4]>]) -> Vec<Vec<[f64; 4]>> {
    let width = rows.first().map_or(0, Vec::len);
    (0..width).map(|j| rows.iter().filter_map(|r| r.get(j).copied()).collect()).collect()
}

/// `a + (b − a)·t` on homogeneous points.
fn lerp4(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * t)
}

/// A clamped B-spline (degree `p`, control `rows[i]` the points at index `i` along the
/// direction, across the other) continued to cover `(lo, hi)`: each end span's polynomial over
/// a longer span. `None` for one that is not clamped or malformed, or whose weights would not
/// stay positive.
fn continue_span(knots: &[f64], p: usize, rows: &[Vec<[f64; 4]>], (lo, hi): (f64, f64)) -> Option<(Vec<f64>, Vec<Vec<[f64; 4]>>)> {
    let (k, c) = continue_end(knots, p, rows, hi)?;
    // The start: the same on the curve run backwards.
    let k: Vec<f64> = k.iter().rev().map(|x| -x).collect();
    let c: Vec<Vec<[f64; 4]>> = c.into_iter().rev().collect();
    let (k, c) = continue_end(&k, p, &c, -lo)?;
    let k: Vec<f64> = k.iter().rev().map(|x| -x).collect();
    let c: Vec<Vec<[f64; 4]>> = c.into_iter().rev().collect();
    c.iter().flatten().all(|x| x[3] > 0.0 && x.iter().all(|v| v.is_finite())).then_some((k, c))
}

/// [`continue_span`] at the end only: the last knot `a` below the end `b` repeated to the
/// degree (Boehm's knot insertion; the B-spline is unchanged), which leaves the last span's
/// own Bézier points; then de Casteljau's subdivision at `(to − a)/(b − a) > 1` gives the
/// Bézier points of the same polynomial over `a…to` (NURBS Book §5.2, A5.1; §1.4).
fn continue_end(knots: &[f64], p: usize, rows: &[Vec<[f64; 4]>], to: f64) -> Option<(Vec<f64>, Vec<Vec<[f64; 4]>>)> {
    let mut k = knots.to_vec();
    let mut c = rows.to_vec();
    if p == 0 || c.len() <= p || k.len() != c.len() + p + 1 || k.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let b = *k.last()?;
    if !(to > b) {
        return Some((k, c));
    }
    if k.iter().rev().take(p + 1).any(|x| *x != b) || !(*k.get(c.len() - 1)? < b) {
        return None;
    }
    let a = *k.get(c.len() - 1)?;
    while k.iter().filter(|x| **x == a).count() < p {
        let n = c.len();
        let span = n - 1;
        let mut next = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let row = if i + p <= span {
                c.get(i)?.clone()
            } else if i > span {
                c.get(i - 1)?.clone()
            } else {
                let (ui, uip) = (*k.get(i)?, *k.get(i + p)?);
                let alpha = if uip > ui { (a - ui) / (uip - ui) } else { 0.0 };
                let (pi, pm) = (c.get(i)?, c.get(i - 1)?);
                pm.iter().zip(pi).map(|(y, x)| lerp4(*y, *x, alpha)).collect()
            };
            next.push(row);
        }
        k.insert(span + 1, a);
        c = next;
    }
    let n = c.len();
    let first = n - p - 1;
    let tau = (to - a) / (b - a);
    let width = c.first()?.len();
    for j in 0..width {
        let mut pts: Vec<[f64; 4]> = (first..n).map(|i| c.get(i).and_then(|r| r.get(j)).copied()).collect::<Option<_>>()?;
        let mut left = Vec::with_capacity(p + 1);
        left.push(*pts.first()?);
        for r in 1..=p {
            for i in 0..=(p - r) {
                let (x, y) = (*pts.get(i)?, *pts.get(i + 1)?);
                *pts.get_mut(i)? = lerp4(x, y, tau);
            }
            left.push(*pts.first()?);
        }
        for (i, x) in left.into_iter().enumerate() {
            *c.get_mut(first + i)?.get_mut(j)? = x;
        }
    }
    let len = k.len();
    for x in k.iter_mut().skip(len - p - 1) {
        *x = to;
    }
    Some((k, c))
}

/// A truck B-spline surface from knots and homogeneous control rows (rows[i][j], i along u):
/// rational when `rational`, else of plain points.
fn nurbs_surface(ku: Vec<f64>, kv: Vec<f64>, rows: Vec<Vec<[f64; 4]>>, rational: bool) -> Result<mt::Surface> {
    let kvs = (mt::KnotVec::from(ku), mt::KnotVec::from(kv));
    let bad = |e: String| KernelError::Failed(format!("extend: the continued surface: {e}"));
    if rational {
        let net: Vec<Vec<mt::Vector4>> = rows.iter().map(|r| r.iter().map(|h| mt::Vector4::new(h[0], h[1], h[2], h[3])).collect()).collect();
        Ok(mt::Surface::NurbsSurface(mt::NurbsSurface::new(mt::BSplineSurface::try_new(kvs, net).map_err(|e| bad(e.to_string()))?)))
    } else {
        let net: Vec<Vec<mt::Point3>> =
            rows.iter().map(|r| r.iter().map(|h| mt::Point3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3])).collect()).collect();
        Ok(mt::Surface::BSplineSurface(mt::BSplineSurface::try_new(kvs, net).map_err(|e| bad(e.to_string()))?))
    }
}

/// A truck B-spline curve from knots and homogeneous control points.
fn nurbs_curve(knots: Vec<f64>, ctrl: Vec<[f64; 4]>, rational: bool) -> Result<mt::Curve> {
    let kv = mt::KnotVec::from(knots);
    let bad = |e: String| KernelError::Failed(format!("extend: the continued profile: {e}"));
    if rational {
        let pts: Vec<mt::Vector4> = ctrl.iter().map(|h| mt::Vector4::new(h[0], h[1], h[2], h[3])).collect();
        Ok(mt::Curve::NurbsCurve(mt::NurbsCurve::new(mt::BSplineCurve::try_new(kv, pts).map_err(|e| bad(e.to_string()))?)))
    } else {
        let pts: Vec<mt::Point3> = ctrl.iter().map(|h| mt::Point3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3])).collect();
        Ok(mt::Curve::BSplineCurve(mt::BSplineCurve::try_new(kv, pts).map_err(|e| bad(e.to_string()))?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{measure, patch_region};
    use mt::InnerSpace;
    use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2};
    use std::collections::HashSet;
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs().max(1e-9)
    }
    fn area(b: &Body) -> f64 {
        measure(b).unwrap().area
    }
    fn with(mode: ExtendMode, limit: ExtendLimit, neighbours: bool) -> ExtendOptions {
        ExtendOptions { mode, limit, neighbours }
    }
    fn natural(d: f64) -> ExtendOptions<'static> {
        with(ExtendMode::Natural, ExtendLimit::Distance(d), true)
    }
    fn edges(b: &Body) -> usize {
        b.solid.edge_iter().map(|e| e.id()).collect::<HashSet<_>>().len()
    }
    /// Edges bounding two faces.
    fn shared(b: &Body) -> usize {
        let mut count: HashMap<mt::EdgeID, usize> = HashMap::new();
        for f in b.solid.face_iter() {
            for w in f.boundaries().iter() {
                for e in w.edge_iter() {
                    *count.entry(e.id()).or_default() += 1;
                }
            }
        }
        count.values().filter(|n| **n == 2).count()
    }
    fn square() -> Body {
        let r = Region2 {
            outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)]),
            holes: vec![],
        };
        patch_region(&Plane::XY, &r).unwrap()
    }
    /// A strip of the cylinder of radius 10 about Z, heights 0…20, angles 0…`sweep`: one face on
    /// an exact surface of revolution (a line turned about the axis), its edges two generators
    /// and two arcs.
    fn cylinder_strip(sweep: f64) -> Body {
        let pt = mt::Point3::new;
        let rev = mt::RevolutedCurve::by_revolution(
            mt::Curve::Line(mt::Line(pt(10.0, 0.0, 0.0), pt(10.0, 0.0, 20.0))),
            pt(0.0, 0.0, 0.0),
            mt::Vector3::unit_z(),
        );
        let s = mt::Surface::RevolutedCurve(mt::Processor::new(rev));
        // Counter-clockwise in (height, angle): up the first generator, along the top arc, down
        // the last generator, back along the bottom arc.
        let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, sweep), (0.0, sweep)];
        let v: Vec<mt::Vertex> = corners.iter().map(|(u, a)| mt::Vertex::new(s.subs(*u, *a))).collect();
        let edges: Vec<mt::Edge> = (0..4)
            .map(|i| {
                let ((u0, a0), (u1, a1)) = (corners[i], corners[(i + 1) % 4]);
                let c = if a0 == a1 { iso_curve(&s, true, a0, u0, u1) } else { iso_curve(&s, false, u0, a0, a1) };
                mt::Edge::new(&v[i], &v[(i + 1) % 4], c.unwrap())
            })
            .collect();
        surface_body(vec![mt::Face::new(vec![edges.into()], s)])
    }

    /// Arc length of z = x²/20 from 0 to x.
    fn parabola_arc(x: f64) -> f64 {
        0.5 * x * (1.0 + (x / 10.0).powi(2)).sqrt() + 5.0 * (x / 10.0).asinh()
    }
    /// The parabolic sheet z = x²/20 over 0 ≤ x, y ≤ 10: a B-spline quadratic along x (u),
    /// straight along y (v), its four edges on its parameter lines.
    fn parabola() -> Body {
        let pt = mt::Point3::new;
        let net = vec![
            vec![pt(0.0, 0.0, 0.0), pt(0.0, 10.0, 0.0)],
            vec![pt(5.0, 0.0, 0.0), pt(5.0, 10.0, 0.0)],
            vec![pt(10.0, 0.0, 5.0), pt(10.0, 10.0, 5.0)],
        ];
        let s = mt::BSplineSurface::new((mt::KnotVec::bezier_knot(2), mt::KnotVec::bezier_knot(1)), net);
        let v = [pt(0.0, 0.0, 0.0), pt(10.0, 0.0, 5.0), pt(10.0, 10.0, 5.0), pt(0.0, 10.0, 0.0)].map(mt::Vertex::new);
        let bez =
            |a: mt::Point3, m: mt::Point3, b: mt::Point3| mt::Curve::BSplineCurve(mt::BSplineCurve::new(mt::KnotVec::bezier_knot(2), vec![a, m, b]));
        let edges = vec![
            mt::Edge::new(&v[0], &v[1], bez(pt(0.0, 0.0, 0.0), pt(5.0, 0.0, 0.0), pt(10.0, 0.0, 5.0))),
            mt::Edge::new(&v[1], &v[2], mt::Curve::Line(mt::Line(v[1].point(), v[2].point()))),
            mt::Edge::new(&v[2], &v[3], bez(pt(10.0, 10.0, 5.0), pt(5.0, 10.0, 0.0), pt(0.0, 10.0, 0.0))),
            mt::Edge::new(&v[3], &v[0], mt::Curve::Line(mt::Line(v[3].point(), v[0].point()))),
        ];
        surface_body(vec![mt::Face::new(vec![edges.into()], mt::Surface::BSplineSurface(s))])
    }

    /// The default (natural, by a distance, neighbours lengthened) on a plane's straight edges
    /// is today's planar extension, exactly; the general path builds the same square.
    #[test]
    fn a_planar_extension_is_unchanged() {
        let s = square();
        let old = crate::extend(&s, &[Vec3::new(10.0, 5.0, 0.0)], 5.0).unwrap();
        let new = extend_surface(&s, &[Vec3::new(10.0, 5.0, 0.0)], &natural(5.0)).unwrap();
        assert_eq!(measure(&old).unwrap().area, measure(&new).unwrap().area);
        assert_eq!(edges(&old), edges(&new));
        // Up to the plane x = 15 takes the general path: the same 15 × 10, its lengthened sides
        // one edge each.
        let tool = SplitTool::Plane(Plane::from_normal(Vec3::new(15.0, 0.0, 0.0), Vec3::X).unwrap());
        let up = extend_surface(&s, &[Vec3::new(10.0, 5.0, 0.0)], &with(ExtendMode::Natural, ExtendLimit::UpTo(&tool), true)).unwrap();
        assert!(rel(area(&up), 150.0) < 1e-9, "{}", area(&up));
        assert_eq!((up.face_count(), edges(&up)), (1, 4));
        // Up to x = 17: 170.
        let tool = SplitTool::Plane(Plane::from_normal(Vec3::new(17.0, 0.0, 0.0), Vec3::X).unwrap());
        let up = extend_surface(&s, &[Vec3::new(10.0, 5.0, 0.0)], &with(ExtendMode::Natural, ExtendLimit::UpTo(&tool), true)).unwrap();
        assert!(rel(area(&up), 170.0) < 1e-9, "{}", area(&up));
        let bb = measure(&up).unwrap().bbox;
        assert!((bb.max.x - 17.0).abs() < 1e-9, "{bb:?}");
    }

    /// A quadrilateral (0,0), (10,0), (10,10), (0,20) (area 150) with its right edge pushed out
    /// by 5. Lengthening the neighbours follows the slanted top down to (15, 5): + 37.5. Without
    /// them the extension is a 5 × 10 strip: + 50. A tangent run-off is the same trapezoid as a
    /// face of its own, sharing the edge.
    #[test]
    fn neighbours_lengthened_or_square() {
        let r = Region2 {
            outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 20.0)]),
            holes: vec![],
        };
        let q = patch_region(&Plane::XY, &r).unwrap();
        let at = [Vec3::new(10.0, 5.0, 0.0)];
        let lengthened = extend_surface(&q, &at, &natural(5.0)).unwrap();
        assert!(rel(area(&lengthened), 187.5) < 1e-9, "{}", area(&lengthened));
        let square_ends = extend_surface(&q, &at, &with(ExtendMode::Natural, ExtendLimit::Distance(5.0), false)).unwrap();
        assert!(rel(area(&square_ends), 200.0) < 1e-9, "{}", area(&square_ends));
        let run_off = extend_surface(&q, &at, &with(ExtendMode::Tangent, ExtendLimit::Distance(5.0), true)).unwrap();
        assert_eq!((run_off.face_count(), shared(&run_off)), (2, 1));
        assert!(rel(area(&run_off), 187.5) < 1e-6, "{}", area(&run_off));
        let h = run_off.history().unwrap();
        assert_eq!(h.faces.iter().map(|f| f.change).collect::<Vec<_>>(), vec![crate::history::Change::Kept, crate::history::Change::Generated]);
    }

    /// A cylinder strip (radius 10, a quarter turn, heights 0…20), on an exact surface of
    /// revolution: extended 5 along its axis (its top arc picked), 10 · π/2 · 25; 5 around its
    /// circumference (a straight edge picked), 10 · (π/2 + ½) · 20; both, meeting at a mitre,
    /// 10 · (π/2 + ½) · 25; up to the plane z = 30, 10 · π/2 · 30. Each stays one cylindrical
    /// face; the sides along the extension are the old edges lengthened.
    #[test]
    fn a_cylinder_strip_extended_along_its_axis_and_around() {
        let c = cylinder_strip(FRAC_PI_2);
        assert!(rel(area(&c), 100.0 * PI) < 1e-3);
        let top = Vec3::new(10.0 * FRAC_PI_4.cos(), 10.0 * FRAC_PI_4.sin(), 20.0);
        let side = Vec3::new(0.0, 10.0, 10.0);
        let cylinder =
            |b: &Body| matches!(crate::face_surface(b, 0), Some(crate::FaceSurface::Cylinder { radius, .. }) if (radius - 10.0).abs() < 1e-6);

        let up = extend_surface(&c, &[top], &natural(5.0)).unwrap();
        let want = 10.0 * FRAC_PI_2 * 25.0;
        assert!(rel(area(&up), want) < 1e-3, "{} vs {want}", area(&up)); // (measured on meshes)
        assert!((measure(&up).unwrap().bbox.max.z - 25.0).abs() < 1e-6);
        assert!(up.face_count() == 1 && cylinder(&up) && edges(&up) == 4, "{}", edges(&up));

        let round = extend_surface(&c, &[side], &natural(5.0)).unwrap();
        let want = 10.0 * (FRAC_PI_2 + 0.5) * 20.0;
        assert!(rel(area(&round), want) < 1e-3, "{} vs {want}", area(&round)); // (measured on meshes)
        assert!(round.face_count() == 1 && cylinder(&round) && edges(&round) == 4);
        // The moved edge is at angle π/2 + ½ (5 mm of arc): x = −10 sin ½.
        assert!((measure(&round).unwrap().bbox.min.x + 10.0 * 0.5f64.sin()).abs() < 1e-6, "{:?}", measure(&round).unwrap().bbox);

        let both = extend_surface(&c, &[top, side], &natural(5.0)).unwrap();
        let want = 10.0 * (FRAC_PI_2 + 0.5) * 25.0;
        assert!(rel(area(&both), want) < 1e-3, "{} vs {want}", area(&both)); // (measured on meshes)

        let tool = SplitTool::Plane(Plane::from_normal(Vec3::new(0.0, 0.0, 30.0), Vec3::Z).unwrap());
        let to = extend_surface(&c, &[top], &with(ExtendMode::Natural, ExtendLimit::UpTo(&tool), true)).unwrap();
        let want = 10.0 * FRAC_PI_2 * 30.0;
        assert!(rel(area(&to), want) < 1e-3, "{} vs {want}", area(&to)); // (measured on meshes)
        assert!((measure(&to).unwrap().bbox.max.z - 30.0).abs() < 1e-6);

        // The history: the face is the one it was, trimmed differently.
        let h = up.history().unwrap();
        assert_eq!(h.faces.first().map(|f| f.change), Some(crate::history::Change::Modified));
    }

    /// The same strip swept as a B-spline sheet (a rational arc extruded): its end spans
    /// continue as themselves, so the extension stays on the cylinder.
    #[test]
    fn an_extruded_cylinder_strip_continues_on_its_cylinder() {
        let arc = vec![Seg2::Arc { center: Vec2::new(0.0, 0.0), radius: 10.0, start: 0.0, sweep: FRAC_PI_2 }];
        let sheet = crate::extrude_sheet(&Plane::XY, &[arc], 0.0, 20.0).unwrap();
        let top = Vec3::new(10.0 * FRAC_PI_4.cos(), 10.0 * FRAC_PI_4.sin(), 20.0);
        let up = extend_surface(&sheet, &[top], &natural(5.0)).unwrap();
        let want = 10.0 * FRAC_PI_2 * 25.0;
        assert!(rel(area(&up), want) < 1e-3, "{} vs {want}", area(&up)); // (measured on meshes)
        let round = extend_surface(&sheet, &[Vec3::new(0.0, 10.0, 10.0)], &natural(5.0)).unwrap();
        let want = 10.0 * (FRAC_PI_2 + 0.5) * 20.0;
        assert!(rel(area(&round), want) < 1e-3, "{} vs {want}", area(&round)); // (measured on meshes)
        assert!(matches!(crate::face_surface(&round, 0), Some(crate::FaceSurface::Cylinder { radius, .. }) if (radius - 10.0).abs() < 1e-6));
    }

    /// A half cylinder pushed round by 40 mm would close on itself (π + 4 > 2π): refused. So
    /// is a concave edge pushed past its centre.
    #[test]
    fn self_intersecting_extensions_are_refused() {
        let c = cylinder_strip(PI);
        let side = Vec3::new(-10.0, 0.0, 10.0);
        let e = extend_surface(&c, &[side], &natural(40.0)).unwrap_err().to_string();
        assert!(e.contains("wrap"), "{e}");
        // A 20 × 10 plate with a half-round notch of radius 5 in its top: the notch's edge
        // pushed out by 2 leaves a notch of radius 3 (area 200 − 4.5π, the top edges
        // lengthened into it); by 6, past the notch's centre, it folds and is refused.
        let outer = Loop2 {
            segs: vec![
                Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(20.0, 0.0) },
                Seg2::Line { a: Vec2::new(20.0, 0.0), b: Vec2::new(20.0, 10.0) },
                Seg2::Line { a: Vec2::new(20.0, 10.0), b: Vec2::new(15.0, 10.0) },
                Seg2::Arc { center: Vec2::new(10.0, 10.0), radius: 5.0, start: 0.0, sweep: -PI },
                Seg2::Line { a: Vec2::new(5.0, 10.0), b: Vec2::new(0.0, 10.0) },
                Seg2::Line { a: Vec2::new(0.0, 10.0), b: Vec2::new(0.0, 0.0) },
            ],
        };
        let plate = patch_region(&Plane::XY, &Region2 { outer, holes: vec![] }).unwrap();
        assert!(rel(area(&plate), 200.0 - 12.5 * PI) < 1e-4); // (measured on meshes)
        let notch = [Vec3::new(10.0, 5.0, 0.0)];
        let less = extend_surface(&plate, &notch, &natural(2.0)).unwrap();
        assert!(rel(area(&less), 200.0 - 4.5 * PI) < 1e-4, "{} vs {}", area(&less), 200.0 - 4.5 * PI); // (measured on meshes)
        for mode in [ExtendMode::Natural, ExtendMode::Tangent, ExtendMode::Curvature] {
            let e = extend_surface(&plate, &notch, &with(mode, ExtendLimit::Distance(6.0), true)).unwrap_err().to_string();
            assert!(e.contains("folds") || e.contains("crosses"), "{mode:?}: {e}");
        }
    }

    /// The parabolic B-spline sheet z = x²/20 (0 ≤ x, y ≤ 10) extended 5 past its edge x = 10.
    /// Natural: the parabola itself continues, 5 mm of arc further (to x* where its arc length
    /// grows by 5): area 10 · (s(10) + 5), reaching z = x*²/20. Curvature: a run-off face along
    /// the edge, its normal and its normal curvature across the edge those of the parabola
    /// there (G1, G2: κ = 0.1 / 2^1.5), area 10 · ∫₀⁵ √(1 + κ²s²) ds more. Tangent: a plane
    /// run-off, 50 more.
    #[test]
    fn a_bspline_extended_naturally_and_by_curvature() {
        let p = parabola();
        let base = 10.0 * parabola_arc(10.0);
        assert!(rel(area(&p), base) < 1e-4, "{} vs {base}", area(&p)); // (measured on meshes)
        let edge = [Vec3::new(10.0, 5.0, 5.0)];

        let nat = extend_surface(&p, &edge, &natural(5.0)).unwrap();
        let (mut lo, mut hi) = (10.0, 20.0);
        for _ in 0..100 {
            let m = 0.5 * (lo + hi);
            if parabola_arc(m) - parabola_arc(10.0) < 5.0 { lo = m } else { hi = m }
        }
        let x = 0.5 * (lo + hi);
        let bb = measure(&nat).unwrap().bbox;
        assert!((bb.max.x - x).abs() < 1e-6 && (bb.max.z - x * x / 20.0).abs() < 1e-6, "{bb:?} vs x* = {x}");
        assert!(rel(area(&nat), base + 50.0) < 1e-4, "{} vs {}", area(&nat), base + 50.0); // (measured on meshes)
        assert_eq!(nat.face_count(), 1);
        // On the extended surface the parabola goes on: its height at x = 12, past the old edge.
        let s = nat.solid.face_iter().next().unwrap().surface();
        let (u, v) = mt::SearchNearestParameter::<mt::D2>::search_nearest_parameter(&s, mt::Point3::new(12.0, 5.0, 7.2), None, 100).unwrap();
        let off = from_p3(s.subs(u, v)).dist(Vec3::new(12.0, 5.0, 7.2));
        assert!(off < 1e-7, "{off}");

        let kappa = 0.1 / 2f64.powf(1.5);
        let curv = extend_surface(&p, &edge, &with(ExtendMode::Curvature, ExtendLimit::Distance(5.0), true)).unwrap();
        assert_eq!((curv.face_count(), shared(&curv)), (2, 1));
        let arc = 2.5 * (1.0 + 25.0 * kappa * kappa).sqrt() + (5.0 * kappa).asinh() / (2.0 * kappa);
        assert!(rel(area(&curv), base + 10.0 * arc) < 1e-4, "{} vs {}", area(&curv), base + 10.0 * arc); // (measured on meshes)
        // G1 and G2 at the old edge, measured on the run-off along its length.
        let run = curv.solid.face_iter().nth(1).unwrap().surface();
        let up = mt::Vector3::new(-1.0, 0.0, 1.0) / 2f64.sqrt();
        for k in 0..=8 {
            let u = k as f64 / 8.0;
            let (su, sv) = (run.uder(u, 0.0), run.vder(u, 0.0));
            let n = su.cross(sv).normalize();
            let n = if n.dot(up) < 0.0 { -n } else { n };
            assert!((n - up).magnitude() < 1e-6, "normal {n:?} at {u}");
            let k_across = run.vvder(u, 0.0).dot(up) / sv.magnitude2();
            assert!(rel(k_across, kappa) < 1e-6, "κ {k_across} vs {kappa} at {u}");
        }

        let flat = extend_surface(&p, &edge, &with(ExtendMode::Tangent, ExtendLimit::Distance(5.0), true)).unwrap();
        assert!(rel(area(&flat), base + 50.0) < 1e-4, "{} vs {}", area(&flat), base + 50.0); // (measured on meshes)
        let run = flat.solid.face_iter().nth(1).unwrap().surface();
        let n = run.uder(0.5, 0.0).cross(run.vder(0.5, 0.0)).normalize();
        assert!(n.cross(up).magnitude() < 1e-6, "G1: {n:?}");
    }

    /// A disc of radius 5 (two half-circle edges meeting smoothly) with both edges pushed out
    /// by 2: naturally, one disc of radius 7 (49π); as tangent run-offs, an annulus of two
    /// faces sharing their sides where the half circles meet (25π + 24π).
    #[test]
    fn a_disc_grows_by_its_curved_edges() {
        let disc = patch_region(&Plane::XY, &Region2 { outer: Loop2::circle(Vec2::ZERO, 5.0), holes: vec![] }).unwrap();
        let both = [Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, -5.0, 0.0)];
        let grown = extend_surface(&disc, &both, &natural(2.0)).unwrap();
        assert_eq!(grown.face_count(), 1);
        assert!(rel(area(&grown), 49.0 * PI) < 1e-4, "{} vs {}", area(&grown), 49.0 * PI); // (measured on meshes)
        let ring = extend_surface(&disc, &both, &with(ExtendMode::Tangent, ExtendLimit::Distance(2.0), true)).unwrap();
        assert_eq!((ring.face_count(), shared(&ring)), (3, 4));
        assert!(rel(area(&ring), 49.0 * PI) < 1e-4, "{} vs {}", area(&ring), 49.0 * PI); // (measured on meshes)
    }

    /// Two faces of a box (its top and front, sharing their edge along x) with their edges at
    /// x = 10 pushed out by 5: the shared edge is lengthened in both, as one edge, so the two
    /// extensions stay joined: 150 + 150, two shared edges.
    #[test]
    fn neighbouring_faces_extend_together() {
        let b = crate::box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let two = crate::copy_faces(&b, &[Vec3::new(5.0, 5.0, 10.0), Vec3::new(5.0, 0.0, 5.0)]).unwrap();
        assert_eq!((two.face_count(), shared(&two)), (2, 1));
        let out = extend_surface(&two, &[Vec3::new(10.0, 5.0, 10.0), Vec3::new(10.0, 0.0, 5.0)], &natural(5.0)).unwrap();
        assert!(rel(area(&out), 300.0) < 1e-9, "{}", area(&out));
        assert_eq!((out.face_count(), shared(&out)), (2, 2));
    }

    /// Back to the surface's own bounds: the half saddle (a B-spline over 0…10 × 0…10, x from 0
    /// to 5) goes back to its whole patch; the half cylinder trimmed by a slanted plane (top
    /// from z = 15 down to 10 and back) goes back to its span, heights 0…15: 150π.
    #[test]
    fn extended_back_to_the_surfaces_bounds() {
        let pt = |x: f64, y: f64| mt::Point3::new(x, y, x * y / 10.0);
        let knots = mt::KnotVec::bezier_knot(1);
        let saddle = mt::BSplineSurface::new((knots.clone(), knots), vec![vec![pt(0.0, 0.0), pt(0.0, 10.0)], vec![pt(10.0, 0.0), pt(10.0, 10.0)]]);
        let corners = [pt(0.0, 0.0), pt(5.0, 0.0), pt(5.0, 10.0), pt(0.0, 10.0)];
        let v: Vec<mt::Vertex> = corners.iter().map(|p| mt::Vertex::new(*p)).collect();
        let e: Vec<mt::Edge> =
            (0..4).map(|i| mt::Edge::new(&v[i], &v[(i + 1) % 4], mt::Curve::Line(mt::Line(corners[i], corners[(i + 1) % 4])))).collect();
        let half = surface_body(vec![mt::Face::new(vec![e.into()], mt::Surface::BSplineSurface(saddle))]);
        let bounds = with(ExtendMode::Natural, ExtendLimit::Bounds, true);
        let whole = extend_surface(&half, &[Vec3::new(5.0, 5.0, 2.5)], &bounds).unwrap();
        let bb = measure(&whole).unwrap().bbox;
        assert!((bb.max.x - 10.0).abs() < 1e-9 && (bb.max.z - 10.0).abs() < 1e-9, "{bb:?}");
        // Already there: refused.
        assert!(extend_surface(&whole, &[Vec3::new(10.0, 5.0, 5.0)], &bounds).is_err());

        let arc = vec![Seg2::Arc { center: Vec2::new(0.0, 0.0), radius: 10.0, start: 0.0, sweep: PI }];
        let sheet = crate::extrude_sheet(&Plane::XY, &[arc], 0.0, 20.0).unwrap();
        let tool = SplitTool::Plane(Plane::from_normal(Vec3::new(0.0, 0.0, 15.0), Vec3::new(0.0, 0.5, 1.0)).unwrap());
        let cut = crate::trim(&sheet, &tool, Vec3::new(0.0, 10.0, 1.0)).unwrap();
        let back = extend_surface(&cut, &[Vec3::new(0.0, 10.0, 10.0)], &bounds).unwrap();
        let a = area(&back);
        assert!(rel(a, 150.0 * PI) < 1e-3, "{a} vs {}", 150.0 * PI); // (measured on meshes)
        assert!((measure(&back).unwrap().bbox.max.z - 15.0).abs() < 1e-6);
    }

    /// Hostile and unsupported requests are refused, never panic.
    #[test]
    fn hostile_requests_are_refused() {
        let s = square();
        let at = [Vec3::new(10.0, 5.0, 0.0)];
        for d in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e9] {
            assert!(extend_surface(&s, &at, &natural(d)).is_err(), "{d}");
        }
        assert!(extend_surface(&s, &[], &natural(1.0)).is_err());
        assert!(extend_surface(&s, &[Vec3::new(f64::NAN, 0.0, 0.0)], &natural(1.0)).is_err());
        assert!(extend_surface(&s, &vec![Vec3::ZERO; MAX_PICKS + 1], &natural(1.0)).is_err());
        assert!(extend_surface(&s, &[Vec3::new(50.0, 50.0, 0.0)], &natural(1.0)).is_err());
        assert!(extend_surface(&s, &at, &with(ExtendMode::Tangent, ExtendLimit::Bounds, true)).is_err());
        // A solid's edges lie between two faces.
        let b = crate::box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)).unwrap();
        assert!(extend_surface(&b, &[Vec3::new(10.0, 5.0, 10.0)], &natural(1.0)).is_err());
        // A hole's edge.
        let r = Region2 {
            outer: Loop2::polygon(&[Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0), Vec2::new(20.0, 20.0), Vec2::new(0.0, 20.0)]),
            holes: vec![Loop2::circle(Vec2::new(10.0, 10.0), 3.0).reversed()],
        };
        let holed = patch_region(&Plane::XY, &r).unwrap();
        let e = extend_surface(&holed, &[Vec3::new(13.0, 10.0, 0.0)], &with(ExtendMode::Tangent, ExtendLimit::Distance(1.0), true)).unwrap_err();
        assert!(e.to_string().contains("hole"), "{e}");
        // A target the extension never reaches (behind the edge).
        let behind = SplitTool::Plane(Plane::from_normal(Vec3::new(-5.0, 0.0, 0.0), Vec3::X).unwrap());
        assert!(extend_surface(&s, &at, &with(ExtendMode::Natural, ExtendLimit::UpTo(&behind), true)).is_err());
    }

    /// Continuing a B-spline's end span leaves it the same on its old range.
    #[test]
    fn a_continued_bspline_is_unchanged_on_its_range() {
        // A cubic with interior knots, rational weights.
        let knots = vec![0.0, 0.0, 0.0, 0.0, 0.3, 0.6, 1.0, 1.0, 1.0, 1.0];
        let ctrl: Vec<Vec<[f64; 4]>> =
            [[0.0, 0.0, 0.0, 1.0], [1.0, 2.0, 0.0, 0.8], [2.0, -1.0, 1.0, 1.2], [3.0, 1.0, 0.5, 1.0], [4.0, 0.0, 2.0, 0.9], [5.0, 1.0, 0.0, 1.0]]
                .iter()
                .map(|p| vec![[p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]])
                .collect();
        let before = solidcraft_geom::tess::EvalCurve { degree: 3, knots: knots.clone(), ctrl: ctrl.iter().map(|r| r[0]).collect() };
        let (k, c) = continue_span(&knots, 3, &ctrl, (-0.2, 1.25)).unwrap();
        let after = solidcraft_geom::tess::EvalCurve { degree: 3, knots: k.clone(), ctrl: c.iter().map(|r| r[0]).collect() };
        assert_eq!((k.first().copied(), k.last().copied()), (Some(-0.2), Some(1.25)));
        // The same curve on the old range, and past it the old end spans' own polynomials
        // (which the evaluator also continues).
        for i in -4..=25 {
            let t = i as f64 / 20.0;
            let (a, b) = (before.eval(t).unwrap().0, after.eval(t).unwrap().0);
            assert!(a.dist(b) < 1e-9, "{t}: {a:?} vs {b:?}");
        }
    }
}
