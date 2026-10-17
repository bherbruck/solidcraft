use crate::*;
use spade::{iterators::*, *};
use truck_topology::{compress::*, *};

#[cfg(not(target_arch = "wasm32"))]
mod parallelizable {
    /// Parallelizable by `rayon`.
    pub trait Parallelizable: Send + Sync {}
    impl<T: Send + Sync> Parallelizable for T {}
}

#[cfg(target_arch = "wasm32")]
mod parallelizable {
    /// No parallelization in the case of wasm.
    pub trait Parallelizable {}
    impl<T> Parallelizable for T {}
}

pub use parallelizable::*;

/// SolidCraft: a disposable preview of a face from already sampled boundary loops.
/// Caps input points and uses a fixed parameter grid, including periodic seams.
/// This is approximate display, never an accuracy-controlled modelling mesh.
pub fn preview_face<S: RobustMeshableSurface>(surface: &S, loops: &[Vec<Point3>], steps: usize) -> Option<PolygonMesh> {
    triangulation::preview_face(surface, loops, steps)
}

/// Periods of a surface, including smooth geometric closure of bounded splines.
/// Meshing wraps inferred periods into the original knot domain before evaluation.
pub fn surface_periods<S: PreMeshableSurface>(surface: &S) -> [Option<f64>; 2] {
    closed::Closed::new(surface).periods()
}

/// Project a trim point with the same bounded, tolerance-aware search used by the mesher.
/// A preceding parameter is a hint, never proof that the search converged correctly.
pub fn project_trim_point<S: RobustMeshableSurface>(surface: &S, point: Point3, hint: Option<(f64, f64)>, tolerance: f64) -> Option<(f64, f64)> {
    triangulation::nearest_projection(surface, point, hint, tolerance, 100)
}

/// SolidCraft: an edge's polyline as the meshers divide it (at most 20 000 points). Exposed for
/// the kernel's regressions in the workspace CI.
#[doc(hidden)]
pub fn bounded_polyline<C: PolylineableCurve>(curve: &C, range: (f64, f64), d: Deflection) -> PolylineCurve {
    triangulation::bounded_polyline(curve, range, d)
}

/// SolidCraft: sampling a curve that may fail to evaluate. truck's `IntersectionCurve::subs`
/// unwraps the projection of its leader's point onto both surfaces, which does not converge
/// everywhere; the mesher samples through this trait so that a failed sample is data, not a panic.
pub trait TrySubs: ParametricCurve3D {
    /// The point at `t`, or `None` where the curve does not evaluate.
    fn try_subs(&self, t: f64) -> Option<Point3> { Some(self.subs(t)) }
    /// A point near the curve at `t` that always exists (an intersection curve's leader).
    fn approx_subs(&self, t: f64) -> Point3 { self.subs(t) }
}

impl TrySubs for truck_geometry::prelude::Line<Point3> {}
impl TrySubs for truck_geometry::prelude::BSplineCurve<Point3> {}
impl TrySubs for truck_geometry::prelude::NurbsCurve<Vector4> {}
impl TrySubs for PolylineCurve {}

impl<C, S> TrySubs for truck_geometry::prelude::IntersectionCurve<C, S>
where
    C: ParametricCurve3D,
    S: ParametricSurface3D + SearchNearestParameter<D2, Point = Point3>,
{
    fn try_subs(&self, t: f64) -> Option<Point3> { self.search_triple(t).map(|triple| triple.0) }
    fn approx_subs(&self, t: f64) -> Point3 { self.leader().subs(t) }
}

impl TrySubs for truck_modeling::Curve {
    fn try_subs(&self, t: f64) -> Option<Point3> {
        match self {
            truck_modeling::Curve::IntersectionCurve(curve) => TrySubs::try_subs(curve, t),
            other => Some(other.subs(t)),
        }
    }
    fn approx_subs(&self, t: f64) -> Point3 {
        match self {
            truck_modeling::Curve::IntersectionCurve(curve) => TrySubs::approx_subs(curve, t),
            other => other.subs(t),
        }
    }
}

/// Gathered the traits used in tessellation.
pub trait PolylineableCurve:
    ParametricCurve3D + BoundedCurve + ParameterDivision1D<Point = Point3> + Parallelizable + TrySubs {
}
impl<
        C: ParametricCurve3D + BoundedCurve + ParameterDivision1D<Point = Point3> + Parallelizable + TrySubs,
    > PolylineableCurve for C
{
}

/// It can be meshed, but not necessarily trimmed.
pub trait PreMeshableSurface: ParametricSurface3D + ParameterDivision2D + Parallelizable {}
impl<S: ParametricSurface3D + ParameterDivision2D + Parallelizable> PreMeshableSurface for S {}

/// The generated mesh can be trimmed only if the boundary curves ride strictly on a surface.
pub trait MeshableSurface: PreMeshableSurface + SearchParameter<D2, Point = Point3> {}
impl<S: PreMeshableSurface + SearchParameter<D2, Point = Point3>> MeshableSurface for S {}

/// The generated mesh can be trimmed if the boundary curves does not ride strictly on a surface.
pub trait RobustMeshableSurface:
    MeshableSurface + SearchNearestParameter<D2, Point = Point3> {
}
impl<S: MeshableSurface + SearchNearestParameter<D2, Point = Point3>> RobustMeshableSurface for S {}

type PolylineCurve = truck_polymesh::PolylineCurve<Point3>;

/// SolidCraft: how finely curves and surfaces are divided into facets. `chord` is the usual
/// tolerance: the most a facet may sag from the true shape. `angle`, when finite, also bounds
/// what one facet spans: a facet that turns more than `angle` is split whatever its size (a
/// radius that is small beside the tolerance would otherwise stay one flat facet), until its
/// sag is below `floor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deflection {
    /// Most a facet may sag from the curve or surface.
    pub chord: f64,
    /// Most a facet may turn (radians: the arc it spans on a circle); infinite for no bound.
    pub angle: f64,
    /// Sag below which `angle` no longer splits a facet.
    pub floor: f64,
    /// SolidCraft: grid lines through the vertices that split a face's ruling edges (see
    /// `with_ruling_vertices`). Meshes for display and export ask for them; the meshes Boolean
    /// operations intersect do not, so a Boolean's result does not depend on them.
    pub rulings: bool,
}

impl Deflection {
    /// Most boundary segments a face may get from the angle bound. Triangulating a face tests
    /// each triangle against the boundary loops around it, so the work grows faster than the
    /// boundary (with its square for one long loop): the curved edges of a face that would
    /// get more are divided at twice the angle, and again, down to the chord tolerance alone.
    pub const MAX_ANGLE_BOUNDARY: usize = 4096;
    /// Most grid points a face may get from the angle bound (beyond them only `chord` refines).
    pub const MAX_ANGLE_GRID: usize = 16_384;
    /// How often the angle is doubled for a crowded face before only `chord` is left.
    pub const MAX_RELAX: u8 = 4;

    /// The chord tolerance alone: what `triangulation(tol)` divides by.
    pub const fn chord(tol: f64) -> Self {
        Self {
            chord: tol,
            angle: f64::INFINITY,
            floor: 0.0,
            rulings: false,
        }
    }

    /// Whether `angle` bounds anything.
    pub fn angular(&self) -> bool { self.angle.is_finite() && self.angle > 0.0 }

    /// The same with the angle doubled `level` times; from [`Deflection::MAX_RELAX`] times, or
    /// past half a turn, the chord tolerance alone.
    pub fn relaxed(&self, level: u8) -> Self {
        let angle = self.angle * f64::from(1u32 << level.min(Self::MAX_RELAX));
        match level == 0 || (level < Self::MAX_RELAX && angle < std::f64::consts::PI) {
            true => Self { angle, ..*self },
            false => Self { rulings: self.rulings, ..Self::chord(self.chord) },
        }
    }

    /// The squared sag over squared length above which a facet turns more than `angle`: a
    /// chord spanning `a` of a circle sags `tan(a / 4) / 2` of its length. Infinite without
    /// an angle bound. SolidCraft: public for the kernel's regressions.
    #[doc(hidden)]
    pub fn turn2(&self) -> f64 {
        match self.angular() && self.angle < std::f64::consts::PI {
            true => (f64::tan(self.angle / 4.0) / 2.0).powi(2),
            false => f64::INFINITY,
        }
    }

    /// Whether the angle bound splits a facet of squared sag `sag2` and squared length `len2`
    /// (`turn2` from [`Deflection::turn2`]). SolidCraft: public for the kernel's regressions.
    #[doc(hidden)]
    pub fn turns(&self, turn2: f64, sag2: f64, len2: f64) -> bool {
        turn2.is_finite() && sag2 > self.floor * self.floor && sag2 > turn2 * len2
    }
}

/// Trait for converting tessellated shape into polygon.
pub trait MeshedShape {
    /// Converts tessellated shape into polygon.
    fn to_polygon(&self) -> PolygonMesh;
}

impl MeshedShape for Shell<Point3, PolylineCurve, PolygonMesh> {
    fn to_polygon(&self) -> PolygonMesh {
        let mut polygon = PolygonMesh::default();
        self.face_iter().for_each(|face| {
            polygon.merge(face.oriented_surface());
        });
        polygon
    }
}

impl MeshedShape for Shell<Point3, PolylineCurve, Option<PolygonMesh>> {
    fn to_polygon(&self) -> PolygonMesh {
        let mut polygon = PolygonMesh::default();
        self.face_iter().for_each(|face| {
            if let Some(mut poly) = face.surface() {
                if !face.orientation() {
                    poly.invert();
                }
                polygon.merge(poly);
            }
        });
        polygon
    }
}

impl<P, C, S> MeshedShape for Solid<P, C, S>
where Shell<P, C, S>: MeshedShape
{
    fn to_polygon(&self) -> PolygonMesh {
        let mut polygon = PolygonMesh::default();
        self.boundaries().iter().for_each(|shell| {
            polygon.merge(shell.to_polygon());
        });
        polygon
    }
}

impl MeshedShape for CompressedShell<Point3, PolylineCurve, PolygonMesh> {
    fn to_polygon(&self) -> PolygonMesh {
        let mut polygon = PolygonMesh::default();
        self.faces.iter().for_each(|face| match face.orientation {
            true => polygon.merge(face.surface.clone()),
            false => polygon.merge(face.surface.inverse()),
        });
        polygon
    }
}

impl MeshedShape for CompressedShell<Point3, PolylineCurve, Option<PolygonMesh>> {
    fn to_polygon(&self) -> PolygonMesh {
        let mut polygon = PolygonMesh::default();
        self.faces.iter().for_each(|face| {
            if let Some(surface) = &face.surface {
                match face.orientation {
                    true => polygon.merge(surface.clone()),
                    false => polygon.merge(surface.inverse()),
                }
            }
        });
        polygon
    }
}

impl<P, C, S> MeshedShape for CompressedSolid<P, C, S>
where CompressedShell<P, C, S>: MeshedShape
{
    fn to_polygon(&self) -> PolygonMesh {
        let mut polygon = PolygonMesh::default();
        self.boundaries.iter().for_each(|shell| {
            polygon.merge(shell.to_polygon());
        });
        polygon
    }
}

/// Trait for tessellating `Shell` and `Solid`.
pub trait MeshableShape {
    /// Shape whose edges are made polylines and faces polygon surface.
    type MeshedShape: MeshedShape;
    /// Tessellates shapes. The division of curves and surfaces are by `ParameterDivision1D` and `ParameterDivision2D`,
    /// and the constrained Delauney triangulation is based on the crate [`spade`](https://crates.io/crates/spade).
    ///
    /// # Panics
    ///
    /// `tol` must be more than `TOLERANCE`.
    ///
    /// # Remarks
    ///
    /// - The tessellated mesh is not necessarily closed even if `self` is `Solid`.
    ///   If you want to get closed mesh, use [`OptimizingFilter::put_together_same_attrs`].
    /// - This method requires that the curve ride strictly on a surface. If not, try [`RobustMeshableShape`].
    ///
    /// [`OptimizingFilter::put_together_same_attrs`]: crate::filters::OptimizingFilter::put_together_same_attrs
    ///
    /// # Examples
    /// ```
    /// use truck_meshalgo::prelude::*;
    /// use truck_modeling::builder;
    /// use truck_topology::shell::ShellCondition;
    ///
    /// // modeling a unit cube
    /// let v = builder::vertex(Point3::origin());
    /// let e = builder::tsweep(&v, Vector3::unit_x());
    /// let f = builder::tsweep(&e, Vector3::unit_y());
    /// let cube = builder::tsweep(&f, Vector3::unit_z());
    ///
    /// // cube is Solid, however, the tessellated mesh is not closed.
    /// let mut mesh = cube.triangulation(0.01).to_polygon();
    /// assert_ne!(mesh.shell_condition(), ShellCondition::Closed);
    ///
    /// // use optimization filters!
    /// mesh.put_together_same_attrs(TOLERANCE);
    /// assert_eq!(mesh.shell_condition(), ShellCondition::Closed);
    /// ```
    fn triangulation(&self, tol: f64) -> Self::MeshedShape;
}

/// Trait for tessellating `Shell` and `Solid` in `truck-modeling`.
pub trait RobustMeshableShape {
    /// Shape whose edges are made polylines and faces polygon surface.
    type MeshedShape: MeshedShape;
    /// Tessellates shapes. The division of curves and surfaces are by `ParameterDivision1D` and `ParameterDivision2D`,
    /// and the constrained Delauney triangulation is based on the crate [`spade`](https://crates.io/crates/spade).
    ///
    /// # Panics
    ///
    /// `tol` must be more than `TOLERANCE`.
    ///
    /// # Remarks
    ///
    /// Since polyline vertices are projected onto the surface, processing speed is often slower than with [`MeshableShape::triangulation`].
    /// It also does not close the mesh of a solid even if one uses [`OptimizingFilter::put_together_same_attrs`].
    ///
    /// [`OptimizingFilter::put_together_same_attrs`]: crate::filters::OptimizingFilter::put_together_same_attrs
    ///
    /// # Examples
    /// ```
    /// use truck_meshalgo::prelude::*;
    /// use truck_modeling::*;
    /// use truck_topology::shell::ShellCondition;
    ///
    /// // manual modeling an open half cylinder
    ///
    /// // points
    /// let p = [
    ///     Point3::new(1.0, 0.0, 0.0),
    ///     Point3::new(-1.0, 0.0, 0.0),
    ///     Point3::new(1.0, 0.0, 1.0),
    ///     Point3::new(-1.0, 0.0, 1.0),
    /// ];
    /// // vertices
    /// let v = Vertex::news(&p);
    /// // Curves that do not ride on a cylinder
    /// let bsp0: Curve = BSplineCurve::new(
    ///     KnotVec::bezier_knot(3),
    ///     vec![
    ///         p[0],
    ///         Point3::new(1.0, 4.0 / 3.0, 0.0),
    ///         Point3::new(-1.0, 4.0 / 3.0, 0.0),
    ///         p[1],
    ///     ],
    /// )
    /// .into();
    /// let bsp1: Curve = BSplineCurve::new(
    ///     KnotVec::bezier_knot(3),
    ///     vec![
    ///         p[3],
    ///         Point3::new(-1.0, 4.0 / 3.0, 1.0),
    ///         Point3::new(1.0, 4.0 / 3.0, 1.0),
    ///         p[2],
    ///     ],
    /// )
    /// .into();
    /// // wire
    /// let w: Wire = vec![
    ///     builder::line(&v[2], &v[0]),
    ///     Edge::new(&v[0], &v[1], bsp0),
    ///     builder::line(&v[1], &v[3]),
    ///     Edge::new(&v[3], &v[2], bsp1),
    /// ]
    /// .into();
    /// // revoluted curve
    /// let surface_raw = RevolutedCurve::by_revolution(
    ///     Curve::Line(Line(p[2], p[0])),
    ///     Point3::origin(),
    ///     Vector3::unit_z(),
    /// );
    /// let surface: Surface = Processor::new(surface_raw).into();
    /// // shell
    /// let shell: Shell = vec![Face::new(vec![w], surface)].into();
    ///
    /// // Simple triangulation fails since some edges do not ride on a cylinder
    /// let poly_shell = shell.triangulation(0.01);
    /// assert!(poly_shell[0].surface().is_none());
    ///
    /// // Robust triangulation!
    /// let poly_shell = shell.robust_triangulation(0.01);
    /// let poly = poly_shell[0].surface().unwrap();
    /// assert!(!poly.positions().is_empty());
    /// ```
    fn robust_triangulation(&self, tol: f64) -> Self::MeshedShape;
}

impl<C: PolylineableCurve, S: MeshableSurface> MeshableShape for Shell<Point3, C, S> {
    type MeshedShape = Shell<Point3, PolylineCurve, Option<PolygonMesh>>;
    fn triangulation(&self, tol: f64) -> Self::MeshedShape {
        nonpositive_tolerance!(tol);
        #[cfg(not(target_arch = "wasm32"))]
        let res = triangulation::shell_tessellation(
            self,
            Deflection::chord(tol),
            triangulation::by_search_parameter,
        );
        #[cfg(target_arch = "wasm32")]
        let res = triangulation::shell_tessellation_single_thread(
            self,
            Deflection::chord(tol),
            triangulation::by_search_parameter,
        );
        res
    }
}

impl<C: PolylineableCurve, S: RobustMeshableSurface> RobustMeshableShape for Shell<Point3, C, S> {
    type MeshedShape = Shell<Point3, PolylineCurve, Option<PolygonMesh>>;
    fn robust_triangulation(&self, tol: f64) -> Self::MeshedShape {
        self.robust_triangulation_within(Deflection::chord(tol))
    }
}

/// SolidCraft: tessellation with a bound on the angle one facet spans as well as on its sag.
pub trait RoundMeshableShape {
    /// Shape whose edges are made polylines and faces polygon surface.
    type MeshedShape: MeshedShape;
    /// [`RobustMeshableShape::robust_triangulation`] divided by `deflection`: with
    /// [`Deflection::chord`] the very same shape; with an angle, curved edges and faces whose
    /// radius is small beside the chord tolerance are divided as well. Every edge is still
    /// divided once, for both faces it bounds, so neighbouring faces keep sharing its points.
    ///
    /// # Panics
    ///
    /// `deflection.chord` must be more than `TOLERANCE`.
    fn robust_triangulation_within(&self, deflection: Deflection) -> Self::MeshedShape;
}

impl<C: PolylineableCurve, S: RobustMeshableSurface> RoundMeshableShape for Shell<Point3, C, S> {
    type MeshedShape = Shell<Point3, PolylineCurve, Option<PolygonMesh>>;
    fn robust_triangulation_within(&self, deflection: Deflection) -> Self::MeshedShape {
        nonpositive_tolerance!(deflection.chord);
        #[cfg(not(target_arch = "wasm32"))]
        let res = triangulation::shell_tessellation(
            self,
            deflection,
            triangulation::by_search_nearest_parameter,
        );
        #[cfg(target_arch = "wasm32")]
        let res = triangulation::shell_tessellation_single_thread(
            self,
            deflection,
            triangulation::by_search_nearest_parameter,
        );
        res
    }
}

impl<C: PolylineableCurve, S: MeshableSurface> MeshableShape for Solid<Point3, C, S> {
    type MeshedShape = Solid<Point3, PolylineCurve, Option<PolygonMesh>>;
    fn triangulation(&self, tol: f64) -> Self::MeshedShape {
        let boundaries = self
            .boundaries()
            .iter()
            .map(|shell| shell.triangulation(tol))
            .collect::<Vec<_>>();
        Solid::new(boundaries)
    }
}

impl<C: PolylineableCurve, S: RobustMeshableSurface> RobustMeshableShape for Solid<Point3, C, S> {
    type MeshedShape = Solid<Point3, PolylineCurve, Option<PolygonMesh>>;
    fn robust_triangulation(&self, tol: f64) -> Self::MeshedShape {
        let boundaries = self
            .boundaries()
            .iter()
            .map(|shell| shell.robust_triangulation(tol))
            .collect::<Vec<_>>();
        Solid::new(boundaries)
    }
}

impl<C: PolylineableCurve, S: MeshableSurface> MeshableShape for CompressedShell<Point3, C, S> {
    type MeshedShape = CompressedShell<Point3, PolylineCurve, Option<PolygonMesh>>;
    fn triangulation(&self, tol: f64) -> Self::MeshedShape {
        nonpositive_tolerance!(tol);
        triangulation::cshell_tessellation(self, tol, triangulation::by_search_parameter)
    }
}

impl<C: PolylineableCurve, S: RobustMeshableSurface> RobustMeshableShape
    for CompressedShell<Point3, C, S>
{
    type MeshedShape = CompressedShell<Point3, PolylineCurve, Option<PolygonMesh>>;
    fn robust_triangulation(&self, tol: f64) -> Self::MeshedShape {
        nonpositive_tolerance!(tol);
        triangulation::cshell_tessellation(self, tol, triangulation::by_search_nearest_parameter)
    }
}

impl<C: PolylineableCurve, S: MeshableSurface> MeshableShape for CompressedSolid<Point3, C, S> {
    type MeshedShape = CompressedSolid<Point3, PolylineCurve, Option<PolygonMesh>>;
    fn triangulation(&self, tol: f64) -> Self::MeshedShape {
        let boundaries = self
            .boundaries
            .iter()
            .map(|shell| shell.triangulation(tol))
            .collect::<Vec<_>>();
        CompressedSolid { boundaries }
    }
}

impl<C: PolylineableCurve, S: RobustMeshableSurface> RobustMeshableShape
    for CompressedSolid<Point3, C, S>
{
    type MeshedShape = CompressedSolid<Point3, PolylineCurve, Option<PolygonMesh>>;
    fn robust_triangulation(&self, tol: f64) -> Self::MeshedShape {
        let boundaries = self
            .boundaries
            .iter()
            .map(|shell| shell.robust_triangulation(tol))
            .collect::<Vec<_>>();
        CompressedSolid { boundaries }
    }
}

mod closed;
mod triangulation;
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use triangulation::include_test_hooks;
