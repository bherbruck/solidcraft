#![allow(clippy::many_single_char_names)]

use super::*;
use crate::filters::NormalFilters;
use crate::Point2;
use array_macro::array;
use itertools::Itertools;
use rustc_hash::FxHashMap as HashMap;

#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

type SPoint2 = spade::Point2<f64>;
type Cdt = ConstrainedDelaunayTriangulation<SPoint2>;
type MeshedShell = Shell<Point3, PolylineCurve, Option<PolygonMesh>>;
type MeshedCShell = CompressedShell<Point3, PolylineCurve, Option<PolygonMesh>>;

pub(super) trait SP<S>:
    Fn(&S, Point3, Option<(f64, f64)>, f64) -> Option<(f64, f64)> + Parallelizable {
}
impl<S, F> SP<S> for F where F: Fn(&S, Point3, Option<(f64, f64)>, f64) -> Option<(f64, f64)> + Parallelizable {}

/// SolidCraft: a parameter inside the surface's bounded, non-periodic ranges.
fn in_domain<S: ParametricSurface>(surface: &S, (u, v): (f64, f64)) -> bool {
    let ok = |x: f64, range: Option<(f64, f64)>, period: Option<f64>| {
        x.is_finite()
            && match (range, period) {
                (_, Some(_)) | (None, None) => true,
                (Some((a, b)), None) => {
                    let m = (b - a).abs() * 1e-6 + 1e-9;
                    x >= a.min(b) - m && x <= a.max(b) + m
                }
            }
    };
    let (ur, vr) = surface.try_range_tuple();
    ok(u, ur, surface.u_period()) && ok(v, vr, surface.v_period())
}

/// SolidCraft: the parameter clamped into the surface's bounded, non-periodic ranges.
fn clamped<S: ParametricSurface>(surface: &S, (u, v): (f64, f64)) -> (f64, f64) {
    let c = |x: f64, range: Option<(f64, f64)>, period: Option<f64>| match (range, period) {
        (Some((a, b)), None) => x.clamp(a.min(b), a.max(b)),
        _ => x,
    };
    let (ur, vr) = surface.try_range_tuple();
    (c(u, ur, surface.u_period()), c(v, vr, surface.v_period()))
}

/// SolidCraft: exact and nearest searches can run off a B-spline's domain, especially at a
/// collapsed edge. Prefer a bounded equivalent within the existing spatial search tolerance;
/// otherwise retain extrapolation only when it is closer (planes have a nominal unit range
/// but extend for ever). At most four boundary candidates are evaluated, with no new search.
fn sane<S: ParametricSurface3D>(surface: &S, point: Point3, uv: (f64, f64)) -> (f64, f64) {
    let c = clamped(surface, uv);
    let d = |(u, v): (f64, f64)| surface.subs(u, v).distance2(point);
    let (mut best, mut error) = (uv, d(uv));
    let dc = d(c);
    let collapsed = |(u, v): (f64, f64), axis| {
        let tangent = if axis == 0 { surface.vder(u, v) } else { surface.uder(u, v) };
        tangent.magnitude2() <= TOLERANCE2
    };
    if !in_domain(surface, uv)
        && (dc < error || (dc <= TOLERANCE2 && (collapsed(c, 0) || collapsed(c, 1)))) {
        (best, error) = (c, dc);
    }
    // SolidCraft: at a collapsed boundary an exact search can return arbitrary,
    // enormous parameters, or stop just inside the pole. Prefer an equivalent
    // in-domain point and the exact boundary when it improves the spatial fit.
    // A plane's nominal unit box must not clip genuinely unbounded geometry.
    let (ur, vr) = surface.try_range_tuple();
    for (axis, range, period) in [(0, ur, surface.u_period()), (1, vr, surface.v_period())] {
        if period.is_some() { continue; }
        if let Some((a, b)) = range {
            for end in [a, b] {
                if !end.is_finite() { continue; }
                let coordinate = if axis == 0 { best.0 } else { best.1 };
                let other = if end == a { b } else { a };
                if (coordinate - end).abs() > (coordinate - other).abs() { continue; }
                let candidate = if axis == 0 { (end, best.1) } else { (best.0, end) };
                let distance = d(candidate);
                // A closed spline seam has equivalent endpoints too, but moving
                // between them changes the trim loop. Only canonicalize poles.
                if distance <= TOLERANCE2 && distance < error && collapsed(candidate, axis) {
                    (best, error) = (candidate, distance);
                }
            }
        }
    }
    best
}

pub(super) fn by_search_parameter<S>(
    surface: &S,
    point: Point3,
    hint: Option<(f64, f64)>,
    _accept: f64,
) -> Option<(f64, f64)>
where
    S: MeshableSurface,
{
    surface
        .search_parameter(point, hint, 100)
        .or_else(|| surface.search_parameter(point, None, 100))
}

pub(super) fn by_search_nearest_parameter<S>(
    surface: &S,
    point: Point3,
    hint: Option<(f64, f64)>,
    accept: f64,
) -> Option<(f64, f64)>
where S: RobustMeshableSurface {
    nearest_projection(surface, point, hint, accept, 100)
}

/// A hint can converge to a spline's continuation outside its knot domain. Prefer a
/// bounded fit within the caller's chord tolerance. Some modelling surfaces really are
/// extended beyond their stored domain; retain that continuation when no bounded fit is
/// accurate enough, rather than losing a valid blend face.
pub(super) fn nearest_projection<S: RobustMeshableSurface>(surface: &S, point: Point3, hint: Option<(f64,f64)>, accept: f64, trials: usize) -> Option<(f64,f64)> {
    // An affine plane's unit parameter box is only a chart, not a finite support.
    // Preserve its exact projection even when the trim extends beyond that box.
    if affine_chart(surface) {
        return surface.search_parameter(point, hint, trials)
            .or_else(|| surface.search_parameter(point, None, trials))
            .or_else(|| surface.search_nearest_parameter(point, hint, trials))
            .or_else(|| surface.search_nearest_parameter(point, None, trials));
    }
    let candidate = |uv| {
        let uv = sane(surface, point, uv);
        let error = surface.subs(uv.0, uv.1).distance2(point);
        (uv.0.is_finite() && uv.1.is_finite() && error.is_finite()).then_some((uv, error))
    };
    let fits = |&(uv, error): &((f64, f64), f64)| in_domain(surface, uv) && error <= accept * accept;
    let exact = surface.search_parameter(point, hint, trials)
        .or_else(|| surface.search_parameter(point, None, trials)).and_then(candidate);
    if let Some((uv, _)) = exact.filter(fits) { return Some(uv); }
    let hinted = surface.search_nearest_parameter(point, hint, trials).and_then(candidate);
    if let Some((uv, _)) = hinted.filter(fits) { return Some(uv); }
    let fresh = surface.search_nearest_parameter(point, None, trials).and_then(candidate);
    if let Some((uv, _)) = fresh.filter(fits) { return Some(uv); }
    let best = [exact, hinted, fresh].into_iter().flatten().min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some(uv) = best.map(|x| x.0).or(hint) {
        if let Some(bounded) = bounded_projection(surface, point, uv).filter(fits) {
            return Some(bounded.0);
        }
    }
    // Even the unhinted search can fail for shallow patches of very large cylinders.
    // Their domain midpoint supplies a finite start without trusting a previous failure.
    if let (Some((a, b)), Some((c, d))) = surface.try_range_tuple() {
        if [a, b, c, d].iter().all(|x| x.is_finite()) {
            let start = (a * 0.5 + b * 0.5, c * 0.5 + d * 0.5);
            if let Some(bounded) = bounded_projection(surface, point, start).filter(fits) {
                return Some(bounded.0);
            }
        }
    }
    best.map(|x| x.0)
}

fn affine_chart<S: ParametricSurface3D>(surface: &S) -> bool {
    let (Some((a, b)), Some((c, d))) = surface.try_range_tuple() else { return false };
    [a, b, c, d].iter().all(|x| x.is_finite()) && [0.0, 0.5, 1.0].into_iter().all(|t| {
        let (u, v) = (a + (b - a) * t, c + (d - c) * t);
        [surface.uuder(u, v), surface.uvder(u, v), surface.vvder(u, v)]
            .iter().all(|x| x.magnitude2() <= 1e-24)
    })
}

/// Damped least squares constrained to the finite knot domain. An unconstrained Newton
/// step is unreliable where the two tangents almost coincide. This fallback has fixed work
/// limits and is used only after the usual searches failed to find a bounded fit.
fn bounded_projection<S: ParametricSurface3D>(surface: &S, point: Point3, start: (f64, f64)) -> Option<((f64, f64), f64)> {
    let mut uv = clamped(surface, start);
    let mut error = surface.subs(uv.0, uv.1).distance2(point);
    let mut lambda = 1e-6_f64;
    for _ in 0..60 {
        let q = surface.subs(uv.0, uv.1);
        let (du, dv) = (surface.uder(uv.0, uv.1), surface.vder(uv.0, uv.1));
        let r = point - q;
        let (a, b, c) = (du.magnitude2(), du.dot(dv), dv.magnitude2());
        let scale = a + c;
        if !scale.is_finite() || scale <= 0.0 { break; }
        let (aa, cc) = (a + lambda * scale, c + lambda * scale);
        let det = aa * cc - b * b;
        if !det.is_finite() || det <= 0.0 { break; }
        let (g, h) = (du.dot(r), dv.dot(r));
        let next = clamped(surface, (uv.0 + (cc * g - b * h) / det, uv.1 + (aa * h - b * g) / det));
        let nd = surface.subs(next.0, next.1).distance2(point);
        if nd.is_finite() && nd <= error {
            let moved = (next.0 - uv.0).abs() + (next.1 - uv.1).abs();
            (uv, error) = (next, nd);
            lambda = (lambda * 0.1).max(1e-12);
            if moved <= 1e-15 * (1.0 + uv.0.abs() + uv.1.abs()) { break; }
        } else {
            lambda *= 10.0;
            if lambda > 1e8 { break; }
        }
    }
    (error.is_finite() && in_domain(surface, uv)).then_some((uv, error))
}

/// Tessellates faces
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn shell_tessellation<'a, C, S>(
    shell: &Shell<Point3, C, S>,
    tol: Deflection,
    sp: impl SP<S>,
) -> MeshedShell
where
    C: PolylineableCurve + 'a,
    S: PreMeshableSurface + 'a,
{
    let cap = face_cap(shell.len());
    let vmap: HashMap<_, _> = shell
        .vertex_par_iter()
        .map(|v| (v.id(), v.mapped(Point3::clone)))
        .collect();
    let eset: HashMap<_, _> = shell.edge_par_iter().map(move |e| (e.id(), e)).collect();
    // SolidCraft: the polylines first, so that the edges of a face crowded by the angle bound
    // can be divided again before any face is meshed.
    let mut polys: HashMap<_, _> = eset
        .par_iter()
        .map(|(id, edge)| {
            let curve = edge.curve();
            (*id, (bounded_polyline(&curve, curve.range_tuple(), tol), 0))
        })
        .collect();
    relax_crowded_faces(shell, &mut polys, tol);
    let edge_map: HashMap<_, _> = eset
        .into_par_iter()
        .map(move |(id, edge)| {
            let v0 = vmap.get(&edge.absolute_front().id()).unwrap();
            let v1 = vmap.get(&edge.absolute_back().id()).unwrap();
            let (poly, level) = polys.get(&id).unwrap().clone();
            (id, (Edge::new_unchecked(v0, v1, poly), level))
        })
        .collect();
    let edge_map = &edge_map;
    let create_edge = |edge: &Edge<Point3, C>| -> Edge<_, _> {
        let (new_edge, _) = edge_map.get(&edge.id()).unwrap();
        match edge.orientation() {
            true => new_edge.clone(),
            false => new_edge.inverse(),
        }
    };
    let create_boundary =
        |wire: &Wire<Point3, C>| -> Wire<_, _> { wire.edge_iter().map(create_edge).collect() };
    let create_face = move |face: &Face<Point3, C, S>| -> Face<_, _, _> {
        let wires: Vec<_> = face
            .absolute_boundaries()
            .iter()
            .map(create_boundary)
            .collect();
        // SolidCraft: a face is divided like the coarsest of its edges.
        let level = face
            .absolute_boundaries()
            .iter()
            .flat_map(Wire::edge_iter)
            .filter_map(|edge| edge_map.get(&edge.id()).map(|(_, level)| *level))
            .max()
            .unwrap_or(0);
        shell_create_polygon(
            &face.surface(),
            wires,
            face.orientation(),
            tol.relaxed(level),
            &sp,
            cap,
        )
    };
    shell.face_par_iter().map(create_face).collect()
}

/// SolidCraft: bounded work for the angle bound. A face bounded by more than
/// `Deflection::MAX_ANGLE_BOUNDARY` segments has its curved edges divided again at twice the
/// angle, as often as it takes, down to the chord tolerance alone (a plate with a thousand
/// small holes is not worth a hundred times the work to draw each hole round). `polys` holds
/// each edge's polyline and how often its angle was doubled. Every edge still has one
/// polyline for both its faces, and the outcome does not depend on the order of the faces.
fn relax_crowded_faces<C, S>(
    shell: &Shell<Point3, C, S>,
    polys: &mut HashMap<EdgeID<C>, (PolylineCurve, u8)>,
    tol: Deflection,
) where
    C: PolylineableCurve,
{
    if !tol.angular() {
        return;
    }
    let faces: Vec<Vec<Edge<Point3, C>>> = shell
        .face_iter()
        .map(|face| {
            let wires = face.absolute_boundaries();
            wires.iter().flat_map(Wire::edge_iter).cloned().collect()
        })
        .collect();
    for level in 1..=Deflection::MAX_RELAX {
        let segments = |edge: &Edge<Point3, C>| match polys.get(&edge.id()) {
            // Straight at the full angle: nothing to relax, and no say in how its faces are divided.
            Some((poly, 0)) if poly.len() <= 2 => (1, false),
            Some((poly, done)) => (poly.len().saturating_sub(1), *done < level),
            None => (0, false),
        };
        let crowded: Vec<&Edge<Point3, C>> = faces
            .iter()
            .filter(|edges| {
                edges.iter().map(|edge| segments(edge).0).sum::<usize>()
                    > Deflection::MAX_ANGLE_BOUNDARY
            })
            .flatten()
            .filter(|edge| segments(edge).1)
            .collect();
        if crowded.is_empty() {
            break;
        }
        let relaxed = tol.relaxed(level);
        for edge in crowded {
            if let Some(slot) = polys.get_mut(&edge.id()).filter(|(_, done)| *done < level) {
                let curve = edge.curve();
                *slot = (bounded_polyline(&curve, curve.range_tuple(), relaxed), level);
            }
        }
    }
}

/// Tessellates faces
#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn shell_tessellation_single_thread<'a, C, S>(
    shell: &'a Shell<Point3, C, S>,
    tol: Deflection,
    sp: impl SP<S>,
) -> MeshedShell
where
    C: PolylineableCurve + 'a,
    S: PreMeshableSurface + 'a,
{
    use truck_base::entry_map::FxEntryMap as EntryMap;
    use truck_topology::Vertex as TVertex;
    let cap = face_cap(shell.len());
    // SolidCraft: the polylines first (see `shell_tessellation`).
    let mut polys: HashMap<_, _> = shell
        .edge_iter()
        .map(|edge| {
            let curve = edge.curve();
            (edge.id(), (bounded_polyline(&curve, curve.range_tuple(), tol), 0))
        })
        .collect();
    relax_crowded_faces(shell, &mut polys, tol);
    let polys = &polys;
    let mut vmap = EntryMap::new(
        move |v: &TVertex<Point3>| v.id(),
        move |v| v.mapped(Point3::clone),
    );
    let mut edge_map = EntryMap::new(
        move |edge: &'a Edge<Point3, C>| edge.id(),
        move |edge| {
            let vf = edge.absolute_front();
            let v0 = vmap.entry_or_insert(vf).clone();
            let vb = edge.absolute_back();
            let v1 = vmap.entry_or_insert(vb).clone();
            let (poly, _) = polys.get(&edge.id()).unwrap().clone();
            Edge::new_unchecked(&v0, &v1, poly)
        },
    );
    let mut create_edge = move |edge: &'a Edge<Point3, C>| -> Edge<_, _> {
        let new_edge = edge_map.entry_or_insert(edge);
        match edge.orientation() {
            true => new_edge.clone(),
            false => new_edge.inverse(),
        }
    };
    let mut create_boundary = move |wire: &'a Wire<Point3, C>| -> Wire<_, _> {
        wire.edge_iter().map(&mut create_edge).collect()
    };
    let create_face = move |face: &'a Face<Point3, C, S>| -> Face<_, _, _> {
        let wires: Vec<_> = face
            .absolute_boundaries()
            .iter()
            .map(&mut create_boundary)
            .collect();
        let level = face
            .absolute_boundaries()
            .iter()
            .flat_map(Wire::edge_iter)
            .filter_map(|edge| polys.get(&edge.id()).map(|(_, level)| *level))
            .max()
            .unwrap_or(0);
        shell_create_polygon(
            &face.surface(),
            wires,
            face.orientation(),
            tol.relaxed(level),
            &sp,
            cap,
        )
    };
    shell.face_iter().map(create_face).collect()
}

/// Tessellates faces
pub(super) fn cshell_tessellation<'a, C, S>(
    shell: &CompressedShell<Point3, C, S>,
    tol: f64,
    sp: impl SP<S>,
) -> MeshedCShell
where
    C: PolylineableCurve + 'a,
    S: PreMeshableSurface + 'a,
{
    let vertices = shell.vertices.clone();
    let cap = face_cap(shell.faces.len());
    let tol = Deflection::chord(tol);
    let tessellate_edge = |edge: &CompressedEdge<C>| {
        let curve = &edge.curve;
        CompressedEdge {
            vertices: edge.vertices,
            curve: bounded_polyline(curve, curve.range_tuple(), tol),
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let edges: Vec<_> = shell.edges.par_iter().map(tessellate_edge).collect();
    #[cfg(target_arch = "wasm32")]
    let edges: Vec<_> = shell.edges.iter().map(tessellate_edge).collect();
    let tessellate_face = |face: &CompressedFace<S>| {
        let boundaries = face.boundaries.clone();
        let wrapped = closed::Closed::new(&face.surface);
        let search = |s: &closed::Closed<'_, S>, point, hint: Option<(f64, f64)>, accept| {
            sp(s.base, point, hint.map(|p| s.wrap(p)), accept)
        };
        let surface = &wrapped;
        let create_edge = |edge_idx: &CompressedEdgeIndex| match edge_idx.orientation {
            true => Some(edges.get(edge_idx.index)?.curve.clone()),
            false => Some(edges.get(edge_idx.index)?.curve.inverse()),
        };
        let create_boundary = |wire: &Vec<CompressedEdgeIndex>| {
            let wire_iter = wire.iter().filter_map(create_edge);
            PolyBoundaryPiece::try_new(surface, wire_iter, &search, tol.chord)
        };
        let preboundary: Option<Vec<_>> = boundaries.iter().map(create_boundary).collect();
        let polygon: Option<PolygonMesh> = (|| {
            let boundary = PolyBoundary::new(preboundary?, &surface, tol);
            Some(trimming_tessellation(&surface, &boundary, tol, cap))
        })();
        CompressedFace {
            boundaries,
            orientation: face.orientation,
            surface: polygon,
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let faces = shell.faces.par_iter().map(tessellate_face).collect();
    #[cfg(target_arch = "wasm32")]
    let faces = shell.faces.iter().map(tessellate_face).collect();
    MeshedCShell {
        vertices,
        edges,
        faces,
    }
}

fn shell_create_polygon<S: PreMeshableSurface>(
    surface: &S,
    wires: Vec<Wire<Point3, PolylineCurve>>,
    orientation: bool,
    tol: Deflection,
    sp: impl SP<S>,
    cap: usize,
) -> Face<Point3, PolylineCurve, Option<PolygonMesh>> {
    let wrapped = closed::Closed::new(surface);
    let search = |s: &closed::Closed<'_, S>, point, hint: Option<(f64, f64)>, accept| {
        sp(s.base, point, hint.map(|p| s.wrap(p)), accept)
    };
    let surface = &wrapped;
    let preboundary = wires
        .iter()
        .map(|wire: &Wire<_, _>| {
            let wire_iter = wire.iter().map(Edge::oriented_curve);
            PolyBoundaryPiece::try_new(surface, wire_iter, &search, tol.chord)
        })
        .collect::<Option<Vec<_>>>();
    let polygon: Option<PolygonMesh> = (|| {
        let boundary = PolyBoundary::new(preboundary?, &surface, tol);
        let mut mesh = trimming_tessellation(surface, &boundary, tol, cap);
        // Display and export meshes only, as the ruling grid lines: the meshes Boolean
        // operations intersect keep truck's points.
        if tol.rulings {
            let points: Vec<Point3> = wires.iter().flat_map(Wire::edge_iter).flat_map(|e| e.curve().0).collect();
            onto_edges(&mut mesh, &boundary, &points, tol.chord);
        }
        Some(mesh)
    })();
    // SolidCraft: faces with seam edges (a loop running along an edge both ways, as STEP
    // writes cylinders) are valid here; `debug_new` asserts simple loops in debug builds.
    let mut new_face = Face::new_unchecked(wires, polygon);
    if !orientation {
        new_face.invert();
    }
    new_face
}

/// SolidCraft: a face's mesh puts every point where its own surface passes the point's
/// parameters, and a boundary point's parameters are those its edge point projects to. An edge
/// that lies on a surface only within tolerance (a sample of an intersection curve whose
/// projection did not converge, an imported edge) then puts the point it shares with the next
/// face in a different place on each face, and the shell's mesh cracks there. Each vertex at a
/// point of the boundary loops (the same parameters, as the triangulation got them) that is
/// farther than `TOLERANCE` from the nearest point of the face's edges goes to that point, when
/// it is within `tol`, so the faces of an edge have the very same points along it. Within
/// `TOLERANCE` the points are already the same, and a face keeps them on its surface: grid
/// points can lie on a boundary between its points (the ruling lines of a plane), and moving
/// only the boundary points would bend the boundary there.
fn onto_edges(mesh: &mut PolygonMesh, boundary: &PolyBoundary, points: &[Point3], tol: f64) {
    let key = |x: f64, y: f64| [spade_round(x).to_bits(), spade_round(y).to_bits()];
    let loops: rustc_hash::FxHashSet<[u64; 2]> = boundary.0.iter().flatten().map(|p| key(p.x, p.y)).collect();
    let on_loops: Vec<bool> = mesh.uv_coords().iter().map(|uv| loops.contains(&key(uv.x, uv.y))).collect();
    let cell = |p: Point3| [p.x, p.y, p.z].map(|x| (x / tol).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<Point3>> = HashMap::default();
    points.iter().for_each(|p| grid.entry(cell(*p)).or_default().push(*p));
    for (p, _) in mesh.positions_mut().iter_mut().zip(on_loops).filter(|(_, on)| *on) {
        let c = cell(*p);
        let mut best = (tol * tol, None);
        for d in (0..27).map(|k| [k % 3 - 1, k / 3 % 3 - 1, k / 9 - 1]) {
            let near = [c[0].saturating_add(d[0]), c[1].saturating_add(d[1]), c[2].saturating_add(d[2])];
            for q in grid.get(&near).into_iter().flatten() {
                let d2 = q.distance2(*p);
                if d2 <= best.0 {
                    best = (d2, Some(*q));
                }
            }
        }
        if let Some(q) = best.1.filter(|_| best.0 > TOLERANCE2) {
            *p = q;
        }
    }
}

#[derive(Debug, Default, Clone)]
struct PolyBoundaryPiece(Vec<Point2>);

impl PolyBoundaryPiece {
    fn try_new<S: PreMeshableSurface>(
        surface: &S,
        wire: impl Iterator<Item = PolylineCurve>,
        sp: impl SP<S>,
        accept: f64,
    ) -> Option<Self> {
        let (up, vp) = (surface.u_period(), surface.v_period());
        let (urange, vrange) = surface.try_range_tuple();
        let bdry3d: Vec<Point3> = wire
            .flat_map(|poly_edge| {
                let n = poly_edge.len() - 1;
                poly_edge.into_iter().take(n)
            })
            .collect();
        let walk = |mut bdry3d: Vec<Point3>| -> Option<Vec<Point2>> {
            bdry3d.push(bdry3d[0]);
            let mut previous = None;
            bdry3d
                .into_iter()
                .flat_map(|pt| {
                    let (mut u, mut v) = match sp(surface, pt, previous, accept) {
                        // SolidCraft: validate exact and nearest results alike, once per
                        // boundary point, including the bounded provisional preview.
                        Some(hint) => sane(surface, pt, hint),
                        None => return vec![None],
                    };
                    if let (Some(up), Some((u0, _))) = (up, previous) {
                        u = get_mindiff(u, u0, up);
                    }
                    if let (Some(vp), Some((_, v0))) = (vp, previous) {
                        v = get_mindiff(v, v0, vp);
                    }
                    let res = (|| {
                        if let Some((u0, v0)) = previous {
                            if !u0.near(&u) && surface.uder(u0, v0).so_small() {
                                return vec![Some(Point2::new(u, v0)), Some(Point2::new(u, v))];
                            } else if !v0.near(&v) && surface.vder(u0, v0).so_small() {
                                return vec![Some(Point2::new(u0, v)), Some(Point2::new(u, v))];
                            }
                        }
                        vec![Some(Point2::new(u, v))]
                    })();
                    previous = Some((u, v));
                    res
                })
                .collect::<Option<Vec<Point2>>>()
        };
        // SolidCraft: each point is searched from the one before it, but the first has no
        // hint, and a search without one can miss a point that lies on the surface (a corner of
        // a fillet strip on a surface extended past it, #882): the whole face was then left
        // unmeshed. When the walk fails, it is tried once more from the first point an unhinted
        // search finds. A walk that succeeds is kept as it is (where a loop starts decides how
        // it is laid out across a periodic surface's seam).
        let mut vec = match walk(bdry3d.clone()) {
            Some(vec) => vec,
            None => {
                let start = bdry3d.iter().skip(1).position(|pt| sp(surface, *pt, None, accept).is_some())? + 1;
                let mut rotated = bdry3d;
                rotated.rotate_left(start);
                walk(rotated)?
            }
        };
        let grav = vec.iter().fold(Point2::origin(), |g, p| g + p.to_vec()) / vec.len() as f64;
        if let (Some(up), Some((u0, _))) = (up, urange) {
            let quot = f64::floor((grav.x - u0) / up);
            vec.iter_mut().for_each(|p| p.x -= quot * up);
        }
        if let (Some(vp), Some((v0, _))) = (vp, vrange) {
            let quot = f64::floor((grav.y - v0) / vp);
            vec.iter_mut().for_each(|p| p.y -= quot * vp);
        }
        let last = *vec.last().unwrap();
        if !vec[0].near(&last) {
            let Point2 { x: u0, y: v0 } = last;
            if surface.uder(u0, v0).so_small() || surface.vder(u0, v0).so_small() {
                vec.push(vec[0]);
            }
        }
        Some(Self(vec))
    }
}

fn abs_diff(previous: f64) -> impl Fn(&f64, &f64) -> std::cmp::Ordering {
    let f = move |x: &f64| f64::abs(x - previous);
    move |x: &f64, y: &f64| f(x).partial_cmp(&f(y)).unwrap()
}
fn get_mindiff(u: f64, u0: f64, up: f64) -> f64 {
    let closure = |i| u + i as f64 * up;
    (-2..=2).map(closure).min_by(abs_diff(u0)).unwrap()
}

/// SolidCraft: the second field holds each loop's parameter box, so `include` skips loops that
/// cannot hold the point (a face with hundreds of holes meshed in quadratic time). The third
/// holds, for each loop, the box of each run of [`RUN`] segments in a row, so `include` tests
/// only the runs its ray can meet (a long trim loop is otherwise crossed segment by segment
/// for every grid point and every triangle).
#[derive(Debug, Default, Clone)]
struct PolyBoundary(Vec<Vec<Point2>>, Vec<BoundingBox<Point2>>, Vec<Vec<BoundingBox<Point2>>>);

/// Segments per run in [`PolyBoundary`]'s third field.
const RUN: usize = 32;

/// The box of each run of [`RUN`] segments of a closed loop (segment `i` joins point `i` to
/// point `i + 1`, the last back to the first).
fn run_boxes(curve: &[Point2]) -> Vec<BoundingBox<Point2>> {
    let n = curve.len();
    (0..n.div_ceil(RUN))
        .map(|k| (k * RUN..((k + 1) * RUN).min(n)).flat_map(|i| [curve[i], curve[(i + 1) % n]]).collect())
        .collect()
}

/// Whether the ray from `c` along `r`, started `margin` behind `c`, can meet `bdb` grown by
/// `margin` on every side. A segment the ray crosses (or passes within the tolerance of `c`)
/// has its crossing point in its box, so a run whose box the ray misses changes nothing.
fn ray_meets_box(c: Point2, r: Vector2, bdb: &BoundingBox<Point2>, margin: f64) -> bool {
    let (min, max) = (bdb.min(), bdb.max());
    let (mut t0, mut t1) = (-margin, f64::INFINITY);
    for i in 0..2 {
        let (lo, hi) = (min[i] - margin, max[i] + margin);
        if r[i].abs() < 1e-300 {
            if c[i] < lo || c[i] > hi {
                return false;
            }
            continue;
        }
        let (a, b) = ((lo - c[i]) / r[i], (hi - c[i]) / r[i]);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return false;
        }
    }
    true
}

fn normalize_range(curve: &mut Vec<Point2>, compidx: usize, (u0, u1): (f64, f64)) {
    let p = curve[0];
    let q = curve[curve.len() - 1];
    let tmp = f64::min(p[compidx], q[compidx]) + TOLERANCE;
    let del = f64::floor((tmp - u0) / (u1 - u0)) * (u1 - u0);
    curve.iter_mut().for_each(|p| p[compidx] -= del);
    let Some(i) = curve
        .iter()
        .position(|p| (curve[0][compidx] - u1) * (p[compidx] - u1) < 0.0)
    else {
        return;
    };
    let mut curve1 = curve.split_off(i + 1);
    curve1.pop();
    curve1.insert(0, curve[i]);
    match curve[0][compidx] < curve[curve.len() - 1][compidx] {
        true => curve1.iter_mut(),
        false => curve.iter_mut(),
    }
    .for_each(|p| p[compidx] -= u1 - u0);
    curve1.append(curve);
    *curve = curve1;
}

fn loop_orientation(curve: &[Point2]) -> bool {
    curve
        .iter()
        .circular_tuple_windows()
        .fold(0.0, |sum, (p, q)| sum + (q.x + p.x) * (q.y - p.y))
        > 0.0
}

impl PolyBoundary {
    fn new(pieces: Vec<PolyBoundaryPiece>, surface: &impl PreMeshableSurface, tol: Deflection) -> Self {
        Self::with_steps(pieces, surface, tol, None)
    }
    // SolidCraft: only the disposable preview supplies fixed seam subdivisions.
    fn with_steps<S: PreMeshableSurface>(pieces: Vec<PolyBoundaryPiece>, surface: &S, tol: Deflection, steps: Option<usize>) -> Self {
        let polyline_on_surface = |surface: &S, p: Point2, q: Point2, tol| {
            if let Some(n) = steps {
                (0..=n).map(|i| p + (q - p) * (i as f64 / n as f64)).collect()
            } else {
                polyline_on_surface(surface, p, q, tol)
            }
        };
        let (mut closed, mut open) = (Vec::new(), Vec::new());
        pieces.into_iter().for_each(|PolyBoundaryPiece(mut vec)| {
            match vec[0].distance(vec[vec.len() - 1]) < 1.0e-3 {
                true => {
                    vec.pop();
                    closed.push(vec)
                }
                false => open.push(vec),
            }
        });
        fn connect_edges(vecs: impl IntoIterator<Item = Vec<Point2>>) -> Vec<Point2> {
            let closure = |vec: Vec<Point2>| {
                let len = vec.len();
                vec.into_iter().take(len - 1)
            };
            vecs.into_iter().flat_map(closure).collect()
        }
        match open.len() {
            1 => {
                let mut curve = open.pop().unwrap();
                let p = curve[0];
                let q = curve[curve.len() - 1];
                if let (Some((u0, u1)), Some((v0, v1))) = surface.try_range_tuple() {
                    if p.x < q.x - TOLERANCE {
                        normalize_range(&mut curve, 0, (u0, u1));
                        let p = curve[0];
                        let q = curve[curve.len() - 1];
                        let x = Point2::new(u0, v1);
                        let y = Point2::new(u1, v1);
                        let vec0 = polyline_on_surface(surface, q, y, tol);
                        let vec1 = polyline_on_surface(surface, y, x, tol);
                        let vec2 = polyline_on_surface(surface, x, p, tol);
                        closed.push(connect_edges([vec0, vec1, vec2, curve]));
                    } else if q.x < p.x - TOLERANCE {
                        normalize_range(&mut curve, 0, (u0, u1));
                        let p = curve[0];
                        let q = curve[curve.len() - 1];
                        let x = Point2::new(u1, v0);
                        let y = Point2::new(u0, v0);
                        let vec0 = polyline_on_surface(surface, q, y, tol);
                        let vec1 = polyline_on_surface(surface, y, x, tol);
                        let vec2 = polyline_on_surface(surface, x, p, tol);
                        closed.push(connect_edges([vec0, vec1, vec2, curve]));
                    } else if p.y < q.y - TOLERANCE {
                        normalize_range(&mut curve, 1, (v0, v1));
                        let p = curve[0];
                        let q = curve[curve.len() - 1];
                        let x = Point2::new(u0, v0);
                        let y = Point2::new(u0, v1);
                        let vec0 = polyline_on_surface(surface, q, y, tol);
                        let vec1 = polyline_on_surface(surface, y, x, tol);
                        let vec2 = polyline_on_surface(surface, x, p, tol);
                        closed.push(connect_edges([vec0, vec1, vec2, curve]));
                    } else if q.y < p.y - TOLERANCE {
                        normalize_range(&mut curve, 1, (v0, v1));
                        let p = curve[0];
                        let q = curve[curve.len() - 1];
                        let x = Point2::new(u1, v1);
                        let y = Point2::new(u1, v0);
                        let vec0 = polyline_on_surface(surface, q, y, tol);
                        let vec1 = polyline_on_surface(surface, y, x, tol);
                        let vec2 = polyline_on_surface(surface, x, p, tol);
                        closed.push(connect_edges([vec0, vec1, vec2, curve]));
                    }
                }
            }
            2 => {
                let mut curve1 = open.pop().unwrap();
                let mut curve0 = open.pop().unwrap();
                fn end_pts<T: Copy>(vec: &[T]) -> (T, T) { (vec[0], vec[vec.len() - 1]) }
                let ((p0, p1), (q0, q1)) = (end_pts(&curve0), end_pts(&curve1));
                if !p0.x.near(&p1.x) && !q0.x.near(&q1.x) {
                    if let (Some(urange), _) = surface.try_range_tuple() {
                        normalize_range(&mut curve0, 0, urange);
                        normalize_range(&mut curve1, 0, urange);
                    }
                } else if !p0.y.near(&p1.y) && !q0.y.near(&q1.y) {
                    if let (_, Some(vrange)) = surface.try_range_tuple() {
                        normalize_range(&mut curve0, 1, vrange);
                        normalize_range(&mut curve1, 1, vrange);
                    }
                }
                let ((p0, p1), (q0, q1)) = (end_pts(&curve0), end_pts(&curve1));
                let vec0 = polyline_on_surface(surface, p1, q0, tol);
                let vec1 = polyline_on_surface(surface, q1, p0, tol);
                closed.push(connect_edges([curve0, vec0, curve1, vec1]));
            }
            _ => {}
        }
        if !closed.iter().any(|curve| loop_orientation(curve)) {
            if let (Some((u0, u1)), Some((v0, v1))) = surface.try_range_tuple() {
                let p = [
                    Point2::new(u0, v0),
                    Point2::new(u1, v0),
                    Point2::new(u1, v1),
                    Point2::new(u0, v1),
                ];
                let vec0 = polyline_on_surface(surface, p[0], p[1], tol);
                let vec1 = polyline_on_surface(surface, p[1], p[2], tol);
                let vec2 = polyline_on_surface(surface, p[2], p[3], tol);
                let vec3 = polyline_on_surface(surface, p[3], p[0], tol);
                closed.push(connect_edges([vec0, vec1, vec2, vec3]));
            }
        }
        let boxes = closed.iter().map(|curve| curve.iter().collect()).collect();
        let runs = closed.iter().map(|curve| run_boxes(curve)).collect();
        Self(closed, boxes, runs)
    }

    /// whether `c` is included in the domain with boundary = `self`.
    fn include(&self, c: Point2) -> bool {
        let t = 2.0 * std::f64::consts::PI * HashGen::hash1(c);
        let r = Vector2::new(f64::cos(t), f64::sin(t));
        // SolidCraft: a closed loop whose box is clear of `c` winds zero times around it, and
        // no crossing with it lies within the tolerance of `c`: it changes nothing here.
        let margin = 2.0 * TOLERANCE;
        let near = |bdb: &BoundingBox<Point2>| {
            let (min, max) = (bdb.min(), bdb.max());
            !(c.x < min.x - margin || c.x > max.x + margin || c.y < min.y - margin || c.y > max.y + margin)
        };
        // Wide enough for every crossing the test below can find: within the tolerance of `c`
        // (the boundary case) and anywhere along the ray, with room for rounding.
        let scale = c.x.abs().max(c.y.abs()).max(1.0);
        let ray_margin = 4.0 * TOLERANCE + 1e-9 * scale;
        self.0
            .iter()
            .zip(&self.1)
            .zip(&self.2)
            .filter(|((_, bdb), _)| near(bdb))
            .flat_map(|((vec, _), runs)| {
                let n = vec.len();
                runs.iter()
                    .enumerate()
                    .filter(move |(_, rb)| ray_meets_box(c, r, rb, ray_margin))
                    .flat_map(move |(k, _)| (k * RUN..((k + 1) * RUN).min(n)).map(move |i| (&vec[i], &vec[(i + 1) % n])))
            })
            .try_fold(0_i32, move |counter, (p0, p1)| {
                let a = p0 - c;
                let b = p1 - c;
                let s0 = r.x * a.y - r.y * a.x; // v times a
                let s1 = r.x * b.y - r.y * b.x; // v times b
                let s2 = a.x * b.y - a.y * b.x; // a times b
                let x = s2 / (s1 - s0);
                if x.so_small() && s0 * s1 < 0.0 {
                    None
                } else if x > 0.0 && s0 <= 0.0 && s1 > 0.0 {
                    Some(counter + 1)
                } else if x > 0.0 && s0 >= 0.0 && s1 < 0.0 {
                    Some(counter - 1)
                } else {
                    Some(counter)
                }
            })
            .map(|counter| counter > 0)
            .unwrap_or(false)
    }

    /// Inserts points and adds constraint into triangulation.
    fn insert_to(&self, triangulation: &mut Cdt) {
        let poly2tri: Vec<_> = self
            .0
            .iter()
            .flatten()
            .map(|pt| {
                let p = [spade_round(pt.x), spade_round(pt.y)];
                triangulation.insert(SPoint2::from(p)).ok()
            })
            .collect();
        let mut prev: Option<usize> = None;
        let mut counter = 0;
        self.0
            .iter()
            .map(Vec::len)
            .flat_map(|len| {
                let range = counter..counter + len;
                counter += len;
                range.circular_tuple_windows()
            })
            .for_each(|(i, j)| {
                let Some(vj) = poly2tri[j] else { return };
                if let Some(p) = prev {
                    let Some(v) = poly2tri[p] else { return };
                    if triangulation.can_add_constraint(v, vj) {
                        triangulation.add_constraint(v, vj);
                        prev = None;
                    }
                } else {
                    let Some(vi) = poly2tri[i] else { return };
                    if triangulation.can_add_constraint(vi, vj) {
                        triangulation.add_constraint(vi, vj);
                    } else {
                        prev = Some(i);
                    }
                }
            });
    }
}

fn spade_round(x: f64) -> f64 {
    match f64::abs(x) < MIN_ALLOWED_VALUE {
        true => 0.0,
        false => x,
    }
}

/// Tessellates one surface trimmed by polyline.
fn trimming_tessellation<S>(surface: &S, polyboundary: &PolyBoundary, tol: Deflection, cap: usize) -> PolygonMesh
where S: PreMeshableSurface {
    trimming_with_steps(surface, polyboundary, tol, cap, None)
}

/// Fixed-size SolidCraft preview path; regular tessellation retains its adaptive policy.
pub(super) fn preview_face<S: RobustMeshableSurface>(surface: &S, loops: &[Vec<Point3>], steps: usize) -> Option<PolygonMesh> {
    if !(1..=16).contains(&steps) || loops.is_empty()
        || loops.iter().any(|w| w.len() < 3)
        || loops.iter().map(Vec::len).sum::<usize>() > 4096 {
        return None;
    }
    let wrapped = closed::Closed::new(surface);
    let search = |s: &closed::Closed<'_, S>, point, hint: Option<(f64, f64)>, accept| {
        nearest_projection(s.base, point, hint.map(|p| s.wrap(p)), accept, 16)
    };
    let surface = &wrapped;
    let pieces = loops.iter().map(|w| {
        PolyBoundaryPiece::try_new(surface, std::iter::once(PolylineCurve(w.clone())), search, 1e-3)
    }).collect::<Option<Vec<_>>>()?;
    let boundary = PolyBoundary::with_steps(pieces, surface, Deflection::chord(1e-3), Some(steps));
    // Projection can duplicate points at singularities; periodic seams add at most
    // eight fixed polylines. Reject non-finite parameters before spade sees them.
    if boundary.0.iter().map(Vec::len).sum::<usize>() > 8192 + 8 * (steps + 1)
        || boundary.0.iter().flatten().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return None;
    }
    Some(trimming_with_steps(surface, &boundary, Deflection::chord(1e-3), (steps + 1).pow(2), Some(steps)))
}

// SolidCraft: the normal entry point supplies None, preserving adaptive meshing.
fn trimming_with_steps<S: PreMeshableSurface>(surface: &S, polyboundary: &PolyBoundary, tol: Deflection, cap: usize, steps: Option<usize>) -> PolygonMesh {
    let mut triangulation = Cdt::new();
    polyboundary.insert_to(&mut triangulation);
    insert_surface(&mut triangulation, surface, polyboundary, tol, cap, steps);
    let mut mesh = triangulation_into_polymesh(
        triangulation.vertices(),
        triangulation.inner_faces(),
        surface,
        polyboundary,
    );
    mesh.make_face_compatible_to_normal();
    mesh
}

/// Inserts parameter divisions into triangulation.
fn insert_surface(
    triangulation: &mut Cdt,
    surface: impl PreMeshableSurface,
    polyline: &PolyBoundary,
    tol: Deflection,
    cap: usize,
    steps: Option<usize>,
) {
    let bdb: BoundingBox<Point2> = polyline.0.iter().flatten().collect();
    let range = ((bdb.min()[0], bdb.max()[0]), (bdb.min()[1], bdb.max()[1]));
    if !(range.0 .0.is_finite() && range.0 .1.is_finite() && range.1 .0.is_finite() && range.1 .1.is_finite()) {
        return;
    }
    // SolidCraft: fixed preview grids skip both adaptive division and balancing.
    let (udiv, vdiv) = if let Some(n) = steps {
        let division = |(a, b): (f64, f64)| (0..=n).map(|i| a * (1. - i as f64 / n as f64) + b * (i as f64 / n as f64)).collect();
        (division(range.0), division(range.1))
    } else {
        let (u, v) = bounded_surface_division(&surface, range, tol, cap);
        let (u_ruled, v_ruled) = (u.len() <= 3, v.len() <= 3);
        let (u, v) = balanced_grid(&surface, u, v, cap);
        let v = with_ruling_vertices(v, polyline, 1, tol.rulings && v_ruled, u.len(), cap);
        let u = with_ruling_vertices(u, polyline, 0, tol.rulings && u_ruled, v.len(), cap);
        let v = with_boundary_lines(v, polyline, 1, tol.rulings && v_ruled && !u_ruled, u.len(), cap);
        let u = with_boundary_lines(u, polyline, 0, tol.rulings && u_ruled && !v_ruled, v.len(), cap);
        (u, v)
    };
    let insert_res: Vec<Vec<Option<_>>> = udiv
        .into_iter()
        .map(|u| {
            vdiv.iter()
                .map(|v| match polyline.include(Point2::new(u, *v)) {
                    true => triangulation.insert(SPoint2::new(u, *v)).ok(),
                    false => None,
                })
                .collect()
        })
        .collect();
    insert_res.windows(2).for_each(|vec| {
        vec[0].windows(2).zip(&vec[1]).for_each(|(a, z)| {
            if let Some(x) = a[0] {
                if let Some(y) = a[1] {
                    if triangulation.can_add_constraint(x, y) {
                        triangulation.add_constraint(x, y);
                    }
                }
                if let Some(z) = z {
                    if triangulation.can_add_constraint(x, *z) {
                        triangulation.add_constraint(x, *z);
                    }
                }
            }
        });
        let idx = vec[0].len() - 1;
        if let (Some(x), Some(y)) = (vec[0][idx], vec[1][idx]) {
            if triangulation.can_add_constraint(x, y) {
                triangulation.add_constraint(x, y);
            }
        }
    });
}

/// SolidCraft: most grid points a face gets (keeps hostile or huge faces bounded).
pub(super) const MAX_GRID: usize = 150_000;
/// SolidCraft: grid points shared by all faces of one shell; many faces get fewer each.
pub(super) const SHELL_GRID_BUDGET: usize = 10_000_000;
/// SolidCraft: most points on one edge's polyline.
const MAX_EDGE_POINTS: usize = 20_000;

/// SolidCraft: the grid budget of each face of a shell with `faces` faces.
pub(super) fn face_cap(faces: usize) -> usize {
    (SHELL_GRID_BUDGET / faces.max(1)).clamp(2_000, MAX_GRID)
}

/// SolidCraft: truck's curve division, bounded: at most `MAX_EDGE_POINTS` points and a
/// bisection depth of 24; a midpoint that does not evaluate (NaN) counts as flat (truck's
/// version recursed on it 100 levels deep, which never finishes). A piece within the chord
/// tolerance is still halved while it turns more than the deflection's angle.
pub(super) fn bounded_polyline<C: PolylineableCurve>(curve: &C, range: (f64, f64), d: Deflection) -> PolylineCurve {
    let (tol, turn2) = (d.chord, d.turn2());
    // A sample that does not evaluate (an intersection curve whose projection fails): an interval
    // end falls back to the curve's approximation so the polyline stays finite; an interior probe
    // is NaN, which counts as flat. Each parameter is evaluated once: a piece carries the points
    // of its ends, and its midpoint, an end of both halves, serves as probe and as end (a point of
    // an intersection curve costs two grid searches).
    let end = |t: f64| curve.try_subs(t).unwrap_or_else(|| curve.approx_subs(t));
    let nan = Point3::new(f64::NAN, f64::NAN, f64::NAN);
    let (p0, p1) = (end(range.0), end(range.1));
    let mut out = vec![p0];
    let mut stack: Vec<(f64, f64, Point3, Point3, usize)> = vec![(range.0, range.1, p0, p1, 0)];
    while let Some((a, b, pa, pb, depth)) = stack.pop() {
        let t = a + (b - a) * 0.4729;
        let chord = pa + (pb - pa) * 0.4729;
        let d2 = curve.try_subs(t).unwrap_or(nan).distance2(chord);
        let tm = 0.5 * (a + b);
        let sm = curve.try_subs(tm);
        let dm = sm.unwrap_or(nan).distance2(pa.midpoint(pb));
        let flat = !(d2 >= tol * tol || dm >= tol * tol);
        let flat = flat && !d.turns(turn2, d2.max(dm), pa.distance2(pb));
        if flat || depth >= 24 || out.len() + stack.len() >= MAX_EDGE_POINTS {
            out.push(pb);
        } else {
            let pm = sm.unwrap_or_else(|| curve.approx_subs(tm));
            // Right half after the left one (stack order).
            stack.push((tm, b, pm, pb, depth + 1));
            stack.push((a, tm, pa, pm, depth + 1));
        }
    }
    PolylineCurve(out)
}

/// SolidCraft: truck's surface division, bounded: stops refining when the grid would exceed
/// `cap` points or after 24 rounds; cells that do not evaluate (NaN) are not refined. A
/// direction within the chord tolerance is still halved where a cell turns more than the
/// deflection's angle along it, while the grid stays within `Deflection::MAX_ANGLE_GRID`.
pub(super) fn bounded_surface_division<S: PreMeshableSurface>(surface: &S, (ur, vr): ((f64, f64), (f64, f64)), d: Deflection, cap: usize) -> (Vec<f64>, Vec<f64>) {
    let (tol, turn2) = (d.chord, d.turn2());
    let (mut udiv, mut vdiv) = (vec![ur.0, ur.1], vec![vr.0, vr.1]);
    for _ in 0..24 {
        let mut fu = vec![false; udiv.len() - 1];
        let mut fv = vec![false; vdiv.len() - 1];
        // What the angle alone asks for.
        let (mut au, mut av) = (fu.clone(), fv.clone());
        for (i, u) in udiv.windows(2).enumerate() {
            for (j, v) in vdiv.windows(2).enumerate() {
                if fu[i] && fv[j] {
                    continue;
                }
                let (p, q) = (0.4729, 0.5271);
                let (u0, v0) = (u[0] * (1.0 - p) + u[1] * p, v[0] * (1.0 - q) + v[1] * q);
                let p0 = surface.subs(u0, v0);
                let pt00 = surface.subs(u[0], v[0]).to_vec();
                let pt01 = surface.subs(u[0], v[1]).to_vec();
                let pt10 = surface.subs(u[1], v[0]).to_vec();
                let pt11 = surface.subs(u[1], v[1]).to_vec();
                let pt = Point3::from_vec(pt00 * (1.0 - p) * (1.0 - q) + pt01 * (1.0 - p) * q + pt10 * p * (1.0 - q) + pt11 * p * q);
                // SolidCraft (#1666): a cell is drawn as two triangles, not as the bilinear blend
                // of its corners. A twisted cell (a bilinear patch is the blend exactly, so `pt`
                // never tells) bows off its corners' plane by a quarter of its twist at the middle,
                // across that plane: a cone's trapezoid between two rulings is flat and needs none.
                let normal = (pt10 - pt00).cross(pt01 - pt00);
                let bow = (pt00 - pt01 - pt10 + pt11).dot(normal) * 0.25;
                let twisted = normal.magnitude2() > 0.0 && bow * bow > tol * tol * normal.magnitude2();
                let far = p0.distance2(pt) > tol * tol || twisted;
                // Which direction bends: refine only that one (a cylinder needs no division
                // along its rulings).
                let (su, sv) = ([surface.subs(u[0], v0), surface.subs(u[1], v0)], [surface.subs(u0, v[0]), surface.subs(u0, v[1])]);
                let alu = su[0].to_vec() * (1.0 - p) + su[1].to_vec() * p;
                let alv = sv[0].to_vec() * (1.0 - q) + sv[1].to_vec() * q;
                let (du, dv) = (p0.distance2(Point3::from_vec(alu)), p0.distance2(Point3::from_vec(alv)));
                let bend_u = du > tol * tol;
                let bend_v = dv > tol * tol;
                if bend_u || bend_v {
                    fu[i] |= bend_u;
                    fv[j] |= bend_v;
                } else if far {
                    fu[i] = true;
                    fv[j] = true;
                }
                au[i] |= d.turns(turn2, du, su[0].distance2(su[1]));
                av[j] |= d.turns(turn2, dv, sv[0].distance2(sv[1]));
            }
        }
        let count = |f: &[bool], a: &[bool]| f.iter().zip(a).filter(|(f, a)| **f || **a).count();
        let (nu, nv) = (udiv.len() + count(&fu, &au), vdiv.len() + count(&fv, &av));
        // The angle refines only while the grid stays small: past that the chord tolerance alone.
        let turning = turn2.is_finite() && nu.saturating_mul(nv) <= cap.min(Deflection::MAX_ANGLE_GRID);
        if !turning {
            au.fill(false);
            av.fill(false);
        }
        fu.iter_mut().zip(&au).for_each(|(f, a)| *f |= *a);
        fv.iter_mut().zip(&av).for_each(|(f, a)| *f |= *a);
        let nu = udiv.len() + fu.iter().filter(|x| **x).count();
        let nv = vdiv.len() + fv.iter().filter(|x| **x).count();
        if nu == udiv.len() && nv == vdiv.len() {
            break;
        }
        if nu.saturating_mul(nv) > cap {
            break;
        }
        let refine = |d: &[f64], f: &[bool]| {
            let mut n = vec![d[0]];
            for (w, b) in d.windows(2).zip(f) {
                if *b {
                    n.push(0.5 * (w[0] + w[1]));
                }
                n.push(w[1]);
            }
            n
        };
        udiv = refine(&udiv, &fu);
        vdiv = refine(&vdiv, &fv);
    }
    (udiv, vdiv)
}

/// SolidCraft: refine the parameter grid so its cells are not much longer in one direction than
/// the other. A ruled surface (cylinder, cone) needs no division along its rulings, and the
/// triangles that then join a trimming loop to the far ends of the grid cut through the
/// surface (a cylinder with a hole lost volume this way).
fn balanced_grid(surface: &impl PreMeshableSurface, udiv: Vec<f64>, vdiv: Vec<f64>, cap: usize) -> (Vec<f64>, Vec<f64>) {
    let mid = |d: &[f64]| if d.is_empty() { 0.0 } else { d[d.len() / 2] };
    let (um, vm) = (mid(&udiv), mid(&vdiv));
    let step = |d: &[f64], along_u: bool| -> Vec<f64> {
        d.windows(2)
            .map(|w| {
                let (a, b) = (w[0], w[1]);
                if along_u { surface.subs(a, vm).distance(surface.subs(b, vm)) } else { surface.subs(um, a).distance(surface.subs(um, b)) }
            })
            .collect()
    };
    let (lu, lv) = (step(&udiv, true), step(&vdiv, false));
    let typical = |l: &[f64]| {
        let mut v: Vec<f64> = l.iter().copied().filter(|x| x.is_finite() && *x > 0.0).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if v.is_empty() { None } else { Some(v[v.len() / 2]) }
    };
    let refine = |d: &[f64], l: &[f64], target: Option<f64>| -> Vec<f64> {
        let Some(t) = target.filter(|t| *t > 0.0) else { return d.to_vec() };
        // At most 24 cells along a ruling: enough for holes and trims to get short triangles.
        let total: f64 = l.iter().filter(|x| x.is_finite()).sum();
        let t = t.max(total / 24.0 / 4.0);
        let mut out = Vec::with_capacity(d.len());
        for (i, w) in d.windows(2).enumerate() {
            let n = l.get(i).map(|x| (x / (4.0 * t)).ceil()).filter(|n| n.is_finite()).unwrap_or(1.0).clamp(1.0, 64.0) as usize;
            for k in 0..n {
                out.push(w[0] + (w[1] - w[0]) * k as f64 / n as f64);
            }
        }
        if let Some(last) = d.last() {
            out.push(*last);
        }
        out
    };
    // Only a direction left undivided (a ruling) needs it; curved directions already follow
    // the tolerance.
    let u2 = if udiv.len() <= 3 { refine(&udiv, &lu, typical(&lv)) } else { udiv.clone() };
    let v2 = if vdiv.len() <= 3 { refine(&vdiv, &lv, typical(&lu)) } else { vdiv.clone() };
    let (u2, v2) = if u2.len().saturating_mul(v2.len()) > cap { (udiv, vdiv) } else { (u2, v2) };
    // Never more than MAX_GRID points: thin out evenly (the mesh gets coarser, not endless).
    let total = u2.len().saturating_mul(v2.len());
    if total <= cap {
        return (u2, v2);
    }
    let f = (total as f64 / cap.max(4) as f64).sqrt();
    let thin = |d: Vec<f64>| -> Vec<f64> {
        let n = ((d.len() as f64 / f).ceil() as usize).max(2);
        if d.len() <= n {
            return d;
        }
        (0..n).map(|i| d[i * (d.len() - 1) / (n - 1)]).collect()
    };
    (thin(u2), thin(v2))
}

/// SolidCraft: most grid lines added for vertices on a ruling edge.
const MAX_RULING_LINES: usize = 64;

/// SolidCraft: `div` (the grid along coordinate `axis`, 0 for u and 1 for v) with a line through
/// every vertex that splits a boundary segment running along a ruling (a straight edge of a
/// cylinder or cone split by a boolean). Without one, the triangulation fans out from such a
/// vertex to the far ends of the face, and its triangles cut through the surface (the outer
/// wall of a shelled cylinder came out cut off). Only a direction that needs no division of
/// its own (`ruled`) gets them, while the grid stays within `cap`.
fn with_ruling_vertices(div: Vec<f64>, polyline: &PolyBoundary, axis: usize, ruled: bool, other: usize, cap: usize) -> Vec<f64> {
    let (Some(&first), Some(&last)) = (div.first(), div.last()) else { return div };
    let span = last - first;
    if !ruled || !(span.is_finite() && span > 0.0) {
        return div;
    }
    let across = 1 - axis;
    // The extent of the face across the rulings, to tell a segment along one.
    let (lo, hi) = polyline.0.iter().flatten().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p[across]), hi.max(p[across])));
    let width = hi - lo;
    if !(width.is_finite() && width > 0.0) {
        return div;
    }
    let (same, apart) = (width * 1e-9, span * 1e-6);
    let mut extra: Vec<f64> = Vec::new();
    for w in &polyline.0 {
        for (a, b) in w.iter().circular_tuple_windows() {
            if (a[across] - b[across]).abs() > same {
                continue;
            }
            for t in [a[axis], b[axis]] {
                let inside = t > first + apart && t < last - apart;
                if inside && t.is_finite() && div.iter().chain(&extra).all(|d| (d - t).abs() > apart) {
                    extra.push(t);
                }
            }
            if extra.len() > MAX_RULING_LINES {
                return div;
            }
        }
    }
    if extra.is_empty() || (div.len() + extra.len()).saturating_mul(other) > cap {
        return div;
    }
    let mut out = div;
    out.extend(extra);
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// SolidCraft: `div` (the grid along coordinate `axis`, 0 for u and 1 for v) with a line through
/// every point of the face's boundary that lies inside the face along that axis: the points of a
/// hole cut into a cylinder or a cone, or of a trim that crosses its rulings (an oblique plane
/// through a cone's narrow chamfer). A ruled direction gets few lines (one cell along a cone's
/// short side), and a boundary point between them has no grid point beside it: the triangulation
/// joins it to the far ends of the face, many degrees round, and those triangles cut through the
/// surface (a cone face with a small hole meshed several mm² too large, and the boolean that made
/// it was refused for its volume). Only a direction that needs no division of its own (`ruled`)
/// while the other does gets them (a plane, ruled both ways, maps to its uv space without
/// stretching and needs none), only for a few such points (corners standing into the face, at
/// most [`MAX_BOUNDARY_LINES`]), and while the grid stays within `cap`.
/// SolidCraft: most boundary points inside a ruled face that get grid lines through them.
const MAX_BOUNDARY_LINES: usize = 16;

fn with_boundary_lines(div: Vec<f64>, polyline: &PolyBoundary, axis: usize, ruled: bool, other: usize, cap: usize) -> Vec<f64> {
    let (Some(&first), Some(&last)) = (div.first(), div.last()) else { return div };
    let span = last - first;
    if !ruled || !(span.is_finite() && span > 0.0) {
        return div;
    }
    // Lines closer than this to another add nothing but slivers.
    let apart = span * 1e-3;
    let mut at: Vec<f64> = polyline.0.iter().flatten().map(|p| p[axis]).filter(|t| t.is_finite() && *t > first + apart && *t < last - apart).collect();
    at.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    at.dedup_by(|a, b| (*a - *b).abs() <= apart);
    // Many: a boundary curve running through the face, which the triangulation already follows
    // with short triangles (a cylinder cut by another); lines along it only crowd it.
    if at.len() > MAX_BOUNDARY_LINES {
        return div;
    }
    let extra: Vec<f64> = at.into_iter().filter(|t| div.iter().all(|d| (d - t).abs() > apart)).collect();
    if extra.is_empty() || (div.len() + extra.len()).saturating_mul(other) > cap {
        return div;
    }
    let mut out = div;
    out.extend(extra);
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Converts triangulation into `PolygonMesh`.
fn triangulation_into_polymesh<'a>(
    vertices: VertexIterator<'a, SPoint2, (), CdtEdge<()>, ()>,
    triangles: InnerFaceIterator<'a, SPoint2, (), CdtEdge<()>, ()>,
    surface: &impl ParametricSurface3D,
    polyline: &PolyBoundary,
) -> PolygonMesh {
    let mut positions = Vec::<Point3>::new();
    let mut uv_coords = Vec::<Vector2>::new();
    let mut normals = Vec::<Vector3>::new();
    let vmap: HashMap<_, _> = vertices
        .enumerate()
        .map(|(i, v)| {
            let p = *v.as_ref();
            let uv = Vector2::new(p.x, p.y);
            positions.push(surface.subs(uv[0], uv[1]));
            uv_coords.push(uv);
            normals.push(surface.normal(uv[0], uv[1]));
            (v.fix(), i)
        })
        .collect();
    let tri_faces: Vec<[StandardVertex; 3]> = triangles
        .map(|tri| tri.vertices())
        .filter(|tri| {
            fn sp2cg(p: SPoint2) -> Point2 { Point2::new(p.x, p.y) }
            let tri = array![i => sp2cg(*tri[i].as_ref()); 3];
            let (a, b) = (tri[1] - tri[0], tri[2] - tri[0]);
            let c = tri[0] + (a + b) / 3.0;
            let area = a.x * b.y - a.y * b.x;
            polyline.include(c) && !area.so_small2()
        })
        .map(|tri| {
            let idcs = array![i => vmap[&tri[i].fix()]; 3];
            array![i => [idcs[i], idcs[i], idcs[i]].into(); 3]
        })
        .collect();
    PolygonMesh::debug_new(
        StandardAttributes {
            positions,
            uv_coords,
            normals,
        },
        Faces::from_tri_and_quad_faces(tri_faces, Vec::new()),
    )
}

fn polyline_on_surface(
    surface: impl PreMeshableSurface,
    p: Point2,
    q: Point2,
    tol: Deflection,
) -> Vec<Point2> {
    use truck_geometry::prelude::*;
    let line = Line(p, q);
    let pcurve = PCurve::new(line, surface);
    let (vec, _) = pcurve.parameter_division(pcurve.range_tuple(), tol.chord);
    let vec = halved_while_turning(|t| pcurve.subs(t), vec, tol);
    vec.into_iter().map(|t| line.subs(t)).collect()
}

/// SolidCraft: `params` with every interval halved while the curve turns more over it than the
/// deflection's angle (at most 12 times each, and `MAX_EDGE_POINTS` in all). Unchanged without
/// an angle.
fn halved_while_turning(curve: impl Fn(f64) -> Point3, params: Vec<f64>, d: Deflection) -> Vec<f64> {
    let turn2 = d.turn2();
    if !turn2.is_finite() || params.len() < 2 {
        return params;
    }
    let mut out = vec![params[0]];
    for w in params.windows(2) {
        let mut stack: Vec<(f64, f64, usize)> = vec![(w[0], w[1], 0)];
        while let Some((a, b, depth)) = stack.pop() {
            let (pa, pb, tm) = (curve(a), curve(b), 0.5 * (a + b));
            let sag2 = curve(tm).distance2(pa.midpoint(pb));
            let split = d.turns(turn2, sag2, pa.distance2(pb));
            if split && depth < 12 && out.len() + stack.len() < MAX_EDGE_POINTS {
                stack.push((tm, b, depth + 1));
                stack.push((a, tm, depth + 1));
            } else {
                out.push(b);
            }
        }
    }
    out
}

#[test]
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn par_bench() {
    use std::time::Instant;
    use truck_modeling::*;
    const JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../resources/shape/bottle.json"
    ));
    let solid: Solid = serde_json::from_str(JSON).unwrap();
    let shell = solid.into_boundaries().pop().unwrap();

    let instant = Instant::now();
    (0..100).for_each(|_| {
        let _shell = shell_tessellation(&shell, Deflection::chord(0.01), by_search_parameter);
    });
    println!("{}ms", instant.elapsed().as_millis());

    let instant = Instant::now();
    (0..100).for_each(|_| {
        let _shell = shell_tessellation_single_thread(&shell, Deflection::chord(0.01), by_search_parameter);
    });
    println!("{}ms", instant.elapsed().as_millis());
}

/// `include` before the runs: kept to check the new one answers the same everywhere.
///
/// Not part of the API. Built only for tests, or with the `test-hooks` feature, so the
/// SolidCraft kernel tests can run this check in CI (this crate's own tests do not run there).
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub mod include_test_hooks {
    use super::*;

    fn old_include(b: &PolyBoundary, c: Point2) -> bool {
        let t = 2.0 * std::f64::consts::PI * HashGen::hash1(c);
        let r = Vector2::new(f64::cos(t), f64::sin(t));
        let margin = 2.0 * TOLERANCE;
        let near = |bdb: &BoundingBox<Point2>| {
            let (min, max) = (bdb.min(), bdb.max());
            !(c.x < min.x - margin || c.x > max.x + margin || c.y < min.y - margin || c.y > max.y + margin)
        };
        b.0.iter()
            .zip(&b.1)
            .filter(|(_, bdb)| near(bdb))
            .flat_map(|(vec, _)| vec.iter().circular_tuple_windows())
            .try_fold(0_i32, move |counter, (p0, p1): (&Point2, &Point2)| {
                let a = p0 - c;
                let b = p1 - c;
                let s0 = r.x * a.y - r.y * a.x;
                let s1 = r.x * b.y - r.y * b.x;
                let s2 = a.x * b.y - a.y * b.x;
                let x = s2 / (s1 - s0);
                if x.so_small() && s0 * s1 < 0.0 {
                    None
                } else if x > 0.0 && s0 <= 0.0 && s1 > 0.0 {
                    Some(counter + 1)
                } else if x > 0.0 && s0 >= 0.0 && s1 < 0.0 {
                    Some(counter - 1)
                } else {
                    Some(counter)
                }
            })
            .map(|counter| counter > 0)
            .unwrap_or(false)
    }

    struct Lcg(u64);
    impl Lcg {
        fn unit(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn boundary(loops: Vec<Vec<Point2>>) -> PolyBoundary {
        let boxes = loops.iter().map(|l| l.iter().collect()).collect();
        let runs = loops.iter().map(|l| run_boxes(l)).collect();
        PolyBoundary(loops, boxes, runs)
    }

    /// The point-in-trim test with run boxes answers as the old one did, on random loops
    /// (some on a grid) probed at random, on vertices, on edges and just either side of them.
    /// Returns how many points were checked.
    pub fn include_answers_as_before(seeds: std::ops::Range<u64>) -> std::result::Result<usize, String> {
        let mut checked = 0usize;
        for seed in seeds {
            let mut g = Lcg(seed * 104729 + 3);
            // A wobbly outer loop and a few holes, up to 300 segments each, some on a grid.
            let mut loops = Vec::new();
            for l in 0..1 + (g.unit() * 4.0) as usize {
                let n = 3 + (g.unit() * 300.0) as usize;
                let (cx, cy, rad) = if l == 0 { (0.0, 0.0, 10.0) } else { (g.unit() * 8.0 - 4.0, g.unit() * 8.0 - 4.0, 0.5 + g.unit() * 2.0) };
                let grid = g.unit() < 0.3;
                let mut lp: Vec<Point2> = (0..n)
                    .map(|i| {
                        let a = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
                        let rr = rad * (0.7 + 0.3 * g.unit());
                        let (x, y) = (cx + rr * a.cos(), cy + rr * a.sin());
                        if grid { Point2::new((x * 4.0).round() / 4.0, (y * 4.0).round() / 4.0) } else { Point2::new(x, y) }
                    })
                    .collect();
                if l > 0 {
                    lp.reverse();
                }
                loops.push(lp);
            }
            let b = boundary(loops.clone());
            let mut wrong = None;
            let mut probe = |c: Point2| {
                if wrong.is_none() && b.include(c) != old_include(&b, c) {
                    wrong = Some(c);
                }
                checked += 1;
            };
            for _ in 0..400 {
                probe(Point2::new(g.unit() * 24.0 - 12.0, g.unit() * 24.0 - 12.0));
            }
            for lp in &loops {
                for i in 0..lp.len() {
                    let (p, q) = (lp[i], lp[(i + 1) % lp.len()]);
                    let t = g.unit();
                    let on = p + (q - p) * t;
                    let d = q - p;
                    let nrm = Vector2::new(-d.y, d.x) * (1.0 / d.magnitude().max(1e-300));
                    probe(p);
                    probe(on);
                    probe(on + nrm * (TOLERANCE * 0.5));
                    probe(on + nrm * (TOLERANCE * 3.0));
                    probe(on - nrm * (TOLERANCE * 3.0));
                }
            }
            if let Some(c) = wrong {
                return Err(format!("seed {seed}: point {c:?} answered differently"));
            }
        }
        Ok(checked)
    }

    #[cfg(test)]
    #[test]
    fn hook_passes() {
        let n = include_answers_as_before(0..120);
        assert!(matches!(n, Ok(n) if n > 100_000), "{n:?}");
    }
}
