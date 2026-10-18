#![allow(clippy::many_single_char_names)]

use super::*;
use super::history::Lineage;
use rustc_hash::FxHashMap as HashMap;
use truck_base::cgmath64::*;
use truck_geometry::prelude::IntersectionCurve;
use truck_meshalgo::prelude::*;
use truck_topology::{Vertex, *};

type PolylineCurve = truck_meshalgo::prelude::PolylineCurve<Point3>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapesOpStatus {
    Unknown,
    And,
    Or,
}

impl ShapesOpStatus {
    fn not(self) -> Self {
        match self {
            Self::Unknown => Self::Unknown,
            Self::And => Self::Or,
            Self::Or => Self::And,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BoundaryWire<P, C> {
    wire: Wire<P, C>,
    status: ShapesOpStatus,
}

impl<P, C> BoundaryWire<P, C> {
    #[inline(always)]
    pub fn new(wire: Wire<P, C>, status: ShapesOpStatus) -> Self { Self { wire, status } }
    #[inline(always)]
    pub fn status(&self) -> ShapesOpStatus { self.status }
    #[inline(always)]
    pub fn invert(&mut self) {
        self.wire.invert();
        self.status = self.status.not();
    }
    #[inline(always)]
    pub fn inverse(&self) -> Self {
        Self {
            wire: self.wire.inverse(),
            status: self.status.not(),
        }
    }
}

impl ShapesOpStatus {
    fn from_is_curve<C, S>(curve: &IntersectionCurve<C, S>) -> Option<ShapesOpStatus>
    where
        C: ParametricCurve3D + BoundedCurve,
        S: ParametricSurface3D + SearchNearestParameter<D2, Point = Point3>, {
        let (t0, t1) = curve.range_tuple();
        let t = (t0 + t1) / 2.0;
        let (_, pt0, pt1) = curve.search_triple(t)?;
        let der = curve.leader().der(t);
        let normal0 = curve.surface0().normal(pt0[0], pt0[1]);
        let normal1 = curve.surface1().normal(pt1[0], pt1[1]);
        match normal0.cross(der).dot(normal1) > 0.0 {
            true => Some(ShapesOpStatus::Or),
            false => Some(ShapesOpStatus::And),
        }
    }
}

impl<P, C> std::ops::Deref for BoundaryWire<P, C> {
    type Target = Wire<P, C>;
    #[inline(always)]
    fn deref(&self) -> &Self::Target { &self.wire }
}

impl<P, C> std::ops::DerefMut for BoundaryWire<P, C> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.wire }
}

#[derive(Clone, Debug)]
pub struct Loops<P, C>(Vec<BoundaryWire<P, C>>);
#[derive(Clone, Debug)]
pub struct LoopsStore<P, C>(Vec<Loops<P, C>>);

impl<P, C> std::ops::Deref for Loops<P, C> {
    type Target = Vec<BoundaryWire<P, C>>;
    #[inline(always)]
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl<P, C> std::ops::DerefMut for Loops<P, C> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

impl<P, C> std::ops::Deref for LoopsStore<P, C> {
    type Target = Vec<Loops<P, C>>;
    #[inline(always)]
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl<P, C> std::ops::DerefMut for LoopsStore<P, C> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

impl<P, C> FromIterator<BoundaryWire<P, C>> for Loops<P, C> {
    #[inline(always)]
    fn from_iter<I: IntoIterator<Item = BoundaryWire<P, C>>>(iter: I) -> Self {
        Self(Vec::from_iter(iter))
    }
}

impl<'a, P, C, S> From<&'a Face<P, C, S>> for Loops<P, C> {
    #[inline(always)]
    fn from(face: &'a Face<P, C, S>) -> Loops<P, C> {
        face.absolute_boundaries()
            .iter()
            .map(|wire| BoundaryWire::new(wire.clone(), ShapesOpStatus::Unknown))
            .collect()
    }
}

impl<'a, P: 'a, C: 'a, S: 'a> FromIterator<&'a Face<P, C, S>> for LoopsStore<P, C> {
    fn from_iter<I: IntoIterator<Item = &'a Face<P, C, S>>>(iter: I) -> Self {
        Self(iter.into_iter().map(Loops::from).collect())
    }
}

impl<'a, P, C> IntoIterator for &'a LoopsStore<P, C> {
    type Item = <&'a Vec<Loops<P, C>> as IntoIterator>::Item;
    type IntoIter = <&'a Vec<Loops<P, C>> as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter { self.0.iter() }
}

#[derive(Clone, Debug, Copy, PartialEq)]
enum ParameterKind {
    Front,
    Back,
    Inner(f64),
}

impl ParameterKind {
    fn try_new(t: f64, (t0, t1): (f64, f64)) -> Option<ParameterKind> {
        if t0.near(&t) {
            Some(ParameterKind::Front)
        } else if t1.near(&t) {
            Some(ParameterKind::Back)
        } else if t0 < t && t < t1 {
            Some(ParameterKind::Inner(t))
        } else {
            None
        }
    }
}

/// SolidCraft: how far apart two ends of curves may be and still be one vertex. Intersection
/// curves ending on the same edge (a sphere's curves on a face and on an earlier cut's face, both
/// ending on the edge between those faces) are traced on different meshes, and their ends agree
/// only to a few `TOLERANCE` (1e-6) where the surfaces meet at an angle (#1717).
const END_TOLERANCE: f64 = 10.0 * TOLERANCE;

impl<P: Copy + Tolerance, C: Clone> Loops<P, C> {
    fn search_parameter(&self, pt: P) -> Option<(usize, usize, ParameterKind)>
    where
        P: MetricSpace<Metric = f64>,
        C: BoundedCurve<Point = P> + SearchParameter<D1, Point = P>, {
        // SolidCraft: a point within `END_TOLERANCE` of an edge's end vertex is that end (the
        // nearest such end), told by distance alone before any curve is searched for it. The
        // search below finds a point on a curve only within `TOLERANCE`, so an intersection
        // curve's end 1.5e-6 mm from a vertex was merged into it in one shell's loops (on an edge
        // it lies on) and found on no edge in the other's (where the vertex ends another
        // intersection curve): the vertex stayed two there, the tool face's loop never closed,
        // and the result lost that face (#1717). By distance, both shells make the same merge.
        let end = self
            .iter()
            .enumerate()
            .flat_map(|(i, wire)| wire.iter().enumerate().map(move |(j, edge)| (i, j, edge)))
            .flat_map(|(i, j, edge)| {
                let (front, back) = (edge.absolute_front().point(), edge.absolute_back().point());
                [(i, j, ParameterKind::Front, pt.distance(front)), (i, j, ParameterKind::Back, pt.distance(back))]
            })
            .filter(|&(_, _, _, d)| d < END_TOLERANCE)
            .min_by(|a, b| a.3.total_cmp(&b.3));
        if let Some((i, j, kind, _)) = end {
            return Some((i, j, kind));
        }
        self.iter()
            .enumerate()
            .flat_map(move |(i, wire)| wire.iter().enumerate().map(move |(j, edge)| (i, j, edge)))
            .find_map(|(i, j, edge)| {
                let curve = edge.curve();
                curve.search_parameter(pt, None, 1).and_then(|t| {
                    // SolidCraft: a point at an end of the curve is that end, however far its
                    // parameter is from the end's. The parameter alone is no distance: on a
                    // polyline it counts segments, and an intersection curve ending 2e-7 mm from
                    // another's end (two walls of a glyph meeting a face) read 1.4e-6 before it,
                    // so it was cut into a sliver and the result's shell never closed (#391).
                    let kind = if pt.near(&curve.front()) {
                        ParameterKind::Front
                    } else if pt.near(&curve.back()) {
                        ParameterKind::Back
                    } else {
                        ParameterKind::try_new(t, curve.range_tuple())?
                    };
                    Some((i, j, kind))
                })
            })
    }

    // SolidCraft: `replaced(old, new)` is told of every edge rebuilt on the new vertex.
    fn change_vertex(
        &mut self,
        old_vertex: &Vertex<P>,
        new_vertex: &Vertex<P>,
        emap: &mut HashMap<EdgeID<C>, Edge<P, C>>,
        replaced: &mut impl FnMut(EdgeID<C>, EdgeID<C>),
    ) {
        self.iter_mut()
            .flat_map(|wire| wire.iter_mut())
            .for_each(|edge| {
                let mut new_edge = if edge.absolute_front() == old_vertex {
                    emap.entry(edge.id()).or_insert_with(|| {
                        Edge::new(new_vertex, edge.absolute_back(), edge.curve())
                    })
                } else if edge.absolute_back() == old_vertex {
                    emap.entry(edge.id()).or_insert_with(|| {
                        Edge::new(edge.absolute_front(), new_vertex, edge.curve())
                    })
                } else {
                    return;
                }
                .clone();
                replaced(edge.id(), new_edge.id());
                if !edge.orientation() {
                    new_edge.invert();
                }
                // Remove the edge from the HashMap when it is no longer there because ID reassignment will occur.
                if edge.count() == 1 {
                    emap.remove(&edge.id());
                }
                *edge = new_edge;
            })
    }

    fn swap_edge_into_wire(&mut self, edge_id: EdgeID<C>, new_wire: &Wire<P, C>) {
        self.iter_mut().for_each(|wire| {
            let mut iter = wire.iter().enumerate();
            if let Some((idx, edge)) = iter.find(|(_, edge)| edge.id() == edge_id) {
                if edge.orientation() {
                    wire.swap_edge_into_wire(idx, new_wire.clone());
                } else {
                    wire.swap_edge_into_wire(idx, new_wire.inverse());
                }
            }
        });
    }

    #[inline(always)]
    fn add_independent_loop(&mut self, r#loop: BoundaryWire<P, C>) {
        self.push(r#loop.inverse());
        self.push(r#loop);
    }

    fn add_edge(
        &mut self,
        edge0: Edge<P, C>,
        status: ShapesOpStatus,
    ) -> [Option<(usize, usize)>; 2] {
        self.add_edge_choosing(edge0, status, &|_, _, _| None)
    }

    // SolidCraft: the edge added at the visits `chosen` (found for the same edge in the mirror
    // store, whose wires and edges lie in the same order); the default where they are no visits
    // of its ends.
    fn add_edge_at(&mut self, edge0: Edge<P, C>, status: ShapesOpStatus, chosen: [Option<(usize, usize)>; 2]) -> [Option<(usize, usize)>; 2] {
        self.add_edge_choosing(edge0, status, &|_, at_back, at_front| {
            let held = |visits: &[(usize, usize)], c: Option<(usize, usize)>| c.is_none_or(|v| visits.contains(&v));
            (held(at_back, chosen[0]) && held(at_front, chosen[1])).then_some(chosen)
        })
    }

    // SolidCraft: `pick(loops, at_back, at_front)` may name the visits (wire, edge) the edge is
    // added at, from the visits of a wire to its back and front vertices; `None` leaves it to the
    // default below.
    #[allow(clippy::type_complexity)]
    fn add_edge_choosing(
        &mut self,
        edge0: Edge<P, C>,
        status: ShapesOpStatus,
        pick: &dyn Fn(&Self, &[(usize, usize)], &[(usize, usize)]) -> Option<[Option<(usize, usize)>; 2]>,
    ) -> [Option<(usize, usize)>; 2] {
        // SolidCraft: a vertex several intersection curves end on (the tangent point of two equal
        // cylinders, say) is visited by more than one wire. The edge belongs where both of its
        // ends are: a wire holding the visit at each end. Taking the first visit of each end
        // joined wires that are different regions of the face, and the face came out with a wire
        // visiting a vertex twice.
        let starting_at = |loops: &Self, vertex: &Vertex<P>| -> Vec<(usize, usize)> {
            loops
                .iter()
                .enumerate()
                .flat_map(|(i, wire)| wire.iter().enumerate().filter(|(_, edge)| edge.front() == vertex).map(move |(j, _)| (i, j)))
                .collect()
        };
        let (at_back, at_front) = (starting_at(self, edge0.back()), starting_at(self, edge0.front()));
        let together = at_back
            .iter()
            .find_map(|&(i, j)| at_front.iter().find(|&&(k, _)| k == i).map(|&(_, l)| ((i, j), (i, l))));
        let (a, b) = match (pick(self, &at_back, &at_front), together) {
            (Some([a, b]), _) => (a, b),
            (None, Some((a, b))) => (Some(a), Some(b)),
            (None, None) => (at_back.first().copied(), at_front.first().copied()),
        };
        if let Some((wire_index0, edge_index0)) = a {
            self[wire_index0].rotate_left(edge_index0);
            self[wire_index0].push_front(edge0.clone());
            self[wire_index0].push_back(edge0.inverse());
        }
        match (a, b) {
            (Some((wire_index0, edge_index0)), Some((wire_index1, edge_index1))) => {
                if wire_index0 == wire_index1 {
                    let len = self[wire_index0].len() - 2;
                    let edge_index1 = (len + edge_index1 - edge_index0) % len + 1;
                    let new_wire = self[wire_index0].split_off(edge_index1);
                    self[wire_index0].status = status;
                    self.push(BoundaryWire::new(new_wire, status.not()));
                } else {
                    let mut new_wire0 = self[wire_index1].clone();
                    let mut new_wire1 = new_wire0.split_off(edge_index1);
                    new_wire0.append(&mut self[wire_index0]);
                    new_wire0.append(&mut new_wire1);
                    self[wire_index0] = new_wire0;
                    self.swap_remove(wire_index1);
                }
            }
            (None, Some((wire_index1, edge_index1))) => {
                self[wire_index1].rotate_left(edge_index1);
                self[wire_index1].push_front(edge0.inverse());
                self[wire_index1].push_back(edge0);
            }
            (None, None) => self.push(BoundaryWire::new(
                vec![edge0.inverse(), edge0].into(),
                ShapesOpStatus::Unknown,
            )),
            _ => {}
        }
        [a, b]
    }
}

/// SolidCraft: where an edge leaves a vertex, as a unit vector: along the polyline from its
/// front (or back) vertex to the first point a tenth of its length away, which is steadier
/// than its first segment where the mesh it was traced on is noisy.
fn leaving(edge: &Edge<Point3, PolylineCurve>, from_front: bool) -> Option<Vector3> {
    let curve = edge.curve();
    let n = curve.len();
    if n < 2 {
        return None;
    }
    let total: f64 = (1..n).map(|k| (curve[k] - curve[k - 1]).magnitude()).sum();
    let (origin, order): (Point3, Vec<usize>) = if from_front { (curve[0], (1..n).collect()) } else { (curve[n - 1], (0..n - 1).rev().collect()) };
    let mut found = None;
    for k in order {
        let d = curve[k] - origin;
        found = Some(d);
        if d.magnitude() >= 0.1 * total {
            break;
        }
    }
    found.filter(|d| d.magnitude() > 1e-12 && d.magnitude().is_finite()).map(|d| d.normalize())
}

/// SolidCraft: the counterclockwise angle about `normal` from `from` to `to`, in `[0, 2π)`.
fn sweep(from: Vector3, to: Vector3, normal: Vector3) -> Option<f64> {
    let e1 = from - normal * from.dot(normal);
    if e1.magnitude() < 1e-9 {
        return None;
    }
    let e1 = e1.normalize();
    let e2 = normal.cross(e1);
    let angle = to.dot(e2).atan2(to.dot(e1));
    Some(if angle < 0.0 { angle + 2.0 * std::f64::consts::PI } else { angle })
}

impl Loops<Point3, PolylineCurve> {
    /// SolidCraft: the visit among `visits` (of the vertex `at`) whose corner holds the direction
    /// `d` the new edge leaves in: the wedge swept counterclockwise about `normal` from the
    /// wire's outgoing edge to the way back along its incoming one. `None` when no corner can be
    /// told (a single visit is returned as it is).
    fn corner_holding(&self, visits: &[(usize, usize)], d: Vector3, normal: Vector3) -> Option<(usize, usize)> {
        if let [only] = visits {
            return Some(*only);
        }
        let tau = 2.0 * std::f64::consts::PI;
        visits
            .iter()
            .filter_map(|&(i, j)| {
                let wire = self.get(i)?;
                let out = leaving(wire.get(j)?, true)?;
                let before = wire.get((j + wire.len() - 1) % wire.len())?;
                let back = leaving(before, false)?;
                let wedge = sweep(out, back, normal)?;
                let wedge = if wedge < 1e-6 { tau } else { wedge };
                let at = sweep(out, d, normal)?;
                (at > 1e-6 && at < wedge - 1e-6).then_some(((i, j), wedge))
            })
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(visit, _)| visit)
    }

    /// SolidCraft: [`Loops::add_edge`], with the corner at each end of the edge told by its
    /// direction there (`normals`: the face's normal at its back and front vertices) where a
    /// vertex is visited more than once.
    fn add_edge_by_angle(&mut self, edge0: Edge<Point3, PolylineCurve>, status: ShapesOpStatus, normals: [Option<Vector3>; 2]) -> [Option<(usize, usize)>; 2] {
        let (d_back, d_front) = (leaving(&edge0, false), leaving(&edge0, true));
        let pick = |loops: &Self, at_back: &[(usize, usize)], at_front: &[(usize, usize)]| {
            if at_back.len() < 2 && at_front.len() < 2 {
                return None;
            }
            let a = match (d_back, normals[0]) {
                (Some(d), Some(n)) => loops.corner_holding(at_back, d, n),
                _ => None,
            };
            let b = match (d_front, normals[1]) {
                (Some(d), Some(n)) => loops.corner_holding(at_front, d, n),
                _ => None,
            };
            // A vertex visited more than once whose corner could not be told is left to the default.
            let told = |visits: &[(usize, usize)], c: Option<(usize, usize)>| visits.len() < 2 || c.is_some();
            (told(at_back, a) && told(at_front, b)).then_some([a.or_else(|| at_back.first().copied()), b.or_else(|| at_front.first().copied())])
        };
        self.add_edge_choosing(edge0, status, &pick)
    }
}

impl<P: Copy + Tolerance, C: Clone> LoopsStore<P, C> {
    #[inline(always)]
    fn change_vertex(
        &mut self,
        old_vertex: &Vertex<P>,
        new_vertex: &Vertex<P>,
        emap: &mut HashMap<EdgeID<C>, Edge<P, C>>,
        replaced: &mut impl FnMut(EdgeID<C>, EdgeID<C>),
    ) {
        self.iter_mut()
            .for_each(|loops| loops.change_vertex(old_vertex, new_vertex, emap, replaced));
    }

    #[inline(always)]
    fn swap_edge_into_wire(&mut self, edge_id: EdgeID<C>, new_wire: &Wire<P, C>) {
        self.iter_mut()
            .for_each(|loops| loops.swap_edge_into_wire(edge_id, new_wire))
    }

    fn add_polygon_vertex(
        &mut self,
        loops_index: usize,
        v: &Vertex<P>,
        emap: &mut HashMap<EdgeID<C>, Edge<P, C>>,
    ) -> Option<(usize, usize, ParameterKind)>
    where
        P: MetricSpace<Metric = f64>,
        C: Cut<Point = P> + SearchParameter<D1, Point = P>,
    {
        let pt = v.point();
        let (wire_index, edge_index, kind) = self[loops_index].search_parameter(pt)?;
        match kind {
            ParameterKind::Front => {
                let old_vertex = self[loops_index][wire_index][edge_index]
                    .absolute_front()
                    .clone();
                self.change_vertex(&old_vertex, v, emap, &mut |_, _| {});
            }
            ParameterKind::Back => {
                let old_vertex = self[loops_index][wire_index][edge_index]
                    .absolute_back()
                    .clone();
                self.change_vertex(&old_vertex, v, emap, &mut |_, _| {});
            }
            ParameterKind::Inner(t) => {
                let edge = self[loops_index][wire_index][edge_index].absolute_clone();
                let edge_id = edge.id();
                let (edge0, edge1) = edge.cut_with_parameter(v, t)?;
                let new_wire: Wire<_, _> = vec![edge0, edge1].into();
                self.swap_edge_into_wire(edge_id, &new_wire);
            }
        }
        Some((wire_index, edge_index, kind))
    }
}

impl<C> LoopsStore<Point3, C> {
    fn add_geom_vertex<S>(
        &mut self,
        (loops_index, wire_index, edge_index): (usize, usize, usize),
        v: &Vertex<Point3>,
        kind: ParameterKind,
        another_surface: &S,
        emap: &mut HashMap<EdgeID<C>, Edge<Point3, C>>,
        lineage: &mut Lineage<C, S>,
    ) -> Option<()>
    where
        C: Cut<Point = Point3, Vector = Vector3> + SearchNearestParameter<D1, Point = Point3>,
        S: ParametricSurface3D + SearchNearestParameter<D2, Point = Point3>,
    {
        match kind {
            ParameterKind::Front => {
                let old_vertex = self[loops_index][wire_index][edge_index]
                    .absolute_front()
                    .clone();
                v.set_point(old_vertex.point());
                self.change_vertex(&old_vertex, v, emap, &mut |old, new| lineage.edge_piece(old, new));
            }
            ParameterKind::Back => {
                let old_vertex = self[loops_index][wire_index][edge_index]
                    .absolute_back()
                    .clone();
                v.set_point(old_vertex.point());
                self.change_vertex(&old_vertex, v, emap, &mut |old, new| lineage.edge_piece(old, new));
            }
            ParameterKind::Inner(_) => {
                let curve = self[loops_index][wire_index][edge_index].curve();
                let (pt, t, _) =
                    curve_surface_projection(&curve, None, another_surface, None, v.point(), 100)?;
                v.set_point(pt);
                let edge = self[loops_index][wire_index][edge_index].absolute_clone();
                let edge_id = edge.id();
                let (edge0, edge1) = edge.cut_with_parameter(v, t)?;
                lineage.edge_piece(edge_id, edge0.id());
                lineage.edge_piece(edge_id, edge1.id());
                let new_wire: Wire<_, _> = vec![edge0, edge1].into();
                self.swap_edge_into_wire(edge_id, &new_wire);
            }
        }
        Some(())
    }
}

fn curve_surface_projection<C, S>(
    curve: &C,
    curve_hint: Option<f64>,
    surface: &S,
    surface_hint: Option<(f64, f64)>,
    point: Point3,
    trials: usize,
) -> Option<(Point3, f64, Point2)>
where
    C: ParametricCurve3D + SearchNearestParameter<D1, Point = Point3>,
    S: ParametricSurface3D + SearchNearestParameter<D2, Point = Point3>,
{
    if trials == 0 {
        return None;
    }
    let t = curve.search_nearest_parameter(point, curve_hint, 10)?;
    let pt0 = curve.subs(t);
    let (u, v) = surface.search_nearest_parameter(point, surface_hint, 10)?;
    let pt1 = surface.subs(u, v);
    if point.near(&pt0) && point.near(&pt1) && pt0.near(&pt1) {
        Some((point, t, Point2::new(u, v)))
    } else {
        let l = curve.der(t);
        let n = surface.normal(u, v);
        let t0 = (pt1 - pt0).dot(n) / l.dot(n);
        curve_surface_projection(
            curve,
            Some(t),
            surface,
            Some((u, v)),
            pt0 + t0 * l,
            trials - 1,
        )
    }
}

fn create_independent_loop<P, C, D>(mut poly_curve0: C) -> Wire<P, D>
where
    C: Cut<Point = P>,
    D: From<C>, {
    let (t0, t1) = poly_curve0.range_tuple();
    let t = (t0 + t1) / 2.0;
    let poly_curve1 = poly_curve0.cut(t);
    let v0 = Vertex::new(poly_curve0.front());
    let v1 = Vertex::new(poly_curve1.front());
    let edge0 = Edge::new(&v0, &v1, poly_curve0.into());
    let edge1 = Edge::new(&v1, &v0, poly_curve1.into());
    vec![edge0, edge1].into()
}

#[allow(dead_code)]
pub struct LoopsStoreQuadruple<C> {
    pub geom_loops_store0: LoopsStore<Point3, C>,
    pub poly_loops_store0: LoopsStore<Point3, PolylineCurve>,
    pub geom_loops_store1: LoopsStore<Point3, C>,
    pub poly_loops_store1: LoopsStore<Point3, PolylineCurve>,
}

pub fn create_loops_stores<C, S>(
    geom_shell0: &Shell<Point3, C, S>,
    poly_shell0: &Shell<Point3, PolylineCurve, Option<PolygonMesh>>,
    geom_shell1: &Shell<Point3, C, S>,
    poly_shell1: &Shell<Point3, PolylineCurve, Option<PolygonMesh>>,
    tol: f64,
    lineage: &mut Lineage<C, S>,
) -> Option<LoopsStoreQuadruple<C>>
where
    C: SearchNearestParameter<D1, Point = Point3>
        + SearchParameter<D1, Point = Point3>
        + Cut<Point = Point3, Vector = Vector3>
        + From<IntersectionCurve<PolylineCurve, S>>,
    S: ParametricSurface3D + SearchNearestParameter<D2, Point = Point3>,
{
    let mut geom_loops_store0: LoopsStore<_, _> = geom_shell0.face_iter().collect();
    let mut poly_loops_store0: LoopsStore<_, _> = poly_shell0.face_iter().collect();
    let mut geom_loops_store1: LoopsStore<_, _> = geom_shell1.face_iter().collect();
    let mut poly_loops_store1: LoopsStore<_, _> = poly_shell1.face_iter().collect();
    let store0_len = geom_loops_store0.len();
    let store1_len = geom_loops_store1.len();
    (0..store0_len)
        .flat_map(move |i| (0..store1_len).map(move |j| (i, j)))
        .try_for_each(|(face_index0, face_index1)| {
            let ori0 = geom_shell0[face_index0].orientation();
            let ori1 = geom_shell1[face_index1].orientation();
            let surface0 = geom_shell0[face_index0].surface();
            let surface1 = geom_shell1[face_index1].surface();
            let meeting = [geom_shell0[face_index0].id(), geom_shell1[face_index1].id()];
            let polygon0 = crate::tr!(poly_shell0[face_index0].surface(), "polygon0");
            let polygon1 = crate::tr!(poly_shell1[face_index1].surface(), "polygon1");
            intersection_curve::intersection_curves(
                surface0.clone(),
                &polygon0,
                surface1.clone(),
                &polygon1,
                tol,
            )
            .into_iter()
            .try_for_each(|(polyline, intersection_curve)| {
                let mut intersection_curve = crate::tr!(intersection_curve, "intersection curve").into();
                let status = crate::tr!(ShapesOpStatus::from_is_curve(&intersection_curve), "status");
                let (status0, status1) = match (ori0, ori1) {
                    (true, true) => (status, status.not()),
                    (true, false) => (status.not(), status.not()),
                    (false, true) => (status, status),
                    (false, false) => (status.not(), status),
                };
                if polyline.front().near(&polyline.back()) {
                    let poly_wire = create_independent_loop(polyline);
                    poly_loops_store0[face_index0]
                        .add_independent_loop(BoundaryWire::new(poly_wire.clone(), status0));
                    poly_loops_store1[face_index1]
                        .add_independent_loop(BoundaryWire::new(poly_wire, status1));
                    let geom_wire = create_independent_loop(intersection_curve);
                    geom_wire.iter().for_each(|edge| lineage.meet(edge.id(), meeting));
                    geom_loops_store0[face_index0]
                        .add_independent_loop(BoundaryWire::new(geom_wire.clone(), status0));
                    geom_loops_store1[face_index1]
                        .add_independent_loop(BoundaryWire::new(geom_wire, status1));
                } else {
                    let pv0 = Vertex::new(polyline.front());
                    let pv1 = Vertex::new(polyline.back());
                    let gv0 = Vertex::new(polyline.front());
                    let gv1 = Vertex::new(polyline.back());
                    let mut pemap0 = HashMap::default();
                    let mut pemap1 = HashMap::default();
                    let mut gemap0 = HashMap::default();
                    let mut gemap1 = HashMap::default();
                    let idx00 =
                        poly_loops_store0.add_polygon_vertex(face_index0, &pv0, &mut pemap0);
                    if let Some((wire_index, edge_index, kind)) = idx00 {
                        crate::tr!(geom_loops_store0.add_geom_vertex(
                            (face_index0, wire_index, edge_index),
                            &gv0,
                            kind,
                            &surface1,
                            &mut gemap0,
                            lineage,
                        ), "add_geom_vertex 1");
                        let polyline = intersection_curve.editor().leader;
                        *polyline.first_mut().unwrap() = gv0.point();
                    }
                    let idx01 =
                        poly_loops_store0.add_polygon_vertex(face_index0, &pv1, &mut pemap1);
                    if let Some((wire_index, edge_index, kind)) = idx01 {
                        crate::tr!(geom_loops_store0.add_geom_vertex(
                            (face_index0, wire_index, edge_index),
                            &gv1,
                            kind,
                            &surface1,
                            &mut gemap1,
                            lineage,
                        ), "add_geom_vertex 2");
                        let polyline = intersection_curve.editor().leader;
                        *polyline.last_mut().unwrap() = gv1.point();
                    }
                    let idx10 =
                        poly_loops_store1.add_polygon_vertex(face_index1, &pv0, &mut pemap0);
                    if let Some((wire_index, edge_index, kind)) = idx10 {
                        crate::tr!(geom_loops_store1.add_geom_vertex(
                            (face_index1, wire_index, edge_index),
                            &gv0,
                            kind,
                            &surface0,
                            &mut gemap0,
                            lineage,
                        ), "add_geom_vertex 3");
                        let polyline = intersection_curve.editor().leader;
                        *polyline.first_mut().unwrap() = gv0.point();
                    }
                    let idx11 =
                        poly_loops_store1.add_polygon_vertex(face_index1, &pv1, &mut pemap1);
                    if let Some((wire_index, edge_index, kind)) = idx11 {
                        crate::tr!(geom_loops_store1.add_geom_vertex(
                            (face_index1, wire_index, edge_index),
                            &gv1,
                            kind,
                            &surface0,
                            &mut gemap1,
                            lineage,
                        ), "add_geom_vertex 4");
                        let polyline = intersection_curve.editor().leader;
                        *polyline.last_mut().unwrap() = gv1.point();
                    }
                    let pedge = Edge::new(&pv0, &pv1, polyline);
                    let gedge = Edge::new(&gv0, &gv1, intersection_curve.into());
                    lineage.meet(gedge.id(), meeting);
                    let normal_at = |surface: &S, p: Point3| -> Option<Vector3> {
                        let (u, v) = surface.search_nearest_parameter(p, None, 100)?;
                        Some(surface.normal(u, v)).filter(|n| n.magnitude() > 0.0 && n.magnitude().is_finite())
                    };
                    let normals0 = [normal_at(&surface0, gv1.point()), normal_at(&surface0, gv0.point())];
                    let normals1 = [normal_at(&surface1, gv1.point()), normal_at(&surface1, gv0.point())];
                    let chosen0 = poly_loops_store0[face_index0].add_edge_by_angle(pedge.clone(), status0, normals0);
                    geom_loops_store0[face_index0].add_edge_at(gedge.clone(), status0, chosen0);
                    let chosen1 = poly_loops_store1[face_index1].add_edge_by_angle(pedge, status1, normals1);
                    geom_loops_store1[face_index1].add_edge_at(gedge, status1, chosen1);
                }
                Some(())
            })
        })?;
    Some(LoopsStoreQuadruple {
        geom_loops_store0,
        poly_loops_store0,
        geom_loops_store1,
        poly_loops_store1,
    })
}

#[cfg(test)]
mod tests;
